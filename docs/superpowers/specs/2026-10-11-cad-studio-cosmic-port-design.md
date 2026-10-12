# CAD Studio for COSMIC — Port Design Spec

Date: 2026-10-11 · Status: approved in conversation, pending spec review
Upstream: https://github.com/filippofinke/cad-studio (MIT, SwiftUI, macOS 15+)
Goal: a complete, working CAD Studio for Linux on the COSMIC desktop — a faithful
port, not a redesign.

## 1. Purpose and success criteria

Port filippofinke/cad-studio — "describe a part, get a printable model" — from
macOS/SwiftUI/RealityKit to Linux/COSMIC (libcosmic + Rust). The user chats with
their locally installed Claude Code (or Codex), which writes and runs a parametric
build123d `model.py`; the app streams the agent's progress, renders the resulting
3D model, shows ISO drawings, manages versions, and hands printable files to the
slicer.

Success = the README feature list works on thelio (Pop!_OS, COSMIC session):

1. The upstream README's feature list functions end to end (chat+agent, 3D viewer
   with measure/section/explode/plate, animation playback, parameter sliders,
   drawings, version history incl. compare, attachments, ZIP export,
   printer-aware rules, panel layouts). UI ships English; Italian/German/French
   ship deactivated.
2. Automated gates green on thelio: unit tests, fake-agent integration tests,
   headless-GPU render tests.
3. A real-agent end-to-end run on thelio: prompt → model renders, drawing shows,
   print plate opens, follow-up produces version 2, restore returns to version 1.
4. `just install` puts the app in the COSMIC app grid with the blueprint-cube
   icon; it launches and runs through the user's `claude` login.

Non-goals (v1): Flatpak packaging, Wayland-other-than-COSMIC polish, translation
activation beyond English, keeping agent turns alive across GUI restarts.

## 2. Decisions (all confirmed with Preston)

| Decision | Choice |
|---|---|
| CAD kernel | **build123d**, unchanged — same system prompt, `model.py` contract, manifest, outputs. Not OpenSCAD. |
| Scope | **Full parity, EN-first.** All features; fluent wired, only `en` active. |
| Process architecture | **A: engine-as-library.** One process; engine crate has zero GUI deps and is headless-testable. No daemon, no IPC. |
| Rendering | **GPU-first**: everything per-frame is wgpu on GPU; CPU does only one-off, event-driven work off-thread. |
| Repo | `~/code/cad-studio-cosmic` (Mac); thelio bare remote `~/Code/cad-studio-cosmic.git`, working clone `~/Code/cad-studio-cosmic`. |
| License | MIT; upstream copyright preserved, port copyright added. |

## 3. Product contract — port 1:1

The porting source of truth is `docs/research/upstream-contract.md` (digest of
upstream source; exact CLI flags, JSON schemas, regexes, algorithms). Where this
spec and the digest disagree, the digest's *behavior* wins unless listed under
"Documented divergences" (§9).

Behaviors that must match upstream exactly:

- Claude/Codex invocation flags, `--append-system-prompt` template rendering with
  all placeholders (`{{PYTHON}}`, `{{ENVIRONMENT}}`, `{{PRINTER}}`, …), restricted
  Bash allowlist, isolation flags.
- stream-json event handling incl. partial-message streaming, tool-row lifecycle
  keyed by `tool_use_id`, thinking rows, result handling.
- Session resume/fork: `--resume <id> --fork-session`, 5 s retry on
  "already has an active writer", fresh-start fallback on missing-session strings,
  session id captured from init/result events, auth-error detection list.
- Python env: uv-first venv creation at `~/.local/share/cad-studio/venv`,
  python3.10–3.13 fallback scan, verification line, `MPLBACKEND=Agg` +
  `PYTHONUNBUFFERED=1`, unpinned build123d/matplotlib/numpy.
- Parameter rewrite regex `(?m)^\s*name\s*(?::[^=\n]+)?=\s*(number)` with
  NSRegularExpression-equivalent semantics, integer formatting rule, atomic write,
  direct re-run of `model.py` (no agent).
- Output watching: `output/` only, 300 ms debounce, per-file mod-time dedup, the
  exact file→reload-kind mapping.
