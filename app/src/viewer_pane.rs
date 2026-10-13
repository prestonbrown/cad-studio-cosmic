//! 3D pane — the embedded viewer: [`ViewerPane`] owns the viewer crate's
//! `ModelScene` behind a `mouse_area`, drives it through the pure
//! [`crate::camera_input`] state machine, and reloads the model from the
//! project's `output/` directory through the engine's watcher.
//!
//! Upstream shape (fixtures/upstream-source/Viewer3D/ModelPane.swift): the
//! pane is the shader canvas plus a bottom status bar — the preset picker,
//! the grid/wireframe/measure/section toggles and the color legend left,
//! the size-only info label right (ModelPane.swift:120-152, 215-219;
//! Shared/PaneStatusBar.swift). The bar's toggle state and the overlay
//! family that floats over the canvas (parts panel top-leading, measure
//! badge top-center, section controls bottom-center — [`crate::parts_overlay`])
//! read from [`tools`](ViewerTools), the T8 state hub. MSAA is an OPT-IN
//! pane option (default 1×): the spec §5 decision renders the mesh pass
//! through `MeshPipeline::with_msaa`'s 4× pipeline set, reached by flipping
//! `Options::msaa` (the in-place rebuild — iced's Storage owns one
//! pipeline per type).
//!
//! Output reload: `engine::watch::watch_directory(output, 300 ms)` (upstream
//! 0.3 s), forwarded through the same relay pattern the schematic pane
//! established. The reload is IDEMPOTENT by re-parse-and-replace: a starved
//! burst can split into two watcher emissions, and each one re-parses the
//! directory and REPLACES the scene — identical content short-circuits
//! before the scene `Arc` (and its GPU residency) is touched, so a burst
//! rebuilds once. 3MF is preferred, STL is the fallback.
//!
//! M5: the pane mounts the PARAMETERS OVERLAY (upstream ModelPane's
//! `topTrailing` ParametersOverlay, ModelPane.swift:41-44) over the canvas
//! through a `stack`, showing it only while a model exists, parameters
//! exist, and `parameters_visible` holds (⌥⌘I toggles it — the keyboard
//! shortcut lives in this pane's subscription until the shell owns
//! commands). The pane owns the overlay's widget state ([`parameters`]
//! rows, field drafts, the committed values commits round against); the
//! SHELL owns everything the bridge knows — it feeds
//! [`ViewerPane::sync_parameters`] on every parameter refresh and
//! intercepts [`Message::ParameterCommit`] (→ `Bridge::set_parameter`),
//! `Message::Parameters(parameters_view::Message::Collapse)` (persist
//! `Settings.parameters_collapsed`, then forward), and `…::Compare` (the
//! compare wiring). The shell passes the rest — comparison values, busy,
//! versions, compare selection — at view time via
//! [`parameters_view::OverlayContext`].
//!
//! M6 measure (ModelPane's `onPick` flow): while the tool measures, a
//! primary-button CLICK (release under 4 px of travel) casts
//! [`camera::pick_ray`] at the release cursor and runs
//! [`picking::first_hit`] on a named std thread — click-driven and
//! off-thread per spec §5, never the cosmic executor — with the answer
//! returning as [`Message::Picked`] through the pane's relay, the same
//! channel reloads and resizes ride. One pick is in flight at a time; a
//! click during a pending pick is dropped. The landed point joins
//! `tools.measure_points`, restages the orange markers
//! ([`ViewerPane::rebuild_markers`], the single writer), and Esc clears
//! the measurement (ModelPane.swift:54-58 — ignored otherwise).
//!
//! M4 status: this module is the pane model + view; the shell task wires it
//! into a window (pane focus commands, i18n via the extracted ftl keys all
//! land there). User-visible strings are plain English pending that wiring,
//! matching the schematic pane's documented M4 state.
//!
//! M6: the pane owns the ARRANGEMENT wiring — [`ViewerTools`] (the
//! ModelViewerState port) drives `viewer::tools::arrangement`, and
//! [`ViewerPane::apply_arrangement`] pushes the answer as per-part pose
//! uniforms (explode offsets, hidden visibility) without touching staged
//! bytes; the agent's `plate.3mf` swaps in a staged plate scene (the bed
//! outline riding the grid in plate view), the grid itself FREEZES at
//! scene/layout/plate-identity changes (upstream rebuilds on sceneID, never
//! on explode/hidden), the section plane follows the effective aabb through
//! the live scene pin, and a layout change re-frames at the current preset
//! against the CURRENT arranged aabb (a forced framing reference, not the
//! reload gate). `parts_layout`/`parts_collapsed` persist through the
//! pane's settings path — [`ViewerPane::new_with_settings`] is the test
//! seam that keeps that off the developer's real config.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cosmic::Element;
use cosmic::iced::futures::StreamExt;
use cosmic::iced::futures::stream::BoxStream;
use cosmic::iced::keyboard::{self, Modifiers};
use cosmic::iced::mouse::ScrollDelta;
use cosmic::iced::stream;
use cosmic::iced::widget::{Stack, responsive, shader};
use cosmic::iced::{Length, Point, Size, Subscription, Task};
use cosmic::widget::{Column, Row, Space, button, container, mouse_area, text, tooltip};
use engine::parsing::Part;
use engine::parsing::stl::parse_stl;
use engine::parsing::threemf::parse_3mf;
use engine::project::manifest::Manifest;
use engine::project::outputs::{self, MissingCtx};
use engine::project::parameters::ModelParameter;
use engine::settings::{MeasurementUnit, PartsLayout, Settings};
use engine::watch::{ReloadKind, WatchHandle, watch_directory};
use viewer::render::markers;
use viewer::render::{ModelScene, NEUTRAL, PartPose, PartScene, camera, camera::Preset};
use viewer::tools::arrangement::{self, PartGroup};
use viewer::tools::picking;
use viewer::tools::section::{self, SectionAxis};

use crate::camera_input::CameraInput;
use crate::fl;
use crate::parameters::ParametersPanel;
use crate::parameters_view::{self, OverlayContext};
use crate::parts_overlay;
use crate::viewer_tools::{EXPLODE_RANGE, ViewerTools, is_double_click};

/// Output watcher debounce (upstream FileWatcher's 0.3 s).
pub const WATCH_DEBOUNCE: Duration = Duration::from_millis(300);
/// Relay retry cadence when the stream channel is momentarily full (same
/// policy as the schematic pane's raster relay).
pub const RELAY_RETRY_MIN: Duration = Duration::from_millis(1);
pub const RELAY_RETRY_MAX: Duration = Duration::from_millis(64);
/// Total time to keep retrying one full-channel message before dropping it
/// (liveness over completeness; the next event re-derives dropped state).
pub const RELAY_RETRY_BUDGET: Duration = Duration::from_secs(2);

/// The pane's messages. Pane focus and viewer preset/fit menu commands are
/// the shell's on top of these.
#[derive(Clone, Debug)]
pub enum Message {
    /// Primary-button press; pan vs orbit is decided from the modifier
    /// state at update time (upstream gates on option/shift,
    /// CameraInputView.swift:43-47).
    Pressed,
    RightPressed,
    MiddlePressed,
    Released,
    Moved(Point),
    Scrolled(ScrollDelta),
    /// A trackpad pinch. No platform event reaches this today (iced's winit
    /// shell translates no magnify gesture); the variant exists so the
    /// parity formula in [`CameraInput::magnify`] is wired the day one
    /// does — facts #116, this task's carry-forward.
    Magnified(f32),
    /// The keyboard modifier set changed (iced emits this on every change;
    /// mouse_area messages carry no modifiers of their own).
    ModifiersChanged(Modifiers),
    Preset(Preset),
    Fit,
    ToggleMsaa,
    /// A watched output file settled (engine watcher → relay).
    Reload(ReloadKind),
    /// The canvas area's size, reported from layout — iced has no resize
    /// event for panes.
    Resized(Size),
    /// A parameters-overlay interaction (see [`parameters_view`]): the pane
    /// owns the widget state; the SHELL intercepts `Collapse` (persist, then
    /// forward) and `Compare` (the bridge wiring) before these arrive.
    Parameters(parameters_view::Message),
    /// A parameter commit settled — delivered via `Task` from the overlay's
    /// release/submit path. The SHELL drives `Bridge::set_parameter` from
    /// it; the pane's own arm is a no-op (the local half — value snap,
    /// draft clear — already applied when it was emitted).
    ParameterCommit(String, f64),
    /// ⌥⌘I — toggles the parameters overlay's visibility
    /// (Commands.swift:69-73); the pane's subscription reports it until the
    /// shell owns menu commands.
    ToggleParameters,
    /// A measure pick settled (the viewer-pick thread → relay): the
    /// MODEL-space hit, or `None` when the ray missed everything.
    Picked(Option<[f32; 3]>),
    /// Esc — clears an in-flight measurement (ModelPane.swift:54-58);
    /// ignored when the tool is not measuring.
    Escape,
    // ── M6 tool controls (the widgets land with the parts-overlay task;
    // the pane arms the semantics) ────────────────────────────────────────
    /// Upstream `showsGrid` (facts #38): the parity grid overlay.
    ToggleGrid,
    /// Upstream `showsWireframe` (facts #44).
    ToggleWireframe,
    /// The measure tool's mode toggle — points clear on any change
    /// (ModelViewerState.swift:51-53); the click flow is the measure task's.
    ToggleMeasure,
    /// Upstream `sectioning`: on/off for the GPU section plane.
    ToggleSection,
    /// The section plane's axis (upstream `sectionAxis`).
    SetSectionAxis(SectionAxis),
    /// The section plane's slider position, 0..1 along the axis.
    SetSectionPosition(f32),
    /// The layout picker (upstream `layout` didSet — the pane re-arranges,
    /// persists, and re-frames).
    SetLayout(PartsLayout),
    /// The explode slider, 0..=2 (PartsOverlay.swift:28).
    SetExplode(f32),
    /// Eye toggle on a parts row: hide/show one group.
    ToggleHiddenGroup(String),
    /// Attach toggle on a parts row (OriginsOverlay): keep a group at its
    /// assembled origin through explodes.
    ToggleAttachedGroup(String),
    /// A parts row was clicked — a double-click isolates the group
    /// (PartsOverlay.swift:145-148).
    RowClicked(String),
    /// The parts drawer's collapse chevron (persisted as
    /// `Settings.parts_collapsed`).
    TogglePartsCollapsed,
}

/// Events crossing the pane's std channel into the subscription: watcher
/// reloads (bridged from the engine's own channel), the canvas size
/// reports, and the measure picks' answers (the viewer-pick thread).
enum PaneEvent {
    Reload(ReloadKind),
    Resized(Size),
    Picked(Option<[f32; 3]>),
}

/// The PaneEvent → Message translation every consumer of the relay uses
/// (the subscription's drain thread and the tests' pump).
fn relay_message(event: PaneEvent) -> Message {
    match event {
        PaneEvent::Reload(kind) => Message::Reload(kind),
        PaneEvent::Resized(size) => Message::Resized(size),
        PaneEvent::Picked(hit) => Message::Picked(hit),
    }
}

/// The subscription's identity + the event receiver slot it takes once
/// (same pattern as the schematic pane's raster relay).
struct OutputRelay {
    id: u64,
    events: Mutex<Option<Receiver<PaneEvent>>>,
}

impl OutputRelay {
    fn take_events(&self) -> Option<Receiver<PaneEvent>> {
        self.events.lock().unwrap().take()
    }

    /// Seat a receiver back after [`OutputRelay::take_events`] — the test
    /// pump's borrow-and-return; production takes exactly once.
    #[cfg(test)]
    fn restore_events(&self, events: Receiver<PaneEvent>) {
        *self.events.lock().unwrap() = Some(events);
    }
}

