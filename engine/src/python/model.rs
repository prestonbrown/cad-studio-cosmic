//! The model.py runner (ModelBuilder.swift + Project.swift's
//! buildDidFinish classification): run the project's interpreter on
//! `model.py` in the project root under the tool environment, stream both
//! outputs as they arrive, and classify the exit into the outcome the app
//! renders.
//!
//! GUI-free by the spec's engine rule: the outcome carries the classified
//! facts and the app owns the display strings — a `Failed` whose `last_line`
//! is `None` maps to the `errore-nello-script` catalog ("Script error"),
//! `Interrupted` to `compilazione-interrotta` ("Build stopped"), and the
//! failure text rides the chat row's detail (Project.swift:393-415).

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, watch};

use crate::agent::locator::shell_env;
use crate::processes::{Line, RunningProcess, spawn_streaming};

/// ModelBuilder.toolEnvironment (ModelBuilder.swift:13-19): the login-shell
/// environment every tool subprocess runs under, plus the headless
/// matplotlib backend (a model.py that plots must never open a window) and
/// unbuffered Python output so lines stream as they print.
pub fn tool_environment() -> Vec<(String, String)> {
    let mut env = shell_env();
    env.insert("MPLBACKEND".to_string(), "Agg".to_string());
    env.insert("PYTHONUNBUFFERED".to_string(), "1".to_string());
    env.into_iter().collect()
}

/// How a build ended. `Failed` carries the failure detail — `output` is the
/// failure text and `last_line` its last line, the `buildError` the app
/// shows; `None` when the text has no content, which the app maps to the
/// "Script error" catalog string (Project.swift:403-405).
#[derive(Debug, Clone, PartialEq)]
pub enum BuildOutcome {
    Success,
    Failed {
        last_line: Option<String>,
        output: String,
    },
    Interrupted,
}

/// The failure detail (Project.swift:403-404): stderr when the process wrote
/// any, else stdout, trimmed. The emptiness check is on the raw stderr — a
/// stderr of only blank lines still wins and trims to empty, matching
/// upstream's `errorOutput.isEmpty` test.
pub fn failure_text(stderr: &str, stdout: &str) -> String {
    if stderr.is_empty() { stdout } else { stderr }
        .trim()
        .to_string()
}

/// The failure text's last line, upstream's `buildError`
/// (Project.swift:405: `split(separator: "\n").last`): Swift's split omits
/// empty subsequences, so trailing blank lines never win. `None` when no
/// line has content — the app's "Script error" fallback.
pub fn last_line(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Driver events: every output line as it arrives (upstream streams both
/// stdout and stderr and displays them only in the failure sheet), then the
/// terminal classification. No further events follow `Finished`.
#[derive(Debug, Clone, PartialEq)]
pub enum BuildEvent {
    Line(String),
    Finished(BuildOutcome),
}

/// A running build: its event stream plus the cancel switch. Dropping the
/// handle drops the event receiver; the build itself runs to completion
/// either way (the child's pipes are drained by the spawner).
pub struct BuildHandle {
    pub events: mpsc::Receiver<BuildEvent>,
    cancel: watch::Sender<bool>,
    pid: Arc<Mutex<Option<u32>>>,
}

impl BuildHandle {
    /// Request interruption: SIGINT now, SIGTERM after 3 s. Idempotent.
    pub fn cancel(&self) {
        let _ = self.cancel.send(true);
    }

    /// The spawned child's pid — the child is up before the handle exists,
    /// so this is always `Some` for a live build.
    pub fn pid(&self) -> Option<u32> {
        *self.pid.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Runs the project's model.py (ModelBuilder.build:4-11): argv
/// `<interpreter> model.py`, cwd `project_root`, environment
/// [`tool_environment`], stdin null — all but the argv fixed by
/// [`spawn_streaming`]. Returns once the child is up — must be called
/// inside a tokio runtime, and the events arrive on the handle's channel.
/// A spawn failure (no interpreter at the path) is the caller's `Err`, the
/// upstream `catch` that sets a build error without a process.
pub fn run_model(interpreter: &Path, project_root: &Path) -> io::Result<BuildHandle> {
    let argv = [
        interpreter.to_string_lossy().into_owned(),
        "model.py".to_string(),
    ];
    let proc = spawn_streaming(&argv, project_root, &tool_environment())?;
    let pid = Arc::new(Mutex::new(Some(proc.pid)));
    let (events, rx) = mpsc::channel(256);
    let (cancel, cancel_rx) = watch::channel(false);
    tokio::spawn(drive_build(proc, events, cancel_rx));
    Ok(BuildHandle {
        events: rx,
        cancel,
        pid,
    })
}

/// The build's driver: forward every output line, then classify the exit
/// (Project.swift:393-415) — a cancel observed at exit is `Interrupted`
/// whatever the code, else the exit status decides between `Success` and
/// `Failed` with the failure text and its last line.
async fn drive_build(
    mut proc: RunningProcess,
    events: mpsc::Sender<BuildEvent>,
    mut cancel: watch::Receiver<bool>,
) {
    let mut stdout_text = String::new();
    let mut stderr_text = String::new();
    // One branch per source: an output line to forward, or a cancel signal.
    // The cancel arm only signals; proc is touched after the select's
    // futures are gone, so the borrows never overlap.
    enum Step {
        Line(Option<Line>),
        Cancel,
        CancelChannelClosed,
    }
    let mut cancel_alive = true;
    loop {
        let step = tokio::select! {
            line = proc.lines.recv() => Step::Line(line),
            changed = cancel.changed(), if cancel_alive => match changed {
                Ok(()) => Step::Cancel,
                Err(_) => Step::CancelChannelClosed,
            },
        };
        match step {
            Step::Cancel => {
                if *cancel.borrow_and_update() {
                    proc.interrupt();
                }
            }
            // the handle is gone: no further cancel can arrive, stop polling
            Step::CancelChannelClosed => cancel_alive = false,
            Step::Line(Some(line)) => {
                let text = match line {
                    Line::Stdout(text) => {
                        stdout_text.push_str(&text);
                        stdout_text.push('\n');
                        text
                    }
                    Line::Stderr(text) => {
                        stderr_text.push_str(&text);
                        stderr_text.push('\n');
                        text
                    }
                };
                let _ = events.send(BuildEvent::Line(text)).await;
            }
            Step::Line(None) => break,
        }
    }
    // the lines channel closes only after both pipes drain AND the child is
    // reaped, so the status is already decided here; a status that still
    // never arrives (the spawner's reader task died) is not success
    let RunningProcess { exit_status, .. } = proc;
    let succeeded = exit_status
        .await
        .map(|status| status.success())
        .unwrap_or(false);
    let outcome = if *cancel.borrow() {
        BuildOutcome::Interrupted
    } else if succeeded {
        BuildOutcome::Success
    } else {
        let output = failure_text(&stderr_text, &stdout_text);
        let last = last_line(&output);
        BuildOutcome::Failed {
            last_line: last,
            output,
        }
    };
    let _ = events.send(BuildEvent::Finished(outcome)).await;
}
