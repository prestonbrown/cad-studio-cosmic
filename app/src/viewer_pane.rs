//! 3D pane — the embedded viewer: [`ViewerPane`] owns the viewer crate's
//! `ModelScene` behind a `mouse_area`, drives it through the pure
//! [`crate::camera_input`] state machine, and reloads the model from the
//! project's `output/` directory through the engine's watcher.
//!
//! Upstream shape (fixtures/upstream-source/Viewer3D/ModelPane.swift): the
//! pane is the shader canvas plus a bottom status bar — controls (the
//! iso/front/top/right preset picker) left, an info label right
//! (Shared/PaneStatusBar.swift). M4 scope binding keeps the status bar
//! MINIMAL: the model size formatted through the display unit
//! (`MeasurementUnit::mm_per_unit`/`fraction_digits`) and the
//! missing-output count from [`engine::project::outputs::missing_requirements`];
//! the grid/wireframe/measure/section controls and the full ActivityViewer
//! port land in their own milestones. MSAA is an OPT-IN pane option
//! (default 1×): the spec §5 decision renders the mesh pass through
//! `MeshPipeline::with_msaa`'s 4× pipeline set, reached by flipping
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

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cosmic::Element;
use cosmic::iced::futures::StreamExt;
use cosmic::iced::futures::stream::BoxStream;
use cosmic::iced::keyboard::{self, Modifiers};
use cosmic::iced::mouse::ScrollDelta;
use cosmic::iced::stream;
use cosmic::iced::widget::{Stack, responsive, shader};
use cosmic::iced::{Length, Point, Size, Subscription, Task};
use cosmic::widget::{Column, Row, Space, button, container, mouse_area, text};
use engine::parsing::Part;
use engine::parsing::stl::parse_stl;
use engine::parsing::threemf::parse_3mf;
use engine::project::manifest::Manifest;
use engine::project::outputs::{self, MissingCtx};
use engine::project::parameters::ModelParameter;
use engine::settings::{MeasurementUnit, PartsLayout};
use engine::watch::{ReloadKind, WatchHandle, watch_directory};
use viewer::render::markers;
use viewer::render::{ModelScene, NEUTRAL, PartScene, camera, camera::Preset};
use viewer::tools::picking;

