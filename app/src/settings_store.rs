//! The app's one door onto the engine settings file. Every reader loads it
//! fresh and every writer goes through [`SettingsStore::update`], so nothing
//! holds a copy that a write elsewhere can leave stale: a project window
//! opened after a toggle in another window sees that toggle. One error
//! policy for every writer: a file that exists but cannot be read is never
//! written over (readers see defaults, writers are refused, so one bad field
//! cannot erase every other setting), and every failure is logged here and
//! returned. The refusal is logged once per process, not once per toggle.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use engine::settings::{Settings, SettingsError, settings_path};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Where the settings live. Cloning shares the file, not a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    /// The user's settings file (`engine::settings::settings_path`).
    pub fn global() -> Self {
        Self::at(settings_path())
    }

    /// A store over `path`.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// A store over a fresh, not-yet-written file in the temp directory —
    /// the test seam that keeps a test off the user's real settings.
    pub fn in_temp_dir() -> Self {
        Self::at(std::env::temp_dir().join(format!("cad-settings-{}.json", uuid::Uuid::new_v4())))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The settings as they are on disk now. A missing file is a fresh
    /// install (`Settings::load`); an unreadable one reads as the defaults.
    pub fn load(&self) -> Settings {
        Settings::load(&self.path).unwrap_or_default()
    }

    /// Load fresh, apply `edit`, save. A file that exists but does not
    /// parse refuses the update untouched; a missing file is a fresh
    /// install and is created.
    pub fn update(&self, edit: impl FnOnce(&mut Settings)) -> Result<(), SettingsError> {
        let mut settings = match Settings::load(&self.path) {
            Ok(settings) => settings,
            Err(why) => {
                if !REFUSAL_LOGGED.swap(true, Ordering::Relaxed) {
                    log(&format!(
                        "settings at {} cannot be read, so no setting is saved until the file is fixed: {why}",
                        self.path.display()
                    ));
                }
                return Err(why);
            }
        };
        edit(&mut settings);
        settings.save(&self.path).inspect_err(|why| {
            log(&format!(
                "settings save to {} failed: {why}",
                self.path.display()
            ));
        })
    }
}

/// Whether this process has already reported an unreadable settings file.
static REFUSAL_LOGGED: AtomicBool = AtomicBool::new(false);

fn log(line: &str) {
    let now = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    eprintln!("{now} cad-studio: {line}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A write lands on disk at once and the next load sees it; a second
    /// store over the same file (another window's handle) sees it too.
    #[test]
    fn an_update_is_visible_to_every_later_load() {
        let store = SettingsStore::in_temp_dir();
        assert!(!store.load().parameters_collapsed, "a fresh install");
        store.update(|s| s.parameters_collapsed = true).unwrap();
        assert!(store.load().parameters_collapsed);
        let other = SettingsStore::at(store.path());
        assert!(other.load().parameters_collapsed);
    }

    /// An update keeps every field it does not touch, including ones
    /// another writer saved after this store was created.
    #[test]
    fn an_update_preserves_fields_it_does_not_edit() {
        let store = SettingsStore::in_temp_dir();
        store.update(|s| s.agent_engine = "codex".into()).unwrap();
        SettingsStore::at(store.path())
            .update(|s| s.default_layout = Some(serde_json::json!({"pane": "chat"})))
            .unwrap();
        store.update(|s| s.parameters_collapsed = true).unwrap();
        let saved = store.load();
        assert_eq!(saved.agent_engine, "codex");
        assert!(saved.default_layout.is_some());
        assert!(saved.parameters_collapsed);
    }

    /// A file that exists but does not parse is never overwritten: one bad
    /// field must not cost every other setting on the next toggle.
    #[test]
    fn an_unreadable_file_refuses_the_update_and_stays_byte_for_byte() {
        let store = SettingsStore::in_temp_dir();
        let bytes =
            br#"{"recentProjects": null, "agentEngine": "codex", "claudePath": "/opt/claude"}"#;
        std::fs::write(store.path(), bytes).unwrap();
        assert!(store.update(|s| s.parameters_collapsed = true).is_err());
        assert_eq!(std::fs::read(store.path()).unwrap(), bytes);
        assert!(
            store.load().claude_path.is_none(),
            "readers keep the defaults"
        );
    }

    /// A save that cannot land is reported, not swallowed.
    #[test]
    fn a_failed_save_is_returned() {
        let dir = std::env::temp_dir().join(format!("cad-settings-ro-{}", uuid::Uuid::new_v4()));
        std::fs::write(&dir, b"a file where the directory should be").unwrap();
        let store = SettingsStore::at(dir.join("settings.json"));
        assert!(store.update(|s| s.parameters_collapsed = true).is_err());
    }
}