impl std::hash::Hash for OutputRelay {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// The 3D pane state.
pub struct ViewerPane {
    /// The embedded viewer scene (pose pushed from `input` after every
    /// event; the pane never mutates the pose fields directly).
    pub scene: ModelScene,
    /// The camera state machine — the single owner of the pose.
    pub input: CameraInput,
    /// The preset picker's selection (upstream keeps the last picked).
    pub preset: Preset,
    /// MSAA opt-in (scope binding): false renders 1× (the default), true
    /// flips `Options::msaa` to 4 — `MeshPipeline::with_msaa`'s pipeline
    /// set, rebuilt in place.
    pub msaa: bool,
    /// Display unit for the status-bar size label.
    pub unit: MeasurementUnit,
    /// Missing-output notes count for the status bar (from
    /// [`outputs::missing_requirements`], refreshed on every reload).
    pub warnings: usize,
    /// The parameters overlay's row state (upstream `project.parameters` →
    /// sliders). Synced from the bridge's parameters via
    /// [`ViewerPane::sync_parameters`]; `collapsed` initializes from
    /// `Settings.parameters_collapsed`.
    pub parameters: ParametersPanel,
    /// Live text-field drafts, one per parameter name (the field commits on
    /// submit; the raw text needs to survive between frames).
    pub parameter_drafts: HashMap<String, String>,
    /// The last-synced committed parameter values — what `commit` rounds
    /// against and what the previous-value comparison reads.
    committed_parameters: BTreeMap<String, f64>,
    /// Whether the parameters overlay shows at all (upstream
    /// `areParametersVisible`, Project.swift:71 — default true; ⌥⌘I
    /// toggles).
    pub parameters_visible: bool,
    // ── M6 arrangement state ──────────────────────────────────────────────
    /// The toolbar/overlay state behind the pane (the ModelViewerState
    /// port, T8).
    pub tools: ViewerTools,
    /// The last cursor position the pane saw (iced top-down, canvas-local)
    /// — the aim a measure click casts its pick ray through. Hover moves
    /// reach it too, so a press without any drag still knows where it
    /// happened.
    last_cursor: Point,
    /// Which button the open press started with: the pane funnels
    /// right/middle releases into [`Message::Released`] the same as
    /// primary ones, and upstream picks on LEFT mouseUp only
    /// (CameraInputView.swift:50-53) — the press kind is the gate.
    last_press_primary: bool,
    /// A measure pick is in flight on the viewer-pick thread. Single
    /// in-flight: a click while this is set is dropped, and the flag
    /// clears when the pick reports back.
    pick_pending: bool,
    // Comparison state (the compare-ghost task owns the full machinery;
    // this is the window-facing surface project.rs calls)
    pub(crate) ghost_version: Option<(i64, std::path::PathBuf)>,
    /// The parsed MODEL parts — the arrangement's input and the grouping
    /// source, kept so re-arranging never re-parses.
    pub model_parts: Vec<Part>,
    /// The cached staged MODEL scene (`model_parts` staged once per reload;
    /// `scene.scene` swaps between it and the plate scene).
    model_scene: Arc<PartScene>,
    /// The agent's parsed plate parts (empty until a plate.3mf loads).
    pub plate_parts: Vec<Part>,
    /// Whether `plate.3mf` parsed into parts (the Plate layout's
    /// plate-scene gate; the flat fallback draws otherwise).
    pub plate_loaded: bool,
    /// The staged plate scene while it is (or was last) the drawn one —
    /// rebuilt only when its inputs move (plate reload, hidden filter).
    plate_scene_cache: Option<Arc<PartScene>>,
    /// Compare mode (the shell/T12 owns the comparison): comparing forces
    /// the Assembled layout (ModelViewerState.swift:78-86).
    pub comparing: bool,
    /// The settings file this pane reads and writes its persisted keys
    /// through (`parts_layout`, `parts_collapsed`) — the real global path
    /// in production; tests inject a temp path
    /// ([`ViewerPane::new_with_settings`]) so parallel pane tests never
    /// race the developer's config.
    pub settings_path: PathBuf,
    /// The effective MODEL aabb of the drawn arrangement (the section
    /// plane's domain; plate-derived in plate view, zero-offset at the
    /// all-hidden fallback).
    pub effective_aabb: ([f32; 3], [f32; 3]),
    /// The scene-space aabb of what is drawn — the re-frame input
    /// ([`ViewerPane::set_layout`] pushes it as the forced framing
    /// reference).
    pub frame_aabb: ([f32; 3], [f32; 3]),
    /// The FROZEN grid box (MODEL): captured only when `grid_stamp` moves,
    /// re-fed verbatim afterwards — upstream rebuilds the grid on sceneID,
    /// never on explode/hidden (ModelViewer.swift:165-170).
    pub grid_aabb: ([f32; 3], [f32; 3]),
    /// The bed the overlay was last stamped with (plate layout only).
    pub grid_bed: Option<[f64; 2]>,
    /// (scene Arc identity, effective layout, plate_loaded) — the
    /// sceneID stand-in that gates `grid_aabb` recapture.
    grid_stamp: Option<(usize, PartsLayout, bool)>,
    /// The last parts-row click (Instant, group) — the double-click window.
    last_row_click: Option<(Instant, String)>,
    project_root: PathBuf,
    output_dir: PathBuf,
    printer_model: Option<String>,
    manifest: Option<Manifest>,
    modifiers: Modifiers,
    /// Keeps the platform watch alive; dropped with the pane.
    watch: Option<WatchHandle>,
    events: Sender<PaneEvent>,
    relay: Arc<OutputRelay>,
    last_size: Mutex<Size>,
}

impl ViewerPane {
    /// The pane over `project_root`'s outputs (`root/output`, the Task 7
    /// `create_project` scaffolding guarantee): loads whatever model is
    /// already there, starts the 300 ms watcher, and computes the status
    /// bar's warning count. A missing output directory only means no watch
    /// (and no model) — the pane still works. Settings persist through the
    /// global settings file.
    pub fn new(project_root: &Path, unit: MeasurementUnit, printer_model: Option<String>) -> Self {
        Self::new_with_settings(
            project_root,
            unit,
            printer_model,
            engine::settings::settings_path(),
        )
    }

    /// [`ViewerPane::new`] with an explicit settings path — the pane's
    /// persisted keys (`parts_layout`, `parts_collapsed`) load from it at
    /// construction and save back through it on change (the ONE-writer
    /// rule's path half). Tests inject a per-project temp file; production
    /// goes through [`ViewerPane::new`]'s global default.
    pub fn new_with_settings(
        project_root: &Path,
        unit: MeasurementUnit,
        printer_model: Option<String>,
        settings_path: PathBuf,
    ) -> Self {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let output_dir = project_root.join("output");
        let (events, receiver) = channel::<PaneEvent>();
        // The engine watcher delivers into its own channel; a bridge thread
        // folds it into the pane's single event channel so the relay below
        // drains one receiver. Dropping the pane drops the WatchHandle (the
        // engine channel disconnects, the bridge ends) and the event
        // senders (the relay ends).
        let watch = watch_directory(&output_dir, WATCH_DEBOUNCE)
            .ok()
            .map(|(handle, reloads)| {
                let events = events.clone();
                let _ = thread::Builder::new()
                    .name("viewer-watch-bridge".into())
                    .spawn(move || {
                        while let Ok(kind) = reloads.recv() {
                            if events.send(PaneEvent::Reload(kind)).is_err() {
                                return; // pane gone
                            }
                        }
                    });
                handle
            });
        let empty = Arc::new(PartScene::from_parts(&[], NEUTRAL));
        let settings = Settings::load(&settings_path).unwrap_or_default();
        let mut pane = ViewerPane {
            scene: ModelScene::new(empty.clone()),
            input: CameraInput::new([0.0; 3], [0.0; 3]),
            preset: Preset::Iso,
            msaa: false,
            unit,
            warnings: 0,
            parameters: ParametersPanel::from_parameters(&[], false),
            parameter_drafts: HashMap::new(),
            committed_parameters: BTreeMap::new(),
            parameters_visible: true,
            tools: ViewerTools::new(settings.parts_layout, settings.parts_collapsed),
            last_cursor: Point::ORIGIN,
            last_press_primary: false,
            pick_pending: false,
            comparing: false,
            ghost_version: None,
            model_parts: Vec::new(),
            model_scene: empty,
            plate_parts: Vec::new(),
            plate_loaded: false,
            plate_scene_cache: None,
            settings_path,
            effective_aabb: ([0.0; 3], [0.0; 3]),
            frame_aabb: ([0.0; 3], [0.0; 3]),
            grid_aabb: ([0.0; 3], [0.0; 3]),
            grid_bed: None,
            grid_stamp: None,
            last_row_click: None,
            project_root: project_root.to_path_buf(),
            output_dir,
            printer_model,
            manifest: None,
            modifiers: Modifiers::empty(),
            watch,
            events,
            relay: Arc::new(OutputRelay {
                id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                events: Mutex::new(Some(receiver)),
            }),
            last_size: Mutex::new(Size::default()),
        };
        pane.reload_outputs();
        pane
    }

    /// Upstream's overlay gates (ModelPane.swift:21-45): every one needs a
    /// model rendering; the parts panel also needs more than one part or
    /// plate parts to speak of.
    fn shows_measure_badge(&self) -> bool {
        self.tools.measuring && !self.scene.scene.parts.is_empty()
    }

    fn shows_parts_panel(&self) -> bool {
        !self.scene.scene.parts.is_empty()
            && (self.model_parts.len() > 1 || !self.plate_parts.is_empty())
    }

    fn shows_section_controls(&self) -> bool {
        self.tools.sectioning && !self.scene.scene.parts.is_empty()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Pressed => {
                self.last_press_primary = true;
                self.input.press(self.pan_modifiers());
                self.sync_pose();
            }
            Message::RightPressed | Message::MiddlePressed => {
                self.last_press_primary = false;
                self.input.press(true);
                self.sync_pose();
            }
            Message::Released => {
                // a click (< 4 px travel) while measuring fires the pick;
                // the pane funnels right/middle releases here too, so the
                // PRESS kind gates (upstream picks on left mouseUp only,
                // CameraInputView.swift:50-53), and one pick at a time —
                // a click during a pending pick is dropped
                let was_click = self.input.release();
                if was_click
                    && self.last_press_primary
                    && self.tools.measuring
                    && !self.pick_pending
                {
                    self.spawn_pick();
                }
            }
            Message::Moved(point) => {
                self.last_cursor = point;
                self.input.move_to(point.x, point.y);
                self.sync_pose();
            }
            Message::Scrolled(delta) => {
                let (x, y, precise) = match delta {
                    ScrollDelta::Lines { x, y } => (x, y, false),
                    ScrollDelta::Pixels { x, y } => (x, y, true),
                };
                self.input.scroll(x, y, precise, self.pan_modifiers());
                self.sync_pose();
            }
            Message::Magnified(m) => {
                self.input.magnify(m);
                self.sync_pose();
            }
            Message::ModifiersChanged(modifiers) => self.modifiers = modifiers,
            Message::Preset(preset) => {
                self.preset = preset;
                self.input.preset(preset);
                self.sync_pose();
            }
            Message::Fit => {
                self.input.fit();
                self.sync_pose();
            }
            Message::ToggleMsaa => {
                self.msaa = !self.msaa;
                self.scene.msaa = if self.msaa { 4 } else { 1 };
            }
            Message::Reload(kind) => match kind {
                // model-shaped outputs rebuild the scene; the manifest
                // feeds the status bar's warning count and the plate-view
                // bed. Schematic reloads are the schematic pane's (its own
                // watcher wiring).
                ReloadKind::Mesh | ReloadKind::Manifest => self.reload_outputs(),
                ReloadKind::Plate => self.reload_plate(),
                ReloadKind::Motion | ReloadKind::Schematic => {}
            },
            Message::Resized(size) => {
                // the relay delivers what report_size already deduped
                // through last_size; recording it here keeps the message
                // self-consistent — anything that drives Resized directly
                // aims picks at the right viewport
                *self.last_size.lock().unwrap() = size;
                self.input.set_view_height(size.height);
            }
            Message::ToggleParameters => {
                self.parameters_visible = !self.parameters_visible;
            }
            Message::Picked(hit) => {
                self.pick_pending = false;
                // the flight can outlive the mode it was fired under (Esc,
                // a toggle mid-pick): a stale pick never lands
                if let (true, Some(p)) = (self.tools.measuring, hit) {
                    self.tools.add_measure_point(p);
                    self.rebuild_markers();
                }
            }
            Message::Escape => {
                if self.tools.measuring {
                    self.tools.clear_measurement();
                    self.rebuild_markers();
                }
            }
            Message::ToggleGrid => {
                self.tools.shows_grid = !self.tools.shows_grid;
                self.scene.show_grid = self.tools.shows_grid;
            }
            Message::ToggleWireframe => {
                self.tools.wireframe = !self.tools.wireframe;
                self.scene.wireframe = self.tools.wireframe;
            }
            Message::ToggleMeasure => {
                // any change drops the collected points
                // (ModelViewerState.swift:51-53), markers with them
                self.tools.toggle_measuring();
                self.rebuild_markers();
            }
            Message::ToggleSection => {
                self.tools.set_sectioning(!self.tools.sectioning);
                self.apply_arrangement();
            }
            Message::SetSectionAxis(axis) => {
                self.tools.section_axis = axis;
                self.apply_arrangement();
            }
            Message::SetSectionPosition(position) => {
                // the slider's own range is SLIDER_MIN..=SLIDER_MAX; a
                // direct message clamps to the same window so the cut
                // always keeps geometry (ModelPane.swift:191)
                self.tools.section_position =
                    position.clamp(section::SLIDER_MIN, section::SLIDER_MAX);
                self.apply_arrangement();
            }
            Message::SetLayout(layout) => self.set_layout(layout),
            Message::SetExplode(amount) => {
                self.tools.explode_amount =
                    amount.clamp(*EXPLODE_RANGE.start(), *EXPLODE_RANGE.end());
                self.apply_arrangement();
            }
            Message::ToggleHiddenGroup(group) => {
                self.tools.toggle_hidden(&group);
                // the plate scene stages the hidden filter, so its cache
                // went stale with the set
                self.plate_scene_cache = None;
                self.apply_arrangement();
            }
            Message::ToggleAttachedGroup(group) => {
                self.tools.toggle_attached(&group);
                self.apply_arrangement();
            }
            Message::RowClicked(group) => {
                let now = Instant::now();
                let double = is_double_click(self.last_row_click.take(), now, &group);
                self.last_row_click = Some((now, group.clone()));
                if double {
                    let names = self
                        .groups()
                        .into_iter()
                        .map(|g| g.name)
                        .collect::<Vec<_>>();
                    self.tools.isolate(&group, &names);
                    self.plate_scene_cache = None;
                    self.apply_arrangement();
                }
            }
            Message::TogglePartsCollapsed => {
                self.tools.collapsed = !self.tools.collapsed;
                self.persist_setting(|s| s.parts_collapsed = self.tools.collapsed);
            }
            // The shell acts on commits; the pane's local half (value snap,
            // draft clear) was applied when the message was emitted.
            Message::ParameterCommit(_, _) => {}
            Message::Parameters(message) => return self.update_parameters(message),
        }
        Task::none()
    }

