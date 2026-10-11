# CAD Studio upstream porting contract

Upstream: github.com/filippofinke/cad-studio (MIT), macOS SwiftUI app, ~7.9k LOC Swift.
Source of truth for a Rust/COSMIC port. All quotes are from /tmp/cad-studio-src (unmodified).
Note: UI strings are Italian literals; source language of the catalog is `it`.

## 1. Agent CLI contract

### Claude Code invocation (ClaudeRunner.swift)
```
claude -p "<prompt>" --output-format stream-json --verbose --include-partial-messages \
  --append-system-prompt "<rendered CADSystemPrompt.md>" \
  --permission-mode acceptEdits \
  --allowedTools Read,Write,Edit,Glob,Grep,Bash \
  [--strict-mcp-config --setting-sources project,local] \
  [--resume <sessionID> --fork-session] [--model <m>] [--effort low|medium|high]
```
- Non-streaming fallback (setting `streamingOutput=false`): `--output-format json` replaces the stream-json triple.
- `--allowedTools` when `restrictedBash=false` is ONE arg: `"Read,Write,Edit,Glob,Grep,Bash"`.
- Restricted Bash (`restrictedBash=true`, Claude only): args `["Read","Write","Edit","Glob","Grep","Bash(<venv-python>:*)"]` plus `Bash(ls:*)`, `Bash(cat:*)`, `Bash(head:*)`, `Bash(tail:*)`, `Bash(wc:*)`, `Bash(file:*)`, `Bash(test:*)`, `Bash(pwd:*)`, `Bash(stat:*)`. Python path is double-quoted iff it contains a space (`quotedPython`).
- Personal config ignored (`isolatesClaude=true`, default): the flags above. Off → flags omitted.
- cwd = project root; env = login-shell env + `MPLBACKEND=Agg` + `PYTHONUNBUFFERED=1` (ModelBuilder.toolEnvironment).
- Attachments: NO CLI flags. Files are copied to `references/<unixts>-<i>-<name>` and listed in the prompt under "Attached reference files:\n- references/...". Prompt gets appended: "Note from CAD Studio: model.py does not produce these required outputs yet. Add them in this turn as well, following your instructions:" + missing list (missing expected outputs, plate.3mf, per-piece schematic-2.svg when parts>1, `drawings`/`bed` keys in manifest, PDF page count >= drawing pages). Empty text + attachments → prompt "Create a model based on the attached files."
- System prompt template placeholders (CADSystemPrompt.md): `{{PYTHON}}` (quoted path), `{{PROJECT_NAME}}`, `{{DATE}}` (numeric, no time), `{{UNITS}}`, `{{PRINTER}}` (model or "not specified"), `{{PRINTING_GUIDELINES}}` (per printer type, see §6), `{{ENVIRONMENT}}` (bullets: python package versions line, "model.py exists (N lines)" / "model.py does not exist yet: create it", "output/: <names>", "references/: <names>").

### Codex invocation (CodexRunner.swift)
```
codex exec --json --skip-git-repo-check --sandbox workspace-write -C <project-root> \
  -c approval_policy=\"never\" -c developer_instructions=\"<TOML-escaped system prompt>\" \
  [--ignore-user-config] [--model <m>] [-c model_reasoning_effort=\"low|medium|high\"] \
  [resume <sessionID>] [-i <image-path>]... <prompt>
```
- Same system-prompt string as Claude (same placeholders). TOML escaping: `\ " \n \r \t`, control chars as `\uXXXX`.
- Images (attachments that `Attachments.isImage` recognizes, i.e. any UTType conforming to image) DO go via repeated `-i <path>`; non-image references only via prompt list.

### Session lifecycle (AgentSession.swift)
- `--resume <id> --fork-session` (Claude) / `resume <id>` (Codex) when metadata.session_id exists.
- Session id captured from: Claude `system/init` (`session_id`) or `result` (`session_id`); Codex `thread.started` (`thread_id`). Saved to project.json immediately.
- "Busy session" retry: if turn errored and combined output contains "already has an active writer": sleep 5 s, retry once with resume; still busy → clear session id, post system message, run fresh (model.py is kept, only conversation context is lost).
- "Missing session": text contains "No conversation found" / "no rollout found" / "session not found" / "thread not found" → clear session id, system message, run fresh.
- Auth errors (case-insensitive any of: "invalid api key", "/login", "not logged in", "authentication", "oauth token", "unauthorized", "codex login", "401") → system message with "open terminal" action instructing `claude` / `codex login`.
- No timeouts. Cancellation = task cancel → SIGINT to child pid, SIGTERM 3 s later (ProcessRunner.interrupt). Pid registry file `~/Library/Application Support/CAD Studio/running-processes` (`pid:starttime` lines); on app launch, orphans (ppid==1, matching start time) are SIGTERMed; SIGTERM to app kills all children.
- Every turn's raw JSONL lines are tee'd to `.cadstudio/logs/yyyy-MM-dd'T'HH-mm-ss.jsonl`.

