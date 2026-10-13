//! The compare-ghost surface of [`ViewerPane`] — the window-facing half
//! of the compare wiring (the ghost pass itself lives in the viewer
//! crate). An impl extension: same crate, separate file.

use std::sync::Arc;

use crate::viewer_pane::ViewerPane;
use crate::viewer_pane::load_model_parts;
use viewer::render::{PartScene, NEUTRAL};

impl ViewerPane {
    /// Stage or clear the compared version's ghost mesh at the CURRENT
    /// scene pin (`PartScene::from_parts_with_pin`) — upstream hangs the
    /// ghost under content, so only the picker's `None` clears it; model
    /// reloads re-stage via [`Self::restage_ghost`].
    pub fn set_compare(&mut self, version: Option<(i64, std::path::PathBuf)>) {
        match version {
            Some((number, dir)) => {
                let pin = self.scene.scene.pin;
                self.scene.ghost = load_model_parts(&dir.join("output"))
                    .map(|parts| Arc::new(PartScene::from_parts_with_pin(&parts, NEUTRAL, pin)));
                self.comparing = self.scene.ghost.is_some();
                self.ghost_version = if self.comparing { Some((number, dir)) } else { None };
            }
            None => {
                self.scene.ghost = None;
                self.comparing = false;
                self.ghost_version = None;
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