- Version snapshots: what is copied, when (recorded after an agent-turn success
  with all expected outputs present, or after a successful manual/slider build —
  in both cases only when model.py differs from the current version's copy;
  upstream gates outputs on the turn path only), restore semantics incl.
  session_id swap.
- On-disk formats unchanged: `project.json`, `chat.json`, `versions/<n>/`,
  `manifest.json`, `animation.json`, LayoutNode JSON. A project folder must be
  portable between the macOS app and the port.
- 3D viewer math: camera formulas and clamps, axis remap, light setup, grid
  spacing/alphas, explode algorithm, plate fallback packing, animation
  interpolation (lerp/slerp), part-grouping suffix strip, tool-summary title
  classification, attachment prompt text, missing-outputs note text.

## 4. Architecture

### 4.1 Workspace and dev workflow

```
cad-studio-cosmic/
├── Cargo.toml            # workspace; [workspace.dependencies] pins libcosmic git rev, wgpu 28
├── engine/               # domain crate — no GUI deps, no libcosmic
├── viewer/               # wgpu renderer + camera/tools math; iced types only in adapter module
├── app/                  # libcosmic UI, binary `cad-studio`
├── fixtures/             # golden outputs (generated by real build123d once) + fake-agent scripts
└── docs/                 # this spec + research reports
```

- Toolchain: rustc ≥ 1.93 (thelio has 1.95), edition 2024, libcosmic pinned by
  git rev (vendored pop-os/iced fork, wgpu 28 — the renderer must use wgpu 28.x
  exactly so device/queue types match).
- libcosmic `wgpu` feature enabled explicitly. tiny-skia fallback keeps the UI
  alive on GL-less machines; the 3D pane is gated off there (never on thelio).
- Dev loop: edit on Mac → `git push thelio` → `ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && cargo test"'`.
  GUI runs from Preston's COSMIC session. Non-interactive ssh shells on thelio
  miss `~/.local/bin`; always `bash -lc`.
- CPU-heavy work (mesh parse/clip/raster) runs on std threads/rayon, never on the
  cosmic executor (single-worker tokio) — only async I/O lives in tasks.

### 4.2 engine crate

Modules mirror the upstream contract; public surface is
`EngineCommand`/`EngineEvent` enums plus pure functions.

- `agent/` — `claude.rs`, `codex.rs` (argv builders + TOML escaping),
  `session.rs` (turn state machine), `stream_event.rs` (serde types; unknown
  event shapes must be ignored, not errors), `tool_summary.rs`, `locator.rs`
  (Linux PATH discovery: `$SHELL -lc` parse + candidates `~/.local/bin`,
  `~/.cargo/bin`, `~/.npm-global/bin`, `/usr/local/bin`, `/usr/bin` + custom
  override; validated with `--version`).
- `python/` — `env.rs` (venv create/verify/progress milestones; the verified
  package-versions line lives in a process-global cell mirroring upstream's
  `PythonEnvironment.shared` — the session prompt's python line reads it, and
  empty/unverified renders the exact fallback "build123d, matplotlib, numpy"),
  `model.rs` (run model.py, stream output, last-line error, interrupt).
- `project/` — store (project.json, chat.json, recents), `versions.rs`
  (snapshot/restore), `manifest.rs`, `outputs.rs` (expected outputs + missing
  list; PDF page count via lopdf), `parameters.rs` (regex rewrite).
- `parsing/` — `stl.rs`, `threemf.rs`, `animation.rs`. 3MF: M0 spike decides
  `threemf` 0.8 vs custom `zip`+`quick-xml` parser; must expose build items,
  components, color groups, per-triangle pid/p1, transforms, unit attribute.
- `watch.rs` — notify 8.2 inotify watcher with upstream debounce/dedup.
- `processes.rs` — tokio children, SIGINT→3 s→SIGTERM, pid registry
  (`~/.local/share/cad-studio/running-processes`), orphan reaping via
  `/proc/<pid>/stat` (ppid, starttime).
- `settings.rs` — all upstream keys/defaults, XDG paths (§6), print-guideline
  strings, units.

### 4.3 viewer crate

