//! Schematic pane — the SPEC §5 raster/cache drawing display.
//!
//! Upstream (fixtures/upstream-source/Schematic/) loads each drawing SVG
//! into a WKWebView whose JS bridge does the zoom/pan/fit math
//! (`setZoom`, drag = `window.scrollBy(-movementX, -movementY)`, wheel =
//! exponential zoom at the cursor, fit = min ratio over view − 48 px).
//! COSMIC has no web view in scope, so the port rasterizes each page with
//! resvg per (page, zoom bucket) on a background `std::thread`, caches the
//! rasters (LRU, byte budget), displays them through
//! `image::Handle::from_rgba` with GPU scaling in between buckets, and
//! pans by translating the displayed raster. The pure math in [`Viewport`]
//! is the JS-bridge port and is pinned to the Swift sources.
//!
//! Documented port additions (no upstream counterpart): power-of-two zoom
//! buckets with a debounced re-raster scheduler, the LRU raster cache, and
//! a PNG fallback (`schematic.png` sibling) when an SVG fails to parse —
//! upstream just shows a blank page in that case.
//!
//! M4 status: this module is the pane model + view; the shell task wires it
//! into a window (zoom/fit menu commands, i18n via the extracted ftl keys,
//! and the "Open in Preview" external handoff all land there).

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cosmic::Element;
use cosmic::iced::alignment::Alignment;
use cosmic::iced::futures::StreamExt;
use cosmic::iced::futures::stream::BoxStream;
use cosmic::iced::mouse::{Interaction, ScrollDelta};
use cosmic::iced::stream;
use cosmic::iced::time;
use cosmic::iced::widget::image::{FilterMethod, Handle};
use cosmic::iced::widget::responsive;
use cosmic::iced::{Length, Padding, Point, Rectangle, Size, Subscription, Task};
use cosmic::widget::{Column, Image, Row, Space, button, container, mouse_area, text};
use engine::project::manifest::Manifest;

use crate::fl;

// ---------------------------------------------------------------------------
// Parity constants (pinned to the Swift sources)
// ---------------------------------------------------------------------------

/// `SchematicViewer.zoomRange` (SchematicViewer.swift:7).
pub const ZOOM_MIN: f64 = 0.1;
/// `SchematicViewer.zoomRange` (SchematicViewer.swift:7).
pub const ZOOM_MAX: f64 = 8.0;
/// Fit margin subtracted from the view before the min scale ratio
/// (SchematicViewer.swift:44). The JS body's 24 px padding is half of it.
pub const FIT_MARGIN: f64 = 48.0;
/// The `body { padding: 24px }` the sheet floats in (SVGWebView.swift:44).
pub const SHEET_MARGIN: f64 = FIT_MARGIN / 2.0;
/// Zoom-in/out buttons (SchematicViewer.swift:16-24, SchematicPane.swift:56-69).
pub const ZOOM_BUTTON_FACTOR: f64 = 1.25;
/// Wheel gain and step clamp (SVGWebView.swift:85-87).
pub const WHEEL_GAIN: f64 = 0.0025;
pub const WHEEL_STEP_MAX: f64 = 0.2;
/// `deltaMode === 1` line height the JS multiplies in (SVGWebView.swift:85).
pub const WHEEL_LINE_PX: f64 = 16.0;

// ---------------------------------------------------------------------------
// Raster/cache policy (SPEC §5)
// ---------------------------------------------------------------------------

/// Rasters are made at power-of-two scales so the display only downsamples
/// a cached raster between re-rasters. Index 3 = 8× covers `ZOOM_MAX`;
/// index −3 = 0.125× covers `ZOOM_MIN`.
pub const BUCKET_MIN: i32 = -3;
pub const BUCKET_MAX: i32 = 3;
/// Longest raster edge; bucket 8× of an A4 sheet would be ~9000 px
/// (~130 MB of RGBA), the cap keeps every entry well under the cache budget.
pub const RASTER_MAX_EDGE: f64 = 4096.0;
/// Default cache budget (bytes of RGBA pixel data).
pub const CACHE_BUDGET: usize = 128 * 1024 * 1024;
/// A wheel burst must settle for this long before a re-raster is sent.
pub const RASTER_DEBOUNCE: Duration = Duration::from_millis(120);
/// Resolution of the debounce tick subscription while one is pending.
pub const DEBOUNCE_TICK: Duration = Duration::from_millis(60);
/// Relay retry cadence when the stream channel is momentarily full.
pub const RELAY_RETRY_MIN: Duration = Duration::from_millis(1);
pub const RELAY_RETRY_MAX: Duration = Duration::from_millis(64);
/// Total time to keep retrying one full-channel message before dropping it
/// (liveness over completeness; the pane re-requests dropped rasters).
pub const RELAY_RETRY_BUDGET: Duration = Duration::from_secs(2);

/// The raster bucket a display scale maps to (raster scale ≥ display scale,
/// so display always downsamples).
pub fn bucket_index(scale: f64) -> i32 {
    if !(scale > 0.0) {
        return BUCKET_MIN;
    }
    (scale.log2().ceil() as i32).clamp(BUCKET_MIN, BUCKET_MAX)
}

/// The raster scale of a bucket index.
pub fn bucket_scale(index: i32) -> f64 {
    2.0f64.powi(index)
}

/// Bucket scale clamped so the raster's long edge fits `RASTER_MAX_EDGE`.
pub fn raster_scale_for(sheet: (f64, f64), index: i32) -> f64 {
    let long_edge = sheet.0.max(sheet.1).max(1.0);
    bucket_scale(index).min(RASTER_MAX_EDGE / long_edge)
}

// ---------------------------------------------------------------------------
// Viewport — the JS-bridge zoom/pan/fit math, pure
// ---------------------------------------------------------------------------

/// Zoom/pan state. `offset` is the sheet's top-left corner in view
/// coordinates: the scroll position of the upstream web view negated, which
/// makes the flex-centered sheet sit at `(view − sheet·scale)/2` when it
/// fits and clamp against the 24 px body padding when it overflows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub scale: f64,
    pub offset: (f64, f64),
    pub view: (f64, f64),
    pub sheet: Option<(f64, f64)>,
    /// Upstream `isFitted`: zoom gestures clear it, `fit()` sets it, a
    /// resize re-fits only while it is set.
    pub fitted: bool,
}

impl Viewport {
    /// Upstream starts at `zoom = 1.0`, `isFitted = true`, no sheet yet.
    pub fn new(view: (f64, f64)) -> Self {
        Viewport {
            scale: 1.0,
            offset: (0.0, 0.0),
            view,
            sheet: None,
            fitted: true,
        }
    }

    /// `fit()` (SchematicViewer.swift:41-46): min ratio over `view − 48`,
    /// clamped to the zoom range, anchored at the view center.
    pub fn fit(&mut self) {
        self.fitted = true;
        let Some((sheet_w, sheet_h)) = self.sheet else {
            return;
        };
        if sheet_w <= 0.0 || sheet_h <= 0.0 || self.view.0 <= 0.0 || self.view.1 <= 0.0 {
            return;
        }
        let avail = (self.view.0 - FIT_MARGIN, self.view.1 - FIT_MARGIN);
        let scale = (avail.0 / sheet_w).min(avail.1 / sheet_h);
        self.set_zoom(scale, None);
    }