### Claude stream-json events consumed (StreamEvent.swift) → UI action
- `{"type":"system","subtype":"init","session_id":String}` → store session id, save project.
- `{"type":"system","subtype":"thinking_tokens","estimated_tokens":Int}` → thinking row "thinking · ~N tokens".
- `{"type":"stream_event","event":{...}}` with `event.type`:
  - `content_block_start` + `event.content_block` `{type:"text"|"thinking"|"redacted_thinking"}` or `{type:"tool_use", id:String, name:String}` (+`event.index:Int`) → begin streaming text row / thinking row (start clock) / live tool row.
  - `content_block_stop` (+index) → stop thinking row ("reasoned for N s") / finalize live tool title.
  - `content_block_delta` with `delta.type` `text_delta` (`delta.text:String`) → append streaming text; `input_json_delta` (`delta.partial_json:String`) → append to live tool input JSON (throttled 0.15 s updates, live title + last-4-lines preview); `thinking_delta` → "reasoning…" status.
- `{"type":"assistant","message":{"content":[...]}}` blocks: `{type:"text","text":String}` → authoritative text (replaces the streamed "unconfirmed" row); `{type:"tool_use","id":String,"name":String,"input":JSON}` → tool activity row with summarized title.
- `{"type":"user","message":{"content":[{type:"tool_result","tool_use_id":String,"content":String|[{"text":String}] ,"is_error":Bool}]}}` → complete tool row (output text, ok/failed state).
- `{"type":"result","subtype":String,"is_error":Bool,"result":String,"session_id":String?}` → end of turn; if no streamed text, `result` becomes the assistant message.

### Codex JSON events consumed (CodexTranslator)
- `{"type":"thread.started","thread_id":String}` → started.
- `item.started`/`item.completed` with `item` `{id, type}`:
  - `command_execution` (`command`, `status`, `exit_code`, `aggregated_output`) → synthesized Bash toolUse + toolResult (failed if status=="failed" or exit_code!=0); shell wrapper `/bin/zsh -lc '...'` etc. is unwrapped for display.
  - `file_change` (`status`, `changes:[{path, kind}]`) → per change a synthesized Write (kind=="add") or Edit toolUse+result with `{"file_path": path}`.
  - `agent_message` (`text`) → assistant text block (blockStarted index tracking).
  - `reasoning` → thinking block start/stop.
- `turn.completed` → finished(success, last agent_message text); `turn.failed` (`error.message`) → finished(error); `error` (`message`) → remembered.

### Tool summaries (ToolSummary.swift)
Title mapping by tool name: Bash → classify first word of command (pip/uv → "Installs components"; `python* model.py` → "Compiles the model", other python → "Analyzes"; rsvg-convert/sips/magick/convert/qlmanage → "Prepares a preview"; ls/cat/head/tail/grep/rg/find/wc/file/stat/sed(non -i) → "Examines files"; sed/perl on model.py → "Edits the model"; sed/mv/cp/rm/mkdir/touch/perl → "Tidies files"; else "Works on the project"). Write/Edit: path basename model.py → "Writes/Edits the model" else support file. Read: `references/` prefix → "Studies the reference"; schematic*/pdf → "Checks the drawing"; manifest.json → "Checks results"; png/jpg/jpeg/svg/webp → "Checks a preview". Live parsing of partial JSON by string scan for keys `file_path`, `pattern`, `command`, `content` (Write line count appended). Paths shown relative to project root; the venv python path is rewritten to `python` and a leading `cd <root> &&` is stripped.