- `camera.rs` — pure functions, upstream formulas verbatim (§3).
- `scene.rs` — mesh → GPU buffers (position/normal/color/part-index), per-part
  transform uniforms, materials, neutral color, transparency modes.
- `tools/` — `picking.rs` (Möller–Trumbore, off-thread, click-driven),
  `arrangement.rs` (grouping, explode, plate fallback, bed outline),
  `section.rs` (CPU Sutherland–Hodgman clip — the fallback path for §5's
  GPU discard+stencil section).
- `playback.rs` — MotionPlayer state-at-time, collision flags.
- `renderer.rs` — iced `shader::Program`/`Primitive`/`Pipeline` adapter on wgpu
  28, modeled on iced's `custom_shader` example; implements
  `Primitive::render()` (not `draw()` — iced's pass has no depth/stencil
  attachment, so the viewer runs its own pass with its own depth+stencil
  textures); plus a surfaceless headless path for tests.

### 4.4 app crate (libcosmic)

- One `Application`; multi-window: Welcome (recents with schematic.png
  thumbnails, new-project sheet with upstream name validation) + one window per
  project (`view_window`).
- Project window: `PaneGrid` with chat / 3D / schematic panes; drag-resize and
  rearrange; 4 upstream presets; LayoutNode JSON persisted in project.json;
  upstream min thicknesses.
- Chat pane: inline-only markdown via pulldown-cmark (upstream AttributedString
  parity — no block elements; fenced code renders as monospace paragraphs);
  streaming rows; tool activity rows
  (live partial-JSON preview throttled 0.15 s, final state); thinking rows
  (block-boundary "reasoned for N s" timing + "~N tokens"; the engine emits
  the boundaries — thinking TEXT is never displayed, this CLI's thinking
  content is signature-only);
  system rows with actions (ask-agent-to-fix, open-terminal); attachments via
  xdg-portal file chooser, drag-drop, clipboard image paste (→ PNG temp);
  queued sends (1 s auto-run after success), pause on failure.
- 3D pane: viewer + overlays — parts list (show/hide/link), floating parameter
  sliders, motion controls (play/pause/scrub/speed/loop, phase label, values,
  collisions), measure badge, section controls, explode slider, layout switch,
  view presets; status bar (M4 ships minimal: units + output warnings; full
  parity incl. bbox/volume/12-swatch legend is re-derived from the vendored
  Viewer3D Swift at M6).
- Schematic pane: the §5 raster/cache mechanism (resvg rasterize per
  page/zoom-level on a background thread, cached, image-handle display,
  debounced re-raster); zoom/pan/fit math is the JS-bridge port; page tabs
  from manifest drawings; PNG thumbnail use.
- Settings + first-run setup sheet (printer type/model/units), agent-missing
  sheet, Python-setup sheet; cosmic-config persistence.
- i18n: fluent, `en` source; extractor script converts the 336 xcstrings keys
  to en/it/de/fr ftl at M0; only `en` activated in v1.
- System: xdg-open (slicer/STEP/PDF), reveal via cosmic-files, ZIP export (zip
  crate), SIGTERM child cleanup, About.
- Packaging: cosmic-app-template-derived justfile (build/run/test/install),
  `.desktop`, metainfo, blueprint-cube icon, install to `~/.local`.

## 5. Rendering policy (GPU-first)

- Per-frame work is 100% GPU. CPU produces buffers/edge lists once per mesh
  load; dynamics (camera, explode, animation poses, visibility) are uniform /
  instance data.
- Section/cut plane: WGSL per-pixel `discard` against the plane equation; red
  cut face via stencil cross-section (inc/dec front/back faces in the clip
  region, one unlit red plane quad where stencil ≠ 0). No per-frame CPU
  clipping. Sutherland–Hodgman retained only as fallback. Stencil requires the
  `Primitive::render()` path — iced's compositor pass carries no depth/stencil
  attachment, so the viewer owns a `Depth24PlusStencil8` texture (resized with
  the viewport, stencil cleared each frame); `Primitive::draw()` cannot be used
  (see libcosmic-landscape.md §2a amendments).
