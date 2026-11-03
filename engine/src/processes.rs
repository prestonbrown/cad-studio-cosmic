//! Process supervision: streamed child spawn with SIGINT→SIGTERM escalation
//! and a pid registry for reaping crash orphans (ProcessRunner.swift).
//!
//! - stdin is ALWAYS [`Stdio::null`]: `claude -p` reads inherited stdin and
//!   merges it into the prompt, so a piped or inherited
//!   stdin corrupts every run.
//! - [`RunningProcess::interrupt`] signals the child's pid only, never its
//!   process group, then escalates to SIGTERM after 3 s
//!   (ProcessRunner.swift:172-179). Upstream gates the escalation on the
//!   in-memory pid set; the port asks the kernel instead (`kill -0`), which
//!   tracks the same "still running" fact without shared state.
//! - The registry persists one `pid:starttime` line per live child, where
//!   starttime is `ps -o lstart=` — the portable shape of upstream's
//!   `pbi_start_tvsec` (and of `/proc/<pid>/stat`'s starttime on Linux). A
//!   pid whose current start time differs from the registry line is a
//!   recycled pid, not our orphan. After a reap the entries that still
//!   resolve with an unchanged start time and were not signalled stay
//!   listed — mismatched recycled pids are dropped, never healed to the
//!   pid's new occupant — where upstream persists `[]`
//!   (ProcessRunner.swift:157), so a child this process cancelled that
//!   outlives the app stays known to a later launch's reap.
//! - Signals and process info go through `/bin/kill` and `/bin/ps` via
//!   [`std::process::Command`] — no libc/nix dependency, and exactly what
//!   reaches the child stays visible under `ps` while debugging. Unix only.
//!
//! Spawn registers the pid and process exit unregisters it, mirroring
//! upstream `ProcessRunner.run`'s insert/defer-remove; the launch-time
//! orphan reap (`CADStudioApp.swift:41`) is app wiring.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// Selects the registry file, for tests and embedders that must not touch
/// the shared default (`data_dir/running-processes`).
const ENV_OVERRIDE: &str = "CAD_STUDIO_RUNNING_PROCESSES";

/// Serializes registry read-modify-write cycles (the in-process stand-in for
/// upstream's `Mutex`-guarded pid set).
static REGISTRY_LOCK: Mutex<()> = Mutex::new(());

/// One streamed output line, decoded lossily and stripped of its newline.
pub enum Line {
    Stdout(String),
    Stderr(String),
}

/// A spawned child whose output streams line by line. The channels close in
/// order of finality: `stderr_text` resolves when stderr reaches EOF, and
/// `exit_status` resolves once the child is reaped and its registry entry
/// removed — awaiting it never needs polling.
pub struct RunningProcess {
    pub pid: u32,
    pub lines: mpsc::Receiver<Line>,
    pub stderr_text: oneshot::Receiver<String>,
    pub exit_status: oneshot::Receiver<ExitStatus>,
}

impl RunningProcess {
    /// SIGINT now, then — if the child is still alive after 3 s — SIGTERM.
    /// Non-blocking: only the escalation waits, on a spawned task. Must be
    /// called from within a tokio runtime; the signal goes to the child's
    /// pid only, matching upstream (no process-group signaling).
    pub fn interrupt(&self) {
        let pid = self.pid;
        let _ = signal(pid, "-INT");
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if signal(pid, "-0").is_ok() {
                let _ = signal(pid, "-TERM");
            }
        });
    }
}

