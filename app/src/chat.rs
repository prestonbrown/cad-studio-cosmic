//! Chat transcript reducer: translates the engine's [`EngineEvent`] stream
//! into persisted [`ChatMessage`] rows — the port of upstream's
//! AgentSession.swift row lifecycle plus ChatViewModel.swift's row helpers.
//! Pure: no cosmic/iced types, no I/O; [`ChatState::apply_at`] takes the
//! current [`Instant`] as a seam so tests drive a fake clock (thinking-row
//! durations, the 0.15 s preview throttle) and the bridge passes wall time.
//!
//! Semantics transcribed from the Swift (AgentSession.swift line numbers in
//! each handler):
//! - `TextStreaming` payloads are ACCUMULATED SNAPSHOTS of the current text
//!   block — the row text is SET, never appended. The engine resets its
//!   accumulation at every `content_block_start` but emits no event for a
//!   plain text block, so a new block is detected by the open row's text no
//!   longer being a prefix of the snapshot; each opened row joins `unconfirmed`
//!   at creation.
//! - `TextConfirmed` rewrites the FIRST unconfirmed row in place (FIFO); only
//!   an empty `unconfirmed` appends. `Started` drops dangling streaming state
//!   (upstream resets `streamingID`/`unconfirmedTextIDs`/`hasText` per run).
//! - Tool rows: input always accumulates; title/preview recompute under the
//!   0.15 s throttle ([`should_emit_preview`]); `ToolFinished` recomputes the
//!   title over the FULL accumulated input before setting output/state and
//!   clearing the preview — the engine emits no tool content-block-stop, so
//!   this is the only home for the title finalize over the tail that landed
//!   inside one throttle window, and the preview clear matches upstream's
//!   block stop (`updateTool(preview: nil)`): a completed row persists
//!   preview-less.
//! - Thinking rows are block-boundary timed through the `apply_at` seam.
//!
//! User-visible strings read through `fl!` at event time (the catalogs are
//! `app/i18n/*/main.ftl`, en the file of record); the engine name and login
//! command are data, not strings to translate.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use time::OffsetDateTime;
use uuid::Uuid;

use engine::agent::locator::AgentKind;
use engine::agent::session::{EngineEvent, SystemRow, TurnOutcome};
use engine::agent::tool_summary::{live_preview, live_title};
use engine::project::store::{Action, ChatMessage, Role, ToolActivity, ToolState};

use crate::fl;

/// Upstream's live-preview throttle window (AgentSession.swift:179).
const PREVIEW_THROTTLE: Duration = Duration::from_millis(150);

/// The live chat transcript plus the per-turn pointers upstream keeps split
/// across AgentSession and ChatViewModel. Rows are [`ChatMessage`]s exactly as
/// persisted; the id fields address rows by their [`Uuid`].
#[derive(Debug, Clone)]
pub struct ChatState {
    /// The transcript, in order — what `save_chat` persists and the chat pane
    /// renders (upstream `ChatViewModel.messages`).
    pub messages: Vec<ChatMessage>,
    /// The open thinking row (upstream `chat.activeThinkingID`).
    pub active_thinking_id: Option<Uuid>,
    /// The open streaming text row (upstream `streamingID`).
    pub streaming_id: Option<Uuid>,
    /// When the open thinking row opened (upstream `thinkingStart`) — read
    /// through the `apply_at` seam, never wall time.
    pub thinking_started_at: Option<Instant>,
    /// Streaming rows not yet confirmed, FIFO (upstream `unconfirmedTextIDs`).
    pub unconfirmed: Vec<Uuid>,
    /// An assistant text block was confirmed this turn (upstream `hasText`);
    /// reset by [`EngineEvent::Started`].
    pub turn_had_confirmed_text: bool,
    /// The engine this conversation's turns run under — the name the
    /// thinking/auth/error rows interpolate and the login command the auth
    /// row shows (AgentLocator.swift:11-30). Set at turn start from the
    /// per-turn settings re-read so a flipped engine names its
    /// own rows from its first turn. Defaults claude, like `AgentEngine(rawValue:)`.
    pub engine: AgentKind,
    /// The input bar's draft (upstream `ChatViewModel.draft`).
    pub draft: String,
    /// The draft's attachment references (upstream `draftAttachments`).
    pub draft_attachments: Vec<String>,
    /// The recorded version numbers the transcript's version markers
    /// resolve against (upstream `project.history.versions`). The window
    /// mirrors them from the bridge; a marker whose number is not here
    /// shows no Current/Restore affordance (ChatView.swift:205).
    pub versions: Vec<i64>,
    /// The version the project currently sits on (upstream
    /// `project.metadata.currentVersion`).
    pub current_version: Option<i64>,
    /// Per tool id, when its title/preview were last recomputed (upstream
    /// `LiveTool.updatedAt`); absent = never emitted.
    tool_last_emit: HashMap<String, Instant>,
}

impl ChatState {
    /// A state over a loaded transcript (callers do
    /// `load_chat(root).unwrap_or_default()` — upstream's `(try? …) ?? []`).
    /// Loaded rows are all closed: no streaming, thinking, or unconfirmed
    /// row reopens, whatever state a crash left them in.
    pub fn new(persisted: Vec<ChatMessage>) -> Self {
        Self {
            messages: persisted,
            active_thinking_id: None,
            streaming_id: None,
            thinking_started_at: None,
            unconfirmed: Vec::new(),
            turn_had_confirmed_text: false,
            engine: AgentKind::Claude,
            draft: String::new(),
            draft_attachments: Vec::new(),
            versions: Vec::new(),
            current_version: None,
            tool_last_emit: HashMap::new(),
        }
    }

    /// Swap the transcript for the freshly loaded rows, keeping the input
    /// bar's draft and attachments — the driver's bookkeeping appends rows
    /// behind the window's back mid-conversation (upstream appends to the
    /// same live model), so the reload must not eat what the user is
    /// typing. Loaded rows are all closed, like [`ChatState::new`].
    pub fn reload_rows(&mut self, persisted: Vec<ChatMessage>) {
        self.messages = persisted;
        self.active_thinking_id = None;
        self.streaming_id = None;
        self.thinking_started_at = None;
        self.unconfirmed.clear();
        self.turn_had_confirmed_text = false;
        self.tool_last_emit.clear();
    }

