//! Chat pane widgets — the port of upstream `Chat/{ChatView,MessageView,
//! ToolActivityRow,ChatInputView}.swift` onto the reducer state from
//! [`crate::chat`]. This module only MAPS: every behavioral rule (row
//! lifecycle, streaming, throttled previews, `can_send`) already lives in
//! `ChatState`; the view renders its fields verbatim and never re-derives
//! them.
//!
//! The row list itself is a pure function — [`entries`] — so row order is
//! unit-testable without a window (upstream `ForEach(chat.messages)` plus the
//! busy indicator, ChatView.swift:42-68).
//!
//! Split of message handling (the shell wires this in the window task):
//! [`ChatPane::update`] owns what is purely widget state (draft editing,
//! disclosures, copy-to-clipboard); `Send`/`Stop`/`RowAction`/`OpenUrl`/
//! `Attach`/`Detach`/`RemoveQueued`/`SendQueuedNow`/`SendSuggestion` are the
//! SHELL's — it reads `ChatState` (e.g. `take_draft()`) and drives the
//! bridge, then calls [`ChatPane::sent`] after a send consumed the draft.
//!
//! User-visible strings are the `app/i18n/en/main.ftl` catalog values; each
//! constant names its key (the `fl!()` swap lands with the i18n scaffold in
//! the windows task — `fl!()` needs cosmic's runtime). Icon names are
//! freedesktop-theme approximations of upstream SF Symbols; the theme in
//! play decides what actually shows (cosmetic — flagged for the shell task).
//!
//! Upstream pieces deliberately NOT here: `VersionMarker` rows (M5 — the
//! versions model doesn't exist yet; summary rows render as plain caption
//! text), image thumbnails for attachments (needs project-root I/O; chips
//! show the file name), drag-and-drop/paste attachment (shell owns the
//! window surface).

use std::collections::HashSet;

use cosmic::Element;
use cosmic::Task;
use cosmic::iced::font::{Style as FontStyle, Weight as FontWeight};
use cosmic::iced::keyboard;
use cosmic::iced::widget::container::Style as ContainerStyle;
use cosmic::iced::widget::text::{Span, Wrapping};
use cosmic::iced::widget::{operation, rich_text};
use cosmic::iced::{Color, Font, Length};
use cosmic::widget::text_editor::{self, Binding, Content};
use cosmic::widget::{Column, Row, Space, button, container};
use engine::project::store::{Action, ChatMessage, Role, ToolState};
use uuid::Uuid;

use crate::chat::ChatState;
use crate::markdown::{self, MdStyle};

/// This module's text widget, pinned to cosmic's theme/renderer (the bare
/// alias would default to iced's own theme and stop converting into
/// [`Element`]s). Lifetime-generic: iced's `Text` is invariant over `'a`, so
/// every widget in one element tree must agree on a single lifetime.
type Text<'a> = cosmic::iced::widget::Text<'a, cosmic::theme::Theme, cosmic::Renderer>;

// ─── catalog strings (each constant names its en/main.ftl key) ─────────────

/// `descrivi-un-oggetto`
const DESCRIBE_AN_OBJECT: &str = "Describe an object";
/// `scrive-lo-script-cad-genera-il-modello-3d-e-la-tavola-tecnica` (`%@` is
/// the engine display name).
const ENGINE_WRITES: &str =
    "%@ writes the CAD script and generates the 3D model and the technical drawing.";
/// The engine display name (AgentLocator.swift:12 — the reducer's auth row
/// uses the same "Claude Code").
const ENGINE_NAME: &str = "Claude Code";
/// `staffa-a-l-40-40-3-mm-con-due-fori-m4`
const SUGGESTION_BRACKET: &str = "40×40×3 mm L bracket with two M4 holes";
/// `scatola-60-40-25-mm-con-coperchio-a-incastro`
const SUGGESTION_BOX: &str = "60×40×25 mm box with a snap-fit lid";
/// `distanziale-cilindrico-12-mm-alto-15-mm-con-foro-m3`
const SUGGESTION_SPACER: &str = "Cylindrical spacer Ø 12 mm, 15 mm tall, with an M3 hole";
/// `supporto-per-smartphone-inclinato-a-60`
const SUGGESTION_STAND: &str = "Smartphone stand tilted at 60°";
/// `dettagli`
const DETAILS: &str = "Details";
/// `mostra-tutto`
const SHOW_ALL: &str = "Show All";
/// `copia`
const COPY: &str = "Copy";
/// `in-corso` (ToolActivityRow stateDescription)
const TOOL_STATE_RUNNING: &str = "in progress";
/// `completato`
const TOOL_STATE_SUCCEEDED: &str = "completed";
/// `non-riuscito`
const TOOL_STATE_FAILED: &str = "failed";
/// `apri-il-terminale` (MessageView title(for:))
const ACTION_OPEN_TERMINAL: &str = "Open Terminal";
/// `chiedi-all-agente-di-correggere`
const ACTION_ASK_TO_FIX: &str = "Ask the agent to fix it";
/// `configura-l-agente`
const ACTION_CONFIGURE: &str = "Set up the agent…";
/// `descrivi-l-oggetto-o-la-modifica`
const PLACEHOLDER: &str = "Describe the object or the change…";
/// `scrivi-il-prossimo-messaggio-partir-al-termine`
const PLACEHOLDER_BUSY: &str = "Write the next message: it goes out when this one finishes…";
/// `invia`
const SEND: &str = "Send";
/// `invia-invio`
const SEND_TOOLTIP: &str = "Send (Return)";
/// `aggiungi-alla-coda`
const ADD_TO_QUEUE: &str = "Add to queue";
/// `aggiungi-alla-coda-partir-al-termine-invio`
const ADD_TO_QUEUE_TOOLTIP: &str = "Add to queue: it goes out when this one finishes (Return)";
/// `stop`
const STOP: &str = "Stop";
/// `interrompi`
const STOP_TOOLTIP: &str = "Stop (⌘.)";
/// `allega-file`
const ATTACH: &str = "Attach files";
/// `allega-immagini-o-file-di-riferimento-puoi-anche-incollarli-con-v`
const ATTACH_TOOLTIP: &str = "Attach reference images or files (you can also paste them with ⌘V)";
/// `rimuovi`
const REMOVE: &str = "Remove";
/// `in-coda-partiranno-uno-alla-volta`
const QUEUE_RUNNING: &str = "Queued, sent one at a time";
/// `in-coda-in-pausa-dopo-l-ultimo-errore`
const QUEUE_PAUSED: &str = "Queued, paused after the last error";
/// `invia-ora`
const SEND_NOW: &str = "Send now";
/// `rimuovi-dalla-coda`
const REMOVE_FROM_QUEUE: &str = "Remove from queue";
/// `solo-immagini`
const IMAGES_ONLY: &str = "Images only";
/// `lld-immagini` (`%lld images`)
const IMAGES_COUNT: &str = "images";

