# Carry-forwards — obligations from M0 for later milestones

Durable record of deferred findings/triage from the M0 whole-branch review (2026-10-11).
The M1+ plans must absorb their sections; strike lines as landed.

## M1 (engine core)

- ~~3MF parser rewrite from the spike: copy `apply()`/token conventions from
  `engine/tests/parse3mf.rs` at bf77ad6 (spec order: 12 tokens, 4 rows × 3,
  row-vector, translation at tokens 9/10/11) — NOT from the M0 plan doc, whose
  code listing is historical and pre-correction.~~ (landed M1)
- ~~Parser must tolerate a missing `<components>` wrapper (handmade fixture omits
  it; golden has it) and keep spec token order throughout.~~ (landed M1)
- ~~Do not cargo-cult the spike's dead code: `parse_objects`' unused
  `depth_in_object`/`components` collection, and the under-asserting golden
  name test (assert names or drop "with_names").~~ (landed M1)
- ~~Drop the `threemf` dev-dep from engine once the custom parser lands.~~ (landed M1)
- Golden facts for tests: 3MF has 4 objects (named meshes id 1 "base" / id 4
  "lid", build items point directly at them, 2 inert component wrappers);
  basematerials #1F78B5FF/#D62629FF; never assert manifest↔3MF color equality
  (1/255 drift); plate.3mf base spans z −4..4 (do not assume parts rest at z=0).