### Chat model (ChatMessage.swift / ChatViewModel.swift)
- `ChatMessage{id: UUID, role: user|assistant|tool|thinking|system|summary, text: String, date: Date, tool?: ToolActivity, detail?: String, action?: askClaudeToFix|openTerminal|configureClaude, attachments?: [String]}` — persisted as JSON array `.cadstudio/chat.json` (ISO8601 dates).
- `ToolActivity{toolUseID: String, name: String, input: String, output?: String, preview?: String, state: running|succeeded|failed}`.
- State machine essentials: streaming text creates one "unconfirmed" assistant message reconciled (setText) by the next full assistant text block; tool rows keyed by tool_use_id (update/complete); on process end all still-running tool rows are marked failed; thinking row active while block open. Message queue: sends while busy are queued (QueuedMessage), auto-run 1 s after a successful turn, paused after a failure ("Send now" button).

## 2. Python environment contract

- Venv location: `URL.applicationSupportDirectory/CAD Studio/venv` → port to `~/.local/share/cad-studio/venv`. Interpreter = `<venv>/bin/python`.
- Packages: `build123d`, `matplotlib`, `numpy` — unpinned latest.
- Creation (PythonEnvironment.install): if `uv` found in login-shell PATH → `uv venv --python 3.12 <dir>` then `uv pip install --python <interpreter> build123d matplotlib numpy`. Else scan `python3.13, python3.12, python3.11, python3.10, python3` (login shell `command -v`, accept `--version` parsing to 3.10–3.13) → `<python> -m venv <dir>`, `<venv-python> -m pip install --upgrade pip`, `<venv-python> -m pip install build123d matplotlib numpy`. Progress milestones 0.05/0.1/0.2/0.25/0.95/1.0, per-line log.
- Verification: `<venv-python> -c "import sys, build123d, matplotlib, numpy; print(build123d.__version__); print(f'Python {…}, build123d {…}, matplotlib {…}, numpy {…}')"`; line 1 = build123d version, line 2 = `versions` string injected into `{{ENVIRONMENT}}`.
- Running the model (ModelBuilder): `<venv-python> model.py`, cwd = project root, env = shell env + `MPLBACKEND=Agg` + `PYTHONUNBUFFERED=1`, stdin = /dev/null. stdout+stderr streamed line-by-line (no UI display unless error). Exit 0 → success path (reload outputs, record version). Exit != 0 → `buildError` = last line of (stderr if non-empty else stdout), activity failed, chat system message with full traceback + "ask agent to fix" action (sends the traceback back as a user message). Interrupted → "compilation interrupted".
- Parameter rebuild path: slider commit → `Project.setParameter` rewrites model.py via regex `(?m)^\s*<name>\s*(?::[^=\n]+)?=\s*([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)` replacing only the number literal (integer formatting when the value is integral and the original had no `.`), atomic write, then `build(versionPrompt: "<name> = <value>")` — i.e. the app itself re-runs model.py, the agent is NOT invoked. Missing constant → failed status "Parameter <name> not found in model.py".
- Prompt ordering of parameters: manifest parameters sorted by first index of the constant name in model.py text (fallback alphabetical).

## 3. Output file contract

`output/` filenames (ProjectFolder): `model.3mf`, `model.stl`, `model.step`, `plate.3mf`, `schematic.pdf`, `schematic.svg`, `schematic.png`, `schematic-2.svg`, `schematic-3.svg`, … (per-piece pages), `manifest.json`, `animation.json`. `expectedOutputs` = [3mf, stl, step, svg, png, pdf, manifest.json] (plate.3mf and pages are demanded through the prompt/missing-list instead).

### manifest.json — fields the app decodes (Manifest.swift)
- `bounding_box: {x: Double, y: Double, z: Double}` (optional)
- `warnings: [String]` (missing → [])
- `parameters: {String: Double}` (non-numeric entries dropped)
- `drawings: [{file: String, title: String}]` (page titles matched by filename)
- `bed: [Float, Float]` (valid only when exactly 2 values, both > 0; printer bed w×d mm; drives the plate view bed outline)

The system prompt additionally makes the agent write `name`, `description`, `units`, `volume_mm3`, `surface_area_mm2`, `iteration`, `parts: [{name, quantity, color-hex-or-null}]` — the app ignores those.

