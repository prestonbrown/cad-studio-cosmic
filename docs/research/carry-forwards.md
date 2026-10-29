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
- ~~M8: `AppleLanguages` setting intentionally omitted in M1 (language
  override lands with i18n).~~ Landed M8 T1 2026-10-28: `language` in
  engine settings, read once at startup before any lookup, next-launch
  semantics (upstream parity).

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
- ~~The auth classifier's `"401"` needle: the port appends
  `api error status {status}` to `TurnOutcome::Failed`'s details, so a 401
  status classifies deterministically — strictly better than upstream's
  substring luck. The needle still substring-matches token counts or other
  numbers in the joined details on nonzero exits (usage objects, durations)
  — upstream-inherited (AgentSession.swift:321, documented in T8's report);
  word-boundary it when M4 touches it, with a test.~~ (landed M4 —
  2617c65's `contains_word` boundary with
  `auth_401_word_boundary_engine` in engine/tests/session_turn.rs; the M8
  inventory's "Additional" row had re-found it as open)
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

- ~~Overlay parameter rows must sync on `ParameterCommitted` and
  `BuildFinished` too (the in-code comment at the project-window arms names
  the triggers); restore already wires sync_parameters — the build/slider
  paths need the same call when their full wiring lands (M6 T10 was the
  owner and landed the refresh seam WITHOUT these triggers — the arms still
  `Task::none()` beside their naming comment; still owed).~~ (landed M8 —
  T11: both arms sync from the pipeline's refreshed set; the window-state
  test `parameter_commit_and_build_finish_sync_the_overlay_rows` pins them.
  Open still populates nothing — the events are the overlay's only
  population path besides restore)

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

- ~~Fit frames the tracked framing reference; the `Fit` arm never re-points
  it, so a reload while in a non-assembled layout leaves Fit framing the
  staged assembled box (upstream's fitToView frames the CURRENT arranged
  mesh, ModelViewer.swift:322-337). One-liner if parity is wanted:
  `set_framing_reference(self.frame_aabb)` in the `Fit` arm (one line +
  test update).~~ (landed M8 — T11: the arm re-points the reference. The
  divergence's real shape was explode/hidden changes, which move
  `frame_aabb` through apply_arrangement WITHOUT touching the reference
  (scene swaps re-point it themselves) — the rewritten
  `preset_message_drives_the_scene_pose` tail pins the explode case)
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

- ~~Space focus: upstream's Space shortcut (MotionControls.swift:19) is a
  SwiftUI button shortcut, inert while a text field has focus; iced's
  global listener cannot see focus, so Space typed into the chat draft
  toggles playback too. M8's command system owns the revisit (the pane's
  module doc has the detail).~~ (no divergence — M8 T9's probe of the pin
  found the recorded behavior impossible: `keyboard::listen` delivers only
  widget-IGNORED events, and a focused text editor/input captures bare
  Space as an insert, so the pane's listener never sees editor-typed
  Space. The M7 entry was a design-time inference, never observed live;
  viewer_pane's module doc now states the mechanism and a test pins the
  editor binding)
- Keyboard-driven scrub pauses: upstream pauses only on drag-start
  (`onEditingChanged`, MotionControls.swift:24-30). CORRECTED at the M8 T11
  probe (2026-10-28): the pinned iced slider (dbc62f2) DOES emit on_change
  from arrow keys (widget/src/slider.rs:503-519, while the cursor is
  anywhere over the whole track — the guard is
  cursor.is_over(layout.bounds()), not the handle) and ctrl+wheel
  (:485-502) — so the port's every-`Scrub`
  -pauses arm really pauses keyboard/wheel scrubs that upstream would not
  (the M7 note's "no keyboard scrub at all" premise was wrong at this pin).
  Pause-gating per upstream semantics is not implementable here: the slider
  publishes drag and keyboard changes on ONE on_change channel;
  mouse_area wrappers cannot observe drag-begin (children update first and
  captured events return early — iced/widget/src/mouse_area.rs:265-279);
  `on_release` fires at drag END only; `listen_with` exposes capture status
  but not which widget consumed; and widget messages flush before
  subscription broadcasts in the same batch (iced/winit/src/lib.rs
  :1408-1417 then :1498-1506), so a pointer-down flag arrives after the
  first scrub of every drag — gating on it would drop the pause for
  click-to-seek (which upstream pauses at mouseDown) to fix the rarer
  keyboard path. Accepted divergence; revisit only if the fork grows an
  editing-changed signal.
- The speed control is a dropdown plus an adjacent loop button where
  upstream nests the picker and the Ripeti toggle in ONE menu (:35-49) —
  one menu's worth of popover machinery for a two-item menu
  (motion_controls' module doc).
- Translucent parts composite in slot order (no sort among translucents,
  depth-write off) where SceneKit sorts transparents back-to-front —
  cosmetic; the offscreen playback shots are the eyeball-verdict venue
  (the M0/M6 pattern).
- ~~`ToggleParameters` is the only keybound command without a menu entry —
  upstream surfaces it as a menu button (Commands.swift:69-73). Deferred
  to M8's command system (the T8 review's minor).~~ (landed M8 T8
  2026-10-28: the View root carries the entry — static label per Ruling
  9, divergence recorded in the M8 section)
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

- ~~Persisted chat.json user rows must list the `references/` COPY names,
  not the original request paths (upstream persists the copies,
  Project.swift:199-200). The engine currently drops the copy names after
  `copy_attachments` (session.rs:412) — M8's attachments work adds the
  engine surface (e.g. a Started/TurnEcho field) and switches the bridge's
  user-row text to the copies.~~ Landed M8 T4 2026-10-28 by MOVING the
  copy bridge-side: `Drive::start_turn` runs the engine's pure
  `attachments::copy_attachments` at turn start (before the row appends,
  upstream's order) and threads the names into the row, the
  `TurnStarted` message, the prompt, and the codex `-i` images; the
  engine's `run_attempt` no longer writes (single writer).

M8 T4 attachments divergences (2026-10-28):

- Paste CONTENT reads ride the pin's `clipboard::read_data`
  (window_clipboard → data-control): real on WAYLAND only. The X11 and
  macOS backends answer nothing (platform/linux.rs returns None for X11;
  macos.rs implements text read/write only), so paste-to-attach silently
  degrades to text paste there. COSMIC/Wayland is the target; no
  subprocess fallback (dependency ruling stands).
- Drag-drop: the Wayland-native `DndDestination` wraps the chat pane;
  winit's window-level `FileDropped` covers X11/macOS — but window-level
  means a drop anywhere on a project window stages into the chat draft
  (upstream scopes the drop to the chat view; ChatView.swift:20-31).
  Drops from portal-sandboxed sources offering only
  `application/vnd.portal.filetransfer` match nothing: accepting that
  mime needs libcosmic's `xdg-portal` feature (a dependency ruling, not
  taken here).
- Thumbnail decode supports png+jpeg (the `image` crate features the app
  already pins); webp/gif/bmp/tiff attachments take the file-type tile
  (upstream's NSImage decodes them). The strip's remove button rides
  BESIDE the tile (top-aligned) — iced has no overlay for upstream's
  corner `xmark.circle.fill` (ChatInputView.swift:88-98).
- `ChatState::image_attachments` (the queue's image count) now cites the
  engine's ONE image allowlist; `heic` drops out of the count (the
  engine-side ruling records the allowlist as the formats the attach
  surfaces produce).

M8 T8 parameters menu entry divergence (2026-10-28):

- The View menu's ⌥⌘I parameters toggle carries ONE STATIC label —
  upstream's own tooltip phrasing "Mostra o nascondi i parametri (⌥⌘I)"
  (`mostra-o-nascondi-i-parametri-i`, ProjectWindow.swift:159) — where
  upstream's menu button flips "Mostra parametri"/"Nascondi parametri"
  with `areParametersVisible` (Commands.swift:69-73). Ruling 9's safe
  branch, taken at plan review: the menu-corruption record names label
  churn inside the corruption family and the boundary is UNPINNED, so
  text-only item flips are ruled out until M9 certifies them
  (docs/research/libcosmic-menu-dynamic-items-bug.md; the plan's struck
  Task 2 records why no probe can certify it today). Revisit only if M9
  pins the boundary. The pre-existing mostra/nascondi-chat View-root
  flip (M4 vintage) is the one remaining text-only flip against that
  ruling — untested hazard, same owner (M9 boundary pin or a fixwave
  that statics its label the same way).

- APP_ID placeholder `net.brownhouse.CadStudio` needs Preston's sign-off.
- Fluent: 5 placeholder-only keys sanitized to key/key-2..key-5 need mapping;
  52 values carry % placeholders needing fluent placeable conversion; 2 values
  per file carry literal \n; esc()'s `\"` branch would emit literal backslash
  if the xcstrings is ever re-vendored (fluent has no backslash escapes).
- Install `just` on any machine expected to run recipes (present on thelio).

## M9 / general

- Attachments divergences (M8 T4 review, 2026-10-28): (a) paste reads
  uri-list + PNG only — upstream also pastes TIFF (Attachments.swift:15-18);
  TIFF paste falls through to text. (b) non-image tiles use the generic
  text-x-generic icon where upstream's NSWorkspace.icon(forFile:) is
  per-file-type (cosmetic). (c) saved_image stages undecodable bytes with no
  decode gate (upstream's NSImage(data:) refuses at paste) — a mislabeled
  paste ships to the agent as a reference; gate or accept. Owner: M9 polish /
  T11 fixwave if cheap. Also: the frame pass owes one real COSMIC Files
  drag-drop (T4's own open probe) and the queue-count nuance (upstream labels
  all attachments "N immagini"; port counts images only — pre-existing).


- Spinner sizing (2026-10-28, Preston's play session): the header
  ActivityViewer spinner and the chat thinking-row spinner render at the
  widget default where upstream pins `.controlSize(.mini)`
  (ActivityViewer.swift:44-46; ChatView.swift:170 — a 12pt-font, 24pt-tall
  bar); both spinners are functionally parity (both show), only sized
  large. M8: size to match if the pinned cosmic rev exposes a size handle.
- Busy-state cold-start (2026-10-28, RESOLVED same day): the observed
  silent swallow after the crash was NOT persisted busy state — a cold
  start is idle by construction (now pinned end-to-end by
  bridge::tests::cold_start_over_crashed_store_starts_turns over the
  crashed store's exact shape). It was `Bridge::enqueue`'s
  python-readiness refusal (EnvState::Unknown until the one-shot venv
  check lands; a failed check never retried) arriving with zero
  chat-surface feedback. Fixed: the refusal sets
  `Activity::Failed("Python setup required")` and every enqueue-driving
  site folds the bridge activity. Two follow-ups: the status-line naming
  is an upstream ADDITION (upstream refuses sheet-only,
  Project.swift:192-196 — recorded divergence), and a FAILED venv check
  has no retry (owner: M8 settings/Python tab or wave 2). The
  synchronous busy-rise half (Send arm sets no local activity where
  upstream's send() does, Project.swift:192-208) remains M8 plan Task 0.
- Version-history chrome divergence (2026-10-28, 16ea45e): upstream's
  VersionMenu is a SwiftUI `Menu` dropdown; the port renders a popover
  card (the issue-list pattern) because cosmic's menu overlay corrupts
  widget state when the item set changes between rebuilds — the crash
  Preston hit twice and the driven repro captured ("Downcast widget
  state", iced tree.rs:507 via menu_inner/RcWrapper; fact pack
  docs/research/libcosmic-menu-dynamic-items-bug.md — internal engineering record; pop-os rejects AI-assisted bug reports so nothing is filed upstream, Preston's ruling 2026-10-28). Content is
  faithful (same trigger/labels/two-line rows/checkmark/busy gating);
  the divergences: popover-vs-menu chrome, and relative timestamps
  FROZEN at open (a live per-frame time-ago is rebuild churn). If
  libcosmic fixes the menu bug, restoring the menu root is a small
  revert; until then the menu bar keeps static roots only.

- ~~Smoke-run visual nits (2026-10-15, first live project window)~~
  **FIXED 2026-10-26 (0a9c41a + review minors 5f6c1ed), pending Preston's
  eyeball verdict.** Root causes: (1) the "blank gray boxes" were the chat
  empty state's SUGGESTION BUTTONS — text under a `Button::Transparent`
  renders invisible: cosmic's transparent style resolves its inherited text
  color from TRANSPARENT_COMPONENT (every color rgba(0,0,0,0),
  theme/mod.rs:32-45) and iced Text inherits it (iced/core text.rs:883);
  explicit ambient-color classes now sit on all text under Transparent
  (chat suggestions, welcome recents names) and the python sheet's
  disclosure moved to `Button::Text`. (2) the duplicated "No model" hero:
  both pane heroes now center mid-pane ContentUnavailableView-style with
  per-pane icons (package-x-generic / x-office-document) + fl! strings —
  upstream shows both too, but centered and icon-distinct. (3) the chat
  input card floats (outer inset) instead of clipping the pane's bottom
  edge. OPEN follow-ups from the fix review: the input row's send area
  should be eyeballed for width starvation (the editor's width DEFAULTS
  to Length::Fill — iced text_editor.rs:159 — and if iced's flex pass
  starves later Shrink children the send button collapses; reviewer's
  hypothesis, unverified live); six hand-copies of the two text-color
  rules across four modules (one shared styling module due when a 7th
  copy appears).
- **FIXED (2026-10-15): project window panicked on its 2nd frame —
  `Downcast on stateless state`** (iced tree.rs:505) at TextEditor layout
  inside the chat pane. Root cause (proven by instrumenting
  `Tree::take_all_named`/`Tree::diff` in the vendored iced, pin dbc62f2):
  cosmic's `text_editor` widget in context-menu mode (`has_context_menu`,
  default true on Linux builds) is a two-level widget — wrapper plus inner
  iced `TextEditor` — and the wrapper's `Widget::id()` forwards the inner
  editor's id (libcosmic src/widget/text_editor.rs). So the chat input's
  `Id::new("chat-input")` is reported at TWO adjacent tree levels, and
  iced's NAMED state-preservation map (thread-local, keyed by id NAME
  only) has both levels clobber each other in `take_all_named` on every
  cached rebuild: the second `insert` under "chat-input" drops the
  wrapper's record (which holds the inner editor's subtree). The re-diff
  then restores one level from the surviving record and leaves the other
  a tag-matching stateless shell — `Tree::diff`'s self-id path swaps its
  already-ripped `None` state back in without rebuilding — and the next
  layout downcasts that shell. Frame 1 never panics because a fresh tree
  comes from `Tree::new` (no rip). Fix: `.context_menu(false)` on the
  chat input (chat_view.rs `input_bar`) so the id lives on exactly one
  tree level; casualty is the editor's right-click context menu until
  libcosmic stops forwarding `id()` from the wrapper (still the case on
  upstream master as of 2026-10-15 — worth filing upstream with this
  evidence). Verified: drive repro (`setup Continue` / `recent`) and the
  full new-project flow (`welcome NewProject` / `sheet Name` / `sheet
  Advance` / `parent`) both survive to the timeout kill with many view
  builds and no panic; `cargo test --workspace` green. The
  CAD_STUDIO_TRACE view/update probes were scaffolding for this hunt and
  are gone; CAD_STUDIO_DRIVE stays as the smoke harness. NOTE for future
  smokes: `parent <dir>` must name an EXISTING directory (the portal
  contract) — `create_project` uses `create_dir`, not `create_dir_all`.
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

- Parameters overlay populates NOTHING at window open (upstream fills it
  from ProjectOutput.reload at open — M8 T11's finding); the landed syncs
  (ParameterCommitted/BuildFinished) are its only population paths today.
  Owner: M9 or the next fixwave pass.
- Multi-window keyboard scope (M8 T9 review, 2026-10-29): the pane's
  keyboard listener rides `keyboard::listen()`, which drops the window tag —
  with TWO project windows open, an editor-unfocused Space/Esc reaches BOTH
  panes' subscriptions (playback toggles in the background window) where
  SwiftUI's shortcuts fire in the key window only. Reachable (open_project
  dedups by root; two roots = two windows); the fix is window-tagged event
  routing (the pane has no window-focus knowledge today) — a later-milestone
  design, recorded here until then. The single-window case is unaffected
  (T9 verified the editor-capture mechanism end-to-end).

- Settings-window follow-ups (M8 T6 review, 2026-10-29, all LOW): (a)
  ConfigureClaude parity is one half short — upstream auto-presents the
  agent-missing sheet PER TURN on a missing executable (AgentSession.swift:
  41-47) and appends the .configureClaude row; the port auto-presents at
  startup only, and a mid-session bad custom path (reachable via the new
  Agente tab) yields a plain error row with no action — the engine marker
  (T3's flagged wiring) and the per-turn auto-present file together; (b) the
  empty-transcript placeholder engine name seeds at window build, stale
  across a mid-session flip until the next turn (upstream reads at render);
  (c) an open settings window doesn't re-seed when the agent sheet writes
  agent_path underneath it (stale display, no lost write). Cosmetic notes:
  mirror-fold on a failed persist save; Python tab interpreter not
  tilde-abbreviated; About unreachable from welcome-only (recorded).
