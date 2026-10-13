//! Stream-event decoding for agent JSONL stdout: one [`parse_line`] call per
//! line of the agent's `--output-format stream-json` output.
//!
//! The shapes are the Claude Code 2.1.296 probe corpus
//! (`docs/research/m2-probe/04-shapes.md`). Every set the emitter treats as
//! open is modeled as open here: unknown fields are ignored (nothing uses
//! `deny_unknown_fields`), unknown `type`/`subtype` strings land in the
//! [`StreamEvent::Unknown`] / `Other` fallbacks carrying the observed string,
//! and a line whose known type has a payload that no longer fits degrades to
//! `Unknown(type)` rather than failing. [`parse_line`] returns `None` only
//! for lines that are not a JSON object with a string `type` field — process
//! noise like blank lines or stray diagnostics — which the caller logs and
//! skips. [`parse_line_strict`] surfaces the underlying serde error instead
//! of collapsing it to `None`.

use serde::Deserialize;
use serde_json::Value;

/// One decoded line of the agent's JSONL event stream.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// `system` line: init/status/thinking_tokens/hook_* bookkeeping.
    System(SystemEvent),
    /// `stream_event` wrapper around a partial-message API event.
    Stream { event: StreamInner },
    /// `assistant` snapshot carrying exactly one content block per event.
    Assistant(AssistantEvent),
    /// `user` line carrying one tool result.
    UserToolResult(ToolResult),
    /// Terminal `result` line, one per turn.
    Result(ResultEvent),
    /// `rate_limit_event`: decoded enough to not be an error, then skipped.
    RateLimit,
    /// Any other top-level `type` string (forward compatibility).
    Unknown(String),
}

/// The decoded payload of a `stream_event` line, with the wrapper's session
/// id kept for routing.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamInner {
    pub session_id: Option<String>,
    pub event: InnerEvent,
}

/// `stream_event.event` — an open set, unmapped types land in
/// [`InnerEvent::Other`] with the observed string.
#[derive(Debug, Clone, PartialEq)]
pub enum InnerEvent {
    MessageStart,
    ContentStart {
        index: i64,
        block: ContentBlock,
    },
    ContentDelta {
        index: i64,
        delta: Delta,
    },
    ContentStop {
        index: i64,
    },
    /// `message_delta` carries the message's final `stop_reason`
    /// (`end_turn`, `tool_use`, ...); streamed assistant snapshots leave it
    /// null, so this is where the turn outcome is learned.
    MessageDelta {
        stop_reason: Option<String>,
    },
    MessageStop,
    Other(String),
}

/// `system` line; `estimated_tokens` is set only on `thinking_tokens`.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemEvent {
    pub kind: SystemKind,
    pub session_id: Option<String>,
    pub estimated_tokens: Option<i64>,
}

/// `system.subtype` — an open set; `hook_*` fire only when user settings
/// load.
#[derive(Debug, Clone, PartialEq)]
pub enum SystemKind {
    Init,
    Status,
    ThinkingTokens,
    HookStarted,
    HookResponse,
    Other(String),
}

impl SystemKind {
    /// Maps a `subtype` string; anything unmapped stays itself in
    /// [`SystemKind::Other`].
    fn from_subtype(subtype: &str) -> SystemKind {
        match subtype {
            "init" => SystemKind::Init,
            "status" => SystemKind::Status,
            "thinking_tokens" => SystemKind::ThinkingTokens,
            "hook_started" => SystemKind::HookStarted,
            "hook_response" => SystemKind::HookResponse,
            other => SystemKind::Other(other.to_string()),
        }
    }
}

/// `assistant` line. Each event carries ONE content block (blocks of one API
/// message share `message.id` and arrive as separate events — append, never
/// replace on message id).
#[derive(Debug, Clone, PartialEq)]
pub struct AssistantEvent {
    pub session_id: Option<String>,
    pub block: ContentBlock,
}

