//! The warm builder: a python server that imports build123d and matplotlib
//! once and forks per build, so a model.py run starts at the geometry
//! instead of the 2-3 s import phase (validated by the 2026-11 spike:
//! consecutive forks produce identical results, no OCCT/OpenBLAS fork
//! hazards, the whole import cost disappears from every run).
//!
//! Layout: [`WarmBuilder::start`] spawns the embedded `warm_server.py`
//! under the project interpreter with the same tool environment as a cold
//! build and waits for its READY line; [`WarmBuilder::run`] connects to
//! its unix socket, sends `RUN <root>`, and translates the socket's
//! tagged-line protocol back into the exact shape a cold
//! [`crate::python::model::run_model`] spawn produces — a
//! `RunningProcess` feeding the same [`BuildHandle`] driver — so
//! streaming, failure classification and the SIGINT→SIGTERM cancel path
//! are the cold path's, not a parallel implementation.
//!
//! Protocol records (newline-terminated): `P <pid>` (the fork child),
//! `O <line>` / `E <line>` (its stdout/stderr), `X <waitpid status>` (the
//! raw wait status, so `ExitStatusExt::from_raw` reproduces the exact
//! exit the cold path's `child.wait()` would see). A socket that closes
//! without `X` — the server died mid-build — classifies as a failed
//! build, and the child is unregistered from the process registry either
//! way (a warm child registers like a spawned one, so the launch-time
//! orphan reap covers it: the reap TERMs the orphaned server and its
//! build child alike).
//!
//! Failure policy: every error here is the caller's signal to fall back
//! to a cold spawn — the warm path must never make a build LESS likely to
//! run. [`WarmBuilder`] is cheap to drop: dropping it SIGTERMs the server
//! and removes its socket.

use std::io;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};

use crate::processes::{
    RunningProcess, register_at, registry_path, spawn_streaming, terminate_pid, unregister_at,
    tool_environment,
};
use crate::python::model::{BuildHandle, from_running_process};

/// The embedded server (see the module docs): written into the builder's
/// work directory at start, run as `<interpreter> warm_server.py <socket>
/// build123d matplotlib`.
const SERVER_SCRIPT: &str = include_str!("warm_server.py");

/// The modules pre-imported by the production server. Matplotlib is
/// special-cased to its Agg backend, as the tool environment's MPLBACKEND
/// already demands.
const WARM_MODULES: [&str; 2] = ["build123d", "matplotlib"];

/// How long the server may take to reach READY — imports are ~2.5 s warm
/// cache, ~12 s on a cold morning disk, so this bounds a bad day.
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Connecting and the first record (`P <pid>`) must be quick: the server
/// is already up, so anything past this window is a wedged server.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// A started warm-builder server. Clone-cheap (one shared interior); the
/// LAST clone dropping SIGTERMs the server and removes its socket.
#[derive(Clone)]
pub struct WarmBuilder {
    inner: Arc<Server>,
}

struct Server {
    socket: PathBuf,
    pid: u32,
}

impl Drop for Server {
    fn drop(&mut self) {
        terminate_pid(self.pid);
        let _ = std::fs::remove_file(&self.socket);
    }
}

impl WarmBuilder {
    /// Spawn the production server: build123d and matplotlib pre-imported.
    /// Must be called inside a tokio runtime.
    pub async fn start(interpreter: &Path, work_dir: &Path) -> io::Result<WarmBuilder> {
        Self::start_with_modules(interpreter, work_dir, &WARM_MODULES).await
    }