### animation.json (MotionStudy.swift)
- `title: String?`
- `frames: [{t: Double (required), label: String? (phase name), parts: {<part-label>: {translate: [x,y,z] (3 floats, else identity), rotate: [qx,qy,qz,qw] (4 floats, normalized on load; missing → identity)}}, values: {String: Double} (≤4 named values), collisions: [{parts: [String], volume: Double?}]}]`
- Frames sorted by t on load; study active only when frames.count > 1. `duration` = last frame t. Interpolation between surrounding frames: lerp translate, slerp rotate, lerp values; label = last frame with t <= time that has one; collisions taken from nearest frame. valueNames = union of `values` keys in (frame order, sorted-key) order.

### 3MF parsing (ThreeMFParser.swift)
- Container read via `/usr/bin/unzip -p <file> 3D/3dmodel.model` (port: zip crate). XML elements: `model[unit= micron|centimeter|inch|foot|meter → mm-per-unit 0.001/10/25.4/304.8/1000, default 1]`; color groups `basematerials`/`colorgroup` (attr `id`) containing `base[displaycolor]` / `color[color]` as `#RRGGBB` or `#RRGGBBAA`; `object[id, name, pid, pindex]` with `vertex x/y/z`, `triangle v1 v2 v3 pid p1`, and `component[objectid, transform]`; `build/item[objectid, transform]`. `transform` = 12 numbers (3×4, row-major rows → matrix columns in code).
- Parts = one mesh per build item (no build items → one per object id, sorted); recursion depth cap 16; part name = object `name` else first descendant component's name else `part N+1`. Per-triangle color from triangle pid/p1 else object pid/pindex; uncolored triangles → material 0 = app neutral color.
- STL (STLParser): binary iff `size == 84 + 50*count`; else ASCII `vertex x y z` triplets. Single color.

### Schematic expectations
Page 1 `schematic.svg` (+`schematic.png` 300 dpi page 1 only, `schematic.pdf` all pages via matplotlib PdfPages). Extra pages discovered by listing output/ for `schematic-<int>.svg`, sorted by number. Page title = manifest `drawings[].title` matching the filename, else "Page N". PNG doubles as recent-project thumbnail.

### Change detection (FileWatcher.swift / ProjectOutput.reload)
FSEvents stream on the `output/` directory only, 0.3 s latency, file-events flag → `reload()` which compares contentModificationDate per file:
- `manifest.json` changed → re-parse manifest + parameters.
- `plate.3mf` changed → re-parse plate parts (off-main-thread).
- `animation.json` changed → reload motion study.
- `model.3mf` or `model.stl` changed → re-parse mesh (3MF preferred, STL fallback; previous parse task cancelled).
- Any `schematic*.svg` or `schematic.png` changed → bump schematicRevision (forces WKWebView reload).
(References/, model.py and .cadstudio/ are not watched; they are re-read at turn end / build end.)

## 4. Version history & project store

- `.cadstudio/` layout: `project.json`, `chat.json`, `logs/<ts>.jsonl` (raw agent lines), `versions/<n>/{model.py, output/* (full copy), chat.json, version.json}`.
- `version.json`: `{number: Int, created_at: ISO8601 Date, prompt: String, session_id: String?}`.
- Recording: after a successful agent turn (only when every expected output exists) or manual build, and only if model.py differs from the current version's copy. A `summary` chat message ("Version N", detail = number) marks it in the transcript.
- Restore: copy back model.py + chat.json, DELETE every file in output/ and copy the version's output/ in; `metadata.session_id = version.session_id` (next turn resumes/forks that conversation), `currentVersion = n`; chat and outputs reloaded from disk.
- `project.json` (ProjectMetadata): `{session_id?: String, current_version?: Int, created_at: Date, updated_at: Date, app_version: String, layout?: LayoutNode}` — ISO8601 dates, pretty-printed sorted keys, atomic writes. LayoutNode encodes as `{"pane":"chat"|"model3D"|"schematic"}` or `{"split":{"axis":"row"|"column","first":<node>,"second":<node>,"fraction":Double}}`.
- Pane layouts: 4 presets (chatLeading 0.34, chatTrailing 0.66, chatBottom 0.62, threeColumns 0.28) + arbitrary drag-drop rearrangement; minimum pane thickness: chat-in-row 300 px, others 240 (row) / 140 (column). Default layout persisted globally in UserDefaults.
- RecentProjects: UserDefaults `recentProjects` = [paths], max 12, deduped by standardized path, filtered to existing folders at load. Welcome window shows them with `output/schematic.png` thumbnails. New project flow: name validation (non-empty, no leading ".", no "/" or ":") → parent-folder picker → create `root/output`, `root/.cadstudio/logs`, write default project.json → open window.

