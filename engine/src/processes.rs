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
//! - The registry persists one `pid:owner:starttime` line per live child,
//!   where owner is the child's parent at registration and starttime is
//!   `ps -o lstart=` — the portable shape of upstream's `pbi_start_tvsec`
//!   (and of `/proc/<pid>/stat`'s starttime on Linux). A pid whose current
//!   start time differs from its line is a recycled pid, not our child: it
//!   is dropped, never signalled and never healed to the pid's new
//!   occupant. A child whose parent is no longer its owner is an orphan,
//!   whoever adopted it. After a sweep the entries that still resolve and
//!   were not signalled stay listed, where upstream persists `[]`
//!   (ProcessRunner.swift:157): the file is shared by every running
//!   instance, and a child this process cancelled that outlives the app
//!   stays known to a later launch's reap. Every rewrite holds an advisory
//!   lock on a sibling `.lock` file, so concurrent instances never lose
//!   each other's entries.
//! - Signals and process info go through `/bin/kill` and `/bin/ps` via
//!   [`std::process::Command`] — no libc/nix dependency, and exactly what
//!   reaches the child stays visible under `ps` while debugging. Unix only.
//!
//! Spawn registers the pid and process exit unregisters it, mirroring
//! upstream `ProcessRunner.run`'s insert/defer-remove; the launch-time
//! orphan reap (`CADStudioApp.swift:41`) is app wiring.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// Selects the registry file, for tests and embedders that must not touch
/// the shared default (`data_dir/running-processes`).
const ENV_OVERRIDE: &str = "CAD_STUDIO_RUNNING_PROCESSES";

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

/// Kill the orphans the registry still lists (see [`orphaned`]) whose start
/// time still matches the registered one, then rewrite the registry to the
/// entries that still resolve with an unchanged start time and were not
/// signalled. Returns the pids that were signalled.
pub fn reap_orphans() -> Vec<u32> {
    let path = registry_path();
    reap_orphans_at(&path)
}

/// SIGTERM every registered pid this process owns — its own children and
/// orphans — then rewrite the registry to the entries a different live
/// process still parents (upstream `RunningProcesses.terminateAll`, run on
/// SIGTERM and at app termination). The registry file is per user, not per
/// instance: a second running app keeps its agent, build and python
/// children across this one's quit. Returns the pids that were signalled.
pub fn terminate_all() -> Vec<u32> {
    let path = registry_path();
    terminate_all_at(&path)
}

/// [`terminate_all`] against an explicit registry file.
pub fn terminate_all_at(path: &Path) -> Vec<u32> {
    let me = std::process::id();
    sweep(path, |entry, ppid| ppid == me || orphaned(entry, ppid))
}

/// [`reap_orphans`] against an explicit registry file. A missing file has
/// nothing to reap and is left uncreated; a present file is rewritten to
/// the still-resolving, start-time-matching, un-signalled entries.
pub fn reap_orphans_at(path: &Path) -> Vec<u32> {
    sweep(path, orphaned)
}

/// Register a live pid, recording its current parent as its owner. A line
/// already listing the pid is replaced. Other lines stay verbatim: their
/// start times are never re-derived, so a recycled pid cannot be healed to
/// its new occupant here.
pub fn register_at(path: &Path, pid: u32) {
    let Some((owner, started)) = ps(pid) else {
        return;
    };
    let _lock = lock(path);
    let mut entries = entries(path);
    entries.retain(|entry| entry.pid != pid);
    entries.push(Entry {
        pid,
        owner: Some(owner),
        started,
    });
    write(path, &entries);
}

/// Drop a pid from its registry. No rewrite happens when the pid is not
/// listed, so unregistering an already-forgotten child is a no-op.
fn unregister_at(path: &Path, pid: u32) {
    let _lock = lock(path);
    let mut entries = entries(path);
    let before = entries.len();
    entries.retain(|entry| entry.pid != pid);
    if entries.len() != before {
        write(path, &entries);
    }
}

