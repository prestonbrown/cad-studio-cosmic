//! The UI drive harness: a script of app messages, parsed from a file and
//! streamed into the update loop with delays — UI smoke-driving without
//! synthetic input (COSMIC 1.6 gates injected pointer/keyboard events
//! per connection, making xdotool-style automation unusable).
//!
//! Enabled only when `CAD_STUDIO_DRIVE` names a script file at launch;
//! unset, the app is untouched. Lines are `<command> [args…]`; `#` starts
//! a comment. The commands are the handful of user actions the smoke
//! flow needs, each mapping to exactly the Message a real interaction
//! would produce:
//!
//! ```text
//! setup Continue                  # the first-run sheet's Continue
//! setup PrinterModel <text>       # type into the model field
//! welcome NewProject              # the Create New Project button
//! sheet Name <text…>              # type a project name
//! sheet Advance                   # the sheet's Next ( Return)
//! sheet Cancel                    # the sheet's Cancel
//! parent <dir>                    # the folder portal answering <dir>
//! saveas <path>                   # the export save portal answering
//!                                 # <path> (the portal needs a user; the
//!                                 # zip + activity path downstream is real)
//! recent <path>                   # click a recents row
//! chat <text…>                    # stage the text as the chat draft and
//!                                 # SEND it through the real shell path
//!                                 # (a keyboard-free user turn)
//! versioncard                     # toggle the project window's version-
//!                                 # history card open/closed — the
//!                                 # toolbar clock button's message (the
//!                                 # frame pass's card capture)
//! build                           # the toolbar's Build Model button
//! viewer <toggle>                 # a viewer toggle on the project window:
//!                                 # Parameters (the overlay), Section,
//!                                 # Animation, PartsCollapsed (the Parts
//!                                 # card's chevron), Playback (play/pause),
//!                                 # Scrub <seconds> (the motion scrubber)
//! python Close                    # the python setup sheet's Close
//! agent Close                     # the agent-missing sheet's Close
//! settings <tab name>             # open (or focus) the settings window
//!                                 # at Generale/Agente/Python/Avanzate —
//!                                 # the ⌘, route without a keyboard
//!                                 # (the :99 frame pass's per-tab capture)
//! about                           # open the About window
//! restore <n>                     # the version card's Ripristina on
//!                                 # version n
//! layout <Assembled|Exploded|Plate> # the parts overlay's layout picker
//! cancel                          # the toolbar's Stop button
//! attach <path>                   # add a file to the chat draft's
//!                                 # attachments, as a drop does
//! viewer Compare <n>              # the parameters overlay's compare
//!                                 # picker on version n
//! param <row> <value|*factor>     # commit the row-th parameter (1-based,
//!                                 # the overlay's order) to a value, or
//!                                 # scale its current value by a factor
//! waitturn <secs>                 # wait until the project window's turn
//!                                 # (or build) is over; past <secs> the
//!                                 # script fails and stops
//! mark <label>                    # log a capture point for the runner
//!                                 # (scripts/rig/e2e/run.sh), with the
//!                                 # project's activity kind
//!                                 # (idle/working/succeeded/failed)
//! noop                            # a no-op message (forces a view pass)
//! sleep <ms>                      # pause before the next line
//! ```
//!
//! Every log line goes to stderr as `<HH:MM:SS.mmm> CAD_STUDIO_DRIVE: …`;
//! a failed script logs `FAIL …` and a finished one logs `DONE`.
//!
//! Parsing is pure and unit-tested; `startup_task` is the only impure
//! seam (env + file read), called once from `init`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cosmic::Task;
use engine::settings::PartsLayout;

use crate::activity::Activity;
use crate::app::Message;
use crate::sheets;
use crate::viewer_pane;
use crate::windows::{project, welcome};

/// One parsed script line: a pause, a message to feed the app, a wait for
/// the project's turn, or a capture point.
#[derive(Debug, Clone)]
pub enum Step {
    Sleep(Duration),
    Msg(Message),
    WaitTurn(Duration),
    Mark(String),
}

/// A `param` line's target value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamValue {
    To(f64),
    Times(f64),
}

