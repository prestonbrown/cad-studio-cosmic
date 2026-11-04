//! The project activity model behind the header's ActivityViewer port
//! (upstream `Activity` + `issues`, Project.swift:21-30 and 113-130, and
//! AgentSession's `show`/`agentDidFinish` transitions). Pure: the bridge
//! classifies every turn transition through [`next`] into the one
//! [`Activity`] it holds, the window mirrors it, and renders [`issues`] in
//! the issue buttons.
//!
//! The build error rides the FIRST issue slot from the live pipeline
//! (`Bridge::build_error`); the missing-output warning is the upstream
//! home of output warnings. The motion-error and collision warnings are
//! the pane-derived pair; the manifest warnings join through the pane's
//! reload-fresh copy of the manifest.


use engine::agent::locator::AgentKind;
use engine::agent::session::TurnOutcome;
#[cfg(test)]
use engine::project::outputs::expected_outputs;

use crate::fl;

/// What the header shows the project is doing (upstream `Activity`,
/// Project.swift:19-26), and the pipeline's sole busy flag: upstream's
/// `isBusy` is `activity.isWorking`.
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
/// warning naming the missing output files, the manifest's warnings one
/// warning each, then the two motion branches — `motion_error` arrives
/// already localized (the pane's reload formats `animation-json-non-valido`
/// itself), the collision count formats into the fl! text.
pub fn issues(
    build_error: Option<&str>,
    missing_outputs: &[String],
    manifest_warnings: &[String],
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
    out.extend(manifest_warnings.iter().map(|warning| Issue {
        kind: IssueKind::Warning,
        message: warning.clone(),
    }));
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

/// The next activity for a signal. `engine` is the engine the turn runs
/// (the engine's own pick, `TurnHandle::engine`); the strings interpolate
/// its display name (AgentLocator.swift:11-16: "Claude Code" / "Codex").
/// `missing_outputs_empty` is the missing-files state AFTER the turn
/// (upstream recomputes before classifying, Project.swift:266-274).
pub fn next(signal: Signal<'_>, missing_outputs_empty: bool, engine: AgentKind) -> Activity {
    let agent = engine.display_name();
    match signal {
        Signal::TurnStarted => Activity::Working(fl!("sta-lavorando", agent = agent)),
        Signal::ToolOpened(title) => {
            Activity::Working(fl!("sta-lavorando-2", agent = agent, title = title))
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
                Activity::Failed(fl!("accesso-a-richiesto", agent = agent))
            }
            TurnOutcome::Failed { .. } | TurnOutcome::StartFailed { .. } => {
                Activity::Failed(fl!("errore-di", agent = agent))
            }
            TurnOutcome::AgentMissing => Activity::Failed(fl!("non-trovato", agent = agent)),
            TurnOutcome::Cancelled => Activity::Failed(crate::chat::interrupted()),
            // Busy/MissingSession never surface as events (the engine's
            // dance absorbs them); a caller driving raw turns still ends
            // failed rather than stuck working.
            TurnOutcome::Busy | TurnOutcome::MissingSession => {
                Activity::Failed(fl!("errore-di", agent = agent))
            }
        },
        Signal::Closed => Activity::Failed(fl!("errore-di", agent = agent)),
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
    /// working·title on each tool, and every end classification. The engine
    /// is the caller's input — the claude rows here, the codex pair below.
    #[test]
    fn activity_transitions_match_upstream() {
        let _lang = crate::localize::pin_en();
        assert_eq!(
            next(Signal::TurnStarted, false, AgentKind::Claude),
            Activity::Working("Claude Code is working…".into())
        );
        assert_eq!(
            next(
                Signal::ToolOpened("Writing model.py"),
                false,
                AgentKind::Claude
            ),
            Activity::Working("Claude Code is working… · Writing model.py".into())
        );
        assert_eq!(
            next(
                Signal::TurnFinished(&TurnOutcome::Success {
                    session_id: None,
                    text: String::new()
                }),
                true,
                AgentKind::Claude
            ),
            Activity::Succeeded("Model generated".into())
        );
        assert_eq!(
            next(
                Signal::TurnFinished(&TurnOutcome::Success {
                    session_id: None,
                    text: String::new()
                }),
                false,
                AgentKind::Claude
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
                true,
                AgentKind::Claude
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
                true,
                AgentKind::Claude
            ),
            Activity::Failed("Claude Code error".into())
        );
        assert_eq!(
            next(
                Signal::TurnFinished(&TurnOutcome::Cancelled),
                true,
                AgentKind::Claude
            ),
            Activity::Failed("Stopped".into())
        );
        assert_eq!(
            next(Signal::Closed, true, AgentKind::Claude),
            Activity::Failed("Claude Code error".into())
        );
    }

    /// Every activity string names the engine it is given, and the engine's
    /// absorbed outcomes (Busy/MissingSession, which `Session::send` never
    /// emits) still end failed rather than stuck working.
    #[test]
    fn activities_name_the_given_engine() {
        let _lang = crate::localize::pin_en();
        assert_eq!(
            next(Signal::TurnStarted, false, AgentKind::Codex),
            Activity::Working("Codex is working…".into())
        );
        assert_eq!(
            next(
                Signal::TurnFinished(&TurnOutcome::Failed {
                    error: "bad".into(),
                    auth: false,
                    exit_code: None
                }),
                true,
                AgentKind::Codex
            ),
            Activity::Failed("Codex error".into())
        );
        for absorbed in [TurnOutcome::Busy, TurnOutcome::MissingSession] {
            assert_eq!(
                next(Signal::TurnFinished(&absorbed), true, AgentKind::Codex),
                Activity::Failed("Codex error".into())
            );
        }
        assert_eq!(
            next(
                Signal::TurnFinished(&TurnOutcome::AgentMissing),
                true,
                AgentKind::Codex
            ),
            Activity::Failed(fl!("non-trovato", agent = "Codex"))
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
        use engine::project::outputs::missing_files as missing_output_files;
        let _lang = crate::localize::pin_en();
        let dir = output_dir("missing");
        assert_eq!(missing_output_files(&dir).len(), 7, "everything missing");
        for name in expected_outputs() {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        assert!(missing_output_files(&dir).is_empty());

        let all_present = issues(None, &missing_output_files(&dir), &[], None, 0);
        assert!(all_present.is_empty());

        std::fs::remove_file(dir.join("model.3mf")).unwrap();
        let missing = missing_output_files(&dir);
        assert_eq!(missing, vec!["model.3mf".to_string()]);
        let warnings_only = issues(None, &missing, &[], None, 0);
        assert_eq!(warnings_only.len(), 1);
        assert_eq!(warnings_only[0].kind, IssueKind::Warning);
        assert_eq!(warnings_only[0].message, "Missing output files: model.3mf");

        let both = issues(Some("SyntaxError: invalid syntax"), &missing, &[], None, 0);
        assert_eq!(both[0].kind, IssueKind::Error, "the build error leads");
        assert_eq!(both.len(), 2);
    }

    /// The motion branches (Project.swift:123-127, appended after the build
    /// error and missing-output ones): the pane's already-substituted error text rides one
    /// warning; a colliding study rides one warning with the frame count
    /// substituted into the fl! text; a clean study stays silent.
    #[test]
    fn motion_issues_render_both_upstream_branches() {
        let _lang = crate::localize::pin_en();
        let issues = crate::activity::issues(
            None,
            &[],
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
        let issues = crate::activity::issues(None, &[], &[], None, 2);
        assert_eq!(issues.len(), 1);
        assert!(
            issues[0].message.contains("2 frames"),
            "count substituted into the fl! text: {}",
            issues[0].message
        );
        assert!(!issues[0].message.contains("%lld"), "placeholder gone");
        // zero-collision and no-error studies stay silent
        assert!(crate::activity::issues(None, &[], &[], None, 0).is_empty());
    }

    /// The manifest's warnings render one warning each, verbatim
    /// (Project.swift:120: `issues += (output.manifest?.warnings ?? []).map
    /// { Issue(kind: .warning, message: $0) }`) — placed after the
    /// missing-output warning and before the motion branches.
    #[test]
    fn manifest_warnings_render_one_warning_each_in_upstream_order() {
        let _lang = crate::localize::pin_en();
        let all = issues(
            Some("SyntaxError: invalid syntax"),
            &["model.3mf".to_string()],
            &[
                "bed smaller than the part".to_string(),
                "wall exceeds the printer height".to_string(),
            ],
            Some("animation.json is not valid: expected a comma"),
            3,
        );
        assert_eq!(all.len(), 6);
        assert_eq!(all[0].kind, IssueKind::Error, "the build error leads");
        assert_eq!(all[1].message, "Missing output files: model.3mf");
        assert_eq!(all[2].message, "bed smaller than the part");
        assert_eq!(all[3].message, "wall exceeds the printer height");
        assert!(all[4].message.contains("not valid"), "motion error follows");
        assert!(all[5].message.contains("3 frames"), "collision count last");
        assert!(
            all[1..]
                .iter()
                .all(|issue| issue.kind == IssueKind::Warning),
            "everything but the build error is a warning"
        );
    }
}