/// One registry line: `pid:owner:starttime`, where `owner` is the pid's
/// parent when it was registered. Lines without the owner field
/// (`pid:starttime`) parse with `owner: None`.
struct Entry {
    pid: u32,
    owner: Option<u32>,
    started: String,
}

impl Entry {
    fn parse(line: &str) -> Option<Entry> {
        let (pid, rest) = line.split_once(':')?;
        let pid = pid.trim().parse().ok()?;
        // starttime itself holds colons (`Wed Nov  4 10:22:01 2026`), so only
        // an all-digit field before the first one is an owner
        let (owner, started) = match rest.split_once(':') {
            Some((owner, started)) if owner.parse::<u32>().is_ok() => (owner.parse().ok(), started),
            _ => (None, rest),
        };
        Some(Entry {
            pid,
            owner,
            started: started.to_string(),
        })
    }

    fn line(&self) -> String {
        match self.owner {
            Some(owner) => format!("{}:{owner}:{}", self.pid, self.started),
            None => format!("{}:{}", self.pid, self.started),
        }
    }
}

/// A registered pid whose owner is gone. A dead owner's children reparent
/// to init or, under `systemd --user` and other child subreapers, to the
/// subreaper, so the test is "the parent is no longer the owner", with
/// `ppid == 1` covering lines that carry no owner. The reaper that adopts
/// an orphan was alive alongside the dead owner, so it can never hold the
/// owner's pid.
fn orphaned(entry: &Entry, ppid: u32) -> bool {
    ppid == 1 || entry.owner.is_some_and(|owner| owner != ppid)
}

/// The one read-check-rewrite pass behind [`reap_orphans_at`] and
/// [`terminate_all_at`]: each line whose pid still resolves with its
/// registered start time is signalled when `kill(entry, ppid)` says so and
/// kept verbatim otherwise. Dead pids and recycled pids (start time
/// changed) drop without a signal, never healed to the pid's new occupant,
/// which a later sweep would then adopt and kill. A missing file is left
/// uncreated.
fn sweep(path: &Path, kill: impl Fn(&Entry, u32) -> bool) -> Vec<u32> {
    let _lock = lock(path);
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut killed = Vec::new();
    let mut kept = Vec::new();
    for entry in text.lines().filter_map(Entry::parse) {
        let Some((ppid, starts_now)) = ps(entry.pid) else {
            continue;
        };
        if starts_now != entry.started {
            continue;
        }
        if kill(&entry, ppid) {
            let _ = signal(entry.pid, "-TERM");
            killed.push(entry.pid);
        } else {
            kept.push(entry);
        }
    }
    write(path, &kept);
    killed
}

/// The registry's lines, unparseable ones skipped.
fn entries(path: &Path) -> Vec<Entry> {
    fs::read_to_string(path)
        .map(|text| text.lines().filter_map(Entry::parse).collect())
        .unwrap_or_default()
}

/// Rewrite the registry to exactly these entries.
fn write(path: &Path, entries: &[Entry]) {
    let lines: Vec<String> = entries.iter().map(Entry::line).collect();
    // best effort: a registry that cannot be written only weakens the next
    // launch's reap
    let _ = crate::storage::atomic_write(path, lines.join("\n").as_bytes());
}

/// An exclusive advisory lock on `<registry>.lock`, held until the returned
/// file drops. Every read-modify-write takes it: the registry is shared by
/// every running instance (and by threads within one, since each call opens
/// its own file description), and without it one writer's rewrite erases
/// another's concurrent entry. The lock lives on a sibling because
/// [`crate::storage::atomic_write`] replaces the registry's inode. `None`
/// when the lock cannot be taken: the registry is best effort, so the
/// caller proceeds unlocked.
fn lock(path: &Path) -> Option<fs::File> {
    let mut name = path.as_os_str().to_owned();
    name.push(".lock");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(PathBuf::from(name))
        .ok()?;
    file.lock().ok()?;
    Some(file)
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
