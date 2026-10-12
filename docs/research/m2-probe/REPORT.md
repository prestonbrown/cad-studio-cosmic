# M2 CLI probe report — Claude Code stream-json on thelio

Date: 2026-10-11. Host: thelio (Linux), user pbrown, claude 2.1.296 (00-version.txt),
invoked via ssh thelio bash -lc. Budget honored: exactly 4 claude session invocations
(captures 2, 3, 5, 6), zero retries. No repo writes, no cargo/build tooling run.

Quirk that cost one plumbing attempt (no extra invocation): piping a script into remote
bash -l -s fails because claude -p READS STDIN AND MERGES IT INTO THE PROMPT — the script
tail after the claude line was consumed as prompt text. The sessions ran fine; only stdout
retrieval was lost, recovered by cat-ing the remote files. M2 lesson: spawn the agent
child with stdin = /dev/null.

## Captures

| # | File | Command essence |
|---|---|---|
| 1 | 01-help.txt, 01-p-help.txt (diff = usage line + Commands section only) | claude --help, claude -p --help |
| 2 | 02-minimal-session.jsonl (19 lines) | contract §1 exact flag set, prompt "Reply with exactly the word: pong" |
| 3 | 03-tool-use.jsonl (66 lines) | same flags, "Create a file named probe.txt ... then read it back" (Write + Read both fired) |
| 4 | 04-shapes.md | field-level digest, jq-extracted from 2+3 (full detail there) |
| 5 | 05-resume-fork.jsonl (12 lines) | --resume <c2 sid> --fork-session "Reply with exactly: pong2" --output-format stream-json --verbose |
| 6 | 06-error.jsonl + 06-stderr.txt + 06-exit.txt (exit 1); sanitized 06-error-minimal.jsonl | --model no-such-model |

Remote dirs (resume needs same cwd): c2/c5 = /tmp/m2probe2.0DOol9OM (sid
94fcba3f-44f9-4003-b32d-c469499dc529, fork 283df95a-9a62-4620-9a04-73d1e3209968),
c3 = /tmp/m2probe3.x7PUMBvS (sid bdd2e831-7c7c-422f-b50a-fc22b182bae3).

### Key observations per capture

- 2 (minimal): init -> status(requesting) -> message_start -> thinking block (EMPTY text,
  signature only; token counts via system/thinking_tokens) -> text block -> message_delta/
  stop -> rate_limit_event -> result("pong", is_error:false).
- 3 (tool-use): thinking + tool_use(Write) + tool_use(Read) in ONE API message; each
  assistant event carries ONE content block (same message.id); user tool_result for Write
  arrived MID-STREAM, interleaved with Reads input_json_deltas; second API message = final
  text. tool_result.content was plain String; is_error ABSENT on success.
- 5 (resume/fork): forked init carries a NEW session_id (!= resumed id); result agrees;
  exit 0; no active-writer error. Without isolation flags, user plugins/hooks fired (8 of
  12 lines hook_started/hook_response; permissionMode defaulted to "auto").
- 6 (error): exit 1; stderr = one diagnostic line "[claude-code:unrecognized_model] {...}";
  stdout still emits full JSONL: init (model echoes bad name) -> SYNTHETIC assistant
  (model:"<synthetic>", top-level error:"model_not_found", request_id,
  is_api_error_message:true, timestamp) -> result with subtype:"success" BUT is_error:true,
  terminal_reason:"api_error", api_error_status:404.

## DELTAS vs upstream-contract.md §1

Flags: NONE missing — every §1 flag exists in 2.1.296. Notes: --effort choices now
low|medium|high|xhigh|max (superset); --permission-mode choices
acceptEdits|auto|bypassPermissions|manual|dontAsk|plan; --allowedTools aliases
--allowed-tools; newer flags: --include-hook-events, --forward-subagent-text,
--session-id, --input-format stream-json.

Event-shape deltas (detail in 04-shapes.md):
1. NEW top-level type rate_limit_event (just before result) — parser must skip unknown types.
2. NEW system subtypes: status ("requesting", also between turns) and, when user settings
   NOT isolated, hook_started/hook_response.
3. system/thinking_tokens adds estimated_tokens_delta.
4. assistant events carry ONE content block each, NOT a cumulative array; message has many
   extra keys (container, diagnostics, context_management, input_transformations,
   stop_details); message.stop_reason null on streamed snapshots (stop_reason lives in
   message_delta); synthetic error variant adds timestamp/error/request_id/is_api_error_message.
5. content_block_start.tool_use adds caller:{type:"direct"}.
6. NEW delta types: signature_delta; thinking_delta carries nullable estimated_tokens.
   Thinking TEXT arrived empty (signature only) — token counts come via system events.
7. tool_result.is_error absent on success -> Option<bool>. content observed only as String
   (keep contract [{text}] variant in serde; unobserved).
8. result: subtype stays "success" on the API-error turn — error detection must use
   is_error (+ terminal_reason, api_error_status), never subtype alone. Many new fields
   (usage, modelUsage, ttft_ms, num_turns, permission_denials, ...).
9. stream_event wrapper adds session_id, parent_tool_use_id (null), uuid, ttft_ms,
   thinking_display, api_message_id.
10. Process semantics: -p merges stdin into the prompt (child needs stdin=/dev/null);
    resume must run in the original session cwd; fork swaps in a new session_id.

## CORPUS RECOMMENDATION

Fixture files as-is: 02-minimal-session.jsonl (thinking+text, no tools),
03-tool-use.jsonl (tool_use + interleaved tool_result + multi-turn — the serde torture
test), 06-error-minimal.jsonl (sanitized init -> synthetic error assistant -> error
result; use this, NOT raw 06-error.jsonl, which embeds Prestons plugin/skill inventory).

05-resume-fork.jsonl: strip the 8 hook lines before use (jq: select(.type!="system" or
(.subtype|startswith("hook_")|not))) -> leaves init/assistant/result with the NEW forked
session_id to assert on.

Adversarial sequences (files 07-09 are first cuts, SYNTHETIC wording — real 2.1.296
wording NOT captured, out of budget):
- busy-session: "already has an active writer" as both a result JSONL
  (07-busy-session.jsonl) and stderr sidecar (07-busy-session-stderr.txt); classifier must
  scan COMBINED stdout+stderr (upstream does).
- missing-session: 08-missing-session.txt ("No conversation found with session ID: ...");
  match via upstream substring list ("No conversation found" / "no rollout found" /
  "session not found" / "thread not found"), never exact equality.
- auth-error: 09-auth-error.txt (case-insensitive: "Invalid API key", "/login",
  "401 unauthorized", "authentication").
- Also synthesize in M2 tests (no file yet): tool_result is_error:true + String content;
  tool_result.content as [{"type":"text","text":...}] array (contract form, unobserved);
  redacted_thinking block (contract handles it, unobserved in 2.1.296); unknown top-level
  type line (forward-compat); --output-format json single-object fallback (not captured —
  needs its own probe if M2 supports the streamingOutput=false setting).

Serde guidance for M2: no deny_unknown_fields anywhere; every enum needs an
#[serde(other)]/fallback variant; result.subtype and system.subtype are open sets.

Follow-up probes worth 1 invocation each if ever needed: bogus --resume <id> (real
missing-session wording), second concurrent writer on one session (real busy wording),
--output-format json fallback.