    /// `zoomIn()` / `zoomOut()` (SchematicViewer.swift:16-24).
    pub fn zoom_in(&mut self) {
        self.zoom_by(ZOOM_BUTTON_FACTOR, None);
    }

    pub fn zoom_out(&mut self) {
        self.zoom_by(1.0 / ZOOM_BUTTON_FACTOR, None);
    }

    /// `zoom(by:at:)` (SchematicViewer.swift:30-33): clears `fitted`, then
    /// `setZoom` at the anchor (view center when `None`).
    pub fn zoom_by(&mut self, factor: f64, anchor: Option<(f64, f64)>) {
        self.fitted = false;
        self.set_zoom(self.scale * factor, anchor);
    }

    /// The JS wheel handler (SVGWebView.swift:82-88): step is
    /// `clamp(−pixels·0.0025, ±0.2)` applied exponentially at the cursor.
    pub fn wheel(&mut self, pixels: f64, cursor: (f64, f64)) {
        self.zoom_by(wheel_factor(pixels), Some(cursor));
    }

    /// Drag pan: `window.scrollBy(-movementX, -movementY)` moves the sheet
    /// with the cursor, clamped to the body margins.
    pub fn pan(&mut self, delta: (f64, f64)) {
        self.offset.0 += delta.0;
        self.offset.1 += delta.1;
        self.clamp_offset();
    }

    /// `viewDidResize` (SchematicViewer.swift:35-39).
    pub fn view_resized(&mut self, view: (f64, f64)) {
        self.view = view;
        if self.fitted {
            self.fit();
        } else {
            self.clamp_offset();
        }
    }

    /// `sheetDidLoad(size:)` (SchematicViewer.swift:48-56): the first load
    /// (and every load while fitted) fits; later loads keep the zoom.
    pub fn sheet_loaded(&mut self, sheet: (f64, f64)) {
        let first = self.sheet.is_none();
        self.sheet = Some(sheet);
        if first || self.fitted {
            self.fit();
        } else {
            self.set_zoom(self.scale, None);
        }
    }

    /// `setZoom(_:anchor:)` (SchematicViewer.swift:58-65) plus the JS
    /// `setZoom(zoom, x, y)` anchor math (SVGWebView.swift:52-62): the sheet
    /// point under the anchor stays under the anchor.
    fn set_zoom(&mut self, value: f64, anchor: Option<(f64, f64)>) {
        let new_scale = value.clamp(ZOOM_MIN, ZOOM_MAX);
        let anchor = anchor.unwrap_or((self.view.0 / 2.0, self.view.1 / 2.0));
        let sheet_point = (
            (anchor.0 - self.offset.0) / self.scale,
            (anchor.1 - self.offset.1) / self.scale,
        );
        self.scale = new_scale;
        self.offset.0 = anchor.0 - sheet_point.0 * new_scale;
        self.offset.1 = anchor.1 - sheet_point.1 * new_scale;
        self.clamp_offset();
    }

    /// The body's flex centering and scroll clamping: centered while the
    /// sheet fits inside the margins, else clamped to the 24 px padding.
    fn clamp_offset(&mut self) {
        let Some((sheet_w, sheet_h)) = self.sheet else {
            return;
        };
        let (vw, vh) = self.view;
        self.offset.0 = clamp_axis(sheet_w * self.scale, vw, self.offset.0);
        self.offset.1 = clamp_axis(sheet_h * self.scale, vh, self.offset.1);
    }
}

/// The wheel step factor `exp(clamp(−pixels·0.0025, ±0.2))`
/// (SVGWebView.swift:85-87).
pub fn wheel_factor(pixels: f64) -> f64 {
    (-(pixels * WHEEL_GAIN))
        .clamp(-WHEEL_STEP_MAX, WHEEL_STEP_MAX)
        .exp()
}

/// One axis of the body layout: the flex box centers the sheet while it
/// fits inside the 48 px margin; once it overflows, scrolling is clamped
/// so the 24 px body padding stays visible (SVGWebView.swift:44, 61).
fn clamp_axis(disp: f64, view: f64, offset: f64) -> f64 {
    if disp + FIT_MARGIN <= view {
        (view - disp) / 2.0
    } else {
        offset.clamp(view - disp - SHEET_MARGIN, SHEET_MARGIN)
    }
}

/// Secondary text at 65% of the ambient color — the empty-state hero's
/// description (the welcome window's dim-text pattern).
fn dim_text_class() -> cosmic::theme::Text {
    cosmic::theme::Text::Custom(|theme| {
        let mut color = cosmic::iced::Color::from(theme.cosmic().background(false).on);
        color.a *= 0.65;
        cosmic::iced::widget::text::Style {
            color: Some(color),
            ..Default::default()
        }
    })
}

/// iced scroll deltas are pixels; the JS multiplies `deltaMode === 1`
/// (lines) by 16 before using them (SVGWebView.swift:85).
pub fn wheel_pixels(delta: ScrollDelta) -> f64 {
    match delta {
        ScrollDelta::Pixels { y, .. } => y as f64,
        ScrollDelta::Lines { y, .. } => y as f64 * WHEEL_LINE_PX,
    }
}

// ---------------------------------------------------------------------------
// RasterScheduler — debounced (page, bucket) re-raster decisions, pure
// ---------------------------------------------------------------------------

/// Decides when a scale change warrants a re-raster: nothing while the
/// bucket is unchanged, and after the scale settles for `RASTER_DEBOUNCE`
/// at most one raster per bucket. Time is injected, so the whole policy is
/// testable without a clock.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RasterScheduler {
    rendered: Option<i32>,
    inflight: Option<i32>,
    target: Option<i32>,
    deadline: Option<Instant>,
}

impl RasterScheduler {
    /// A scale change re-arms the debounce; every event in a burst extends
    /// it, so only the settled bucket is ever rastered.
    pub fn scale_changed(&mut self, scale: f64, now: Instant) {
        let bucket = bucket_index(scale);
        if Some(bucket) == self.rendered {
            self.target = None;
            self.deadline = None;
        } else {
            self.target = Some(bucket);
            self.deadline = Some(now + RASTER_DEBOUNCE);
        }
    }

    /// The raster to send now, if the debounce has expired. `None` while
    /// the target equals the rendered or in-flight bucket.
    pub fn poll(&mut self, now: Instant) -> Option<i32> {
        let bucket = self.target?;
        if self.deadline.is_some_and(|deadline| now < deadline) {
            return None;
        }
        self.target = None;
        self.deadline = None;
        if Some(bucket) == self.inflight {
            return None;
        }
        self.inflight = Some(bucket);
        Some(bucket)
    }

    /// Cache hit path: this bucket is on screen without a worker round trip.
    pub fn mark_rendered(&mut self, bucket: i32) {
        self.rendered = Some(bucket);
        if self.inflight == Some(bucket) {
            self.inflight = None;
        }
    }

    /// A worker outcome landed; still-current buckets count as rendered.
    pub fn raster_arrived(&mut self, bucket: i32, current_scale: f64) {
        if self.inflight == Some(bucket) {
            self.inflight = None;
        }
        if bucket_index(current_scale) == bucket {
            self.rendered = Some(bucket);
        }
    }

