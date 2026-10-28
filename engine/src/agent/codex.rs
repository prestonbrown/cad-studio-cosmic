//! The codex runner's pure half: argv construction (with TOML scalar
//! escaping) and the JSONL translator that maps codex's event lines onto
//! the claude-shaped [`StreamEvent`] pipeline the session already drains —
//! transcribed from fixtures/upstream-source/Agent/CodexRunner.swift:19-40
//! (argv + `tomlString`) and :63-171 (`CodexTranslator`). Nothing here
//! spawns a process or reads settings state the caller did not hand it.
//!
//! Translator semantics (CodexRunner.swift:63-171): one codex line may
//! yield ZERO, ONE, or several `StreamEvent`s — `file_change` synthesizes a
//! Write/Edit toolUse+toolResult pair per change, `agent_message` opens a
//! text block then confirms it. The failure paths: `turn.failed` finishes
//! the turn with `error.message` (fallback: the message stashed from an
//! earlier top-level `error` line, then "Codex failed"); a bare top-level
//! `error` line only stashes. Command text arrives wrapped
//! (`/bin/zsh -lc '…'`) and is unwrapped before it reaches the tool row.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Value, json};

use crate::agent::attachments::is_image;
use crate::agent::claude::TurnRequest;
use crate::agent::stream_event::{
    AssistantEvent, ContentBlock, InnerEvent, ResultEvent, StreamEvent, StreamInner, SystemEvent,
    SystemKind, ToolResult, ToolResultContent,
};
use crate::settings::Settings;

/// The argv of one codex turn, in upstream order (CodexRunner.swift:19-40):
/// the exec block with `-C <directory>`, the two `-c` overrides, the
/// optional isolation/model/effort flags, `resume`, one `-i` per IMAGE
/// attachment, then the prompt.
pub fn build_args(
    s: &Settings,
    req: &TurnRequest,
    directory: &Path,
    system_prompt: &str,
    session_id: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "exec".to_string(),
        "--json".to_string(),
        "--skip-git-repo-check".to_string(),
        "--sandbox".to_string(),
        "workspace-write".to_string(),
        "-C".to_string(),
        directory.to_string_lossy().into_owned(),
    ];
    args.push("-c".to_string());
    args.push("approval_policy=\"never\"".to_string());
    args.push("-c".to_string());
    args.push(format!("developer_instructions={}", toml_string(system_prompt)));
    if s.isolates_claude {
        args.push("--ignore-user-config".to_string());
    }
    if let Some(model) = &s.codex_model {
        args.extend(["--model", model.as_str()].map(String::from));
    }
    if let Some(effort) = &s.claude_effort {
        args.push("-c".to_string());
        args.push(format!("model_reasoning_effort={}", toml_string(effort)));
    }
    if let Some(session_id) = session_id {
        args.extend(["resume", session_id].map(String::from));
    }
    for image in &req.images {
        if is_image(image) {
            args.extend(["-i", &image.to_string_lossy()].map(String::from));
        }
    }
    args.push(req.prompt.clone());
    args
}

