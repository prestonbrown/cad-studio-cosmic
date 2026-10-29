//! Attachment classification and the reference-copy writer shared
//! engine-side (upstream Chat/Attachments.swift:4-7: `UTType(
//! filenameExtension:)?.conforms(to: .image)`; Project.swift:210-219:
//! `copyReferences`). UTType conformance is a macOS registry lookup; the
//! port rules it to an explicit extension allowlist — the formats the
//! picker and the paste path produce — so the codex argv's `-i` filter and
//! the app's thumbnail widget (M8) cite ONE predicate. The copy is
//! engine-owned but BRIDGE-CALLED: it runs at turn start, before the user
//! row is appended (upstream `copyReferences` inside `send`,
//! Project.swift:199-201), and the engine's session consumes the returned
//! names without writing — one writer of `references/`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Whether `path` names an image by extension: png/jpg/jpeg/tiff/webp/gif/
/// bmp, case-insensitive. Upstream's UTType conformance also admits formats
/// (heic, svg, …) the port's attach surfaces never produce; the allowlist
/// is the recorded ruling, not a decode of the macOS registry.
pub fn is_image(path: &Path) -> bool {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => matches!(
            ext.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "tiff" | "webp" | "gif" | "bmp"
        ),
        None => false,
    }
}

/// Copy the turn's attachment sources into `<root>/references/` as
/// `<unix-seconds>-<index>-<name>` and return the prompt-relative paths
/// (Project.swift:210-219). The index is 1-based, upstream's `index + 1`.
/// Copies that fail are dropped from the list, as upstream's `compactMap`
/// does. Called at TURN START by the bridge (upstream runs it inside
/// `send()`, before the row appends, Project.swift:199-201); a queued send
/// holds its pre-copy paths until its own turn starts
/// (Project.swift:177-190).
pub fn copy_attachments(sources: &[PathBuf], root: &Path) -> Vec<String> {
    if sources.is_empty() {
        return Vec::new();
    }
    let references = root.join("references");
    // the directory is created only when there is something to copy
    let _ = std::fs::create_dir_all(&references);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    sources
        .iter()
        .enumerate()
        .filter_map(|(index, source)| {
            let number = index + 1;
            let name = format!("{stamp}-{number}-{}", source.file_name()?.to_string_lossy());
            std::fs::copy(source, references.join(&name)).ok()?;
            Some(format!("references/{name}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::is_image;
    use std::path::Path;

    #[test]
    fn allowlist_matches_case_insensitively() {
        for name in [
            "sketch.png",
            "sketch.PNG",
            "photo.jpg",
            "photo.Jpeg",
            "scan.tiff",
            "shot.webp",
            "anim.gif",
            "bmp.bmp",
        ] {
            assert!(is_image(Path::new(name)), "{name} is an image");
        }
    }

    #[test]
    fn non_images_and_extensionless_paths_are_not_images() {
        for name in [
            "model.py",
            "notes.txt",
            "render.svg",
            "photo.png.orig",
            "photo",
            "/references/IMG_0001",
            "",
        ] {
            assert!(!is_image(Path::new(name)), "{name} is not an image");
        }
    }
}
