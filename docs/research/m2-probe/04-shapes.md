# Capture 4 — shape extraction (from 02-minimal-session.jsonl + 03-tool-use.jsonl)

Claude Code 2.1.296, Linux (thelio), user pbrown. All shapes below are verbatim-observed.
`?` = nullable (seen null or absent). "extra keys tolerated" = field list is NOT closed.

## Top-level event types observed

| type | where | notes |
|---|---|---|
| `system` (subtypes: `init`, `status`, `thinking_tokens`, `hook_started`, `hook_response`) | all captures | hook_* only when user settings load (no `--setting-sources project,local --strict-mcp-config`) |
| `stream_event` | c2/c3 only (needs `--include-partial-messages`) | wrapper: `{type, event, session_id, parent_tool_use_id?, uuid, ttft_ms?, thinking_display?, api_message_id?}` |
| `assistant` | all | one content block per event (NOT cumulative) |
| `user` | c3 | tool_result carrier |
| `result` | all | terminal, one per turn |
| `rate_limit_event` | c2/c3/c5 | new type, emitted just before `result` |

## system/init (fields, in capture order)

`cwd, session_id, tools[], mcp_servers[], model, permissionMode, slash_commands[], terminal_slash_commands[], apiKeySource, claude_code_version, output_style, agents[], skills[], plugins[], capabilities[], analytics_disabled, product_feedback_disabled, uuid, memory_paths{auto}, messaging_socket_path, fast_mode_state, fast_mode_disabled_reason, per_turn_effort_active, view_mode`

- With isolation flags (`--strict-mcp-config --setting-sources project,local`): `tools` = 26 builtins, `mcp_servers: []`, no plugins/hooks fire.
- Without them: user plugins inflate `tools` (`mcp__*` names), `mcp_servers` lists status per server, and `hook_started`/`hook_response` events precede init.
- `permissionMode` echoes the flag (`acceptEdits`) or the default (`auto` when unset).

## system/status

`{type:"system", subtype:"status", status:"requesting", session_id, uuid}` — after init and again between API turns.

## system/thinking_tokens

`{type:"system", subtype:"thinking_tokens", estimated_tokens:Int, estimated_tokens_delta:Int, session_id, uuid}` — `estimated_tokens_delta` is NEW vs contract §1. Interleaves with thinking deltas. In these runs the thinking *text* arrived EMPTY (`thinking:""`, signature present) — `thinking_tokens` events are the reliable thinking signal, not block text.

## stream_event.event variants

- `message_start`: `{type, message:{model,id,type:"message",role:"assistant",content:[],container:null,stop_reason:null,stop_sequence:null,stop_details:null,usage{...},input_transformations:[],diagnostics:null,context_management:null}}`
- `content_block_start`: `{type, index:Int, content_block}` where content_block is:
  - text: `{type:"text", text:""}`
  - thinking: `{type:"thinking", thinking:"", signature:""}`
  - tool_use: `{type:"tool_use", id:"toolu_...", name:"Write", input:{}, caller:{type:"direct"}}` — `caller` is NEW vs contract.
- `content_block_delta`: `{type, index:Int, delta}` where delta.type ∈ `text_delta` (`text:String`), `thinking_delta` (`thinking:String`, `estimated_tokens:?Int`), `input_json_delta` (`partial_json:String` — many events, first has `""`), `signature_delta` (`signature:String`, NEW vs contract).
- `content_block_stop`: `{type, index:Int}`
- `message_delta`: `{type, delta:{stop_reason:"tool_use"|"end_turn", stop_sequence:null, stop_details:null, container:null}, usage{...}, context_management:{applied_edits:[]}}`
- `message_stop`: `{type}`

## assistant

Top level: `{type, message, session_id, uuid, parent_tool_use_id?, timestamp?, error?, request_id?, is_api_error_message?}`.
`message` keys: `container?, content[], context_management?, diagnostics?, id ("msg_..." or UUID for synthetic), input_transformations?, model, role, stop_details?, stop_reason? (null on streamed snapshots — use message_delta), stop_sequence?, type:"message", usage`.
- **One content block per event.** Blocks of one API message share `message.id`; a new API turn (after tool results) has a new id. Do not replace-on-message-id; append blocks.
- Block shapes: `{type:"text", text}`, `{type:"thinking", thinking, signature}`, `{type:"tool_use", id, name, input:JSON}`.
- API-error synthetic variant (capture 06): `model:"<synthetic>"`, `stop_reason:"stop_sequence"`, plus top-level `error:"model_not_found"`, `request_id:"req_..."`, `is_api_error_message:true`, `timestamp:"2026-10-12T00:49:04.869Z"`.

## user (tool_result)

`{type:"user", message:{role:"user", content:[{type:"tool_result", tool_use_id, content:String}], ...}, session_id, uuid}` — `message.id` was null. `content` observed as plain String only (contract's `[{text}]` array form NOT observed — keep both in serde). **`is_error` is ABSENT on success** → `Option<bool>`, not defaulted-true.

## result

Contract fields all present (`subtype, is_error, result, session_id`) plus: `duration_ms, duration_api_ms, total_cost_usd, usage{...}, modelUsage{...}, permission_denials[], terminal_reason ("completed"|"api_error"), num_turns, stop_reason, api_error_status (Int?, e.g. 404), ttft_ms, ttft_stream_ms, time_to_request_ms, first_content_frame_ms, queued_turn_count, result_index, fast_mode_state, fast_mode_disabled_reason, subagent_stats{...}, safety_stops, uuid`.
- **`subtype` was `"success"` even on the model-404 error** (`is_error:true`, `terminal_reason:"api_error"`, `api_error_status:404`). Branch on `is_error`, never on subtype alone.
- `session_id` present in every result observed (still model as optional).

## rate_limit_event (NEW type)

`{type:"rate_limit_event", rate_limit_info:{status, resetsAt:epoch_s, rateLimitType, utilization, overageStatus, overageResetsAt, isUsingOverage, surpassedThreshold, unifiedWindows:{five_hour:{utilization,resetsAt}, seven_day:{...}}, overageEnabled}, uuid, session_id}` — emitted once just before `result`. Parser must skip unknown top-level types.

## Observed ordering (c3, the richest)

init → status(requesting) → message_start → block_start(thinking) → deltas + thinking_tokens → assistant(thinking) → block_stop → block_start(tool_use Write) → input_json_deltas → assistant(Write) → block_stop → block_start(tool_use Read) → input_json_deltas → **user(tool_result Write) MID-STREAM** → more input_json_deltas → assistant(Read) → block_stop → user(tool_result Read) → message_delta(stop_reason:tool_use) → message_stop → status(requesting) → [second API message: text] → rate_limit_event → result.

Tool results interleave with the NEXT block's input_json_delta stream — do not assume user events arrive between messages only.

## Process-level facts

- `claude -p "prompt"` **reads stdin and merges it into the prompt** (observed: trailing script lines on stdin became prompt context). Spawn the child with stdin = /dev/null.
- Invalid model: exit 1; stderr gets one diagnostic line (`[claude-code:unrecognized_model] {"model":"no-such-model","query_source":"sdk"}`); stdout still emits full JSONL (init → synthetic assistant → result).
- `--resume <id> --fork-session`: init carries a NEW session_id (forked), result agrees; no active-writer error against an idle original. Resume must be run from the same cwd as the original session.
- `--effort` choices now `low|medium|high|xhigh|max`; `--permission-mode` choices `acceptEdits|auto|bypassPermissions|manual|dontAsk|plan`; `--allowedTools` has alias `--allowed-tools`.
