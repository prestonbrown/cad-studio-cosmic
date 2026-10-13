//! Viewer tool state — the port of upstream `ModelViewerState.swift` plus
//! the parts-overlay rules it feeds: one struct holding the toolbar/
//! overlay state behind the 3D pane (grid, wireframe, measure, section,
//! layout, explode, hidden/attached part sets, parts drawer collapse) and
//! the small decision rules that don't need the scene: which layout is
//! effective, which layouts the picker offers, and the double-click decode
//! for part rows.
//!
//! Pure: no cosmic/iced types, no I/O, part-name keyed (`BTreeSet<String>`)
//! so the pane resolves T1's `groups` to names at the call site. Layout and
//! collapse arrive as constructor arguments — the CALLER reads them from
//! engine settings (`parts_layout`/`parts_collapsed`) and writes them back
//! there per the settings-persistence law; this module never touches the
//! settings file. Scene-side consequences (re-frame on layout change, mesh
//! rebuild on section change) stay with the pane, which owns the camera
//! ([`crate::camera_input`]) and the scene.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use engine::settings::PartsLayout;
use viewer::tools::section::SectionAxis;

/// The explode slider's range (ModelViewerState.swift:29,
/// `explodedAmount` 0...2): 0 assembled-in-place, 2 fully spread.
pub const EXPLODE_RANGE: std::ops::RangeInclusive<f32> = 0.0..=2.0;

/// The part-row double-click window: a documented stand-in for SwiftUI's
/// `onTapGesture(count:2)`, which uses the system's own interval. 500 ms
/// sits inside the range AppKit/SwiftUI use.
pub const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(500);

/// The `ModelViewerState` port. Field defaults per
/// ModelViewerState.swift:21-47: grid on, wireframe/measure/section off,
/// axis X, position mid, explode 1.0, sets empty; `layout` and `collapsed`
/// come from settings (upstream defaults Exploded / not collapsed).
#[derive(Debug, Clone)]
pub struct ViewerTools {
    pub shows_grid: bool,
    pub wireframe: bool,
    pub measuring: bool,
    pub sectioning: bool,
    pub section_axis: SectionAxis,
    /// The section plane's slider position, 0..1 along the axis; the
    /// MODEL-space plane value comes from
    /// `viewer::tools::section::plane_value` at render time.
    pub section_position: f32,
    pub layout: PartsLayout,
    pub explode_amount: f32,
    pub hidden: BTreeSet<String>,
    pub attached: BTreeSet<String>,
    pub measure_points: Vec<[f32; 3]>,
    pub collapsed: bool,
}

impl ViewerTools {
    /// ModelViewerState.swift:21-47 defaults; `layout`/`collapsed` are the
    /// settings-read values passed in by the caller.
    pub fn new(layout: PartsLayout, collapsed: bool) -> Self {
        Self {
            shows_grid: true,
            wireframe: false,
            measuring: false,
            sectioning: false,
            section_axis: SectionAxis::X,
            section_position: 0.5,
            layout,
            explode_amount: 1.0,
            hidden: BTreeSet::new(),
            attached: BTreeSet::new(),
            measure_points: Vec::new(),
            collapsed,
        }
    }

    /// Layout change (ModelViewerState.swift:42, `layout` didSet): clears
    /// the in-flight measurement; the caller re-frames the camera —
    /// upstream does that in the same didSet, here the pane owns the pose.
    pub fn set_layout(&mut self, l: PartsLayout) {
        self.layout = l;
        self.measure_points.clear();
    }

    /// Toggle measure mode (ModelViewerState.swift:51-53): any change —
    /// on or off — drops the points collected under the old mode.
    pub fn toggle_measuring(&mut self) {
        self.measuring = !self.measuring;
        self.measure_points.clear();
    }

    /// Section toggle: a plain set; the mesh rebuild it triggers upstream
    /// is the pane's job (it owns the scene pipeline).
    pub fn set_sectioning(&mut self, on: bool) {
        self.sectioning = on;
    }

    /// Record a measure click (ModelViewerState.swift:67-72): two points
    /// make a measurement; the third click starts a new one.
    pub fn add_measure_point(&mut self, p: [f32; 3]) {
        if self.measure_points.len() == 2 {
            self.measure_points.clear();
        }
        self.measure_points.push(p);
    }

    /// Esc in measure mode (clearMeasurement).
    pub fn clear_measurement(&mut self) {
        self.measure_points.clear();
    }

    /// New mesh loaded (ModelViewerState.swift:128): points collected
    /// against the old geometry are meaningless.
    pub fn mesh_changed(&mut self) {
        self.measure_points.clear();
    }

    /// Eye toggle on a parts row: membership flip in the hidden set.
    pub fn toggle_hidden(&mut self, group: &str) {
        if !self.hidden.remove(group) {
            self.hidden.insert(group.to_string());
        }
    }

    /// Attach (OriginsOverlay) toggle: membership flip in the attached set.
    pub fn toggle_attached(&mut self, group: &str) {
        if !self.attached.remove(group) {
            self.attached.insert(group.to_string());
        }
    }

    /// Double-click on a parts row (PartsOverlay.swift:145-148): toggle
    /// every OTHER row's hidden flag — the first click isolates `group`
    /// (all others hidden), the second restores them.
    pub fn isolate(&mut self, group: &str, all: &[String]) {
        for other in all {
            if other != group {
                self.toggle_hidden(other);
            }
        }
    }

    /// The layout the renderer should use (ModelViewerState.swift:78-86):
    /// comparing forces Assembled, and Exploded needs at least 2 parts —
    /// one part has nothing to explode away from. Upstream also forces
    /// Assembled while `isAnimating` (:79); deliberately omitted — M6 has
    /// no animation, and M7's animation work will add it load-bearing.
    pub fn effective_layout(&self, parts_count: usize, comparing: bool) -> PartsLayout {
        if comparing || (self.layout == PartsLayout::Exploded && parts_count < 2) {
            PartsLayout::Assembled
        } else {
            self.layout
        }
    }

    /// The layout picker's choices (PartsOverlay.swift:53-55): Exploded is
    /// only offered with more than one part.
    pub fn available_layouts(parts_count: usize) -> Vec<PartsLayout> {
        if parts_count > 1 {
            vec![
                PartsLayout::Assembled,
                PartsLayout::Exploded,
                PartsLayout::Plate,
            ]
        } else {
            vec![PartsLayout::Assembled, PartsLayout::Plate]
        }
    }
}

/// Is this tap the second click on the same part row inside
/// [`DOUBLE_CLICK_WINDOW`]? The pane keeps the last tap
/// (`Option<(Instant, String)>`) and asks here — the stateless form keeps
/// this module free of the pane's event stream.
pub fn is_double_click(prev: Option<(Instant, String)>, now: Instant, group: &str) -> bool {
    match prev {
        Some((t, g)) => g == group && now.duration_since(t) <= DOUBLE_CLICK_WINDOW,
        None => false,
    }
}