/// Upstream caps the collapsed tool output at 1500 CHARACTERS before the
/// "Show All" affordance (ToolActivityRow.swift:8, `output.count` /
/// `String(prefix:)`).
const OUTPUT_PREVIEW_LEN: usize = 1500;

/// The transcript scrollable's widget id — bottom-anchoring
/// (ChatView.swift:61-67).
fn transcript_id() -> cosmic::iced::widget::Id {
    cosmic::iced::widget::Id::new("chat-transcript")
}

/// The input editor's widget id — focus requests (ChatInputView.swift:59-61).
fn input_id() -> cosmic::iced::widget::Id {
    cosmic::iced::widget::Id::new("chat-input")
}

// ─── pure row model ────────────────────────────────────────────────────────

/// What the transcript shows, in order: one entry per row, plus the busy
/// spinner after a trailing user row (ChatView.swift:50-54 — busy with the
/// last row from the user means the agent hasn't opened a row yet).
#[derive(Debug, Clone, PartialEq)]
pub enum ChatEntry<'a> {
    Row(&'a ChatMessage),
    StreamingIndicator,
}

/// The transcript's row list. Order IS `chat.messages` order — never sorted,
/// never rebuilt — so streaming updates rewrite rows in place (the reducer's
/// contract) and this mapping is stable by construction.
pub fn entries<'a>(chat: &'a ChatState, busy: bool) -> Vec<ChatEntry<'a>> {
    let mut out: Vec<ChatEntry> = chat.messages.iter().map(ChatEntry::Row).collect();
    let last_is_user = chat.messages.last().is_some_and(|m| m.role == Role::User);
    if busy && last_is_user {
        out.push(ChatEntry::StreamingIndicator);
    }
    out
}

/// The collapsed tool-output preview: the first [`OUTPUT_PREVIEW_LEN`]
/// characters plus an ellipsis line, unless showing full (upstream
/// `shownOutput`, ToolActivityRow.swift:69-71 — Swift's `prefix(_:)` counts
/// characters, not bytes).
pub fn shown_output(output: &str, show_full: bool) -> String {
    if show_full || output.chars().count() <= OUTPUT_PREVIEW_LEN {
        return output.to_string();
    }
    let mut out: String = output.chars().take(OUTPUT_PREVIEW_LEN).collect();
    out.push_str("\n…");
    out
}

// ─── messages ──────────────────────────────────────────────────────────────

/// The chat pane's messages. Variants without a comment are handled entirely
/// by [`ChatPane::update`]; the commented ones are the shell's (the bridge
/// and the project model live above this module).
#[derive(Clone, Debug)]
pub enum Message {
    /// An editor interaction: applied to the pane's content, then the shell
    /// mirrors `pane.draft()` into `chat.draft` (`can_send` reads it).
    Draft(text_editor::Action),
    /// Return without shift/option, or the send button. The SHELL acts on it
    /// first (`chat.take_draft()` → bridge), then calls [`ChatPane::sent`].
    Send,
    /// The stop button while busy.
    Stop,
    /// A suggestion tapped in the empty state; the payload goes out as a
    /// user turn.
    SendSuggestion(String),
    ToggleToolRow(Uuid),
    ToggleFullOutput(Uuid),
    ToggleDetail(Uuid),
    /// A system row's action button.
    RowAction(Uuid, Action),
    /// A markdown link in an assistant row.
    OpenUrl(String),
    /// A code block's copy button.
    CopyCode(String),
    /// The paperclip.
    Attach,
    /// A staged attachment's remove button.
    Detach(String),
    RemoveQueued(Uuid),
    SendQueuedNow,
}

// ─── pane state ────────────────────────────────────────────────────────────

/// The chat pane's widget-local state — upstream's per-row `@State`
/// (`ToolActivityRow.isExpanded`/`showsFullOutput`,
/// `SystemMessageView.showsDetail`) keyed by row id, plus the input editor's
/// content (upstream binds `chat.draft`; here the editor owns the buffer and
/// the shell mirrors it into `ChatState::draft`).
#[derive(Debug, Default)]
pub struct ChatPane {
    expanded: HashSet<Uuid>,
    full_output: HashSet<Uuid>,
    detail: HashSet<Uuid>,
    input: Content,
}

