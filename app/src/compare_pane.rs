//! The compare-ghost surface of [`ViewerPane`] — the window-facing half
//! of the compare wiring (the ghost pass itself lives in the viewer
//! crate). An impl extension: same crate, separate file.

use std::sync::Arc;

use crate::viewer_pane::ViewerPane;
use crate::viewer_pane::load_model_parts;
use viewer::render::{NEUTRAL, PartScene};

impl ViewerPane {
    /// The compare picker's landing (upstream `compare(with:)`,
    /// Project.swift:363-368 + `loadComparison`): stage the chosen
    /// version's MESH as the ghost, or drop the comparison.
    ///
    /// `Some((number, version_dir))` parses `version_dir/output` through
    /// the same 3MF-then-STL seam the live model loads
    /// ([`load_model_parts`]) and stages it with the CURRENT scene's pin —
    /// the ghost floats where the version's model sits relative to the
    /// CURRENT floor, where its own pin would ground it. A version whose
    /// mesh does not parse stages no ghost while the selection still
    /// holds (upstream's `try?` leaves `comparisonMesh` nil,
    /// ProjectOutput.swift:177). `None` clears the ghost and the flag —
    /// the ONLY clear: a mesh reload or a version restore never drops the
    /// comparison.
    ///
    /// Both edges re-arrange through [`Self::set_comparing`]: comparing
    /// forces the Assembled layout (ModelViewerState.swift:78-86), so the
    /// scene collapses under the ghost on enter and re-explodes on clear
    /// — the ghost never floats over stale poses.
    pub fn set_compare(&mut self, version: Option<(i64, std::path::PathBuf)>) {
        match version {
            Some((number, dir)) => {
                self.ghost_version = Some((number, dir));
                self.restage_ghost();
                self.set_comparing(true);
            }
            None => {
                self.scene.ghost = None;
                self.ghost_version = None;
                self.set_comparing(false);
            }
        }
    }

    /// Re-stage the recorded version's ghost at the (possibly new) scene
    /// pin after a model reload.
    pub(crate) fn restage_ghost(&mut self) {
        let Some((_, dir)) = self.ghost_version.clone() else {
            return;
        };
        let pin = self.scene.scene.pin;
        self.scene.ghost = load_model_parts(&dir.join("output"))
            .map(|parts| Arc::new(PartScene::from_parts_with_pin(&parts, NEUTRAL, pin)));
    }
}