    /// Translate one engine event at wall time `now`.
    pub fn apply_at(&mut self, event: &EngineEvent, now: Instant) {
        match event {
            EngineEvent::Started { .. } => self.begin_turn(),
            EngineEvent::ThinkingStarted => self.thinking_started(now),
            EngineEvent::Thinking { estimated_tokens } => self.thinking_tokens(*estimated_tokens),
            EngineEvent::ThinkingFinished => self.thinking_finished(now),
            EngineEvent::TextStreaming { text } => self.text_streaming(text),
            EngineEvent::TextConfirmed { text } => self.text_confirmed(text),
            EngineEvent::ToolStarted { id, name, title } => {
                self.tool_started(id, name, &crate::engine_text::tool_title(title))
            }
            EngineEvent::ToolInput { id, partial } => self.tool_input(id, partial, now),
            EngineEvent::ToolFinished {
                id,
                output,
                is_error,
            } => {
                self.complete_tool(id, output, *is_error);
            }
            EngineEvent::System { row } => {
                self.append_row(Role::System, system_row_text(*row));
            }
            EngineEvent::TurnFinished(outcome) => self.turn_finished(outcome),
        }
    }

    /// [`ChatState::apply_at`] on the wall clock — the production seam.
    pub fn apply(&mut self, event: &EngineEvent) {
        self.apply_at(event, Instant::now());
    }

    /// Running tool rows become failed with their preview cleared — upstream
    /// `failRunningTools` (ChatViewModel.swift:87-92), run at every turn end.
    pub fn fail_running_tools(&mut self) {
        for row in &mut self.messages {
            if let Some(tool) = row.tool.as_mut() {
                if tool.state == ToolState::Running {
                    tool.state = ToolState::Failed;
                    tool.preview = None;
                }
            }
        }
    }

    /// The input bar can send when the draft has non-whitespace text or an
    /// attachment is staged (upstream `canSend`).
    pub fn can_send(&self) -> bool {
        !self.draft.trim().is_empty() || !self.draft_attachments.is_empty()
    }

    /// Take the draft for a send: trimmed text plus staged attachments,
    /// clearing both. `None` when there is nothing to send (upstream
    /// `takeDraft`).
    pub fn take_draft(&mut self) -> Option<(String, Vec<String>)> {
        let text = self.draft.trim().to_string();
        if text.is_empty() && self.draft_attachments.is_empty() {
            return None;
        }
        let attachments = std::mem::take(&mut self.draft_attachments);
        self.draft.clear();
        Some((text, attachments))
    }

    /// Stage attachments into the draft, skipping ones already staged
    /// (upstream `attach`, ChatViewModel.swift:22-28:
    /// `draftAttachments += urls.filter { !draftAttachments.contains($0) }`).
    /// The paste and drag-drop surfaces both land here.
    pub fn attach(&mut self, paths: Vec<String>) {
        let fresh: Vec<String> = paths
            .into_iter()
            .filter(|path| !self.draft_attachments.contains(path))
            .collect();
        self.draft_attachments.extend(fresh);
    }

    /// Unstage one attachment (upstream `detach`, ChatViewModel.swift:26-28).
    pub fn detach(&mut self, path: &str) {
        self.draft_attachments.retain(|staged| staged != path);
    }

    /// Append a row, returning its id (upstream `ChatViewModel.append`).
    pub fn append(&mut self, message: ChatMessage) -> Uuid {
        let id = message.id;
        self.messages.push(message);
        id
    }

    /// Upstream `run()`'s per-turn resets (AgentSession.swift:33-38): the
    /// dangling streaming row is dropped from the pointers (its partial text
    /// stays in the transcript), the unconfirmed queue with it.
    fn begin_turn(&mut self) {
        self.streaming_id = None;
        self.unconfirmed.clear();
        self.turn_had_confirmed_text = false;
        self.tool_last_emit.clear();
    }

    /// Upstream `startBlock`'s thinking case (AgentSession.swift:149-153).
    fn thinking_started(&mut self, now: Instant) {
        let id = self.append_row(
            Role::Thinking,
            fl!("sta-ragionando", agent = self.engine.display_name()),
        );
        self.active_thinking_id = Some(id);
        self.thinking_started_at = Some(now);
    }

    /// Upstream `updateThinking` (AgentSession.swift:188-191).
    fn thinking_tokens(&mut self, estimated_tokens: i64) {
        if let Some(id) = self.active_thinking_id {
            self.set_text(
                id,
                fl!("sta-ragionando-circa-token", tokens = estimated_tokens),
            );
        }
    }

    /// Upstream `stopBlock`'s thinking case (AgentSession.swift:165-170):
    /// `max(1, round(interval))`, the row id and start both cleared.
    fn thinking_finished(&mut self, now: Instant) {
        let Some(id) = self.active_thinking_id.take() else {
            return;
        };
        let seconds = match self.thinking_started_at.take() {
            Some(start) => (now - start).as_secs_f64().round().max(1.0) as i64,
            None => 1,
        };
        self.set_text(id, fl!("ha-ragionato-per-lld-s", seconds = seconds));
    }

    /// Set the open streaming row's text to the snapshot, opening a row for a
    /// new block when the open row's text is not a prefix of it. Opened rows
    /// join `unconfirmed` at creation (AgentSession.swift:225-235 — upstream
    /// appends the empty row there; the engine's payload is the accumulated
    /// snapshot, so the row opens holding it).
    fn text_streaming(&mut self, text: &str) {
        let open = self.streaming_id.filter(|&id| {
            self.messages
                .iter()
                .rev()
                .any(|row| row.id == id && text.starts_with(row.text.as_str()))
        });
        match open {
            Some(id) => self.set_text(id, text),
            None => {
                let id = self.append_row(Role::Assistant, text);
                self.unconfirmed.push(id);
                self.streaming_id = Some(id);
            }
        }
    }

    /// Upstream's assistant text block (AgentSession.swift:239-245): FIFO
    /// rewrite of the first unconfirmed row, appending only when none queued.
    fn text_confirmed(&mut self, text: &str) {
        self.turn_had_confirmed_text = true;
        match self.unconfirmed.first().copied() {
            Some(id) => {
                self.unconfirmed.remove(0);
                self.set_text(id, text);
            }
            None => {
                self.append_row(Role::Assistant, text);
            }
        }
    }

    /// Upstream `startBlock`'s toolUse case (AgentSession.swift:154-158).
    fn tool_started(&mut self, id: &str, name: &str, title: &str) {
        self.messages.push(ChatMessage {
            id: Uuid::new_v4(),
            role: Role::Tool,
            text: title.to_string(),
            date: OffsetDateTime::now_utc(),
            tool: Some(ToolActivity {
                tool_use_id: id.to_string(),
                name: name.to_string(),
                input: String::new(),
                output: None,
                preview: None,
                state: ToolState::Running,
            }),
            detail: None,
            action: None,
            attachments: Vec::new(),
        });
    }