    /// The pane's subscriptions: the output-event relay plus keyboard
    /// modifier changes (the pan-vs-zoom gate for scroll and primary-drag),
    /// the parameters overlay's ⌥⌘I toggle, and Esc (clears an in-flight
    /// measurement, ModelPane.swift:54-58).
    pub fn subscription(&self) -> Subscription<Message> {
        let relay = Subscription::run_with(self.relay.clone(), relay_stream);
        let modifiers = keyboard::listen().filter_map(|event| match event {
            keyboard::Event::ModifiersChanged(modifiers) => {
                Some(Message::ModifiersChanged(modifiers))
            }
            // ⌥⌘I (Commands.swift:69-73); the character form matches the
            // logical key with option held (macOS composes ⌥I — the
            // ignoring-modifiers key stays "i").
            keyboard::Event::KeyPressed { key, modifiers, .. }
                if modifiers.alt()
                    && modifiers.logo()
                    && matches!(&key, keyboard::Key::Character(c) if c.eq_ignore_ascii_case("i")) =>
            {
                Some(Message::ToggleParameters)
            }
            keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            } => Some(Message::Escape),
            _ => None,
        });
        Subscription::batch([relay, modifiers])
    }

    // ── parameters overlay (M5) ──────────────────────────────────────────

    /// Follow the bridge's parameter refresh: committed values update
    /// (clearing any draft whose value changed — upstream's bound field
    /// text follows `parameter.value`), and the panel rows sync (keeping
    /// mid-drag edits, re-fitting only escaped ranges).
    /// Seed the overlay's collapse state from the persisted setting —
    /// the window hands it in at open ([`Settings::parameters_collapsed`];
    /// the field's own doc). `sync` preserves it across parameter refreshes.
    pub fn set_parameters_collapsed(&mut self, collapsed: bool) {
        self.parameters.collapsed = collapsed;
    }

    pub fn sync_parameters(&mut self, params: &[ModelParameter]) {
        for param in params {
            if self.committed_parameters.get(&param.name) != Some(&param.value) {
                self.parameter_drafts.remove(&param.name);
            }
        }
        self.committed_parameters = params
            .iter()
            .map(|param| (param.name.clone(), param.value))
            .collect();
        // a removed parameter's row is gone; its draft goes with it
        self.parameter_drafts
            .retain(|name, _| self.committed_parameters.contains_key(name));
        self.parameters.sync(params);
    }

    /// One overlay message. `Release`/`Submit` return the settled commit
    /// (if any) as a [`Message::ParameterCommit`] task for the shell to
    /// intercept.
    fn update_parameters(&mut self, message: parameters_view::Message) -> Task<Message> {
        match message {
            parameters_view::Message::Drag(name, value) => {
                if let Some((_, state)) = self.slider_mut(&name) {
                    state.value = value;
                    state.editing = true;
                }
            }
            parameters_view::Message::Release(name) => {
                if let Some((name, value)) = self.commit_parameter(&name) {
                    return Task::done(Message::ParameterCommit(name, value));
                }
            }
            parameters_view::Message::Draft(name, text) => {
                // The thumb follows valid field text live (upstream's
                // binding); the raw string stages until submit.
                if let Ok(value) = text.parse::<f64>() {
                    if let Some((_, state)) = self.slider_mut(&name) {
                        state.value = value;
                    }
                }
                self.parameter_drafts.insert(name, text);
            }
            parameters_view::Message::Submit(name) => {
                if let Some((name, value)) = self.submit_parameter(&name) {
                    return Task::done(Message::ParameterCommit(name, value));
                }
            }
            parameters_view::Message::Collapse(collapsed) => {
                self.parameters.collapsed = collapsed;
            }
            parameters_view::Message::Close => self.parameters_visible = false,
            // The shell owns compare state; the picker's report stops here.
            parameters_view::Message::Compare(_) => {}
            // The busy-gated slider's placeholder (parameters_view docs).
            parameters_view::Message::Inert => {}
        }
        Task::none()
    }

    fn slider_mut(&mut self, name: &str) -> Option<&mut (String, crate::parameters::SliderState)> {
        self.parameters.sliders.iter_mut().find(|(n, _)| n == name)
    }

    /// The release path: end the drag, commit against the parameter's
    /// committed value, and apply the local half (the value snaps and the
    /// range re-fits when the commit escaped it — together the round-trip
    /// of upstream's in-commit refit plus onChange). The returned pair is
    /// what the shell forwards to the bridge.
    fn commit_parameter(&mut self, name: &str) -> Option<(String, f64)> {
        let current = *self.committed_parameters.get(name)?;
        let (_, state) = self.slider_mut(name)?;
        state.editing = false;
        let value = state.commit(current)?;
        state.sync_from(value);
        Some((name.to_string(), value))
    }

    /// The field's submit path: stage the draft's value (an unparseable
    /// one just clears — upstream's numeric formatter never stages one),
    /// then the release path's commit. A submit with NO draft commits the
    /// live value, like upstream's field bound to `$value`.
    fn submit_parameter(&mut self, name: &str) -> Option<(String, f64)> {
        if let Some(draft) = self.parameter_drafts.remove(name) {
            if let Some(value) = draft.parse::<f64>().ok().filter(|value| value.is_finite()) {
                if let Some((_, state)) = self.slider_mut(name) {
                    state.value = value;
                }
            }
        }
        self.commit_parameter(name)
    }

    /// The 3D pane's element: the shader canvas in a mouse area with the
    /// overlay family stacked over it, over the status bar (upstream
    /// ModelPane's VStack: content then PaneStatusBar, ModelPane.swift:17-49).
    pub fn view_model_3d(&self, overlay: Option<OverlayContext<'_>>) -> Element<'_, Message> {
        let content: Element<'_, Message> = if self.scene.scene.parts.is_empty() {
            Column::new()
                .push(text("No model").size(18))
                .push(text("Describe an object in the chat to get started"))
                .spacing(8)
                .align_x(cosmic::iced::alignment::Alignment::Center)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            self.view_canvas()
        };
        // The overlay stack, in upstream's z-order: measure badge top,
        // parts panel top-leading, parameters top-trailing, section bottom.
        let mut stack = Stack::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(content);
        if self.shows_measure_badge() {
            let badge = parts_overlay::view_measure_badge(&self.tools.measure_points, self.unit);
            stack = stack.push(top_center(container(badge).padding(10)));
        }
        if self.shows_parts_panel() {
            let groups = arrangement::groups(&self.model_parts);
            let effective = self
                .tools
                .effective_layout(self.model_parts.len(), self.comparing);
            let panel = parts_overlay::view_parts_panel(
                &self.tools,
                &groups,
                effective,
                !self.plate_parts.is_empty(),
                self.comparing,
            );
            stack = stack.push(
                Row::new()
                    .push(container(panel).padding([32.0, 0.0, 0.0, 10.0]))
                    .push(Space::new().width(Length::Fill))
                    .width(Length::Fill),
            );
        }
        if let Some(ctx) = overlay.filter(|_| self.show_parameters_overlay()) {
            let parameters = parameters_view::view_parameters(
                &self.parameters,
                &self.parameter_drafts,
                &self.committed_parameters,
                &ctx,
            )
            .map(Message::Parameters);
            let positioned = Row::new()
                .push(Space::new().width(Length::Fill))
                .push(container(parameters).padding(10))
                .width(Length::Fill);
            stack = stack.push(positioned);
        }
        if self.shows_section_controls() {
            let controls = parts_overlay::view_section_controls(&self.tools);
            stack = stack.push(bottom_center(container(controls).padding(10)));
        }
        Column::new()
            .push(stack)
            .push(self.view_status_bar())
            .into()
    }

    /// Upstream's overlay gate: visible AND parameters exist AND a model
    /// renders (ModelPane.swift:41-44).
    fn show_parameters_overlay(&self) -> bool {
        self.parameters_visible
            && !self.parameters.sliders.is_empty()
            && !self.scene.scene.parts.is_empty()
    }

    /// Re-read everything model-shaped from `output/` and replace the model
    /// scene — idempotent by construction (see the module docs): the change
    /// check runs against the cached MODEL scene, never the drawn one (in
    /// plate view the drawn scene is the plate's, and identical model
    /// content must not masquerade as a change). The >50 % re-frame gate
    /// stays the RELOAD rule ([`CameraInput::mesh_changed`]); the
    /// arrangement then re-applies over the new parts (which is also what
    /// swaps the drawn scene when a plate view is active). Also refreshes
    /// the manifest, the status bar's warning count, and drops measure
    /// points collected against the old geometry.
    fn reload_outputs(&mut self) {
        self.manifest = Manifest::load(&self.output_dir.join("manifest.json")).ok();
        if let Some(parts) = load_model_parts(&self.output_dir) {
            let next = Arc::new(PartScene::from_parts(&parts, NEUTRAL));
            if scenes_differ(&self.model_scene, &next) {
                self.model_parts = parts;
                self.input.mesh_changed(next.aabb_min, next.aabb_max);
                self.tools.mesh_changed();
                // points collected against the old geometry are gone
                // (ModelViewerState.swift:128) — their markers go with them
                self.rebuild_markers();
                self.model_scene = next;
            }
        }
        self.apply_arrangement();
        // after the arrangement: the drawn scene (a plate view holds the
        // plate's parts) is what the warning count describes
        self.refresh_warnings();
    }

    /// Parse `output/plate.3mf` (the agent's print plate) and re-arrange: a
    /// well-formed parse REPLACES the plate outright — empty parts
    /// included; an empty plate is a plate with nothing on it, so the flat
    /// fallback lays the model out instead. An unreadable or unparseable
    /// file keeps the LAST GOOD plate (the pane's established model law).
    fn reload_plate(&mut self) {
        if let Ok(bytes) = std::fs::read(self.output_dir.join("plate.3mf")) {
            if let Ok(parts) = parse_3mf(&bytes) {
                self.plate_loaded = !parts.is_empty();
                self.plate_parts = parts;
                self.plate_scene_cache = None;
                self.apply_arrangement();
            }
        }
    }

    // ── arrangement wiring (M6) ───────────────────────────────────────────

    /// THE pose push (ModelPane.swift:70-85's port): recompute the
    /// arrangement for the current tools state, select the drawn scene
    /// (model vs the agent's plate), write per-part pose uniforms, the
    /// effective aabbs, the frozen grid stamp, and the section uniform.
    /// Called after every layout, explode, hidden, attached, section,
    /// comparing, and scene change; it never moves the camera by itself —
    /// only [`Self::set_layout`] and a scene swap touch the framing
    /// reference.
    fn apply_arrangement(&mut self) {
        let layout = self
            .tools
            .effective_layout(self.model_parts.len(), self.comparing);
        let arranged = arrangement::arrange(
            &self.model_parts,
            &self.plate_parts,
            layout,
            self.tools.explode_amount,
            &self.tools.hidden,
            &self.tools.attached,
        );
        let all_hidden = !arranged.visible.iter().any(|v| *v);
        // ModelPane.swift:78-80: an arrangement whose visible set is empty
        // draws the assembled MODEL (`output.mesh`), never the plate — the
        // model scene stages ahead of the plate whenever every group hides.
        let draw_plate = arranged.plate_scene && !all_hidden;

        // Scene selection: the plate replaces the model when the
        // arrangement calls for it, hidden-filtered by group like the model
        // slots. The caches keep a scene swap off the explode/hidden sweep
        // path — those write poses only.
        let mut swapped = false;
        if draw_plate {
            if self.plate_scene_cache.is_none() {
                self.plate_scene_cache = Some(Arc::new(PartScene::from_parts(
                    &self.visible_plate_parts(),
                    NEUTRAL,
                )));
            }
            let plate = self.plate_scene_cache.clone().expect("built above");
            if !Arc::ptr_eq(&self.scene.scene, &plate) {
                self.scene.replace_scene(plate);
                swapped = true;
            }
        } else if !Arc::ptr_eq(&self.scene.scene, &self.model_scene) {
            let model = self.model_scene.clone();
            self.scene.replace_scene(model);
            swapped = true;
        }

        // Poses: MODEL offsets through the staging remap (x, z, -y) —
        // parts.rs' `remap`, restated at the app boundary (crate-private in
        // the viewer) — as translations; hidden slots drop out of the draw.
        let identity = || Some(glam::Mat4::IDENTITY.to_cols_array_2d());
        let poses: Vec<PartPose> = if draw_plate {
            // the plate scene stages pre-arranged geometry
            vec![identity(); self.scene.scene.parts.len()]
        } else if all_hidden {
            // ModelPane.swift:79-80: all-hidden falls back to the assembled
            // model — identity poses, everything visible
            vec![identity(); self.model_parts.len()]
        } else {
            arranged
                .offsets
                .iter()
                .zip(&arranged.visible)
                .map(|(offset, visible)| {
                    visible.then(|| {
                        let scene_offset = [offset[0], offset[2], -offset[1]];
                        glam::Mat4::from_translation(glam::Vec3::from_array(scene_offset))
                            .to_cols_array_2d()
                    })
                })
                .collect()
        };
        self.scene.poses = poses.into();

        // The SECTION's effective box (MODEL): the plate scene's staged box
        // mapped back when it is the drawn geometry (upstream's arranged
        // mesh IS the plate mesh, ModelViewerState.swift:116-121); the
        // ZERO-offset all-parts union at all-hidden (the fallback draws the
        // assembled model — the plane must not keep the exploded aabb);
        // the visible-only arrangement otherwise.
        let eff = if draw_plate {
            self.plate_scene_aabb_as_model()
        } else if all_hidden {
            arrangement::effective_model_aabb(
                &self.model_parts,
                &arrangement::Arranged {
                    offsets: vec![[0.0; 3]; self.model_parts.len()],
                    visible: vec![true; self.model_parts.len()],
                    plate_scene: false,
                },
            )
        } else {
            arrangement::effective_model_aabb(&self.model_parts, &arranged)
        };
        // The FRAMING box instead takes effective_model_aabb's own answer:
        // at all-hidden that KEEPS the arrangement offsets (T1's caller
        // contract — the degrade feeds framing only, so the all-hidden
        // frame matches the layout the user lands in on unhiding).
        let framing_eff = if draw_plate {
            eff
        } else {
            arrangement::effective_model_aabb(&self.model_parts, &arranged)
        };
        self.effective_aabb = eff;
        self.frame_aabb = if draw_plate {
            (self.scene.scene.aabb_min, self.scene.scene.aabb_max)
        } else {
            self.scene_aabb_of(framing_eff)
        };
        if swapped {
            // replace_scene re-derived the framing fields against the new
            // scene; `input` owns the pose, so re-assert it — and adopt the
            // drawn box as the framing reference outright (the plate swap
            // is not a mesh reload; no >50 % gate applies).
            self.input.set_framing_reference(self.frame_aabb);
            self.sync_pose();
            self.restage_ghost();
        }

        // The frozen grid stamp — upstream's sceneID (mesh + layout + plate
        // identity, ModelViewer.swift:165-170 via ModelPane.swift:83): the
        // grid box is captured ONLY when this stamp moves and re-fed
        // verbatim afterwards (set_overlay_frame's own change gate makes
        // the repeats free). The bed, by contrast, follows the manifest
        // live and only draws in the plate layout.
        let stamp = (
            Arc::as_ptr(&self.scene.scene) as usize,
            layout,
            self.plate_loaded,
        );
        if self.grid_stamp != Some(stamp) {
            self.grid_stamp = Some(stamp);
            self.grid_aabb = eff;
        }
        self.grid_bed = (layout == PartsLayout::Plate)
            .then_some(self.manifest.as_ref().and_then(|m| m.bed))
            .flatten();
        self.scene
            .set_overlay_frame(self.scene_aabb_of(self.grid_aabb), self.grid_bed);

        // The section uniform: the raw plane over the effective MODEL aabb,
        // then the LIVE scene pin added by mapping a plane point through
        // to_scene (scene_plane's value is pre-pin — exact only for
        // pin-symmetric scenes; the golden fixture is not).
        let section_plane = self.tools.sectioning.then(|| {
            let (axis, raw) =
                section::scene_plane(self.tools.section_axis, self.tools.section_position, eff);
            // a MODEL point whose pinned scene coordinate along `axis` is
            // exactly raw + the pin's component on that axis (scene x =
            // model x, scene y = model z, scene z = -model y)
            let model_point = match axis {
                0 => [raw, 0.0, 0.0],
                1 => [0.0, 0.0, raw],
                _ => [0.0, -raw, 0.0],
            };
            let pinned = self.scene.scene.to_scene(model_point)[axis as usize];
            (axis, pinned)
        });
        self.scene.set_section(section_plane);
    }

    /// The plate parts the hidden filter keeps — group-keyed like the model
    /// slots (a plate part outside every model group falls back to its own
    /// base name, mirroring arrange()'s mapping). Never called all-hidden:
    /// that fallback draws the assembled MODEL, so the plate stages nothing.
    fn visible_plate_parts(&self) -> Vec<Part> {
        let group_of: HashMap<String, String> = arrangement::groups(&self.model_parts)
            .iter()
            .flat_map(|g| g.members.iter().map(move |m| (m.clone(), g.name.clone())))
            .collect();
        self.plate_parts
            .iter()
            .filter(|p| {
                let group = group_of
                    .get(&p.name)
                    .cloned()
                    .unwrap_or_else(|| arrangement::group_name(&p.name));
                !self.tools.hidden.contains(&group)
            })
            .cloned()
            .collect()
    }

    /// The plate scene's staged aabb mapped back to MODEL (`to_model`): the
    /// effective box while the plate is the drawn geometry. The y axis
    /// negates under the inverse remap, so min/max re-fold per axis.
    fn plate_scene_aabb_as_model(&self) -> ([f32; 3], [f32; 3]) {
        let scene = &self.scene.scene;
        let a = scene.to_model(scene.aabb_min);
        let b = scene.to_model(scene.aabb_max);
        (
            [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])],
            [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])],
        )
    }

    /// A MODEL-space box as this scene's pinned SCENE box — the
    /// [`model_size`] inverse: the remap permutes extents (scene x = model
    /// x span, scene y = model z span, scene z = model y span, y negated)
    /// and the pin translates every axis.
    fn scene_aabb_of(&self, model: ([f32; 3], [f32; 3])) -> ([f32; 3], [f32; 3]) {
        let pin = self.scene.scene.pin;
        let (mn, mx) = model;
        (
            [mn[0] + pin[0], mn[2] + pin[1], -mx[1] + pin[2]],
            [mx[0] + pin[0], mx[2] + pin[1], -mn[1] + pin[2]],
        )
    }

    /// Layout change (ModelViewerState.swift:42, `layout` didSet): the
    /// tools state, persistence, the re-arrangement — then ALWAYS re-frame
    /// at the current preset against the CURRENT arranged aabb, pushed as a
    /// FORCED framing reference (upstream's cameraID bump re-frames against
    /// the ARRANGED mesh unconditionally, ModelViewer.swift:77 → :328-337 —
    /// `preset` alone would re-frame against whatever `mesh_changed` last
    /// pushed, the assembled scene, leaving Plate/Exploded mis-framed).
    fn set_layout(&mut self, layout: PartsLayout) {
        self.tools.set_layout(layout);
        self.persist_setting(|s| s.parts_layout = layout);
        self.apply_arrangement();
        self.input.set_framing_reference(self.frame_aabb);
        self.input.preset(self.preset);
        self.sync_pose();
    }

    /// The parts tree's groups (T1) — the parts panel and the row-click
    /// isolate consume them.
    pub fn groups(&self) -> Vec<PartGroup> {
        arrangement::groups(&self.model_parts)
    }

    /// The compare-mode flag (the shell owns the comparison state): it
    /// forces the Assembled layout (ModelViewerState.swift:78-86), so the
    /// arrangement re-applies when it flips.
    pub fn set_comparing(&mut self, comparing: bool) {
        if self.comparing != comparing {
            self.comparing = comparing;
            self.apply_arrangement();
        }
    }

    /// Load-modify-save the pane's settings keys through
    /// [`Self::settings_path`], preserving every other field (the
    /// ONE-writer rule: this pane owns `parts_layout`/`parts_collapsed`
    /// only) — the project window's established persist pattern.
    fn persist_setting(&self, edit: impl FnOnce(&mut Settings)) {
        let mut settings = Settings::load(&self.settings_path).unwrap_or_default();
        edit(&mut settings);
        if let Err(why) = settings.save(&self.settings_path) {
            eprintln!("cad-studio: settings save failed: {why}");
        }
    }

    /// Recompute the missing-output count from the outputs on disk.
    fn refresh_warnings(&mut self) {
        let ctx = MissingCtx {
            project_root: &self.project_root,
            output_dir: &self.output_dir,
            parts_count: self.scene.scene.parts.len(),
            printer_model: self.printer_model.as_deref(),
            manifest: self.manifest.as_ref(),
        };
        self.warnings = outputs::missing_requirements(&ctx).len();
    }

    /// Whether the output watcher is running (false when the output
    /// directory was missing at construction — the Task 7 scaffolding
    /// guarantee makes that the exception).
    pub fn is_watching(&self) -> bool {
        self.watch.is_some()
    }

    /// Push the state machine's settled pose into the scene.
    fn sync_pose(&mut self) {
        self.scene.set_pose(
            self.input.yaw,
            self.input.pitch,
            self.input.distance,
            self.input.target,
        );
    }

    // ── measure (M6) ────────────────────────────────────────────────────

    /// Fire the measure pick: click-driven and OFF-THREAD per spec §5 — a
    /// named std thread runs [`picking::first_hit`] over an immutable
    /// snapshot (the scene `Arc`, the poses, the section tuple, the ray),
    /// never the cosmic executor — and the answer returns as
    /// [`PaneEvent::Picked`] through the pane's relay. The caller gates:
    /// primary click, measuring, no pick in flight.
    fn spawn_pick(&mut self) {
        let size = *self.last_size.lock().unwrap();
        if size.width <= 0.0 || size.height <= 0.0 {
            return; // no layout pass yet: nothing to aim at
        }
        let (origin, dir) = camera::pick_ray(
            self.input.distance,
            self.input.yaw,
            self.input.pitch,
            self.input.target,
            size.width / size.height,
            (self.last_cursor.x, self.last_cursor.y),
            (size.width, size.height),
        );
        let scene = self.scene.scene.clone();
        let poses = self.scene.poses.to_vec();
        let section = self.pick_section();
        let events = self.events.clone();
        self.pick_pending = true;
        let spawned = thread::Builder::new()
            .name("viewer-pick".into())
            .spawn(move || {
                let hit = picking::first_hit(&scene, &poses, section, origin, dir);
                let _ = events.send(PaneEvent::Picked(hit));
            });
        if spawned.is_err() {
            // never left the ground: let the next click try again
            self.pick_pending = false;
        }
    }

    /// The section tuple a pick honors — the same discard plane the mesh
    /// pass is running (what [`ModelScene::set_section`] last stored), so
    /// a click never answers through geometry the user cannot see; with
    /// sectioning off every pick runs the whole scene.
    fn pick_section(&self) -> Option<(u32, f32)> {
        self.scene.section()
    }

    /// Restage the measure markers from the tool's points — the ONE writer
    /// of `scene.markers`, so every `measure_points` clear site (toggle,
    /// Esc, mesh change) funnels here and an empty measurement stages
    /// nothing. Radius rides the assembled scene's framing radius
    /// (ModelViewer.swift:383: `max(radius · 0.03, 0.4)`); upstream's
    /// `modelRadius` is the last-REFRAMED size, so marker size carries a
    /// bounded cosmetic drift across layouts — the plan's recorded
    /// acceptance.
    fn rebuild_markers(&mut self) {
        let radius = markers::marker_radius(camera::framing_radius(
            self.scene.scene.aabb_min,
            self.scene.scene.aabb_max,
        ));
        let points = self
            .tools
            .measure_points
            .iter()
            .map(|&p| self.scene.scene.to_scene(p))
            .collect::<Vec<_>>();
        self.scene.markers =
            (!points.is_empty()).then(|| Arc::new(markers::geometry(&points, radius)));
    }

    /// shift/option held — the scroll/primary-drag pan gate
    /// (CameraInputView.swift:43-47,63-66).
    fn pan_modifiers(&self) -> bool {
        self.modifiers.shift() || self.modifiers.alt()
    }

    fn view_canvas(&self) -> Element<'_, Message> {
        let area = responsive(move |size: Size| self.canvas_element(size));
        mouse_area(area)
            .on_press(Message::Pressed)
            .on_right_press(Message::RightPressed)
            .on_middle_press(Message::MiddlePressed)
            .on_release(Message::Released)
            .on_right_release(Message::Released)
            .on_middle_release(Message::Released)
            .on_move(|point| Message::Moved(point))
            .on_scroll(|delta| Message::Scrolled(delta))
            .interaction(if self.input.is_pressed() {
                cosmic::iced::mouse::Interaction::Grabbing
            } else {
                cosmic::iced::mouse::Interaction::Grab
            })
            .into()
    }

    fn canvas_element(&self, size: Size) -> Element<'_, Message> {
        self.report_size(size);
        shader(&self.scene)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// The canvas area's size, reported from layout — deduped through
    /// `last_size`.
    fn report_size(&self, size: Size) {
        let mut last = self.last_size.lock().unwrap();
        if *last != size {
            *last = size;
            let _ = self.events.send(PaneEvent::Resized(size));
        }
    }

    fn view_status_bar(&self) -> Element<'_, Message> {
        // Upstream's controls row (ModelPane.swift:120-152): preset picker,
        // the four toggles, the color legend — then the port-specific MSAA
        // toggle at the row's end, the info label right.
        let mut bar = Row::new()
            .spacing(8)
            .push(self.preset_picker())
            .push(tool_toggle(
                self.tools.shows_grid,
                fl!("griglia"),
                fl!("mostra-la-griglia"),
                Message::ToggleGrid,
            ))
            .push(tool_toggle(
                self.tools.wireframe,
                fl!("wireframe"),
                fl!("mostra-il-wireframe"),
                Message::ToggleWireframe,
            ))
            .push(tool_toggle(
                self.tools.measuring,
                fl!("misura"),
                fl!("misura-la-distanza-tra-due-punti"),
                Message::ToggleMeasure,
            ))
            .push(tool_toggle(
                self.tools.sectioning,
                fl!("sezione"),
                fl!("mostra-una-sezione-del-modello"),
                Message::ToggleSection,
            ));
        // The animation slot between section and legend is M7's
        // (ModelPane.swift:136-147).
        bar = bar
            .push(view_legend(&arrangement::legend_colors(&self.model_parts)))
            .push(self.msaa_toggle())
            .push(Space::new().width(Length::Fill))
            .push(text(status_info(
                (!self.scene.scene.parts.is_empty())
                    .then(|| model_size(self.scene.scene.aabb_min, self.scene.scene.aabb_max)),
                self.unit,
            )));
        container(bar.padding([4.0, 12.0]))
            .width(Length::Fill)
            .into()
    }

    /// The iso/front/top/right preset bar (upstream's segmented
    /// `Picker("Vista")`, ModelPane.swift:121-131): one button per preset,
    /// the selected one in the suggested style. The segmented-control
    /// widget's model borrows its owner, which a per-view rebuild cannot
    /// satisfy — four stateless buttons carry the same selection.
    fn preset_picker(&self) -> Element<'_, Message> {
        const PRESETS: [Preset; 4] = [Preset::Iso, Preset::Front, Preset::Top, Preset::Right];
        let mut bar = Row::new().spacing(4);
        for preset in PRESETS {
            bar = bar.push(
                button::text(preset_label(preset))
                    .class(if preset == self.preset {
                        cosmic::theme::Button::Suggested
                    } else {
                        cosmic::theme::Button::Standard
                    })
                    .on_press(Message::Preset(preset)),
            );
        }
        bar.into()
    }

    /// The MSAA opt-in: shows the active sample count, clicks flip 1× ↔ 4×
    /// (the `with_msaa` pipeline set, rebuilt in place).
    fn msaa_toggle(&self) -> Element<'_, Message> {
        button::text(format!("{}×", self.scene.msaa))
            .on_press(Message::ToggleMsaa)
            .into()
    }
}

