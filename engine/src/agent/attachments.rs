//! Attachment classification shared engine-side (upstream
//! Chat/Attachments.swift:4-7: `UTType(filenameExtension:)?.conforms(to:
//! .image)`). UTType conformance is a macOS registry lookup; the port rules
//! it to an explicit extension allowlist — the formats the picker and the
//! paste path produce — so the codex argv's `-i` filter and the app's
//! thumbnail widget (M8) cite ONE predicate.

use std::path::Path;

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