    /// Upstream `appendToolInput` (AgentSession.swift:176-186): the input
    /// always accumulates; the title/preview recompute only past the
    /// throttle window.
    fn tool_input(&mut self, id: &str, partial: &str, now: Instant) {
        let Some(at) = self.last_tool_row(id) else {
            return;
        };
        let Some(tool) = self.messages[at].tool.as_mut() else {
            return;
        };
        tool.input.push_str(partial);
        if !should_emit_preview(self.tool_last_emit.get(id).copied(), now) {
            return;
        }
        self.tool_last_emit.insert(id.to_string(), now);
        let (title, preview) = match self.messages[at].tool.as_ref() {
            Some(tool) => (
                crate::engine_text::tool_title(&live_title(&tool.name, &tool.input)),
                live_preview(&tool.input),
            ),
            None => return,
        };
        self.messages[at].text = title;
        if let Some(tool) = self.messages[at].tool.as_mut() {
            tool.preview = preview;
        }
    }

    /// Upstream `completeTool` (ChatViewModel.swift:81-85) preceded by the
    /// title finalize the engine has no event for (the tool's
    /// content-block-stop, AgentSession.swift:171-173): the title recomputes
    /// over the ACCUMULATED input, then output/state land and the preview
    /// CLEARS — upstream's block stop passes `preview: nil`, so the persisted
    /// end state of a completed row is preview-less (chat.json §3 parity).
    /// A tool whose input never streamed (a row the engine opened at an
    /// assistant snapshot, title already `summarize`d
    /// over the full input) keeps that title: upstream's finalize only runs
    /// for stream-announced tools (`liveTools[index]`), and recomputing over
    /// an empty accumulation would clobber it with the generic fallback.
    fn complete_tool(&mut self, id: &str, output: &str, failed: bool) {
        let Some(at) = self.last_tool_row(id) else {
            return;
        };
        self.tool_last_emit.remove(id);
        let streamed = self.messages[at]
            .tool
            .as_ref()
            .is_some_and(|tool| !tool.input.is_empty());
        if streamed {
            let title = match self.messages[at].tool.as_ref() {
                Some(tool) => crate::engine_text::tool_title(&live_title(&tool.name, &tool.input)),
                None => return,
            };
            self.messages[at].text = title;
        }
        let Some(tool) = self.messages[at].tool.as_mut() else {
            return;
        };
        tool.preview = None;
        tool.output = Some(output.to_string());
        tool.state = if failed {
            ToolState::Failed
        } else {
            ToolState::Succeeded
        };
    }

    /// Upstream `finish`/`reportFailure` (AgentSession.swift:264-303).
    fn turn_finished(&mut self, outcome: &TurnOutcome) {
        self.fail_running_tools();
        self.active_thinking_id = None;
        match outcome {
            TurnOutcome::Success { text, .. } => {
                if !self.turn_had_confirmed_text && !text.is_empty() {
                    self.append_row(Role::Assistant, text.clone());
                }
            }
            TurnOutcome::Failed {
                error,
                auth,
                exit_code,
            } => {
                let text = if *auth {
                    fl!(
                        "non-autenticato-apri-il-terminale-ed-esegui-per-effettuare-l-accesso",
                        agent = self.engine.display_name(),
                        command = self.engine.login_command()
                    )
                } else {
                    match exit_code {
                        // `ha-terminato-con-un-errore-codice-d`; a code-less
                        // failure (signal death, never spawned) reads the
                        // port-added code-less row — upstream's status is
                        // non-optional
                        Some(code) => fl!(
                            "ha-terminato-con-un-errore-codice-d",
                            agent = self.engine.display_name(),
                            code = code
                        ),
                        None => fl!(
                            "ha-terminato-con-un-errore",
                            agent = self.engine.display_name()
                        ),
                    }
                };
                let id = Uuid::new_v4();
                self.messages.push(ChatMessage {
                    id,
                    role: Role::System,
                    text,
                    date: OffsetDateTime::now_utc(),
                    tool: None,
                    // upstream's auth row keeps the detail unconditionally;
                    // the error row only when non-empty
                    detail: if *auth || !error.is_empty() {
                        Some(error.clone())
                    } else {
                        None
                    },
                    action: auth.then_some(Action::OpenTerminal),
                    attachments: Vec::new(),
                });
            }
            // upstream's interrupted summary row; Busy/MissingSession never
            // reach the app (the engine's dance absorbs them)
            TurnOutcome::Cancelled => {
                self.append_row(Role::Summary, interrupted());
            }
            // AgentSession.swift:42-46: the row offers Configure agent
            TurnOutcome::AgentMissing => {
                let text = fl!(
                    "non-installato-oppure-non-stato-trovato-installalo-o-indica-il-percorso-dell-eseguibile",
                    agent = self.engine.display_name()
                );
                self.append_row(Role::System, text);
                if let Some(row) = self.messages.last_mut() {
                    row.action = Some(Action::ConfigureClaude);
                }
            }
            // AgentSession.swift:98-99
            TurnOutcome::StartFailed { error } => {
                let text = fl!("impossibile-avviare", agent = self.engine.display_name());
                self.append_row(Role::System, text);
                if let Some(row) = self.messages.last_mut() {
                    row.detail = Some(error.clone());
                }
            }
            TurnOutcome::Busy | TurnOutcome::MissingSession => {}
        }
    }

    /// Index of the LAST row carrying `id`'s tool activity (upstream looks
    /// rows up by `messages.lastIndex`).
    fn last_tool_row(&self, id: &str) -> Option<usize> {
        self.messages
            .iter()
            .rposition(|row| row.tool.as_ref().is_some_and(|tool| tool.tool_use_id == id))
    }

    /// Upstream `setText` (ChatViewModel.swift:59-62): a missing id is a
    /// no-op, never an error.
    fn set_text(&mut self, id: Uuid, text: impl Into<String>) {
        if let Some(row) = self.messages.iter_mut().rev().find(|row| row.id == id) {
            row.text = text.into();
        }
    }

    fn append_row(&mut self, role: Role, text: impl Into<String>) -> Uuid {
        let id = Uuid::new_v4();
        self.messages.push(ChatMessage {
            id,
            role,
            text: text.into(),
            date: OffsetDateTime::now_utc(),
            tool: None,
            detail: None,
            action: None,
            attachments: Vec::new(),
        });
        id
    }
}

/// Upstream's 0.15 s live-preview throttle (AgentSession.swift:179): a
/// recompute fires only when the last one is older than the window — strictly
/// older, as upstream's `>` comparisons. `None` means never emitted (upstream
/// seeds `LiveTool.updatedAt` with `Date.distantPast`).
pub fn should_emit_preview(last_emit: Option<Instant>, now: Instant) -> bool {
    match last_emit {
        Some(last) => now - last > PREVIEW_THROTTLE,
        None => true,
    }
}