/// One content block, from an `assistant` message or a
/// `content_block_start`.
#[derive(Debug, Clone, PartialEq)]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    /// `redacted_thinking`; the encrypted `data` needs no consumer.
    RedactedThinking,
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

/// `content_block_delta` payload — an open set with a unit fallback. The
/// thinking-token estimate rides `system/thinking_tokens`, not
/// `thinking_delta`, so it is not carried here.
#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    Text { text: String },
    Thinking { thinking: String },
    InputJson { partial_json: String },
    Signature { signature: String },
    Other,
}

/// `user`-line tool result. `is_error` is ABSENT on success (null is not
/// sent), so it stays `Option` — a `false` default would erase that
/// distinction.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: ToolResultContent,
    pub is_error: Option<bool>,
}

/// `tool_result.content`: a plain string (the observed form) or the API's
/// block array; anything else degrades to [`ToolResultContent::Other`]
/// without failing the line.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolResultContent {
    Text(String),
    Blocks(Vec<ToolResultBlock>),
    Other,
}

/// One `{type, text}` entry of a block-array tool result.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResultBlock {
    /// The block's `type` field (e.g. `text`).
    pub kind: String,
    pub text: String,
}

/// Terminal `result` line. `subtype` stays `"success"` on errors — branch on
/// `is_error` / `terminal_reason` / `api_error_status`, never on subtype
/// alone.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ResultEvent {
    pub subtype: String,
    #[serde(default)]
    pub is_error: bool,
    pub result: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub terminal_reason: Option<String>,
    #[serde(default)]
    pub api_error_status: Option<i64>,
}

/// Decodes one JSONL line. `None` means "not an event" (process noise) —
/// log and skip it.
pub fn parse_line(line: &str) -> Option<StreamEvent> {
    parse_line_strict(line).ok()
}

/// Like [`parse_line`], surfacing the serde error instead of collapsing it
/// to `None`.
pub fn parse_line_strict(line: &str) -> Result<StreamEvent, serde_json::Error> {
    serde_json::from_str::<StreamEventRaw>(line).map(StreamEventRaw::into_event)
}

// ─── serde layer ───────────────────────────────────────────────────────────
//
// Raw structs are flat super-sets of the observed shapes with optional
// fields; the mapping below selects members by the discriminator string.
// Nothing rejects unknown fields, so a new field from a newer agent build
// flows through untouched.

/// Two-stage line decode: a known top-level type takes the typed path; any
/// other JSON object with a string `type` decodes as
/// [`StreamEvent::Unknown`].
#[derive(Deserialize)]
#[serde(untagged)]
enum StreamEventRaw {
    Typed(TypedLineRaw),
    TypeOnly(TypeTag),
}