impl ChatPane {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a message to the widget state. Shell-owned messages (`Send`,
    /// `Stop`, …) pass through untouched; the shell handles them before or
    /// after calling this.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Draft(action) => self.input.perform(action),
            Message::ToggleToolRow(id) => toggle(&mut self.expanded, id),
            Message::ToggleFullOutput(id) => toggle(&mut self.full_output, id),
            Message::ToggleDetail(id) => toggle(&mut self.detail, id),
            Message::CopyCode(text) => return cosmic::iced::clipboard::write(text),
            _ => {}
        }
        Task::none()
    }

    /// The input editor's current text (what `ChatState::draft` mirrors).
    pub fn draft(&self) -> String {
        self.input.text()
    }

    /// After a send consumed the draft (upstream's `takeDraft` clearing the
    /// bound field).
    pub fn sent(&mut self) {
        self.input = Content::new();
    }

    /// Bottom-anchor the transcript (upstream `defaultScrollAnchor(.bottom)`
    /// + the scroll-to-bottom on change; the shell fires it when rows land).
    pub fn scroll_to_bottom() -> Task<Message> {
        operation::snap_to_end(transcript_id())
    }

    /// Focus the input editor (upstream `inputFocusRequest`).
    pub fn focus_input() -> Task<Message> {
        operation::focus(input_id())
    }
}

fn toggle(set: &mut HashSet<Uuid>, id: Uuid) {
    if !set.insert(id) {
        set.remove(&id);
    }
}

/// A queued user turn as the queue list renders it (upstream iterates
/// `project.queue` whose items are full `ChatMessage`s; the bridge's queue
/// maps onto this — the view needs identity, preview text and image count).
#[derive(Debug, Clone)]
pub struct QueueItem {
    pub id: Uuid,
    pub text: String,
    pub images: usize,
}

// ─── view ──────────────────────────────────────────────────────────────────

/// The whole chat pane: transcript (or empty state) over the input bar
/// (ChatView.swift:9-19).
pub fn view_chat<'a>(
    pane: &'a ChatPane,
    chat: &'a ChatState,
    busy: bool,
    queue: &'a [QueueItem],
    queue_paused: bool,
) -> Element<'a, Message> {
    // The transcript area is ALWAYS a scrollable — swapping an empty-state
    // column into the slot and back lands a stateful scrollable on a
    // stateless widget tree on the first message, which iced's positional
    // tree diff cannot reconcile (panic: "Downcast on stateless state").
    let body: Element<'_, Message> = if chat.messages.is_empty() {
        container(empty_state())
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(cosmic::iced::alignment::Horizontal::Center)
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into()
    } else {
        let mut rows = Column::new().spacing(12).padding(12).width(Length::Fill);
        for entry in entries(chat, busy) {
            rows = rows.push(match entry {
                ChatEntry::Row(message) => row_for(pane, chat, message),
                ChatEntry::StreamingIndicator => streaming_indicator(),
            });
        }
        rows.into()
    };
    let transcript = cosmic::widget::scrollable::scrollable(body)
        .id(transcript_id())
        .height(Length::Fill)
        .width(Length::Fill);
    Column::new()
        .push(transcript)
        .push(input_bar(pane, chat, busy, queue, queue_paused))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// Upstream `row(for:)` (ChatView.swift:71-90).
fn row_for<'a>(
    pane: &'a ChatPane,
    chat: &'a ChatState,
    message: &'a ChatMessage,
) -> Element<'a, Message> {
    match message.role {
        Role::Tool => tool_row(pane, message),
        Role::Thinking => thinking_row(chat, message),
        // VersionMarker rows (summary rows whose detail parses as a version
        // number) are M5: without the versions model every summary row is
        // the plain interrupted-marker text (ChatView.swift:78-86).
        Role::Summary => summary_row(message),
        Role::User | Role::System | Role::Assistant => message_view(pane, message),
    }
}

// ─── rows ──────────────────────────────────────────────────────────────────

/// Upstream `MessageView` (MessageView.swift:3-34).
fn message_view<'a>(pane: &'a ChatPane, message: &'a ChatMessage) -> Element<'a, Message> {
    match message.role {
        Role::User => user_row(message),
        Role::System => system_row(pane, message),
        _ => markdown_text(&message.text),
    }
}

/// The user's bubble: attachments over the text, on a subtle rounded fill
/// (MessageView.swift:10-27).
fn user_row<'a>(message: &'a ChatMessage) -> Element<'a, Message> {
    let mut stack = Column::new().spacing(8);
    if !message.attachments.is_empty() {
        stack = stack.push(attachment_chips(&message.attachments, false));
    }
    if !message.text.is_empty() {
        stack = stack.push(body_text(message.text.as_str()));
    }
    container(stack)
        .padding(10)
        .width(Length::Fill)
        .class(subtle_style(8.0, 1.0))
        .into()
}

/// Name chips for staged/persisted attachments. Upstream renders image
/// thumbnails loaded from the project root (AttachmentThumbnail); that I/O
/// belongs to the shell, so chips carry the name here. `removable` adds the
/// draft strip's remove button (upstream attachmentStrip,
/// ChatInputView.swift:82-104).
fn attachment_chips<'a>(paths: &'a [String], removable: bool) -> Element<'a, Message> {
    let mut chips = Row::new().spacing(6);
    for path in paths {
        let name = path.rsplit('/').next().unwrap_or(path);
        let mut chip = Row::new()
            .spacing(4)
            .push(secondary_text(name.to_string()).size(12));
        if removable {
            chip = chip.push(
                button::icon(icon_handle("window-close", 12))
                    .on_press(Message::Detach(path.clone()))
                    .name(REMOVE),
            );
        }
        chips = chips.push(
            container(chip)
                .padding([2.0, 8.0])
                .class(subtle_style(6.0, 0.5)),
        );
    }
    chips.into()
}