/// `interrotto` — the cancelled turn's summary row and failure activity
/// (the bridge's [`crate::bridge`] turn-terminal reuses it).
pub fn interrupted() -> String {
    fl!("interrotto")
}

/// The engine's system rows, localized (AgentSession.swift:81/:91). The
/// engine emits stable [`SystemRow`] ids — it has no catalog — and this is
/// the app boundary every System row folds through: the driver's transcript
/// and the window's live state both apply events via [`ChatState::apply`],
/// so the text resolves once here and chat.json persists it like any row.
fn system_row_text(row: SystemRow) -> String {
    match row {
        SystemRow::BusyFreshContext => fl!(
            "la-sessione-precedente-ancora-in-uso-da-un-altro-processo-il-contesto-della-conversazione-ripartito-ma-il-model-py-esistente-resta-la-base-del-lavoro"
        ),
        SystemRow::MissingSession => fl!(
            "la-sessione-precedente-non-stata-trovata-il-contesto-della-conversazione-ripartito-ma-il-model-py-esistente-resta-la-base-del-lavoro"
        ),
    }
}

/// The version a summary row marks, if it is a version marker (upstream
/// `VersionMarker.number(of:)`, ChatView.swift:185-192): the detail field
/// when it parses as an integer, else a two-word text whose second word
/// does ("Versione 3"). Any other summary row (the interrupted marker) is
/// plain caption text.
pub fn version_number(message: &ChatMessage) -> Option<i64> {
    if let Some(number) = message.detail.as_deref().and_then(swift_int) {
        return Some(number);
    }
    // Swift's `split(separator: " ")` drops empty subsequences
    let words: Vec<&str> = message.text.split(' ').filter(|w| !w.is_empty()).collect();
    match words.as_slice() {
        [_, number] => swift_int(number),
        _ => None,
    }
}

/// Swift's `Int(String)`: an optional sign then ASCII digits only — no
/// surrounding whitespace, no leading `+` alone.
fn swift_int(s: &str) -> Option<i64> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// What a version marker row offers at its trailing edge
/// (ChatView.swift:205-215).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerAffordance {
    /// The version is not in the history: nothing shows.
    None,
    /// The project sits on this version: the "Attuale" caption.
    Current,
    /// Any other recorded version: the "Ripristina" link, disabled while
    /// the project is busy.
    Restore { enabled: bool },
}

