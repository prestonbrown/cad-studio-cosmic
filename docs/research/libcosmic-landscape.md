# libcosmic landscape for a CAD-port app (research, Oct 2026)

Goal: Rust COSMIC desktop app (Pop!_OS, libcosmic) embedding an interactive 3D mesh
viewport (STL/3MF, orbit/zoom/pan, per-part colors, cut planes) + chat UI + SVG/PDF
display + background process streaming. All claims cite primary sources.

## 1. libcosmic basics (2026)

- Version: libcosmic master is **1.0.0**, edition 2024, **rust-version 1.93**.
  https://github.com/pop-os/libcosmic/blob/master/Cargo.toml (workspace `[package]`).
  Repo active: last push 2026-10-08 (https://github.com/pop-os/libcosmic).
- **Not on crates.io** — the crates.io API returns "crate `libcosmic` does not exist"
  (verified 2026-10-11 via https://crates.io/api/v1/crates/libcosmic). All real apps
  use a git dependency: cosmic-edit and cosmic-files both pin
  `git = "https://github.com/pop-os/libcosmic.git"` with explicit feature lists
  (https://github.com/pop-os/cosmic-edit/blob/master/Cargo.toml,
  https://github.com/pop-os/cosmic-files/blob/master/Cargo.toml).
- iced base: master **vendors iced as a git submodule** at `./iced` pointing at the
  pop-os fork (libcosmic tree, `iced` entry `type: submodule` → repo pop-os/iced,
  https://github.com/pop-os/libcosmic). The fork (https://github.com/pop-os/iced,
  parent iced-rs/iced) is at **workspace version 0.14.0** — the same line as upstream
  iced 0.14.0 released on crates.io 2025-12-07 (https://crates.io/api/v1/crates/iced;
  upstream master is 0.15.0-dev). Key pins inside the fork's workspace Cargo.toml
  (https://github.com/pop-os/iced/blob/master/Cargo.toml): **wgpu 28.0**, cosmic-text
  0.19, resvg 0.45, image 0.25, tiny-skia 0.11, lyon 1.0, tokio 1.x.
- Default features of libcosmic: `winit, tokio, a11y, dbus-config, x11, wayland,
  multi-window`; an explicit `wgpu = ["iced/wgpu", "iced_wgpu"]` feature exists and
  the `wayland` feature activates `iced_wgpu/wayland`, so default builds include the
  wgpu renderer alongside the always-on tiny-skia fallback
  (https://github.com/pop-os/libcosmic/blob/master/Cargo.toml `[features]`).
- App structure: implement `cosmic::Application` — `core: cosmic::app::Core`,
  `type Executor`, `type Flags`, `type Message`, `init(core, flags)`, `update(msg)`,
  `view()` / `view_window(id: window::Id)`, `subscription()`. Run via
  `cosmic::app::run::<App>(settings, flags)`. Canonical shape:
  https://github.com/pop-os/libcosmic/blob/master/examples/multi-window/src/window.rs
  and the template app https://github.com/pop-os/cosmic-app-template/blob/master/src/app.rs
- Command/Task: it is **`cosmic::Task`** now (alias of `iced::Task`,
  https://github.com/pop-os/libcosmic/blob/master/src/lib.rs `pub use iced::Task`).
  Helpers at https://github.com/pop-os/libcosmic/blob/master/src/task.rs:
  `cosmic::task::{batch, future, message, stream, none}`.
- Async runtime: with the default `tokio` feature,
  `cosmic::executor::Default = cosmic::executor::single::Executor`, a tokio
  multi-thread runtime with **one worker thread and `enable_all()`** (io/time/fs/net
  drivers on) — https://github.com/pop-os/libcosmic/blob/master/src/executor/single.rs,
  https://github.com/pop-os/libcosmic/blob/master/src/executor/mod.rs. So
  `tokio::process`, `tokio::fs`, timers all work out of the box; use
  `std::thread`/rayon for CPU-heavy mesh work, not the executor.
- Background process + streaming stdout (canonical pattern): spawn with
  `tokio::process::Command` (stdout piped), read lines with
  `tokio::io::BufReader::lines()`, map each line to a Message inside an async
  `futures::Stream`, and hand it to the app with `cosmic::task::stream(stream)` (or
  return `Task::stream` from `update`). `cosmic::task::stream` is defined at
  https://github.com/pop-os/libcosmic/blob/master/src/task.rs. Kill via the retained
  `tokio::process::Child::kill().await`. Note: `cosmic::process::spawn`
  (https://github.com/pop-os/libcosmic/blob/master/src/process.rs) is for
  double-fork-detached launching with null stdio — not for streaming; do not use it
  here.
- Docs: generated API docs https://pop-os.github.io/libcosmic/cosmic/ and the official
  book https://pop-os.github.io/libcosmic-book/ (linked from
  https://github.com/pop-os/libcosmic/blob/master/README.md).

## 2. 3D rendering inside a cosmic/iced app — the core question

### (a) Custom wgpu draw inside an iced/cosmic widget — YES, first-class

- The iced 0.14 lineage libcosmic tracks has a trait-based custom-primitive API in
  `iced_wgpu`: implement `Primitive` (`prepare(device, queue, …)`, `draw(render_pass)`
  or `render(encoder, target, clip_bounds)`) with an associated `Pipeline` that owns
  your wgpu pipeline state, buffers, textures — created once per type via
  `Pipeline::new(device, queue, format)`. Source:
  https://github.com/pop-os/iced/blob/master/wgpu/src/primitive.rs
  (docs mirror: https://docs.rs/iced/0.14.0/iced/widget/shader/trait.Primitive.html).
- The public widget is `iced::widget::shader`: a `shader::Program<Message>` whose
  `draw()` returns your `Primitive`; render into the app's own GPU surface with your
  own passes (including depth). Sources:
  https://github.com/pop-os/iced/blob/master/widget/src/shader.rs,
  https://github.com/pop-os/iced/blob/master/widget/src/shader/program.rs
- The bundled example `custom_shader` is a real 3D scene: N rotating cubes, dynamic
  light color, **depth buffer with a "show depth buffer" toggle**, animated at 60fps
  via a time subscription. https://github.com/pop-os/iced/tree/master/examples/custom_shader
  (`scene.rs` implements `shader::Program` + `shader::Primitive` + `shader::Pipeline`).
- Orbit/zoom/pan input: `mouse_area` widget provides on_press/release, right/middle
  press/release, on_scroll, on_move, on_drag —
  https://github.com/pop-os/iced/blob/master/widget/src/mouse_area.rs — wrap the
  shader widget with it and mutate camera state in `update`; redraw on demand with a
  `cosmic::subscription` or `window::redraw`.
- Constraints: your crate must use **wgpu 28.x** (the fork's pin,
  https://github.com/pop-os/iced/blob/master/Cargo.toml) so the `wgpu::Device`/`Queue`
  types in `Primitive::prepare` match; enable libcosmic's `wgpu` feature explicitly.
  The shader widget only renders under the wgpu backend — tiny-skia (the software
  fallback) has no custom-primitive support
  (https://github.com/pop-os/iced/blob/master/tiny_skia/src/primitive.rs — no shader
  plumbing; widget docs: "renders custom shaders with Iced's wgpu backend").
- Mesh loading: binary STL is trivial to parse (e.g. crate `stl` 0.2.1,
  https://crates.io/api/v1/crates/stl, or ~50 LOC hand-rolled); 3MF via `threemf`
  0.8.0 (active 2026-03, 45k downloads) from https://github.com/hannobraun/3mf-rs.
  Per-part colors → vertex/instance attributes; cut planes → fragment-shader discard
  against plane equation (WGSL has no gl_ClipDistance; discard is the standard route).
- Maturity: this is iced's `advanced` API (enabled by libcosmic), stable across
  0.13→0.14 and unchanged in spirit upstream. Known 3D-in-iced apps in the wild: none
  prominent found (GitHub search for Rust STL viewers surfaces wgpu/winit projects,
  not iced ones) — you write the renderer; `custom_shader` is the reference.
- Perf: full GPU pipeline you control — 100k–1M triangles is trivial for a modern
  GPU; mesh upload happens in `prepare` on your schedule.
- **Source-verified amendments (2026-10-11, pop-os/iced fork master — the tree
  libcosmic vendors):**
  1. iced's compositor main render pass creates **no depth/stencil attachment**
     (`depth_stencil_attachment: None` at every `begin_render_pass` site,
     https://github.com/pop-os/iced/blob/master/wgpu/src/lib.rs) — a pipeline
     with stencil state bound to that pass fails validation. Custom primitives
     must not rely on iced's pass for depth/stencil.
  2. Therefore implement **`Primitive::render()`**, not `draw()`. The
     `Primitive::draw()` default returns false
     (https://github.com/pop-os/iced/blob/master/wgpu/src/primitive.rs); iced
     then calls `Primitive::render(pipeline, encoder, target_view, clip_bounds)`
     **outside its own pass** — the primitive runs its own render pass with its
     own depth (+ stencil) texture. The bundled `custom_shader` example does
     exactly this: own `Depth32Float` texture, own pass with `LoadOp::Load`
     color, depth cleared per frame, texture resized on viewport change
     (https://github.com/pop-os/iced/blob/master/examples/custom_shader/src/scene/pipeline.rs).
  3. `Depth24PlusStencil8` is in wgpu's guaranteed render-attachment format
     set, so the stencil cross-section technique (inc/dec front/back faces,
     unlit red quad where stencil ≠ 0) is safe on thelio: a
     depth24plus-stencil8 texture owned by the viewer pipeline, resized with
     the viewport, stencil cleared each frame.
  4. Ordering: within an iced layer, custom primitives are stage 3 of 5 (quads
     1, triangles 2, PRIMITIVES 3, images 4, text 5)
     (https://github.com/pop-os/iced/blob/master/wgpu/src/layer.rs — `start()`
     / `end()` staging). Later stages and layers composite on top, so iced
     overlay widgets above the 3D pane keep working. Note: render()-path
     primitives execute after iced's pass ends, drawing over earlier stages of
     that layer — fine for the 3D pane.
  5. **MSAA gap**: iced's own-pass MSAA settings do **not** apply to a custom
     `render()` pass. The mesh pass needs its own multisample count + resolve
     target (render to an owned MSAA texture, resolve into an offscreen then
     blit; or attach the frame view as `resolve_target`) — without it, CAD
     silhouette edges alias/shimmer. Decision scheduled at M3; spike at M0.

### (b) Software rasterization to a canvas/image widget — viable fallback only

- `cosmic::widget::Canvas` (re-export of iced canvas,
  https://github.com/pop-os/libcosmic/blob/master/src/widget/mod.rs line `pub use
  iced::widget::{Canvas, canvas}`) is a lyon path-tessellation API — per-frame CPU
  tessellation of 100k triangles is not what it is for.
- Better CPU route: rasterize RGBA8 on a background thread and display via
  `image::Handle::from_rgba(width, height, pixels)` (exists in the fork:
  https://github.com/pop-os/iced/blob/master/core/src/image.rs `pub fn from_rgba`).
  ~100k triangles at ~1080p is achievable at 30–60fps with a multithreaded
  depth-buffered rasterizer, but it burns CPU/battery, re-uploads a full-frame
  texture per frame, and you implement AA yourself. Use only for GL-less targets.

### (c) Separate top-level window with raw winit+wgpu — unnecessary; use iced multi-window

- libcosmic ships **multi-window as a default feature** (`multi-window` in default
  features, https://github.com/pop-os/libcosmic/blob/master/Cargo.toml), with the
  official example: `HashMap<window::Id, Window>` state, `view_window(id)`,
  `window::open(window::Settings)` returning `(id, Task)`, `window::close(id)` —
  https://github.com/pop-os/libcosmic/blob/master/examples/multi-window/src/window.rs.
  A 3D viewport window can be just another `view_window` containing the shader widget.
- A *separate raw winit+wgpu loop* in the same process is not viable: iced owns the
  event loop. The supported inverse (embed iced into your wgpu app) exists —
  https://github.com/pop-os/iced/tree/master/examples/integration — but forfeits the
  cosmic Application framework, theming, and app infrastructure. Not needed here.

### (d) Embedding an existing 3D engine crate — not viable today

- **three-d 0.19** renders via **OpenGL/glow 0.17** with its own glutin/winit window
  (https://github.com/asny/three-d/blob/main/Cargo.toml) — it cannot share iced's
  wgpu 28 device/surface, so it cannot live inside an iced widget.
- **bevy_iced** (Bevy + iced glue) last updated 2024-07 and pinned to old iced
  (https://crates.io/api/v1/crates/bevy_iced) — experimental/stale; Bevy also wants
  to own the loop. Not production path.
- Conclusion: skip engine embedding. A focused wgpu-28 mesh renderer behind
  `shader::Program` is less code than fighting an engine's ownership model, and it
  composes with cosmic theming/panes.

### RECOMMENDATION

Build **one libcosmic Application**; make the mesh viewport an iced `shader` widget:
implement `shader::Program` + custom `Primitive`/`Pipeline` on **wgpu 28** (matching
libcosmic's pinned iced fork), with orbit/zoom/pan from `mouse_area` messages,
per-part colors as vertex attributes, cut planes as fragment discard. Enable the
`wgpu` libcosmic feature explicitly; accept tiny-skia as a degraded-CPU fallback that
simply won't draw 3D (or gate the viewport). This is GPU-accelerated, in-window
(composes with pane_grid/chat UI), and is the officially supported custom-draw path
of the iced line libcosmic tracks. Software rasterization (b) is the fallback only
for non-GPU environments; raw windows (c) and engine embedding (d) are dead ends.

## 3. SVG + PDF display

- SVG: `cosmic::widget::svg` is a default export (libcosmic enables iced's `svg`
  feature; the fork renders SVG via **resvg 0.45**,
  https://github.com/pop-os/libcosmic/blob/master/src/widget/mod.rs and
  https://github.com/pop-os/iced/blob/master/Cargo.toml). Vector, scales cleanly;
  for heavy zooming of dense drawings, rasterize with standalone **resvg 0.48.1**
  (https://crates.io/api/v1/crates/resvg) at target DPI into an RGBA image and pan
  with a scrollable + `image::Handle::from_rgba`.
- PDF: three options —
  1. Do nothing: the app's Python side already emits SVG+PNG+PDF; display the PNGs
     (zero new deps) and open PDFs via xdg-open. Recommended first.
  2. **pdfium-render 0.9.4** (https://crates.io/api/v1/crates/pdfium-render, updated
     2026-09): page→RGBA rendering; needs the pdfium binary shipped, or its `static`
     feature + libstdc++ static link (features list in its Cargo.toml,
     https://github.com/ajrcarey/pdfium-render).
  3. **poppler-rs 0.26.0** (https://crates.io/api/v1/crates/poppler-rs): GObject
     bindings; needs system libpoppler-glib at build/run.

## 4. Supporting crates

- File watching: **notify 8.2.0** (2026-08-30; 9.0.0-rc.5 is release-candidate —
  avoid for now), inotify backend on Linux
  (https://crates.io/api/v1/crates/notify, https://docs.rs/notify/8.2.0/notify/).
  Watch the Python output dir; debounce; reload images/mesh.
- JSON-lines streaming: **serde_json 1.0.152** (2026-10-10,
  https://crates.io/api/v1/crates/serde_json); per-line `from_str` inside the
  `cosmic::task::stream` loop is simplest; `StreamDeserializer` for byte-slice
  sources (https://docs.rs/serde_json/1.0.152/serde_json/struct.StreamDeserializer.html).
- zip: **zip 9.0.1** (2026-10-10, https://crates.io/api/v1/crates/zip) — 3MF is a
  zip+XML container (threemf already wraps this), also good for project export.
- XDG paths: **directories 6.0.0** (https://crates.io/api/v1/crates/directories).
- Open-in-slicer: **open 5.4.4** (2026-09-10, https://crates.io/api/v1/crates/open;
  wraps xdg-open etc.), or plain `std::process::Command::new("xdg-open")`.
- i18n: built into libcosmic — `cosmic::localize` uses rust-embed + i18n-embed-fluent
  and exposes the `cosmic::fl!` macro + `cosmic::localize::localize()` at startup
  (https://github.com/pop-os/libcosmic/blob/master/src/localize.rs); apps ship
  `i18n/<lang>/*.ftl` + `i18n.toml` — see the template
  (https://github.com/pop-os/cosmic-app-template/tree/master/i18n) and
  cosmic-edit/cosmic-files for the identical setup.

## 5. Pane layout & multi-window

- Resizable panels: `cosmic::widget::PaneGrid` (re-export of iced pane_grid,
  https://github.com/pop-os/libcosmic/blob/master/src/widget/mod.rs; example with
  drag-resize/split: https://github.com/pop-os/iced/tree/master/examples/pane_grid).
  Libcosmic also has its own non-resizable `cosmic::widget::grid`.
- Persisting layout/state: **cosmic-config** (crate in the libcosmic workspace,
  https://github.com/pop-os/libcosmic/tree/master/cosmic-config) — the canonical
  pattern is the template's config module
  (https://github.com/pop-os/cosmic-app-template/blob/master/src/config.rs); store
  pane sizes/window size as config keys.
- Multiple top-level windows (welcome + project): supported pattern is the official
  multi-window example — `HashMap<window::Id, ...>` per-window state, one
  `Application`, `window::open(window::Settings{ decorations: false, .. })` plus a
  cosmic `header_bar` per window:
  https://github.com/pop-os/libcosmic/blob/master/examples/multi-window/src/window.rs.
  `cosmic::app::Core` tracks `main_window_id()`/`focused_window()`.

## 6. Packaging & dev loop

- Start from **cosmic-app-template**:
  `cargo generate gh:pop-os/cosmic-app-template`
  (https://github.com/pop-os/cosmic-app-template). It ships `justfile`
  (`just run`, `just build-release`, vendored tarball builds), `build.rs`,
  `resources/app.desktop` + `app.metainfo.xml` (appstream) + hicolor/scalable icon,
  and the fluent i18n scaffold — everything a COSMIC app is expected to install.
- Dev loop on the metal: plain `cargo build`/`cargo run` (or `just run`); no Flatpak
  needed for development. Distribution is via Flathub, or pop-os/cosmic-flatpak for
  COSMIC-specific components (per
  https://github.com/pop-os/libcosmic/blob/master/README.md and
  https://github.com/pop-os/cosmic-app-template/blob/master/README.md).
- Toolchain on Pop!_OS 24.04: use rustup (need **Rust ≥ 1.93**, edition 2024 — apt's
  rustc on 24.04 is far older) + `build-essential`; `pkg-config` +
  `libfontconfig1-dev` are the only notable -dev packages (cosmic-text 0.19 defaults
  to its `fontconfig` feature → fontdb/fontconfig C link:
  https://github.com/pop-os/cosmic-text/blob/master/Cargo.toml). Runtime: Vulkan
  loader + Mesa drivers for the wgpu renderer (preinstalled on Pop!_OS), plus
  libxkbcommon/wayland/X11 client libs — iced/iced_wgpu are pure Rust and dlopen
  these; nothing exotic to compile. If wgpu surface init fails, tiny-skia keeps the
  UI alive (3D viewport excepted).
- Pin libcosmic by git rev for reproducibility (cosmic-edit pattern,
  https://github.com/pop-os/cosmic-edit/blob/master/Cargo.toml), and use a
  `[patch.'https://github.com/pop-os/libcosmic']` path override for local toolkit
  experiments (same file shows the commented pattern).

---
Verification: all GitHub URLs above were fetched 2026-10-11 from pop-os/libcosmic
master (v1.0.0) and pop-os/iced master (v0.14.0 workspace, submodule of libcosmic);
crates.io versions read live from the crates.io API the same day.