/// A TOML basic string scalar: quoted, with backslash/quote/newline/carriage
/// return/tab escaped and every other C0 control (plus DEL) as `\uXXXX`
/// (CodexRunner.swift:42-60). Non-ASCII scalars pass through untouched.
fn toml_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for scalar in text.chars() {
        match scalar {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Rust chars are scalar values; the C0 range and DEL are the
            // ones TOML basic strings refuse verbatim
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Per-turn translation state (CodexRunner.swift:63-69): the thread id, the
/// last agent message (turn.completed's result text), the stashed
/// top-level error message, the command ids that already opened a row, and
/// the stable block index per reasoning/message id.
#[derive(Debug, Default)]
pub struct CodexTranslator {
    thread_id: Option<String>,
    last_message: String,
    error_message: Option<String>,
    started: HashSet<String>,
    indexes: HashMap<String, i64>,
}

impl CodexTranslator {
    /// Translate one raw codex stdout line into zero or more stream events
    /// (CodexRunner.swift:70-95). A line that is not a JSON object with a
    /// string `type` yields nothing, like upstream's guard.
    pub fn events(&mut self, line: &str) -> Vec<StreamEvent> {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        let Value::Object(event) = &value else {
            return Vec::new();
        };
        let Some(kind) = event.get("type").and_then(Value::as_str) else {
            return Vec::new();
        };
        match kind {
            "thread.started" => {
                // no thread id → nothing (the guard)
                let Some(id) = event.get("thread_id").and_then(Value::as_str) else {
                    return Vec::new();
                };
                self.thread_id = Some(id.to_string());
                vec![StreamEvent::System(SystemEvent {
                    kind: SystemKind::Init,
                    session_id: Some(id.to_string()),
                    estimated_tokens: None,
                })]
            }
            "item.started" => self.item_started(event.get("item")),
            "item.completed" => self.item_completed(event.get("item")),
            "turn.completed" => vec![self.turn_result(false, self.last_message.clone())],
            "turn.failed" => {
                let message = value
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| self.error_message.clone())
                    .unwrap_or_else(|| "Codex failed".to_string());
                vec![self.turn_result(true, message)]
            }
            "error" => {
                self.error_message = event.get("message").and_then(Value::as_str).map(str::to_string);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The terminal result event both turn endings emit
    /// (CodexRunner.swift:84-88): subtype success/error, the text, and the
    /// thread id as the (engine-blind) session id.
    fn turn_result(&self, is_error: bool, text: String) -> StreamEvent {
        StreamEvent::Result(ResultEvent {
            subtype: if is_error { "error" } else { "success" }.to_string(),
            is_error,
            result: text,
            session_id: self.thread_id.clone(),
            terminal_reason: None,
            api_error_status: None,
        })
    }

    /// The stable block index for `id`, assigning the next sequential one on
    /// first sight (CodexRunner.swift:97-104).
    fn index_for(&mut self, id: &str) -> i64 {
        let next = self.indexes.len() as i64;
        *self.indexes.entry(id.to_string()).or_insert(next)
    }

    /// `item.started` (CodexRunner.swift:106-117): a command execution opens
    /// its Bash row (unwrapping the shell wrapper from the command text);
    /// reasoning opens a thinking block. Upstream pairs the reasoning start
    /// with a bare `.thinking` activity-only event the port's pipeline has
    /// no analog for — the thinking row itself opens here.
    fn item_started(&mut self, item: Option<&Value>) -> Vec<StreamEvent> {
        let Some(item) = item else { return Vec::new() };
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            return Vec::new();
        };
        let Some(kind) = item.get("type").and_then(Value::as_str) else {
            return Vec::new();
        };
        match kind {
            "command_execution" => {
                self.started.insert(id.to_string());
                vec![command_event(id, item)]
            }
            "reasoning" => vec![StreamEvent::Stream {
                event: StreamInner {
                    session_id: None,
                    event: InnerEvent::ContentStart {
                        index: self.index_for(id),
                        block: ContentBlock::Thinking {
                            thinking: String::new(),
                            signature: String::new(),
                        },
                    },
                },
            }],
            _ => Vec::new(),
        }
    }

    /// `item.completed` (CodexRunner.swift:119-152).
    fn item_completed(&mut self, item: Option<&Value>) -> Vec<StreamEvent> {
        let Some(item) = item else { return Vec::new() };
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            return Vec::new();
        };
        let Some(kind) = item.get("type").and_then(Value::as_str) else {
            return Vec::new();
        };
        match kind {
            "command_execution" => {
                // a row item.started never announced opens here first
                let mut events = Vec::new();
                if !self.started.contains(id) {
                    events.push(command_event(id, item));
                }
                let failed = item.get("status").and_then(Value::as_str) == Some("failed")
                    || item.get("exit_code").and_then(Value::as_f64).unwrap_or(0.0) != 0.0;
                let output = item
                    .get("aggregated_output")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                events.push(StreamEvent::UserToolResult(ToolResult {
                    tool_use_id: id.to_string(),
                    content: ToolResultContent::Text(output),
                    is_error: Some(failed),
                }));
                events
            }
            "file_change" => {
                let failed = item.get("status").and_then(Value::as_str) == Some("failed");
                let changes = item
                    .get("changes")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut events = Vec::new();
                for (offset, change) in changes.iter().enumerate() {
                    let tool_id = format!("{id}-{offset}");
                    let path = change.get("path").and_then(Value::as_str).unwrap_or_default();
                    let name = if change.get("kind").and_then(Value::as_str) == Some("add") {
                        "Write"
                    } else {
                        "Edit"
                    };
                    events.push(StreamEvent::Assistant(AssistantEvent {
                        session_id: None,
                        block: ContentBlock::ToolUse {
                            id: tool_id.clone(),
                            name: name.to_string(),
                            input: json!({ "file_path": path }),
                        },
                    }));
                    events.push(StreamEvent::UserToolResult(ToolResult {
                        tool_use_id: tool_id,
                        content: ToolResultContent::Text(String::new()),
                        is_error: Some(failed),
                    }));
                }
                events
            }
            "agent_message" => {
                let Some(text) = item.get("text").and_then(Value::as_str) else {
                    return Vec::new();
                };
                if text.is_empty() {
                    return Vec::new();
                }
                self.last_message = text.to_string();
                vec![
                    StreamEvent::Stream {
                        event: StreamInner {
                            session_id: None,
                            event: InnerEvent::ContentStart {
                                index: self.index_for(id),
                                block: ContentBlock::Text { text: String::new() },
                            },
                        },
                    },
                    StreamEvent::Assistant(AssistantEvent {
                        session_id: None,
                        block: ContentBlock::Text { text: text.to_string() },
                    }),
                ]
            }
            "reasoning" => vec![StreamEvent::Stream {
                event: StreamInner {
                    session_id: None,
                    event: InnerEvent::ContentStop {
                        index: self.index_for(id),
                    },
                },
            }],
            "error" => {
                self.error_message =
                    item.get("message").and_then(Value::as_str).map(str::to_string);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

/// The Bash toolUse event a command execution opens
/// (CodexRunner.swift:154-156): the command unwrapped from its shell
/// wrapper, as the row's input JSON.
fn command_event(id: &str, item: &Value) -> StreamEvent {
    let command = unwrapped(item.get("command").and_then(Value::as_str).unwrap_or_default());
    StreamEvent::Assistant(AssistantEvent {
        session_id: None,
        block: ContentBlock::ToolUse {
            id: id.to_string(),
            name: "Bash".to_string(),
            input: json!({ "command": command }),
        },
    })
}

/// Strip the shell wrapper codex wraps commands in
/// (CodexRunner.swift:158-170): a known `-lc` prefix, then one layer of
/// matching quotes; double-quoted inner text additionally unescapes `\"`
/// and `\$`.
fn unwrapped(command: &str) -> String {
    for prefix in ["/bin/zsh -lc ", "/bin/bash -lc ", "bash -lc ", "zsh -lc "] {
        let Some(mut inner) = command.strip_prefix(prefix) else {
            continue;
        };
        let first = inner.chars().next();
        let quoted = matches!(first, Some('\'' | '"'))
            && inner.chars().next_back() == first
            && inner.chars().count() > 1;
        if quoted {
            inner = &inner[1..inner.len() - 1];
            if first == Some('"') {
                return inner.replace("\\\"", "\"").replace("\\$", "$");
            }
        }
        return inner.to_string();
    }
    command.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── toml_string (CodexRunner.swift:42-60) ─────────────────────────────

    #[test]
    fn toml_plain_is_quoted_verbatim() {
        assert_eq!(toml_string("abc"), "\"abc\"");
        assert_eq!(toml_string(""), "\"\"");
    }

    #[test]
    fn toml_escapes_the_five_named_scalars() {
        assert_eq!(toml_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(toml_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(toml_string("a\nb"), "\"a\\nb\"");
        assert_eq!(toml_string("a\rb"), "\"a\\rb\"");
        assert_eq!(toml_string("a\tb"), "\"a\\tb\"");
    }

    #[test]
    fn toml_escapes_c0_and_del_as_four_hex_uppercase() {
        assert_eq!(toml_string("a\u{1}b"), "\"a\\u0001b\"");
        assert_eq!(toml_string("a\u{1f}b"), "\"a\\u001Fb\"", "uppercase hex");
        assert_eq!(toml_string("a\u{7f}b"), "\"a\\u007Fb\"", "DEL escapes too");
    }

    #[test]
    fn toml_passes_non_ascii_through() {
        assert_eq!(toml_string("piñata — ✓"), "\"piñata — ✓\"");
    }

    // ─── unwrapped (CodexRunner.swift:158-170) ─────────────────────────────

    #[test]
    fn unwrapped_strips_each_shell_prefix() {
        for prefix in ["/bin/zsh -lc ", "/bin/bash -lc ", "bash -lc ", "zsh -lc "] {
            assert_eq!(unwrapped(&format!("{prefix}ls -la")), "ls -la", "{prefix}");
        }
    }

    #[test]
    fn unwrapped_strips_one_layer_of_single_quotes() {
        assert_eq!(unwrapped("/bin/zsh -lc 'cd /tmp && ls'"), "cd /tmp && ls");
        // single-quoted inner text is NOT unescaped
        assert_eq!(unwrapped("/bin/zsh -lc 'echo \"x\"'"), "echo \"x\"");
    }

    #[test]
    fn unwrapped_strips_double_quotes_and_unescapes() {
        assert_eq!(unwrapped("/bin/zsh -lc \"echo \\\"hi\\\"\""), "echo \"hi\"");
        assert_eq!(unwrapped("/bin/zsh -lc \"a\\$b\""), "a$b");
    }

    #[test]
    fn unwrapped_without_quotes_or_prefix() {
        // prefix matched but nothing quoted: inner text as-is
        assert_eq!(unwrapped("/bin/zsh -lc ls"), "ls");
        // a lone quote character is not a wrapper (count > 1 guard)
        assert_eq!(unwrapped("/bin/zsh -lc '"), "'");
        // first char quotes but the last does not match: no strip, and the
        // prefix is still consumed (upstream drops it before the test)
        assert_eq!(unwrapped("/bin/zsh -lc 'echo"), "'echo");
        // no known prefix: untouched
        assert_eq!(unwrapped("echo hello"), "echo hello");
        assert_eq!(unwrapped("/bin/zsh -c 'ls'"), "/bin/zsh -c 'ls'");
    }

    // ─── the translator, from recorded codex lines ─────────────────────────

    fn started(id: &str, command: &str) -> StreamEvent {
        StreamEvent::Assistant(AssistantEvent {
            session_id: None,
            block: ContentBlock::ToolUse {
                id: id.into(),
                name: "Bash".into(),
                input: json!({ "command": command }),
            },
        })
    }

    fn tool_finished(id: &str, output: &str, failed: bool) -> StreamEvent {
        StreamEvent::UserToolResult(ToolResult {
            tool_use_id: id.into(),
            content: ToolResultContent::Text(output.into()),
            is_error: Some(failed),
        })
    }

    fn thinking_started(index: i64) -> StreamEvent {
        StreamEvent::Stream {
            event: StreamInner {
                session_id: None,
                event: InnerEvent::ContentStart {
                    index,
                    block: ContentBlock::Thinking {
                        thinking: String::new(),
                        signature: String::new(),
                    },
                },
            },
        }
    }

    fn block_stopped(index: i64) -> StreamEvent {
        StreamEvent::Stream {
            event: StreamInner {
                session_id: None,
                event: InnerEvent::ContentStop { index },
            },
        }
    }

    fn text_started(index: i64) -> StreamEvent {
        StreamEvent::Stream {
            event: StreamInner {
                session_id: None,
                event: InnerEvent::ContentStart {
                    index,
                    block: ContentBlock::Text { text: String::new() },
                },
            },
        }
    }

    fn text_confirmed(text: &str) -> StreamEvent {
        StreamEvent::Assistant(AssistantEvent {
            session_id: None,
            block: ContentBlock::Text { text: text.into() },
        })
    }

    fn result(subtype: &str, is_error: bool, text: &str, session_id: Option<&str>) -> StreamEvent {
        StreamEvent::Result(ResultEvent {
            subtype: subtype.into(),
            is_error,
            result: text.into(),
            session_id: session_id.map(str::to_string),
            terminal_reason: None,
            api_error_status: None,
        })
    }

    /// thread.started → the started event carrying the thread id, which the
    /// session persists as the (engine-blind) session id.
    #[test]
    fn thread_started_emits_started_with_thread_id() {
        let mut t = CodexTranslator::default();
        assert_eq!(
            t.events(r#"{"type":"thread.started","thread_id":"th_123"}"#),
            vec![StreamEvent::System(SystemEvent {
                kind: SystemKind::Init,
                session_id: Some("th_123".into()),
                estimated_tokens: None,
            })]
        );
        // no thread_id → nothing (the guard)
        assert!(t
            .events(r#"{"type":"thread.started"}"#)
            .is_empty());
    }

    /// item.started command_execution opens the Bash row with the command
    /// unwrapped; item.started reasoning opens a thinking block at the id's
    /// assigned index.
    #[test]
    fn item_started_command_and_reasoning() {
        let mut t = CodexTranslator::default();
        assert_eq!(
            t.events(
                r#"{"type":"item.started","item":{"id":"cmd_1","type":"command_execution","command":"/bin/zsh -lc 'cd /tmp && ls'"}}"#
            ),
            vec![started("cmd_1", "cd /tmp && ls")]
        );
        assert_eq!(
            t.events(r#"{"type":"item.started","item":{"id":"rs_1","type":"reasoning"}}"#),
            vec![thinking_started(0)]
        );
        // a second reasoning id takes the NEXT sequential index
        assert_eq!(
            t.events(r#"{"type":"item.started","item":{"id":"rs_2","type":"reasoning"}}"#),
            vec![thinking_started(1)]
        );
        // unknown item types are dropped silently
        assert!(t
            .events(r#"{"type":"item.started","item":{"id":"x","type":"web_search"}}"#)
            .is_empty());
    }

    /// item.completed command_execution closes the row: output from
    /// aggregated_output, failed when status says failed OR the exit code is
    /// nonzero. A row item.started never announced opens here first.
    #[test]
    fn command_completion_closes_the_row() {
        let mut t = CodexTranslator::default();
        t.events(r#"{"type":"item.started","item":{"id":"cmd_1","type":"command_execution","command":"ls"}}"#);
        assert_eq!(
            t.events(
                r#"{"type":"item.completed","item":{"id":"cmd_1","type":"command_execution","command":"ls","status":"completed","exit_code":0,"aggregated_output":"file1\nfile2"}}"#
            ),
            vec![tool_finished("cmd_1", "file1\nfile2", false)]
        );

        // never started: the row opens at completion, then closes
        let mut t = CodexTranslator::default();
        assert_eq!(
            t.events(
                r#"{"type":"item.completed","item":{"id":"cmd_2","type":"command_execution","command":"/bin/zsh -lc 'make'","status":"failed","aggregated_output":"boom"}}"#
            ),
            vec![
                started("cmd_2", "make"),
                tool_finished("cmd_2", "boom", true),
            ]
        );

        // nonzero exit code alone marks the row failed
        let mut t = CodexTranslator::default();
        t.events(r#"{"type":"item.started","item":{"id":"cmd_3","type":"command_execution","command":"ls"}}"#);
        assert_eq!(
            t.events(
                r#"{"type":"item.completed","item":{"id":"cmd_3","type":"command_execution","command":"ls","status":"completed","exit_code":2,"aggregated_output":""}}"#
            ),
            vec![tool_finished("cmd_3", "", true)]
        );
    }

    /// file_change synthesizes a Write/Edit toolUse + toolResult pair per
    /// change, ids `<item id>-<offset>`, name from the change kind
    /// (CodexRunner.swift:126-138).
    #[test]
    fn file_change_synthesizes_write_edit_pairs() {
        let mut t = CodexTranslator::default();
        assert_eq!(
            t.events(
                r#"{"type":"item.completed","item":{"id":"fc_1","type":"file_change","status":"completed","changes":[{"path":"/p/model.py","kind":"add"},{"path":"/p/helper.py","kind":"edit"}]}}"#
            ),
            vec![
                StreamEvent::Assistant(AssistantEvent {
                    session_id: None,
                    block: ContentBlock::ToolUse {
                        id: "fc_1-0".into(),
                        name: "Write".into(),
                        input: json!({ "file_path": "/p/model.py" }),
                    },
                }),
                tool_finished("fc_1-0", "", false),
                StreamEvent::Assistant(AssistantEvent {
                    session_id: None,
                    block: ContentBlock::ToolUse {
                        id: "fc_1-1".into(),
                        name: "Edit".into(),
                        input: json!({ "file_path": "/p/helper.py" }),
                    },
                }),
                tool_finished("fc_1-1", "", false),
            ]
        );
        // a failed file_change fails every synthesized row
        let mut t = CodexTranslator::default();
        let events = t.events(
            r#"{"type":"item.completed","item":{"id":"fc_2","type":"file_change","status":"failed","changes":[{"path":"/p/model.py","kind":"add"}]}}"#,
        );
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[1], StreamEvent::UserToolResult(r) if r.is_error == Some(true)));
    }

    /// agent_message opens a text block at the id's index and confirms the
    /// text; an empty message emits nothing (CodexRunner.swift:139-143).
    #[test]
    fn agent_message_opens_and_confirms_text() {
        let mut t = CodexTranslator::default();
        assert_eq!(
            t.events(r#"{"type":"item.completed","item":{"id":"msg_1","type":"agent_message","text":"I made a box."}}"#),
            vec![text_started(0), text_confirmed("I made a box.")]
        );
        assert!(t
            .events(r#"{"type":"item.completed","item":{"id":"msg_2","type":"agent_message","text":""}}"#)
            .is_empty());
    }

    /// item.completed reasoning closes the block its start opened — the
    /// SAME index, because both halves key on the id.
    #[test]
    fn reasoning_completion_stops_the_block() {
        let mut t = CodexTranslator::default();
        t.events(r#"{"type":"item.started","item":{"id":"rs_1","type":"reasoning"}}"#);
        assert_eq!(
            t.events(r#"{"type":"item.completed","item":{"id":"rs_1","type":"reasoning"}}"#),
            vec![block_stopped(0)]
        );
    }

    /// turn.completed finishes with the LAST agent message as the result
    /// text, success, and the thread id as the session id.
    #[test]
    fn turn_completed_finishes_with_last_message() {
        let mut t = CodexTranslator::default();
        t.events(r#"{"type":"thread.started","thread_id":"th_9"}"#);
        t.events(r#"{"type":"item.completed","item":{"id":"m","type":"agent_message","text":"first"}}"#);
        t.events(r#"{"type":"item.completed","item":{"id":"m2","type":"agent_message","text":"final answer"}}"#);
        assert_eq!(
            t.events(r#"{"type":"turn.completed","usage":{"tokens":5}}"#),
            vec![result("success", false, "final answer", Some("th_9"))]
        );
    }

    /// turn.failed finishes errored with error.message, falling back to the
    /// message stashed by an earlier top-level `error` line, then to
    /// "Codex failed" (CodexRunner.swift:86-91).
    #[test]
    fn turn_failed_uses_inline_then_stashed_then_default_message() {
        // inline error wins over a stash
        let mut t = CodexTranslator::default();
        t.events(r#"{"type":"error","message":"stashed earlier"}"#);
        t.events(r#"{"type":"thread.started","thread_id":"th_1"}"#);
        assert_eq!(
            t.events(r#"{"type":"turn.failed","error":{"message":"stream dropped"}}"#),
            vec![result("error", true, "stream dropped", Some("th_1"))]
        );

        // no inline error: the stash
        let mut t = CodexTranslator::default();
        t.events(r#"{"type":"error","message":"experimental api"}"#);
        assert_eq!(
            t.events(r#"{"type":"turn.failed"}"#),
            vec![result("error", true, "experimental api", None)]
        );

        // neither: the fixed fallback
        let mut t = CodexTranslator::default();
        assert_eq!(
            t.events(r#"{"type":"turn.failed"}"#),
            vec![result("error", true, "Codex failed", None)]
        );
    }

    /// A top-level `error` line only stashes (zero events); an item-level
    /// error inside item.completed stashes too. Non-JSON lines and objects
    /// without a string type yield nothing.
    #[test]
    fn error_lines_only_stash_and_noise_yields_nothing() {
        let mut t = CodexTranslator::default();
        assert!(t.events(r#"{"type":"error","message":"m1"}"#).is_empty());
        assert!(t
            .events(r#"{"type":"item.completed","item":{"id":"e","type":"error","message":"m2"}}"#)
            .is_empty());
        // both stashes are visible through a later bare turn.failed — the
        // LAST stash wins (it overwrote m1)
        assert_eq!(
            t.events(r#"{"type":"turn.failed"}"#),
            vec![result("error", true, "m2", None)]
        );

        assert!(t.events("not json at all").is_empty());
        assert!(t.events(r#"[1,2]"#).is_empty());
        assert!(t.events(r#"{"no_type":true}"#).is_empty());
        assert!(t.events(r#"{"type":42}"#).is_empty());
    }

    // ─── build_args (CodexRunner.swift:19-40) ──────────────────────────────

    fn req(images: &[&str]) -> TurnRequest {
        TurnRequest {
            prompt: "make a box".into(),
            attachments: Vec::new(),
            images: images.iter().map(Path::new).map(Path::to_path_buf).collect(),
        }
    }

    /// The whole argv, in upstream order, for the default flag combination.
    #[test]
    fn argv_default_full_vector() {
        let s = Settings::default(); // isolates_claude = true
        let args = build_args(&s, &req(&[]), Path::new("/proj"), "SYS", None);
        assert_eq!(
            args,
            [
                "exec",
                "--json",
                "--skip-git-repo-check",
                "--sandbox",
                "workspace-write",
                "-C",
                "/proj",
                "-c",
                "approval_policy=\"never\"",
                "-c",
                "developer_instructions=\"SYS\"",
                "--ignore-user-config",
                "make a box",
            ]
        );
    }

    #[test]
    fn argv_optional_flags_in_upstream_order() {
        let mut s = Settings::default();
        s.isolates_claude = false;
        s.codex_model = Some("gpt-5.2-codex".into());
        s.claude_effort = Some("high".into());
        let args = build_args(
            &s,
            &req(&["/p/shot.png", "/p/notes.txt"]),
            Path::new("/proj"),
            "line1\nline2",
            Some("th_7"),
        );
        assert_eq!(
            args,
            [
                "exec",
                "--json",
                "--skip-git-repo-check",
                "--sandbox",
                "workspace-write",
                "-C",
                "/proj",
                "-c",
                "approval_policy=\"never\"",
                "-c",
                "developer_instructions=\"line1\\nline2\"",
                "--model",
                "gpt-5.2-codex",
                "-c",
                "model_reasoning_effort=\"high\"",
                "resume",
                "th_7",
                "-i",
                "/p/shot.png",
                "make a box",
            ]
        );
    }

    /// Claude's argv consumes NO images (Ruling 2 / upstream parity) — the
    /// filter is codex's `-i` arm alone, and it skips non-image attachments.
    #[test]
    fn argv_images_filter_is_image_only() {
        let s = Settings::default();
        let args = build_args(
            &s,
            &req(&["/p/a.webp", "/p/b.PNG", "/p/c.pdf"]),
            Path::new("/proj"),
            "SYS",
            None,
        );
        // exactly the two images ride -i, in request order; the pdf never does
        let paths: Vec<&str> = args
            .windows(2)
            .filter(|w| w[0] == "-i")
            .map(|w| w[1].as_str())
            .collect();
        assert_eq!(paths, ["/p/a.webp", "/p/b.PNG"], "{args:?}");
    }
}
