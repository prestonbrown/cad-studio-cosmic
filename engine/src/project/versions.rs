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
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

use crate::project::store::atomic_write;

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

/// Snapshots live under `<root>/.cadstudio/versions`.
pub fn versions_root(project_root: &Path) -> PathBuf {
    project_root.join(".cadstudio").join("versions")
}

fn version_dir(project_root: &Path, number: i64) -> PathBuf {
    versions_root(project_root).join(number.to_string())
}

/// Every `versions/<n>/version.json` that parses, ascending by number
/// (upstream compactMap + sort). Unparseable or absent entries are skipped,
/// an absent versions root is simply no versions.
pub fn load_versions(project_root: &Path) -> Vec<ProjectVersion> {
    let Ok(entries) = std::fs::read_dir(versions_root(project_root)) else {
        return Vec::new();
    };
    let mut versions: Vec<ProjectVersion> = entries
        .flatten()
        .filter_map(|entry| {
            let json =
                std::fs::read_to_string(entry.path().join("version.json")).ok()?;
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
pub fn has_changes(
    project_root: &Path,
    versions: &[ProjectVersion],
    current: Option<i64>,
) -> bool {
    let Some(version) = versions.iter().find(|v| Some(v.number) == current) else {
        return true;
    };
    let saved = std::fs::read(version_dir(project_root, version.number).join("model.py")).ok();
    let live = std::fs::read(project_root.join("model.py")).ok();
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
    copy_if_present(&project_root.join("model.py"), &dir.join("model.py"))?;
    copy_tree_if_present(&project_root.join("output"), &dir.join("output"))?;
    copy_if_present(
        &project_root.join(".cadstudio/chat.json"),
        &dir.join("chat.json"),
    )?;
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
    replace_with(&project_root.join("model.py"), &dir.join("model.py"))?;
    replace_with(
        &project_root.join(".cadstudio/chat.json"),
        &dir.join("chat.json"),
    )?;
    let output = project_root.join("output");
    for entry in std::fs::read_dir(&output)? {
        remove_path(&entry?.path())?;
    }
    // the version's output dir is read best-effort (upstream `try? ?? []`)
    if let Ok(entries) = std::fs::read_dir(dir.join("output")) {
        for entry in entries.flatten() {
            let target = output.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_tree(&entry.path(), &target)?;
            } else {
                std::fs::copy(entry.path(), &target)?;
            }
        }
    }
    Ok(())
}

/// The parameter values a version was recorded with, keyed for the
/// comparison picker: the version's stored model.py copy parsed by the
/// parameter scan. Empty when `number` is None, names no stored model, or
/// the copy is unreadable.
pub fn comparison_parameters(
    project_root: &Path,
    number: Option<i64>,
) -> BTreeMap<String, f64> {
    let Some(number) = number else {
        return BTreeMap::new();
    };
    let model = std::fs::read_to_string(version_dir(project_root, number).join("model.py"))
        .unwrap_or_default();
    scan_parameters(&model)
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
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
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
    if destination.is_dir() {
        std::fs::remove_dir_all(destination)?;
    } else if destination.exists() {
        std::fs::remove_file(destination)?;
    }
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

/// Stand-in for `crate::project::parameters::scan` until that module (M5
/// T1, parallel branch) merges: the `name = <number>` assignments of a
/// model.py, last write per name. The merge swaps the single call in
/// [`comparison_parameters`] for the real scan.
fn scan_parameters(script: &str) -> BTreeMap<String, f64> {
    let mut values = BTreeMap::new();
    for line in script.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((left, right)) = line.split_once('=') else {
            continue;
        };
        // `total == 3` lands here as right = "= 3"; `x += 1` fails the
        // identifier check on "x +"
        let name = left.split(':').next().unwrap_or_default().trim();
        let plain_assignment = !right.starts_with('=')
            && !name.is_empty()
            && name.chars().all(|c| c.is_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit());
        if plain_assignment
            && let Some(value) = right.trim().parse::<f64>().ok().filter(|v| v.is_finite())
        {
            values.insert(name.to_owned(), value);
        }
    }
    values
}
