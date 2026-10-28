# M8 Parity Tail Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the parity tail between the COSMIC port and upstream cad-studio: alternate engine (codex), completed attachments flow, ZIP export, settings window, i18n activation, packaging — plus the recorded small-parity items and the two new findings from Preston's hands-on session.

**Architecture:** App-side features build on the M4-M7 shells (bridge, panes, sheets, settings law). The engine gains a second agent runner (translation layer onto the existing StreamEvent pipeline) and cedes attachment copying to the bridge (upstream's ordering). i18n regenerates all four catalogs from the vendored xcstrings with real placeable syntax and sweeps every remaining literal. Packaging is freedesktop-native (`just install` to `~/.local`). Menu-shape decisions are evidence-gated by the dynamic-menu repro example.

**Tech Stack:** Rust workspace (engine/viewer/app), libcosmic pin dbc62f2, Fluent catalogs via the existing `fl!` scaffold, the drive harness + Xvfb :99 rig for UI verification.

**Spec:** `docs/superpowers/specs/2026-10-11-cad-studio-cosmic-port-design.md` (§M8 scope; amended through M7). Upstream behavior source: `fixtures/upstream-source/` (Swift). Item-level citations with live line numbers: `.superpowers/sdd/m8-inventory.md` (read-only reference for every task; sections 1-17).

## Global Constraints

- Tests run on thelio, never mercury (battery law). Implementers: `cargo check -p <crate>`, `cargo build -p app`, and focused pure tests locally; full suites only via the controller's thelio gate.
- `cargo build -p app` is the verification bar for app changes (codegen-only failures).
- **Visual consistency is part of the bar**: every UI task cites its upstream fixture (`file:line`), records divergences in `docs/research/carry-forwards.md` BEFORE merge, and frame-verifies on the :99 rig when the difference is visual. The milestone close runs the visual match pass (Task 16).
- Static menu roots only (cosmic menu overlay corrupts on dynamic item sets — `docs/research/libcosmic-menu-dynamic-items-bug.md`); dynamic dropdowns are popovers. Text-only item flips are gated on Task 2's probe.
- No new workspace dependencies without a plan-level ruling (the ZIP task rules: subprocess, not a crate).
- ONE settings writer per key (engine `settings.json` for engine-visible keys; cosmic-config for app-UI state).
- Stage explicit paths; conventional commits; no heredoc-backtick text; pkill bracket discipline.

## Review Focus

1. **A send that arrives while the bridge task is between enqueue and TurnStarted** — the busy window's double-send belt must hold (bridge busy check), and the UI must rise busy synchronously (Task 0). A second Return in that window must never double-send nor silently vanish.
2. **Engine-flip mid-conversation** (Task 3): flipping claude↔codex between turns must resume correctly or start cleanly per upstream's re-read-per-turn — no crossed argv, no leaked translator state.
3. **Attachment paths through the pipeline** (Tasks 4-5): a pasted image, a dragged file, and a picked file must all land as `references/<stamp>-<n>-<name>` copies, render as thumbnails in the draft strip AND the sent row (two different roots), and never double-copy across queue deferrals.
4. **The i18n regeneration must not lose messages** (Task 6): every `substitute_placeholders` call site and every swept literal needs its catalog twin present in ALL FOUR files; arg counts must match between format and call.
5. **ZIP export with hostile names** (Task 7): spaces/quotes in project names and output filenames through `/usr/bin/zip` argv (no shell), destination that already exists, empty output dir — every path surfaces an activity row, never a panic.

## Rulings (the inventory's 48 open questions, decided)

1. **Engine flip mid-project** (inv §1): port upstream's re-read-per-turn verbatim — the code wins over the footer text; record as an upstream quirk note in the settings task's docs.
2. **Codex images** ride `-i` flags; Claude gets none (upstream argv parity; the TurnRequest grows an images split the bridge fills from image-type attachments only).
3. **Locator** grows the second instance + per-turn setting read (mirroring AgentLocator.swift:64-138); `find_agent` keeps one entry point with a kind.
4. **Reference copies move to the bridge** at enqueue (upstream's order — Project.swift:192-208): the bridge calls the engine's pure `copy_attachments` before appending the user row, so rows persist COPY names and prompts see the same list. Engine's run_attempt stops copying (single writer).
5. **ZIP** = `/usr/bin/zip` subprocess, no shell, NOT-FOUND surfaces the failed activity string (dependency law; upstream parity). Save panel = xdg `SaveFile` portal (suggested `"<name>.zip"`, Downloads default).
6. **Settings window** = a new cosmic window (the sheets precedent is body-replacement for modal gates; settings is non-modal — new window), four tabs per upstream, `ConfigureClaude` opens the Agente tab. About = simple dialog with the credits key.
7. **i18n**: regenerate from xcstrings with the upgraded extractor (placeables `{$arg}`, five real key names — `motion-value-row`, `version-timestamps`, `activity-accessibility-label`, `speed-option`, `settings-none` — landing in all four catalogs at once; literal `\n` stays; the `\"` branch fixed). Engine system rows emit stable ids; the app localizes on receipt (upstream parity route). Language setting: engine `settings.json` `language`, next-launch semantics (parity).
8. **APP_ID**: `net.brownhouse.CadStudio` stands unless Preston rules otherwise BEFORE Task 14 (the plan's one blocking external input; flagged in the task).
9. **ToggleParameters entry** (Task 8): flip-label IF Task 2's text-flip probe is clean, else static label + recorded divergence.
10. **Whole-bar disable** gates on `scene.parts.is_empty()` (upstream disables on error too); MSAA included (port-specific, consistent).
11. **Space routing** moves to the app-layer key router, dropped while any text editor holds focus (the ⌥⌘I precedent, viewer_pane.rs:812-814).
12. **Speed menu**: single menu IF Task 2's static-set + moving-checkmark probe is clean (the five speeds + Ripeti are a fixed set); else the recorded divergence stands and the item closes as re-ruled (Task 10 applies the ruling).
13. **Spinner sizing**: probe the pinned rev's circular-progress size handle; if none, the divergence records and the item closes (cosmetic-only).
14. **Busy-state**: Task 0 fixes BOTH the synchronous rise AND the root cause the in-flight investigation lands (fold point below); reopen-mid-turn stays parity (upstream persists no activity) unless the investigation proves a swallow bug.
15. **Translucent sort**: Preston's verdict on the delivered playback shots decides — visible wrong order ⇒ the per-frame far→near translucent sort task runs in Wave 2; not visible ⇒ strike.
16. **Tie rounding**: port the half-up tie-break helper with a test (tiny; closes the item rather than carrying it).
17. **Keyboard scrub**: verify no non-drag `Scrub` emitter exists; then record-and-strike (theoretical divergence).
18. **Additionals**: absorb overlay-sync triggers, the Fit framing one-liner, the auth-needle word boundary, manifest warnings in ActivityViewer, the README `just` note. Leave the plate grid-stamp and staged-box framing items carried (real-multi-part-plate observability) and the M4 pinch watch (pin-bump class).

## Pending inputs (folded before review dispatch)

- ~~busy-state root cause~~ **FOLDED 2026-10-28** into Task 0: python-readiness refusal, silently surfaced (see Task 0's root-cause note).
- ~~upstream gallery capture~~ **FOLDED 2026-10-28**: 11 states captured to `shots/upstream/` (setup-sheet, welcome, welcome-recents, new-project, project-model, project-empty, version-menu, parameters, parts, section, measure) — `.superpowers/sdd/upstream-gallery-report.md` documents the build + per-state capture. Task 16 compares against this set.

---

### Task 0: Busy-state correctness (synchronous rise + refusal surfacing)

**Files:**
- Modify: `app/src/windows/project.rs` (Send arm, `set_activity`), `app/src/bridge.rs` (already: the python-readiness refusal sets `Activity::Failed("Python setup required")` and every enqueue-driving site folds the bridge activity — implemented 2026-10-28, `.superpowers/sdd/busy-state-report.md`; this task lands the REMAINING half).
- Test: the touched files' existing test modules.

**Interfaces:**
- Consumes: `Bridge::activity()` (bridge.rs), `Bridge::enqueue` refusal paths (now surfaced).
- Produces: a Send arm that rises busy synchronously (the one unlanded half).

**Root cause on record (2026-10-28):** NOT persisted busy state — a cold start is idle by construction (pinned by `cold_start_over_crashed_store_starts_turns`); the observed swallow was `Bridge::enqueue`'s python-readiness refusal arriving with zero chat-surface feedback (EnvState::Unknown until the one-shot venv check lands; a send during that window vanished). Surfacing is landed with the refusal fix; two follow-ups recorded in carry-forwards: the status-line naming is an upstream ADDITION (upstream refuses sheet-only — Project.swift:192-196), and a FAILED venv check has no retry (the Python tab or a wave-2 item owns that).

- [ ] **Step 1:** Failing test: a send press flips `is_busy()` true immediately — no bridge round-trip window where the stop button/streaming indicator lag (upstream Project.swift:192-208 sets `.working` synchronously in `send()`).
- [ ] **Step 2:** Implement: the Send arm's synchronous `set_activity` (matching the refusal fold's shape), keeping the bridge's busy-check belt as the double-send guard.
- [ ] **Step 3:** GREEN locally; thelio gate via controller (this task absorbs landing the reviewed refusal-surfacing diff if not already merged).

### Task 1: i18n corpus regeneration + fl! sweep

**Files:**
- Modify: `scripts/extract_xcstrings.py` (placeables, five key names, `\"` fix), `app/i18n/{en,it,de,fr}/main.ftl` (regenerated), every literal-bearing module (`app/src/{chat,chat_view,activity,version_menu,parameters_view,localize}.rs`, `app/src/windows/project.rs` menu labels), `engine/src/agent/session.rs` (system-row ids), `app/src/bridge.rs` (id → localized row).
- Test: `app/src/localize.rs` tests + per-module string tests.

**Interfaces:**
- Produces: `fl!("key", args…)` with named args; engine `SystemRowId` (or equivalent) enum; `substitute_placeholders` DELETED (three call sites migrate: activity.rs:101, viewer_pane.rs:1148, viewer_pane.rs:1950); `language` field in `engine/src/settings.rs` read in `main.rs` before `localize()`.

- [ ] **Step 1:** Upgrade the extractor: `%@`/`%lld`/`%d`/`%.Nf` → `{$name}` with positional arg names (`{$arg}`, `{$arg-2}`, … or semantic names where the sweep knows them); name the five format-only keys (`motion-value-row`, `version-timestamps`, `activity-accessibility-label`, `speed-option`, `settings-none`); fix the `\"` branch (fluent has no backslash escape); preserve literal `\n`.
- [ ] **Step 2:** Regenerate all four catalogs; diff-audit: no message lost, arg counts match (a test iterates every key in all four files asserting equal `{…}` placeholder sets).
- [ ] **Step 3:** Sweep the literals → `fl!` (the inventory §6 lists every module with counts; engine system rows become ids localized at the bridge).
- [ ] **Step 4:** Language setting: `language: Option<String>` in engine settings, `select()` at startup, next-launch semantics; the Settings task's language picker writes it.
- [ ] **Step 5:** Tests: placeholder-set parity across catalogs; every swept module's string tests updated; RED→GREEN evidence per the house pattern.

### Task 2: Menu-shape probes (the evidence gate)

**Files:**
- Modify: `app/examples/dynamic_menu.rs` (two variants).
- Test: none (probe; outcome recorded in carry-forwards).

**Interfaces:**
- Produces: two rulings — text-flip safety (gates Task 12) and static-set+moving-checkmark safety (gates Task 13's menu shape).

- [ ] **Step 1:** Add `flip` variant: static item COUNT, one item's label alternating each rebuild.
- [ ] **Step 2:** Add `check` variant: static five items, a checkmark glyph moving between rows each rebuild.
- [ ] **Step 3:** Run both on thelio :99 (the established rig), 60s each, RUST_BACKTRACE=1; record panic-or-clean in carry-forwards with the ruling each gates.

### Task 3: Codex runner (engine + settings + sheets)

**Files:**
- Modify: `engine/src/agent/locator.rs` (second instance, per-turn read), `engine/src/agent/codex.rs` (NEW: argv + translator), `engine/src/agent/session.rs` (dispatch + per-line translation), `engine/src/settings.rs` (readers for agent_engine/codex_path/codex_model), `app/src/sheets.rs` (per-engine install commands), `app/src/activity.rs` (engine display name by setting).
- Test: `engine/tests/codex_runner.rs` (NEW, fake-agent harness pattern), unit tests in codex.rs.

**Interfaces:**
- Consumes: the existing `StreamEvent` pipeline (session.rs process()); `AgentKind` grows `Codex`.
- Produces: `codex::build_args(&Settings, &TurnRequest, …)` (argv per CodexRunner.swift:19-40 incl. `tomlString` escaping and `-i` per image attachment); `CodexTranslator::events(line) -> Vec<StreamEvent>` mapping per CodexRunner.swift:63-171 (thread.started/item.started/item.completed/file_change/agent_message/turn.completed mapping, `/bin/zsh -lc` unwrapping).

- [ ] **Step 1:** Unit tests for `tomlString` (backslash, quote, \n, \r, \t, C0 as \uXXXX) and the translator's event mapping from recorded fake lines — RED.
- [ ] **Step 2:** Implement codex.rs (argv + translator) — GREEN.
- [ ] **Step 3:** Locator + settings readers + session dispatch (re-read per turn, engine-blind resume sid — Ruling 1; engine display name).
- [ ] **Step 4:** Fake-agent e2e on thelio: a scripted codex-JSON emitter drives a full turn through the existing harness (thread started → tool rows → file_change pairs → completed).
- [ ] **Step 5:** Sheets' install commands + auth naming ("codex login" already needled).

### Task 4: Attachments completion (paste, drag-drop, thumbnails, reference-copy move)

**Files:**
- Modify: `app/src/app.rs` (⌘V routing when a chat editor holds focus; DroppedFile handling per the probe), `app/src/chat_view.rs` (thumbnail widget — 52px draft strip, 64px rows, file-type icon + extension caption fallback per AttachmentThumbnail), `app/src/bridge.rs` (copy at enqueue via the engine's pure fn), `engine/src/agent/{session,claude}.rs` (run_attempt stops copying; build_prompt consumes the passed list), `app/src/chat.rs` (attach dedupe).
- Test: bridge tests (copy-at-enqueue, row names), chat_view build tests, engine session tests.

**Interfaces:**
- Consumes: `engine::agent::claude::copy_attachments` (moves to a path-neutral home if ruled by the implementer — keep it engine-owned, bridge-called).
- Produces: persisted rows carrying `references/<stamp>-<n>-<name>`; a `references_from` surface the prompt reuses; image-type attachments split for codex `-i` (Task 3 consumes).

- [ ] **Step 1:** Bridge copy-at-enqueue tests (row carries copy names; queue deferral copies at START not enqueue — Ruling 4/inv §3) — RED.
- [ ] **Step 2:** Move the copy call; rows + prompt consistent; engine run_attempt single-writer cleanup — GREEN.
- [ ] **Step 3:** Thumbnail widget + strip/rows wiring (draft strip roots at original paths; sent rows root at projectRoot — one widget, root parameter).
- [ ] **Step 4:** Paste: app-level ⌘V routing (⌥⌘I precedent) while the chat editor holds focus; clipboard image → temp file (`std::env::temp_dir().join("CAD Studio Attachments")`, upstream's naming shape) → attach; file URLs → attach; empty clipboard falls through.
- [ ] **Step 5:** Drag-drop: probe the pinned rev's DroppedFile surface (window-level at minimum); wire what exists, record the rest as divergence if the pin cannot surface it.
- [ ] **Step 6:** Visual: fixture citations (ChatInputView.swift:62-104, 156-184; MessageView.swift:10-23) + :99 frames of the draft strip and a sent row.

### Task 5: ZIP export

**Files:**
- Modify: `app/src/windows/project.rs` (Command::Export, menu + toolbar + ⇧⌘E, hasThreeMF gating — menu ignores busy, toolbar doesn't: copy as-is), `app/src/app.rs` (SaveFile portal + spawn + reveal).
- Test: a pure `export_files(root) -> Vec<PathBuf>` (model.py + non-dot output, existence-filtered) with tests; the spawn is thin.

**Interfaces:**
- Consumes: the existing reveal (ShowItems) and activity surfaces; the catalog keys `pacchetto-esportato` / `esportazione-non-riuscita` (Task 1 ensures they're placeable-clean).
- Produces: `Command::Export`; `/usr/bin/zip -j -q <dest> <files…>` argv (no shell), NOT-FOUND → the failed activity row.

- [ ] **Step 1:** `export_files` tests (dotfile exclusion, missing model.py, empty output) — RED → GREEN.
- [ ] **Step 2:** Portal SaveFile (suggested `"<name>.zip"`, Downloads), spawn, status → activity + reveal; menu/toolbar/keybind wiring per Commands.swift:130-132 and ProjectWindow.swift:135-140.
- [ ] **Step 3:** :99 frame of the flow (the portal is bypassable via the drive harness if needed — extend `drive` with `saveas <path>` if the portal blocks scripted runs).

### Task 6: Settings window + About

**Files:**
- Modify: `app/src/app.rs` (window open/close, ⌘, binding), `app/src/windows/settings.rs` (NEW: four tabs), `app/src/windows/project.rs` (ConfigureClaude → Agente tab), `engine/src/settings.rs` (any missing readers).
- Test: settings window state tests (per-tab field round-trips), locator-verify flow tests.

**Interfaces:**
- Consumes: Task 3's engine settings readers; the python sheet's env states (the Python tab reuses them).
- Produces: `windows::settings` with Generale (location, language — writes Task 1's field, next-launch footer; PrintPreferencesSection reuse), Agente (engine picker + per-engine sections + effort + restricted-bash claude-only + codex sandbox note), Python (interpreter/verify/reinstall), Avanzate (streaming picker, isolate toggle + apiKeyHelper footer).

- [ ] **Step 1:** Tab-state round-trip tests — RED.
- [ ] **Step 2:** The window (new-window pattern per Ruling 6), fields bound through the ONE-writer law (engine settings.json; cosmic-config only for window state).
- [ ] **Step 3:** About: the credits dialog (the literal-\n key survives Task 1); version from `env!("CARGO_PKG_VERSION")`.
- [ ] **Step 4:** ConfigureClaude row action + agent-sheet configure path open the Agente tab.
- [ ] **Step 5:** :99 frames of each tab vs the gallery's upstream captures (SettingsView.swift:3-242 citations in-module).

### Task 7: Whole-bar no-model disable

**Files:** `app/src/viewer_pane.rs` (`view_status_bar` + `preset_picker` + `msaa_toggle`).
- [ ] **Step 1:** Test: with `scene.parts.is_empty()`, every pressable in the bar reports no `on_press` (drive-message level where possible; the on_press_maybe sweep at minimum gets a construction test per toggle).
- [ ] **Step 2:** The sweep: preset buttons, grid/wireframe (newly gated), measure/section (compose with `!animating`), animation slot per upstream's own gating, MSAA — all `on_press_maybe((!parts_empty).then…)` per ModelPane.swift:120-152.
- [ ] **Step 3:** Frame pair into shots/: bar enabled vs disabled.

### Task 8: ToggleParameters menu entry

**Files:** `app/src/windows/project.rs` (View menu build).
- [ ] **Step 1:** Apply Task 2's `flip` ruling: flip-label ("Mostra parametri"/"Nascondi parametri", Commands.swift:69-73) if clean; else static label + carry-forwards divergence entry.
- [ ] **Step 2:** Menu-construction test + ⌥⌘I still routed once (no double-fire with the app router).

### Task 9: Space-through-editor fix

**Files:** `app/src/app.rs` (key router), `app/src/viewer_pane.rs` (drop the pane-level Space arm).
- [ ] **Step 1:** The router drops bare Space while any text editor holds focus (focus tracking per the pin's surface — the M7 module doc's revisit note); the pane's listener keeps Esc only.
- [ ] **Step 2:** Tests: subscription-level (Space with editor focus → no TogglePlayback; without → reaches the pane) — the subscription-builds pattern plus whatever focus surface the probe finds; MotionControls.swift:19 citation.

### Task 10: Speed-menu shape (ruled by Task 2)

**Files:** `app/src/motion_controls.rs`.
- [ ] **Step 1:** If `check` probe clean: the single menu (five speeds + Ripeti, MotionControls.swift:35-49) with the checkmark as selection state; else close as re-ruled divergence (record in carry-forwards, strike from future lists).

### Task 11: Fixwave (spinner, tie rounding, scrub strike, additionals)

**Files:** `app/src/windows/project.rs` + `app/src/chat_view.rs` (spinner sizing per the API probe), `app/src/motion_controls.rs` (half-up tie helper), `app/src/viewer_pane.rs` (scrub-emitter audit → strike), overlay-sync triggers (`project.rs` ParameterCommitted/BuildFinished arms), the Fit framing one-liner, `engine/src/agent/session.rs` (auth needle word boundary + test), `app/src/activity.rs` (manifest warnings), `README.md` (just note).
- [ ] **Step 1:** Each item: test-first where behavioral (tie helper: `0.125(1,2)` → "0.13"; auth needle: "Authorization failed 401x" does not match; overlay sync: rows update on the two events), construction/frame checks where visual (spinner), record-and-strike where theoretical (scrub).
- [ ] **Step 2:** The translucent-sort ruling applies here (Ruling 15): if Preston's verdict says visible, the far→near sort lands as this task's renderer step; else strike.

### Task 12: Version-popover watch (pin-bump probe)

**Files:** none in-tree (probe in a scratch worktree).
- [ ] **Step 1:** Worktree with libcosmic `cargo update` to the newest rev satisfying the wgpu-28 constraint; build + the drive repro (the jar project, study-drop + clock-click). Fixed upstream ⇒ the small revert (menu root restores, timestamps decision re-ruled); not fixed ⇒ carry the watch to M9. Either way: carry-forwards updated, worktree discarded.

### Task 13: Packaging (`just install`)

**Files:** `justfile` (install target), `app/resources/` (a .desktop + metainfo, icons already present), `README.md`.
**BLOCKING INPUT: APP_ID sign-off (Ruling 8) — Preston confirms `net.brownhouse.CadStudio` (or renames) before this task dispatches.**
- [ ] **Step 1:** `.desktop` (StartupWMClass = APP_ID — verified against COSMIC window grouping on :99), hicolor icons named per the icon-lookup law (PYTHON_SHEET_ICON assumption), metainfo with CARGO_PKG_VERSION sourcing.
- [ ] **Step 2:** `just install` → `~/.local` (the thelio hand-install becomes reproducible); `just uninstall`.
- [ ] **Step 3:** Verify on thelio: install, launch from the dock, window groups under one icon, uninstall clean.

### Task 14: Drive-harness extensions accumulated this milestone

**Files:** `app/src/drive.rs`.
- [ ] **Step 1:** Whatever Tasks 5-6 needed (`saveas <path>`, settings commands if added) lands as tested parse arms — the harness is how every UI task self-verifies.

### Task 15: Whole-branch capstone review (opus)

- [ ] **Step 1:** The most-capable-model whole-branch review over the M8 range (BASE recorded at wave start); feature-grep every task's user-visible behavior post-merge (the M6 lesson).

### Task 16: Visual match pass (the close gate's named step)

- [ ] **Step 1:** Enumerate the touched upstream screens (welcome, project window states, sheets, settings tabs, menus, version card, attachments surfaces); capture each port state on :99 (drive + xwd) into `shots/m8/`.
- [ ] **Step 2:** Side-by-side against the upstream gallery (`shots/upstream/`; the capture agent's report names what exists); divergences dispositioned into carry-forwards or fixed.
- [ ] **Step 3:** Preston's eyeball verdict on the gallery closes M8.

---

## Wave structure (dispatch order)

- **Wave 0:** Tasks 0, 1, 2 (busy-state, i18n, probes — 0 and 2 parallel; 1 serialized alone, it touches every module).
- **Wave 1:** Tasks 3, 4, 5, 6 (codex, attachments, ZIP, settings — 4 after 1; 3 and 6 share the settings surface, stagger their merges).
- **Wave 2:** Tasks 7-13 (parity polish + packaging; mostly independent).
- **Close:** Tasks 14-16 (accumulated harness, capstone, visual match).
