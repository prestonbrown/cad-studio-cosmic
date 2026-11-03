//! Viewer tool state — the port of upstream `ModelViewerState.swift` plus
//! the parts-overlay rules it feeds: one struct holding the toolbar/
//! overlay state behind the 3D pane (grid, wireframe, measure, section,
//! layout, explode, hidden/attached part sets, parts drawer collapse,
//! motion playback) and the small decision rules that don't need the
//! scene: which layout is effective, which layouts the picker offers,
//! whether motion is offered, and the double-click decode for part rows.
//!
//! Pure: no cosmic/iced types, no I/O, part-name keyed (`BTreeSet<String>`)
//! so the pane resolves T1's `groups` to names at the call site. Layout and
//! collapse arrive as constructor arguments — the CALLER reads them from
//! engine settings (`parts_layout`/`parts_collapsed`) and writes them back
//! there per the settings-persistence law; this module never touches the
//! settings file. Scene-side consequences (re-frame on layout change, mesh
//! rebuild on section change) stay with the pane, which owns the camera
//! ([`viewer::camera_input`]) and the scene.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use engine::settings::PartsLayout;
use viewer::playback::MotionPlayer;
use viewer::tools::section::SectionAxis;

/// The explode slider's range (PartsOverlay.swift:28, `Slider(…, in:
/// 0...2)`): 0 assembled-in-place, 2 fully spread.
pub const EXPLODE_RANGE: std::ops::RangeInclusive<f32> = 0.0..=2.0;

/// The part-row double-click window: a documented stand-in for SwiftUI's
/// `onTapGesture(count:2)`, which uses the system's own interval. 500 ms
/// sits inside the range AppKit/SwiftUI use.
pub const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(500);

/// The `ModelViewerState` port. Field defaults per
/// ModelViewerState.swift:21-47: grid on, wireframe/measure/section/
/// animation off, axis X, position mid, explode 1.0, sets empty, player
/// idle; `layout` and `collapsed` come from settings (upstream defaults
/// Exploded / not collapsed).
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
    /// Animation running (`isAnimating`): sectioning/measuring are
    /// excluded and the layout is forced Assembled while it is on —
    /// set through [`Self::set_animating`], which owns the side effects.
    pub animating: bool,
    /// Motion playback state (MotionPlayer.swift:6-9): time, play flag,
    /// speed, loop. Idle at 0, not playing, 1×, looping.
    pub player: MotionPlayer,
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
            animating: false,
            player: MotionPlayer::default(),
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

    /// Animation toggle (ModelViewerState.swift:28-38, `isAnimating`
    /// didSet, verbatim): every toggle — on or off — restarts the clock
    /// at 0 and sets playback to match; turning it ON also drops
    /// sectioning and measuring. Turning it OFF leaves those two alone:
    /// they were already cleared when it went on.
    pub fn set_animating(&mut self, on: bool) {
        self.animating = on;
        self.player.time = 0.0;
        self.player.is_playing = on;
        if on {
            self.sectioning = false;
            self.measuring = false;
        }
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

    /// Double-click on a parts row (PartsOverlay.swift:145-148):
    /// compare-and-assign — assign hidden to every OTHER row's set, unless
    /// it already equals that set, in which case clear (second
    /// double-click restores). After the call hidden is exactly-others or
    /// empty, so the target always ends visible and a partially
    /// eye-hidden state doesn't leak through (a per-row toggle would
    /// resurrect an eye-hidden part).
    pub fn isolate(&mut self, group: &str, all: &[String]) {
        let others: BTreeSet<String> = all
            .iter()
            .filter(|n| n.as_str() != group)
            .cloned()
            .collect();
        if self.hidden == others {
            self.hidden.clear();
        } else {
            self.hidden = others;
        }
    }

    /// The layout the renderer should use (ModelViewerState.swift:78-86):
    /// animating ([`Self::set_animating`]) or comparing forces Assembled,
    /// and Exploded needs at least 2 parts — one part has nothing to
    /// explode away from.
    pub fn effective_layout(&self, parts_count: usize, comparing: bool) -> PartsLayout {
        if self.animating || comparing || (self.layout == PartsLayout::Exploded && parts_count < 2)
        {
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

    /// The Animazione toggle's gate (ModelPane.swift:13-15, :136): motion
    /// is offered iff a study is present AND the model has more than one
    /// part; otherwise the pane shows the ask-the-agent button instead
    /// (:137-144).
    pub fn motion_available(&self, parts_count: usize, study_present: bool) -> bool {
        study_present && parts_count > 1
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