/// The `type` field alone, for unknown top-level types.
#[derive(Deserialize)]
struct TypeTag {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum TypedLineRaw {
    #[serde(rename = "system")]
    System(SystemRaw),
    #[serde(rename = "stream_event")]
    Stream(StreamLineRaw),
    #[serde(rename = "assistant")]
    Assistant(AssistantRaw),
    #[serde(rename = "user")]
    User(UserRaw),
    #[serde(rename = "result")]
    Result(ResultEvent),
    /// Payload (`rate_limit_info`) has no consumer yet.
    #[serde(rename = "rate_limit_event")]
    RateLimit,
}

#[derive(Deserialize)]
struct SystemRaw {
    #[serde(default)]
    subtype: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    estimated_tokens: Option<i64>,
}

#[derive(Deserialize)]
struct StreamLineRaw {
    event: InnerEventRaw,
    #[serde(default)]
    session_id: Option<String>,
}

/// Same two-stage decode as [`StreamEventRaw`], one level down: unknown
/// `event.type` strings survive as [`InnerEvent::Other`].
#[derive(Deserialize)]
#[serde(untagged)]
enum InnerEventRaw {
    Typed(TypedInnerRaw),
    TypeOnly(TypeTag),
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum TypedInnerRaw {
    #[serde(rename = "message_start")]
    MessageStart,
    #[serde(rename = "content_block_start")]
    ContentStart {
        #[serde(default)]
        index: i64,
        content_block: BlockRaw,
    },
    #[serde(rename = "content_block_delta")]
    ContentDelta {
        #[serde(default)]
        index: i64,
        delta: DeltaRaw,
    },
    #[serde(rename = "content_block_stop")]
    ContentStop {
        #[serde(default)]
        index: i64,
    },
    #[serde(rename = "message_delta")]
    MessageDelta {
        #[serde(default)]
        delta: Option<StopDeltaRaw>,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
}

/// The nested `delta` of `message_delta` (`stop_reason` lives here, not on
/// the event).
#[derive(Deserialize)]
struct StopDeltaRaw {
    #[serde(default)]
    stop_reason: Option<String>,
}

/// Flat super-set of every observed content-block shape; `type` selects
/// which members are meaningful (`redacted_thinking`'s `data` is dropped).
#[derive(Deserialize)]
struct BlockRaw {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<Value>,
}

/// Flat super-set of every observed delta shape; `type` selects the member.
#[derive(Deserialize)]
struct DeltaRaw {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
    #[serde(default)]
    signature: Option<String>,
}

#[derive(Deserialize)]
struct AssistantRaw {
    message: AssistantMessageRaw,
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct AssistantMessageRaw {
    content: Vec<BlockRaw>,
}

#[derive(Deserialize)]
struct UserRaw {
    message: UserMessageRaw,
}

#[derive(Deserialize)]
struct UserMessageRaw {
    content: Vec<ToolResultRaw>,
}

/// One `message.content` item of a `user` line. `tool_use_id` is optional at
/// this layer so non-tool-result siblings parse; the mapping below skips
/// them.
#[derive(Deserialize)]
struct ToolResultRaw {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    tool_use_id: Option<String>,
    #[serde(default)]
    content: Value,
    #[serde(default)]
    is_error: Option<bool>,
}

// ─── mapping layer ─────────────────────────────────────────────────────────

impl StreamEventRaw {
    fn into_event(self) -> StreamEvent {
        match self {
            StreamEventRaw::Typed(TypedLineRaw::System(raw)) => StreamEvent::System(SystemEvent {
                kind: SystemKind::from_subtype(&raw.subtype),
                session_id: raw.session_id,
                estimated_tokens: raw.estimated_tokens,
            }),
            StreamEventRaw::Typed(TypedLineRaw::Stream(raw)) => StreamEvent::Stream {
                event: raw.into_inner(),
            },
            StreamEventRaw::Typed(TypedLineRaw::Assistant(raw)) => {
                match raw.message.content.first().and_then(ContentBlock::from_raw) {
                    Some(block) => StreamEvent::Assistant(AssistantEvent {
                        session_id: raw.session_id,
                        block,
                    }),
                    // An assistant line with no mappable block (empty content
                    // or a block type this build does not know) is noise from
                    // a newer agent, not a decode failure.
                    None => StreamEvent::Unknown("assistant".into()),
                }
            }
            StreamEventRaw::Typed(TypedLineRaw::User(raw)) => {
                match raw
                    .message
                    .content
                    .into_iter()
                    .find(|item| item.kind == "tool_result" && item.tool_use_id.is_some())
                {
                    Some(item) => StreamEvent::UserToolResult(ToolResult {
                        tool_use_id: item.tool_use_id.unwrap_or_default(),
                        content: tool_result_content(&item.content),
                        is_error: item.is_error,
                    }),
                    None => StreamEvent::Unknown("user".into()),
                }
            }
            StreamEventRaw::Typed(TypedLineRaw::Result(event)) => StreamEvent::Result(event),
            StreamEventRaw::Typed(TypedLineRaw::RateLimit) => StreamEvent::RateLimit,
            StreamEventRaw::TypeOnly(tag) => StreamEvent::Unknown(tag.kind),
        }
    }
}

impl StreamLineRaw {
    fn into_inner(self) -> StreamInner {
        let event = match self.event {
            InnerEventRaw::Typed(TypedInnerRaw::MessageStart) => InnerEvent::MessageStart,
            InnerEventRaw::Typed(TypedInnerRaw::ContentStart {
                index,
                content_block,
            }) => {
                match ContentBlock::from_raw(&content_block) {
                    Some(block) => InnerEvent::ContentStart { index, block },
                    // Unknown block type from a newer agent: keep the event
                    // kind, drop the payload.
                    None => InnerEvent::Other("content_block_start".into()),
                }
            }
            InnerEventRaw::Typed(TypedInnerRaw::ContentDelta { index, delta }) => {
                InnerEvent::ContentDelta {
                    index,
                    delta: delta.into_delta(),
                }
            }
            InnerEventRaw::Typed(TypedInnerRaw::ContentStop { index }) => {
                InnerEvent::ContentStop { index }
            }
            InnerEventRaw::Typed(TypedInnerRaw::MessageDelta { delta }) => {
                InnerEvent::MessageDelta {
                    stop_reason: delta.and_then(|d| d.stop_reason),
                }
            }
            InnerEventRaw::Typed(TypedInnerRaw::MessageStop) => InnerEvent::MessageStop,
            InnerEventRaw::TypeOnly(tag) => InnerEvent::Other(tag.kind),
        };
        StreamInner {
            session_id: self.session_id,
            event,
        }
    }
}

impl ContentBlock {
    /// Maps a raw block; `None` for a block `type` this build does not know
    /// or a `tool_use` without its identifying fields.
    fn from_raw(raw: &BlockRaw) -> Option<ContentBlock> {
        match raw.kind.as_str() {
            "text" => Some(ContentBlock::Text {
                text: raw.text.clone().unwrap_or_default(),
            }),
            "thinking" => Some(ContentBlock::Thinking {
                thinking: raw.thinking.clone().unwrap_or_default(),
                signature: raw.signature.clone().unwrap_or_default(),
            }),
            "redacted_thinking" => Some(ContentBlock::RedactedThinking),
            "tool_use" => Some(ContentBlock::ToolUse {
                id: raw.id.clone()?,
                name: raw.name.clone()?,
                input: raw.input.clone().unwrap_or(Value::Null),
            }),
            _ => None,
        }
    }
}

impl DeltaRaw {
    fn into_delta(self) -> Delta {
        match self.kind.as_str() {
            "text_delta" => Delta::Text {
                text: self.text.unwrap_or_default(),
            },
            "thinking_delta" => Delta::Thinking {
                thinking: self.thinking.unwrap_or_default(),
            },
            "input_json_delta" => Delta::InputJson {
                partial_json: self.partial_json.unwrap_or_default(),
            },
            "signature_delta" => Delta::Signature {
                signature: self.signature.unwrap_or_default(),
            },
            _ => Delta::Other,
        }
    }
}

/// String content passes through; a block array maps when every entry has a
/// `type`; anything else degrades to [`ToolResultContent::Other`].
fn tool_result_content(content: &Value) -> ToolResultContent {
    match content {
        Value::String(text) => ToolResultContent::Text(text.clone()),
        Value::Array(items) => {
            let blocks = items
                .iter()
                .map(|item| {
                    Some(ToolResultBlock {
                        kind: item.get("type")?.as_str()?.to_string(),
                        text: item
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .collect::<Option<Vec<_>>>();
            match blocks {
                Some(blocks) => ToolResultContent::Blocks(blocks),
                None => ToolResultContent::Other,
            }
        }
        _ => ToolResultContent::Other,
    }
}
