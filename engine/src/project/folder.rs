//! The project folder layout (upstream ProjectFolder.swift): every path
//! under a project root that more than one module names. Callers join
//! through these instead of spelling `.cadstudio` or `output` themselves.
//!
//! ```text
//! <root>/model.py
//! <root>/references/<stamp>-<n>-<name>   (turn attachments)
//! <root>/output/{model.3mf, model.stl, plate.3mf, manifest.json, …}
//! <root>/.cadstudio/{project.json, chat.json}
//! <root>/.cadstudio/logs/<timestamp>.jsonl
//! <root>/.cadstudio/versions/<n>/{model.py, output/, chat.json, version.json}
//! ```
//!
//! A version snapshot dir mirrors the root for `model.py` and `output/`, so
//! [`model_script`] and [`output_dir`] accept either.

use std::path::{Path, PathBuf};

/// The agent-written build script.
pub const MODEL_SCRIPT: &str = "model.py";
/// Everything model.py produces sits under this directory.
pub const OUTPUT_DIR: &str = "output";
/// Turn attachments are copied in here; the prompt names them by
/// `references/<name>`.
pub const REFERENCES_DIR: &str = "references";
/// Hidden per-project support directory.
pub const SUPPORT_DIR: &str = ".cadstudio";
/// The chat transcript's file name, in the support dir and in a version dir.
pub const CHAT_FILE: &str = "chat.json";
/// The project metadata's file name, in the support dir.
pub const METADATA_FILE: &str = "project.json";

/// The output files the app and the reload watcher name, under [`OUTPUT_DIR`].
pub const MODEL_3MF: &str = "model.3mf";
pub const MODEL_STL: &str = "model.stl";
pub const MODEL_STEP: &str = "model.step";
pub const PLATE_3MF: &str = "plate.3mf";
pub const MANIFEST: &str = "manifest.json";
pub const ANIMATION: &str = "animation.json";
pub const SCHEMATIC_SVG: &str = "schematic.svg";
pub const SCHEMATIC_PNG: &str = "schematic.png";
pub const SCHEMATIC_PDF: &str = "schematic.pdf";

/// `<root>/model.py` (also valid for a version snapshot dir).
pub fn model_script(root: &Path) -> PathBuf {
    root.join(MODEL_SCRIPT)
}

/// `<root>/output` (also valid for a version snapshot dir).
pub fn output_dir(root: &Path) -> PathBuf {
    root.join(OUTPUT_DIR)
}

/// `<root>/references`.
pub fn references_dir(root: &Path) -> PathBuf {
    root.join(REFERENCES_DIR)
}

/// `<root>/.cadstudio`.
pub fn support_dir(root: &Path) -> PathBuf {
    root.join(SUPPORT_DIR)
}

/// `<root>/.cadstudio/project.json`.
pub fn metadata_path(root: &Path) -> PathBuf {
    support_dir(root).join(METADATA_FILE)
}

/// `<root>/.cadstudio/chat.json`.
pub fn chat_path(root: &Path) -> PathBuf {
    support_dir(root).join(CHAT_FILE)
}

/// `<root>/.cadstudio/logs`: one tee log per agent turn.
pub fn logs_dir(root: &Path) -> PathBuf {
    support_dir(root).join("logs")
}

/// `<root>/.cadstudio/versions`: one numbered snapshot dir per version.
pub fn versions_dir(root: &Path) -> PathBuf {
    support_dir(root).join("versions")
}

/// `<root>/.cadstudio/versions/<number>`.
pub fn version_dir(root: &Path, number: i64) -> PathBuf {
    versions_dir(root).join(number.to_string())
}
