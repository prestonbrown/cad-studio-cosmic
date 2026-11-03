//! Version history (upstream VersionHistory.swift): immutable snapshots
//! under `<root>/.cadstudio/versions/<n>/` holding `model.py`, a recursive
//! copy of `output/`, `chat.json`, and `version.json`. Restore swaps those
//! files back and empties `output/` first; metadata (`session_id`,
//! `current_version`) belongs to the caller. Comparison parameters come
//! from the version's stored model.py copy, not its output manifest — a
//! slider rewrite of model.py never regenerates the manifest, so the model
//! source is the only value that stays truthful.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use time::OffsetDateTime;

use crate::project::folder::{self, CHAT_FILE, MODEL_SCRIPT, OUTPUT_DIR, version_dir};
use crate::storage::atomic_write;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectVersion {
    pub number: i64,
    #[serde(with = "crate::project::store::iso8601_utc")]
    pub created_at: OffsetDateTime,
    pub prompt: String,
    pub session_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum VersionError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Every `versions/<n>/version.json` that parses, ascending by number
/// (upstream compactMap + sort). Unparseable or absent entries are skipped,
/// an absent versions root is simply no versions.
pub fn load_versions(project_root: &Path) -> Vec<ProjectVersion> {
    let Ok(entries) = std::fs::read_dir(folder::versions_dir(project_root)) else {
        return Vec::new();
    };
    let mut versions: Vec<ProjectVersion> = entries
        .flatten()
        .filter_map(|entry| {
            let json = std::fs::read_to_string(entry.path().join("version.json")).ok()?;
            serde_json::from_str(&json).ok()
        })
        .collect();
    versions.sort_by_key(|version| version.number);
    versions
}

/// `last.number + 1`, `1` when the history is empty.
pub fn next_number(versions: &[ProjectVersion]) -> i64 {
    versions.last().map_or(1, |version| version.number + 1)
}

/// Whether model.py drifted from the current version's copy. Unknown or
/// absent `current` means changed; both sides missing compares equal
/// (upstream `try? Data` nils, VersionHistory.swift:37-42).
pub fn has_changes(project_root: &Path, versions: &[ProjectVersion], current: Option<i64>) -> bool {
    let Some(version) = versions.iter().find(|v| Some(v.number) == current) else {
        return true;
    };
    let saved = std::fs::read(folder::model_script(&version_dir(
        project_root,
        version.number,
    )))
    .ok();
    let live = std::fs::read(folder::model_script(project_root)).ok();
    saved != live
}

/// Snapshot the project as the next version: copy-if-present `model.py`,
/// `output/` (recursive), and `.cadstudio/chat.json`, then write
/// `version.json`. Absent sources are skipped — a version can legitimately
/// hold no model or chat. On success the version is appended and returned.
pub fn record(
    project_root: &Path,
    versions: &mut Vec<ProjectVersion>,
    prompt: &str,
    session_id: Option<&str>,
    now: OffsetDateTime,
) -> Result<ProjectVersion, VersionError> {
    let version = ProjectVersion {
        number: next_number(versions),
        created_at: now,
        prompt: prompt.to_owned(),
        session_id: session_id.map(str::to_owned),
    };
    let dir = version_dir(project_root, version.number);
    std::fs::create_dir_all(&dir)?;
    copy_if_present(&project_root.join(MODEL_SCRIPT), &dir.join(MODEL_SCRIPT))?;
    copy_tree_if_present(&project_root.join(OUTPUT_DIR), &dir.join(OUTPUT_DIR))?;
    copy_if_present(&folder::chat_path(project_root), &dir.join(CHAT_FILE))?;
    let json = serde_json::to_string_pretty(&version)?;
    atomic_write(&dir.join("version.json"), json.as_bytes())?;
    versions.push(version.clone());
    Ok(version)
}

/// Swap a version back in: `model.py` and `.cadstudio/chat.json` are
/// remove-then-copy-if-present (a version recorded without them restores to
/// their absence), every entry in `output/` is removed, then the version's
/// `output/` contents are copied in. A version with no `output/` dir just
/// leaves `output/` empty. Metadata is the caller's business.
pub fn restore(project_root: &Path, version: &ProjectVersion) -> Result<(), VersionError> {
    let dir = version_dir(project_root, version.number);
    replace_with(&project_root.join(MODEL_SCRIPT), &dir.join(MODEL_SCRIPT))?;
    replace_with(&folder::chat_path(project_root), &dir.join(CHAT_FILE))?;
    let output = project_root.join(OUTPUT_DIR);
    for entry in std::fs::read_dir(&output)? {
        remove_path(&entry?.path())?;
    }
    // the version's output dir is read best-effort (upstream `try? ?? []`)
    if let Ok(entries) = std::fs::read_dir(dir.join(OUTPUT_DIR)) {
        copy_entries(entries.flatten(), &output)?;
    }
    Ok(())
}

/// The parameter values a version was recorded with, keyed for the
/// comparison picker: the version's stored model.py copy parsed by
/// [`crate::project::parameters::scan`]. Empty when `number` is None,
/// names no stored model, or the copy is unreadable.
pub fn comparison_parameters(project_root: &Path, number: Option<i64>) -> BTreeMap<String, f64> {
    let Some(number) = number else {
        return BTreeMap::new();
    };
    let model = std::fs::read_to_string(folder::model_script(&version_dir(project_root, number)))
        .unwrap_or_default();
    crate::project::parameters::scan(&model)
}

fn copy_if_present(source: &Path, destination: &Path) -> std::io::Result<()> {
    if source.exists() {
        std::fs::copy(source, destination)?;
    }
    Ok(())
}

/// Recursive directory copy (upstream `copyItem` on a directory).
fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    copy_entries(
        std::fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?,
        destination,
    )
}

/// Copy each entry into `destination` under its own name, recursing into
/// directories.
fn copy_entries(
    entries: impl IntoIterator<Item = std::fs::DirEntry>,
    destination: &Path,
) -> std::io::Result<()> {
    for entry in entries {
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn copy_tree_if_present(source: &Path, destination: &Path) -> std::io::Result<()> {
    if source.is_dir() {
        copy_tree(source, destination)?;
    }
    Ok(())
}

/// Remove `destination`, then copy `source` in when it exists (upstream
/// `replace`).
fn replace_with(destination: &Path, source: &Path) -> std::io::Result<()> {
    remove_path(destination)?;
    copy_if_present(source, destination)
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}