impl ParamValue {
    /// The committed value given the row's current one.
    pub fn apply(self, current: f64) -> f64 {
        match self {
            ParamValue::To(value) => value,
            ParamValue::Times(factor) => current * factor,
        }
    }
}

/// How long a project must have been idle before `waitturn` trusts it: a
/// finished turn can hand straight over to a build.
const SETTLE: Duration = Duration::from_millis(1500);
/// How long `waitturn` waits for a just-sent turn to show up as busy before
/// taking an idle project as "nothing started" (a refused send).
const START_GRACE: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(100);

/// The app's side of `waitturn` and `mark`: the project window's activity
/// kind, published after every update, and how many busy-to-idle edges
/// (finished turns or builds) it has seen.
#[derive(Debug, Default)]
pub struct TurnProbe {
    kind: AtomicU8,
    finished: AtomicU64,
}

const IDLE: u8 = 0;
const WORKING: u8 = 1;
const SUCCEEDED: u8 = 2;
const FAILED: u8 = 3;

impl TurnProbe {
    pub fn publish(&self, activity: &Activity) {
        let kind = match activity {
            Activity::Idle => IDLE,
            Activity::Working(_) => WORKING,
            Activity::Succeeded(_) => SUCCEEDED,
            Activity::Failed(_) => FAILED,
        };
        if self.kind.swap(kind, Ordering::SeqCst) == WORKING && kind != WORKING {
            self.finished.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub fn busy(&self) -> bool {
        self.kind.load(Ordering::SeqCst) == WORKING
    }

    /// The activity kind as a `mark` line logs it.
    pub fn activity(&self) -> &'static str {
        match self.kind.load(Ordering::SeqCst) {
            WORKING => "working",
            SUCCEEDED => "succeeded",
            FAILED => "failed",
            _ => "idle",
        }
    }

    pub fn finished(&self) -> u64 {
        self.finished.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitVerdict {
    Wait,
    Idle,
    TimedOut,
}

/// One `waitturn` poll: `turn_ended` is whether a busy-to-idle edge
/// happened since the wait began, `idle_for` how long the project has been
/// continuously idle as the waiter saw it.
fn wait_verdict(
    elapsed: Duration,
    timeout: Duration,
    busy: bool,
    turn_ended: bool,
    idle_for: Duration,
) -> WaitVerdict {
    if !busy && idle_for >= SETTLE && (turn_ended || elapsed >= START_GRACE) {
        WaitVerdict::Idle
    } else if elapsed >= timeout {
        WaitVerdict::TimedOut
    } else {
        WaitVerdict::Wait
    }
}

/// Poll the probe until [`wait_verdict`] settles; `Ok` carries the wait.
async fn wait_turn(probe: &TurnProbe, timeout: Duration) -> Result<Duration, Duration> {
    let start = Instant::now();
    let before = probe.finished();
    let mut idle_since = (!probe.busy()).then_some(start);
    loop {
        let now = Instant::now();
        let busy = probe.busy();
        idle_since = match (busy, idle_since) {
            (true, _) => None,
            (false, None) => Some(now),
            (false, since) => since,
        };
        let idle_for = idle_since.map_or(Duration::ZERO, |since| now - since);
        let elapsed = now - start;
        match wait_verdict(elapsed, timeout, busy, probe.finished() > before, idle_for) {
            WaitVerdict::Idle => return Ok(elapsed),
            WaitVerdict::TimedOut => return Err(elapsed),
            WaitVerdict::Wait => tokio::time::sleep(POLL).await,
        }
    }
}

/// A drive log line on stderr, timestamped.
pub fn log(line: &str) {
    eprintln!(
        "{} CAD_STUDIO_DRIVE: {line}",
        chrono::Local::now().format("%H:%M:%S%.3f")
    );
}

/// `<command> <arg>` lines whose argument is required.
fn required<'a>(rest: &'a str, number: usize, what: &str) -> Result<&'a str, String> {
    if rest.is_empty() {
        Err(format!("line {}: {what}", number + 1))
    } else {
        Ok(rest)
    }
}

fn project_msg(message: project::Message) -> Step {
    Step::Msg(Message::DriveProject(message))
}

/// Parse a drive script. Unknown commands are errors (a typo'd line
/// should fail the smoke run loudly, not silently no-op).
pub fn parse(script: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    for (number, raw) in script.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (command, rest) = match line.split_once(' ') {
            Some((command, rest)) => (command, rest.trim()),
            None => (line, ""),
        };
        let step = match command {
            "sleep" => {
                let ms: u64 = rest
                    .parse()
                    .map_err(|_| format!("line {}: sleep wants milliseconds", number + 1))?;
                Step::Sleep(Duration::from_millis(ms))
            }
            "setup" => {
                let (sub, text) = rest
                    .split_once(' ')
                    .map(|(c, r)| (c, r.trim()))
                    .unwrap_or((rest, ""));
                match (sub, text) {
                    ("Continue", "") => {
                        Step::Msg(Message::SetupSheet(sheets::SetupMessage::Continue))
                    }
                    ("PrinterModel", text) => Step::Msg(Message::SetupSheet(
                        sheets::SetupMessage::PrinterModel(text.to_string()),
                    )),
                    _ => return Err(format!("line {}: unknown setup command", number + 1)),
                }
            }
            "welcome" => match rest {
                "NewProject" => Step::Msg(Message::Welcome(welcome::Message::NewProject)),
                _ => return Err(format!("line {}: unknown welcome command", number + 1)),
            },
            "sheet" => {
                let (sub, text) = rest
                    .split_once(' ')
                    .map(|(c, r)| (c, r.trim()))
                    .unwrap_or((rest, ""));
                match sub {
                    "Name" => Step::Msg(Message::Welcome(welcome::Message::NewSheet(
                        welcome::new_project::Message::Name(text.to_string()),
                    ))),
                    "Advance" => Step::Msg(Message::Welcome(welcome::Message::NewSheet(
                        welcome::new_project::Message::Advance,
                    ))),
                    "Cancel" => Step::Msg(Message::Welcome(welcome::Message::NewSheet(
                        welcome::new_project::Message::Cancel,
                    ))),
                    _ => return Err(format!("line {}: unknown sheet command", number + 1)),
                }
            }
            "parent" => Step::Msg(Message::ParentFolderChosen(Ok(vec![PathBuf::from(rest)]))),
            "saveas" => {
                if rest.is_empty() {
                    return Err(format!(
                        "line {}: saveas wants a destination path",
                        number + 1
                    ));
                }
                Step::Msg(Message::DriveSaveAs(PathBuf::from(rest)))
            }
            "recent" => Step::Msg(Message::Welcome(welcome::Message::OpenRecent(
                PathBuf::from(rest),
            ))),
            "chat" => {
                if rest.is_empty() {
                    return Err(format!("line {}: chat wants text", number + 1));
                }
                Step::Msg(Message::DriveChat(rest.to_string()))
            }
            "versioncard" => Step::Msg(Message::DriveVersionCard),
            "build" => Step::Msg(Message::DriveBuild),
            "viewer" => project_msg(project::Message::Viewer(viewer_toggle(rest).ok_or_else(
                || {
                    format!(
                        "line {}: viewer wants Parameters/Section/Animation/PartsCollapsed/Playback/Scrub <s>/Compare <n>",
                        number + 1
                    )
                },
            )?)),
            "python" => match rest {
                "Close" => Step::Msg(Message::PythonSheet(sheets::PythonMessage::Close)),
                _ => return Err(format!("line {}: unknown python command", number + 1)),
            },
            "agent" => match rest {
                "Close" => Step::Msg(Message::AgentSheet(sheets::AgentMessage::Close)),
                _ => return Err(format!("line {}: unknown agent command", number + 1)),
            },
            "noop" => Step::Msg(Message::Noop),
            "settings" => {
                let tab = match rest {
                    "Generale" | "" => crate::windows::settings::Tab::General,
                    "Agente" => crate::windows::settings::Tab::Agent,
                    "Python" => crate::windows::settings::Tab::Python,
                    "Avanzate" => crate::windows::settings::Tab::Advanced,
                    _ => {
                        return Err(format!(
                            "line {}: settings wants a tab (Generale/Agente/Python/Avanzate)",
                            number + 1
                        ));
                    }
                };
                Step::Msg(Message::OpenSettings(tab))
            }
            "about" => Step::Msg(Message::OpenAbout),
            "restore" => project_msg(project::Message::Restore(
                rest.parse()
                    .map_err(|_| format!("line {}: restore wants a version number", number + 1))?,
            )),
            "layout" => {
                let layout = match rest {
                    "Assembled" => PartsLayout::Assembled,
                    "Exploded" => PartsLayout::Exploded,
                    "Plate" => PartsLayout::Plate,
                    _ => {
                        return Err(format!(
                            "line {}: layout wants Assembled/Exploded/Plate",
                            number + 1
                        ));
                    }
                };
                project_msg(project::Message::Viewer(viewer_pane::Message::SetLayout(
                    layout,
                )))
            }
            "cancel" => {
                if !rest.is_empty() {
                    return Err(format!("line {}: cancel takes no argument", number + 1));
                }
                project_msg(project::Message::Command(project::Command::Stop))
            }
            "attach" => project_msg(project::Message::Chat(
                crate::chat_view::Message::AttachContent(
                    crate::attachments::ClipboardContent::Files(vec![PathBuf::from(required(
                        rest,
                        number,
                        "attach wants a file path",
                    )?)]),
                ),
            )),
            "param" => {
                let wants = || format!("line {}: param wants <row ≥ 1> <value|*factor>", number + 1);
                let (row, value) = rest.split_once(' ').ok_or_else(wants)?;
                let row: usize = row.parse().ok().filter(|row| *row >= 1).ok_or_else(wants)?;
                let value = value.trim();
                let value = match value.strip_prefix('*') {
                    Some(factor) => ParamValue::Times(factor.parse().map_err(|_| wants())?),
                    None => ParamValue::To(value.parse().map_err(|_| wants())?),
                };
                Step::Msg(Message::DriveParameter(row, value))
            }
            "waitturn" => {
                let seconds: u64 = rest.parse().ok().filter(|s| *s > 0).ok_or_else(|| {
                    format!("line {}: waitturn wants a timeout in seconds", number + 1)
                })?;
                Step::WaitTurn(Duration::from_secs(seconds))
            }
            "mark" => Step::Mark(required(rest, number, "mark wants a label")?.to_string()),
            _ => return Err(format!("line {}: unknown command `{command}`", number + 1)),
        };
        steps.push(step);
    }
    Ok(steps)
}