use crate::camera_input::CameraInput;
use crate::parameters::ParametersPanel;
use crate::parameters_view::{self, OverlayContext};
use crate::viewer_tools::ViewerTools;

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
    /// Toggle measure mode (ModelViewerState.swift:51-53): any change drops
    /// the points collected under the old mode, markers with them.
    ToggleMeasure,
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
    /// The viewer tool state (T8's ModelViewerState port — grid,
    /// wireframe, measure, section, layout flags the pane's messages and
    /// views drive; the pane owns the instance).
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
    /// (and no model) — the pane still works.
    pub fn new(project_root: &Path, unit: MeasurementUnit, printer_model: Option<String>) -> Self {
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
        let mut pane = ViewerPane {
            scene: ModelScene::new(empty),
            input: CameraInput::new([0.0; 3], [0.0; 3]),
            preset: Preset::Iso,
            msaa: false,
            unit,
            warnings: 0,
            parameters: ParametersPanel::from_parameters(&[], false),
            parameter_drafts: HashMap::new(),
            committed_parameters: BTreeMap::new(),
            parameters_visible: true,
            // the settings defaults (Exploded / not collapsed); the pane's
            // layout wiring owns reading the persisted values
            tools: ViewerTools::new(PartsLayout::Exploded, false),
            last_cursor: Point::ORIGIN,
            last_press_primary: false,
            pick_pending: false,
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
                // feeds the status bar's warning count. Schematic reloads
                // are the schematic pane's (its own watcher wiring).
                ReloadKind::Mesh | ReloadKind::Plate | ReloadKind::Manifest => {
                    self.reload_outputs()
                }
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
            Message::ToggleMeasure => {
                self.tools.toggle_measuring();
                self.rebuild_markers();
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

    /// The 3D pane's element: the shader canvas in a mouse area over the
    /// status bar (upstream ModelPane's VStack: content then
    /// PaneStatusBar).
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
        // The overlay floats over the canvas, top-trailing with 10 px of
        // padding (ModelPane.swift:41-44). `None` (or upstream's gate — no
        // model, no parameters, hidden) skips the layer entirely.
        let content: Element<'_, Message> = match overlay.filter(|_| self.show_parameters_overlay())
        {
            Some(ctx) => {
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
                Stack::new()
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .push(content)
                    .push(positioned)
                    .into()
            }
            None => content,
        };
        Column::new()
            .push(content)
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

    /// Re-read everything model-shaped from `output/` and replace the
    /// scene — idempotent by construction (see the module docs). Also
    /// refreshes the manifest and the status bar's warning count.
    fn reload_outputs(&mut self) {
        self.manifest = Manifest::load(&self.output_dir.join("manifest.json")).ok();
        if let Some(parts) = load_model_parts(&self.output_dir) {
            let next = Arc::new(PartScene::from_parts(&parts, NEUTRAL));
            if scenes_differ(&self.scene.scene, &next) {
                self.input.mesh_changed(next.aabb_min, next.aabb_max);
                self.scene.replace_scene(next);
                // points collected against the old geometry are meaningless
                // (ModelViewerState.swift:128) — and their markers go with
                // them (replace_scene already dropped the staged mesh)
                self.tools.mesh_changed();
                self.rebuild_markers();
                self.sync_pose();
            }
        }
        self.refresh_warnings();
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
    /// pass is running, so a click never answers through geometry the user
    /// cannot see. The pane's section wiring (its `apply_arrangement`)
    /// owns the live tuple; with no section state wired the tool never
    /// sections and every pick runs the whole scene.
    fn pick_section(&self) -> Option<(u32, f32)> {
        None
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
        container(
            Row::new()
                .push(self.preset_picker())
                .push(Space::new().width(Length::Fill))
                .push(self.msaa_toggle())
                .push(Space::new().width(8))
                .push(text(status_info(
                    (!self.scene.scene.parts.is_empty())
                        .then(|| model_size(self.scene.scene.aabb_min, self.scene.scene.aabb_max)),
                    self.unit,
                    self.warnings,
                )))
                .padding([4.0, 12.0]),
        )
        .width(Length::Fill)
        .into()
    }

    /// The iso/front/top/right preset bar (upstream's segmented
    /// `Picker("Vista")`, ModelPane.swift:121-131): one button per preset,
    /// the selected one in the suggested style. The segmented-control
    /// widget's model borrows its owner, which a per-view rebuild cannot
    /// satisfy — four stateless buttons carry the same selection.
    fn preset_picker(&self) -> Element<'_, Message> {
        const PRESETS: [(Preset, &str); 4] = [
            (Preset::Iso, "Iso"),
            (Preset::Front, "Front"),
            (Preset::Top, "Top"),
            (Preset::Right, "Right"),
        ];
        let mut bar = Row::new().spacing(4);
        for (preset, label) in PRESETS {
            bar = bar.push(
                button::text(label)
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

/// The status bar's info label (upstream ModelPane.info, M4-minimal): the
/// model size `X × Y × Z unit` when a model shows, plus the missing-output
/// count when one is missing. Empty when there is nothing to say.
pub fn status_info(size: Option<[f64; 3]>, unit: MeasurementUnit, warnings: usize) -> String {
    let mut info = match size {
        // upstream puts the symbol on the last term only
        Some([x, y, z]) => format!(
            "{} × {} × {}",
            format_length(x, unit, false),
            format_length(y, unit, false),
            format_length(z, unit, true),
        ),
        None => String::new(),
    };
    if warnings > 0 {
        if !info.is_empty() {
            info.push_str(" · ");
        }
        info.push_str(&format!("{warnings} warnings"));
    }
    info
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

    fn pane(tag: &str, root_files: &[&str], output_files: &[&str]) -> (PathBuf, ViewerPane) {
        let root = mk_project(tag, root_files, output_files);
        let pane = ViewerPane::new(&root, MeasurementUnit::Millimeters, None);
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
    fn status_info_combines_size_and_warnings() {
        let unit = MeasurementUnit::Millimeters;
        assert_eq!(
            status_info(Some([30.0, 15.5, 15.5]), unit, 0),
            "30.0 × 15.5 × 15.5 mm"
        );
        assert_eq!(
            status_info(Some([30.0, 15.5, 15.5]), unit, 2),
            "30.0 × 15.5 × 15.5 mm · 2 warnings"
        );
        assert_eq!(status_info(None, unit, 3), "3 warnings");
        assert_eq!(status_info(None, unit, 0), "");
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
        let _ = pane.update(Message::ToggleMeasure);
        let _ = pane.update(Message::Moved(Point::new(100.0, 100.0)));
        let _ = pane.update(Message::Pressed);
        let _ = pane.update(Message::Moved(Point::new(200.0, 200.0)));
        let _ = pane.update(Message::Released);
        assert!(pane.tools.measure_points.is_empty(), "drag never picks");
        // a stationary non-primary release is not a pick either: mirror a right
        // press through the recorded press kind (upstream: left mouseUp only)
        pane.last_press_primary = false; // test seam
        let _ = pane.update(Message::Moved(Point::new(400.0, 300.0)));
        let _ = pane.update(Message::Pressed);
        let _ = pane.update(Message::Released);
        assert!(
            pane.tools.measure_points.is_empty(),
            "non-primary release never picks"
        );
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
}
