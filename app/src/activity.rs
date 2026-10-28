//! The project activity model behind the header's ActivityViewer port
//! (upstream `Activity` + `issues`, Project.swift:21-30 and 113-130, and
//! AgentSession's `show`/`agentDidFinish` transitions). Pure: the window
//! folds bridge messages into an [`Activity`] through [`next`] and renders
//! [`issues`] in the issue buttons.
//!
//! The build error rides the FIRST issue slot from the live pipeline
//! (`Bridge::build_error`, wired M5 T8); the missing-output warning is
//! the upstream home of output warnings. The motion-error and collision
//! warnings joined at M7 (the pane-derived pair); the manifest warnings
//! remain M8's (the ActivityViewer full port).

use std::path::Path;

use engine::agent::session::TurnOutcome;
#[cfg(test)]
use engine::project::outputs::expected_outputs;

use crate::fl;

/// The engine display name (AgentLocator.swift:12 — upstream "Claude Code").
pub const ENGINE_NAME: &str = "Claude Code";

/// What the header shows the project is doing (upstream `Activity`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Working(String),
    Succeeded(String),
    Failed(String),
}

impl Activity {
    /// Upstream `isWorking` — drives the elapsed timer and the stop button.
    pub fn is_working(&self) -> bool {
        matches!(self, Activity::Working(_))
    }

    /// The status text (ActivityViewer.statusText; idle is `pronto`).
    pub fn text(&self) -> String {
        match self {
            Activity::Idle => fl!("pronto"),
            Activity::Working(text) | Activity::Succeeded(text) | Activity::Failed(text) => {
                text.clone()
            }
        }
    }
}

impl Default for Activity {
    fn default() -> Self {
        Self::Idle
    }
}

/// The issue kinds the ActivityViewer counts (upstream `Issue.Kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub kind: IssueKind,
    pub message: String,
}

/// The issue list (Project.swift:113-130): the build error first, one
/// warning naming the missing output files, then the two motion branches —
/// `motion_error` arrives already localized (the pane's reload formats
/// `animation-json-non-valido` itself), the collision count formats into
/// the fl! text. The manifest warnings join at M8.
pub fn issues(
    build_error: Option<&str>,
    missing_outputs: &[String],
    motion_error: Option<&str>,
    collision_frames: usize,
) -> Vec<Issue> {
    let mut out = Vec::new();
    if let Some(error) = build_error {
        out.push(Issue {
            kind: IssueKind::Error,
            message: error.to_string(),
        });
    }
    if !missing_outputs.is_empty() {
        out.push(Issue {
            kind: IssueKind::Warning,
            message: fl!(
                "file-di-output-mancanti",
                files = missing_outputs.join(", ")
            ),
        });
    }
    if let Some(error) = motion_error {
        out.push(Issue {
            kind: IssueKind::Warning,
            message: error.to_string(),
        });
    }
    if collision_frames > 0 {
        out.push(Issue {
            kind: IssueKind::Warning,
            message: fl!(
                "l-animazione-ha-collisioni-tra-parti-in-lld-fotogrammi",
                frames = collision_frames
            ),
        });
    }
    out
}

/// Upstream `ProjectOutput.missingFiles` (ProjectOutput.swift:23-27): the
/// expected outputs that do not exist yet, by file name. This is the
/// ActivityViewer's warning — distinct from the richer prompt notes in
/// [`engine::project::outputs::missing_requirements`]. Delegates to the
/// engine's [`missing_files`][engine::project::outputs::missing_files]
/// (the version-record gate's rule): one implementation, so the header's
/// warning list and the record gate can never drift apart.
pub fn missing_output_files(output_dir: &Path) -> Vec<String> {
    engine::project::outputs::missing_files(output_dir)
}