/// Assistant text: inline markdown, mapped span by span (upstream
/// `MarkdownText`; the fence segments already flattened to code-styled spans
/// by [`markdown::to_ir`]).
fn markdown_text(md: &str) -> Element<'static, Message> {
    let spans: Vec<Span<'_, String>> = markdown::to_ir(md)
        .into_iter()
        .map(|span| {
            let mut out = Span::<String>::new(span.text);
            match span.style {
                MdStyle::Plain => {}
                MdStyle::Strong => {
                    out = out.font(Font {
                        weight: FontWeight::Bold,
                        ..cosmic::font::default()
                    });
                }
                MdStyle::Emphasis => {
                    out = out.font(Font {
                        style: FontStyle::Italic,
                        ..cosmic::font::default()
                    });
                }
                MdStyle::Code => {
                    out = out.font(cosmic::font::mono());
                }
                MdStyle::Link { url } => {
                    out = out.underline(true).link(url);
                }
            }
            out
        })
        .collect();
    rich_text(spans)
        .on_link_click(Message::OpenUrl)
        .font(cosmic::font::default())
        .wrapping(Wrapping::Word)
        .into()
}

/// Upstream `SystemMessageView` (MessageView.swift:36-78): icon + text, a
/// details disclosure for the error text, and the row's action button.
fn system_row<'a>(pane: &'a ChatPane, message: &'a ChatMessage) -> Element<'a, Message> {
    let mut stack = Column::new().spacing(8);
    let warns = message.detail.is_some();
    let head = Row::new()
        .spacing(8)
        .push(icon(
            if warns {
                "dialog-warning"
            } else {
                "dialog-information"
            },
            16,
        ))
        .push(body_text(message.text.as_str()));
    stack = stack.push(head);
    if message.detail.is_some() {
        stack = stack.push(
            button::text(DETAILS)
                .class(cosmic::theme::Button::Text)
                .on_press(Message::ToggleDetail(message.id)),
        );
        if pane.detail.contains(&message.id) {
            if let Some(detail) = &message.detail {
                stack = stack.push(code_block(detail));
            }
        }
    }
    if let Some(action) = message.action {
        stack = stack.push(
            button::text(action_title(action))
                .class(cosmic::theme::Button::Text)
                .on_press(Message::RowAction(message.id, action)),
        );
    }
    container(stack)
        .padding(10)
        .width(Length::Fill)
        .class(warn_style())
        .into()
}

/// Upstream `title(for:)` (MessageView.swift:71-77).
fn action_title(action: Action) -> &'static str {
    match action {
        Action::AskClaudeToFix => ACTION_ASK_TO_FIX,
        Action::OpenTerminal => ACTION_OPEN_TERMINAL,
        Action::ConfigureClaude => ACTION_CONFIGURE,
    }
}

/// Upstream `ThinkingRow` (ChatView.swift:156-179): secondary text with a
/// spinner while the row is the active one.
fn thinking_row<'a>(chat: &'a ChatState, message: &'a ChatMessage) -> Element<'a, Message> {
    let active = chat.active_thinking_id == Some(message.id);
    let mut line = Row::new()
        .spacing(6)
        .push(icon("brain", 14))
        .push(secondary_text(message.text.as_str()));
    if active {
        line = line.push(cosmic::widget::progress_bar::indeterminate_circular());
    }
    container(line)
        .padding([2.0, 8.0])
        .width(Length::Fill)
        .into()
}

/// Upstream's plain summary row (ChatView.swift:80-86): caption-size mono
/// secondary text.
fn summary_row<'a>(message: &'a ChatMessage) -> Element<'a, Message> {
    container(
        secondary_text(message.text.as_str())
            .font(cosmic::font::mono())
            .size(12),
    )
    .padding([0.0, 0.0, 6.0, 0.0])
    .width(Length::Fill)
    .into()
}

/// Upstream `ToolActivityRow` (ToolActivityRow.swift:12-67): collapsed
/// title+state+live preview, expanding to the accumulated input and the
/// output.
fn tool_row<'a>(pane: &'a ChatPane, message: &'a ChatMessage) -> Element<'a, Message> {
    let Some(tool) = &message.tool else {
        return body_text(message.text.as_str()).into();
    };
    let expanded = pane.expanded.contains(&message.id);

    // collapsed label: icon · title · state (tap toggles)
    let mut label = Row::new()
        .spacing(6)
        .push(icon(tool_icon(&tool.name), 16))
        .push(body_text(message.text.as_str()).width(Length::Fill));
    label = label.push(state_mark(tool.state));
    let label = cosmic::widget::mouse_area(container(label).width(Length::Fill))
        .on_press(Message::ToggleToolRow(message.id));

    let mut stack = Column::new().spacing(4).push(label);
    if let Some(preview) = tool.preview.as_deref() {
        if !preview.is_empty() && tool.state == ToolState::Running {
            stack = stack.push(
                container(
                    secondary_text(preview.to_string())
                        .font(cosmic::font::mono())
                        .size(11),
                )
                .padding([0.0, 0.0, 0.0, 22.0])
                .width(Length::Fill),
            );
        }
    }
    if expanded {
        let mut body = Column::new().spacing(6);
        if !tool.input.is_empty() {
            body = body.push(code_block(&tool.input));
        }
        if let Some(output) = &tool.output {
            if !output.is_empty() {
                let full = pane.full_output.contains(&message.id);
                body = body.push(code_block(&shown_output(output, full)));
                if output.chars().count() > OUTPUT_PREVIEW_LEN && !full {
                    body = body.push(
                        button::text(SHOW_ALL)
                            .class(cosmic::theme::Button::Text)
                            .on_press(Message::ToggleFullOutput(message.id)),
                    );
                }
            }
        }
        stack = stack.push(body);
    }
    container(stack)
        .padding([4.0, 8.0])
        .width(Length::Fill)
        .class(subtle_style(6.0, 0.35))
        .into()
}

