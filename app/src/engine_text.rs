//! The engine's user-visible ids, localized. The engine has no catalog: it
//! emits stable ids ([`EnvStep`], [`InstallError`], [`EnvLog`],
//! [`ToolTitle`]) and this module is the one boundary that turns each into
//! catalog text. Every id maps to an upstream catalog key; the tests check
//! each key exists in all four catalogs, since `fl!` resolves keys at run
//! time and a typo would render the raw key.

use engine::agent::tool_summary::{ToolAction, ToolTitle};
use engine::project::ProjectStoreError;
use engine::python::env::{EnvFailure, EnvLog, EnvStep, InstallError};

use crate::fl;

/// The catalog key of a progress step (PythonEnvironment.swift:68-97).
pub fn env_step_key(step: EnvStep) -> &'static str {
    match step {
        EnvStep::SearchingPython => "ricerca-di-python-3-10-3-13",
        EnvStep::CreatingVenvWithUv => "creazione-dell-ambiente-virtuale-con-uv",
        EnvStep::CreatingVenv => "creazione-dell-ambiente-virtuale",
        EnvStep::UpgradingPip => "aggiornamento-di-pip",
        EnvStep::InstallingPackages => "installazione-di-build123d-matplotlib-e-numpy",
        EnvStep::Verifying => "verifica-dell-installazione",
        EnvStep::Installed => "componenti-installati",
        EnvStep::InstallFailed => "installazione-non-riuscita",
    }
}

pub fn env_step(step: EnvStep) -> String {
    lookup(env_step_key(step))
}

/// Upstream `PythonSetupError.errorDescription`; a spawn error is the OS
/// text after the program name, as ProcessRunner's thrown error reads.
pub fn install_error(error: &InstallError) -> String {
    match error {
        InstallError::PythonNotFound => fl!(
            "non-stato-trovato-n-uv-n-un-python-3-10-3-13-installa-uv-brew-install-uv-oppure-python-3-12-e-riprova"
        ),
        InstallError::CommandFailed { command } => fl!(
            "il-comando-non-riuscito-controlla-il-log-per-i-dettagli",
            command = command
        ),
        InstallError::VerificationFailed => {
            fl!("build123d-e-matplotlib-non-si-importano-correttamente-nell-ambiente-creato")
        }
        InstallError::SpawnFailed { command, error } => format!("{command}: {error}"),
    }
}

/// The failure label a finished driver leaves in `EnvState::Failed`;
/// `Missing` has none upstream.
pub fn env_failure(failure: &EnvFailure) -> String {
    match failure {
        EnvFailure::Missing => String::new(),
        EnvFailure::CheckFailed => {
            fl!("l-ambiente-python-esiste-ma-build123d-o-matplotlib-non-si-importano")
        }
        EnvFailure::InstallFailed(error) => install_error(error),
    }
}

/// One install-log line (upstream `appendLog`).
pub fn env_log(line: &EnvLog) -> String {
    match line {
        EnvLog::Text(text) => text.clone(),
        EnvLog::Step(step) => env_step(*step),
        EnvLog::FoundPython { version, path } => {
            fl!("trovato-in", version = version, path = path.display())
        }
        EnvLog::Failure(error) => install_error(error),
    }
}

/// The catalog key of a tool action (ToolSummary.swift).
pub fn tool_action_key(action: ToolAction) -> &'static str {
    match action {
        ToolAction::InstallComponents => "installa-i-componenti",
        ToolAction::BuildModel => "compila-il-modello",
        ToolAction::AnalyzeModel => "analizza-il-modello",
        ToolAction::PreparePreview => "prepara-un-anteprima",
        ToolAction::ExamineFiles => "esamina-i-file",
        ToolAction::EditModel => "modifica-il-modello",
        ToolAction::TidyFiles => "sistema-i-file",
        ToolAction::WorkProject => "lavora-sul-progetto",
        ToolAction::WriteFile => "scrive-un-file",
        ToolAction::WriteModel => "scrive-il-modello",
        ToolAction::WriteHelper => "prepara-un-file-di-supporto",
        ToolAction::EditHelper => "modifica-un-file-di-supporto",
        ToolAction::ReadFile => "legge-un-file",
        ToolAction::StudyReference => "studia-il-riferimento",
        ToolAction::RereadModel => "rilegge-il-modello",
        ToolAction::CheckDrawing => "controlla-il-disegno",
        ToolAction::CheckResults => "controlla-i-risultati",
        ToolAction::CheckPreview => "controlla-un-anteprima",
        ToolAction::SearchProject => "cerca-nel-progetto",
        ToolAction::UpdatePlan => "aggiorna-il-piano-di-lavoro",
        ToolAction::SearchWeb => "cerca-sul-web",
    }
}

/// A tool row's title: the action, the streaming Write's `lld-righe` line
/// count (ToolSummary.swift:74), then `…` while the input still streams.
pub fn tool_title(title: &ToolTitle) -> String {
    let action = lookup(tool_action_key(title.action));
    let mut text = match title.lines {
        Some(lines) => fl!("lld-righe", file = action, lines = lines),
        None => action,
    };
    if title.streaming {
        text.push('…');
    }
    text
}