    /// Page load: raster immediately, no debounce.
    pub fn load_now(&mut self, bucket: i32) {
        self.target = Some(bucket);
        self.deadline = None;
    }

    /// Page switch: nothing about the old page's rasters applies.
    pub fn reset(&mut self) {
        *self = RasterScheduler::default();
    }

    /// Whether a debounce is armed (drives the tick subscription).
    pub fn pending(&self) -> bool {
        self.target.is_some()
    }

    /// The bucket the current debounce is settling toward.
    pub fn target_bucket(&self) -> Option<i32> {
        self.target
    }
}

// ---------------------------------------------------------------------------
// PageTabs — the drawing-page model, pure
// ---------------------------------------------------------------------------

/// Upstream `DrawingPage` (ProjectOutput.swift:190).
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub file: PathBuf,
    pub title: String,
}

/// Titled pages in display order. Upstream builds these in
/// `ProjectOutput.reload()`: discovered `schematic*.svg` files titled from
/// the manifest's `drawings` (matched on file name), falling back to
/// "Pagina N" (ProjectOutput.swift:52-58); the page index clamps to
/// `count − 1` (SchematicPane.swift:14).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PageTabs {
    pages: Vec<Page>,
    current: usize,
}

impl PageTabs {
    /// Titles pages from the manifest drawings; the fallback title reads
    /// the `pagina-lld` catalog key.
    pub fn from_parts(files: &[PathBuf], manifest: Option<&Manifest>) -> Self {
        let pages = files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let title = manifest
                    .and_then(|manifest| {
                        manifest
                            .drawings
                            .iter()
                            .find(|drawing| {
                                Path::new(&drawing.file).file_name() == file.file_name()
                            })
                            .map(|drawing| drawing.title.clone())
                    })
                    .unwrap_or_else(|| fl!("pagina-lld", page = index + 1));
                Page {
                    file: file.clone(),
                    title,
                }
            })
            .collect();
        PageTabs { pages, current: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    pub fn current(&self) -> Option<&Page> {
        let n = self.pages.len();
        if n == 0 {
            None
        } else {
            self.pages.get(self.current.min(n - 1))
        }
    }

    /// The clamped current index (upstream `min(viewer.page, count - 1)`).
    pub fn current_index(&self) -> Option<usize> {
        let n = self.pages.len();
        (n > 0).then(|| self.current.min(n - 1))
    }

    /// Upstream `viewer.page = offset` (the page menu) — clamped.
    pub fn select(&mut self, index: usize) {
        if !self.pages.is_empty() {
            self.current = index.min(self.pages.len() - 1);
        }
    }

    /// Upstream prev/next buttons (SchematicPane.swift:87, 108).
    pub fn step(&mut self, delta: i32) {
        if !self.pages.is_empty() {
            let last = self.pages.len() as i32 - 1;
            let current = self.current.min(self.pages.len() - 1) as i32;
            self.current = (current + delta).clamp(0, last) as usize;
        }
    }

    pub fn titles(&self) -> Vec<String> {
        self.pages.iter().map(|p| p.title.clone()).collect()
    }
}

// ---------------------------------------------------------------------------
// Rasterization — resvg, with a PNG fallback
// ---------------------------------------------------------------------------