/// Upstream `stateIcon` (ToolActivityRow.swift:85-98). The green/red are
/// upstream's explicit semantic colors, not theme-derived (cosmetic).
fn state_mark(state: ToolState) -> Element<'static, Message> {
    // the tooltip carries upstream's stateDescription (the accessibility
    // label); iced's glyphs have no label of their own
    let mark = match state {
        ToolState::Running => Element::from(cosmic::widget::progress_bar::indeterminate_circular()),
        ToolState::Succeeded => glyph("✓", STATE_OK),
        ToolState::Failed => glyph("✗", STATE_ERR),
    };
    let description = match state {
        ToolState::Running => TOOL_STATE_RUNNING,
        ToolState::Succeeded => TOOL_STATE_SUCCEEDED,
        ToolState::Failed => TOOL_STATE_FAILED,
    };
    cosmic::widget::tooltip(
        mark,
        body_text(description.to_string()),
        cosmic::widget::tooltip::Position::FollowCursor,
    )
    .into()
}

const STATE_OK: Color = Color::from_rgb(0.18, 0.66, 0.34);
const STATE_ERR: Color = Color::from_rgb(0.90, 0.32, 0.32);

fn glyph(ch: &str, color: Color) -> Element<'static, Message> {
    body_text(ch.to_string())
        .class(cosmic::theme::Text::Color(color))
        .into()
}

/// Upstream `icon` (ToolActivityRow.swift:74-83): per-tool SF Symbol →
/// freedesktop names (cosmetic — theme decides).
fn tool_icon(name: &str) -> &'static str {
    match name {
        "Bash" => "utilities-terminal",
        "Write" => "text-editor",
        "Edit" | "MultiEdit" => "document-edit",
        "Read" | "Grep" | "Glob" => "edit-find",
        "TodoWrite" => "view-list",
        _ => "system-run",
    }
}

/// Upstream `CodeBlock` (MessageView.swift:114-144): mono text on a subtle
/// rounded fill with a copy button. Upstream scrolls horizontally and caps
/// the tool input at 12 lines; iced has neither here — text wraps and the
/// full text shows (flagged for the shell's polish pass).
fn code_block<'a>(code: &str) -> Element<'a, Message> {
    let body = Row::new()
        .push(
            secondary_text(code.to_string())
                .font(cosmic::font::mono())
                .width(Length::Fill)
                .wrapping(Wrapping::Glyph),
        )
        .push(
            button::icon(icon_handle("edit-copy", 14))
                .on_press(Message::CopyCode(code.to_string()))
                .name(COPY),
        )
        .spacing(8);
    container(body)
        .padding(8)
        .width(Length::Fill)
        .class(subtle_style(6.0, 1.0))
        .into()
}

/// Upstream ChatView.swift:50-54: a small busy marker while the agent works
/// before its first row opens.
fn streaming_indicator() -> Element<'static, Message> {
    container(cosmic::widget::progress_bar::indeterminate_circular())
        .padding([0.0, 0.0, 0.0, 2.0])
        .into()
}

/// Upstream `ChatEmptyState` (ChatView.swift:107-154).
fn empty_state() -> Element<'static, Message> {
    let suggestions = [
        SUGGESTION_BRACKET,
        SUGGESTION_BOX,
        SUGGESTION_SPACER,
        SUGGESTION_STAND,
    ];
    let mut list = Column::new().spacing(6);
    for suggestion in suggestions {
        list = list.push(
            button::custom(
                // the label needs an explicit color class: under a
                // Transparent button the inherited one is fully transparent
                // (see ambient_text_class)
                container(
                    body_text(suggestion.to_string())
                        .width(Length::Fill)
                        .class(ambient_text_class()),
                )
                .padding([6.0, 10.0])
                .width(Length::Fill)
                .class(subtle_style(6.0, 0.6)),
            )
            .class(cosmic::theme::Button::Transparent)
            .on_press(Message::SendSuggestion(suggestion.to_string())),
        );
    }
    Column::new()
        .spacing(16)
        // upstream `cube.transparent`; package-x-generic is the Pop-theme
        // stand-in (preferences-desktop-3d exists in no Pop theme)
        .push(icon("package-x-generic", 40))
        .push(body_text(DESCRIBE_AN_OBJECT).size(18))
        .push(secondary_text(ENGINE_WRITES.replace("%@", ENGINE_NAME)))
        .push(list)
        // upstream caps the column at 340 pt; Fill+max keeps a narrower
        // chat pane from overflowing its bounds
        .width(Length::Fill)
        .max_width(340)
        .padding(20)
        .align_x(cosmic::iced::alignment::Alignment::Center)
        .into()
}

// ─── input bar ─────────────────────────────────────────────────────────────

/// Upstream `ChatInputView` (ChatInputView.swift:4-80) with `QueueList`
/// (ChatInputView.swift:186-248).
fn input_bar<'a>(
    pane: &'a ChatPane,
    chat: &'a ChatState,
    busy: bool,
    queue: &'a [QueueItem],
    queue_paused: bool,
) -> Element<'a, Message> {
    let mut stack = Column::new();
    stack = stack.push(cosmic::widget::divider::horizontal::default());
    if !queue.is_empty() {
        stack = stack.push(queue_list(queue, queue_paused, busy));
    }
    if !chat.draft_attachments.is_empty() {
        stack = stack.push(
            container(attachment_chips(&chat.draft_attachments, true))
                .padding([10.0, 12.0, 0.0, 12.0]),
        );
    }
    stack = stack.push(input_row(pane, chat, busy));

    // the card floats inside the pane: the outer inset keeps its rounded
    // corners off the pane edge instead of clipping against it
    container(
        container(stack)
            .width(Length::Fill)
            .padding(10)
            .class(subtle_style(8.0, 1.0)),
    )
    .width(Length::Fill)
    .padding([0.0, 6.0, 6.0, 6.0])
    .into()
}