/// Spawn `argv` (program first) in `cwd`, streaming stdout and stderr as
/// [`Line`]s. `env` REPLACES the child's entire environment — upstream
/// assigns `Process.environment` wholesale, so callers pass a complete
/// environment (see `ModelBuilder.toolEnvironment`). stdin is always null.
pub fn spawn_streaming(
    argv: &[String],
    cwd: &Path,
    env: &[(String, String)],
) -> io::Result<RunningProcess> {
    let Some((program, args)) = argv.split_first() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "argv needs at least the program",
        ));
    };
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("child pid unavailable right after spawn"))?;
    let registry = registry_path();
    register_at(&registry, pid);

    let stdout = child.stdout.take().expect("stdout requested piped");
    let stderr = child.stderr.take().expect("stderr requested piped");
    let (lines_tx, lines) = mpsc::channel(256);
    let (stderr_tx, stderr_text) = oneshot::channel();
    let (exit_tx, exit_status) = oneshot::channel();
    tokio::spawn(async move {
        let stderr_arm = async {
            let text = drain(BufReader::new(stderr), &lines_tx, Line::Stderr).await;
            let _ = stderr_tx.send(text);
        };
        // Exit is reported when the child itself is reaped (upstream's
        // terminationHandler), NOT when the pipes drain: a grandchild that
        // inherited the pipe keeps the lines open past the child's death.
        let wait_arm = async {
            let status = child.wait().await.expect("we alone own and reap the child");
            unregister_at(&registry, pid);
            let _ = exit_tx.send(status);
        };
        let _ = tokio::join!(
            drain(BufReader::new(stdout), &lines_tx, Line::Stdout),
            stderr_arm,
            wait_arm
        );
        // lines_tx dies here, so the lines channel closes only after every
        // forwarded line; the exit status was already delivered at reap time.
    });
    Ok(RunningProcess {
        pid,
        lines,
        stderr_text,
        exit_status,
    })
}

/// ModelBuilder.toolEnvironment (ModelBuilder.swift:13-19): the login-shell
/// environment every tool subprocess runs under (model.py builds and agent
/// turns alike), plus the headless matplotlib backend (a model.py that
/// plots must never open a window) and unbuffered Python output so lines
/// stream as they print.
pub fn tool_environment() -> Vec<(String, String)> {
    let mut env = crate::agent::locator::shell_env();
    env.insert("MPLBACKEND".to_string(), "Agg".to_string());
    env.insert("PYTHONUNBUFFERED".to_string(), "1".to_string());
    env.into_iter().collect()
}

/// The registry file: [`crate::storage::data_dir`] plus `running-processes`,
/// unless `CAD_STUDIO_RUNNING_PROCESSES` points elsewhere.
pub fn registry_path() -> PathBuf {
    if let Some(path) = std::env::var_os(ENV_OVERRIDE) {
        return PathBuf::from(path);
    }
    crate::storage::data_dir().join("running-processes")
}

/// Kill processes the registry still lists whose parent is now init and
/// whose start time still matches the registered one, then rewrite the
/// registry to the entries that still resolve with an unchanged start time
/// and were not signalled. Returns the pids that were signalled.
pub fn reap_orphans() -> Vec<u32> {
    let path = registry_path();
    reap_orphans_at(&path)
}

/// Signal EVERY registered pid with SIGTERM — orphan or not — then empty the
/// registry (upstream `RunningProcesses.terminateAll`, run on SIGTERM and at
/// app termination; the reap's ppid/starttime guards are crash recovery and
/// do not apply to an orderly shutdown). Returns the pids that were
/// signalled.
pub fn terminate_all() -> Vec<u32> {
    let path = registry_path();
    terminate_all_at(&path)
}

/// [`terminate_all`] against an explicit registry file.
pub fn terminate_all_at(path: &Path) -> Vec<u32> {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let killed = registered_pids(path)
        .into_iter()
        .filter(|&pid| ps(pid).is_some())
        .map(|pid| {
            let _ = signal(pid, "-TERM");
            pid
        })
        .collect();
    persist(path, &BTreeSet::new());
    killed
}

/// [`register`] against an explicit registry file.
pub fn register_at(path: &Path, pid: u32) {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut pids = registered_pids(path);
    pids.insert(pid);
    persist(path, &pids);
}