/// A rasterized page: RGBA pixels plus the SVG's natural (CSS px) size the
/// viewport fit math needs.
#[derive(Debug, Clone)]
pub struct Raster {
    pub natural: (f64, f64),
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Rasterize SVG bytes at `raster_scale` (1.0 = natural size).
pub fn rasterize_svg(data: &[u8], raster_scale: f64) -> Result<Raster, String> {
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(data, &options)
        .map_err(|error| format!("svg parse: {error}"))?;
    let size = tree.size();
    let natural = (size.width() as f64, size.height() as f64);
    if !(natural.0 > 0.0 && natural.1 > 0.0) {
        return Err("svg has no intrinsic size".to_string());
    }
    let scale = if raster_scale > 0.0 {
        raster_scale
    } else {
        1.0
    };
    let width = ((natural.0 * scale).round() as u32).max(1);
    let height = ((natural.1 * scale).round() as u32).max(1);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| "pixmap allocation failed".to_string())?;
    // upstream paints the <img> white (SVGWebView.swift:45); filling the
    // raster gives the transparent matplotlib paper the same read in dark
    // themes
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    let transform = resvg::tiny_skia::Transform::from_scale(
        width as f32 / size.width(),
        height as f32 / size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Ok(Raster {
        natural,
        width,
        height,
        pixels: pixmap.data().to_vec(),
    })
}

/// Rasterize a page file: the SVG, falling back to its `.png` sibling at
/// natural size when the SVG fails to parse or renders at zero size.
pub fn rasterize_page(path: &Path, raster_scale: f64) -> Result<Raster, String> {
    let svg = std::fs::read(path).map_err(|error| format!("svg read: {error}"))?;
    match rasterize_svg(&svg, raster_scale) {
        Ok(raster) => Ok(raster),
        Err(_) => {
            // the documented port addition: fall back to the generated PNG
            // (upstream shows a blank page for a broken SVG)
            let png_path = path.with_extension("png");
            let png = std::fs::read(&png_path).map_err(|error| format!("png read: {error}"))?;
            let image = image::load_from_memory(&png)
                .map_err(|error| format!("png decode: {error}"))?
                .to_rgba8();
            let (width, height) = (image.width(), image.height());
            if width == 0 || height == 0 {
                return Err("png is empty".to_string());
            }
            Ok(Raster {
                natural: (width as f64, height as f64),
                width,
                height,
                pixels: image.into_raw(),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// RasterCache — LRU keyed by (page, bucket)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CacheEntry {
    handle: Handle,
    raster: (u32, u32),
    natural: (f64, f64),
    bytes: usize,
}

/// LRU of raster handles under a byte budget. `get` refreshes recency and
/// clones the handle (the iced texture id is shared, so no re-upload).
///
/// ## Revision contract (for the shell task)
///
/// The key carries the pane's content revision, the port's analog of
/// upstream `ProjectOutput.schematicRevision`: when a page's SVG changes on
/// disk, the caller MUST bump the revision it passes here (via
/// [`SchematicPane::set_revision`]) — stale entries then simply never match
/// and age out through the LRU budget instead of showing old drawings.
pub struct RasterCache {
    entries: HashMap<(usize, u64, i32), CacheEntry>,
    order: VecDeque<(usize, u64, i32)>,
    budget: usize,
    used: usize,
}

impl RasterCache {
    pub fn new(budget: usize) -> Self {
        RasterCache {
            entries: HashMap::new(),
            order: VecDeque::new(),
            budget,
            used: 0,
        }
    }

    /// The handle plus raster/natural sizes, for the display and fit math.
    pub fn get(
        &mut self,
        page: usize,
        revision: u64,
        bucket: i32,
    ) -> Option<(Handle, (u32, u32), (f64, f64))> {
        let key = (page, revision, bucket);
        if !self.entries.contains_key(&key) {
            return None;
        }
        if let Some(position) = self.order.iter().position(|k| *k == key) {
            self.order.remove(position);
            self.order.push_back(key);
        }
        self.entries
            .get(&key)
            .map(|entry| (entry.handle.clone(), entry.raster, entry.natural))
    }

    pub fn insert(&mut self, page: usize, revision: u64, bucket: i32, raster: Raster) {
        let key = (page, revision, bucket);
        let bytes = raster.pixels.len();
        let entry = CacheEntry {
            handle: Handle::from_rgba(raster.width, raster.height, raster.pixels),
            raster: (raster.width, raster.height),
            natural: raster.natural,
            bytes,
        };
        if let Some(old) = self.entries.insert(key, entry) {
            self.used -= old.bytes;
            if let Some(position) = self.order.iter().position(|k| *k == key) {
                self.order.remove(position);
            }
        }
        self.order.push_back(key);
        self.used += bytes;
        while self.used > self.budget {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.entries.remove(&oldest) {
                self.used -= old.bytes;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SchematicPane — model + view composing the pieces above
// ---------------------------------------------------------------------------

/// Events from the raster threads and the view's size report, funneled
/// through one std channel into the subscription's relay stream.
enum PaneEvent {
    Raster {
        job: RasterJob,
        raster: Result<Raster, String>,
    },
    Resized(Size),
}

/// A tiny blank raster for tests.
#[cfg(test)]
fn test_raster() -> Raster {
    Raster {
        natural: (4.0, 2.0),
        width: 4,
        height: 2,
        pixels: vec![255u8; 4 * 2 * 4],
    }
}

struct RasterJob {
    page: usize,
    revision: u64,
    bucket: i32,
    path: PathBuf,
    raster_scale: f64,
}

fn spawn_raster(job: RasterJob, events: Sender<PaneEvent>) {
    let _ = thread::Builder::new()
        .name("schematic-raster".to_string())
        .spawn(move || {
            let raster = rasterize_page(&job.path, job.raster_scale);
            // a closed channel means the pane is gone; the send failure is
            // this thread's cue to end
            let _ = events.send(PaneEvent::Raster { job, raster });
        });
}

/// The subscription's identity + the event receiver slot it takes once.
struct RasterRelay {
    id: u64,
    events: Mutex<Option<Receiver<PaneEvent>>>,
}

impl RasterRelay {
    fn take_events(&self) -> Option<Receiver<PaneEvent>> {
        self.events.lock().unwrap().take()
    }
}

// Hash covers only the id: every Arc clone of the same relay must identify
// the same subscription so the stream is never rebuilt mid-pane.
impl Hash for RasterRelay {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// The pane's messages. Zoom/fit keyboard shortcuts and the page menu are
/// the shell's commands on top of these.
#[derive(Clone, Debug)]
pub enum Message {
    /// Wheel over the sheet; the cursor comes from hover tracking because
    /// iced's `on_scroll` carries no position.
    Wheel(ScrollDelta),
    CursorMoved(Point),
    Pressed,
    Released,
    DoubleClick,
    ZoomIn,
    ZoomOut,
    Fit,
    SelectPage(usize),
    PrevPage,
    NextPage,
    Resized(Size),
    DebounceTick,
    /// A raster finished on a background thread (or both the SVG and its
    /// PNG fallback failed).
    RasterArrived {
        page: usize,
        revision: u64,
        bucket: i32,
        raster: Result<Raster, String>,
    },
}

/// What the sheet area is showing: the cached handle plus its raster size
/// (the crop math needs the raster's own dimensions).
#[derive(Clone)]
struct Displayed {
    handle: Handle,
    raster: (u32, u32),
}

pub struct SchematicPane {
    pub tabs: PageTabs,
    pub viewport: Viewport,
    /// Content revision analog of upstream `schematicRevision`; see
    /// [`RasterCache`]'s revision contract.
    revision: u64,
    scheduler: RasterScheduler,
    cache: RasterCache,
    displayed: Option<Displayed>,
    dragging: bool,
    cursor: Option<Point>,
    last_cursor: Point,
    events: Sender<PaneEvent>,
    relay: Arc<RasterRelay>,
    last_size: Mutex<Size>,
}

impl SchematicPane {
    pub fn new(files: &[PathBuf], manifest: Option<&Manifest>) -> Self {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let (events, receiver) = mpsc::channel();
        let tabs = PageTabs::from_parts(files, manifest);
        let mut pane = SchematicPane {
            tabs,
            viewport: Viewport::new((900.0, 700.0)),
            revision: 0,
            scheduler: RasterScheduler::default(),
            cache: RasterCache::new(CACHE_BUDGET),
            displayed: None,
            dragging: false,
            cursor: None,
            last_cursor: Point::default(),
            events,
            relay: Arc::new(RasterRelay {
                id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                events: Mutex::new(Some(receiver)),
            }),
            last_size: Mutex::new(Size::default()),
        };
        // upstream loads the current page immediately (no debounce on load)
        pane.request_current_page();
        pane
    }

    /// The shell calls this when the watcher sees a page's content change
    /// (upstream bumps `schematicRevision` and the web view reloads): the
    /// current page re-rasters under the new revision; cached rasters of
    /// older revisions stop matching.
    pub fn set_revision(&mut self, revision: u64) {
        if revision == self.revision {
            return;
        }
        self.revision = revision;
        // upstream reloads the web view when the revision changes: same
        // flow as a page switch, same page
        self.scheduler.reset();
        self.displayed = None;
        self.request_current_page();
    }

    /// The shell's output-watch handoff (upstream `output.reload()`'s page
    /// re-discovery plus the revision bump): pages re-derive from the files
    /// on disk and the manifest's titles, the current page stays selected
    /// when it survives, and content revisions advance so stale cached
    /// rasters stop matching.
    pub fn reload(&mut self, files: &[PathBuf], manifest: Option<&Manifest>) {
        let selected = self.tabs.current().map(|page| page.file.clone());
        self.tabs = PageTabs::from_parts(files, manifest);
        if let Some(file) = selected {
            if let Some(index) = files.iter().position(|f| *f == file) {
                self.tabs.select(index);
            }
        }
        self.set_revision(self.revision.wrapping_add(1));
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Wheel(delta) => {
                let pixels = wheel_pixels(delta);
                if pixels != 0.0 {
                    let (vw, vh) = self.viewport.view;
                    let cursor = self
                        .cursor
                        .map(|p| (p.x as f64, p.y as f64))
                        .unwrap_or((vw / 2.0, vh / 2.0));
                    self.viewport.wheel(pixels, cursor);
                    self.zoom_changed();
                }
            }
            Message::CursorMoved(point) => {
                if self.dragging {
                    let delta = (point.x - self.last_cursor.x, point.y - self.last_cursor.y);
                    self.viewport.pan((delta.0 as f64, delta.1 as f64));
                }
                self.last_cursor = point;
                self.cursor = Some(point);
            }
            Message::Pressed => self.dragging = true,
            Message::Released => self.dragging = false,
            Message::DoubleClick | Message::Fit => {
                self.viewport.fit();
                self.zoom_changed();
            }
            Message::ZoomIn => {
                self.viewport.zoom_in();
                self.zoom_changed();
            }
            Message::ZoomOut => {
                self.viewport.zoom_out();
                self.zoom_changed();
            }
            Message::SelectPage(index) => {
                self.tabs.select(index);
                self.page_changed();
            }
            Message::PrevPage => {
                self.tabs.step(-1);
                self.page_changed();
            }
            Message::NextPage => {
                self.tabs.step(1);
                self.page_changed();
            }
            Message::Resized(size) => {
                self.viewport
                    .view_resized((size.width as f64, size.height as f64));
                self.zoom_changed();
            }
            Message::DebounceTick => self.flush(),
            Message::RasterArrived {
                page,
                revision,
                bucket,
                raster,
            } => self.raster_arrived(page, revision, bucket, raster),
        }
        Task::none()
    }

    /// Rasters that land for the current page update the viewport's sheet
    /// size (upstream `sheetDidLoad`) and the display when still current.
    fn raster_arrived(
        &mut self,
        page: usize,
        revision: u64,
        bucket: i32,
        raster: Result<Raster, String>,
    ) {
        if self.tabs.current_index() != Some(page) || revision != self.revision {
            // a stale page or content revision can never be displayed
            // again; caching it would only eat the budget
            return;
        }
        match raster {
            Ok(raster) => {
                self.cache.insert(page, revision, bucket, raster);
                self.scheduler.raster_arrived(bucket, self.viewport.scale);
                if let Some((handle, raster_size, natural)) = self.cache.get(page, revision, bucket)
                {
                    self.viewport.sheet_loaded(natural);
                    if bucket == bucket_index(self.viewport.scale) {
                        self.displayed = Some(Displayed {
                            handle,
                            raster: raster_size,
                        });
                    }
                }
                // the load's fit may have moved the scale into another
                // bucket; re-run the scheduling so that bucket rasterizes
                self.zoom_changed();
            }
            Err(_) => {
                // both the SVG and the PNG fallback failed: keep the old
                // display, stop requesting this bucket
                self.scheduler.raster_arrived(bucket, self.viewport.scale);
            }
        }
    }

    fn zoom_changed(&mut self) {
        self.scheduler
            .scale_changed(self.viewport.scale, Instant::now());
        self.display_from_cache();
    }

    /// A cached bucket goes on screen without a worker round trip.
    fn display_from_cache(&mut self) {
        let Some(target) = self.scheduler.target_bucket() else {
            return;
        };
        if let Some(page) = self.tabs.current_index() {
            if let Some((handle, raster, _natural)) = self.cache.get(page, self.revision, target) {
                self.scheduler.mark_rendered(target);
                self.displayed = Some(Displayed { handle, raster });
            }
        }
    }

    fn flush(&mut self) {
        if let Some(bucket) = self.scheduler.poll(Instant::now()) {
            self.request_raster(bucket);
        }
    }

    /// Page load: raster the current bucket right away (upstream reloads
    /// the web view on every revision/page change).
    fn request_current_page(&mut self) {
        let Some(page) = self.tabs.current_index() else {
            return;
        };
        let bucket = bucket_index(self.viewport.scale);
        if let Some((handle, raster, natural)) = self.cache.get(page, self.revision, bucket) {
            self.scheduler.mark_rendered(bucket);
            self.viewport.sheet_loaded(natural);
            self.displayed = Some(Displayed { handle, raster });
            // the load's fit may land in another bucket; arm it if so
            self.zoom_changed();
        } else {
            self.scheduler.load_now(bucket);
            self.flush();
        }
    }

    fn request_raster(&mut self, bucket: i32) {
        let Some(page) = self.tabs.current_index() else {
            return;
        };
        let Some(path) = self.tabs.current().map(|p| p.file.clone()) else {
            return;
        };
        // before the first raster lands the sheet size is the JS default
        let sheet = self.viewport.sheet.unwrap_or((1123.0, 794.0));
        spawn_raster(
            RasterJob {
                page,
                revision: self.revision,
                bucket,
                path,
                raster_scale: raster_scale_for(sheet, bucket),
            },
            self.events.clone(),
        );
    }

    /// Page switch: the old page's raster state does not apply.
    fn page_changed(&mut self) {
        self.scheduler.reset();
        self.displayed = None;
        self.request_current_page();
    }

    /// The pane's subscriptions: the event relay plus a debounce tick that
    /// only exists while a raster decision is pending.
    pub fn subscription(&self) -> Subscription<Message> {
        let relay = Subscription::run_with(self.relay.clone(), relay_stream);
        let tick = if self.scheduler.pending() {
            time::every(DEBOUNCE_TICK).map(|_| Message::DebounceTick)
        } else {
            Subscription::none()
        };
        Subscription::batch([relay, tick])
    }

    pub fn view(&self) -> Element<'_, Message> {
        let content: Element<'_, Message> = if self.tabs.is_empty() {
            // upstream ContentUnavailableView (SchematicPane.swift:43-47):
            // icon + title + secondary description, centered mid-pane —
            // `doc.text.image`; x-office-document is the Pop-theme stand-in
            container(
                Column::new()
                    .push(
                        cosmic::widget::icon::from_name("x-office-document")
                            .size(40)
                            .icon(),
                    )
                    .push(text(fl!("nessun-modello")).size(18))
                    .push(
                        text(fl!("descrivi-un-oggetto-nella-chat-per-iniziare"))
                            .class(dim_text_class()),
                    )
                    .spacing(8)
                    .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into()
        } else {
            self.view_sheet()
        };
        Column::new()
            .push(content)
            .push(self.view_status_bar())
            .into()
    }

    fn view_sheet(&self) -> Element<'_, Message> {
        let area = responsive(move |size: Size| self.sheet_element(size));
        mouse_area(area)
            .on_scroll(|delta| Message::Wheel(delta))
            .on_move(|point| Message::CursorMoved(point))
            .on_press(Message::Pressed)
            .on_release(Message::Released)
            .on_double_click(Message::DoubleClick)
            .interaction(if self.dragging {
                Interaction::Grabbing
            } else {
                Interaction::Grab
            })
            .into()
    }

    /// The displayed raster positioned by the viewport offset: padded when
    /// the sheet fits, cropped when it overflows — the same rectangle the
    /// web view would show, without a scrollable.
    fn sheet_element(&self, size: Size) -> Element<'_, Message> {
        self.report_size(size);
        let (vw, vh) = (size.width as f64, size.height as f64);
        let empty = || -> Element<'_, Message> {
            container(Space::new().width(Length::Fill).height(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };
        let Some(displayed) = &self.displayed else {
            return empty();
        };
        let Some((sheet_w, sheet_h)) = self.viewport.sheet else {
            return empty();
        };
        let scale = self.viewport.scale;
        let (off_x, off_y) = self.viewport.offset;
        let disp_w = sheet_w * scale;
        let disp_h = sheet_h * scale;
        let (rw, rh) = (displayed.raster.0 as f64, displayed.raster.1 as f64);
        // raster pixels per display pixel
        let kx = rw / disp_w;
        let ky = rh / disp_h;
        let crop_x = ((-off_x).max(0.0) * kx).floor() as u32;
        let crop_y = ((-off_y).max(0.0) * ky).floor() as u32;
        let region_w = ((vw - off_x.max(0.0)).min(disp_w - (-off_x).max(0.0))).max(0.0);
        let region_h = ((vh - off_y.max(0.0)).min(disp_h - (-off_y).max(0.0))).max(0.0);
        let avail_rw = displayed.raster.0.saturating_sub(crop_x).max(1);
        let avail_rh = displayed.raster.1.saturating_sub(crop_y).max(1);
        let region_rw = ((region_w * kx).ceil() as u32).clamp(1, avail_rw);
        let region_rh = ((region_h * ky).ceil() as u32).clamp(1, avail_rh);
        let image = Image::new(displayed.handle.clone())
            .crop(Rectangle {
                x: crop_x,
                y: crop_y,
                width: region_rw,
                height: region_rh,
            })
            .width(Length::Fixed(region_w as f32))
            .height(Length::Fixed(region_h as f32))
            .filter_method(FilterMethod::Linear);
        let paper = container(image)
            .width(Length::Fixed(region_w as f32))
            .height(Length::Fixed(region_h as f32));
        container(paper)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Padding {
                top: off_y.max(0.0) as f32,
                left: off_x.max(0.0) as f32,
                right: 0.0,
                bottom: 0.0,
            })
            .into()
    }

    /// The sheet area's size, reported from layout (iced has no resize
    /// event for panes) — deduped through `last_size`.
    fn report_size(&self, size: Size) {
        let mut last = self.last_size.lock().unwrap();
        if *last != size {
            *last = size;
            let _ = self.events.send(PaneEvent::Resized(size));
        }
    }

    fn view_status_bar(&self) -> Element<'_, Message> {
        let has_sheet = self.displayed.is_some();
        let pages = self.tabs.pages.len();
        let zoom = if has_sheet {
            text(format!("{:.0}%", self.viewport.scale * 100.0))
        } else {
            text("")
        };
        let mut controls = Row::new().spacing(8);
        if pages > 1 {
            let index = self.tabs.current_index().unwrap_or(0);
            let title = self
                .tabs
                .current()
                .map(|p| p.title.as_str())
                .unwrap_or_default();
            controls = controls
                .push(button::text("<").on_press_maybe((index > 0).then_some(Message::PrevPage)))
                .push(text(fl!(
                    "lld-lld",
                    page = index + 1,
                    total = pages,
                    title = title
                )))
                .push(
                    button::text(">")
                        .on_press_maybe((index + 1 < pages).then_some(Message::NextPage)),
                )
        }
        controls = controls
            .push(button::text("-").on_press_maybe(has_sheet.then_some(Message::ZoomOut)))
            .push(button::text("+").on_press_maybe(has_sheet.then_some(Message::ZoomIn)))
            .push(button::text(fl!("adatta")).on_press_maybe(has_sheet.then_some(Message::Fit)));
        container(
            Row::new()
                .push(zoom)
                .push(Space::new().width(Length::Fill))
                .push(controls)
                .padding([4.0, 12.0]),
        )
        .width(Length::Fill)
        .into()
    }
}

/// The relay stream: one thread draining the std event channel into the
/// subscription. It ends when the pane closes (senders drop, the worker
/// threads' sends fail, and this recv errors).
fn relay_stream(relay: &Arc<RasterRelay>) -> BoxStream<'static, Message> {
    stream::channel(64, {
        let relay = relay.clone();
        async move |output| {
            let Some(events) = relay.take_events() else {
                std::future::pending::<()>().await;
                return;
            };
            let mut sender = output.clone();
            let _ = thread::Builder::new()
                .name("schematic-events".to_string())
                .spawn(move || {
                    while let Ok(event) = events.recv() {
                        let mut message = match event {
                            PaneEvent::Raster { job, raster } => Message::RasterArrived {
                                page: job.page,
                                revision: job.revision,
                                bucket: job.bucket,
                                raster,
                            },
                            PaneEvent::Resized(size) => Message::Resized(size),
                        };
                        // a full stream channel is transient (the runtime
                        // drains it); only disconnection or the retry
                        // budget ends the relay, never a burst
                        let mut backoff = RELAY_RETRY_MIN;
                        let started = Instant::now();
                        loop {
                            match sender.try_send(message) {
                                Ok(()) => break,
                                Err(err) if err.is_disconnected() => return,
                                Err(err) => {
                                    if started.elapsed() > RELAY_RETRY_BUDGET {
                                        // drop this message, keep serving
                                        break;
                                    }
                                    message = err.into_inner();
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
    use cosmic::iced::futures::FutureExt;
    use std::io::Cursor;

    fn approx(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "approx: {actual} vs {expected}"
        );
    }

    fn fitted(sheet: (f64, f64), view: (f64, f64)) -> Viewport {
        let mut vp = Viewport::new(view);
        vp.sheet_loaded(sheet);
        vp
    }

    #[test]
    fn fit_scales_to_min_ratio_minus_margins() {
        // width-limited: min(852/1123, 652/794) = 852/1123
        let vp = fitted((1123.0, 794.0), (900.0, 700.0));
        approx(vp.scale, 852.0 / 1123.0);
        approx(vp.offset.0, (900.0 - 1123.0 * vp.scale) / 2.0);
        approx(vp.offset.1, (700.0 - 794.0 * vp.scale) / 2.0);
        assert!(vp.fitted);

        // height-limited: min(852/500, 652/400) = 652/400
        let vp = fitted((500.0, 400.0), (900.0, 700.0));
        approx(vp.scale, 652.0 / 400.0);

        // a tiny sheet's fit ratio clamps at ZOOM_MAX (setZoom clamps)
        let mut vp = fitted((10.0, 10.0), (900.0, 700.0));
        vp.sheet_loaded((10.0, 10.0));
        approx(vp.scale, ZOOM_MAX);
    }

    #[test]
    fn zoom_clamps_0p1_to_8() {
        let mut vp = fitted((1000.0, 800.0), (900.0, 700.0));
        for _ in 0..30 {
            vp.zoom_in();
        }
        approx(vp.scale, ZOOM_MAX);
        for _ in 0..60 {
            vp.zoom_out();
        }
        approx(vp.scale, ZOOM_MIN);
        assert!(!vp.fitted);

        // a single huge wheel step is clamped to −0.2 before exp (this
        // sheet is height-limited: fit = 652/800)
        let mut vp = fitted((1000.0, 800.0), (900.0, 700.0));
        vp.wheel(1.0e6, (450.0, 350.0));
        approx(vp.scale, (652.0 / 800.0) * (-0.2f64).exp());
    }

    #[test]
    fn zoom_at_keeps_focal_point() {
        let mut vp = fitted((1000.0, 800.0), (900.0, 700.0));
        let focal = (400.0, 300.0);
        let before = (
            (focal.0 - vp.offset.0) / vp.scale,
            (focal.1 - vp.offset.1) / vp.scale,
        );
        vp.zoom_by(2.0, Some(focal));
        let after = (
            (focal.0 - vp.offset.0) / vp.scale,
            (focal.1 - vp.offset.1) / vp.scale,
        );
        approx(before.0, after.0);
        approx(before.1, after.1);
        assert!(!vp.fitted);
        // the anchor was inside the sheet, so the same sheet point stays
        // under the cursor
        assert!((0.0..=1000.0).contains(&before.0));
        assert!((0.0..=800.0).contains(&before.1));
    }

    #[test]
    fn wheel_step_clamps_to_pm0p2() {
        approx(wheel_factor(100.0), (-0.2f64).exp());
        approx(wheel_factor(-100.0), 0.2f64.exp());
        approx(wheel_factor(8.0), (-(8.0 * WHEEL_GAIN)).exp());
        assert!(wheel_factor(40.0) < 1.0);
        assert!(wheel_factor(-40.0) > 1.0);
    }

    #[test]
    fn zoom_debounces_to_buckets() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut sched = RasterScheduler::default();
        // page-load raster for bucket 1 (zoom in the (1, 2] bucket) on screen
        sched.mark_rendered(1);

        let mut jobs = Vec::new();
        // wheel burst: wiggle inside bucket 1, cross into 2, settle there
        for (ms, scale) in [
            (0u64, 1.1),
            (16, 1.5),
            (33, 1.9),
            (50, 2.1),
            (66, 2.5),
            (83, 2.7),
        ] {
            sched.scale_changed(scale, at(ms));
            if let Some(job) = sched.poll(at(ms)) {
                jobs.push(job);
            }
        }
        // nothing while the burst is extending the deadline
        assert!(jobs.is_empty(), "during burst: {jobs:?}");

        // quiet period: exactly one raster, for the settled bucket
        for ms in [100u64, 150, 186, 300] {
            if let Some(job) = sched.poll(at(ms)) {
                jobs.push(job);
            }
        }
        assert_eq!(jobs, vec![2]);
        sched.raster_arrived(2, 2.5);

        // wiggling inside bucket 2 never re-rasters
        for (ms, scale) in [(400u64, 2.2), (416, 2.9), (433, 2.6)] {
            sched.scale_changed(scale, at(ms));
            if let Some(job) = sched.poll(at(ms + 200)) {
                jobs.push(job);
            }
        }
        assert_eq!(jobs, vec![2]);

        // crossing to bucket 3 rasters once, and only once
        sched.scale_changed(4.1, at(600));
        assert_eq!(sched.poll(at(800)), Some(3));
        assert_eq!(sched.poll(at(900)), None);
        sched.raster_arrived(3, 4.1);
        assert!(!sched.pending());
    }

    #[test]
    fn page_tabs_from_manifest_fixture() {
        let _lang = crate::localize::pin_en();
        let manifest_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden/manifest.json");
        let manifest = Manifest::load(&manifest_path).unwrap();

        let out = Path::new("/tmp/does-not-exist-output");
        let files = vec![
            out.join("schematic.svg"),
            out.join("schematic-2.svg"),
            out.join("schematic-3.svg"),
        ];
        let mut tabs = PageTabs::from_parts(&files, Some(&manifest));
        // manifest titles match on file name; page 3 falls back
        assert_eq!(tabs.titles(), ["Assembly", "Lid", "Page 3"]);
        assert_eq!(tabs.current_index(), Some(0));

        tabs.select(9); // upstream clamps to count - 1
        assert_eq!(tabs.current_index(), Some(2));
        tabs.step(-1);
        assert_eq!(tabs.current_index(), Some(1));
        tabs.step(-1);
        tabs.step(-1); // prev is disabled at 0: index stays
        assert_eq!(tabs.current_index(), Some(0));

        // no manifest: every page gets the numbered fallback
        let anon = PageTabs::from_parts(&files, None);
        assert_eq!(anon.titles(), ["Page 1", "Page 2", "Page 3"]);
        assert!(PageTabs::from_parts(&[], None).current().is_none());
    }

    #[test]
    fn svg_rasterizes_to_ink() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="48">
            <rect x="8" y="8" width="48" height="32" fill="#222222"/>
        </svg>"##;
        let raster = rasterize_svg(svg.as_bytes(), 1.0).unwrap();
        assert_eq!((raster.width, raster.height), (64, 48));
        approx(raster.natural.0, 64.0);
        approx(raster.natural.1, 48.0);
        let ink = raster
            .pixels
            .chunks(4)
            .filter(|p| p[3] > 0 && (p[0] < 240 || p[1] < 240 || p[2] < 240))
            .count();
        assert_eq!(ink, 48 * 32, "the rect must rasterize to ink");

        // raster_scale shrinks the pixmap, natural size is untouched
        let small = rasterize_svg(svg.as_bytes(), 0.5).unwrap();
        assert_eq!((small.width, small.height), (32, 24));
        approx(small.natural.0, 64.0);
    }

    #[test]
    fn png_fallback_when_svg_fails_to_parse() {
        let dir = std::env::temp_dir().join("cad-studio-schematic-test");
        std::fs::create_dir_all(&dir).unwrap();
        let svg_path = dir.join("bad.svg");
        std::fs::write(&svg_path, b"<svg><garbage").unwrap();

        let mut png_bytes = Vec::new();
        let mut img = image::RgbaImage::new(6, 4);
        img.put_pixel(0, 0, image::Rgba([20, 20, 20, 255]));
        img.write_to(&mut Cursor::new(&mut png_bytes), image::ImageFormat::Png)
            .unwrap();
        std::fs::write(dir.join("bad.png"), &png_bytes).unwrap();

        // the unparseable SVG falls back to its png sibling at natural size
        let raster = rasterize_page(&svg_path, 2.0).unwrap();
        assert_eq!((raster.width, raster.height), (6, 4));
        approx(raster.natural.0, 6.0);
        approx(raster.natural.1, 4.0);

        // nothing to fall back to: an error, not a panic
        assert!(rasterize_page(&dir.join("missing.svg"), 1.0).is_err());
    }

    #[test]
    fn pan_clamps_and_refits_on_resize() {
        let mut vp = fitted((1000.0, 800.0), (900.0, 700.0));
        vp.zoom_by(3.0, None);
        vp.pan((1.0e6, 1.0e6));
        // dragging past the edge stops at the 24 px body padding
        approx(vp.offset.0, SHEET_MARGIN);
        approx(vp.offset.1, SHEET_MARGIN);
        vp.pan((-1.0e6, -1.0e6));
        approx(vp.offset.0, 900.0 - 1000.0 * vp.scale - SHEET_MARGIN);
        approx(vp.offset.1, 700.0 - 800.0 * vp.scale - SHEET_MARGIN);

        // a fitted viewport re-fits on resize (viewDidResize)
        let mut vp2 = fitted((1000.0, 800.0), (900.0, 700.0));
        vp2.view_resized((1200.0, 900.0));
        approx(vp2.scale, 852.0 / 800.0); // min(1152/1000, 852/800)
        assert!(vp2.fitted);

        // a zoomed viewport keeps its scale on resize, only re-clamps
        let before = vp.scale;
        vp.view_resized((1200.0, 900.0));
        approx(vp.scale, before);
        assert!(!vp.fitted);
    }

    #[test]
    fn lru_evicts_least_recently_used() {
        let raster = |n: u32| Raster {
            natural: (n as f64, 1.0),
            width: n,
            height: 1,
            pixels: vec![0u8; n as usize * 4],
        };
        let mut cache = RasterCache::new(1000);
        cache.insert(0, 0, 0, raster(100));
        cache.insert(0, 0, 1, raster(100));
        assert!(cache.get(0, 0, 0).is_some()); // refresh (0, 0)
        cache.insert(0, 0, 2, raster(100)); // over budget: evicts (0, 1)
        assert!(cache.get(0, 0, 1).is_none(), "least recently used evicted");
        assert!(cache.get(0, 0, 0).is_some());
        assert!(cache.get(0, 0, 2).is_some());
    }

    // ------------------------------------------------------------------
    // Fix round 1: the relay stream and the pane's message core, driven
    // headlessly (no widgets, no runtime)
    // ------------------------------------------------------------------

    /// Pull the next relay message without ever hanging the test thread:
    /// poll the stream and give up at a wall-clock deadline.
    fn next_message(
        stream: &mut BoxStream<'static, Message>,
        deadline: Instant,
    ) -> Option<Message> {
        while Instant::now() < deadline {
            if let Some(message) = stream.as_mut().next().now_or_never() {
                return message;
            }
            thread::sleep(Duration::from_millis(1));
        }
        None
    }

    /// `update` returns a `Task` the headless core ignores (always none).
    fn drive(pane: &mut SchematicPane, message: Message) {
        let _ = pane.update(message);
    }

    fn test_relay() -> (Sender<PaneEvent>, Arc<RasterRelay>) {
        let (events, receiver) = mpsc::channel();
        let relay = Arc::new(RasterRelay {
            id: 0xDEAD,
            events: Mutex::new(Some(receiver)),
        });
        (events, relay)
    }

    #[test]
    fn relay_maps_resize_and_raster_events() {
        // the round-1 bug this pins: PaneEvent::Resized must surface as
        // Message::Resized (it used to be swallowed by a FromPane arm)
        let (events, relay) = test_relay();
        let mut stream = relay_stream(&relay);
        events
            .send(PaneEvent::Resized(Size::new(320.0, 240.0)))
            .unwrap();
        events
            .send(PaneEvent::Raster {
                job: RasterJob {
                    page: 1,
                    revision: 4,
                    bucket: 2,
                    path: PathBuf::from("/tmp/missing.svg"),
                    raster_scale: 1.0,
                },
                raster: Ok(test_raster()),
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        match next_message(&mut stream, deadline) {
            Some(Message::Resized(size)) => assert_eq!(size, Size::new(320.0, 240.0)),
            other => panic!("resize event lost in the relay: {other:?}"),
        }
        match next_message(&mut stream, deadline) {
            Some(Message::RasterArrived {
                page,
                revision,
                bucket,
                raster,
            }) => {
                assert_eq!((page, revision, bucket), (1, 4, 2));
                assert!(raster.is_ok());
            }
            other => panic!("raster event lost in the relay: {other:?}"),
        }
        drop(events); // disconnected: the relay thread ends on its own
    }

    #[test]
    fn relay_survives_a_full_channel() {
        // a burst larger than the stream channel's 64 slots must not kill
        // the relay for the session (it used to exit on the first failed
        // try_send)
        let (events, relay) = test_relay();
        let mut stream = relay_stream(&relay);
        const BURST: u32 = 200;
        for i in 0..BURST {
            events
                .send(PaneEvent::Resized(Size::new(i as f32, 0.0)))
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = 0;
        let mut last = None;
        while seen < BURST as usize {
            match next_message(&mut stream, deadline) {
                Some(message) => {
                    seen += 1;
                    last = Some(message);
                }
                None => panic!("relay died after {seen}/{BURST} messages"),
            }
        }
        match last {
            Some(Message::Resized(size)) => assert_eq!(size.width, (BURST - 1) as f32),
            other => panic!("wrong mapping: {other:?}"),
        }
        drop(events);
    }

    #[test]
    fn pane_re_arms_raster_when_load_fit_changes_bucket() {
        // the round-1 bug this pins: a raster arrives, sheet_loaded's fit
        // moves the scale into another bucket, and the pane must arm that
        // bucket's raster instead of staying blank
        let mut pane = SchematicPane::new(&[PathBuf::from("/tmp/missing-page.svg")], None);
        // a small viewport makes the post-load fit land far below bucket 0
        drive(&mut pane, Message::Resized(Size::new(200.0, 200.0)));
        assert_eq!(pane.viewport.view, (200.0, 200.0));

        let big = Raster {
            natural: (1000.0, 800.0),
            width: 1000,
            height: 800,
            pixels: vec![255u8; 1000 * 800 * 4],
        };
        drive(
            &mut pane,
            Message::RasterArrived {
                page: 0,
                revision: 0,
                bucket: 0,
                raster: Ok(big),
            },
        );
        // the load fit fired (sheetDidLoad -> fit), width-limited
        approx(pane.viewport.scale, 152.0 / 1000.0);
        // bucket 0 is not the fit scale's bucket: nothing displayed yet,
        // but the raster for bucket -2 must be armed
        assert!(pane.displayed.is_none());
        assert_eq!(pane.scheduler.target_bucket(), Some(-2));
        assert!(pane.scheduler.pending());

        // the debounce expires -> the tick emits the job
        thread::sleep(RASTER_DEBOUNCE + Duration::from_millis(50));
        drive(&mut pane, Message::DebounceTick);
        assert!(!pane.scheduler.pending(), "job emitted for the fit bucket");

        // delivering that raster puts it on screen, fit kept
        let fitted = Raster {
            natural: (1000.0, 800.0),
            width: 250,
            height: 200,
            pixels: vec![255u8; 250 * 200 * 4],
        };
        drive(
            &mut pane,
            Message::RasterArrived {
                page: 0,
                revision: 0,
                bucket: -2,
                raster: Ok(fitted),
            },
        );
        assert!(pane.displayed.is_some());
        approx(pane.viewport.scale, 152.0 / 1000.0);
    }

    #[test]
    fn stale_revision_raster_is_ignored() {
        // the revision contract: T7 bumps set_revision when a page's SVG
        // changes on disk; rasters requested under the old revision must
        // not display
        let mut pane = SchematicPane::new(&[PathBuf::from("/tmp/missing-page.svg")], None);
        pane.set_revision(7);
        assert_eq!(pane.revision, 7);

        // natural-sized so the load fit stays inside bucket 0
        let page_raster = Raster {
            natural: (1000.0, 800.0),
            width: 250,
            height: 200,
            pixels: vec![255u8; 250 * 200 * 4],
        };
        drive(
            &mut pane,
            Message::RasterArrived {
                page: 0,
                revision: 6,
                bucket: 0,
                raster: Ok(page_raster.clone()),
            },
        );
        assert!(pane.displayed.is_none(), "stale revision displayed");
        assert!(pane.viewport.sheet.is_none(), "stale revision fitted");
        assert!(pane.cache.get(0, 6, 0).is_none(), "stale raster not cached");

        drive(
            &mut pane,
            Message::RasterArrived {
                page: 0,
                revision: 7,
                bucket: 0,
                raster: Ok(page_raster),
            },
        );
        assert!(pane.displayed.is_some(), "current revision displays");
    }

    #[test]
    fn cache_revision_keys_do_not_collide() {
        let raster = |n: u32| Raster {
            natural: (n as f64, 1.0),
            width: n,
            height: 1,
            pixels: vec![0u8; n as usize * 4],
        };
        let mut cache = RasterCache::new(10_000);
        cache.insert(0, 3, 1, raster(10));
        assert!(cache.get(0, 4, 1).is_none(), "other revision must miss");
        assert!(cache.get(0, 3, 1).is_some());
    }
}
