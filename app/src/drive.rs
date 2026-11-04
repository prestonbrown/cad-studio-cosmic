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
//! noop                            # a no-op message (forces a view pass)
//! sleep <ms>                      # pause before the next line
//! ```
//!
//! Parsing is pure and unit-tested; `startup_task` is the only impure
//! seam (env + file read), called once from `init`.

use std::path::PathBuf;
use std::time::Duration;

use cosmic::Task;

use crate::app::Message;
use crate::sheets;
use crate::viewer_pane;
use crate::windows::welcome;

/// One parsed script line: a pause, or a message to feed the app.
#[derive(Debug, Clone)]
pub enum Step {
    Sleep(Duration),
    Msg(Message),
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
            "viewer" => Step::Msg(Message::DriveViewer(viewer_toggle(rest).ok_or_else(
                || {
                    format!(
                        "line {}: viewer wants Parameters/Section/Animation/PartsCollapsed/Playback/Scrub <s>",
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
            let seconds = name.strip_prefix("Scrub ")?.trim().parse().ok()?;
            viewer_pane::Message::Scrub(seconds)
        }
    })
}

/// The `init` seam: when `CAD_STUDIO_DRIVE` names a script, stream its
/// steps into the update loop (a `sleep` line delays the next message).
pub fn startup_task() -> Task<Message> {
    let Some(path) = std::env::var_os("CAD_STUDIO_DRIVE") else {
        return Task::none();
    };
    let script = match std::fs::read_to_string(&path) {
        Ok(script) => script,
        Err(why) => {
            eprintln!("CAD_STUDIO_DRIVE: cannot read {}: {why}", path.display());
            return Task::none();
        }
    };
    let steps = match parse(&script) {
        Ok(steps) => steps,
        Err(why) => {
            eprintln!("CAD_STUDIO_DRIVE: {why}");
            return Task::none();
        }
    };
    Task::stream(cosmic::iced::stream::channel(
        16,
        async move |mut sender| {
            for step in steps {
                match step {
                    Step::Sleep(duration) => tokio::time::sleep(duration).await,
                    Step::Msg(message) => {
                        let _ = sender.try_send(message);
                    }
                }
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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
                Step::Msg(Message::DriveViewer(msg)) => format!("{msg:?}"),
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
