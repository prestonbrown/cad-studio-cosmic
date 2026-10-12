# M2 Agent Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Claude-agent turn lifecycle in the engine crate — CLI location, exact argv + prompt assembly, ground-truth stream-json decoding, the resume/fork session state machine with busy/missing/auth handling, process supervision, and the fake-agent end-to-end suite — per spec §10 M2.

**Architecture:** Pure engine library (no GUI deps). The stream decoder is designed against the PROBED shapes of claude 2.1.296 (docs/research/m2-probe/04-shapes.md — one-content-block assistant events, `rate_limit_event`, `is_error`-based error detection, stdin-merge hazard), NOT against upstream-contract §1's older event model; where probe and contract conflict, the probe wins, and where the contract describes app-side semantics (prompt strings, retry dance, classification lists), the contract wins. Codex is M8, out of scope.

**Tech Stack:** tokio (process/io/time/sync), serde, the M1 engine surface (settings, project store, outputs::missing_requirements with the two-dir MissingCtx).

**Spec:** docs/superpowers/specs/2026-10-11-cad-studio-cosmic-port-design.md (§4.2 agent/ + processes.rs, §10 M2). Behavioral authority: docs/research/upstream-contract.md §1; probe ground truth: docs/research/m2-probe/{REPORT.md,04-shapes.md}; fidelity sources: fixtures/upstream-source/{AgentSession,ClaudeRunner,StreamEvent,ToolSummary,ShellEnvironment,AgentLocator}.swift (vendor in Task 1) + fixtures/upstream/CADSystemPrompt.md.

## Global Constraints

- engine crate: NO GUI dependencies (tokio is fine; libcosmic/iced/wgpu never).
- Tests run on thelio via `ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && git pull -q && cargo test -p engine"'`; no network, no venv, no REAL claude invocation in tests — the fake agent covers everything; the real-agent smoke is a separate gated step.
- Stream serde: NO `deny_unknown_fields` anywhere; every enum has a fallback variant (`#[serde(other)]` or open-set `String`); `result.subtype` and `system.subtype` are open sets; error detection branches on `is_error` (+ `terminal_reason`/`api_error_status`), NEVER on subtype alone.
- Agent children spawn with **stdin = /dev/null** (`-p` merges stdin into the prompt) and **cwd = project root** (resume needs the original cwd).
- Upstream strings are verbatim: prompt note texts, classification substring lists (busy/missing/auth), tool-summary titles — from the vendored Swift.
- NEVER `git rm` (plain `rm` + `git add` the path); conventional commits; explicit-path staging.
- thelio login shell is `bash -lc`.

## Review Focus

1. **CLI drift (unknown event shapes)** → decoder ignores unknown top-level types and unknown subtypes, never errors a line → Task 3 corpus + synthetic unknown-line test.
2. **Error turns misclassified as success** (`result.subtype == "success"` with `is_error: true`) → classification uses `is_error`/`terminal_reason`/`api_error_status` → Tasks 3 + 8.
3. **Busy-session race** (resume collides with an active writer) → 5 s wait, one retry with resume, still busy → fresh turn preserving model.py → Task 8 fake-agent scenario.
4. **Cancellation leaves orphans** (child survives GUI cancel) → SIGINT → 3 s → SIGTERM, pid registry, orphan reaping → Task 6.
5. **Partial JSON replay corrupts tool previews** (input_json_delta first chunk is `""`) → preview builder tolerates empty/garbage partials → Task 4.

---

### Task 1: Probe corpus promotion + watch-flake fix

**Files:**
- Create: `fixtures/fake-agent/{02-minimal-session.jsonl, 03-tool-use.jsonl, 06-error-minimal.jsonl, 05-resume-fork-isolated.jsonl, fake_claude.py}` and `fixtures/fake-agent/scenarios/*.jsonl`
- Create: `docs/research/m2-probe/{REPORT.md, 04-shapes.md}` (moved from `.superpowers/sdd/m2-probes/`)
- Vendor (from `/tmp/cad-studio-src`, re-clone if gone): `fixtures/upstream-source/{AgentSession,ClaudeRunner,StreamEvent,ToolSummary,ShellEnvironment,AgentLocator,ProcessRunner,RunningProcesses}.swift`
- Modify: `engine/tests/watch.rs` (poll loop)

**Interfaces:**
- Produces: the corpus every later task's tests read; `fixtures/fake-agent/fake_claude.py` harness contract (Task 7/8 consume): invoked as `fake_claude.py <args...>` with env `CAD_FAKE_SCENARIO=<path>` and `CAD_FAKE_ARGV_OUT=<path>`; emits the scenario file's lines to stdout (honoring an optional `_sleep_ms` directive line), writes `{"argv": [...], "cwd": "..."}` to the argv-out path, exit code from `_exit` directive (default 0).