/// The trailing affordance of the marker for version `number`.
pub fn marker_affordance(
    number: i64,
    versions: &[i64],
    current: Option<i64>,
    busy: bool,
) -> MarkerAffordance {
    if !versions.contains(&number) {
        MarkerAffordance::None
    } else if current == Some(number) {
        MarkerAffordance::Current
    } else {
        MarkerAffordance::Restore { enabled: !busy }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started() -> EngineEvent {
        EngineEvent::Started { session_id: None }
    }

    fn ts(text: &str) -> EngineEvent {
        EngineEvent::TextStreaming { text: text.into() }
    }

    fn tc(text: &str) -> EngineEvent {
        EngineEvent::TextConfirmed { text: text.into() }
    }

    fn tool_started(id: &str, name: &str) -> EngineEvent {
        EngineEvent::ToolStarted {
            id: id.into(),
            name: name.into(),
            title: live_title(name, ""),
        }
    }

    fn tool_input(id: &str, partial: &str) -> EngineEvent {
        EngineEvent::ToolInput {
            id: id.into(),
            partial: partial.into(),
        }
    }

    fn tool_finished(id: &str, output: &str, is_error: bool) -> EngineEvent {
        EngineEvent::ToolFinished {
            id: id.into(),
            output: output.into(),
            is_error,
        }
    }

    fn success(text: &str) -> EngineEvent {
        EngineEvent::TurnFinished(TurnOutcome::Success {
            session_id: None,
            text: text.into(),
        })
    }

    fn failed(error: &str, auth: bool, exit_code: Option<i32>) -> EngineEvent {
        EngineEvent::TurnFinished(TurnOutcome::Failed {
            error: error.into(),
            auth,
            exit_code,
        })
    }

    fn row_text(state: &ChatState) -> Vec<&str> {
        state.messages.iter().map(|m| m.text.as_str()).collect()
    }

    // ─── streaming snapshots ────────────────────────────────────────────────

    /// TextStreaming payloads are accumulated snapshots: the row text is SET,
    /// so "He" then "Hello" leaves one row reading "Hello" — appending would
    /// read "HeHello".
    #[test]
    fn snapshot_streaming_sets_not_appends() {
        let mut state = ChatState::new(Vec::new());
        state.apply_at(&started(), Instant::now());
        state.apply_at(&ts("He"), Instant::now());
        state.apply_at(&ts("Hello"), Instant::now());
        assert_eq!(row_text(&state), ["Hello"], "{:?}", state.messages);
        assert_eq!(state.unconfirmed.len(), 1, "the open row is unconfirmed");
        assert_eq!(state.streaming_id, Some(state.messages[0].id));

        // single block + confirm: exactly ONE row survives, holding the
        // confirmed text in place
        state.apply_at(&tc("Hello!"), Instant::now());
        assert_eq!(row_text(&state), ["Hello!"]);
        assert!(state.unconfirmed.is_empty());
        assert!(state.turn_had_confirmed_text);

        // two streamed blocks: a second row opens when the open row's text is
        // not a prefix of the snapshot, and the confirm lands on the FIRST
        // row, in place (same id), not on a new append
        let mut state = ChatState::new(Vec::new());
        state.apply_at(&started(), Instant::now());
        state.apply_at(&ts("He"), Instant::now());
        state.apply_at(&ts("Hello"), Instant::now());
        state.apply_at(&ts("A new block"), Instant::now());
        assert_eq!(row_text(&state), ["Hello", "A new block"]);
        assert_eq!(state.unconfirmed.len(), 2);
        let first_streamed = state.messages[0].id;
        state.apply_at(&tc("Hello, revised"), Instant::now());
        state.apply_at(&tc("A new block"), Instant::now());
        assert_eq!(row_text(&state), ["Hello, revised", "A new block"]);
        assert_eq!(state.messages[0].id, first_streamed, "confirmed in place");
        assert!(state.unconfirmed.is_empty());
    }

    /// Confirm with an empty unconfirmed queue appends (text that never
    /// streamed).
    #[test]
    fn confirmed_without_streaming_appends() {
        let mut state = ChatState::new(Vec::new());
        state.apply_at(&started(), Instant::now());
        state.apply_at(&tc("only confirmed"), Instant::now());
        assert_eq!(row_text(&state), ["only confirmed"]);
        assert_eq!(state.messages[0].role, Role::Assistant);
    }

    /// A new turn's Started drops the dangling streaming row from the
    /// pointers: its partial text stays as a row, and a later confirm cannot
    /// reach it (upstream resets `unconfirmedTextIDs` per run,
    /// AgentSession.swift:34).
    #[test]
    fn started_drops_dangling_streaming_row() {
        let mut state = ChatState::new(Vec::new());
        state.apply_at(&started(), Instant::now());
        state.apply_at(&ts("partial answ"), Instant::now());
        let partial_id = state.messages[0].id;

        state.apply_at(&started(), Instant::now());
        assert_eq!(state.streaming_id, None);
        assert!(state.unconfirmed.is_empty());
        assert!(!state.turn_had_confirmed_text, "hasText resets per run");

        state.apply_at(&tc("the full answer"), Instant::now());
        assert_eq!(row_text(&state), ["partial answ", "the full answer"]);
        assert_ne!(state.messages[1].id, partial_id);
    }

    // ─── thinking rows ──────────────────────────────────────────────────────

    /// The thinking row's three text forms and the ≥1 s duration rule.
    #[test]
    fn thinking_row_lifecycle() {
        let _lang = crate::localize::pin_en();
        let t0 = Instant::now();
        let mut state = ChatState::new(Vec::new());
        state.apply_at(&started(), t0);
        state.apply_at(&EngineEvent::ThinkingStarted, t0);
        assert_eq!(row_text(&state), ["Claude Code is thinking…"]);
        assert_eq!(state.messages[0].role, Role::Thinking);
        let thinking_id = state.messages[0].id;
        assert_eq!(state.active_thinking_id, Some(thinking_id));
        assert_eq!(state.thinking_started_at, Some(t0));

        state.apply_at(
            &EngineEvent::Thinking {
                estimated_tokens: 50,
            },
            t0,
        );
        assert_eq!(row_text(&state), ["Thinking · about 50 tokens"]);

        // 2.3 s of open interval rounds to 2
        state.apply_at(
            &EngineEvent::ThinkingFinished,
            t0 + Duration::from_millis(2300),
        );
        assert_eq!(row_text(&state), ["Thought for 2 s"]);
        assert_eq!(state.active_thinking_id, None);
        assert_eq!(state.thinking_started_at, None);

        // sub-second intervals clamp up to 1 s (upstream max(1, …))
        let t0 = Instant::now();
        state.apply_at(&EngineEvent::ThinkingStarted, t0);
        state.apply_at(
            &EngineEvent::ThinkingFinished,
            t0 + Duration::from_millis(200),
        );
        assert_eq!(state.messages.last().unwrap().text, "Thought for 1 s");
        state.apply_at(&EngineEvent::ThinkingStarted, t0);
        state.apply_at(
            &EngineEvent::ThinkingFinished,
            t0 + Duration::from_millis(600),
        );
        assert_eq!(state.messages.last().unwrap().text, "Thought for 1 s");
    }

    /// Token estimates and stops without an open row are no-ops (upstream
    /// guards both on `activeThinkingID`; pong-style turns carry
    /// `system/thinking_tokens` with no thinking block at all).
    #[test]
    fn thinking_events_without_open_row_are_noops() {
        let _lang = crate::localize::pin_en();
        let now = Instant::now();
        let mut state = ChatState::new(Vec::new());
        state.apply_at(&started(), now);
        state.apply_at(
            &EngineEvent::Thinking {
                estimated_tokens: 50,
            },
            now,
        );
        state.apply_at(&EngineEvent::ThinkingFinished, now);
        assert!(state.messages.is_empty());

        // a stop left dangling by the turn end does not resurrect the row
        state.apply_at(&EngineEvent::ThinkingStarted, now);
        state.apply_at(&success("done"), now);
        state.apply_at(&EngineEvent::ThinkingFinished, now);
        assert_eq!(state.active_thinking_id, None);
        assert_eq!(row_text(&state), ["Claude Code is thinking…", "done"]);
    }

    // ─── tool rows ──────────────────────────────────────────────────────────

    /// Full tool row lifecycle: open, throttled live updates, completion with
    /// the title finalized over the FULL input even when the tail landed
    /// inside one throttle window, the preview cleared at completion (the
    /// persisted end state is preview-less), name kept, state set.
    #[test]
    fn tool_lifecycle_full() {
        let t0 = Instant::now();
        let mut state = ChatState::new(Vec::new());
        state.apply_at(&started(), t0);
        state.apply_at(&tool_started("t1", "Write"), t0);
        let row = &state.messages[0];
        assert_eq!(row.role, Role::Tool);
        assert_eq!(row.text, "Writing a file", "title from the engine event");
        let tool = row.tool.as_ref().unwrap();
        assert_eq!(tool.name, "Write");
        assert_eq!(tool.tool_use_id, "t1");
        assert_eq!(tool.state, ToolState::Running);
        assert_eq!(tool.input, "");
        assert_eq!(tool.preview, None);
        assert_eq!(tool.output, None);

        // first delta emits immediately (never emitted before)
        state.apply_at(
            &tool_input("t1", r#"{"file_path":"model.py","content":"alpha"#),
            t0,
        );
        let tool = state.messages[0].tool.as_ref().unwrap();
        assert_eq!(state.messages[0].text, "Writing the model · 1 lines…");
        assert_eq!(tool.input, r#"{"file_path":"model.py","content":"alpha"#);
        assert_eq!(tool.preview.as_deref(), Some("alpha"));

        // the tail lands 10 ms later — INSIDE the 0.15 s window, so no
        // recompute even though the input keeps accumulating
        state.apply_at(
            &tool_input("t1", r#"\nbeta"}"#),
            t0 + Duration::from_millis(10),
        );
        assert_eq!(state.messages[0].text, "Writing the model · 1 lines…");
        assert_eq!(
            state.messages[0].tool.as_ref().unwrap().input,
            r#"{"file_path":"model.py","content":"alpha\nbeta"}"#
        );

        // completion recomputes over the ACCUMULATED input: the line count
        // now reflects both fragments, the unterminated-string ellipsis is
        // gone, the preview CLEARS (upstream's block stop nils it before the
        // result lands), and output/state land after the title
        state.apply_at(
            &tool_finished("t1", "wrote it", false),
            t0 + Duration::from_millis(20),
        );
        assert_eq!(state.messages[0].text, "Writing the model · 2 lines");
        let tool = state.messages[0].tool.as_ref().unwrap();
        assert_eq!(tool.preview, None, "completion clears the preview");
        assert_eq!(tool.output.as_deref(), Some("wrote it"));
        assert_eq!(tool.state, ToolState::Succeeded);
        assert_eq!(tool.name, "Write", "name kept on completion");

        // an errored tool lands failed
        state.apply_at(&tool_started("t2", "Bash"), t0);
        state.apply_at(&tool_finished("t2", "Traceback", true), t0);
        assert_eq!(
            state.messages[1].tool.as_ref().unwrap().state,
            ToolState::Failed
        );

        // input for an unknown row is dropped
        state.apply_at(&tool_input("ghost", "{}"), t0);
        assert_eq!(state.messages.len(), 2);
    }

    /// A delta inside the window does not recompute; once the window has
    /// passed, the next delta recomputes over everything accumulated.
    #[test]
    fn tool_input_recomputes_after_window() {
        let t0 = Instant::now();
        let mut state = ChatState::new(Vec::new());
        state.apply_at(&tool_started("t1", "Write"), t0);
        state.apply_at(
            &tool_input("t1", r#"{"file_path":"model.py","content":"a"#),
            t0,
        );
        assert_eq!(state.messages[0].text, "Writing the model · 1 lines…");
        // 10 ms later, inside the window: the input accumulates but the title
        // stays at the first fragment's form
        state.apply_at(&tool_input("t1", r#"\nb"#), t0 + Duration::from_millis(10));
        assert_eq!(state.messages[0].text, "Writing the model · 1 lines…");
        // 151 ms after the emit, past the window: the recompute sees both
        // fragments plus its own
        state.apply_at(
            &tool_input("t1", r#"\nc"}"#),
            t0 + PREVIEW_THROTTLE + Duration::from_millis(1),
        );
        assert_eq!(state.messages[0].text, "Writing the model · 3 lines");
        assert_eq!(
            state.messages[0].tool.as_ref().unwrap().preview.as_deref(),
            Some("a\nb\nc")
        );
    }

    /// Exactly the boundary: strictly older emits, equal does not (upstream's
    /// strict `>`).
    #[test]
    fn preview_throttle_boundary() {
        let t0 = Instant::now();
        assert!(should_emit_preview(None, t0));
        assert!(!should_emit_preview(Some(t0), t0));
        assert!(!should_emit_preview(Some(t0), t0 + PREVIEW_THROTTLE));
        assert!(should_emit_preview(
            Some(t0),
            t0 + PREVIEW_THROTTLE + Duration::from_nanos(1)
        ));
    }

    /// A tool the engine opened at an assistant snapshot (no streamed input)
    /// already carries the `summarize` title computed over the block's full
    /// input; completion must keep it, not fall back over an empty
    /// accumulation (upstream's finalize only runs for stream-announced
    /// tools).
    #[test]
    fn tool_finished_keeps_engine_title_without_streamed_input() {
        let _lang = crate::localize::pin_en();
        let t0 = Instant::now();
        let mut state = ChatState::new(Vec::new());
        state.apply_at(&started(), t0);
        state.apply_at(
            &EngineEvent::ToolStarted {
                id: "snap".into(),
                name: "Bash".into(),
                title: engine::agent::tool_summary::ToolAction::AnalyzeModel.into(),
            },
            t0,
        );
        state.apply_at(&tool_finished("snap", "Traceback", true), t0);
        assert_eq!(state.messages[0].text, "Analyzing the model");
        let tool = state.messages[0].tool.as_ref().unwrap();
        assert_eq!(tool.state, ToolState::Failed);
        assert_eq!(tool.output.as_deref(), Some("Traceback"));
        assert_eq!(tool.preview, None);
    }

    // ─── turn ends ──────────────────────────────────────────────────────────

    /// Every turn end fails open tool rows (state failed, preview cleared)
    /// and clears the thinking pointer, whatever the outcome.
    #[test]
    fn fail_open_at_turn_end() {
        for outcome in [
            success("done"),
            failed("boom", false, Some(1)),
            EngineEvent::TurnFinished(TurnOutcome::Cancelled),
        ] {
            let t0 = Instant::now();
            let mut state = ChatState::new(Vec::new());
            state.apply_at(&started(), t0);
            state.apply_at(&tool_started("t1", "Write"), t0);
            state.apply_at(
                &tool_input("t1", r#"{"file_path":"model.py","content":"x"#),
                t0,
            );
            state.apply_at(&EngineEvent::ThinkingStarted, t0);
            state.apply_at(&outcome, t0);
            let tool = state.messages[0].tool.as_ref().unwrap();
            assert_eq!(tool.state, ToolState::Failed, "{:?}", state.messages);
            assert_eq!(tool.preview, None, "fail-open clears the preview");
            assert_eq!(state.active_thinking_id, None);
        }
    }

    /// A successful turn without confirmed text appends the outcome text as
    /// the assistant row (the engine already applied upstream's hasText rule;
    /// the app appends only when no confirmed row arrived) — including on a
    /// LATER turn after an earlier one had text, because Started resets the
    /// flag.
    #[test]
    fn success_no_text_appends_turn_text() {
        let t0 = Instant::now();
        let mut state = ChatState::new(Vec::new());

        // turn 1: streamed + confirmed, so the outcome text must NOT append
        state.apply_at(&started(), t0);
        state.apply_at(&ts("Hi"), t0);
        state.apply_at(&tc("Hi there"), t0);
        state.apply_at(&success("Hi there"), t0);
        assert_eq!(
            row_text(&state),
            ["Hi there"],
            "no duplicate of the outcome text"
        );

        // turn 2: no text at all — the fallback still fires
        state.apply_at(&started(), t0);
        state.apply_at(&success("the result text"), t0);
        assert_eq!(row_text(&state), ["Hi there", "the result text"]);
        assert_eq!(state.messages[1].role, Role::Assistant);

        // empty outcome text appends nothing
        state.apply_at(&started(), t0);
        state.apply_at(&success(""), t0);
        assert_eq!(state.messages.len(), 2);
    }

    /// The auth failure row carries the login command text and the
    /// open-terminal action, and both survive the chat.json round-trip.
    #[test]
    fn auth_row_persists_open_terminal_action() {
        let _lang = crate::localize::pin_en();
        let now = Instant::now();
        let mut state = ChatState::new(Vec::new());
        state.apply_at(&failed("api error status 401", true, Some(1)), now);
        assert_eq!(
            row_text(&state),
            ["Claude Code isn't signed in. Open Terminal and run `claude` to sign in."]
        );
        let row = &state.messages[0];
        assert_eq!(row.role, Role::System);
        assert_eq!(row.action, Some(Action::OpenTerminal));
        assert_eq!(row.detail.as_deref(), Some("api error status 401"));

        let root = std::env::temp_dir().join(format!("cad-chat-auth-{}", Uuid::new_v4()));
        engine::project::store::save_chat(&root, &state.messages).unwrap();
        let reloaded = engine::project::store::load_chat(&root).unwrap();
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded[0].action, Some(Action::OpenTerminal));
        assert_eq!(reloaded[0].text, state.messages[0].text);
        assert_eq!(reloaded[0].detail, state.messages[0].detail);
        let raw: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join(".cadstudio/chat.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(raw[0]["action"], "openTerminal");
    }

    /// A codex conversation names Codex, not Claude Code: the thinking row,
    /// the auth row (engine name AND login command — `codex login`), and
    /// both error rows interpolate the turn's engine
    /// (AgentLocator.swift:11-30; the name and command are per-engine data,
    /// never catalog text).
    #[test]
    fn codex_turn_names_codex_in_rows() {
        let _lang = crate::localize::pin_en();
        let now = Instant::now();
        let mut state = ChatState::new(Vec::new());
        state.engine = AgentKind::Codex;
        assert_eq!(state.engine, AgentKind::Codex);

        state.apply_at(&started(), now);
        state.apply_at(&EngineEvent::ThinkingStarted, now);
        assert_eq!(row_text(&state), ["Codex is thinking…"]);

        let mut auth = ChatState::new(Vec::new());
        auth.engine = AgentKind::Codex;
        auth.apply_at(&failed("401 unauthorized", true, Some(1)), now);
        assert_eq!(
            row_text(&auth),
            ["Codex isn't signed in. Open Terminal and run `codex login` to sign in."]
        );

        let mut errored = ChatState::new(Vec::new());
        errored.engine = AgentKind::Codex;
        errored.apply_at(&failed("spawn failed", false, Some(3)), now);
        assert_eq!(row_text(&errored), ["Codex ended with an error (code 3)."]);
        errored.apply_at(&started(), now);
        errored.apply_at(&failed("killed", false, None), now);
        assert_eq!(
            row_text(&errored).last(),
            Some(&"Codex ended with an error.")
        );

        // the claude default is unchanged
        let mut claude = ChatState::new(Vec::new());
        claude.apply_at(&failed("401", true, Some(1)), now);
        assert_eq!(
            row_text(&claude),
            ["Claude Code isn't signed in. Open Terminal and run `claude` to sign in."]
        );
    }

    /// The non-auth failure row carries the child's exit code; a code-less
    /// failure (signal, never spawned) renders without inventing one.
    #[test]
    fn non_auth_failure_row_carries_exit_code() {
        let _lang = crate::localize::pin_en();
        let now = Instant::now();
        let mut state = ChatState::new(Vec::new());
        state.apply_at(&failed("the agent fell over", false, Some(3)), now);
        assert_eq!(
            row_text(&state),
            ["Claude Code ended with an error (code 3)."]
        );
        let row = &state.messages[0];
        assert_eq!(row.role, Role::System);
        assert_eq!(row.action, None);
        assert_eq!(row.detail.as_deref(), Some("the agent fell over"));

        state.apply_at(&failed("signal death", false, None), now);
        assert_eq!(state.messages[1].text, "Claude Code ended with an error.");
        assert_eq!(state.messages[1].detail.as_deref(), Some("signal death"));

        // empty details carry no detail field (upstream's isEmpty-nil)
        state.apply_at(&failed("", false, Some(1)), now);
        assert_eq!(state.messages[2].detail, None);
    }

    /// A cancelled turn appends the summary row; Busy/MissingSession (which
    /// the engine never emits) still fail open without a row.
    #[test]
    fn cancelled_appends_summary_row() {
        let _lang = crate::localize::pin_en();
        let now = Instant::now();
        let mut state = ChatState::new(Vec::new());
        state.apply_at(&EngineEvent::TurnFinished(TurnOutcome::Cancelled), now);
        assert_eq!(row_text(&state), [interrupted()]);
        assert_eq!(state.messages[0].role, Role::Summary);

        state.apply_at(&EngineEvent::TurnFinished(TurnOutcome::Busy), now);
        state.apply_at(&EngineEvent::TurnFinished(TurnOutcome::MissingSession), now);
        assert_eq!(state.messages.len(), 1);
    }

    /// System events append verbatim system rows.
    #[test]
    fn system_event_appends_row() {
        let _lang = crate::localize::pin_en();
        let mut state = ChatState::new(Vec::new());
        state.apply_at(
            &EngineEvent::System {
                row: SystemRow::MissingSession,
            },
            Instant::now(),
        );
        // the engine names the row; the reducer resolves the catalog text
        assert_eq!(
            row_text(&state),
            [
                "The previous session wasn’t found: the conversation context has been reset, but the existing model.py remains the basis of the work."
            ]
        );
        assert_eq!(state.messages[0].role, Role::System);
        assert_eq!(state.messages[0].action, None);
    }

    // ─── persisted-open + draft ─────────────────────────────────────────────

    /// An absent or corrupt chat.json opens an empty transcript — the
    /// call-site pattern `load_chat(root).unwrap_or_default()`.
    #[test]
    fn corrupt_chat_json_opens_empty() {
        let root = std::env::temp_dir().join(format!("cad-chat-corrupt-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".cadstudio")).unwrap();
        std::fs::write(root.join(".cadstudio/chat.json"), "{ not json").unwrap();
        let loaded = engine::project::store::load_chat(&root).unwrap_or_default();
        let state = ChatState::new(loaded);
        assert!(state.messages.is_empty());

        let absent = std::env::temp_dir().join(format!("cad-chat-absent-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&absent).unwrap();
        let mut state =
            ChatState::new(engine::project::store::load_chat(&absent).unwrap_or_default());
        assert!(state.messages.is_empty());

        // and a re-save over the corrupt file rewrites it valid
        state.apply_at(&tc("fresh"), Instant::now());
        engine::project::store::save_chat(&absent, &state.messages).unwrap();
        assert_eq!(engine::project::store::load_chat(&absent).unwrap().len(), 1);
    }

    /// A persisted transcript loads as-is with no open pointers, even one
    /// whose last tool row a crash left running.
    #[test]
    fn persisted_rows_load_closed() {
        let persisted = vec![ChatMessage {
            id: Uuid::new_v4(),
            role: Role::Tool,
            text: "Working on the project".into(),
            date: OffsetDateTime::now_utc(),
            tool: Some(ToolActivity {
                tool_use_id: "old".into(),
                name: "Bash".into(),
                input: String::new(),
                output: None,
                preview: None,
                state: ToolState::Running,
            }),
            detail: None,
            action: None,
            attachments: Vec::new(),
        }];
        let state = ChatState::new(persisted);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.streaming_id, None);
        assert!(state.unconfirmed.is_empty());
        assert_eq!(state.active_thinking_id, None);
        assert!(!state.turn_had_confirmed_text);
    }

    /// canSend/takeDraft, ChatViewModel verbatim: whitespace-only drafts
    /// cannot send, trimming happens on take, attachments alone can send.
    #[test]
    fn can_send_and_take_draft() {
        let mut state = ChatState::new(Vec::new());
        assert!(!state.can_send());
        assert_eq!(state.take_draft(), None);

        state.draft = "   \n  ".into();
        assert!(!state.can_send());
        assert_eq!(state.take_draft(), None);

        state.draft = "  make a bracket  ".into();
        assert!(state.can_send());
        assert_eq!(
            state.take_draft(),
            Some(("make a bracket".into(), Vec::new()))
        );
        assert_eq!(state.draft, "");
        assert!(!state.can_send());

        state.draft_attachments = vec!["references/bracket.png".into()];
        assert!(state.can_send());
        assert_eq!(
            state.take_draft(),
            Some(("".into(), vec!["references/bracket.png".into()]))
        );
        assert!(state.draft_attachments.is_empty());
    }

    /// attach stages without duplicating what is already staged, and
    /// detach removes exactly the addressed path (ChatViewModel.swift:22-28).
    #[test]
    fn attach_dedupes_and_detach_removes_one() {
        let mut state = ChatState::new(Vec::new());
        state.attach(vec!["/tmp/sketch.png".into(), "/tmp/notes.txt".into()]);
        assert_eq!(
            state.draft_attachments,
            ["/tmp/sketch.png".to_string(), "/tmp/notes.txt".to_string()]
        );
        // the already-staged path is not staged twice
        state.attach(vec!["/tmp/sketch.png".into(), "/tmp/bracket.jpg".into()]);
        assert_eq!(
            state.draft_attachments,
            [
                "/tmp/sketch.png".to_string(),
                "/tmp/notes.txt".to_string(),
                "/tmp/bracket.jpg".to_string()
            ]
        );
        state.detach("/tmp/notes.txt");
        assert_eq!(state.draft_attachments.len(), 2);
        assert!(state.can_send(), "attachments alone still send");
    }

    /// reload_rows swaps the transcript but keeps the input bar's draft
    /// and attachments: the driver's bookkeeping appends rows mid-
    /// conversation, and a reload that ate the typing draft would make
    /// the next Send a silent no-op.
    #[test]
    fn reload_rows_keeps_the_draft() {
        let mut state = ChatState::new(Vec::new());
        state.draft = "thicken the walls".into();
        state.draft_attachments = vec!["references/bracket.png".into()];
        let reloaded = vec![ChatMessage {
            id: Uuid::new_v4(),
            role: Role::Summary,
            text: "Version 1".into(),
            date: OffsetDateTime::now_utc(),
            tool: None,
            detail: Some("1".into()),
            action: None,
            attachments: Vec::new(),
        }];
        state.reload_rows(reloaded);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].text, "Version 1");
        assert_eq!(state.draft, "thicken the walls");
        assert_eq!(state.draft_attachments, vec!["references/bracket.png"]);
        assert!(state.can_send(), "the draft still sends after the reload");
        // the loaded rows are all closed, like a fresh construction
        assert_eq!(state.streaming_id, None);
        assert!(state.unconfirmed.is_empty());
    }

    /// append returns the row's id and appends in order.
    #[test]
    fn append_returns_id() {
        let mut state = ChatState::new(Vec::new());
        let id = state.append(ChatMessage {
            id: Uuid::new_v4(),
            role: Role::User,
            text: "hello".into(),
            date: OffsetDateTime::now_utc(),
            tool: None,
            detail: None,
            action: None,
            attachments: Vec::new(),
        });
        assert_eq!(state.messages[0].id, id);
        assert_eq!(state.messages[0].role, Role::User);
    }

    fn summary(text: &str, detail: Option<&str>) -> ChatMessage {
        ChatMessage {
            id: Uuid::new_v4(),
            role: Role::Summary,
            text: text.into(),
            date: time::OffsetDateTime::now_utc(),
            tool: None,
            detail: detail.map(str::to_string),
            action: None,
            attachments: Vec::new(),
        }
    }

    /// VersionMarker.number(of:) parity: the detail wins when it is an
    /// integer; otherwise exactly two words with an integer second word;
    /// everything else (the interrupted marker, prose) is not a marker.
    #[test]
    fn version_number_parses_like_upstream() {
        assert_eq!(version_number(&summary("Versione 3", Some("3"))), Some(3));
        // detail beats text
        assert_eq!(version_number(&summary("Versione 3", Some("7"))), Some(7));
        // no detail: the two-word text form (pre-detail rows, any locale)
        assert_eq!(version_number(&summary("Version 12", None)), Some(12));
        assert_eq!(version_number(&summary("Version  12", None)), Some(12));
        // a non-integer detail falls through to the text
        assert_eq!(version_number(&summary("Versione 4", Some("x"))), Some(4));
        assert_eq!(version_number(&summary("Versione 4", Some(" 4"))), Some(4));
        // not markers
        assert_eq!(version_number(&summary("Interrotto", None)), None);
        assert_eq!(version_number(&summary("Versione tre", None)), None);
        assert_eq!(version_number(&summary("La versione 3 pronta", None)), None);
        assert_eq!(version_number(&summary("Versione 3.5", None)), None);
        assert_eq!(version_number(&summary("Versione +", None)), None);
    }

    /// The trailing affordance: unknown versions show nothing, the current
    /// one its caption, every other recorded one a Restore link that is
    /// disabled while busy.
    #[test]
    fn marker_affordance_follows_history_and_busy() {
        let versions = [1, 2, 3];
        assert_eq!(
            marker_affordance(9, &versions, Some(3), false),
            MarkerAffordance::None
        );
        assert_eq!(
            marker_affordance(3, &versions, Some(3), true),
            MarkerAffordance::Current
        );
        assert_eq!(
            marker_affordance(2, &versions, Some(3), false),
            MarkerAffordance::Restore { enabled: true }
        );
        assert_eq!(
            marker_affordance(2, &versions, Some(3), true),
            MarkerAffordance::Restore { enabled: false }
        );
        assert_eq!(
            marker_affordance(1, &versions, None, false),
            MarkerAffordance::Restore { enabled: true }
        );
    }

    /// A transcript reload keeps the version mirror (the window sets it
    /// once per history change, not per reload).
    #[test]
    fn reload_rows_keeps_version_mirror() {
        let mut state = ChatState::new(Vec::new());
        state.versions = vec![1, 2];
        state.current_version = Some(2);
        state.reload_rows(vec![summary("Versione 2", Some("2"))]);
        assert_eq!(state.versions, vec![1, 2]);
        assert_eq!(state.current_version, Some(2));
    }
}