/// Upstream `QueueList` (ChatInputView.swift:186-248).
fn queue_list<'a>(queue: &'a [QueueItem], paused: bool, busy: bool) -> Element<'a, Message> {
    let mut head = Row::new()
        .spacing(6)
        .push(icon(
            if paused {
                "media-playback-pause"
            } else {
                "document-open-recent"
            },
            14,
        ))
        .push(secondary_text(if paused {
            QUEUE_PAUSED
        } else {
            QUEUE_RUNNING
        }))
        .push(Space::new().width(Length::Fill));
    if paused && !busy {
        head = head.push(
            button::text(SEND_NOW)
                .class(cosmic::theme::Button::Text)
                .on_press(Message::SendQueuedNow),
        );
    }
    let mut items = Column::new().spacing(4);
    for (index, item) in queue.iter().enumerate() {
        let mut body = Column::new().spacing(2);
        let preview = if item.text.is_empty() {
            IMAGES_ONLY.to_string()
        } else {
            item.text.clone()
        };
        body = body.push(body_text(preview));
        if item.images > 0 {
            body = body.push(secondary_text(format!("{} {}", item.images, IMAGES_COUNT)));
        }
        items = items.push(
            container(
                Row::new()
                    .spacing(8)
                    .push(secondary_text(format!("{}", index + 1)).size(12))
                    .push(body.width(Length::Fill))
                    .push(
                        button::icon(icon_handle("window-close", 14))
                            .on_press(Message::RemoveQueued(item.id))
                            .name(REMOVE_FROM_QUEUE),
                    ),
            )
            .padding([7.0, 10.0])
            .width(Length::Fill)
            .class(subtle_style(7.0, 0.5)),
        );
    }
    container(Column::new().spacing(6).push(head).push(items))
        .padding([10.0, 10.0, 0.0, 10.0])
        .width(Length::Fill)
        .into()
}

/// The editor row: attach · input · send (ChatInputView.swift:20-57) and the
/// busy-state send area (ChatInputView.swift:116-153).
fn input_row<'a>(pane: &'a ChatPane, chat: &'a ChatState, busy: bool) -> Element<'a, Message> {
    let mut line = Row::new()
        .spacing(6)
        .align_y(cosmic::iced::alignment::Alignment::End);
    line = line.push(
        button::icon(icon_handle("mail-attachment", 14))
            .on_press(Message::Attach)
            .name(ATTACH)
            .description(ATTACH_TOOLTIP),
    );
    line = line.push(
        text_editor::text_editor(&pane.input)
            .id(input_id())
            // The id must live on exactly ONE tree level: cosmic's editor
            // wrapper (context-menu mode, default) forwards the inner
            // editor's id as its own, so both levels enter iced's
            // name-keyed NAMED state map and clobber each other's records
            // — the inner slot is left stateless and the next layout
            // panics ("Downcast on stateless state"). Collapsing the
            // wrapper keeps rip+restore symmetric; the right-click menu
            // is the casualty until libcosmic stops forwarding id().
            .context_menu(false)
            .placeholder(if busy { PLACEHOLDER_BUSY } else { PLACEHOLDER })
            .on_action(Message::Draft)
            // Return sends; shift/option+Return breaks the line
            // (ChatInputView.swift:39-47).
            .key_binding(|press| {
                if matches!(press.key, keyboard::Key::Named(keyboard::key::Named::Enter))
                    && !press.modifiers.shift()
                    && !press.modifiers.alt()
                {
                    Some(Binding::Custom(Message::Send))
                } else {
                    Binding::from_key_press(press)
                }
            })
            // iced's TextEditor takes only a pixel width (no Length::Fill),
            // so the editor sizes to its content — placeholder included
            .min_height(24)
            .max_height(140),
    );
    line = line.push(send_area(chat, busy));
    line.into()
}

/// Upstream `sendButton` (ChatInputView.swift:116-153): while busy, queue-add
/// (only with something to queue) plus stop; otherwise the send button,
/// disabled unless `can_send`.
fn send_area<'a>(chat: &'a ChatState, busy: bool) -> Element<'a, Message> {
    let mut buttons = Row::new().spacing(4);
    if busy {
        if chat.can_send() {
            buttons = buttons.push(
                button::icon(icon_handle("list-add", 15))
                    .on_press(Message::Send)
                    .name(ADD_TO_QUEUE)
                    .description(ADD_TO_QUEUE_TOOLTIP),
            );
        }
        buttons = buttons.push(
            button::icon(icon_handle("media-playback-stop", 18))
                .on_press(Message::Stop)
                .name(STOP)
                .description(STOP_TOOLTIP),
        );
    } else {
        buttons = buttons.push(
            button::icon(icon_handle("go-up", 18))
                .on_press_maybe(chat.can_send().then_some(Message::Send))
                .name(SEND)
                .description(SEND_TOOLTIP),
        );
    }
    buttons.into()
}

// ─── shared bits ───────────────────────────────────────────────────────────