// ---------------------------------------------------------------------------
// Pure status-bar + reload helpers
// ---------------------------------------------------------------------------

/// The unit's short symbol (upstream `MeasurementUnit.symbol`; the engine
/// type exposes only the long `prompt_name`).
pub fn unit_symbol(unit: MeasurementUnit) -> &'static str {
    match unit {
        MeasurementUnit::Millimeters => "mm",
        MeasurementUnit::Centimeters => "cm",
        MeasurementUnit::Meters => "m",
        MeasurementUnit::Inches => "in",
    }
}

/// One millimeter length in the display unit, formatted with the unit's
/// fraction digits and (optionally) its symbol — upstream ModelPane's
/// `length(_:symbol:)` (ModelPane.swift:221-225).
pub fn format_length(millimeters: f64, unit: MeasurementUnit, symbol: bool) -> String {
    let value = millimeters / unit.mm_per_unit();
    let digits = unit.fraction_digits() as usize;
    let formatted = format!("{value:.digits$}");
    if symbol {
        format!("{formatted} {}", unit_symbol(unit))
    } else {
        formatted
    }
}

/// The MODEL-space size of a scene aabb: the viewer stages MODEL Z-up as
/// SCENE Y-up via `(x, z, -y)`, so the scene extents permute back as
/// `(sx, sz, sy)`.
pub fn model_size(aabb_min: [f32; 3], aabb_max: [f32; 3]) -> [f64; 3] {
    let scene = [
        (aabb_max[0] - aabb_min[0]) as f64,
        (aabb_max[1] - aabb_min[1]) as f64,
        (aabb_max[2] - aabb_min[2]) as f64,
    ];
    [scene[0], scene[2], scene[1]]
}