## 5. 3D viewer inventory

- Rendering (ModelViewer.swift): RealityKit; model axis remap (model is Z-up) scene = (x, z, −y). PerspectiveCamera near 1 / far 100000 / FOV 35°. Lights: key directional 2600 from (60,120,90), fill 900 from (−90,40,−60).
- Orbit/zoom/pan: orbit yaw −= dx·0.008, pitch clamped ±1.5607 rad; camera = target + (cosP·sinY, sinP, cosP·cosY)·distance, up (0,1,0). Zoom clamps distance to [0.1·radius, 200·radius]; scroll = 0.9^steps, pinch = 1/(1+mag). Pan scales by 2·distance·tan(halfFov)/viewHeight along camera right/up. Frame: radius = max(|size|/2, 1), distance = radius/sin(halfFov)·1.15, target (0, size.y/2, 0); presets iso (yaw π/4, pitch 0.6) / front (0,0) / top (0, max) / right (π/2, 0). Re-frames when mesh size changes >50%.
- Input (CameraInputView): left-drag orbit (opt/shift-drag pan), right/middle-drag pan, click (drag < 4 px) = measure pick via CPU Möller–Trumbore over the whole displayed mesh.
- Grid: 10 mm lines, half-extent = max(100, ceil(max(x,y footprint)·0.75/10)·10); minor 10% alpha / 0.25 width, major (every 100 mm) 28% / 0.5; axis tripod (X red, Y green, −Z-scene blue... model axes X red, Y green, Z blue) at footprint corner + 15% margin. Bed outline rectangle (accent color) at y=0.06 when in plate layout with manifest `bed`.
- Wireframe: PBR material triangleFillMode=lines, faceCulling none (fill mode + back culling otherwise); metallic 0.15, roughness 0.55; material 0 = neutral (dark 0.62/0.70/0.80, light 0.55/0.64/0.75), then one material per part color; 12-swatch color legend in the status bar.
- Measure: two picked points → orange spheres (r = max(0.03·radius, 0.4)) + connecting box; badge shows total, ΔX, ΔY, ΔZ in the display unit; Esc clears; resets on layout/mesh change.
- Section: per-triangle Sutherland–Hodgman clip on CPU (fan triangulation), axis X/Y/Z, position 0.001–0.999 of bbox; keeps-below for X/Z, keeps-above for Y; cut face drawn red unlit with front-face culling.
- Part arrangement (PartArrangement.swift): grouping strips a trailing `[_\-\s]?\d+` suffix (name_1/name_2 group together, ×N badge). Exploded: largest-bbox-volume part is anchor; others pushed along the dominant axis from anchor center in steps until no bbox overlap with gap = max(0.06·overall size, 2 mm), scaled by explode slider 0–2; result lifted so min-Z matches assembled floor; "attach" (link) keeps a group with the anchor. Plate: plate.3mf parts, or fallback shelf-packing (sort by descending Y, rows of width max(√area·1.3, maxWidth), 5 mm gaps, centered). Hidden groups filtered from the merged mesh.
- Animation: ModelScene swaps the single mesh for per-part entities; static parts translucent (opacity 0.22), moving parts opaque; 60 fps timer advances MotionPlayer (speed 0.1–2×, loop), poses = translate + quaternion from `state(at:)`; parts in a collision frame get emissive red material; MotionControls overlay = play/pause (space), scrub slider, speed/loop menu, phase label, interpolated values, collision list with mm³ volumes.
- Version compare: chosen version's output parsed off-thread; rendered as orange unlit wireframe ghost (opacity 0.6) alongside current; parameters panel shows struck-through old values in orange.
- Parameters overlay: sliders from manifest.parameters (order = first occurrence in model.py), range heuristic (v>0: 0…max(2v,1); v<0: 2v…max(−2v,1); 0: −10…10), commit rounds to integer when integral else 3 decimals; each commit rewrites model.py and re-runs it directly (see §2).

## 6. Settings, units, i18n, print rules