- ~~Upstream-contract.md:102's "3×4, row-major rows → matrix columns" phrasing is
  the ambiguous Swift-derived line that caused the original convention bug —
  the 3MF spec layout above is authoritative for the port.~~ (landed M1 —
  consumed by Task 4's spec-order work)

From M1's reviews, for later milestones:

- ~~M4: mesh reload prefers 3MF, falls back to STL with parts `[]` — consumer-side,
  per upstream parseMesh.~~ (landed M4 — `viewer_pane::load_model_parts`, empty-parts
  fallback closed at T8 with the handmade `empty.3mf` fixture)
- ~~M4: project-folder scaffolding (`output/`, `.cadstudio/logs`, default
  `project.json`) must exist BEFORE `watch_directory` starts (it errors on a
  missing dir) — upstream ProjectStore.prepare/create, deliberately deferred
  from M1.~~ (landed M4 — T0's `create_project`/`prepare`, the T7 window and
  both pane watchers open over it)
- ~~M4: watcher wiring passes ~300 ms debounce (upstream 0.3 s).~~ (landed M4 —
  `WATCH_DEBOUNCE` in `viewer_pane` and the project window)
- M8: `AppleLanguages` setting intentionally omitted in M1 (language override
  lands with i18n).

## M4 (app shell)

From the M4 T5 3D-pane review cycle (2026-10-13):

- Pinch/magnify zoom DELIVERY is toolkit-blocked: the pinned iced fork
  (dbc62f2) drops winit's `PinchGesture` in its event conversion
  (iced/winit/src/conversion.rs). The app's pure pinch layer is complete,
  clamped, and tested; `viewer_pane`'s message wiring needs exactly one
  match arm when a future iced/winit rev surfaces the event. Re-check the
  fork's conversion on the next libcosmic pin bump.

From the M3 whole-branch review (2026-10-12):

- ~~Pinch/magnify zoom (trackpad gesture) is in the viewer facts' M3 input
  list (#116) but never got an implementation, owner, or carry-forward:
  M4 Task 5's `camera_input` OWNS it alongside the parity input wiring
  (zoom clamp [0.1r, 200r] applies).~~ (landed M4 — T5's pure layer, clamped
  and tested; only the toolkit-blocked DELIVERY note above remains)
- ~~The "re-frame on size change > 50 % diagonal" behavior slid from M3 T2
  to M4 (the M4 plan's Task 5 owns it explicitly) — noted here so the
  deferral is visible where deferrals live, matching its siblings.~~
  (landed M4 — T5)

From M2's agent-lifecycle work (task 9 wrap):

- EngineEvent is the chat-row source: tool rows are keyed by the
  `tool_use` id they carry; `TextStreaming` is the unconfirmed row —
  reconcile a `TextConfirmed` by replacing the FIRST unconfirmed row,
  FIFO (upstream `unconfirmedTextIDs.removeFirst()`).
- `ToolInput` throttling is app-side, never by not reading: upstream
  throttles the row's accumulating preview at 0.15 s; the engine
  forwards every delta.
- The app drains EngineEvents for the whole dance (every attempt): the
  256-deep event channel backpressures a stalled reader — and cancel is
  only polled between event translations, so a stalled drain stalls the
  cancel too (T7/T8 forensics).
- An auth-classified `TurnOutcome::Failed { auth: true, .. }` is the one
  failure whose chat row carries the `openTerminal` action hint
  (upstream's auth system message instructs running `claude` / `codex
  login` in a terminal; upstream-contract.md:41).
- The auth classifier's `"401"` needle: the port appends
  `api error status {status}` to `TurnOutcome::Failed`'s details, so a 401
  status classifies deterministically — strictly better than upstream's
  substring luck. The needle still substring-matches token counts or other
  numbers in the joined details on nonzero exits (usage objects, durations)
  — upstream-inherited (AgentSession.swift:321, documented in T8's report);
  word-boundary it when M4 touches it, with a test.
- A `user` line with MULTIPLE tool_result blocks decodes only the FIRST
  (probe-shaped; upstream reads every block). A second would leave a tool
  row open — M4 fails still-open rows at TurnFinished per upstream's
  failRunningTools; if it ever observes that, extend the decode.
- Thinking blocks without an open streaming row must be dropped app-side:
  the engine only emits `TextStreaming`/`TextConfirmed`, never a thinking
  row (thinking surfaces through `system/thinking_tokens`).
- An assistant content block this build cannot map degrades to
  `StreamEvent::Unknown("assistant")` — skipped like any unknown, so a
  newer agent's block type never breaks the stream decode.

## M5 (python environment)

- ~~`session.rs`'s `PYTHON_UNVERIFIED` display text must adopt upstream's
  exact fallback: `"build123d, matplotlib, numpy"` when the verified
  package list is empty (AgentSession.swift:206,
  `PythonEnvironment.shared.versions.isEmpty ? "build123d, matplotlib, numpy"`)
  — not the current "python interpreter not verified yet" wording.~~
  (landed M5 — T4: session.rs builds the line through
  `python::env::versions_display()`, whose fallback `VERSIONS_FALLBACK`
  pins the upstream text; engine/tests/python_env.rs gates it)
- ~~Intentional classifier divergences to PRESERVE when porting
  ModelBuilder-side summaries (all supersets of upstream, each with a
  port-side test):~~ (landed M5 — T4: all three preserved with tests in
  engine/tests/tool_summary.rs)
  - ~~`live_preview` also previews MultiEdit: it scans `content` OR
    `new_string`, where upstream's `livePreview` keys off the tool name
    (Write → `content`, Edit → `new_string`, MultiEdit → none).~~
  - ~~`read_title` matches `references/` as a path SEGMENT
    (`starts_with("references/") || contains("/references/")`); upstream
    is prefix-only (`hasPrefix("references/")`).~~
  - ~~`short_command` with an empty root strips ANY leading `cd … &&`
    (upstream requires the cd target to be the project root), so a
    foreign-cd command classifies one class up from upstream.~~
- ~~`EngineCommand::Shutdown`'s `reap_orphans` empties the registry
  unconditionally: a just-cancelled child of THIS process is not signalled
  (only ppid-1 entries are) but its listing is cleared, so if the app dies
  before the child does, a later launch's reap no longer knows the pid —
  consider excluding still-live children from the clearing pass when M5
  touches process teardown.~~ (landed M5 — T4: `reap_orphans_at` keeps
  still-live non-orphaned entries listed; engine/tests/processes.rs's
  `reap_keeps_live_children_listed` gates it)

## M3 (viewer productization)

Source of record: `.superpowers/sdd/m3-viewer-facts.md` — it moves to
`docs/research/` at M3 close (after whole-branch review), not before.

- ~~Upload-once: replace per-frame `TriMesh` clone + full vertex re-upload
  (scene.rs `draw()`, mod.rs prepare) with `Arc<TriMesh>` identity or a dirty
  flag. GPU-first policy requires it.~~ (landed M3)
- ~~Derive camera aspect from viewport (16:9 is hardcoded in `default_view_proj`
  spike shorthand).~~ (landed M3)
- ~~Orientation-sensitive blit test (a UV flip in blit.wgsl currently passes all
  MSAA tests — correct today, untested).~~ (landed M3)
- ~~Wire `with_msaa` into the windowed example Storage (enables the 1×/4× visual
  toggle Preston asked about; headless proof already stands).~~ (landed M3)
- ~~Real normals upload replaces the dpdx/dpdy screen-space spike shorthand.~~
  (landed M3)
- cull None doubles fragment work (stencil parity needs both faces) — revisit
  if profiling cares.
- Bounded poll timeout in headless harness if CI-style flakes appear.
- ~~image dev-dep: `default-features = false, features = ["png"]` at next
  touch.~~ (landed M3)
- Dependency law: iced must pin via `git = "pop-os/libcosmic.git"` (the vendored
  fork inside libcosmic's repo), never pop-os/iced.git — a second iced instance
  breaks type identity (Cargo.toml comment records this).

## M6 (viewer tools)

From the M4 whole-branch review's minors (2026-10-13, ownerless until now):

- ~~`PresentSetupSheet` self-loop: the project window's update returns the
  variant to itself — route it to the app layer (or confirm the
  pass-through is the intended shell seam) when M6's app wave touches the
  window message flow.~~ (landed M7 — T8 verified app.rs's intercept stands
  ahead of the fall-through update, so the window arm was dead; folded into
  the `CloseRequested | App(_)` "the app layer acts" arm)
- ~~Global ⌥⌘I: the parameters toggle is viewer-pane-local; upstream binds it
  app-wide (Commands.swift:69-73). Move the match arm to the app layer when
  the toolbar/command surface is next touched.~~ (landed M7 — T8's
  `(true, true, false, "i")` arm in the window's `project_shortcut` routes
  `Command::ToggleParameters` to the pane; the pane-local listener removed
  in the same edit)

From the M5 T7 review (2026-10-13):

- Overlay parameter rows must sync on `ParameterCommitted` and
  `BuildFinished` too (the in-code comment at the project-window arms names
  the triggers); restore already wires sync_parameters — the build/slider
  paths need the same call when their full wiring lands (M6 T10 was the
  owner and landed the refresh seam WITHOUT these triggers — the arms still
  `Task::none()` beside their naming comment; still owed).

- ~~Winding parity assumes consistently-wound closed meshes; real STL imports
  with flipped facets will produce stray caps/holes — winding robustness
  needed.~~ (landed M6 — the section half is pinned: the discard tests the
  fragment's world position alone, never winding
  (`section_discard_is_winding_independent`); the stray-caps half is
  upstream-inherited — upstream shows the same stray caps on
  flipped-winding imports, so the port matches and nothing further is owed)
- ~~No test pins the tripod corner's z placement (from M3's overlay work: the
  grid tests assert vertex counts and the line/bed blocks, not where the
  corner sits).~~ (landed M6 — T7's rewritten test pins it: the z extent
  asserts half-the-MODEL-y-span + margin against scene size, and the y pin
  lands with the 0.05 literal written out)
- ~~msaa 4× composited over the alpha-blended grid is untested through the
  resolve path — the 4× tests render the mesh alone, so a grid-alpha
  regression there fails visually only (from M3).~~ (landed M6 — T7's
  `msaa_resolves_keep_grid_alpha` pins both regression directions: vanished
  lines fail the grid-ink count, lost alpha fails the no-opaque-white
  assert)

From the M6 T10 review (2026-10-13):

- Fit frames the tracked framing reference; the `Fit` arm never re-points
  it, so a reload while in a non-assembled layout leaves Fit framing the
  staged assembled box (upstream's fitToView frames the CURRENT arranged
  mesh, ModelViewer.swift:322-337). One-liner if parity is wanted:
  `set_framing_reference(self.frame_aabb)` in the `Fit` arm (one line +
  test update).
- The frozen-grid stamp keys on scene Arc identity, so plate-view
  hidden/isolate toggles MOVE the grid: the re-staged filtered plate is a
  fresh Arc, the stamp moves, and the grid recaptures from the shrunken
  plate box (upstream keys on mesh+layout+plateID, never the hidden set —
  ModelViewerState.swift:88-91); observable only on a real multi-part
  plate. `grid_bed` is fed LIVE rather than frozen with the stamp —
  upstream-faithful (ModelPane.swift:96 re-sends the bed every update), a
  deliberate split from the plan's literal "recomputed only on stamp
  change".
- The > 50 % reload gate frames the STAGED assembled box, not the arranged
  one — pre-existing (the staged-box feed to `mesh_changed` is T7's rule;
  T10 changed only the tracked reference, an improvement for the next Fit).

## M7 (animation playback)

From the M7 wrap (2026-10-13) — the accepted divergences and deferred
minors, each with its owner:

- Space focus: upstream's Space shortcut (MotionControls.swift:19) is a
  SwiftUI button shortcut, inert while a text field has focus; iced's
  global listener cannot see focus, so Space typed into the chat draft
  toggles playback too. M8's command system owns the revisit (the pane's
  module doc has the detail).
- Keyboard-driven scrub pauses: upstream pauses only on drag-start
  (`onEditingChanged`, MotionControls.swift:24-30), which iced's slider
  does not surface — every `Scrub` message pauses, drag or keys.
- The speed control is a dropdown plus an adjacent loop button where
  upstream nests the picker and the Ripeti toggle in ONE menu (:35-49) —
  one menu's worth of popover machinery for a two-item menu
  (motion_controls' module doc).
- Translucent parts composite in slot order (no sort among translucents,
  depth-write off) where SceneKit sorts transparents back-to-front —
  cosmetic; the offscreen playback shots are the eyeball-verdict venue
  (the M0/M6 pattern).
- `ToggleParameters` is the only keybound command without a menu entry —
  upstream surfaces it as a menu button (Commands.swift:69-73). Deferred
  to M8's command system (the T8 review's minor).
- The no-model whole-bar disable is absent: upstream disables the ENTIRE
  controls row when `output.mesh == nil` (ModelPane.swift:151); the port's
  bar keeps preset/toggles clickable with no model. M6 T9 provenance
  (pre-existing at M7's base); fixwave-shaped — an `on_press_maybe` sweep
  across the whole bar, including the port-specific MSAA toggle upstream
  cannot answer.
- Exact-tie rounding: `format_significant`'s headroom path formats
  ties-to-even (Rust `{:.N}`) where Swift/ICU rounds half-up — e.g.
  `0.125(1,2)` → "0.12" vs "0.13". Unpinned by the brief and contrived
  for real study values; a note for future formatter hardening (the T5
  review's minor).

## M8 (parity tail)

From the M4 T3 bridge review (2026-10-13):

- Persisted chat.json user rows must list the `references/` COPY names,
  not the original request paths (upstream persists the copies,
  Project.swift:199-200). The engine currently drops the copy names after
  `copy_attachments` (session.rs:412) — M8's attachments work adds the
  engine surface (e.g. a Started/TurnEcho field) and switches the bridge's
  user-row text to the copies.

- APP_ID placeholder `net.brownhouse.CadStudio` needs Preston's sign-off.
- Fluent: 5 placeholder-only keys sanitized to key/key-2..key-5 need mapping;
  52 values carry % placeholders needing fluent placeable conversion; 2 values
  per file carry literal \n; esc()'s `\"` branch would emit literal backslash
  if the xcstrings is ever re-vendored (fluent has no backslash escapes).
- Install `just` on any machine expected to run recipes (present on thelio).

## M9 / general

- Visual gates honestly unclosed by M0: Preston's eyeball verdict on
  shots/ PNGs still pending; ~~window smoke-run of the scaffold app
  deferred.~~ (First window smoke-run 2026-10-15 — first-run sheet and
  window chrome findings fixed on branch `ui-smoke`; see below.)
- libcosmic (pin dbc62f2) modal popovers capture mouse events
  WINDOW-WIDE (popover.rs Overlay::update swallows every Mouse/Touch
  event outside the popup card, header bar included): a modal popover
  kills drag/close/maximize on the CSD header. Sheets that must keep the
  chrome alive replace the window body instead (first-run setup,
  new-project — app.rs `view_window`); modal popovers stay only over the
  project canvas (python/agent sheets) where blocking is the intent.
- Custom `decorate` must wire header_bar `on_drag`/`on_double_click`/
  `on_right_click` itself — libcosmic's shell does it for its own bars
  (src/app/mod.rs:782-784); the port's bar was inert until 2026-10-15.
- Welcome window height diverges from upstream's 460 (WelcomeWindow.swift)
  to 560: macOS sheets may exceed their window; popovers/content cannot,
  and 460 clipped the setup card. Documented at WINDOW_HEIGHT.
- Desktop entry + icon (`net.brownhouse.CadStudio`) installed USER-LOCAL
  on thelio only (~/.local/share/{applications,icons}) for the dock;
  needs a real packaging story (and the APP_ID sign-off below) in M8.
- ~~M4: watcher debounce can split a burst into two emissions under extreme backend starvation (observed once under 40G fio, T1 forensics) — coalescing or idempotent reload needed in the app wiring; the watch test's 300ms negative window deliberately avoids gating on it.~~ (landed M4 — T5's idempotent re-parse-and-replace reload, split-burst test in `viewer_pane`)
