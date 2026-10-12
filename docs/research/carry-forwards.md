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

- M4: mesh reload prefers 3MF, falls back to STL with parts `[]` — consumer-side,
  per upstream parseMesh.
- M4: project-folder scaffolding (`output/`, `.cadstudio/logs`, default
  `project.json`) must exist BEFORE `watch_directory` starts (it errors on a
  missing dir) — upstream ProjectStore.prepare/create, deliberately deferred
  from M1.
- M4: watcher wiring passes ~300 ms debounce (upstream 0.3 s).
- M8: `AppleLanguages` setting intentionally omitted in M1 (language override
  lands with i18n).

## M4 (app shell)

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
- The auth classifier's `"401"` needle can substring-match token counts
  or other numbers in the joined details on nonzero exits (usage
  objects, durations) — upstream-inherited (AgentSession.swift:321,
  documented in T8's report); expect a false auth flag to surface here
  first.

## M3 (viewer productization)

- Upload-once: replace per-frame `TriMesh` clone + full vertex re-upload
  (scene.rs `draw()`, mod.rs prepare) with `Arc<TriMesh>` identity or a dirty
  flag. GPU-first policy requires it.
- Derive camera aspect from viewport (16:9 is hardcoded in `default_view_proj`
  spike shorthand).
- Orientation-sensitive blit test (a UV flip in blit.wgsl currently passes all
  MSAA tests — correct today, untested).
- Wire `with_msaa` into the windowed example Storage (enables the 1×/4× visual
  toggle Preston asked about; headless proof already stands).
- Real normals upload replaces the dpdx/dpdy screen-space spike shorthand.
- cull None doubles fragment work (stencil parity needs both faces) — revisit
  if profiling cares.
- Bounded poll timeout in headless harness if CI-style flakes appear.
- image dev-dep: `default-features = false, features = ["png"]` at next touch.
- Dependency law: iced must pin via `git = "pop-os/libcosmic.git"` (the vendored
  fork inside libcosmic's repo), never pop-os/iced.git — a second iced instance
  breaks type identity (Cargo.toml comment records this).

## M6 (viewer tools)

- Winding parity assumes consistently-wound closed meshes; real STL imports
  with flipped facets will produce stray caps/holes — winding robustness needed.

## M8 (parity tail)

- APP_ID placeholder `net.brownhouse.CadStudio` needs Preston's sign-off.
- Fluent: 5 placeholder-only keys sanitized to key/key-2..key-5 need mapping;
  52 values carry % placeholders needing fluent placeable conversion; 2 values
  per file carry literal \n; esc()'s `\"` branch would emit literal backslash
  if the xcstrings is ever re-vendored (fluent has no backslash escapes).
- Install `just` on any machine expected to run recipes (present on thelio).

## M9 / general

- Visual gates honestly unclosed by M0: Preston's eyeball verdict on
  shots/ PNGs still pending; window smoke-run of the scaffold app deferred.
- M4: watcher debounce can split a burst into two emissions under extreme backend starvation (observed once under 40G fio, T1 forensics) — coalescing or idempotent reload needed in the app wiring; the watch test's 300ms negative window deliberately avoids gating on it.