All settings in UserDefaults (plist) — port to a JSON/TOML file under XDG config. Keys and defaults:
- `defaultProjectLocation` (String path; default Documents), `agentEngine` ("claude"|"codex", default claude), `claudePath`/`codexPath` (custom executable override, tilde-expanded), `claudeModel`/`codexModel` (free text), `claudeEffort` (""|low|medium|high), `restrictedBash` (Bool, default false), `streamingOutput` (default true), `isolatesClaude` (default true), `measurementUnit` (mm|cm|m|in; default = US region ? in : mm), `printerType` (fdm|fdmMultiMaterial|resin|sls, default fdm), `printerModel` (free text, e.g. "Bambu Lab P1S"), `hasCompletedSetup` (Bool), `defaultLayout` (JSON Data of LayoutNode), `recentProjects` ([String]), `partsLayout` (assembled|exploded|plate, default exploded), `partsCollapsed`, `parametersCollapsed`, `AppleLanguages` (language override).
- Agent executable discovery (AgentLocator): custom path → login-shell `command -v` → hardcoded candidates. Claude: `~/.claude/local/claude`, `~/.local/bin/claude`, `/opt/homebrew/bin/claude`, `/usr/local/bin/claude`, `~/.npm-global/bin/claude`. Codex: `~/.local/bin/codex`, `/opt/homebrew/bin/codex`, `/usr/local/bin/codex`, `~/.npm-global/bin/codex`, `~/.cargo/bin/codex`. Validated with `--version` exit 0. PATH from `/bin/zsh -lc "echo $PATH"` + fallback dirs (add ~/.cargo/bin etc. for Linux).
- Units: mm/cm/m/in with mmPerUnit 1/10/1000/25.4; display fraction digits 1/2/3/2; files always mm; prompt name e.g. "millimeters (mm)".
- Print rules: `PrinterType.promptGuidelines` — hardcoded English strings in PrintPreferences.swift injected via `{{PRINTING_GUIDELINES}}` (FDM: walls ≥ 1.2 mm, fit clearance 0.2 mm, overhangs ≤ 45°, teardrop holes…; multi-material FDM: one color per 3MF part; resin: walls ≥ 0.8 mm, clearance 0.1–0.15 mm, drain holes ≥ 3 mm…; SLS: clearance 0.3–0.5 mm, escape holes ≥ 5 mm, no colors). Not stored anywhere else; only reach the agent through the system prompt. First-run SetupSheet collects printer type/model/unit.
- i18n: String Catalog `Localizable.xcstrings`, source language `it`, 336 keys, complete translations for it/en/de/fr. Language switch writes `AppleLanguages`, applied next launch.

## 7. macOS-specific inventory + port risks