/// The status bar's info label (upstream ModelPane.info,
/// ModelPane.swift:215-219): the model size `X × Y × Z unit` when a model
/// shows — the size string ONLY (issues live in the header's
/// ActivityViewer, and M4's interim " · N warnings" suffix is dropped
/// rather than recorded as a divergence). Empty when no model shows.
pub fn status_info(size: Option<[f64; 3]>, unit: MeasurementUnit) -> String {
    match size {
        // upstream puts the symbol on the last term only
        Some([x, y, z]) => format!(
            "{} × {} × {}",
            format_length(x, unit, false),
            format_length(y, unit, false),
            format_length(z, unit, true),
        ),
        None => String::new(),
    }
}

/// Parse the output directory's model: `model.3mf` preferred (one part per
/// build item), `model.stl` the fallback (a single part) — including when
/// the 3MF parses but yields ZERO parts (upstream parseMesh's fallback; a
/// well-formed-but-empty file is not a model). Neither readable → `None`
/// (upstream keeps its last good mesh; so does the pane).
pub fn load_model_parts(output_dir: &Path) -> Option<Vec<Part>> {
    if let Ok(bytes) = std::fs::read(output_dir.join("model.3mf")) {
        if let Ok(parts) = parse_3mf(&bytes) {
            if !parts.is_empty() {
                return Some(parts);
            }
        }
    }
    std::fs::read(output_dir.join("model.stl"))
        .ok()
        .and_then(|bytes| parse_stl(&bytes).ok())
        .map(|part| vec![part])
}

/// Whether two staged scenes differ in content: part count, or any part's
/// name, staged-vertex identity, or aabb. The content hash covers every
/// staged byte (colors, normals, the floor pin), so equal parts mean an
/// identical upload — the reload's short-circuit. Poses are per-frame
/// uniforms, not content, and do not belong here.
pub fn scenes_differ(current: &PartScene, next: &PartScene) -> bool {
    current.parts.len() != next.parts.len()
        || current.parts.iter().zip(&next.parts).any(|(a, b)| {
            a.name != b.name
                || a.content_hash != b.content_hash
                || a.aabb_min != b.aabb_min
                || a.aabb_max != b.aabb_max
        })
}

/// The relay stream: one thread draining the std event channel into the
/// subscription. It ends when the pane closes (the pane's sender drops, the
/// watcher bridge's sends fail, and this recv errors) — the schematic
/// pane's established pattern (bounded retry on a transiently full channel:
/// liveness over completeness; a dropped resize or reload is re-derived by
/// the next event or the next layout pass).
fn relay_stream(relay: &Arc<OutputRelay>) -> BoxStream<'static, Message> {
    stream::channel(64, {
        let relay = relay.clone();
        async move |output| {
            let Some(events) = relay.take_events() else {
                // a second subscription over the same relay: the first one
                // owns the receiver, this stream idles forever
                std::future::pending::<()>().await;
                return;
            };
            let mut sender = output.clone();
            let _ = thread::Builder::new()
                .name("viewer-events".to_string())
                .spawn(move || {
                    while let Ok(event) = events.recv() {
                        let message = relay_message(event);
                        let mut backoff = RELAY_RETRY_MIN;
                        let started = std::time::Instant::now();
                        loop {
                            match sender.try_send(message.clone()) {
                                Ok(()) => break,
                                Err(err) if err.is_disconnected() => return,
                                Err(err) => {
                                    if started.elapsed() > RELAY_RETRY_BUDGET {
                                        break; // drop this message, keep serving
                                    }
                                    let _ = err.into_inner();
                                    thread::sleep(backoff);
                                    backoff = (backoff * 2).min(RELAY_RETRY_MAX);
                                }
                            }
                        }
                    }
                });
            std::future::pending::<()>().await;
        }
    })
    .boxed()
}

pub(crate) fn stl_of_box(bounds: [[f64; 3]; 2]) -> Vec<u8> {
    fn push_f32s(out: &mut Vec<u8>, fs: &[f32]) {
        for f in fs {
            out.extend_from_slice(&f.to_le_bytes());
        }
    }
    let mn = bounds[0].map(|c| c as f32);
    let mx = bounds[1].map(|c| c as f32);
    // corner (i, j, k) picks min/max per axis; the six faces as quads
    let corner = |i: u8, j: u8, k: u8| {
        [
            [mn[0], mx[0]][i as usize],
            [mn[1], mx[1]][j as usize],
            [mn[2], mx[2]][k as usize],
        ]
    };
    let quads: [[(u8, u8, u8); 4]; 6] = [
        [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)], // z = min
        [(0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1)], // z = max
        [(0, 0, 0), (1, 0, 0), (1, 0, 1), (0, 0, 1)], // y = min
        [(0, 1, 0), (1, 1, 0), (1, 1, 1), (0, 1, 1)], // y = max
        [(0, 0, 0), (0, 1, 0), (0, 1, 1), (0, 0, 1)], // x = min
        [(1, 0, 0), (1, 1, 0), (1, 1, 1), (1, 0, 1)], // x = max
    ];
    let mut out = Vec::new();
    out.extend_from_slice(&[0u8; 80]);
    out.extend_from_slice(&12u32.to_le_bytes());
    for quad in quads {
        for tri in [(0usize, 1, 2), (0, 2, 3)] {
            push_f32s(&mut out, &[0.0; 3]); // normal: unused by the parser
            for idx in [tri.0, tri.1, tri.2] {
                push_f32s(&mut out, &corner(quad[idx].0, quad[idx].1, quad[idx].2));
            }
            out.extend_from_slice(&0u16.to_le_bytes());
        }
    }
    out
}

/// A Stack layer pinned top-center: the element between fill spacers.
fn top_center<'a>(layer: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Row::new()
        .push(Space::new().width(Length::Fill))
        .push(layer.into())
        .push(Space::new().width(Length::Fill))
        .width(Length::Fill)
        .into()
}

/// A Stack layer pinned bottom-center: fill space above a centered row.
fn bottom_center<'a>(layer: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Column::new()
        .push(Space::new().height(Length::Fill))
        .push(top_center(layer))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// The legend's dots: the first 12 colors (ModelPane.swift:204,
/// `prefix(12)`).
pub fn legend_dots(colors: &[[u8; 3]]) -> &[[u8; 3]] {
    &colors[..colors.len().min(12)]
}

/// The legend's accessibility text (ModelPane.swift:212): the count of
/// EVERY distinct color. The catalog key still carries upstream's
/// Swift-style `%lld` placeholder (the M8 placeable sweep migrates it to a
/// fluent `{$count}` arg), so the count substitutes onto the lookup —
/// this substitution is the interim recorded for that sweep.
pub fn legend_label(total: usize) -> String {
    fl!("lld-colori").replace("%lld", &total.to_string())
}

/// A preset's picker label (CameraPreset.title,
/// ModelPane.swift:122-129): `iso`/`fronte`/`alto`/`destra`.
fn preset_label(preset: Preset) -> String {
    match preset {
        Preset::Iso => fl!("iso"),
        Preset::Front => fl!("fronte"),
        Preset::Top => fl!("alto"),
        Preset::Right => fl!("destra"),
    }
}

/// The status bar's color legend (ModelPane.swift:202-213): ten-pixel
/// dots for the first twelve distinct colors; the tooltip carries the
/// count label (the a11y stand-in).
fn view_legend(colors: &[[u8; 3]]) -> Element<'static, Message> {
    let mut dots = Row::new().spacing(3);
    for color in legend_dots(colors) {
        dots = dots.push(parts_overlay::color_dot(Some(*color), 10.0));
    }
    tooltip(
        dots,
        text(legend_label(colors.len())),
        cosmic::widget::tooltip::Position::FollowCursor,
    )
    .into()
}