/// Upstream `ProjectError.errorDescription` (ProjectStore.swift:25-31)
/// for the variants it names; I/O and JSON failures carry the system's own
/// text, as upstream's Foundation errors do.
pub fn project_store_error(error: &ProjectStoreError) -> String {
    match error {
        ProjectStoreError::NotAFolder(path) => {
            fl!("non-una-cartella", path = path.display().to_string())
        }
        ProjectStoreError::Io(error) => error.to_string(),
        ProjectStoreError::Json(error) => error.to_string(),
        ProjectStoreError::InvalidName(error) => error.to_string(),
    }
}

fn lookup(key: &str) -> String {
    crate::localize::localize();
    crate::localize::lookup(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Keys every arg-less id resolves through [`lookup`], plus the
    /// literal keys the `fl!` arms above use.
    fn all_keys() -> Vec<&'static str> {
        let mut keys: Vec<&'static str> = EnvStep::ALL.iter().map(|s| env_step_key(*s)).collect();
        keys.extend(ToolAction::ALL.iter().map(|a| tool_action_key(*a)));
        keys.extend([
            "non-stato-trovato-n-uv-n-un-python-3-10-3-13-installa-uv-brew-install-uv-oppure-python-3-12-e-riprova",
            "il-comando-non-riuscito-controlla-il-log-per-i-dettagli",
            "build123d-e-matplotlib-non-si-importano-correttamente-nell-ambiente-creato",
            "l-ambiente-python-esiste-ma-build123d-o-matplotlib-non-si-importano",
            "trovato-in",
            "lld-righe",
            "non-una-cartella",
        ]);
        keys
    }

    #[test]
    fn every_engine_id_has_a_key_in_all_four_catalogs() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("i18n");
        for lang in ["en", "it", "de", "fr"] {
            let text = std::fs::read_to_string(dir.join(lang).join("main.ftl")).unwrap();
            for key in all_keys() {
                let prefix = format!("{key} = ");
                assert!(
                    text.lines().any(|line| line.starts_with(&prefix)),
                    "{lang} has no `{key}`"
                );
            }
        }
    }

    #[test]
    fn ids_render_in_the_selected_language() {
        let _lang = crate::localize::pin_en();
        assert_eq!(env_step(EnvStep::Verifying), "Verifying the installation…");
        crate::localize::select("it");
        let italian = env_step(EnvStep::Verifying);
        let title = tool_title(&ToolTitle {
            action: ToolAction::WriteModel,
            lines: Some(3),
            streaming: true,
        });
        crate::localize::select("en");
        assert_eq!(italian, "Verifica dell’installazione…");
        assert_eq!(title, "Scrive il modello · 3 righe…");
    }

    #[test]
    fn failures_and_log_lines_carry_their_args() {
        let _lang = crate::localize::pin_en();
        assert_eq!(
            install_error(&InstallError::CommandFailed {
                command: "uv".into()
            }),
            "The command “uv” failed. Check the log for details."
        );
        assert_eq!(env_failure(&EnvFailure::Missing), "");
        assert_eq!(
            env_log(&EnvLog::FoundPython {
                version: "Python 3.12.4".into(),
                path: "/usr/bin/python3.12".into(),
            }),
            "Found Python 3.12.4 at /usr/bin/python3.12"
        );
        assert_eq!(tool_title(&ToolAction::WriteFile.into()), "Writing a file");
    }

    /// The load-error window's text: upstream's `notAFolder` sentence with
    /// the path kept, in every catalog language.
    #[test]
    fn a_store_error_names_its_path_in_each_language() {
        let _lang = crate::localize::pin_en();
        let error = ProjectStoreError::NotAFolder(PathBuf::from("/tmp/Staffa.txt"));
        let mut rendered = Vec::new();
        for lang in ["en", "it", "de", "fr"] {
            crate::localize::select(lang);
            rendered.push(project_store_error(&error));
        }
        crate::localize::select("en");
        assert_eq!(
            rendered,
            [
                "“/tmp/Staffa.txt” isn’t a folder.",
                "“/tmp/Staffa.txt” non è una cartella.",
                "„/tmp/Staffa.txt“ ist kein Ordner.",
                "« /tmp/Staffa.txt » n’est pas un dossier.",
            ]
        );
    }

    /// The turn-level markers become upstream's system rows: AgentMissing
    /// carries the Configure-agent action (AgentSession.swift:42-46),
    /// StartFailed the OS error as detail (:98-99).
    #[test]
    fn turn_markers_append_localized_system_rows() {
        use engine::agent::session::{EngineEvent, TurnOutcome};
        use engine::project::store::{Action, Role};
        let _lang = crate::localize::pin_en();
        let mut chat = crate::chat::ChatState::new(Vec::new());
        chat.apply(&EngineEvent::TurnFinished(TurnOutcome::AgentMissing));
        let row = chat.messages.last().unwrap();
        assert_eq!(row.role, Role::System);
        assert_eq!(row.action, Some(Action::ConfigureClaude));
        assert!(
            row.text.contains("isn't installed or couldn't be found"),
            "{}",
            row.text
        );
        chat.apply(&EngineEvent::TurnFinished(TurnOutcome::StartFailed {
            error: "Permission denied".into(),
        }));
        let row = chat.messages.last().unwrap();
        assert!(row.text.starts_with("Couldn't start"), "{}", row.text);
        assert_eq!(row.detail.as_deref(), Some("Permission denied"));
        assert_eq!(row.action, None);
    }
}