/// [`reap_orphans`] against an explicit registry file. A missing file has
/// nothing to reap and is left uncreated; a present file is rewritten to
/// the still-resolving, start-time-matching, un-signalled entries.
pub fn reap_orphans_at(path: &Path) -> Vec<u32> {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut killed = Vec::new();
    // what survives the reap: live pids whose start time still matches their
    // line and that this pass did not signal
    let mut remaining = BTreeSet::new();
    for line in text.lines() {
        let Some((pid_text, started)) = line.split_once(':') else {
            continue;
        };
        let Ok(pid) = pid_text.trim().parse::<u32>() else {
            continue;
        };
        let Some((ppid, starts_now)) = ps(pid) else {
            continue;
        };
        if starts_now != started {
            // a recycled pid: not ours to signal and not ours to keep —
            // the line must drop, because persist re-derives start times
            // from ps and would otherwise heal it to the new occupant,
            // making a later reap adopt (and kill) that process
            continue;
        }
        if ppid == 1 {
            let _ = signal(pid, "-TERM");
            killed.push(pid);
        } else {
            remaining.insert(pid);
        }
    }
    persist(path, &remaining);
    killed
}

/// Drop a pid from its registry. No rewrite happens when the pid is not
/// listed, so unregistering an already-forgotten child is a no-op.
fn unregister_at(path: &Path, pid: u32) {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut pids = registered_pids(path);
    if pids.remove(&pid) {
        persist(path, &pids);
    }
}

/// Pids currently listed in the registry file.
fn registered_pids(path: &Path) -> BTreeSet<u32> {
    let Ok(text) = fs::read_to_string(path) else {
        return BTreeSet::new();
    };
    text.lines()
        .filter_map(|line| line.split_once(':'))
        .filter_map(|(pid, _)| pid.trim().parse().ok())
        .collect()
}

/// Rewrite the registry, one `pid:starttime` line per still-live pid, pids
/// that no longer resolve are dropped (upstream's persist does the same via
/// compactMap over proc info).
fn persist(path: &Path, pids: &BTreeSet<u32>) {
    let lines: Vec<String> = pids
        .iter()
        .filter_map(|pid| ps(*pid).map(|(_, started)| format!("{pid}:{started}")))
        .collect();
    // best effort: a registry that cannot be written only weakens the next
    // launch's reap
    let _ = crate::storage::atomic_write(path, lines.join("\n").as_bytes());
}

/// `(ppid, start time)` of a pid, via `ps`. `None` when the pid is gone or
/// `ps` cannot answer — the callers treat that as "not ours to touch".
fn ps(pid: u32) -> Option<(u32, String)> {
    let output = Command::new("/bin/ps")
        .args(["-o", "ppid=,lstart=", "-p", &pid.to_string()])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    let (ppid, started) = text.split_once(char::is_whitespace)?;
    Some((ppid.parse().ok()?, started.trim().to_string()))
}

/// Send a signal to a pid with `/bin/kill` (`-INT`, `-TERM`, `-0` probe).
fn signal(pid: u32, sig: &str) -> io::Result<()> {
    let delivered = Command::new("/bin/kill")
        .arg(sig)
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())?;
    if delivered {
        Ok(())
    } else {
        Err(io::Error::other(format!("kill {sig} {pid} did not land")))
    }
}

/// Forward one pipe's lines into the shared channel, returning the full text
/// with each line newline-terminated (upstream `readLines` accumulates the
/// same way). Reading continues even when the consumer is gone, so a dropped
/// receiver can never wedge the child on a full pipe.
async fn drain<R: AsyncBufRead + Unpin>(
    mut reader: R,
    lines: &mpsc::Sender<Line>,
    wrap: fn(String) -> Line,
) -> String {
    let mut text = String::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => return text,
            Ok(_) => {
                let line = decode_line(&buf);
                text.push_str(&line);
                text.push('\n');
                let _ = lines.send(wrap(line)).await;
            }
            Err(_) => return text,
        }
    }
}

/// Lossy UTF-8 decode of one raw line, newline (and its CR) stripped. A
/// final chunk without a newline is still a line, as upstream's LineBuffer
/// yields its remainder.
fn decode_line(buf: &[u8]) -> String {
    let mut line = String::from_utf8_lossy(buf).into_owned();
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
    line
}