/// The viewer toggle a `viewer` line names.
fn viewer_toggle(name: &str) -> Option<viewer_pane::Message> {
    Some(match name {
        "Parameters" => viewer_pane::Message::ToggleParameters,
        "Section" => viewer_pane::Message::ToggleSection,
        "Animation" => viewer_pane::Message::ToggleAnimation,
        "PartsCollapsed" => viewer_pane::Message::TogglePartsCollapsed,
        "Playback" => viewer_pane::Message::TogglePlayback,
        _ => {
            if let Some(version) = name.strip_prefix("Compare ") {
                return Some(viewer_pane::Message::Parameters(
                    crate::parameters_view::Message::Compare(Some(version.trim().parse().ok()?)),
                ));
            }
            let seconds = name.strip_prefix("Scrub ")?.trim().parse().ok()?;
            viewer_pane::Message::Scrub(seconds)
        }
    })
}

/// The `init` seam: when `CAD_STUDIO_DRIVE` names a script, stream its
/// steps into the update loop (a `sleep` line delays the next message, a
/// `waitturn` line polls the returned probe, which the app publishes into).
pub fn startup_task() -> (Task<Message>, Option<Arc<TurnProbe>>) {
    let Some(path) = std::env::var_os("CAD_STUDIO_DRIVE") else {
        return (Task::none(), None);
    };
    let script = match std::fs::read_to_string(&path) {
        Ok(script) => script,
        Err(why) => {
            log(&format!("FAIL cannot read {}: {why}", path.display()));
            return (Task::none(), None);
        }
    };
    let steps = match parse(&script) {
        Ok(steps) => steps,
        Err(why) => {
            log(&format!("FAIL {why}"));
            return (Task::none(), None);
        }
    };
    let probe = Arc::new(TurnProbe::default());
    let waiter = probe.clone();
    let task = Task::stream(cosmic::iced::stream::channel(
        16,
        async move |mut sender| {
            for step in steps {
                match step {
                    Step::Sleep(duration) => tokio::time::sleep(duration).await,
                    Step::Msg(message) => {
                        let _ = sender.try_send(message);
                    }
                    Step::WaitTurn(timeout) => match wait_turn(&waiter, timeout).await {
                        Ok(waited) => {
                            log(&format!("waitturn idle after {:.1}s", waited.as_secs_f64()))
                        }
                        Err(waited) => {
                            log(&format!(
                                "FAIL waitturn: still busy after {:.1}s (timeout {}s)",
                                waited.as_secs_f64(),
                                timeout.as_secs()
                            ));
                            return;
                        }
                    },
                    Step::Mark(label) => {
                        log(&format!("MARK {label} activity={}", waiter.activity()))
                    }
                }
            }
            log("DONE");
        },
    ));
    (task, Some(probe))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::project;
    use engine::settings::PartsLayout;

    #[test]
    fn parses_the_smoke_flow() {
        let steps = parse(
            "# first-run, then a project\n\
             setup Continue\n\
             sleep 100\n\
             welcome NewProject\n\
             sheet Name L bracket\n\
             sheet Advance\n\
             parent /tmp/smoke\n",
        )
        .unwrap();
        assert_eq!(steps.len(), 6);
        assert!(matches!(
            steps[0],
            Step::Msg(Message::SetupSheet(sheets::SetupMessage::Continue))
        ));
        assert!(matches!(steps[1], Step::Sleep(d) if d == Duration::from_millis(100)));
        assert!(matches!(
            &steps[3],
            Step::Msg(Message::Welcome(welcome::Message::NewSheet(
                welcome::new_project::Message::Name(name)
            ))) if name == "L bracket"
        ));
        assert!(matches!(
            &steps[5],
            Step::Msg(Message::ParentFolderChosen(Ok(paths))) if paths == &vec![PathBuf::from("/tmp/smoke")]
        ));
    }

    #[test]
    fn unknown_commands_are_errors() {
        assert!(parse("welcom NewProject").is_err());
        assert!(parse("sleep lots").is_err());
        assert!(parse("setup Explode").is_err());
        assert!(parse("chat").is_err(), "chat wants text");
    }

    #[test]
    fn chat_command_parses_a_full_user_turn() {
        let steps = parse("chat animate the cap screwing onto the jar\n").unwrap();
        assert!(matches!(
            &steps[0],
            Step::Msg(Message::DriveChat(text)) if text == "animate the cap screwing onto the jar"
        ));
    }

    /// `saveas` injects the SaveFile portal's answer for the export flow —
    /// the portal itself needs a user, so a scripted export never opens it;
    /// the window receives the chosen destination directly and zips from
    /// there (the real downstream path).
    #[test]
    fn saveas_command_parses_the_portal_answer() {
        let steps = parse("saveas /tmp/smoke/l-bracket.zip\n").unwrap();
        assert!(matches!(
            &steps[0],
            Step::Msg(Message::DriveSaveAs(path)) if path == &PathBuf::from("/tmp/smoke/l-bracket.zip")
        ));
        assert!(parse("saveas").is_err(), "saveas wants a destination path");
    }

    #[test]
    fn comments_and_blanks_are_skipped() {
        assert!(parse("# nothing\n\n   \n").unwrap().is_empty());
    }

    /// `settings` opens the settings window at a named tab (the frame
    /// pass's per-tab capture route — the ⌘, command without a keyboard);
    /// `about` opens the About window. Unknown tabs are errors, a bare
    /// `settings` opens Generale.
    #[test]
    fn settings_and_about_commands_parse() {
        let steps = parse("settings Agente\nabout\nsettings\n").unwrap();
        assert!(matches!(
            steps[0],
            Step::Msg(Message::OpenSettings(crate::windows::settings::Tab::Agent))
        ));
        assert!(matches!(steps[1], Step::Msg(Message::OpenAbout)));
        assert!(matches!(
            steps[2],
            Step::Msg(Message::OpenSettings(
                crate::windows::settings::Tab::General
            ))
        ));
        assert!(parse("settings Impostazioni").is_err());
    }

    /// `versioncard` flips the project window's version-history card
    /// open/closed — the toolbar clock button's message, so the frame
    /// pass can capture the card up.
    #[test]
    fn versioncard_command_toggles_the_card() {
        let steps = parse("versioncard\n").unwrap();
        assert!(matches!(steps[0], Step::Msg(Message::DriveVersionCard)));
    }

    /// `build` is the toolbar's Build Model press on the focused project
    /// window.
    #[test]
    fn build_command_presses_build() {
        let steps = parse("build\n").unwrap();
        assert!(matches!(steps[0], Step::Msg(Message::DriveBuild)));
    }

    /// `viewer <toggle>` delivers the named viewer-pane toggle; an unknown
    /// or missing toggle is an error.
    #[test]
    fn viewer_command_names_each_toggle() {
        let steps = parse(
            "viewer Parameters\nviewer Section\nviewer Animation\n\
             viewer PartsCollapsed\nviewer Playback\nviewer Scrub 0.1\n",
        )
        .unwrap();
        let toggles: Vec<_> = steps
            .iter()
            .map(|step| match step {
                Step::Msg(Message::DriveProject(project::Message::Viewer(msg))) => {
                    format!("{msg:?}")
                }
                other => panic!("not a viewer step: {other:?}"),
            })
            .collect();
        assert_eq!(
            toggles,
            [
                "ToggleParameters",
                "ToggleSection",
                "ToggleAnimation",
                "TogglePartsCollapsed",
                "TogglePlayback",
                "Scrub(0.1)"
            ]
        );
        assert!(parse("viewer Grid").is_err());
        assert!(parse("viewer").is_err());
        assert!(parse("viewer Scrub soon").is_err());
    }

    /// The arms the smoke-flow test executes but never asserts, plus
    /// `PrinterModel` and `Cancel` which no test touched at all.
    #[test]
    fn printer_model_welcome_and_cancel_arms_parse() {
        let steps = parse(
            "setup PrinterModel Bambu P1S\n\
             welcome NewProject\n\
             sheet Advance\n\
             sheet Cancel\n",
        )
        .unwrap();
        assert!(matches!(
            &steps[0],
            Step::Msg(Message::SetupSheet(sheets::SetupMessage::PrinterModel(model)))
                if model == "Bambu P1S"
        ));
        assert!(matches!(
            steps[1],
            Step::Msg(Message::Welcome(welcome::Message::NewProject))
        ));
        assert!(matches!(
            steps[2],
            Step::Msg(Message::Welcome(welcome::Message::NewSheet(
                welcome::new_project::Message::Advance
            )))
        ));
        assert!(matches!(
            steps[3],
            Step::Msg(Message::Welcome(welcome::Message::NewSheet(
                welcome::new_project::Message::Cancel
            )))
        ));
    }

    /// `restore <n>` is the version card's Ripristina for version n.
    #[test]
    fn restore_command_restores_a_version() {
        let steps = parse("restore 1\nrestore 12\n").unwrap();
        assert!(matches!(
            steps[0],
            Step::Msg(Message::DriveProject(project::Message::Restore(1)))
        ));
        assert!(matches!(
            steps[1],
            Step::Msg(Message::DriveProject(project::Message::Restore(12)))
        ));
        assert!(parse("restore x").is_err());
        assert!(parse("restore").is_err());
    }

    /// `layout <name>` is the parts overlay's layout picker.
    #[test]
    fn layout_command_names_each_layout() {
        let steps = parse("layout Assembled\nlayout Exploded\nlayout Plate\n").unwrap();
        let layouts: Vec<_> = steps
            .iter()
            .map(|step| match step {
                Step::Msg(Message::DriveProject(project::Message::Viewer(
                    viewer_pane::Message::SetLayout(layout),
                ))) => *layout,
                other => panic!("not a layout step: {other:?}"),
            })
            .collect();
        assert_eq!(
            layouts,
            [
                PartsLayout::Assembled,
                PartsLayout::Exploded,
                PartsLayout::Plate
            ]
        );
        assert!(parse("layout Foo").is_err());
        assert!(parse("layout").is_err());
    }

    /// `cancel` is the toolbar's Stop button.
    #[test]
    fn cancel_command_presses_stop() {
        let steps = parse("cancel\n").unwrap();
        assert!(matches!(
            steps[0],
            Step::Msg(Message::DriveProject(project::Message::Command(
                project::Command::Stop
            )))
        ));
        assert!(parse("cancel now").is_err());
    }

    /// `waitturn <secs>` waits for the project's turn with a timeout in
    /// whole seconds; the seconds are required.
    #[test]
    fn waitturn_command_carries_its_timeout() {
        let steps = parse("waitturn 600\n").unwrap();
        assert!(matches!(steps[0], Step::WaitTurn(d) if d == Duration::from_secs(600)));
        assert!(parse("waitturn").is_err());
        assert!(parse("waitturn soon").is_err());
        assert!(
            parse("waitturn 0").is_err(),
            "a zero timeout can never pass"
        );
    }

    /// `mark <label>` names a capture point; the label is required.
    #[test]
    fn mark_command_carries_its_label() {
        let steps = parse("mark 03-plate\n").unwrap();
        assert!(matches!(&steps[0], Step::Mark(label) if label == "03-plate"));
        assert!(parse("mark").is_err());
    }

    /// `attach <path>` adds a file to the chat draft's attachments, as a
    /// drop on the chat pane does.
    #[test]
    fn attach_command_attaches_a_file() {
        let steps = parse("attach /tmp/e2e/sketch.png\n").unwrap();
        assert!(matches!(
            &steps[0],
            Step::Msg(Message::DriveProject(project::Message::Chat(
                crate::chat_view::Message::AttachContent(
                    crate::attachments::ClipboardContent::Files(paths)
                )
            ))) if paths == &vec![PathBuf::from("/tmp/e2e/sketch.png")]
        ));
        assert!(parse("attach").is_err());
    }

    /// `viewer Compare <n>` is the parameters overlay's compare picker.
    #[test]
    fn viewer_compare_picks_a_version() {
        let steps = parse("viewer Compare 1\n").unwrap();
        assert!(matches!(
            steps[0],
            Step::Msg(Message::DriveProject(project::Message::Viewer(
                viewer_pane::Message::Parameters(crate::parameters_view::Message::Compare(Some(1)))
            )))
        ));
        assert!(parse("viewer Compare one").is_err());
    }

    /// `param <row> <value>` commits the n-th parameter row (1-based, the
    /// overlay's order) to a value, or scales it with a `*` factor.
    #[test]
    fn param_command_targets_a_row() {
        let steps = parse("param 1 25\nparam 2 *1.5\n").unwrap();
        assert!(matches!(
            steps[0],
            Step::Msg(Message::DriveParameter(1, ParamValue::To(v))) if v == 25.0
        ));
        assert!(matches!(
            steps[1],
            Step::Msg(Message::DriveParameter(2, ParamValue::Times(f))) if f == 1.5
        ));
        assert_eq!(ParamValue::To(25.0).apply(20.0), 25.0);
        assert_eq!(ParamValue::Times(1.5).apply(20.0), 30.0);
        assert!(parse("param 0 5").is_err(), "rows are 1-based");
        assert!(parse("param 1").is_err());
        assert!(parse("param x 5").is_err());
        assert!(parse("param 1 *big").is_err());
    }

    /// The real-agent end-to-end script (scripts/rig/e2e/e2e.drive) parses
    /// once run.sh renders its paths, and waits for every turn it sends.
    #[test]
    fn the_e2e_script_parses() {
        let script = include_str!("../../scripts/rig/e2e/e2e.drive")
            .replace("@PROJECT@", "/tmp/e2e/e2e-cubo")
            .replace("@PNG@", "/tmp/e2e/schizzo.png")
            .replace("@ZIP@", "/tmp/e2e/e2e-cubo.zip");
        let steps = parse(&script).unwrap();
        let chats = steps
            .iter()
            .filter(|step| matches!(step, Step::Msg(Message::DriveChat(_))))
            .count();
        let waits = steps
            .iter()
            .filter(|step| matches!(step, Step::WaitTurn(_)))
            .count();
        assert_eq!(chats, 5, "the five agent turns");
        assert!(waits >= chats, "every turn is waited for");
    }

    /// The probe counts busy-to-idle edges: each one is a finished turn
    /// (or build); repeated reports and settled-to-settled changes are not
    /// edges. It names the activity kind a `mark` logs.
    #[test]
    fn turn_probe_counts_finished_turns() {
        let working = Activity::Working("turn".into());
        let probe = TurnProbe::default();
        assert!(!probe.busy());
        assert_eq!(probe.activity(), "idle");
        probe.publish(&Activity::Idle);
        assert_eq!(probe.finished(), 0);
        probe.publish(&working);
        probe.publish(&working);
        assert!(probe.busy());
        assert_eq!(probe.activity(), "working");
        assert_eq!(probe.finished(), 0);
        probe.publish(&Activity::Succeeded("done".into()));
        probe.publish(&Activity::Failed("build".into()));
        assert!(!probe.busy());
        assert_eq!(probe.activity(), "failed");
        assert_eq!(probe.finished(), 1);
    }

    /// `waitturn` returns once the project has been idle for the settle
    /// time AND either a turn ended since the wait began or the start grace
    /// passed without one starting (a refused send never goes busy); it
    /// fails once the timeout passes first.
    #[test]
    fn wait_verdicts() {
        let s = Duration::from_secs;
        let ms = Duration::from_millis;
        let t = s(60);
        // just sent: idle, nothing ended, inside the grace — keep waiting
        assert_eq!(
            wait_verdict(ms(200), t, false, false, ms(200)),
            WaitVerdict::Wait
        );
        // the turn is running
        assert_eq!(
            wait_verdict(s(30), t, true, false, ms(0)),
            WaitVerdict::Wait
        );
        // the turn ended, but idle only briefly (a build may follow)
        assert_eq!(
            wait_verdict(s(31), t, false, true, ms(300)),
            WaitVerdict::Wait
        );
        // the turn ended and the project settled idle
        assert_eq!(
            wait_verdict(s(33), t, false, true, SETTLE),
            WaitVerdict::Idle
        );
        // nothing ever started: idle past the grace
        assert_eq!(
            wait_verdict(START_GRACE, t, false, false, START_GRACE),
            WaitVerdict::Idle
        );
        // still busy at the timeout
        assert_eq!(
            wait_verdict(t, t, true, false, ms(0)),
            WaitVerdict::TimedOut
        );
        // idle at the timeout but not yet settled after an ended turn
        assert_eq!(
            wait_verdict(t, t, false, true, ms(100)),
            WaitVerdict::TimedOut
        );
    }

    /// `recent` clicks a recents row; `python`/`agent` Close their
    /// sheets; `noop` feeds the loop a harmless ack (forces a view pass).
    #[test]
    fn recent_python_agent_and_noop_arms_parse() {
        let steps = parse(
            "recent /tmp/smoke/bracket.proj\n\
             python Close\n\
             agent Close\n\
             noop\n",
        )
        .unwrap();
        assert!(matches!(
            &steps[0],
            Step::Msg(Message::Welcome(welcome::Message::OpenRecent(path)))
                if path == &PathBuf::from("/tmp/smoke/bracket.proj")
        ));
        assert!(matches!(
            steps[1],
            Step::Msg(Message::PythonSheet(sheets::PythonMessage::Close))
        ));
        assert!(matches!(
            steps[2],
            Step::Msg(Message::AgentSheet(sheets::AgentMessage::Close))
        ));
        assert!(matches!(steps[3], Step::Msg(Message::Noop)));
    }
}