/// The ambient readable text color at full strength. For text inside a
/// `Button::Transparent`: cosmic's transparent button style resolves its
/// inherited text color from a component whose every color is
/// `(0,0,0,0)` (theme TRANSPARENT_COMPONENT), and iced's `Text` uses the
/// inherited color when none is set — so un-colored text under a
/// Transparent button renders invisible. An explicit class opts back in.
fn ambient_text_class() -> cosmic::theme::Text {
    cosmic::theme::Text::Custom(|theme| cosmic::iced::widget::text::Style {
        color: Some(theme.cosmic().background(false).on.into()),
        ..Default::default()
    })
}

/// Default-body text that wraps at word boundaries. The owned binding keeps
/// iced's invariant `Text<'a>` at `'static` — a borrowed `&str` would tie the
/// widget to the row reference's lifetime.
fn body_text<'a>(body: impl Into<String>) -> Text<'a> {
    let owned: String = body.into();
    cosmic::widget::text(owned).wrapping(Wrapping::Word)
}

/// A themed icon widget by freedesktop name; size in px (cosmetic layer).
fn icon(name: &str, size: u16) -> cosmic::widget::Icon {
    cosmic::widget::icon::from_name(name).size(size).icon()
}

/// The same, as a button's icon handle.
fn icon_handle(name: &str, size: u16) -> cosmic::widget::icon::Handle {
    cosmic::widget::icon::from_name(name).size(size).handle()
}

fn secondary_text<'a>(body: impl Into<String>) -> Text<'a> {
    let owned: String = body.into();
    body_text(owned).class(secondary_text_class())
}

fn secondary_text_class() -> cosmic::theme::Text {
    cosmic::theme::Text::Custom(|theme| {
        let mut color = Color::from(theme.cosmic().background(false).on);
        color.a *= 0.65;
        cosmic::iced::widget::text::Style {
            color: Some(color),
            ..Default::default()
        }
    })
}

/// A subtle rounded fill with a hairline border — the port of upstream's
/// `.quaternary.opacity(alpha)` + separator stroke. `alpha` is upstream's
/// opacity for the surface it picked (1.0 = the solid fill); `radius`
/// approximates upstream's corner radius.
fn subtle_style(radius: f32, alpha: f32) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| {
        let mut base = Color::from(theme.cosmic().background(false).component.base);
        base.a *= alpha;
        let mut border_color = Color::from(theme.cosmic().background(false).on);
        border_color.a = 0.15;
        let r = theme.cosmic().corner_radii.radius_s;
        let scale = radius / 8.0;
        ContainerStyle {
            background: Some(cosmic::iced::Background::Color(base)),
            border: cosmic::iced::Border {
                color: border_color,
                width: 1.0,
                radius: [r[0] * scale, r[1] * scale, r[2] * scale, r[3] * scale].into(),
            },
            ..Default::default()
        }
    })
}