| macOS API | Where | Rust/COSMIC equivalent |
|---|---|---|
| RealityKit (RealityView, ModelEntity, MeshDescriptor, PBR/UnlitMaterial, PerspectiveCamera, lights) | ModelViewer | wgpu renderer (or rend3/filament/bevy_render); custom shaders for unlit/wireframe/translucent; per-face material indices trivial |
| NSEvent mouse/scroll/magnify in NSView subclass | CameraInputView, SheetWebView | cosmic/iced pointer + scroll events; touchpad pinch via libinput gesture events |
| WKWebView + WKScriptMessageHandler (SVG pane, JS zoom/pan bridge, context-menu suppression) | SVGWebView | render SVG natively (resvg → cairo/skia pixmap in a canvas widget); the JS only implements zoom/pan/fit, all reproducible natively — no webview needed |
| PDFKit (PDFDocument pageCount) | Project.missingRequirements, SchematicPane | `lopdf`/`pdf` crate or mupdf bindings, page count only |
| FSEvents (FSEventStreamCreate, file events, 0.3 s) | FileWatcher | `notify` crate (inotify), keep 0.3 s debounce |
| DispatchSourceSignal SIGTERM handling | CADStudioApp | signal-hook / tokio::signal; keep "kill children on SIGTERM" semantics |
| NSPasteboard (files + PNG/TIFF data), NSEvent keyDown monitor for Cmd+V | Attachments, ChatInputView, CodeBlock copy | arboard/clipboard-manager; cosmic text-input key handling; pasted image → `image` crate → PNG temp file |
| Drag & drop (onDrop fileURL/image, NSItemProvider) | ChatView | cosmic/iced dropped-file events |
| NSSplitViewController + NSHostingController pane tree, drag-handle pane rearrangement | PaneSplitViewController, PaneContainer, ProjectWindow | custom split layout in cosmic (or iced panes); hit-testing logic ports as-is |
| NSOpenPanel / NSSavePanel | ProjectPanels, ChatInputView, export | xdg-desktop-portal FileChooser (cosmic-files) |
| NSWorkspace (reveal, open in slicer, launch Terminal.app, file icons) | Project, ChatView, AgentMissingSheet | `gio open`/DBus FileManager.ShowItems, xdg-open, `/usr/bin/x-terminal-emulator` or `cosmic-term`; mime icons via shared-mime-info |
| `/usr/bin/zip` export, `/usr/bin/unzip -p` 3MF | Project.exportPackage, ThreeMFParser | `zip` crate both ways |
| UserDefaults plist | AppSettings, RecentProjects, @AppStorage | config file in ~/.config/cad-studio (serde JSON/TOML) |
| `/bin/zsh -lc` login-shell PATH | ShellEnvironment | `$SHELL -lc` (bash/zsh/fish differ — parse defensively) + XDG-era fallback dirs |
| Application Support dir for venv + pid registry | PythonEnvironment, RunningProcesses | ~/.local/share/cad-studio; /proc/<pid>/stat (ppid, starttime) for orphan reaping |
| proc_pidinfo/proc_bsdinfo | RunningProcesses | /proc parsing |
| Swift concurrency (@Observable/@MainActor, Task, AsyncStream) | everywhere | tokio + Elm-style update messages |
| Localizable.xcstrings + AppleLanguages | i18n | fluent-rs bundles; source language is Italian |
| AttributedString markdown (inline-only) | MessageView | pulldown-cmark → styled text |
| Timer 60 fps on RunLoop | ModelScene playback | render-loop tick / request_animation_frame |
| simd (SIMD3/4, quatf, slerp, float4x4) | everywhere | glam |
| NSImage thumbnails / tiff→png | Attachments, WelcomeWindow | `image` crate |
| Menus/Commands/keyboard shortcuts, toolbar, About panel | Commands, ProjectToolbar | cosmic app-menu + key bindings |
| Assets.xcassets, Bundle CFBundleShortVersionString | Bundle+Version | .desktop + icon files, env!("CARGO_PKG_VERSION") |
| Sudden/automatic termination APIs | RunningProcesses | no-op / drop |

### Top 10 port risks (ranked)
1. 3D viewer parity: RealityKit gives PBR materials, per-face materials, wireframe fill mode, transparency, picking, and a camera for free; a wgpu port must hand-roll all of it (the mesh is unindexed, per-face material indices, no scene graph). Largest single workstream.
2. SVG schematic pane: WKWebView renders arbitrary matplotlib SVG and provides zoom/pan; resvg lacks interactivity but the needed behavior (zoom/pan/fit + scrollbars) is simple enough to rebuild — risk is SVG feature coverage (fonts, dashes) in resvg vs WebKit.
3. Agent CLI drift: the port pins behavior to today's `claude` stream-json shapes and `codex exec --json` experimental event names; both are moving targets and the Codex translator synthesizes fake tool_use events. Version-test both.
4. Session resume/fork semantics: `--resume --fork-session` + "already has an active writer" retry dance is subtle; getting it wrong loses conversation context or double-runs turns.
5. Login-shell PATH resolution: macOS zsh tricks don't map; Linux shells (bash/fish/nix) and Flatpak sandboxes hide user-installed CLIs. Needs robust candidate list + `$SHELL -lc`.
6. Python env reproducibility: unpinned build123d breaks often (OCC API churn); uv-vs-venv dual path, python3.10–3.13 detection, and the 30 s model runtime budget must be kept; consider pinning versions per platform.
7. File watching and reload ordering: 0.3 s FSEvents coalescing vs inotify event storms while the agent writes many files; modification-date dedup logic must port exactly or the viewer flickers/misses updates.
8. Pane drag-and-drop layout editor: custom hit-testing + NSSplitView behaviors (fraction persistence, min thickness, rebuild-on-structure-change) — fiddly to redo in cosmic without a split widget of equal power.
9. Parameter rewrite regex on model.py: the `(?m)^ name ( : type )? = number` pattern is load-bearing for slider rebuilds; a Rust regex port must match Swift NSRegularExpression semantics exactly (or the app corrupts scripts).
10. i18n inversion: source strings are Italian with 4 catalogs; a port must decide the source language and keep 336 keys × 4 locales in sync (fluent), or the localized UI regresses.