- [ ] **Step 1: Vendor the agent Swift sources** (8 files, same pattern as M1's Task 1 Step 0):

```bash
mkdir -p fixtures/upstream-source
cp /tmp/cad-studio-src/CADStudio/Agent/{AgentSession,ClaudeRunner,StreamEvent,ToolSummary,AgentLocator}.swift /tmp/cad-studio-src/CADStudio/Shared/{ShellEnvironment,ProcessRunner}.swift /tmp/cad-studio-src/CADStudio/App/CADStudioApp.swift /tmp/cad-studio-src/CADStudio/Agent/AgentMissingSheet.swift fixtures/upstream-source/ 2>/dev/null || cp /tmp/cad-studio-src/CADStudio/Agent/{AgentSession,ClaudeRunner,StreamEvent,ToolSummary,AgentLocator}.swift fixtures/upstream-source/
# RunningProcesses.swift lives in CADStudio/App/ or Shared — find it:
find /tmp/cad-studio-src -name "RunningProcesses.swift" -exec cp {} fixtures/upstream-source/ \; || true
# (if /tmp clone is gone: git clone --depth 1 https://github.com/filippofinke/cad-studio /tmp/cad-studio-src)
```

Adjust the exact paths to what exists (the fidelity set needed: AgentSession, ClaudeRunner, StreamEvent, ToolSummary, AgentLocator, ShellEnvironment, ProcessRunner, RunningProcesses). Commit whatever subset exists; note any absence in the report.

- [ ] **Step 2: Promote the probe corpus.**

```bash
mkdir -p docs/research/m2-probe fixtures/fake-agent/scenarios
cp .superpowers/sdd/m2-probes/REPORT.md .superpowers/sdd/m2-probes/04-shapes.md docs/research/m2-probe/
cp .superpowers/sdd/m2-probes/02-minimal-session.jsonl fixtures/fake-agent/
cp .superpowers/sdd/m2-probes/03-tool-use.jsonl fixtures/fake-agent/
cp .superpowers/sdd/m2-probes/06-error-minimal.jsonl fixtures/fake-agent/
# fork capture: strip the 8 hook lines (no isolation flags in that probe run)
jq -c 'select(.type != "system" or (.subtype | startswith("hook_") | not))' \
  .superpowers/sdd/m2-probes/05-resume-fork.jsonl > fixtures/fake-agent/05-resume-fork-isolated.jsonl
```

If `.superpowers/sdd/m2-probes/` no longer exists (scratch cleanup), regenerate 05 from `06-error-minimal`'s pattern is NOT acceptable — instead STOP and report: the captures are the ground truth; the controller re-runs the probe. (They exist as of plan-writing.)

- [ ] **Step 3: Write the fake agent harness** — `fixtures/fake-agent/fake_claude.py`:

```python
#!/usr/bin/env python3
"""Fake claude CLI for engine tests. Emits a scripted stream-json scenario.

Env: CAD_FAKE_SCENARIO=path to jsonl lines; CAD_FAKE_ARGV_OUT=path for argv dump.
Scenario directives (lines starting with '#'):  #sleep <ms>   pause before next line
                                                #exit <code>  exit code (default 0)
All other lines are written verbatim to stdout, flushed one by one.
"""
import json
import os
import sys
import time
from pathlib import Path

argv_out = os.environ.get("CAD_FAKE_ARGV_OUT")
if argv_out:
    Path(argv_out).write_text(json.dumps({"argv": sys.argv[1:], "cwd": os.getcwd()}) + "\n")

scenario = Path(os.environ["CAD_FAKE_SCENARIO"]).read_text().splitlines()
code = 0
for line in scenario:
    if line.startswith("#sleep"):
        time.sleep(int(line.split()[1]) / 1000.0)
        continue
    if line.startswith("#exit"):
        code = int(line.split()[1])
        continue
    sys.stdout.write(line + "\n")
    sys.stdout.flush()
sys.exit(code)
```

`chmod +x` it (the mode bit is committed).

- [ ] **Step 4: Author the adversarial scenario corpus** (synthetics per REPORT.md's recommendation — wording marked SYNTHETIC in a comment line where real wording wasn't captured):

`fixtures/fake-agent/scenarios/pong.jsonl` — the minimal turn shape (copy `02-minimal-session.jsonl`, keep init/one text block/result with `is_error:false`).
`fixtures/fake-agent/scenarios/tool-use.jsonl` — copy `03-tool-use.jsonl` verbatim (the serde torture test: interleaved tool_result, one-block assistants, message ids).
`fixtures/fake-agent/scenarios/error-result.jsonl` — copy `06-error-minimal.jsonl` (synthetic error assistant + `is_error:true` result).
`fixtures/fake-agent/scenarios/busy-then-success.jsonl` — `#exit 1` after a result-shaped line with `"is_error": true, "result": "...already has an active writer..."` (SYNTHETIC wording) + a stderr sidecar file `busy-then-success.stderr` containing the same phrase; the SECOND-run scenario `busy-then-success-second.jsonl` is a clean pong.
`fixtures/fake-agent/scenarios/missing-session.jsonl` — `#exit 1` + stderr sidecar `missing-session.stderr`: `No conversation found with session ID: 00000000-0000-0000-0000-000000000000` (upstream substring `No conversation found` must match).
`fixtures/fake-agent/scenarios/auth-error.jsonl` — `#exit 1` + sidecar `auth-error.stderr`: `Unauthorized: Invalid API key. Please run /login.` (case-insensitive list must match).
`fixtures/fake-agent/scenarios/cancel-mid-stream.jsonl` — init + a text block_start + `#sleep 5000` + text delta + result (the test SIGINTs during the sleep).
`fixtures/fake-agent/scenarios/tool-failure.jsonl` — a tool_use + tool_result with `"is_error": true, "content": "Traceback (most recent call last): boom"` + clean result.
`fixtures/fake-agent/scenarios/array-tool-result.jsonl` — tool_result with `content: [{"type":"text","text":"listed"}]` (contract's unobserved array form) + clean result.
`fixtures/fake-agent/scenarios/redacted-thinking.jsonl` — a `content_block_start` with `{"type":"redacted_thinking","data":"..."}` + clean result.
`fixtures/fake-agent/scenarios/unknown-events.jsonl` — an unknown top-level type `{"type":"future_event","payload":{...}}` + `system` with subtype `"future_subtype"` + a `result` missing optional fields + clean minimal result line — decoder must skip the unknowns and finish the turn.
`fixtures/fake-agent/scenarios/nonstreaming-json.jsonl` — per contract §1's streamingOutput=false fallback: a SINGLE `result`-shaped JSON object (one line, not an event stream).

(Each file committed; keep them small — 3–15 lines. The .stderr sidecars sit next to their scenarios; the fake harness gains a `CAD_FAKE_STDERR` env that cats a file to stderr if set — add that to fake_claude.py: 4 lines.)

- [ ] **Step 5: Fix the watch-test flake** — `engine/tests/watch.rs` `maps_files_to_reload_kinds`: replace the fixed `sleep(200ms)` + single drain with a bounded poll loop:

```rust
fn drain_until(rx: &std::sync::mpsc::Receiver<ReloadKind>, want: ReloadKind, deadline_ms: u64) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_millis(deadline_ms) {
        while let Ok(k) = rx.try_recv() { seen.insert(k); }
        if seen.contains(&want) { return true; }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}
```

Assert per-kind with `drain_until(&rx, ReloadKind::Manifest, 2000)` etc. Apply the same pattern to `storm_collapses_to_one_reload_per_file` and `second_write_after_quiet_emits_again` (deadline 2000 ms, then final drain). Verify by running the watch tests 5× on thelio under parallel load: `for i in 1 2 3 4 5; do cargo test -p engine --test watch 2>&1 | grep -c "0 failed"; done` — five `0`s.

- [ ] **Step 6: Commit**

```bash
git add fixtures/fake-agent fixtures/fake-agent/scenarios docs/research/m2-probe fixtures/upstream-source engine/tests/watch.rs
git commit -m "test(agent): promote probe corpus + fake-agent harness + scenarios; watch tests poll-loop"
git push thelio main
```

---

### Task 2: Agent locator

**Files:**
- Create: `engine/src/agent/mod.rs`, `engine/src/agent/locator.rs`, `engine/tests/agent_locator.rs`
- Modify: `engine/src/lib.rs` (`pub mod agent;`)

**Interfaces:**
- Produces: `engine::agent::locator::{find_agent(kind: AgentKind, custom_path: Option<&str>) -> Option<AgentPath>, AgentKind::{Claude}, AgentPath { path: PathBuf, version: String }}`. Discovery: custom path (tilde-expanded) → `$SHELL -lc 'echo $PATH'` dirs + fixed candidates → validate with `--version` (exit 0, first line captured). Candidates for claude (Linux): `~/.local/bin/claude`, `~/.cargo/bin/claude`, `~/.npm-global/bin/claude`, `/usr/local/bin/claude`, `/usr/bin/claude`, plus `~/.claude/local/claude` (upstream's first candidate). Consumes nothing from earlier M2 tasks.

- [ ] **Step 1: Failing tests** — `engine/tests/agent_locator.rs`:

```rust
use engine::agent::locator::{find_agent, AgentKind};

#[test]
fn custom_path_wins_and_is_tilde_expanded() {
    let tmp = std::env::temp_dir().join(format!("cad-loc-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let bin = tmp.join("my-claude");
    std::fs::write(&bin, "#!/bin/sh\necho '1.0.0 (fake)'\n").unwrap();
    make_executable(&bin);
    let home = tmp.to_string_lossy().replace('\\', "/");
    let custom = Some(format!("~/my-claude").replace("~", &home)); // absolute after expansion
    // tilde expansion test: pass "~/<relative-under-fake-home>" via HOME override
    std::env::set_var("HOME", &tmp);
    let found = find_agent(AgentKind::Claude, Some("~/my-claude"));
    std::env::remove_var("HOME");
    let found = found.expect("custom path resolves");
    assert_eq!(found.path, bin);
    assert!(found.version.starts_with("1.0.0"));
}

#[test]
fn login_shell_path_and_candidates_discovered_in_order() {
    // a fake claude in a temp dir; put that dir on PATH via a fake login shell is
    // environment-dependent — instead rely on the candidates list with HOME override
    let tmp = std::env::temp_dir().join(format!("cad-loc-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(tmp.join(".local/bin")).unwrap();
    let bin = tmp.join(".local/bin/claude");
    std::fs::write(&bin, "#!/bin/sh\necho '9.9.9 (fake)'\n").unwrap();
    make_executable(&bin);
    std::env::set_var("HOME", &tmp);
    let found = find_agent(AgentKind::Claude, None);
    std::env::remove_var("HOME");
    // may ALSO find a real claude on the test host's PATH — accept ours or any valid hit
    match found {
        Some(p) => assert!(p.version.contains('.')),
        None => panic!("expected ~/.local/bin/claude candidate to be found under fake HOME"),
    }
}

#[test]
fn invalid_binary_is_rejected() {
    let tmp = std::env::temp_dir().join(format!("cad-loc-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let bin = tmp.join("claude");
    std::fs::write(&bin, "#!/bin/sh\nexit 3\n").unwrap();
    make_executable(&bin);
    std::env::set_var("HOME", &tmp);
    // remove other candidates from contention: HOME override makes ~/.local absent, but
    // PATH discovery could still find a real one; assert OUR binary was skipped by
    // checking that if something is found it is NOT the failing one
    if let Some(p) = find_agent(AgentKind::Claude, Some(bin.to_str().unwrap())) {
        assert_ne!(p.path, bin, "--version exit != 0 must be rejected");
    }
    std::env::remove_var("HOME");
}

fn make_executable(p: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}
```

(Env-var mutation makes these three tests order-sensitive — annotate `#[serial]`-style by running them in one `#[test]` fn with three blocks instead, or accept the shared-HOME serialization by keeping them in one test. SIMPLEST: fold all three into one `#[test] fn locator_discovery()` with sequential blocks and restored env — do that.)

- [ ] **Step 2: FAIL; Step 3: Implement** `locator.rs` per the interface: tilde expansion (`~/` → `$HOME`), login-shell PATH via `$SHELL -lc "echo $PATH"` (fallback `/bin/bash -lc`), candidate list above, `--version` spawn with 3 s timeout (std::process, not tokio — locator is sync), first line as version. `agent/mod.rs`: `pub mod locator;` + module doc.

- [ ] **Step 4: PASS on thelio (the REAL claude at ~/.local/bin/claude will be found on thelio — the test's fake-HOME blocks must therefore run before any discovery that could pick the real one; keep the single-test-fn structure). Step 5: Commit** `feat(engine): agent locator — login-shell PATH + candidates + version validation`.

---

### Task 3: Stream-event decoder (probe-grounded)

**Files:**
- Create: `engine/src/agent/stream_event.rs`, `engine/tests/stream_event.rs`

**Interfaces:**
- Consumes: the corpus (Task 1).
- Produces: `engine::agent::stream_event::{StreamEvent, SystemEvent, AssistantEvent, ContentBlock, Delta, ToolResult, ResultEvent, parse_line(&str) -> Option<StreamEvent>}` where:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    System(SystemEvent),
    Stream { event: StreamInner },          // the stream_event wrapper; session_id captured
    Assistant(AssistantEvent),              // ONE content block per event (probe delta 4)
    UserToolResult(ToolResult),
    Result(ResultEvent),
    RateLimit,                              // new type: parsed enough to not error, then skipped
    Unknown(String),                        // forward-compat: unknown top-level type
}
```

with `SystemEvent { kind: SystemKind, session_id: Option<String>, estimated_tokens: Option<i64> }`, `SystemKind` an open set (`Init`, `Status`, `ThinkingTokens`, `HookStarted`, `HookResponse`, `Other(String)`); `ContentBlock::{Text{text}, Thinking{thinking,signature}, RedactedThinking, ToolUse{id,name,input:serde_json::Value}}`; `Delta::{Text{text}, Thinking{thinking}, InputJson{partial_json}, Signature{signature}, Other}`; `ToolResult { tool_use_id: String, content: ToolResultContent, is_error: Option<bool> }`, `ToolResultContent::{Text(String), Blocks(Vec<{type,text}>), Other}`; `ResultEvent { subtype: String, is_error: bool, result: String, session_id: Option<String>, terminal_reason: Option<String>, api_error_status: Option<i64> }` — `subtype` is `String` (open set), `is_error` defaults false ONLY via `#[serde(default)]` when absent (observed always-present), and the decoder NEVER rejects a line for unknown fields/types.

- [ ] **Step 1: Failing tests** — `engine/tests/stream_event.rs`:

```rust
use engine::agent::stream_event::{parse_line, ContentBlock, ResultEvent, StreamEvent};
use std::path::PathBuf;

fn fixture(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/fake-agent").join(p)
}

#[test]
fn corpus_02_decodes_line_by_line() {
    let lines = std::fs::read_to_string(fixture("02-minimal-session.jsonl")).unwrap();
    let mut n = 0;
    for line in lines.lines().filter(|l| !l.trim().is_empty()) {
        let ev = parse_line(line).unwrap_or_else(|| panic!("line failed: {line}"));
        n += 1;
        let _ = ev; // every line decodes to SOME variant
    }
    assert_eq!(n, 19);
    // terminal line is a result with is_error false
    let last = lines.lines().filter(|l| !l.trim().is_empty()).last().unwrap();
    match parse_line(last).unwrap() {
        StreamEvent::Result(r) => { assert!(!r.is_error); assert_eq!(r.result, "pong"); }
        other => panic!("last line is a result, got {other:?}"),
    }
}

#[test]
fn corpus_03_tooluse_shapes() {
    let lines: Vec<_> = std::fs::read_to_string(fixture("03-tool-use.jsonl")).unwrap()
        .lines().filter(|l| !l.trim().is_empty()).collect();
    let mut tool_results = 0;
    let mut assistant_tool_uses = 0;
    for line in &lines {
        match parse_line(line).unwrap() {
            StreamEvent::Assistant(a) => {
                if matches!(a.block, ContentBlock::ToolUse { .. }) { assistant_tool_uses += 1; }
            }
            StreamEvent::UserToolResult(_) => tool_results += 1,
            _ => {}
        }
    }
    assert_eq!(assistant_tool_uses, 2, "Write + Read");
    assert_eq!(tool_results, 2);
    // the Write tool_result interleaved MID-STREAM decodes with String content, is_error None
}

#[test]
fn corpus_06_error_result_is_detected_by_is_error() {
    let lines = std::fs::read_to_string(fixture("06-error-minimal.jsonl")).unwrap();
    let last = lines.lines().filter(|l| !l.trim().is_empty()).last().unwrap();
    match parse_line(last).unwrap() {
        StreamEvent::Result(r) => {
            assert_eq!(r.subtype, "success", "probe: subtype stays success on errors");
            assert!(r.is_error);
            assert_eq!(r.terminal_reason.as_deref(), Some("api_error"));
            assert_eq!(r.api_error_status, Some(404));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn unknown_types_and_subtypes_never_error() {
    for line in std::fs::read_to_string(fixture("scenarios/unknown-events.jsonl")).unwrap().lines() {
        if line.trim().is_empty() { continue; }
        let ev = parse_line(line).unwrap_or_else(|| panic!("rejected: {line}"));
        let _ = ev;
    }
    assert!(matches!(parse_line(r#"{"type":"future_event","x":1}"#).unwrap(), StreamEvent::Unknown(t) if t == "future_event"));
    assert!(matches!(
        parse_line(r#"{"type":"system","subtype":"brand_new","session_id":"s"}"#).unwrap(),
        StreamEvent::System(s) if matches!(s.kind, engine::agent::stream_event::SystemKind::Other(ref k) if k == "brand_new")
    ));
}

#[test]
fn tool_result_is_error_and_array_forms() {
    let lines = std::fs::read_to_string(fixture("scenarios/tool-failure.jsonl")).unwrap();
    assert!(lines.contains(r#""is_error": true"#));
    let arr = std::fs::read_to_string(fixture("scenarios/array-tool-result.jsonl")).unwrap();
    assert!(arr.contains(r#""content": [{"#));
    // both decode without error via full-file parse in later session tests; here assert the
    // single tool_result lines decode to the right variants
    for file in ["scenarios/tool-failure.jsonl", "scenarios/array-tool-result.jsonl"] {
        for line in std::fs::read_to_string(fixture(file)).unwrap().lines() {
            if line.contains("tool_result") {
                assert!(matches!(parse_line(line).unwrap(), StreamEvent::UserToolResult(_)));
            }
        }
    }
}

#[test]
fn synthetic_error_assistant_fields_tolerated() {
    // corpus 06's assistant has error/request_id/is_api_error_message/timestamp at top level
    let lines = std::fs::read_to_string(fixture("06-error-minimal.jsonl")).unwrap();
    let has_synth = lines.lines().any(|l| l.contains("is_api_error_message"));
    assert!(has_synth, "corpus must contain the synthetic error assistant");
    for line in lines.lines().filter(|l| l.contains("is_api_error_message")) {
        assert!(matches!(parse_line(line).unwrap(), StreamEvent::Assistant(_)));
    }
}
```

- [ ] **Step 2: FAIL; Step 3: Implement** `stream_event.rs` per the interface: serde with `#[serde(default)]` on optional fields, untagged/fallback enums, `parse_line` = `serde_json::from_str::<StreamEventRaw>` → mapped (unknown type → `Unknown(type_string)`); a parse failure on ONE line returns `None` (caller decides: log + skip — the session treats unparseable lines as noise, matching upstream's tolerance) — implement parse_line returning `Option` and a `parse_line_strict` used by tests.

- [ ] **Step 4: PASS; Step 5: Commit** `feat(engine): stream-event decoder grounded in claude 2.1.296 probe shapes`.

---

### Task 4: Tool summaries

**Files:**
- Create: `engine/src/agent/tool_summary.rs`, `engine/tests/tool_summary.rs`

**Interfaces:**
- Consumes: none (pure).
- Produces: `engine::agent::tool_summary::{summarize(name: &str, input: &serde_json::Value) -> String, live_summary(name: &str, partial_json: &str) -> String, rel_path(path: &str, root: &str) -> String}` — titles per the vendored ToolSummary.swift table (Bash first-word classes: pip/uv → "Installs components"; `python* model.py` → "Compiles the model"; other python → "Analyzes"; image converters → "Prepares a preview"; read-only file cmds → "Examines files"; sed/perl on model.py → "Edits the model"; other file shuffling → "Tidies files"; else "Works on the project". Write/Edit: basename model.py → "Writes the model"/"Edits the model" else support-file wording. Read: references/ → "Studies the reference"; schematic*/pdf → "Checks the drawing"; manifest.json → "Checks results"; image exts → "Checks a preview". Path display: relative to project root, venv python → "python", leading `cd <root> && ` stripped).

- [ ] **Step 1: Failing tests** — `engine/tests/tool_summary.rs`: a table test with every case above (inputs as `serde_json::json!` values; ~25 assertions), plus live-summary cases: partial `{"file_path": "/tmp/p/model.py", "conte` → "Writes the model" + trailing "…"; partial starting `{` empty → generic tool name; garbage `not json` → tool name. Plus `rel_path`: absolute under root → relative; the venv python rewrite; `cd /root && python model.py` → `python model.py`.

- [ ] **Step 2: FAIL; Step 3: Implement** by transcribing the vendored ToolSummary.swift classification (verbatim titles; the logic translated faithfully — read the Swift first). Live partial parsing: string-scan for the four keys (file_path/pattern/command/content) on the best-effort prefix (upstream's approach), never erroring.

- [ ] **Step 4: PASS; Step 5: Commit** `feat(engine): tool-summary classifier (upstream table) + live partial-JSON previews`.

---

### Task 5: Claude runner — argv + prompt assembly

**Files:**
- Create: `engine/src/agent/claude.rs`, `engine/tests/claude_runner.rs`

**Interfaces:**
- Consumes: M1 `settings::Settings`, `project::outputs::{missing_requirements, MissingCtx}`, `project::{ProjectMetadata, save_metadata}`; Task 2's AgentPath.
- Produces:

```rust
pub struct TurnRequest {
    pub prompt: String,
    pub attachments: Vec<std::path::PathBuf>,
}
pub fn build_args(
    claude: &AgentPath,
    prompt: &str,
    system_prompt: &str,
    s: &Settings,
    session_id: Option<&str>,
) -> Vec<String>;
pub fn render_system_prompt(tpl: &str, ctx: &PromptCtx) -> String;
pub struct PromptCtx {
    pub project_name: String,
    pub python_display: String,      // M2: "python interpreter not verified yet"; M5: venv line
    pub venv_python: std::path::PathBuf, // used in restricted-bash allowlist + {{PYTHON}}
    pub environment: Vec<String>,    // bullets: python line, model.py status, output/, references/
    pub units: String,
    pub printer: String,
    pub guidelines: String,
    pub date: String,                // numeric, no time
}
pub fn build_prompt(req: &TurnRequest, root: &Path, manifest: Option<&Manifest>) -> String;
pub fn copy_attachments(req: &TurnRequest, root: &Path) -> Vec<String>; // references/<unixts>-<i>-<name>
```

Argv per vendored ClaudeRunner.swift + contract §1: `-p <prompt> --output-format stream-json --verbose --include-partial-messages --append-system-prompt <rendered> --permission-mode acceptEdits --allowedTools Read,Write,Edit,Glob,Grep,Bash` (+ `--strict-mcp-config --setting-sources project,local` when isolates_claude; resume: `--resume <id> --fork-session`; model/effort when set). Restricted bash swaps the allowedTools value for the 15-entry allowlist (one arg, comma-joined, venv-python double-quoted iff it contains a space). Non-streaming fallback (streaming_output=false): `--output-format json` replaces the stream-json/verbose/include-partial trio.

- [ ] **Step 1: Failing tests** — `engine/tests/claude_runner.rs` (~6 tests, all real code in the plan): golden argv equality for the four flag combinations (default / isolated-off / restricted-bash / resume+model+effort); rendered prompt contains every `{{PLACEHOLDER}}` replaced and NONE left behind; `build_prompt` with attachments produces the "Attached reference files:" list + (empty text → "Create a model based on the attached files.") + the missing-note ("Note from CAD Studio: model.py does not produce these required outputs yet. Add them in this turn as well, following your instructions:" + indented missing list) driven by a temp project with model.py + partial output/; `copy_attachments` writes `references/<unixts>-0-sketch.png` and returns prompt-relative paths; system prompt template loaded from `fixtures/upstream/CADSystemPrompt.md` renders with all seven placeholders.

```rust
#[test]
fn argv_default_isolation() {
    let claude = fake_path(); // AgentPath { path: "/usr/bin/claude".into(), version: String::new() }
    let s = engine::settings::Settings::default(); // isolates_claude = true
    let args = build_args(&claude, "make a box", "SYSTEM", &s, None);
    assert_eq!(args[0], "-p");
    let joined = args.join("\u{1f}");
    assert!(joined.contains("--output-format\u{1f}stream-json"));
    assert!(joined.contains("--verbose"));
    assert!(joined.contains("--include-partial-messages"));
    assert!(joined.contains("--permission-mode\u{1f}acceptEdits"));
    assert!(joined.contains("--allowedTools\u{1f}Read,Write,Edit,Glob,Grep,Bash"));
    assert!(joined.contains("--strict-mcp-config"));
    assert!(joined.contains("--setting-sources\u{1f}project,local"));
    assert!(!joined.contains("--resume"));
}
```

The six companion tests (same file, same style — complete bodies):

```rust
#[test]
fn argv_restricted_bash_allowlist() {
    let mut s = engine::settings::Settings::default();
    s.restricted_bash = true;
    let args = build_args(&fake_path(), "p", "SYS", &s, None);
    let expected = "Read,Write,Edit,Glob,Grep,Bash(\"/v/python\":*),Bash(ls:*),Bash(cat:*),\
Bash(head:*),Bash(tail:*),Bash(wc:*),Bash(file:*),Bash(test:*),Bash(pwd:*),Bash(stat:*)";
    assert!(args.iter().any(|a| a == expected), "exact allowlist arg, got {args:?}");
}
```

(The expected string's venv-python segment uses ctx.venv_python, double-quoted iff it contains a space — this test's ctx uses "/v/python" (no space → unquoted); a second assertion with a spacey path ("My venv/python") covers the quoted form. Full list per vendored ClaudeRunner.swift: the 5 tools + Bash(<venv-python>:*) + the 9 read-only Bash prefixes.)

```rust
#[test]
fn argv_resume_fork_model_effort() {
    let mut s = engine::settings::Settings::default();
    s.claude_model = Some("opus".into());
    s.claude_effort = Some("high".into());
    let args = build_args(&fake_path(), "p", "SYS", &s, Some("sid-123"));
    let joined = args.join("\u{1f}");
    assert!(joined.contains("--resume\u{1f}sid-123"));
    assert!(joined.contains("--fork-session"));
    assert!(joined.contains("--model\u{1f}opus"));
    assert!(joined.contains("--effort\u{1f}high"));
}

#[test]
fn argv_nonstreaming_json() {
    let mut s = engine::settings::Settings::default();
    s.streaming_output = false;
    let args = build_args(&fake_path(), "p", "SYS", &s, None);
    let joined = args.join("\u{1f}");
    assert!(joined.contains("--output-format\u{1f}json"));
    assert!(!joined.contains("--include-partial-messages"));
    assert!(!joined.contains("stream-json"));
}

#[test]
fn system_prompt_all_placeholders_rendered() {
    let tpl = std::fs::read_to_string(fixture_upstream("CADSystemPrompt.md")).unwrap();
    let ctx = PromptCtx { project_name: "Gearbox".into(), python_display: "python".into(),
        environment: vec!["model.py does not exist yet: create it".into()],
        units: "millimeters (mm)".into(), printer: "Bambu Lab P1S".into(),
        guidelines: "walls".into(), date: "2026-10-12".into(), venv_python: "/v/python".into() };
    let out = render_system_prompt(&tpl, &ctx);
    assert!(!out.contains("{{"), "no placeholder left: {out}");
    assert!(out.contains("Gearbox"));
    assert!(out.contains("model.py does not exist yet: create it"));
}

#[test]
fn prompt_assembly_with_attachments_and_missing_note() {
    // temp project: model.py present, output/ has model.3mf only
    let root = mk_project(&["model.py"], &["model.3mf"]);
    let req = TurnRequest { prompt: String::new(), attachments: vec![PathBuf::from("/tmp/sketch.png")] };
    // copy_attachments first (writes references/), then build_prompt
    let rels = copy_attachments(&req, &root);
    assert_eq!(rels.len(), 1);
    let p = build_prompt(&req, &root, None);
    assert!(p.contains("Create a model based on the attached files."));
    assert!(p.contains(&rels[0]));
    let manifest = engine::project::Manifest::default();
    let p2 = build_prompt(&TurnRequest { prompt: "go".into(), attachments: vec![] }, &root, Some(&manifest));
    assert!(p2.contains("Note from CAD Studio: model.py does not produce these required outputs yet."));
    assert!(p2.contains("model.stl"));
}

#[test]
fn attachments_copied_with_timestamp_names() {
    let root = mk_project(&[], &[]);
    std::fs::write("/tmp/cad-att-sketch.png", b"png").unwrap();
    let req = TurnRequest { prompt: "x".into(), attachments: vec![PathBuf::from("/tmp/cad-att-sketch.png")] };
    let rels = copy_attachments(&req, &root);
    let refs_dir = root.join("references");
    let names: Vec<_> = std::fs::read_dir(&refs_dir).unwrap().collect();
    assert_eq!(names.len(), 1);
    let name = names[0].file_name().into_string().unwrap();
    assert!(name.ends_with("-0-cad-att-sketch.png"), "unixts-i-name: {name}");
    assert_eq!(rels[0], format!("references/{name}"));
}
```

(`mk_project(root_files, output_files)` helper creates tmp root + model.py + output/ copies from `fixtures/golden/`; `fixture_upstream(p)` = `fixtures/upstream/<p>`.)

- [ ] **Step 2: FAIL; Step 3: Implement** `claude.rs` by transcribing ClaudeRunner.swift's argv construction and AgentSession.swift's prompt assembly (attachment note text and missing-note text VERBATIM from the vendored Swift; `{{ENVIRONMENT}}` bullets: python line, `model.py exists (N lines)` / `model.py does not exist yet: create it`, `output/: a, b` or `output/ is empty`, `references/: ...` when non-empty). `build_args` is pure (no spawning).

- [ ] **Step 4: PASS; Step 5: Commit** `feat(engine): claude argv builder + system-prompt rendering + attachment/missing-note prompt assembly`.

---

### Task 6: Process supervision

**Files:**
- Create: `engine/src/processes.rs`, `engine/tests/processes.rs`
- Modify: `engine/Cargo.toml` (`tokio = { workspace = true }`), root `Cargo.toml` (workspace tokio: `features = ["rt-multi-thread", "process", "io-util", "time", "sync", "macros"]`)

**Interfaces:**
- Produces: `engine::processes::{spawn_streaming(argv: &[String], cwd: &Path, env: &[(String,String)]) -> std::io::Result<RunningProcess>, RunningProcess { pid: u32, lines: tokio::sync::mpsc::Receiver<Line>, stderr_text: tokio::sync::oneshot::Receiver<String> }, Line::{Stdout(String), Stderr(String)}, RunningProcess::interrupt(&self) (SIGINT → 3 s → SIGTERM, non-blocking), registry: `register(pid)`, `reap_orphans() -> Vec<u32>` (ppid==1 + /proc/<pid>/stat starttime matches the registry line), registry path `~/.local/share/cad-studio/running-processes`.** Spawn sets stdin to Stdio::null() ALWAYS.

- [ ] **Step 1: Failing tests** (tokio runtime per test; scripts under `std::env::temp_dir()`):
  - `stdout_and_stderr_stream_line_by_line`: script echoes 3 stdout lines + 1 stderr line + `#exit 0`; assert Line order per stream and stderr_text content.
  - `interrupt_escalates_to_sigterm`: script traps SIGINT (ignores it) then sleeps 30; interrupt(); assert process exits within ~5 s (SIGTERM path) and registry entry removed.
  - `gentle_interrupt_kills_cooperative_script`: script exits on SIGINT; interrupt(); exits quickly, exit status signaled.
  - `stdin_is_dev_null`: script reads stdin (`cat`) with none attached; must not hang (completes immediately).
  - `orphan_reaping`: spawn `setsid sleep 300` via a disowning wrapper, register its pid, kill the wrapper, force ppid==1 by double-fork pattern (a 5-line C-less trick: `sh -c 'sleep 300 & exit'` leaves the sleep re-parented to init); reap_orphans() returns and kills it.
  - `registry_survives_and_deduplicates`: register(pid) twice → one line.
- [ ] **Step 2: FAIL; Step 3: Implement** `processes.rs` (tokio::process, `kill -INT`/`kill -TERM` via libc-free `nix`-free approach: `std::process::Command::new("kill")` — simplest and test-visible; document choice). SIGINT is delivered to the child's pid only (not process group — matches upstream ProcessRunner.interrupt).
- [ ] **Step 4: PASS; Step 5: Commit** `feat(engine): process supervision — streaming spawn, SIGINT→SIGTERM, pid registry, orphan reaping`.

---

### Task 7: Session turn state machine (single turn) + EngineEvent

**Files:**
- Create: `engine/src/agent/session.rs`, `engine/tests/session_turn.rs`

**Interfaces:**
- Consumes: Tasks 2–6 + M1 store.
- Produces: `engine::agent::session::{Session, EngineEvent, TurnOutcome, TurnHandle}`:

```rust
pub enum EngineEvent {
    Started { session_id: Option<String> },
    Thinking { estimated_tokens: i64 },
    TextStreaming { text: String },          // accumulated unconfirmed text
    TextConfirmed { text: String },          // authoritative assistant text block
    ToolStarted { id: String, title: String },
    ToolInput { id: String, partial: String },
    ToolFinished { id: String, output: String, is_error: bool },
    TurnFinished(TurnOutcome),
}
pub enum TurnOutcome {
    Success { session_id: Option<String>, text: String },
    Failed { error: String, auth: bool },
    Busy,                 // both attempts collided; caller decides fresh turn
    MissingSession,       // cleared; caller re-runs fresh
    Cancelled,
}
pub struct Session::run_turn(&mut self, root: &Path, req: &TurnRequest) -> TurnHandle; // spawns the async turn; events via the handle's tokio mpsc
```

Turn mechanics per AgentSession.swift + probe: spawn via processes (stdin null, cwd root), tee EVERY raw stdout line to `.cadstudio/logs/<yyyy-MM-dd'T'HH-mm-ss>.jsonl` (created alongside), translate events (one-block assistant semantics: TextConfirmed per assistant text block — the CURRENT unconfirmed row is REPLACED by each confirmed text, matching upstream's reconcile), tool rows keyed by tool_use id (live partial previews throttled by the caller — engine emits every input_json_delta; the app throttles), thinking via thinking_tokens, session-id capture from init OR result (persist via save_metadata immediately), synthetic-error assistant ignored as text (model == "<synthetic>" or is_api_error_message → skip TextConfirmed), result → TurnFinished (Success iff !is_error; Failed with error=result text + auth classification), process exit without result → Failed with last-line stderr. Cancel via handle → interrupt() → Cancelled.

- [ ] **Step 1: Failing tests** — `engine/tests/session_turn.rs` (fake agent from Task 1; CLAUDE resolution order: env `CAD_TEST_CLAUDE` → locator):
  - `pong_turn_emits_full_event_sequence` (scenario pong.jsonl; assert event subsequence: Started → Text* → TurnFinished Success{text:"pong"}; project.json gains session_id; logs file exists with exactly N lines).
  - `tool_turn_tracks_tool_rows` (tool-use.jsonl: Write + Read ToolStarted/ToolFinished with outputs; is_error false; the interleaved mid-stream tool_result decodes and completes the Write row while Read still streams).
  - `error_result_fails_turn` (error-result.jsonl: TurnFinished Failed with auth=false, error mentions model_not_found/404).
  - `tool_failure_marks_row_failed` (tool-failure.jsonl: ToolFinished is_error true with Traceback text).
  - `unknown_events_do_not_break_turn` (unknown-events.jsonl: turn completes Success).
  - `argv_dump_asserts_flags` (fake writes argv JSON; assert resume/fork/isolation flags per settings; runs in a temp project root; cwd == root).
  - `cancel_mid_stream` (cancel-mid-stream.jsonl: start, wait for ToolStarted/TextStreaming, cancel → TurnFinished Cancelled within 2 s; process gone (kill -0 fails)).
- [ ] **Step 2: FAIL; Step 3: Implement** `session.rs` (single `run_turn` path; the retry/busy/missing/auth logic is Task 8 — `run_turn` classifies but does not retry).
- [ ] **Step 4: PASS; Step 5: Commit** `feat(engine): session turn state machine — event translation, tee logging, cancellation`.

---

### Task 8: Session lifecycle — busy/missing/auth/queue

**Files:**
- Modify: `engine/src/agent/session.rs`
- Create: `engine/tests/session_lifecycle.rs`

**Interfaces:**
- Produces: `Session::send(root, req)` — the full upstream turn dance: with session_id → resume+fork argv; on Busy (combined stdout+stderr contains "already has an active writer"): sleep 5 s, retry ONCE with resume, still busy → clear session_id, emit a system EngineEvent, run FRESH (model.py untouched); on MissingSession (substring list: "No conversation found" / "no rollout found" / "session not found" / "thread not found"): clear session_id, system event, fresh run; auth (case-insensitive list: "invalid api key", "/login", "not logged in", "authentication", "oauth token", "unauthorized", "401"): TurnOutcome::Failed{auth: true} — NO retry. Queue: `Session::queue` holds sends while a turn runs; auto-dispatch 1 s after Success; after Failed/Busy-fresh-failure the queue PAUSES (caller drains). Message texts for the system rows verbatim from AgentSession.swift.

- [ ] **Step 1: Failing tests** — `engine/tests/session_lifecycle.rs` using a stateful fake wrapper (`CAD_FAKE_SCENARIO` pointing at a scenario DIRECTORY: the fake reads `first.jsonl` then `second.jsonl` per invocation counter file — extend fake_claude.py by 6 lines: if scenario path is a directory, pick `<dir>/<n>.jsonl` where n = existing counter file + 1, write counter):
  - `busy_retries_once_then_fresh`: first/ busy, second/ busy, third/ pong → Busy…fresh…Success; session_id ends set from the THIRD run's init; assert exactly 3 spawns (argv dumps).
  - `busy_retry_succeeds_second_time`: first/ busy, second/ pong → Success with 2 spawns, resume used on both.
  - `missing_session_falls_back_fresh`: stderr has "No conversation found…" then pong → fresh run Success; metadata session_id replaced.
  - `auth_error_no_retry`: stderr auth text → Failed{auth:true}, exactly 1 spawn.
  - `queue_autoruns_after_success`: two sends queued; first turn Success → second auto-dispatches ~1 s later (assert 2 argv dumps, order).
  - `queue_pauses_after_failure`: first Failed → queued send NOT dispatched (assert 1 dump after 2 s).
- [ ] **Step 2: FAIL; Step 3: Implement** the dance in session.rs (time::sleep for the 5 s — test injects a shorter retry delay via `Session::with_retry_delay(Duration)`; production default 5 s).
- [ ] **Step 4: PASS; Step 5: Commit** `feat(engine): session lifecycle — busy retry, missing/auth fallbacks, message queue`.

---

### Task 9: M2 wrap — EngineCommand surface, docs, gate, (gated) real smoke

**Files:**
- Modify: `engine/src/agent/mod.rs` (re-exports + `EngineCommand`), `engine/src/lib.rs`, `docs/research/carry-forwards.md`

**Interfaces:**
- Produces: `engine::agent::{EngineCommand (Send{root, TurnRequest}, Cancel, Shutdown), EngineEvent}` re-exported at the crate root's agent module — the M4 app drives sessions through this pair.

- [ ] **Step 1: EngineCommand** thin wrapper (Send → Session::send; Cancel → running handle interrupt; Shutdown → interrupt all + reap) + a doctest-style integration test (tokio) driving one pong turn via the command enum.
- [ ] **Step 2: lib.rs docs** — agent module map (locator/stream_event/tool_summary/claude/session + processes), EngineCommand/EngineEvent note.
- [ ] **Step 3: carry-forwards** — add M4 lines: engine's EngineEvent is the app's chat-row source (tool rows keyed by tool_use_id; TextStreaming is the unconfirmed row); throttling of ToolInput is app-side (0.15 s per upstream); auth Failure carries an OpenTerminal action hint. Strike nothing (M1 section already struck).
- [ ] **Step 4: Full gate** — `cargo test --workspace --all-targets` thelio, all green, `grep -c warning` = 0. Expect 60 + ~35 new.
- [ ] **Step 5: (GATED, manual) Real-agent smoke** — do NOT run without Preston's go. Prepare the command in the report: `ssh thelio 'bash -lc "cd <fresh-project> && <engine smoke binary or test --ignored real_smoke>"'` with a one-word prompt; Preston runs it when ready.
- [ ] **Step 6: Commit** `docs: M2 complete — EngineCommand surface, module docs, carry-forwards`; push; report.