/// Upstream's orange system-row tint (MessageView.swift:67-68) — explicit
/// color like upstream's, faded over the theme background.
fn warn_style() -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(|theme| {
        let base = Color::from(theme.cosmic().background(false).base);
        let orange = Color::from_rgb(1.0, 0.58, 0.02);
        let blended = Color::from_rgba(
            orange.r * 0.08 + base.r * 0.92,
            orange.g * 0.08 + base.g * 0.92,
            orange.b * 0.08 + base.b * 0.92,
            1.0,
        );
        let mut border = orange;
        border.a = 0.25;
        ContainerStyle {
            background: Some(cosmic::iced::Background::Color(blended)),
            border: cosmic::iced::Border {
                color: border,
                width: 1.0,
                radius: theme.cosmic().corner_radii.radius_s.into(),
            },
            ..Default::default()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::ChatState;
    use engine::agent::session::EngineEvent;
    use engine::project::store::{Action, Role};
    use std::time::Instant;
    use uuid::Uuid;

    fn started() -> EngineEvent {
        EngineEvent::Started { session_id: None }
    }

    fn ts(text: &str) -> EngineEvent {
        EngineEvent::TextStreaming { text: text.into() }
    }

    fn tc(text: &str) -> EngineEvent {
        EngineEvent::TextConfirmed { text: text.into() }
    }

    fn user_row(text: &str) -> ChatMessage {
        ChatMessage {
            id: Uuid::new_v4(),
            role: Role::User,
            text: text.into(),
            date: time::OffsetDateTime::now_utc(),
            tool: None,
            detail: None,
            action: None,
            attachments: Vec::new(),
        }
    }

    /// Row order is state order, verbatim: streaming snapshots open rows in
    /// order, confirms rewrite rows in place (same ids, same slots), and
    /// interleaved thinking rows never reorder what came before. This is
    /// the mapping contract that keeps the transcript from shuffling under a
    /// live stream.
    #[test]
    fn row_order_stable_under_streaming() {
        let now = Instant::now();
        let mut chat = ChatState::new(Vec::new());
        chat.apply_at(&started(), now);
        chat.apply_at(&ts("He"), now);
        chat.apply_at(&ts("Hello"), now);
        chat.apply_at(&ts("A new block"), now);

        let ids = |chat: &ChatState| -> Vec<Uuid> {
            entries(chat, false)
                .into_iter()
                .filter_map(|e| match e {
                    ChatEntry::Row(m) => Some(m.id),
                    ChatEntry::StreamingIndicator => None,
                })
                .collect()
        };
        let opened = ids(&chat);
        assert_eq!(opened.len(), 2, "{:?}", chat.messages);

        // confirms land on the SAME rows, in the same slots
        chat.apply_at(&tc("Hello, revised"), now);
        chat.apply_at(&tc("A new block"), now);
        assert_eq!(ids(&chat), opened, "confirmed in place, no reorder");
        assert_eq!(
            entries(&chat, false)
                .into_iter()
                .filter_map(|e| match e {
                    ChatEntry::Row(m) => Some(m.text.clone()),
                    ChatEntry::StreamingIndicator => None,
                })
                .collect::<Vec<_>>(),
            ["Hello, revised", "A new block"]
        );

        // a later turn's thinking row appends after; streaming beneath it
        // appends too — nothing reorders
        chat.apply_at(&started(), now);
        chat.apply_at(&EngineEvent::ThinkingStarted, now);
        let with_thinking = ids(&chat);
        assert_eq!(with_thinking.len(), 3);
        assert_eq!(&with_thinking[..2], &opened[..]);
        chat.apply_at(&ts("fresh answer"), now);
        let streamed = ids(&chat);
        assert_eq!(
            streamed.len(),
            with_thinking.len() + 1,
            "a new block opens a row"
        );
        assert_eq!(
            &streamed[..with_thinking.len()],
            &with_thinking[..],
            "appended at the tail"
        );
    }

    /// The busy indicator shows only while busy AND the last row is the
    /// user's — once the agent opens any row it disappears
    /// (ChatView.swift:50-54).
    #[test]
    fn streaming_indicator_only_after_trailing_user_row() {
        let now = Instant::now();
        let mut chat = ChatState::new(Vec::new());
        chat.append(user_row("make a bracket"));

        let indicator = |chat: &ChatState, busy: bool| {
            entries(chat, busy).last() == Some(&ChatEntry::StreamingIndicator)
        };
        assert!(indicator(&chat, true), "busy after user row → indicator");
        assert!(!indicator(&chat, false), "idle → no indicator");

        chat.apply_at(&started(), now);
        chat.apply_at(&EngineEvent::ThinkingStarted, now);
        assert!(!indicator(&chat, true), "agent opened a row → gone");

        // empty transcript never shows it, busy or not
        let empty = ChatState::new(Vec::new());
        assert!(entries(&empty, true).is_empty());
    }

    /// shownOutput parity (ToolActivityRow.swift:69-71): clip at 1500
    /// CHARACTERS with an ellipsis line, full text on demand, short outputs
    /// untouched. The clip is character-based like Swift's `prefix(_:)`, so
    /// multi-byte text keeps whole characters.
    #[test]
    fn shown_output_clips_at_1500_chars() {
        let short = "done";
        assert_eq!(shown_output(short, false), "done");
        assert_eq!(shown_output(short, true), "done");

        let long = "x".repeat(1501);
        let clipped = shown_output(&long, false);
        assert_eq!(clipped.chars().count(), 1500 + "\n…".chars().count());
        assert!(clipped.ends_with("\n…"));
        assert_eq!(shown_output(&long, true), long);

        // exactly 1500 is not clipped
        let edge = "y".repeat(1500);
        assert_eq!(shown_output(&edge, false), edge);

        // 2-byte characters clip at 1500 CHARS, not 1500 bytes
        let multibyte = "é".repeat(1600);
        assert_eq!(shown_output(&multibyte, false), "é".repeat(1500) + "\n…");
    }

    /// The pane's toggle state round-trips through update messages, and the
    /// editor buffer mirrors what `draft()` reads (what `ChatState::draft`
    /// syncs from).
    #[test]
    fn pane_toggles_and_draft() {
        let mut pane = ChatPane::new();
        let id = Uuid::new_v4();
        let _ = pane.update(Message::ToggleToolRow(id));
        assert!(pane.expanded.contains(&id));
        let _ = pane.update(Message::ToggleToolRow(id));
        assert!(!pane.expanded.contains(&id));

        let _ = pane.update(Message::Draft(text_editor::Action::Edit(
            cosmic::iced::widget::text_editor::Edit::Insert('h'),
        )));
        let _ = pane.update(Message::Draft(text_editor::Action::Edit(
            cosmic::iced::widget::text_editor::Edit::Insert('i'),
        )));
        assert_eq!(pane.draft(), "hi");
        pane.sent();
        assert_eq!(pane.draft(), "");
    }

    /// A system row keeps its action through the entry mapping — the view's
    /// Open Terminal button is driven straight off the persisted field.
    #[test]
    fn system_row_carries_action_into_view_model() {
        let now = Instant::now();
        let mut chat = ChatState::new(Vec::new());
        chat.apply_at(
            &EngineEvent::TurnFinished(engine::agent::session::TurnOutcome::Failed {
                error: "api error status 401".into(),
                auth: true,
                exit_code: Some(1),
            }),
            now,
        );
        let rows: Vec<&ChatMessage> = entries(&chat, false)
            .into_iter()
            .filter_map(|e| match e {
                ChatEntry::Row(m) => Some(m),
                ChatEntry::StreamingIndicator => None,
            })
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].role, Role::System);
        assert_eq!(rows[0].action, Some(Action::OpenTerminal));
    }

    /// The pane builds in every shape: empty (the suggestion-button empty
    /// state), populated, busy, with a queue. Element construction is pure
    /// — but it instantiates every widget class the runtime would, which
    /// is where tree-shape regressions surface first (the always-scrollable
    /// hardening's lesson).
    #[test]
    fn view_chat_builds_in_every_shape() {
        let pane = ChatPane::new();
        let empty = ChatState::new(Vec::new());
        let _ = view_chat(&pane, &empty, false, &[], false);
        let _ = view_chat(&pane, &empty, true, &[], false);

        let mut busy = ChatState::new(Vec::new());
        busy.append(user_row("make a bracket"));
        let queue = vec![QueueItem {
            id: Uuid::new_v4(),
            text: "next up".into(),
            images: 1,
        }];
        let _ = view_chat(&pane, &busy, true, &queue, true);
        let _ = view_chat(&pane, &busy, false, &queue, false);
    }
}