/// One status-bar toggle button (upstream's borderless `Toggle`, shown as
/// an icon-less text button — the documented glyph divergence): the
/// selected state in the suggested class, the help text on a tooltip.
fn tool_toggle(
    selected: bool,
    label: String,
    help: String,
    message: Message,
) -> Element<'static, Message> {
    let toggle = button::text(label)
        .class(if selected {
            cosmic::theme::Button::Suggested
        } else {
            cosmic::theme::Button::Standard
        })
        .on_press(message);
    tooltip(
        toggle,
        text(help),
        cosmic::widget::tooltip::Position::FollowCursor,
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic::iced::mouse::ScrollDelta;
    use engine::project::versions::ProjectVersion;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("cad-viewer-pane-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn fixture(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures")
            .join(rel)
    }

    fn mk_project(tag: &str, root_files: &[&str], output_files: &[&str]) -> PathBuf {
        let root = tmp_dir(tag);
        std::fs::create_dir_all(root.join("output")).unwrap();
        for name in root_files {
            std::fs::write(root.join(name), b"# model\n").unwrap();
        }
        for rel in output_files {
            // fixture-relative paths like "golden/model.3mf" land flat in
            // output/ under their file name
            std::fs::copy(
                fixture(rel),
                root.join("output")
                    .join(Path::new(rel).file_name().unwrap()),
            )
            .unwrap();
        }
        root
    }

    /// The pane with the project's own temp dir as its settings path — the
    /// ONE-writer rule's test half: layout/collapse persistence rounds
    /// through a disposable file, never the developer's real config
    /// (parallel pane tests would race it).
    fn pane(tag: &str, root_files: &[&str], output_files: &[&str]) -> (PathBuf, ViewerPane) {
        let root = mk_project(tag, root_files, output_files);
        let pane = ViewerPane::new_with_settings(
            &root,
            MeasurementUnit::Millimeters,
            None,
            root.join("settings.json"),
        );
        (root, pane)
    }

    #[test]
    fn unit_symbol_table() {
        assert_eq!(unit_symbol(MeasurementUnit::Millimeters), "mm");
        assert_eq!(unit_symbol(MeasurementUnit::Centimeters), "cm");
        assert_eq!(unit_symbol(MeasurementUnit::Meters), "m");
        assert_eq!(unit_symbol(MeasurementUnit::Inches), "in");
    }

    #[test]
    fn format_length_uses_mm_per_unit_and_fraction_digits() {
        // ModelPane.swift:221-225: value = mm / mmPerUnit, fraction digits
        // from the unit, symbol appended on request.
        assert_eq!(
            format_length(12.34, MeasurementUnit::Millimeters, false),
            "12.3"
        );
        assert_eq!(
            format_length(12.34, MeasurementUnit::Millimeters, true),
            "12.3 mm"
        );
        assert_eq!(
            format_length(25.4, MeasurementUnit::Inches, true),
            "1.00 in"
        );
        assert_eq!(
            format_length(1500.0, MeasurementUnit::Meters, true),
            "1.500 m"
        );
        assert_eq!(
            format_length(15.0, MeasurementUnit::Centimeters, false),
            "1.50"
        );
    }

    #[test]
    fn model_size_permutes_scene_yup_back_to_model_zup() {
        // The (x, z, -y) remap stages MODEL (x, y, z) as SCENE (x, z, y):
        // the MODEL size is the staged extents permuted (sx, sz, sy).
        let size = model_size([0.0; 3], [10.0, 20.0, 30.0]);
        assert_eq!(size, [10.0, 30.0, 20.0]);
    }

    #[test]
    fn status_info_is_the_size_string_only() {
        // Upstream's info is the size alone (ModelPane.swift:215-219);
        // M4's " · N warnings" suffix is dropped — issues live in the
        // header's ActivityViewer, not the status bar.
        let unit = MeasurementUnit::Millimeters;
        assert_eq!(
            status_info(Some([30.0, 15.5, 15.5]), unit),
            "30.0 × 15.5 × 15.5 mm"
        );
        assert_eq!(status_info(None, unit), "");
    }

    #[test]
    fn legend_view_wraps_at_twelve() {
        // 13 distinct colors -> 12 dots, the a11y count says 13; asserted
        // through the pure legend_colors input the view consumes.
        crate::localize::select("en");
        let parts: Vec<Part> = (0..13u8)
            .map(|i| Part {
                name: format!("p{i}"),
                positions: vec![[0.0; 3], [1.0; 3], [0.0, 1.0, 0.0]],
                triangle_colors: vec![Some([i, 255 - i, i * 17])],
            })
            .collect();
        let colors = arrangement::legend_colors(&parts);
        assert_eq!(colors.len(), 13);
        assert_eq!(legend_dots(&colors).len(), 12);
        assert_eq!(legend_label(13), "13 colors");
        let _ = view_legend(&colors);
    }

    #[test]
    fn load_model_parts_prefers_3mf_over_stl() {
        let both = tmp_dir("both");
        std::fs::copy(fixture("golden/model.3mf"), both.join("model.3mf")).unwrap();
        std::fs::copy(fixture("golden/model.stl"), both.join("model.stl")).unwrap();
        assert_eq!(load_model_parts(&both).unwrap().len(), 2, "3MF: 2 parts");

        // an empty-parts 3MF is a fallback, not a model: upstream parseMesh
        // reads the STL beside it rather than rendering nothing
        let empty_3mf = tmp_dir("empty-3mf");
        std::fs::copy(fixture("handmade/empty.3mf"), empty_3mf.join("model.3mf")).unwrap();
        assert_eq!(
            parse_3mf(&std::fs::read(empty_3mf.join("model.3mf")).unwrap()).unwrap(),
            Vec::<Part>::new(),
            "fixture truth: the 3MF parses, to zero parts"
        );
        std::fs::copy(fixture("golden/model.stl"), empty_3mf.join("model.stl")).unwrap();
        assert_eq!(
            load_model_parts(&empty_3mf).unwrap().len(),
            1,
            "empty 3MF: the STL beside it loads"
        );

        let stl_only = tmp_dir("stl");
        std::fs::copy(fixture("golden/model.stl"), stl_only.join("model.stl")).unwrap();
        assert_eq!(load_model_parts(&stl_only).unwrap().len(), 1, "STL: 1 part");

        let neither = tmp_dir("none");
        assert!(load_model_parts(&neither).is_none());
    }

    #[test]
    fn scenes_differ_compares_staged_content() {
        let a = PartScene::from_parts(
            &[parse_stl(&std::fs::read(fixture("golden/model.stl")).unwrap()).unwrap()],
            NEUTRAL,
        );
        let a_again = PartScene::from_parts(
            &[parse_stl(&std::fs::read(fixture("golden/model.stl")).unwrap()).unwrap()],
            NEUTRAL,
        );
        assert!(!scenes_differ(&a, &a_again), "same bytes, same staging");

        let b = PartScene::from_parts(
            &parse_3mf(&std::fs::read(fixture("golden/model.3mf")).unwrap()).unwrap(),
            NEUTRAL,
        );
        assert!(scenes_differ(&a, &b));
    }

    #[test]
    fn reload_is_idempotent_across_a_split_burst() {
        // A starved burst reaches the pane as two Reload messages with the
        // same content: the first replaces the scene, the second
        // short-circuits — ONE scene rebuild (same Arc identity).
        let (root, mut pane) = pane("burst", &[], &[]);
        assert!(pane.scene.scene.parts.is_empty(), "starts empty");

        std::fs::copy(fixture("golden/model.3mf"), root.join("output/model.3mf")).unwrap();
        let before = Arc::as_ptr(&pane.scene.scene);
        let _ = pane.update(Message::Reload(ReloadKind::Mesh));
        let after_first = Arc::as_ptr(&pane.scene.scene);
        assert_ne!(before, after_first, "first emission builds the scene");
        assert_eq!(pane.scene.scene.parts.len(), 2);

        let _ = pane.update(Message::Reload(ReloadKind::Mesh));
        let after_second = Arc::as_ptr(&pane.scene.scene);
        assert_eq!(
            after_first, after_second,
            "the burst's second emission rebuilds nothing"
        );
        assert_eq!(pane.scene.scene.parts.len(), 2);
    }

    #[test]
    fn reload_follows_content_changes() {
        let (root, mut pane) = pane("swap", &[], &["golden/model.3mf"]);
        assert_eq!(pane.scene.scene.parts.len(), 2);

        // a different model on disk replaces the scene
        std::fs::remove_file(root.join("output/model.3mf")).unwrap();
        std::fs::copy(fixture("golden/model.stl"), root.join("output/model.stl")).unwrap();
        let _ = pane.update(Message::Reload(ReloadKind::Mesh));
        assert_eq!(pane.scene.scene.parts.len(), 1, "STL fallback reloads");
    }

    #[test]
    fn new_loads_existing_outputs_and_watches() {
        let (_, pane) = pane("existing", &["model.py"], &["golden/model.3mf"]);
        assert_eq!(pane.scene.scene.parts.len(), 2);
        assert!(pane.is_watching(), "output dir exists -> watch runs");

        // model.py present + most outputs missing -> warnings counted
        let ctx = MissingCtx {
            project_root: pane.project_root.as_path(),
            output_dir: pane.output_dir.as_path(),
            parts_count: pane.scene.scene.parts.len(),
            printer_model: pane.printer_model.as_deref(),
            manifest: pane.manifest.as_ref(),
        };
        assert_eq!(pane.warnings, outputs::missing_requirements(&ctx).len());
        assert!(pane.warnings > 0);
    }

    #[test]
    fn msaa_defaults_off_and_toggles_pipeline_count() {
        let (_, mut pane) = pane("msaa", &[], &[]);
        assert!(!pane.msaa, "opt-in: default off");
        assert_eq!(pane.scene.msaa, 1, "1x by default");

        let _ = pane.update(Message::ToggleMsaa);
        assert!(pane.msaa);
        assert_eq!(pane.scene.msaa, 4, "with_msaa's pipeline set");

        let _ = pane.update(Message::ToggleMsaa);
        assert!(!pane.msaa);
        assert_eq!(pane.scene.msaa, 1);
    }

    #[test]
    fn preset_message_drives_the_scene_pose() {
        let (_, mut pane) = pane("preset", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::Preset(Preset::Top));
        assert!((pane.scene.pitch - 1.5607).abs() < 1e-4);
        assert!((pane.scene.yaw - 0.0).abs() < 1e-4);
        assert!((pane.scene.pitch - pane.input.pitch).abs() < 1e-6);
        assert!((pane.scene.yaw - pane.input.yaw).abs() < 1e-6);

        // Fit frames the tracked reference; pin the layout so that
        // reference IS the staged scene's own box (settings default the
        // pane to Exploded, whose arranged frame is wider)
        let _ = pane.update(Message::SetLayout(PartsLayout::Assembled));
        let _ = pane.update(Message::Fit);
        let (_, distance) =
            viewer::render::camera::frame(pane.scene.scene.aabb_min, pane.scene.scene.aabb_max);
        assert!((pane.scene.distance - distance).abs() < 1e-4);
    }

    #[test]
    fn modifier_scroll_pans_plain_scroll_zooms() {
        let (_, mut pane) = pane("scroll", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::Preset(Preset::Front));
        // the pan formula scales by the viewport height, normally reported
        // from the first layout pass
        pane.input.set_view_height(600.0);
        let d0 = pane.scene.distance;
        let t0 = pane.input.target;

        // plain scroll zooms (facts #20)
        let _ = pane.update(Message::Scrolled(ScrollDelta::Pixels { x: 0.0, y: 24.0 }));
        assert!((pane.scene.distance - d0 * 0.9f32.powf(2.0)).abs() < 1e-3);

        // shift+scroll pans without zooming (CameraInputView.swift:63-66)
        let _ = pane.update(Message::ModifiersChanged(Modifiers::SHIFT));
        let _ = pane.update(Message::Scrolled(ScrollDelta::Pixels { x: 0.0, y: -24.0 }));
        assert!(
            (pane.scene.distance - d0 * 0.9f32.powf(2.0)).abs() < 1e-3,
            "modifier scroll never zooms"
        );
        assert!(pane.input.target != t0, "the target moved");
    }

    #[test]
    fn view_builds_with_and_without_a_model() {
        // Element construction is pure — the shader program only reaches
        // the GPU at draw time.
        let (_, pane_empty) = pane("view-empty", &[], &[]);
        let _ = pane_empty.view_model_3d(None);

        let (_, pane_full) = pane("view-full", &[], &["golden/model.3mf"]);
        let _ = pane_full.view_model_3d(None);
    }

    #[test]
    fn parameters_visible_by_default_and_option_command_i_toggles() {
        // areParametersVisible default true (Project.swift:71); ⌥⌘I flips
        // it (Commands.swift:69-73).
        let (_, mut pane) = pane("opt-cmd-i", &[], &["golden/model.3mf"]);
        assert!(pane.parameters_visible);
        let _ = pane.update(Message::ToggleParameters);
        assert!(!pane.parameters_visible);
        let _ = pane.update(Message::ToggleParameters);
        assert!(pane.parameters_visible);
    }

    #[test]
    fn drag_then_release_commits_against_the_committed_value() {
        // The full release path: drag (live, editing), release → commit
        // computed against the SYNCED committed value, the local half
        // applied (editing cleared, value snapped to the commit).
        let (_, mut pane) = pane("drag-release", &[], &["golden/model.3mf"]);
        pane.sync_parameters(&[ModelParameter {
            name: "wall".into(),
            value: 3.0,
        }]);

        let _ = pane.update(Message::Parameters(parameters_view::Message::Drag(
            "wall".into(),
            4.4,
        )));
        let (state_value, editing) = {
            let (_, state) = &pane.parameters.sliders[0];
            (state.value, state.editing)
        };
        assert_eq!((state_value, editing), (4.4, true), "drag is live");

        let commit = pane.commit_parameter("wall");
        assert_eq!(commit, Some(("wall".to_string(), 4.0)), "integer rule");
        let (_, state) = &pane.parameters.sliders[0];
        assert_eq!(
            (state.value, state.editing),
            (4.0, false),
            "snapped, settled"
        );
    }

    #[test]
    fn typed_draft_follows_the_thumb_and_submits() {
        let (_, mut pane) = pane("draft-submit", &[], &["golden/model.3mf"]);
        pane.sync_parameters(&[ModelParameter {
            name: "wall".into(),
            value: 0.5,
        }]);

        let _ = pane.update(Message::Parameters(parameters_view::Message::Draft(
            "wall".into(),
            "30".into(),
        )));
        assert!(pane.parameter_drafts.contains_key("wall"));
        assert_eq!(pane.parameters.sliders[0].1.value, 30.0, "thumb follows");

        let commit = pane.submit_parameter("wall");
        assert_eq!(commit, Some(("wall".to_string(), 30.0)), "3-decimal rule");
        assert!(
            !pane.parameter_drafts.contains_key("wall"),
            "draft consumed"
        );
        let (_, state) = &pane.parameters.sliders[0];
        assert_eq!((state.min, state.max), (0.0, 60.0), "escaped range re-fit");

        // an unparseable draft clears without moving the commit: the field
        // falls back to the live value (still 30; the committed map stays
        // at 0.5 until the shell syncs the refresh)
        pane.parameter_drafts.insert("wall".into(), "-".into());
        let commit = pane.submit_parameter("wall");
        assert_eq!(
            commit,
            Some(("wall".to_string(), 30.0)),
            "garbage draft falls back to the live value"
        );
    }

    #[test]
    fn sync_clears_drafts_whose_value_changed() {
        let (_, mut pane) = pane("sync-drafts", &[], &["golden/model.3mf"]);
        pane.sync_parameters(&[
            ModelParameter {
                name: "wall".into(),
                value: 2.0,
            },
            ModelParameter {
                name: "height".into(),
                value: 10.0,
            },
        ]);
        pane.parameter_drafts.insert("wall".into(), "2.5".into());
        pane.parameter_drafts.insert("height".into(), "12".into());

        // wall changed externally (a refused build left it rewritten);
        // height did not
        pane.sync_parameters(&[
            ModelParameter {
                name: "wall".into(),
                value: 2.5,
            },
            ModelParameter {
                name: "height".into(),
                value: 10.0,
            },
        ]);
        assert!(
            !pane.parameter_drafts.contains_key("wall"),
            "changed → cleared"
        );
        assert!(pane.parameter_drafts.contains_key("height"), "kept");

        // a REMOVED parameter drops its draft with its row (a fresh draft,
        // so the value-change rule alone would keep it)
        pane.parameter_drafts.insert("wall".into(), "3".into());
        pane.sync_parameters(&[ModelParameter {
            name: "height".into(),
            value: 10.0,
        }]);
        assert!(
            !pane.parameter_drafts.contains_key("wall"),
            "removed → dropped"
        );
        assert!(pane.parameter_drafts.contains_key("height"));

        // collapse and close round-trip through the overlay messages
        let _ = pane.update(Message::Parameters(parameters_view::Message::Collapse(
            true,
        )));
        assert!(pane.parameters.collapsed);
        let _ = pane.update(Message::Parameters(parameters_view::Message::Collapse(
            false,
        )));
        assert!(!pane.parameters.collapsed);
        let _ = pane.update(Message::Parameters(parameters_view::Message::Close));
        assert!(!pane.parameters_visible);
    }

    #[test]
    fn overlay_view_builds_in_every_shape() {
        // Element construction is pure; the shapes: collapsed, comparing
        // with versions, busy, and an empty previous map.
        let (_, mut pane) = pane("overlay-view", &[], &["golden/model.3mf"]);
        pane.sync_parameters(&[
            ModelParameter {
                name: "wall".into(),
                value: 2.0,
            },
            ModelParameter {
                name: "height".into(),
                value: 10.0,
            },
        ]);
        let previous = BTreeMap::from([("wall".to_string(), 1.5)]);
        let versions = vec![ProjectVersion {
            number: 1,
            created_at: time::OffsetDateTime::now_utc(),
            prompt: "a bracket".into(),
            session_id: None,
        }];
        for collapsed in [false, true] {
            pane.parameters.collapsed = collapsed;
            for busy in [false, true] {
                let ctx = OverlayContext {
                    previous: &previous,
                    busy,
                    versions: &versions,
                    compare: Some(1),
                };
                let _ = pane.view_model_3d(Some(ctx));
            }
        }
        // hidden → the overlay layer never mounts
        pane.parameters_visible = false;
        let ctx = OverlayContext {
            previous: &previous,
            busy: false,
            versions: &versions,
            compare: None,
        };
        let _ = pane.view_model_3d(Some(ctx));
    }

    // ── M6 T10: arrangement wiring (layouts, explode, hidden, plate, bed,
    // section) — the tests below drive the pane's messages end to end ──────

    /// A minimal manifest carrying only a bed rectangle.
    fn manifest_with_bed(width: f64, depth: f64) -> String {
        format!("{{\"bed\": [{width}, {depth}]}}")
    }

    #[test]
    fn layout_change_reframes_at_preset_and_rebuilds_grid() {
        let (_root, mut pane) = pane("layout", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::Preset(Preset::Front));
        // orbit away from the preset — the press must precede the moves:
        // the first move after a press only anchors the reference point
        // (hover moves are ignored entirely)
        let _ = pane.update(Message::Pressed);
        let _ = pane.update(Message::Moved(Point::new(300.0, 300.0)));
        let _ = pane.update(Message::Moved(Point::new(340.0, 300.0)));
        let yaw_before = pane.input.yaw;
        assert!(yaw_before.abs() > 1e-3);
        let dist_before = pane.input.distance;
        let _ = pane.update(Message::SetLayout(PartsLayout::Plate));
        assert!(
            (pane.input.yaw - 0.0).abs() < 1e-4,
            "front preset yaw restored"
        );
        assert!((pane.input.pitch - 0.0).abs() < 1e-4);
        assert!(
            pane.input.distance > dist_before,
            "re-frame used the CURRENT arranged aabb: the plate fallback lays parts side by side (wider than assembled), so the camera pulls BACK"
        );
        assert_eq!(pane.tools.layout, PartsLayout::Plate);
        // persisted — through the pane's injected TEMP settings path, never the
        // developer's real global file (parallel pane tests would race it)
        let s = engine::settings::Settings::load(&pane.settings_path).unwrap_or_default();
        assert_eq!(
            s.parts_layout,
            PartsLayout::Plate,
            "ONE writer: the pane's settings path"
        );
    }

    #[test]
    fn explode_sweep_leaves_the_grid_frozen() {
        let (_root, mut pane) = pane("grid-freeze", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::SetLayout(PartsLayout::Exploded));
        let identity = pane.scene.grid_identity_for_tests();
        let _ = pane.update(Message::SetExplode(2.0));
        let _ = pane.update(Message::ToggleHiddenGroup("lid".into()));
        assert_eq!(
            pane.scene.grid_identity_for_tests(),
            identity,
            "grid freezes at the layout-switch-time aabb (upstream rebuilds only on sceneID, not explode/hidden)"
        );
    }

    #[test]
    fn explode_updates_poses_without_a_scene_swap() {
        let (_root, mut pane) = pane("explode", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::SetLayout(PartsLayout::Exploded));
        let scene_ptr = Arc::as_ptr(&pane.scene.scene);
        let pose_at_one = pane.scene.poses[1];
        // to_cols_array_2d is column-major: the translation is column 3, so
        // read it back through from_cols_array_2d's w_axis — indexing
        // m[0][3]/m[1][3]/m[2][3] reads the BOTTOM row, zeros for a
        // translation matrix
        let mag = |p: viewer::render::PartPose| {
            p.map_or(0.0, |m| {
                glam::Mat4::from_cols_array_2d(&m)
                    .w_axis
                    .truncate()
                    .length()
            })
        };
        let _ = pane.update(Message::SetExplode(2.0));
        assert_eq!(
            Arc::as_ptr(&pane.scene.scene),
            scene_ptr,
            "no scene rebuild"
        );
        assert!(
            (mag(pane.scene.poses[1]) - 2.0 * mag(pose_at_one)).abs() < 1e-3,
            "offset scales with explode: {} vs {}",
            mag(pane.scene.poses[1]),
            2.0 * mag(pose_at_one)
        );
        let _ = pane.update(Message::SetExplode(0.5));
        assert!(
            mag(pane.scene.poses[1]) < mag(pose_at_one),
            "monotone in explode"
        );
        let _ = pane.update(Message::SetExplode(0.0));
        assert_eq!(
            mag(pane.scene.poses[1]),
            0.0,
            "explode 0 = assembled positions (+lift)"
        );
    }

    #[test]
    fn hiding_every_group_falls_back_to_assembled() {
        let (_root, mut pane) = pane("hide-all", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::SetLayout(PartsLayout::Exploded));
        let _ = pane.update(Message::ToggleHiddenGroup("base".into()));
        let _ = pane.update(Message::ToggleHiddenGroup("lid".into()));
        assert!(
            pane.scene.poses.iter().all(|p| p.is_some()),
            "quirk: all-hidden shows assembled"
        );
        // and unhiding base leaves exactly ONE group out of the draw set:
        // hidden == {"lid"} — the lid's pose drops, base draws
        let _ = pane.update(Message::ToggleHiddenGroup("base".into()));
        assert!(
            pane.scene
                .poses
                .iter()
                .enumerate()
                .all(|(i, p)| pane.scene.scene.parts[i].name == "base" || p.is_none())
        );
    }

    #[test]
    fn plate_reload_stages_the_plate_scene_and_bed() {
        let (root, mut pane) = pane("plate", &[], &["golden/model.3mf"]);
        std::fs::copy(fixture("golden/plate.3mf"), root.join("output/plate.3mf")).unwrap();
        // manifest with a bed
        std::fs::write(
            root.join("output/manifest.json"),
            manifest_with_bed(220.0, 220.0),
        )
        .unwrap();
        let _ = pane.update(Message::Reload(ReloadKind::Plate));
        let _ = pane.update(Message::Reload(ReloadKind::Manifest));
        let _ = pane.update(Message::SetLayout(PartsLayout::Plate));
        assert!(pane.plate_loaded);
        assert_eq!(
            pane.scene.bed,
            Some([220.0, 220.0]),
            "bed outline in plate view"
        );
        let _ = pane.update(Message::SetLayout(PartsLayout::Assembled));
        assert_eq!(pane.scene.bed, None, "bed only in plate view");
    }

    #[test]
    fn all_hidden_in_plate_view_falls_back_to_the_assembled_model() {
        // ModelPane.swift:78-80: an arrangement whose visible set is empty
        // draws `output.mesh` — the assembled MODEL, not the unfiltered
        // plate — so the drawn scene swaps back with identity poses and a
        // model-derived effective box (the section/framing domain follows)
        let (root, mut pane) = pane("plate-hide-all", &[], &["golden/model.3mf"]);
        std::fs::copy(fixture("golden/plate.3mf"), root.join("output/plate.3mf")).unwrap();
        let _ = pane.update(Message::Reload(ReloadKind::Plate));
        let _ = pane.update(Message::SetLayout(PartsLayout::Plate));
        assert!(
            !Arc::ptr_eq(&pane.scene.scene, &pane.model_scene),
            "precondition: the plate scene draws"
        );
        let _ = pane.update(Message::ToggleHiddenGroup("base".into()));
        let _ = pane.update(Message::ToggleHiddenGroup("lid".into()));
        assert!(
            Arc::ptr_eq(&pane.scene.scene, &pane.model_scene),
            "all-hidden draws the assembled model, never the plate"
        );
        assert!(
            pane.scene.poses.iter().all(|p| p.is_some()),
            "the fallback shows everything: identity poses"
        );
        // the effective box is the assembled MODEL's (the whole staged
        // scene mapped back), not the plate's
        let scene = &pane.scene.scene;
        let a = scene.to_model(scene.aabb_min);
        let b = scene.to_model(scene.aabb_max);
        let want = (
            [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])],
            [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])],
        );
        for axis in 0..3 {
            assert!(
                (pane.effective_aabb.0[axis] - want.0[axis]).abs() < 1e-3
                    && (pane.effective_aabb.1[axis] - want.1[axis]).abs() < 1e-3,
                "axis {axis}: effective {:?} vs the assembled model {:?}",
                pane.effective_aabb,
                want
            );
        }
    }

    #[test]
    fn section_slider_maps_through_the_arranged_aabb() {
        let (_root, mut pane) = pane("section", &[], &["golden/model.3mf"]);
        // settings default the pane to Exploded; pin the layout so the
        // assert below states the ASSEMBLED mid
        let _ = pane.update(Message::SetLayout(PartsLayout::Assembled));
        let _ = pane.update(Message::ToggleSection);
        let _ = pane.update(Message::SetSectionAxis(SectionAxis::Z));
        let _ = pane.update(Message::SetSectionPosition(0.5));
        let (axis, pos) = pane.scene.section_for_tests().expect("sectioning on");
        assert_eq!(axis, 1, "MODEL Z -> scene y");
        // golden: model z in [-4, 22]; assembled aabb mid = 9. The uniform
        // carries the STAGED plane, so the live scene pin rides along
        // (d944b65: scene_plane's value is pre-pin; the golden pin is +4
        // along scene y — floor z=-4 pins to y=0) — 9 + 4, not the raw 9.
        let pin_y = pane.scene.scene.pin[1];
        assert!((pin_y - 4.0).abs() < 1e-4, "golden's scene pin: {pin_y}");
        assert!(
            (pos - (9.0 + pin_y)).abs() < 1.0,
            "plane at the arranged mid, pin included: {pos}"
        );
        // exploding moves the aabb hence the plane for the SAME t (upstream
        // quirk): the lid pushes +z while the lift holds min z at -4, so the
        // mid rises
        let _ = pane.update(Message::SetLayout(PartsLayout::Exploded));
        let (_, pos2) = pane.scene.section_for_tests().expect("still sectioning");
        assert!(pos2 > pos, "same t, moved plane: {pos} -> {pos2}");
    }

    #[test]
    fn y_axis_section_maps_negated() {
        let (_root, mut pane) = pane("section-y", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::SetLayout(PartsLayout::Assembled));
        let _ = pane.update(Message::ToggleSection);
        let _ = pane.update(Message::SetSectionAxis(SectionAxis::Y));
        let _ = pane.update(Message::SetSectionPosition(0.5));
        let (axis, _) = pane.scene.section_for_tests().expect("sectioning on");
        assert_eq!(axis, 2, "MODEL Y -> scene z");
    }

    #[test]
    fn row_double_click_isolates_the_group() {
        let (_root, mut pane) = pane("row-click", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::SetLayout(PartsLayout::Assembled));
        // two clicks on DIFFERENT rows inside the window: not a double
        let _ = pane.update(Message::RowClicked("base".into()));
        let _ = pane.update(Message::RowClicked("lid".into()));
        assert!(pane.tools.hidden.is_empty(), "different groups: no isolate");
        // the same row again, inside the window: everything else hides
        let _ = pane.update(Message::RowClicked("lid".into()));
        assert_eq!(
            pane.tools.hidden.iter().collect::<Vec<_>>(),
            [&"base".to_string()],
            "isolate = exactly-others"
        );
        // and the draw set follows: the lid draws, the base's pose drops
        assert!(
            pane.scene
                .poses
                .iter()
                .enumerate()
                .all(|(i, p)| pane.scene.scene.parts[i].name == "lid" || p.is_none())
        );
        // a second double-click on the same row restores (compare-and-assign
        // toggles per double: a THIRD would isolate again)
        let _ = pane.update(Message::RowClicked("lid".into()));
        assert!(pane.tools.hidden.is_empty(), "second double-click clears");
    }

    #[test]
    fn tool_toggles_drive_scene_flags_and_persist_collapse() {
        let (_root, mut pane) = pane("tool-toggles", &[], &["golden/model.3mf"]);
        assert!(
            pane.scene.show_grid && !pane.scene.wireframe,
            "ModelViewerState defaults"
        );
        let _ = pane.update(Message::ToggleGrid);
        let _ = pane.update(Message::ToggleWireframe);
        assert!(!pane.scene.show_grid && pane.scene.wireframe);
        assert!(
            !pane.tools.shows_grid && pane.tools.wireframe,
            "tools and scene stay in sync"
        );
        // the parts drawer's chevron persists through the pane's path
        let _ = pane.update(Message::TogglePartsCollapsed);
        let s = engine::settings::Settings::load(&pane.settings_path).unwrap_or_default();
        assert!(s.parts_collapsed, "ONE writer: the pane's settings path");
    }

    // ── M6 T11: the measure click flow ────────────────────────────────────

    impl ViewerPane {
        /// Drain the pane's relay until `done` holds or `timeout` elapses,
        /// translating each event exactly as `relay_stream` does — tests
        /// never start the subscription, so the receiver still sits in the
        /// relay to borrow (take → pump → restore). Panics on timeout: the
        /// pick thread stalled or never answered.
        fn pump_pending_pick(&mut self, timeout: Duration, done: impl Fn(&ViewerPane) -> bool) {
            let events = self
                .relay
                .take_events()
                .expect("relay receiver still seated");
            let started = std::time::Instant::now();
            while !done(self) {
                assert!(
                    started.elapsed() < timeout,
                    "pump timed out after {timeout:?}"
                );
                match events.try_recv() {
                    Ok(event) => {
                        let _ = self.update(relay_message(event));
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        panic!("relay disconnected before the pick answered")
                    }
                }
            }
            self.relay.restore_events(events);
        }
    }

    #[test]
    fn click_while_measuring_adds_a_model_space_point() {
        let (_, mut pane) = pane("pick", &[], &["golden/model.3mf"]);
        // settings default the pane to Exploded; pin Assembled so the
        // camera's center ray runs at the geometry the framing derived
        let _ = pane.update(Message::SetLayout(PartsLayout::Assembled));
        let _ = pane.update(Message::Resized(Size::new(800.0, 600.0)));
        let _ = pane.update(Message::ToggleMeasure);
        // aim at the canvas center: the golden base's top face is under the
        // iso camera's center ray; drive press + tiny move + release
        let _ = pane.update(Message::Moved(Point::new(400.0, 300.0)));
        let _ = pane.update(Message::Pressed);
        let _ = pane.update(Message::Moved(Point::new(401.0, 300.0))); // < 4px: still a click
        let _ = pane.update(Message::Released);
        assert!(pane.pick_pending, "the pick spawned off-thread");
        // the pick ran on a thread and reports through the relay: drain its
        // message into update until Picked lands (bounded, house pattern)
        pane.pump_pending_pick(Duration::from_secs(2), |pane| {
            pane.tools.measure_points.len() == 1
        });
        let p = pane.tools.measure_points[0];
        // golden base spans MODEL z -4..4 (lid 12..22): the center ray with
        // the section OFF hits the lid top at z=22 or the base — assert on-model
        assert!(p[2] > -5.0 && p[2] < 23.0, "hit the model region: {p:?}");
        assert!(pane.scene.markers.is_some(), "markers staged");
        assert!(!pane.pick_pending, "single in-flight cleared on the report");
        // second click completes the pair; the pair stages the link box
        // (the badge's ΔX/ΔY/ΔZ delta form over the pair is parts_overlay's
        // own pinned test — T9's badge mounts over exactly this state)
        let _ = pane.update(Message::Moved(Point::new(430.0, 300.0)));
        let _ = pane.update(Message::Pressed);
        let _ = pane.update(Message::Released);
        let one_point = pane.scene.markers.as_ref().unwrap().vertices.len();
        pane.pump_pending_pick(Duration::from_secs(2), |pane| {
            pane.tools.measure_points.len() == 2
        });
        let markers = pane.scene.markers.as_ref().unwrap();
        // the second sphere (24 segments x 16 rings x 2 triangles) plus the
        // 12-triangle link box (markers.rs geometry)
        let sphere = 24 * 16 * 2 * 3;
        assert_eq!(
            markers.vertices.len() - one_point,
            sphere + 36,
            "two spheres plus the two-point link"
        );
    }

    #[test]
    fn drag_is_not_a_click() {
        let (_, mut pane) = pane("drag", &[], &["golden/model.3mf"]);
        // a real viewport, so a gate removal actually spawns (pick_pending
        // is the observable) instead of dying at spawn_pick's 0x0 early-out
        let _ = pane.update(Message::Resized(Size::new(800.0, 600.0)));
        let _ = pane.update(Message::ToggleMeasure);
        let _ = pane.update(Message::Moved(Point::new(100.0, 100.0)));
        let _ = pane.update(Message::Pressed);
        // the first post-press move only anchors; the second accumulates
        // the travel (CameraInput.move_to) — 199 px is a drag, not a click
        let _ = pane.update(Message::Moved(Point::new(101.0, 100.0)));
        let _ = pane.update(Message::Moved(Point::new(200.0, 200.0)));
        let _ = pane.update(Message::Released);
        assert!(
            !pane.pick_pending,
            "199 px of travel is a drag — the was_click gate must hold the pick"
        );
        // a stationary NON-primary release is not a pick either: the right
        // press is the production path that records the press kind (the pane
        // funnels right releases into Released; upstream picks on left
        // mouseUp only, CameraInputView.swift:50-53)
        let _ = pane.update(Message::Moved(Point::new(400.0, 300.0)));
        let _ = pane.update(Message::RightPressed);
        let _ = pane.update(Message::Released);
        assert!(
            !pane.pick_pending,
            "stationary right release never spawns a pick"
        );
        assert!(pane.tools.measure_points.is_empty());
    }

    #[test]
    fn escape_clears_only_when_measuring() {
        let (_, mut pane) = pane("esc", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::ToggleMeasure);
        pane.tools.measure_points = vec![[1.0; 3], [2.0; 3]];
        pane.rebuild_markers(); // test seam, or set via the same private path
        assert!(pane.scene.markers.is_some());
        let _ = pane.update(Message::Escape);
        assert!(pane.tools.measure_points.is_empty());
        assert!(pane.scene.markers.is_none());
        // Esc with measure off: no panic, no state change
        let _ = pane.update(Message::Escape);
    }

    #[test]
    fn toggling_measure_drops_the_points_and_markers() {
        // ModelViewerState.swift:51-53: ANY toggle — on or off — drops the
        // points collected under the old mode, and the markers follow
        let (_, mut pane) = pane("measure-toggle", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::ToggleMeasure);
        pane.tools.measure_points = vec![[0.0; 3], [1.0, 0.0, 0.0]];
        pane.rebuild_markers();
        assert!(pane.scene.markers.is_some());
        let _ = pane.update(Message::ToggleMeasure);
        assert!(pane.tools.measure_points.is_empty());
        assert!(pane.scene.markers.is_none());
    }

    #[test]
    fn a_miss_or_a_stale_pick_lands_no_point() {
        let (_, mut pane) = pane("miss", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::ToggleMeasure);
        // the ray missed: pending clears, nothing lands
        pane.pick_pending = true;
        let _ = pane.update(Message::Picked(None));
        assert!(!pane.pick_pending, "pending cleared on the report");
        assert!(pane.tools.measure_points.is_empty());
        // a pick that settles after the tool toggled off drops its point —
        // the flight outlived the mode it was fired under
        let _ = pane.update(Message::ToggleMeasure); // off
        pane.pick_pending = true;
        let _ = pane.update(Message::Picked(Some([1.0, 2.0, 3.0])));
        assert!(!pane.pick_pending);
        assert!(
            pane.tools.measure_points.is_empty(),
            "stale pick never lands"
        );
        assert!(pane.scene.markers.is_none());
    }

    #[test]
    fn mesh_reload_clears_the_measurement() {
        let (root, mut pane) = pane("measure-reload", &[], &["golden/model.3mf"]);
        let _ = pane.update(Message::ToggleMeasure);
        pane.tools.measure_points = vec![[0.0; 3], [1.0, 0.0, 0.0]];
        pane.rebuild_markers();
        assert!(pane.scene.markers.is_some());
        // a different model on disk: points collected against the old
        // geometry are meaningless (ModelViewerState.swift:128)
        std::fs::remove_file(root.join("output/model.3mf")).unwrap();
        std::fs::copy(fixture("golden/model.stl"), root.join("output/model.stl")).unwrap();
        let _ = pane.update(Message::Reload(ReloadKind::Mesh));
        assert!(pane.tools.measure_points.is_empty());
        assert!(pane.scene.markers.is_none());
    }

    // ── M6 T12: the compare ghost ─────────────────────────────────────────

    /// A version-store snapshot for the compare tests:
    /// `.cadstudio/versions/<n>/output/model.stl` holding one box, returning
    /// the version directory (the handoff shape the window passes the pane).
    fn write_version(root: &Path, number: i64, bounds: [[f64; 3]; 2]) -> PathBuf {
        let dir = root
            .join(".cadstudio")
            .join("versions")
            .join(number.to_string());
        std::fs::create_dir_all(dir.join("output")).unwrap();
        std::fs::write(dir.join("output").join("model.stl"), stl_of_box(bounds)).unwrap();
        dir
    }

    #[test]
    fn ghost_uses_the_current_scene_pin() {
        let (root, mut pane) = pane("ghost", &[], &["golden/model.3mf"]);
        // a "version" whose model floats at MODEL z 40..60 — above the
        // current scene's z -4..22. Staged with the CURRENT pin it keeps
        // the current floor: golden's pin lifts model z -4 to scene y 0, so
        // the version's z 40 stages at scene y 44. Its OWN pin would read
        // 0 (from_parts grounds every scene at its own floor).
        let vdir = write_version(&root, 1, [[0.0, 0.0, 40.0], [10.0, 10.0, 60.0]]);
        pane.set_compare(Some((1, vdir)));
        let ghost = pane.scene.ghost.as_ref().unwrap();
        assert!(
            (ghost.aabb_min[1] - 44.0).abs() < 1e-3,
            "current pin kept (golden floor model z -4 -> scene y 0): {:?}",
            ghost.aabb_min
        );
        assert_eq!(ghost.parts.len(), 1, "the ghost mesh is the version's STL");
        assert!(pane.comparing);
    }

    #[test]
    fn compare_stages_and_clears_on_none() {
        let (root, mut pane) = pane("ghost2", &[], &["golden/model.3mf"]);
        // the settings default leaves the pane Exploded: construction
        // staged non-identity poses, the precondition for the forcing
        // assertion below
        let identity = || glam::Mat4::IDENTITY.to_cols_array_2d();
        let all_identity = |pane: &ViewerPane| {
            pane.scene
                .poses
                .iter()
                .all(|p| p.map_or(false, |m| m == identity()))
        };
        assert!(
            !all_identity(&pane),
            "precondition: the exploded default scatters the poses"
        );
        let vdir = write_version(&root, 1, [[0.0, 0.0, 40.0], [10.0, 10.0, 60.0]]);
        pane.set_compare(Some((1, vdir)));
        assert!(pane.scene.ghost.is_some(), "a version with a mesh stages");
        assert!(pane.comparing);
        assert!(
            all_identity(&pane),
            "entering compare re-arranges: the effective layout forces Assembled (ModelViewerState.swift:78-86), so the exploded poses collapse under the ghost"
        );
        // None is the ONLY clear: the flag, the ghost, the recorded
        // version, and the arrangement all go together
        pane.set_compare(None);
        assert!(pane.scene.ghost.is_none());
        assert!(!pane.comparing);
        assert!(pane.ghost_version.is_none());
        assert!(!all_identity(&pane), "clearing re-explodes the scene");
        // clearing with nothing staged settles the same way
        pane.set_compare(None);
        assert!(pane.scene.ghost.is_none() && !pane.comparing);
    }

    #[test]
    fn mesh_reload_keeps_the_comparison_restaging_at_the_new_pin() {
        let (root, mut pane) = pane("ghost-reload", &[], &["golden/model.3mf"]);
        let vdir = write_version(&root, 1, [[0.0, 0.0, 40.0], [10.0, 10.0, 60.0]]);
        pane.set_compare(Some((1, vdir)));
        assert!(
            (pane.scene.ghost.as_ref().unwrap().aabb_min[1] - 44.0).abs() < 1e-3,
            "precondition: staged at golden's pin"
        );

        // a different model lands in output/: the scene swaps, and the
        // comparison SURVIVES it (upstream clears on nothing but the
        // picker's None) with the ghost re-staged at the NEW pin — the flat
        // box's floor is model z 0, so the version's z 40 stages at y 40
        std::fs::remove_file(root.join("output/model.3mf")).unwrap();
        std::fs::write(
            root.join("output/model.stl"),
            stl_of_box([[0.0, 0.0, 0.0], [20.0, 20.0, 10.0]]),
        )
        .unwrap();
        let _ = pane.update(Message::Reload(ReloadKind::Mesh));
        assert!(pane.comparing, "the selection survives a mesh reload");
        let ghost = pane.scene.ghost.as_ref().unwrap();
        assert!(
            (ghost.aabb_min[1] - 40.0).abs() < 1e-3,
            "re-staged at the new pin: {:?}",
            ghost.aabb_min
        );
        assert!(
            (ghost.aabb_max[1] - 60.0).abs() < 1e-3,
            "still the VERSION's mesh (its model z 40..60 box)"
        );

        // an idempotent reload (same content) never re-stages: the ghost
        // keeps its Arc identity through the short-circuit
        let ghost_before = pane.scene.ghost.clone().unwrap();
        let _ = pane.update(Message::Reload(ReloadKind::Mesh));
        assert!(
            Arc::ptr_eq(&ghost_before, pane.scene.ghost.as_ref().unwrap()),
            "identical content rebuilds nothing"
        );
    }

    #[test]
    fn comparing_without_a_readable_version_mesh_has_no_ghost() {
        let (root, mut pane) = pane("ghost-none", &[], &["golden/model.3mf"]);
        // a picked version whose output never materialized: upstream's
        // `try?` leaves comparisonMesh nil (ProjectOutput.swift:177) while
        // the selection still holds (it gates the layout and the picker)
        pane.set_compare(Some((7, root.join(".cadstudio/versions/7"))));
        assert!(pane.comparing);
        assert!(pane.scene.ghost.is_none());
        // a real version still stages afterwards
        let vdir = write_version(&root, 8, [[0.0, 0.0, 40.0], [10.0, 10.0, 60.0]]);
        pane.set_compare(Some((8, vdir)));
        assert!(pane.scene.ghost.is_some());
    }
}
