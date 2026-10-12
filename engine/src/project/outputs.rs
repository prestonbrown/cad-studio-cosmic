//! Output requirements: the notes appended to the agent prompt when
//! model.py does not produce a required output yet, transcribed from the
//! vendored Project.swift:221-243. Everything is an existence check —
//! upstream filters on `FileManager.fileExists`, never size or validity.

use std::path::{Path, PathBuf};

use crate::project::manifest::Manifest;

/// The outputs model.py must produce, in upstream report order.
pub fn expected_outputs() -> [&'static str; 7] {
    [
        "model.3mf",
        "model.stl",
        "model.step",
        "schematic.svg",
        "schematic.png",
        "schematic.pdf",
        "manifest.json",
    ]
}

/// Page count of the PDF at `path`, or the parse error as a string.
pub fn pdf_page_count(path: &Path) -> Result<u32, String> {
    lopdf::Document::load(path)
        .map_err(|e| e.to_string())
        .map(|doc| doc.get_pages().len() as u32)
}

pub struct MissingCtx<'a> {
    /// The project root, where model.py lives: its existence gates
    /// everything (upstream returns [] on the first turn of a fresh
    /// project).
    pub project_root: &'a Path,
    /// The output directory — everything model.py produces sits under it
    /// (contract §85); every file check below reads under this path.
    pub output_dir: &'a Path,
    /// Piece count from the parsed 3MF (upstream `output.parts.count`);
    /// 0 when the model has not been parsed.
    pub parts_count: usize,
    /// The printer configured in settings (upstream `AppSettings.printerModel`).
    pub printer_model: Option<&'a str>,
    /// None when manifest.json is missing or unparseable — the caller
    /// decides and passes None (upstream `output.manifest` is nil either
    /// way); a default() manifest must never be substituted here.
    pub manifest: Option<&'a Manifest>,
}

/// Upstream `ProjectOutput.discoverDrawingPages`: schematic.svg (when
/// present) followed by every `schematic-<n>.svg` in the root, sorted by n.
fn drawing_pages_on_disk(root: &Path) -> Vec<PathBuf> {
    // an unreadable root simply has no pages (upstream `try? ?? []`)
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut numbered: Vec<(i64, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "svg"))
        .filter_map(|path| {
            let stem = path.file_stem()?.to_str()?;
            let number = stem.strip_prefix("schematic-")?.parse::<i64>().ok()?;
            Some((number, path))
        })
        .collect();
    numbered.sort_by_key(|&(number, _)| number);
    let mut pages: Vec<PathBuf> = numbered.into_iter().map(|(_, path)| path).collect();
    let first = root.join("schematic.svg");
    if first.exists() {
        pages.insert(0, first);
    }
    pages
}

/// The missing-output notes, in upstream append order.
pub fn missing_requirements(ctx: &MissingCtx) -> Vec<String> {
    if !ctx.project_root.join("model.py").exists() {
        return Vec::new();
    }
    let mut missing: Vec<String> = expected_outputs()
        .into_iter()
        .filter(|name| !ctx.output_dir.join(name).exists())
        .map(str::to_string)
        .collect();
    // the plate is demanded even for single-piece objects
    if !ctx.output_dir.join("plate.3mf").exists() {
        missing.push("plate.3mf (print plate)".to_string());
    }
    // page 2 only: further pages surface through the drawing-page count below
    if ctx.parts_count > 1 && !ctx.output_dir.join("schematic-2.svg").exists() {
        missing.push(
            "one drawing page per piece (schematic-2.svg, …) with the parts list on page 1"
                .to_string(),
        );
    }
    let drawing_pages = drawing_pages_on_disk(ctx.output_dir);
    // both manifest notes sit inside upstream's `if let manifest`
    if let Some(manifest) = ctx.manifest {
        if drawing_pages.len() > 1 && manifest.drawings.is_empty() {
            missing.push("`drawings` in manifest.json (page titles)".to_string());
        }
        if manifest.bed.is_none() && ctx.printer_model.is_some() {
            missing.push("`bed` in manifest.json (bed size of the user's printer)".to_string());
        }
    }
    // an unreadable or absent pdf counts as 0 pages (upstream `?? 0`); this
    // branch is outside `if let manifest` and fires without one too
    if drawing_pages.len() > 1
        && (pdf_page_count(&ctx.output_dir.join("schematic.pdf")).unwrap_or(0) as usize)
            < drawing_pages.len()
    {
        missing.push("schematic.pdf with all drawing pages in one file (PdfPages)".to_string());
    }
    missing
}