- Wireframe: WebGPU has no polygon line-fill; edge index buffers built once per
  mesh (off-thread), drawn as GPU line-lists.
- Animation playback: per-part transform uniforms per frame; no vertex rewrites.
- MSAA: iced's own-pass multisample settings do not apply to a custom
  `render()` pass; the mesh pass owns its multisample count + resolve target
  (owned MSAA texture → resolve → blit, or the frame view as
  `resolve_target`). Decision at M3; M0 spike proves the resolve path.
- SVG schematic: resvg rasterizes per page/zoom-level on a background thread,
  cached; panning is a GPU texture translate; re-raster debounced.
- Measure picking: CPU ray-cast per click (event-driven hit test, off-thread),
  never per-frame.
- Documented CPU path: the tiny-skia/no-Vulkan machine fallback (3D gated off).

## 6. Linux path & platform mapping

| macOS upstream | Port |
|---|---|
| `~/Library/Application Support/CAD Studio/venv` | `~/.local/share/cad-studio/venv` |
| pid registry (same dir) | `~/.local/share/cad-studio/running-processes` |
| UserDefaults (plist) | cosmic-config → `~/.config/cad-studio/` |
| `/bin/zsh -lc` PATH | `$SHELL -lc` + candidate dirs (§4.2 locator) |
| NSWorkspace open/reveal | xdg-open / DBus FileManager.ShowItems |
| NSOpenPanel/SNSavePanel | xdg-desktop-portal FileChooser |
| `/usr/bin/zip` / `/usr/bin/unzip -p` | zip crate (both directions) |
| WKWebView SVG | resvg widget + native zoom/pan |
| PDFKit page count | lopdf |
| Localizable.xcstrings (it source) | fluent ftl (en source) |
| `xattr` notarization workaround | none needed |

## 7. Testing strategy

- **Unit** (any machine): parser goldens from fixtures; parameter-regex corpus
  (typed constants, exponents, integer-vs-float formatting, adversarial lines);
  tool-summary table; camera/arrangement/interpolation property tests; missing-
  outputs logic.
- **Integration, headless on thelio (the automated gate)**: fake-agent script
  emitting scripted stream-json sequences; asserts full turn lifecycle, argv
  (resume/fork/isolation/restricted flags), busy-session retry, missing-session
  fallback, auth-error path, cancellation mid-turn (SIGINT escalation), watcher
  reload under rapid writes, parameter rebuild with a fake python that copies
  fixtures, version snapshot/restore.
- **Fixtures**: generated once by real build123d on thelio (multi-part colored
  model.3mf, plate.3mf, schematic.svg/pdf, manifest.json, animation.json),
  committed; tests never need the venv or network.
- **Render tests**: surfaceless wgpu on thelio GPU; canonical scenes; pixel
  readback compared with tolerances (GPU nondeterminism).
- **Real-agent smoke (gated, manual)**: one "20 mm cube" turn via Preston's
  thelio `claude` login; full M9 end-to-end run per §1.
- Peer/machine etiquette: builds on thelio are sized against `ListAgents`
  activity; announce before heavy phases.

## 8. Licensing and attribution

