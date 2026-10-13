//! Recently-opened projects (upstream `RecentProjects`, RecentProjects.swift)
//! as the welcome window and the recents menu see it: a thin wrapper that
//! DELEGATES the list semantics to [`engine::project::store::RecentProjects`]
//! (cap 12, canonicalize-dedupe, most-recent-first — never re-implemented
//! here) and persists ONLY the path list through cosmic-config.
//!
//! Cosmic-config is the app-UI state store by the settings single-writer law
//! (plan Global Constraints): the engine's `settings.json` is untouched by
//! recents; `Settings.recent_projects` stays dead in M4.

use std::path::{Path, PathBuf};

use cosmic::cosmic_config::{self, Config, ConfigGet, ConfigSet};
use engine::project::store::RecentProjects;

/// The cosmic-config key holding the path list.
pub const RECENTS_KEY: &str = "recents";

/// The first-run flag's cosmic-config key (the SetupSheet gate).
pub const FIRST_RUN_KEY: &str = "first-run-complete";

/// The recents list plus the config it round-trips through. `config` is
/// `None` only in tests that exercise the engine delegation without a
/// filesystem.
pub struct Recents {
    inner: RecentProjects,
    config: Option<Config>,
}

impl Recents {
    /// Load the persisted path list (missing key or unreadable config starts
    /// empty), dropping entries whose project folder no longer exists
    /// (upstream filters on load, RecentProjects.swift:12-16).
    pub fn load(config: Config) -> Self {
        let paths: Vec<String> = config.get(RECENTS_KEY).unwrap_or_default();
        let inner = RecentProjects(paths.into_iter().map(PathBuf::from).collect::<Vec<_>>())
            .filter_existing();
        Self {
            inner,
            config: Some(config),
        }
    }

    /// A recents list over an engine list with no persistence behind it.
    pub fn detached(inner: RecentProjects) -> Self {
        Self {
            inner,
            config: None,
        }
    }

    /// The project roots, most recent first.
    pub fn paths(&self) -> &[PathBuf] {
        &self.inner.0
    }

    /// Note a project was opened (upstream `add`): dedupe to the front, cap
    /// at 12, persist.
    pub fn add(&mut self, path: impl AsRef<Path>) {
        self.inner.add(path);
        self.save();
    }

    /// Drop one entry (upstream `remove`, the recents context menu).
    pub fn remove(&mut self, path: impl AsRef<Path>) {
        self.inner.remove(path);
        self.save();
    }

    /// Empty the list (upstream `clear`, the menu's "Clear Menu").
    pub fn clear(&mut self) {
        self.inner.clear();
        self.save();
    }

    /// Persist the path list. The only thing cosmic-config ever hears about
    /// recents; a config-less instance is in-memory only.
    fn save(&self) {
        if let Some(config) = &self.config {
            let paths: Vec<String> = self
                .inner
                .0
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            let _ = config.set(RECENTS_KEY, paths);
        }
    }
}

/// Whether the first-run SetupSheet must show: only the cosmic-config flag
/// decides (the engine's `has_completed_setup` is dead in M4 by the
/// single-writer law).
pub fn needs_setup(config: &Config) -> bool {
    !config.get::<bool>(FIRST_RUN_KEY).unwrap_or(false)
}

/// Mark first-run setup complete in cosmic-config.
pub fn complete_setup(config: &Config) -> Result<(), cosmic_config::Error> {
    config.set(FIRST_RUN_KEY, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_at(dir: &Path) -> Config {
        Config::with_custom_path("cad-studio-tests", 1, dir.to_path_buf()).unwrap()
    }

    fn project(parent: &Path, name: &str) -> PathBuf {
        let p = parent.join(name);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// The wrapper delegates the list semantics to the engine type: cap 12,
    /// canonicalize-dedupe to the front, most-recent-first — checked by
    /// driving the wrapper and reading the engine list back.
    #[test]
    fn recents_wrap_engine_semantics() {
        let dir = std::env::temp_dir().join(format!("cad-recents-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = config_at(&dir);
        let mut recents = Recents::load(config);

        for i in 0..14 {
            recents.add(project(&dir, &format!("p{i}")));
        }
        assert_eq!(recents.paths().len(), 12, "engine cap of 12 applies");

        // a second spelling of p5 (through a symlinked parent) dedupes: the
        // engine canonicalizes, so both land on one entry at the front
        let link = dir.join("alias");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        recents.add(link.join("p5"));
        assert_eq!(recents.paths().len(), 12, "dedupe keeps the cap");
        assert_eq!(
            recents.paths()[0],
            project(&dir, "p5").canonicalize().unwrap(),
            "the re-opened project moved to the front, deduped"
        );

        recents.remove(link.join("p5"));
        assert!(
            !recents.paths().iter().any(|p| p.ends_with("p5")),
            "remove hits through either spelling"
        );

        recents.clear();
        assert!(recents.paths().is_empty());
    }

    /// cosmic-config persists ONLY the path list: a fresh load over the same
    /// config sees exactly what was saved, and a path whose project folder
    /// vanished between save and load is filtered out (upstream's load-time
    /// existence filter).
    #[test]
    fn recents_round_trip_through_cosmic_config() {
        let dir = std::env::temp_dir().join(format!("cad-recents-rt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let gone = project(&dir, "gone");
        let kept = project(&dir, "kept");

        let config = config_at(&dir);
        let mut recents = Recents::load(config.clone());
        recents.add(&gone);
        recents.add(&kept);
        std::fs::remove_dir_all(&gone).unwrap();

        let reloaded = Recents::load(config_at(&dir));
        assert_eq!(
            reloaded.paths(),
            &[kept.canonicalize().unwrap()],
            "the path list round-trips and a deleted project drops out"
        );

        // the persisted artifact is the path list and nothing else
        let stored: Vec<String> = config.get(RECENTS_KEY).unwrap();
        assert_eq!(stored.len(), 2, "both adds persisted before the reload");
        assert!(stored.iter().any(|p| p.contains("gone")));
        assert!(stored.iter().any(|p| p.contains("kept")));
    }

    #[test]
    fn first_run_flag_logic() {
        let dir = std::env::temp_dir().join(format!("cad-recents-fr-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = config_at(&dir);

        assert!(
            needs_setup(&config),
            "a fresh config has not completed setup"
        );
        complete_setup(&config).unwrap();
        assert!(!needs_setup(&config), "the flag sticks across loads");
    }
}