/// What happened to the running turn, as the activity model hears it — one
/// variant per bridge message that can move the activity.
#[derive(Debug, Clone, PartialEq)]
pub enum Signal<'a> {
    /// A send started (Project.send sets "working").
    TurnStarted,
    /// A tool row opened (AgentSession.handle's `show(.working(… · title))`).
    ToolOpened(&'a str),
    /// The turn ended.
    TurnFinished(&'a TurnOutcome),
    /// The event channel closed without a terminal (the bridge's fail-open
    /// end; upstream has no analog — classify as the engine-error state).
    Closed,
}

/// The next activity for a signal. `missing_outputs_empty` is the
/// missing-files state AFTER the turn (upstream recomputes before
/// classifying, Project.swift:266-274).
pub fn next(signal: Signal<'_>, missing_outputs_empty: bool) -> Activity {
    match signal {
        Signal::TurnStarted => Activity::Working(fl!("sta-lavorando", agent = ENGINE_NAME)),
        Signal::ToolOpened(title) => {
            Activity::Working(fl!("sta-lavorando-2", agent = ENGINE_NAME, title = title))
        }
        Signal::TurnFinished(outcome) => match outcome {
            TurnOutcome::Success { .. } => {
                if missing_outputs_empty {
                    Activity::Succeeded(fl!("modello-generato"))
                } else {
                    Activity::Succeeded(fl!("risposta-completata"))
                }
            }
            // upstream `agentDidFinish(.failed(...))` classifications
            // (AgentSession.swift:269, 294, 301)
            TurnOutcome::Failed { auth: true, .. } => {
                Activity::Failed(fl!("accesso-a-richiesto", agent = ENGINE_NAME))
            }
            TurnOutcome::Failed { .. } => Activity::Failed(fl!("errore-di", agent = ENGINE_NAME)),
            TurnOutcome::Cancelled => Activity::Failed(crate::chat::interrupted()),
            // Busy/MissingSession never surface as events (the engine's
            // dance absorbs them); a caller driving raw turns still ends
            // failed rather than stuck working.
            TurnOutcome::Busy | TurnOutcome::MissingSession => {
                Activity::Failed(fl!("errore-di", agent = ENGINE_NAME))
            }
        },
        Signal::Closed => Activity::Failed(fl!("errore-di", agent = ENGINE_NAME)),
    }
}

/// The elapsed-work label (ActivityViewer's minuteSecond format):
/// `m:ss`, minutes rolling past the hour rather than growing a third field.
pub fn elapsed_text(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("cad-activity-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The activity transitions match AgentSession's: working on start,
    /// working·title on each tool, and every end classification.
    #[test]
    fn activity_transitions_match_upstream() {
        let _lang = crate::localize::pin_en();
        assert_eq!(
            next(Signal::TurnStarted, false),
            Activity::Working("Claude Code is working…".into())
        );
        assert_eq!(
            next(Signal::ToolOpened("Writing model.py"), false),
            Activity::Working("Claude Code is working… · Writing model.py".into())
        );
        assert_eq!(
            next(
                Signal::TurnFinished(&TurnOutcome::Success {
                    session_id: None,
                    text: String::new()
                }),
                true
            ),
            Activity::Succeeded("Model generated".into())
        );
        assert_eq!(
            next(
                Signal::TurnFinished(&TurnOutcome::Success {
                    session_id: None,
                    text: String::new()
                }),
                false
            ),
            Activity::Succeeded("Response completed".into())
        );
        assert_eq!(
            next(
                Signal::TurnFinished(&TurnOutcome::Failed {
                    error: "bad".into(),
                    auth: true,
                    exit_code: Some(1)
                }),
                true
            ),
            Activity::Failed("Sign-in to Claude Code required".into())
        );
        assert_eq!(
            next(
                Signal::TurnFinished(&TurnOutcome::Failed {
                    error: "bad".into(),
                    auth: false,
                    exit_code: Some(2)
                }),
                true
            ),
            Activity::Failed("Claude Code error".into())
        );
        assert_eq!(
            next(Signal::TurnFinished(&TurnOutcome::Cancelled), true),
            Activity::Failed("Stopped".into())
        );
        assert_eq!(
            next(Signal::Closed, true),
            Activity::Failed("Claude Code error".into())
        );
    }

    #[test]
    fn elapsed_label_is_minute_second() {
        assert_eq!(elapsed_text(0), "0:00");
        assert_eq!(elapsed_text(5), "0:05");
        assert_eq!(elapsed_text(65), "1:05");
        assert_eq!(elapsed_text(3600), "60:00", "no third field: minutes roll");
    }

    /// The warning names exactly the missing expected outputs, in upstream
    /// report order; a full output directory produces none.
    #[test]
    fn missing_output_warning_lists_absent_files() {
        let _lang = crate::localize::pin_en();
        let dir = output_dir("missing");
        assert_eq!(missing_output_files(&dir).len(), 7, "everything missing");
        for name in expected_outputs() {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        assert!(missing_output_files(&dir).is_empty());

        let all_present = issues(None, &missing_output_files(&dir), None, 0);
        assert!(all_present.is_empty());

        std::fs::remove_file(dir.join("model.3mf")).unwrap();
        let missing = missing_output_files(&dir);
        assert_eq!(missing, vec!["model.3mf".to_string()]);
        let warnings_only = issues(None, &missing, None, 0);
        assert_eq!(warnings_only.len(), 1);
        assert_eq!(warnings_only[0].kind, IssueKind::Warning);
        assert_eq!(warnings_only[0].message, "Missing output files: model.3mf");

        // the drift pin (M5 branch review MINOR 2): the header's warning
        // list IS the version-record gate's rule — one engine
        // implementation — so the two can never disagree
        assert_eq!(
            missing,
            engine::project::outputs::missing_files(&dir),
            "the app twin delegates to the engine twin"
        );

        let both = issues(Some("SyntaxError: invalid syntax"), &missing, None, 0);
        assert_eq!(both[0].kind, IssueKind::Error, "the build error leads");
        assert_eq!(both.len(), 2);
    }

    /// The motion branches (Project.swift:123-127, appended after the two
    /// M5 ones): the pane's already-substituted error text rides one
    /// warning; a colliding study rides one warning with the frame count
    /// substituted into the fl! text; a clean study stays silent.
    #[test]
    fn motion_issues_render_both_upstream_branches() {
        let _lang = crate::localize::pin_en();
        let issues = crate::activity::issues(
            None,
            &[],
            Some("animation.json is not valid: expected a comma"),
            0,
        );
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, IssueKind::Warning);
        assert_eq!(
            issues[0].message,
            "animation.json is not valid: expected a comma"
        );
        let issues = crate::activity::issues(None, &[], None, 2);
        assert_eq!(issues.len(), 1);
        assert!(
            issues[0].message.contains("2 frames"),
            "count substituted into the fl! text: {}",
            issues[0].message
        );
        assert!(!issues[0].message.contains("%lld"), "placeholder gone");
        // zero-collision and no-error studies stay silent
        assert!(crate::activity::issues(None, &[], None, 0).is_empty());
    }
}