MIT. LICENSE preserves Filippo Finke's copyright and adds the port's. README
credits upstream prominently ("a port of filippofinke/cad-studio for the COSMIC
desktop"). Upstream assets used: the blueprint-cube app icon and
CADSystemPrompt.md text (both MIT). No Anthropic/Claude branding as the app's
own (upstream's commitment, kept).

## 9. Documented divergences & risks

Divergences (intentional, not parity bugs):

1. Lighting: simple Blinn-Phong tuned to resemble RealityKit's PBR look
   (metallic 0.15 / roughness 0.55); not a full PBR pipeline.
2. SVG rendering via resvg may differ subtly from WebKit (fonts, dashes) —
   validated against fixtures at M0.
3. Pane rearrange interaction feel follows iced PaneGrid, not NSSplitView;
   LayoutNode schema and presets are identical.
4. PDF: page count in-app (missing-outputs check), viewing opens externally;
   in-app page display uses the SVG/PNG sheets (same as upstream's page content).
5. i18n source language inverted it → en; translations carried over by
   extractor; only en active in v1.

Risks and mitigations (from research; top items):

| Risk | Mitigation |
|---|---|
| Renderer is the largest workstream | viewer crate isolated; staged milestones M3→M7; `custom_shader` example as skeleton; headless render tests |
| Agent CLI event drift | tolerant serde (ignore unknown), fixture corpus, fake-agent adversarial lines |
| Session resume/fork subtlety | scripted tests covering every branch |
| Linux PATH/agent discovery | locator + agent-missing setup sheet; verified claude+uv present on thelio |
| inotify event storms | port debounce + mod-dedup exactly; rapid-write stress test |
| Parameter regex corrupts model.py | corpus tests, atomic writes |
| build123d unpinned breaks | parity choice; env line logged; agent-fix loop recovers |
| wgpu 28 pin drift in libcosmic | pin git rev in workspace; bump deliberately |

## 10. Milestones

Each milestone ends with the full test suite green on thelio.

- **M0 Bootstrap** — workspace + template; thelio remotes; toolchain check
  (libfontconfig1-dev etc.); generate fixtures with real build123d; resvg
  vs matplotlib SVG check; 3MF spike (threemf vs custom); xcstrings→ftl
  extractor; shader-widget mesh-render proof on thelio GPU via the
  `Primitive::render()` path — stencil-attachment proof (own
  `Depth24PlusStencil8` pass, stencil inc/dec + red quad) and an MSAA
  resolve-target spike.
- **M1 Engine core** — project store, parsers, manifest, watcher, settings (+tests).
- **M2 Agent lifecycle** — claude runner, session machine, streaming, summaries;
  fake-agent e2e green.
- **M3 Viewer basics** — mesh + orbit/zoom/pan + grid + colors + wireframe;
  headless render tests; MSAA decision (own multisample + resolve target for
  the custom `render()` pass).
- **M4 App shell** — windows, panes, chat, schematic. **Vertical slice: prompt →
  (fake or real) agent → outputs appear → model renders.** Scope binding
  (2026-10-12, spec-drift review): IN = welcome + project windows (multi-window),
  PaneGrid with upstream layout parity + persistence, chat pane complete
  (streaming/tool/system rows incl. thinking-row block boundaries, send queue
  with 1 s auto-run + pause-on-failure, inline-only markdown), schematic pane
  via the §5 raster/cache mechanism, viewer embed + ALL parity input wiring
  incl. pan (M3 delivers formulas only), viewer adapter productization rename
  (Spike\*→Model\*), first-run setup + agent-missing sheets, xdg-portal
  directory chooser for new/open project, MSAA 4× app-side opt-in, project
  scaffolding before `watch_directory` starts, i18n `fl!()` wired from day one
  (en corpus; it/de/fr activation + corpus repair M8), engine `EngineEvent`
  extended with thinking block start/stop boundaries. OUT (with owner):
  parameter sliders + Python env (M5); parts list / measure / section /
  explode overlays + full status-bar parity re-derived from the vendored
  Viewer3D Swift (M6); motion controls (M7); attachments UI, ZIP export,
  packaging (M8). Settings have ONE writer: the engine's
  `~/.config/cad-studio/settings.json` (`settings_path()`); cosmic-config
  holds app-UI state only (recents, first-run flag). The M4 section of
  `docs/research/carry-forwards.md` is normative input to the M4 plan.
- **M5 Python + params + versions** — env setup, sliders rebuild, snapshot/restore.
  Scope binding (2026-10-12, plan-review adjudications): M5 owns the compare
  PICKER, `comparison_parameters`, and the struck-through previous values (they
  live in ParametersOverlay.swift); M6 owns only the orange ghost-mesh render.
  Manual/slider builds and slider commits are M5 (incl. the Compila ⌘B command
  M4 reserved); the version menu/restore/summary rows are M5.
- **M6 Viewer tools** — measure, section (GPU), explode/plate/bed, parts overlay, compare.
- **M7 Animation playback.**
- **M8 Parity tail** — codex runner, attachments (picker/drag/paste), ZIP export,
  settings parity, i18n scaffold, packaging/install.
- **M9 End-to-end** — real-agent run on thelio, polish, README/docs.