    /// Spawn the server importing only `modules` — the test seam (a bare
    /// interpreter with no modules speaks the same fork/pipe protocol, so
    /// the machinery is testable without the venv).
    pub async fn start_with_modules(
        interpreter: &Path,
        work_dir: &Path,
        modules: &[&str],
    ) -> io::Result<WarmBuilder> {
        std::fs::create_dir_all(work_dir)?;
        let script = work_dir.join("warm_server.py");
        std::fs::write(&script, SERVER_SCRIPT)?;
        let socket = work_dir.join("warm.sock");
        let _ = std::fs::remove_file(&socket);

        let mut argv = vec![
            interpreter.to_string_lossy().into_owned(),
            script.to_string_lossy().into_owned(),
            socket.to_string_lossy().into_owned(),
        ];
        argv.extend(modules.iter().map(|m| (*m).to_string()));
        let mut proc = spawn_streaming(&argv, work_dir, &tool_environment())?;
        // READY: the first content line the server prints. Anything else
        // first is a failure the server already named.
        let ready = tokio::time::timeout(READY_TIMEOUT, async {
            while let Some(line) = proc.lines.recv().await {
                match line {
                    crate::processes::Line::Stdout(text) if text.trim() == "READY" => {
                        return true;
                    }
                    crate::processes::Line::Stdout(_) => continue,
                    crate::processes::Line::Stderr(_) => return false,
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        if !ready {
            proc.interrupt();
            return Err(io::Error::other(
                "warm builder never reached READY (import failure or timeout)",
            ));
        }
        Ok(WarmBuilder {
            inner: Arc::new(Server {
                socket,
                pid: proc.pid,
            }),
        })
    }

    /// The server's pid (the fork parent), for `ps`-style debugging.
    pub fn pid(&self) -> u32 {
        self.inner.pid
    }

    /// Whether the server still answers — a connect probe, the same
    /// operation [`WarmBuilder::run`] opens with, so a false positive
    /// here is immediately re-discovered as a run error.
    pub fn alive(&self) -> bool {
        std::os::unix::net::UnixStream::connect(&self.inner.socket).is_ok()
    }

    /// Run one build through the warm server, returning the same handle
    /// shape a cold [`crate::python::model::run_model`] gives: every
    /// output line as it arrives, then the classified outcome. Only the
    /// handshake runs inline (a connect and one line on a socket the
    /// server already holds open); the stream continues on the tokio
    /// runtime, so this is safe on the UI thread. An error is the
    /// caller's signal to fall back to a cold spawn.
    pub fn run(&self, project_root: &Path) -> io::Result<BuildHandle> {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream as StdUnixStream;

        let mut stream = StdUnixStream::connect(&self.inner.socket)?;
        stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        stream.write_all(format!("RUN {}\n", project_root.display()).as_bytes())?;
        let pid = BufReader::new(&mut stream)
            .lines()
            .next()
            .transpose()
            .ok()
            .flatten()
            .and_then(|line| line.strip_prefix("P ")?.trim().parse().ok())
            .ok_or_else(|| io::Error::other("warm builder closed before the child pid"))?;
        register_at(&registry_path(), pid);

        let (lines_tx, lines) = mpsc::channel(256);
        let (stderr_tx, stderr_text) = oneshot::channel();
        let (exit_tx, exit_status) = oneshot::channel();
        stream.set_nonblocking(true)?;
        let stream = UnixStream::from_std(stream)?;
        tokio::spawn(forward(stream, pid, lines_tx, stderr_tx, exit_tx));
        Ok(from_running_process(RunningProcess {
            pid,
            lines,
            stderr_text,
            exit_status,
        }))
    }
}

/// Translate the socket's tagged-line protocol into the RunningProcess
/// channels: `O`/`E` lines forward as they arrive (accumulating the
/// stderr text the failure classification prefers), `X` resolves the exit
/// status. A close without `X` — the server died mid-build — resolves as
/// a failed exit, never a hang: the driver's `exit_status` await must
/// always complete.
async fn forward(
    mut stream: UnixStream,
    pid: u32,
    lines: mpsc::Sender<crate::processes::Line>,
    stderr_text: oneshot::Sender<String>,
    exit_status: oneshot::Sender<ExitStatus>,
) {
    use crate::processes::Line;

    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut stderr_all = String::new();
    let mut exit: Option<ExitStatus> = None;
    'read: loop {
        let read = match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break 'read,
            Ok(read) => read,
        };
        buffer.extend_from_slice(&chunk[..read]);
        // every record is one newline-terminated line, so the buffer is
        // split exhaustively; a partial record waits for more bytes
        while let Some(nl) = buffer.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = buffer.drain(..=nl).collect();
            let text = decode(&raw);
            let (tag, rest) = text.split_at(2.min(text.len()));
            match tag {
                "O " => {
                    let _ = lines.send(Line::Stdout(rest.to_string())).await;
                }
                "E " => {
                    stderr_all.push_str(rest);
                    stderr_all.push('\n');
                    let _ = lines.send(Line::Stderr(rest.to_string())).await;
                }
                "X " => {
                    let raw_status: i32 = rest.trim().parse().unwrap_or(1 << 8);
                    exit = Some(ExitStatus::from_raw(raw_status));
                    break 'read;
                }
                _ => {} // unknown record: ignored, forward nothing
            }
        }
    }
    unregister_at(&registry_path(), pid);
    let _ = exit_status.send(exit.unwrap_or_else(|| ExitStatus::from_raw(1 << 8)));
    let _ = stderr_text.send(stderr_all);
    // lines_tx dies here: the driver's Line(None) arrives after every
    // forwarded line, the same ordering the cold spawner guarantees
}

/// Lossy UTF-8 decode of one record line, newline and CR stripped — the
/// cold path's `decode_line` semantics.
fn decode(buf: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(buf).into_owned();
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    text
}
