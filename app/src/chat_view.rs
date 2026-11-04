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
//! `Attach`/`Detach`/`RemoveQueued`/`SendQueuedNow`/`SendSuggestion`/
//! `RestoreVersion` are the
//! SHELL's — it reads `ChatState` (e.g. `take_draft()`) and drives the
//! bridge, then calls [`ChatPane::sent`] after a send consumed the draft.
//!
//! User-visible strings read through `fl!` at render time (the key names
//! the en/main.ftl entry). Upstream's SF Symbols ship as SVGs drawn after
//! the symbols ([`crate::ui::icon`]), tinted with the macOS semantic
//! colors resolved against the active theme ([`crate::ui::tone`]); text uses the macOS text
//! styles' point sizes and line heights ([`TextStyle`]).
//!
//! Version markers (summary rows naming a version) resolve against the
//! version mirror on [`ChatState`] (`versions`, `current_version`), which
//! the window keeps in sync with the bridge; their Restore link is
//! [`Message::RestoreVersion`], the shell's. Attachments render as
//! upstream's `AttachmentThumbnail`s — one widget, the draft strip at 52px
//! over the staged originals and sent rows at 64px over the project root
//! (the paste and drag-drop surfaces are the shell's; their content type
//! lives in [`crate::attachments`]).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use cosmic::Element;
use cosmic::Task;
use cosmic::iced::advanced::text::EllipsizeHeightLimit;
use cosmic::iced::alignment::{Horizontal, Vertical};
use cosmic::iced::font::{Style as FontStyle, Weight as FontWeight};
use cosmic::iced::keyboard;
use cosmic::iced::widget::container::Style as ContainerStyle;
use cosmic::iced::widget::image::Handle;
use cosmic::iced::widget::text::{Ellipsize, LineHeight, Span, Wrapping};
use cosmic::iced::widget::{operation, rich_text};
use cosmic::iced::{Alignment, Color, Font, Length};
use cosmic::widget::text_editor::{self, Binding, Content};
use cosmic::widget::{Column, Row, Space, button, container};
use engine::project::store::{Action, ChatMessage, Role, ToolState};

use crate::ui::{self, Backdrop, Corner, Symbol, Tone};
use uuid::Uuid;

use crate::attachments::{self, ClipboardContent};
use crate::chat::{self, ChatState, MarkerAffordance};
use crate::fl;
use crate::markdown::{self, MdStyle};
use crate::typography;

/// This module's text widget, pinned to cosmic's theme/renderer (the bare
/// alias would default to iced's own theme and stop converting into
/// [`Element`]s). Lifetime-generic: iced's `Text` is invariant over `'a`, so
/// every widget in one element tree must agree on a single lifetime.
type Text<'a> = cosmic::iced::widget::Text<'a, cosmic::theme::Theme, cosmic::Renderer>;

// ─── catalog strings ────────────────────────────────────────────────────────
// Every user-visible string reads through `fl!` at render time; the key
// names the en/main.ftl entry. The engine display name is data, not a
// string to translate.

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
    /// REPLACE the input buffer wholesale (the drive harness's chat
    /// command — no keyboard involved). The shell's tail sync mirrors it
    /// into `ChatState::draft` like any edit.
    SetDraft(String),
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
    /// ⌘V arrived at the editor (ChatInputView.swift:62-73's keyDown
    /// monitor): the SHELL reads the clipboard's content — files or an
    /// image attach ([`Message::AttachContent`]), anything else falls
    /// through to the editor's own text paste.
    Paste,
    /// Pasted or dropped content to stage into the draft
    /// (upstream `chat.attach(urls)`, ChatViewModel.swift:22-28 — the
    /// shell dedupes via [`ChatState::attach`]). Raw image bytes land as
    /// a temp-file path first (`Attachments.savedImage`).
    AttachContent(ClipboardContent),
    RemoveQueued(Uuid),
    SendQueuedNow,
    /// A version marker's "Ripristina" link (ChatView.swift:209-213): the
    /// shell restores that version.
    RestoreVersion(i64),
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
            Message::SetDraft(text) => self.set_draft(&text),
            Message::ToggleToolRow(id) => toggle(&mut self.expanded, id),
            Message::ToggleFullOutput(id) => toggle(&mut self.full_output, id),
            Message::ToggleDetail(id) => toggle(&mut self.detail, id),
            Message::CopyCode(text) => return cosmic::iced::clipboard::write(text),
            // the shell's (project window's update_chat)
            Message::Send
            | Message::Stop
            | Message::SendSuggestion(_)
            | Message::RowAction(_, _)
            | Message::OpenUrl(_)
            | Message::Attach
            | Message::Detach(_)
            | Message::Paste
            | Message::AttachContent(_)
            | Message::RemoveQueued(_)
            | Message::SendQueuedNow
            | Message::RestoreVersion(_) => {}
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

    /// Replace the input buffer in one step ([`Message::SetDraft`]'s arm —
    /// the drive harness's keyboard-free chat entry).
    pub fn set_draft(&mut self, text: &str) {
        self.input = Content::with_text(text);
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
/// (ChatView.swift:9-19). `root` is the project root the SENT rows'
/// `references/…` attachment names resolve against (MessageView.swift:14-16)
/// and the draft strip's originals resolve against as themselves.
pub fn view_chat<'a>(
    pane: &'a ChatPane,
    chat: &'a ChatState,
    busy: bool,
    queue: &'a [QueueItem],
    queue_paused: bool,
    root: &'a Path,
) -> Element<'a, Message> {
    // The transcript area is ALWAYS a scrollable — swapping an empty-state
    // column into the slot and back lands a stateful scrollable on a
    // stateless widget tree on the first message, which iced's positional
    // tree diff cannot reconcile (panic: "Downcast on stateless state").
    let body: Element<'_, Message> = if chat.messages.is_empty() {
        container(empty_state(chat.engine))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .into()
    } else {
        // upstream LazyVStack(spacing: 12) + .padding(12), closed by the
        // 1pt "bottom" anchor row (ChatView.swift:45-59)
        let mut rows = Column::new().spacing(12).padding(12).width(Length::Fill);
        for entry in entries(chat, busy) {
            rows = rows.push(match entry {
                ChatEntry::Row(message) => row_for(pane, chat, message, busy, root),
                ChatEntry::StreamingIndicator => streaming_indicator(),
            });
        }
        rows = rows.push(Space::new().height(Length::Fixed(1.0)));
        // Bottom anchoring for a short history (defaultScrollAnchor(.bottom),
        // ChatView.swift:61): the pane column hands its Fill child a minimum
        // height equal to the viewport, the scrollable passes that minimum
        // to its content, and a Shrink container resolves to
        // max(content, minimum) — so the rows sit at the bottom until they
        // outgrow the viewport. Longer histories scroll; the shell's
        // `scroll_to_bottom` keeps the newest row in view.
        container(rows)
            .width(Length::Fill)
            .align_y(Vertical::Bottom)
            .into()
    };
    let transcript = cosmic::widget::scrollable::scrollable(body)
        .id(transcript_id())
        .height(Length::Fill)
        .width(Length::Fill);
    Column::new()
        .push(transcript)
        .push(input_bar(pane, chat, busy, queue, queue_paused, root))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// Upstream `row(for:)` (ChatView.swift:71-90).
fn row_for<'a>(
    pane: &'a ChatPane,
    chat: &'a ChatState,
    message: &'a ChatMessage,
    busy: bool,
    root: &Path,
) -> Element<'a, Message> {
    match message.role {
        Role::Tool => tool_row(pane, message),
        Role::Thinking => thinking_row(chat, message),
        Role::Summary => match chat::version_number(message) {
            Some(number) => version_marker(chat, message, number, busy),
            None => summary_row(message),
        },
        Role::User | Role::System | Role::Assistant => message_view(pane, message, root),
    }
}

// ─── rows ──────────────────────────────────────────────────────────────────

/// Upstream `MessageView` (MessageView.swift:3-34).
fn message_view<'a>(
    pane: &'a ChatPane,
    message: &'a ChatMessage,
    root: &Path,
) -> Element<'a, Message> {
    match message.role {
        Role::User => user_row(message, root),
        Role::System => system_row(pane, message),
        _ => markdown_text(&message.text),
    }
}

/// The user's bubble (MessageView.swift:10-27): 64px attachment thumbnails
/// over the body text, padding 10, `.quaternary.opacity(0.5)` fill in an
/// 8pt rounded rectangle with a separator stroke.
fn user_row<'a>(message: &'a ChatMessage, root: &Path) -> Element<'a, Message> {
    let mut stack = Column::new().spacing(8);
    if !message.attachments.is_empty() {
        let mut thumbs = Row::new().spacing(6);
        for path in &message.attachments {
            thumbs = thumbs.push(attachment_thumbnail(path, root, 64.0));
        }
        stack = stack.push(thumbs);
    }
    if !message.text.is_empty() {
        // .textSelection(.enabled) (MessageView.swift:21)
        stack = stack.push(styled_text(message.text.as_str(), BODY, Tone::Primary).selectable());
    }
    container(stack)
        .padding(10)
        .width(Length::Fill)
        .class(ui::surface::fill(Tone::Quaternary, 0.5, Corner::S, true))
        .into()
}

/// One attachment as upstream's `AttachmentThumbnail`
/// (ChatInputView.swift:156-184): the decoded image when the resolved path
/// names one, else the file-type icon over an uppercase extension caption;
/// rounded quaternary fill, hairline border, help = the file name. ONE
/// widget serves both strips through `root` + `size`: the draft strip shows
/// its staged originals at 52px (ChatInputView.swift:82-104) and sent rows
/// the `references/…` copies at 64px (MessageView.swift:10-23) — the
/// resolution rule ([`attachments::resolve_attachment`]) treats an
/// absolute path (a legacy row's original request path) as itself.
fn attachment_thumbnail<'a>(path: &str, root: &Path, size: f32) -> Element<'a, Message> {
    let resolved = attachments::resolve_attachment(path, root);
    let file_name = path.rsplit('/').next().unwrap_or(path).to_string();
    let caption = resolved
        .extension()
        .map(|ext| ext.to_string_lossy().to_uppercase())
        .unwrap_or_default();
    // upstream: `if isImage(url), let image = NSImage(contentsOf: url)` —
    // a failed image load falls to the file-icon branch, so the decode
    // (not the extension alone) decides
    let body = if engine::agent::attachments::is_image(&resolved) {
        match decoded_thumbnail(&resolved) {
            Some(handle) => cosmic::widget::image(handle)
                .width(Length::Fixed(size))
                .height(Length::Fixed(size))
                // upstream aspectRatio(.fill) + clipShape
                .content_fit(cosmic::iced::ContentFit::Cover)
                .border_radius(Corner::S.radius(&cosmic::theme::active()))
                .into(),
            None => file_type_tile(&caption, size),
        }
    } else {
        file_type_tile(&caption, size)
    };
    let tile = ui::button::with_help(
        container(body)
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .class(ui::surface::fill(Tone::Quaternary, 1.0, Corner::S, true)),
        file_name,
        cosmic::widget::tooltip::Position::FollowCursor,
    );
    tile.into()
}

/// The non-image tile — AttachmentThumbnail's else branch
/// (ChatInputView.swift:167-175): the file-type icon at half size over the
/// uppercase extension caption at max(8, size * 0.15).
fn file_type_tile<'a>(caption: &str, size: f32) -> Element<'a, Message> {
    Column::new()
        .spacing(3)
        .push(ui::icon::named("text-x-generic", (size * 0.5) as u16))
        .push(
            styled_text(
                caption.to_string(),
                TextStyle {
                    size: (8.0f32).max(size * 0.15),
                    line_height: (8.0f32).max(size * 0.15) * 1.25,
                },
                Tone::Secondary,
            )
            .font(crate::typography::font(FontWeight::Semibold)),
        )
        .align_x(Alignment::Center)
        .into()
}

// Decoded image handles by resolved path, cached per view thread the way
// upstream's SwiftUI identity caches its `NSImage`s — a decode happens
// once per path per session, and a path that failed (unreadable,
// undecodable) stays on the file-type tile instead of retrying every
// frame. `None` = known-failed.
thread_local! {
    static THUMBNAILS: RefCell<HashMap<String, Option<Handle>>> =
        RefCell::new(HashMap::new());
}

fn decoded_thumbnail(path: &Path) -> Option<Handle> {
    let key = path.to_string_lossy().into_owned();
    THUMBNAILS
        .with(|cache| cache.borrow().get(&key).cloned())
        .flatten()
        .or_else(|| {
            let decoded = std::fs::read(path)
                .ok()
                .and_then(|bytes| image::load_from_memory(&bytes).ok())
                .map(|img| {
                    let rgba = img.to_rgba8();
                    let (width, height) = rgba.dimensions();
                    Handle::from_rgba(width, height, rgba.into_raw())
                });
            THUMBNAILS.with(|cache| cache.borrow_mut().insert(key, decoded.clone()));
            decoded
        })
}

/// The draft strip (ChatInputView.swift:82-104): the staged attachments'
/// 52px thumbnails in a horizontal scroll, spacing 8, each with its remove
/// button overlaid on the top-trailing corner, offset (5, -5) — the tile
/// sits 5px down and in from a 57px cell so the overhanging button stays
/// inside the cell's bounds (and clickable).
fn attachment_strip<'a>(paths: &'a [String], root: &Path) -> Element<'a, Message> {
    let mut tiles = Row::new().spacing(8 - 5);
    for path in paths {
        // `.black.opacity(0.6)` behind a white mark: the circle's
        // knock-out shows the tile through
        let scrim = ui::icon::icon(Symbol::XmarkCircleFill, 14.0, Tone::Primary, BACKDROP)
            .opacity(0.6)
            .class(cosmic::theme::Svg::custom(|_| {
                cosmic::iced::widget::svg::Style {
                    color: Some(Color::BLACK),
                }
            }));
        let remove = button::custom(scrim)
            .padding(0)
            .class(ui::button::plain())
            .on_press(Message::Detach(path.clone()));
        let cell = cosmic::iced::widget::stack![
            container(attachment_thumbnail(path, root, 52.0)).padding([5.0, 5.0, 0.0, 0.0]),
            container(ui::button::with_help(
                remove,
                fl!("rimuovi"),
                cosmic::widget::tooltip::Position::FollowCursor,
            ))
            .width(Length::Fill)
            .align_x(Horizontal::Right),
        ];
        tiles = tiles.push(cell);
    }
    cosmic::widget::scrollable::horizontal(tiles).into()
}

/// Assistant text (upstream `MarkdownText`, MessageView.swift:80-96): the
/// message's segments stacked with spacing 8 — prose as inline-markdown
/// body text, fenced code as a [`code_block`] card.
fn markdown_text(md: &str) -> Element<'static, Message> {
    let mut column = Column::new().spacing(8).width(Length::Fill);
    for segment in markdown::segments(md) {
        column = column.push(match segment {
            markdown::Segment::Text(spans) => markdown_spans(spans),
            markdown::Segment::Code(code) => code_block(&code, None),
        });
    }
    column.into()
}

/// One prose segment's inline spans as rich text at body size.
fn markdown_spans(ir: Vec<markdown::MdSpan>) -> Element<'static, Message> {
    let spans: Vec<Span<'_, String>> = ir
        .into_iter()
        .map(|span| {
            let mut out = Span::<String>::new(span.text);
            match span.style {
                MdStyle::Plain => {}
                MdStyle::Strong => {
                    out = out.font(crate::typography::font(FontWeight::Bold));
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
        .size(BODY.size)
        .line_height(BODY.line_height())
        .width(Length::Fill)
        .wrapping(Wrapping::Word)
        .into()
}

/// Upstream `SystemMessageView` (MessageView.swift:36-78): callout text
/// with an info (secondary) or warning (orange) symbol, a "Dettagli"
/// disclosure over the error text, and the row's action button; padding
/// 10 on an orange 8% fill with a 25% orange stroke, radius 8.
fn system_row<'a>(pane: &'a ChatPane, message: &'a ChatMessage) -> Element<'a, Message> {
    let mut stack = Column::new().spacing(8);
    let warns = message.detail.is_some();
    let head = Row::new()
        .spacing(6)
        .align_y(Alignment::Start)
        .push(if warns {
            sf_icon(Symbol::ExclamationmarkTriangleFill, 14, Tone::Warning)
        } else {
            sf_icon(Symbol::InfoCircleFill, 14, Tone::Secondary)
        })
        // .textSelection(.enabled) (MessageView.swift:45)
        .push(styled_text(message.text.as_str(), CALLOUT, Tone::Primary).selectable());
    stack = stack.push(head);
    if message.detail.is_some() {
        let open = pane.detail.contains(&message.id);
        stack = stack.push(disclosure_label(
            open,
            styled_text(fl!("dettagli"), CALLOUT, Tone::Primary).into(),
            Message::ToggleDetail(message.id),
        ));
        if open {
            if let Some(detail) = &message.detail {
                stack = stack.push(code_block(detail, None));
            }
        }
    }
    if let Some(action) = message.action {
        // .controlSize(.small): the small push button's 11pt label
        stack = stack.push(
            button::custom(styled_text(
                action_title(action),
                CAPTION_LINK,
                Tone::Primary,
            ))
            .padding([3.0, 9.0])
            .class(cosmic::theme::Button::Standard)
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
fn action_title(action: Action) -> String {
    match action {
        Action::AskClaudeToFix => fl!("chiedi-all-agente-di-correggere"),
        Action::OpenTerminal => fl!("apri-il-terminale"),
        Action::ConfigureClaude => fl!("configura-l-agente"),
    }
}

/// Upstream `ThinkingRow` (ChatView.swift:156-179): callout, secondary
/// brain symbol and text, a mini spinner while the row is the active one;
/// padding 8 horizontal, 2 vertical.
fn thinking_row<'a>(chat: &'a ChatState, message: &'a ChatMessage) -> Element<'a, Message> {
    let active = chat.active_thinking_id == Some(message.id);
    let mut line = Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(sf_icon(Symbol::Brain, 14, Tone::Secondary))
        .push(styled_text(message.text.as_str(), CALLOUT, Tone::Secondary));
    if active {
        // .controlSize(.mini) (ChatView.swift:170) — cosmic's default is a
        // 48px ring; mini is the ~12px macOS size
        line = line.push(cosmic::widget::progress_bar::indeterminate_circular().size(12.0));
    }
    container(line)
        .padding([2.0, 8.0])
        .width(Length::Fill)
        .into()
}

/// Upstream's plain summary row (ChatView.swift:80-86): caption-size mono
/// secondary text, 6pt bottom padding.
fn summary_row<'a>(message: &'a ChatMessage) -> Element<'a, Message> {
    container(
        styled_text(message.text.as_str(), CAPTION, Tone::Secondary).font(cosmic::font::mono()),
    )
    .padding([0.0, 0.0, 6.0, 0.0])
    .width(Length::Fill)
    .into()
}

/// Upstream `VersionMarker` (ChatView.swift:181-220): caption, secondary —
/// the clock-arrow symbol, the row text in mono, then "Attuale" for the
/// version the project sits on or a "Ripristina" link (disabled while
/// busy) for any other recorded version; 6pt bottom padding.
fn version_marker<'a>(
    chat: &'a ChatState,
    message: &'a ChatMessage,
    number: i64,
    busy: bool,
) -> Element<'a, Message> {
    let mut line = Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(sf_icon(Symbol::ClockArrowCirclepath, 11, Tone::Secondary))
        .push(
            styled_text(message.text.as_str(), CAPTION, Tone::Secondary).font(cosmic::font::mono()),
        )
        .push(Space::new().width(Length::Fill));
    match chat::marker_affordance(number, &chat.versions, chat.current_version, busy) {
        MarkerAffordance::None => {}
        MarkerAffordance::Current => {
            line = line.push(styled_text(fl!("attuale"), CAPTION, Tone::Secondary));
        }
        MarkerAffordance::Restore { enabled } => {
            line = line.push(link_button(
                fl!("ripristina"),
                CAPTION,
                enabled.then_some(Message::RestoreVersion(number)),
            ));
        }
    }
    container(line)
        .padding([0.0, 0.0, 6.0, 0.0])
        .width(Length::Fill)
        .into()
}

/// Upstream `ToolActivityRow` (ToolActivityRow.swift:12-67): a
/// DisclosureGroup whose label is the tool symbol (16pt frame, secondary),
/// the one-line middle-truncated title and the state mark; the live
/// preview under it while running; expanding to the input (12-line cap)
/// and output code cards. Callout text, padding 8×4, a
/// `.quaternary.opacity(0.35)` fill in a 6pt rounded rectangle, no stroke.
fn tool_row<'a>(pane: &'a ChatPane, message: &'a ChatMessage) -> Element<'a, Message> {
    let Some(tool) = &message.tool else {
        return styled_text(message.text.as_str(), CALLOUT, Tone::Primary).into();
    };
    let expanded = pane.expanded.contains(&message.id);

    let title = Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(
            container(sf_icon(tool_icon(&tool.name), 14, Tone::Secondary))
                .width(Length::Fixed(16.0))
                .align_x(Horizontal::Center),
        )
        .push(
            styled_text(message.text.as_str(), CALLOUT, Tone::Primary)
                .wrapping(Wrapping::None)
                .ellipsize(Ellipsize::Middle(EllipsizeHeightLimit::Lines(1)))
                .width(Length::Fill),
        )
        .push(state_mark(tool.state));
    let mut label = Column::new().spacing(4).push(title);
    if let Some(preview) = tool.preview.as_deref() {
        if !preview.is_empty() && tool.state == ToolState::Running {
            label = label.push(
                container(
                    styled_text(preview.to_string(), PREVIEW, Tone::Secondary)
                        .font(cosmic::font::mono())
                        .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(4))),
                )
                .padding([0.0, 0.0, 0.0, 22.0])
                .width(Length::Fill),
            );
        }
    }

    let mut stack = Column::new().push(disclosure_label(
        expanded,
        label.into(),
        Message::ToggleToolRow(message.id),
    ));
    if expanded {
        let mut body = Column::new().spacing(6).padding([4.0, 0.0, 0.0, 0.0]);
        body = body.push(code_block(&tool.input, Some(TOOL_INPUT_LINES)));
        if let Some(output) = &tool.output {
            if !output.is_empty() {
                let full = pane.full_output.contains(&message.id);
                body = body.push(code_block(&shown_output(output, full), None));
                if output.chars().count() > OUTPUT_PREVIEW_LEN && !full {
                    body = body.push(link_button(
                        fl!("mostra-tutto"),
                        CAPTION_LINK,
                        Some(Message::ToggleFullOutput(message.id)),
                    ));
                }
            }
        }
        stack = stack.push(body);
    }
    container(stack)
        .padding([4.0, 8.0])
        .width(Length::Fill)
        .class(ui::surface::fill(Tone::Quaternary, 0.35, Corner::S, false))
        .into()
}

/// A macOS DisclosureGroup label: the chevron (right when closed, down
/// when open) in a 13px leading column, then the label centered against
/// it; the whole row is the tap target, and the chevron's column holds it
/// at the AppKit disclosure row height at minimum.
fn disclosure_label<'a>(
    open: bool,
    label: Element<'a, Message>,
    on_press: Message,
) -> Element<'a, Message> {
    let chevron = container(sf_icon(
        if open {
            Symbol::ChevronDown
        } else {
            Symbol::ChevronRight
        },
        9,
        Tone::Secondary,
    ))
    .width(Length::Fixed(13.0))
    .height(Length::Fixed(DISCLOSURE_ROW))
    .align_y(Vertical::Center);
    let row = Row::new()
        .spacing(0)
        .align_y(Alignment::Center)
        .push(chevron)
        .push(container(label).width(Length::Fill));
    cosmic::widget::mouse_area(row).on_press(on_press).into()
}

/// Upstream `stateIcon` (ToolActivityRow.swift:85-98): a mini spinner while
/// running, a green checkmark on success, a red xmark otherwise. The
/// tooltip carries upstream's stateDescription (the accessibility label).
fn state_mark(state: ToolState) -> Element<'static, Message> {
    let mark = match state {
        // the running mark is .controlSize(.mini) (ToolActivityRow.swift:90)
        ToolState::Running => {
            Element::from(cosmic::widget::progress_bar::indeterminate_circular().size(12.0))
        }
        ToolState::Succeeded => sf_icon(Symbol::Checkmark, 13, Tone::Success),
        ToolState::Failed => sf_icon(Symbol::Xmark, 12, Tone::Danger),
    };
    let description = match state {
        ToolState::Running => fl!("in-corso"),
        ToolState::Succeeded => fl!("completato"),
        ToolState::Failed => fl!("non-riuscito"),
    };
    ui::button::with_help(
        mark,
        description.to_string(),
        cosmic::widget::tooltip::Position::FollowCursor,
    )
    .into()
}

/// Upstream `icon` (ToolActivityRow.swift:74-83): the per-tool SF Symbol.
fn tool_icon(name: &str) -> Symbol {
    match name {
        "Bash" => Symbol::Terminal,
        "Write" => Symbol::DocBadgePlus,
        "Edit" | "MultiEdit" => Symbol::Pencil,
        "Read" | "Grep" | "Glob" => Symbol::DocTextMagnifyingglass,
        "TodoWrite" => Symbol::Checklist,
        _ => Symbol::WrenchAndScrewdriver,
    }
}

/// Upstream `CodeBlock`, copying through the chat's [`Message::CopyCode`].
fn code_block<'a>(code: &str, line_limit: Option<usize>) -> Element<'a, Message> {
    ui::code_block::code_block(code, line_limit, Message::CopyCode(code.to_string()))
}

/// Upstream ChatView.swift:50-54: a small busy marker while the agent works
/// before its first row opens (.controlSize(.small) — the 16pt size, one
/// step above the rows' mini spinners).
fn streaming_indicator() -> Element<'static, Message> {
    container(cosmic::widget::progress_bar::indeterminate_circular().size(16.0))
        .padding([0.0, 0.0, 0.0, 2.0])
        .into()
}

/// Upstream `ChatEmptyState` (ChatView.swift:107-154): the 40pt light
/// `cube.transparent` symbol in tertiary, the title3-semibold title over
/// the callout secondary subtitle (spacing 4), then the suggestion buttons
/// — callout `sparkles` labels padded 10×6 on a `.quaternary.opacity(0.6)`
/// 6pt rounded fill, spacing 6, at most 340pt wide; spacing 16 between
/// the groups, padding 20. The engine line names the engine that will run
/// the first turn (upstream reads `AgentEngine.current` at render; the
/// port seeds ChatState's engine from the settings at window build — an
/// empty transcript means no turn ran to re-seed it).
fn empty_state(engine: engine::agent::locator::AgentKind) -> Element<'static, Message> {
    let suggestions = [
        fl!("staffa-a-l-40-40-3-mm-con-due-fori-m4"),
        fl!("scatola-60-40-25-mm-con-coperchio-a-incastro"),
        fl!("distanziale-cilindrico-12-mm-alto-15-mm-con-foro-m3"),
        fl!("supporto-per-smartphone-inclinato-a-60"),
    ];
    let mut list = Column::new().spacing(6).width(Length::Fill).max_width(340);
    for suggestion in suggestions {
        let label = Row::new()
            .spacing(8)
            .align_y(Alignment::Start)
            .push(
                container(sf_icon(Symbol::Sparkles, 14, Tone::Primary))
                    .height(Length::Fixed(CALLOUT.line_height))
                    .align_y(Vertical::Center),
            )
            .push(styled_text(suggestion.to_string(), CALLOUT, Tone::Primary).width(Length::Fill));
        list = list.push(
            button::custom(
                container(label)
                    .padding([6.0, 10.0])
                    .width(Length::Fill)
                    .class(ui::surface::fill(Tone::Quaternary, 0.6, Corner::S, false)),
            )
            .padding(0)
            .width(Length::Fill)
            .class(ui::button::plain())
            .on_press(Message::SendSuggestion(suggestion.to_string())),
        );
    }
    let heading = Column::new()
        .spacing(4)
        .align_x(Alignment::Center)
        .push(
            styled_text(fl!("descrivi-un-oggetto"), TITLE3, Tone::Primary)
                .font(crate::typography::font(FontWeight::Semibold)),
        )
        .push(
            styled_text(
                fl!(
                    "scrive-lo-script-cad-genera-il-modello-3d-e-la-tavola-tecnica",
                    agent = engine.display_name()
                ),
                CALLOUT,
                Tone::Secondary,
            )
            .align_x(Horizontal::Center),
        );
    Column::new()
        .spacing(16)
        .push(sf_icon(Symbol::CubeTransparent, 40, Tone::Tertiary))
        .push(heading)
        .push(list)
        .width(Length::Fill)
        .padding(20)
        .align_x(Alignment::Center)
        .into()
}

// ─── input bar ─────────────────────────────────────────────────────────────

/// Upstream `ChatInputView` (ChatInputView.swift:11-58): a divider, the
/// queue list and draft strip when present, then the input field inset 10
/// on every side.
fn input_bar<'a>(
    pane: &'a ChatPane,
    chat: &'a ChatState,
    busy: bool,
    queue: &'a [QueueItem],
    queue_paused: bool,
    root: &Path,
) -> Element<'a, Message> {
    let mut stack = Column::new().width(Length::Fill);
    stack = stack.push(
        container(Space::new().height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .class(ui::surface::fill(Tone::Separator, 1.0, Corner::None, false)),
    );
    if !queue.is_empty() {
        stack = stack.push(queue_list(queue, queue_paused, busy));
    }
    if !chat.draft_attachments.is_empty() {
        stack = stack.push(
            container(attachment_strip(&chat.draft_attachments, root)).padding([
                10.0 - 5.0,
                12.0,
                0.0,
                12.0,
            ]),
        );
    }
    stack = stack.push(container(input_field(pane, chat, busy)).padding(10));
    stack.into()
}

/// Upstream `QueueList` (ChatInputView.swift:186-248): the paused/active
/// caption header with "Invia ora", then the numbered callout rows on a
/// `.quaternary.opacity(0.5)` 7pt fill, padding 10×7, each with its
/// tertiary remove button; padding 10 (no bottom).
fn queue_list<'a>(queue: &'a [QueueItem], paused: bool, busy: bool) -> Element<'a, Message> {
    let mut head = Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(sf_icon(
            if paused {
                Symbol::PauseCircle
            } else {
                Symbol::Clock
            },
            13,
            Tone::Secondary,
        ))
        .push(styled_text(
            if paused {
                fl!("in-coda-in-pausa-dopo-l-ultimo-errore")
            } else {
                fl!("in-coda-partiranno-uno-alla-volta")
            },
            CAPTION,
            Tone::Secondary,
        ))
        .push(Space::new().width(Length::Fill));
    if paused && !busy {
        head = head.push(link_button(
            fl!("invia-ora"),
            CAPTION_LINK,
            Some(Message::SendQueuedNow),
        ));
    }
    let mut items = Column::new().spacing(4);
    for (index, item) in queue.iter().enumerate() {
        let mut body = Column::new().spacing(2).width(Length::Fill);
        let preview = if item.text.is_empty() {
            fl!("solo-immagini")
        } else {
            item.text.clone()
        };
        body = body.push(
            styled_text(preview, CALLOUT, Tone::Primary)
                .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(2))),
        );
        if item.images > 0 {
            body = body.push(
                Row::new()
                    .spacing(4)
                    .align_y(Alignment::Center)
                    .push(sf_icon(Symbol::Photo, 11, Tone::Secondary))
                    .push(styled_text(
                        fl!("lld-immagini", count = item.images),
                        CAPTION,
                        Tone::Secondary,
                    )),
            );
        }
        let remove = ui::button::with_help(
            button::custom(sf_icon(Symbol::XmarkCircleFill, 14, Tone::Tertiary))
                .padding(0)
                .class(ui::button::plain())
                .on_press(Message::RemoveQueued(item.id)),
            fl!("rimuovi-dalla-coda"),
            cosmic::widget::tooltip::Position::FollowCursor,
        );
        items = items.push(
            container(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Start)
                    .push(
                        container(styled_text(
                            format!("{}", index + 1),
                            CAPTION,
                            Tone::Secondary,
                        ))
                        .width(Length::Fixed(14.0))
                        .height(Length::Fixed(CALLOUT.line_height))
                        .align_x(Horizontal::Center)
                        .align_y(Vertical::Center),
                    )
                    .push(body)
                    .push(remove),
            )
            .padding([7.0, 10.0])
            .width(Length::Fill)
            .class(ui::surface::fill(Tone::Quaternary, 0.5, Corner::S, false)),
        );
    }
    let items = cosmic::widget::scrollable::vertical(items).height(Length::Shrink);
    container(
        Column::new()
            .spacing(6)
            .push(head)
            .push(container(items).max_height(150.0)),
    )
    .padding([10.0, 10.0, 0.0, 10.0])
    .width(Length::Fill)
    .into()
}

/// The input field (ChatInputView.swift:20-57): a text-background field,
/// radius 8, separator stroke (accent at 60% while focused), holding the
/// paperclip at its leading edge, the plain multi-line text field (1-6
/// lines) and the send area at its trailing edge, bottom-aligned, spacing
/// 6, padding 10 leading / 6 trailing / 6 vertical.
///
/// The editor itself draws the field (its style is the only place the
/// focus state is visible), with its text inset past the two buttons; the
/// buttons ride a transparent layer stacked over it. Clicks between the
/// buttons fall through to the editor (iced's stack passes the cursor on
/// when the upper layer's interaction there is none).
fn input_field<'a>(pane: &'a ChatPane, chat: &'a ChatState, busy: bool) -> Element<'a, Message> {
    let attach = ui::button::with_help(
        button::custom(sf_icon(Symbol::Paperclip, 14, Tone::Secondary))
            .padding(0)
            .class(ui::button::plain())
            .on_press(Message::Attach),
        fl!("allega-immagini-o-file-di-riferimento-puoi-anche-incollarli-con-v"),
        cosmic::widget::tooltip::Position::Top,
    );
    let (send, send_width) = send_area(chat, busy);
    let editor = text_editor::text_editor(&pane.input)
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
        .placeholder(if busy {
            fl!("scrivi-il-prossimo-messaggio-partir-al-termine")
        } else {
            fl!("descrivi-l-oggetto-o-la-modifica")
        })
        .on_action(Message::Draft)
        // Return sends; shift/option+Return breaks the line
        // (ChatInputView.swift:39-47). Ctrl+V routes to the shell
        // while focused — upstream's local keyDown monitor
        // (ChatInputView.swift:62-73) attaches files when the
        // clipboard holds them and lets every other paste fall
        // through to the editor.
        .key_binding(|press| {
            if matches!(press.key, keyboard::Key::Named(keyboard::key::Named::Enter))
                && !press.modifiers.shift()
                && !press.modifiers.alt()
            {
                Some(Binding::Custom(Message::Send))
            } else if let keyboard::Key::Character(ch) = &press.key {
                if ch.eq_ignore_ascii_case("v")
                    && press.modifiers.control()
                    && !press.modifiers.shift()
                    && !press.modifiers.alt()
                    && matches!(press.status, text_editor::Status::Focused { .. })
                {
                    Some(Binding::Custom(Message::Paste))
                } else {
                    Binding::from_key_press(press)
                }
            } else {
                Binding::from_key_press(press)
            }
        })
        .size(BODY.size)
        .line_height(BODY.line_height())
        // leading: padding 10 + the 14pt paperclip in its 16pt frame +
        // spacing 6; trailing: spacing 6 + the send area + padding 6
        .padding([
            FIELD_PAD_Y,
            6.0 + send_width + 6.0,
            FIELD_PAD_Y,
            10.0 + 16.0 + 6.0,
        ])
        // .lineLimit(1...6) — iced bounds the TEXT area with these and
        // adds the padding on top
        .min_height(BODY.line_height)
        .max_height(6.0 * BODY.line_height)
        .style(field_style);
    let buttons = Row::new()
        .align_y(Alignment::End)
        .push(
            container(attach)
                .width(Length::Fixed(16.0))
                .height(Length::Fixed(BODY.line_height))
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center),
        )
        .push(Space::new().width(Length::Fill))
        .push(send);
    cosmic::iced::widget::stack![
        editor,
        container(buttons)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([SEND_PAD_Y, 6.0, SEND_PAD_Y, 10.0])
            .align_y(Vertical::Bottom),
    ]
    .into()
}

/// Upstream `sendButton` (ChatInputView.swift:116-153): while busy, the
/// accent `text.badge.plus` queue-add (only with something to queue) plus
/// the 18pt `stop.circle.fill`; otherwise the 18pt `arrow.up.circle.fill`,
/// accent when `can_send`, secondary and disabled otherwise. Returns the
/// area and its width (the editor's trailing inset).
fn send_area<'a>(chat: &'a ChatState, busy: bool) -> (Element<'a, Message>, f32) {
    let glyph_button =
        |symbol: Symbol, size: u16, tone: Tone, msg: Option<Message>, tip: String| {
            Element::from(ui::button::with_help(
                button::custom(sf_icon(symbol, size, tone))
                    .padding(0)
                    .class(ui::button::plain())
                    .on_press_maybe(msg),
                tip,
                cosmic::widget::tooltip::Position::Top,
            ))
        };
    if busy {
        let mut row = Row::new().spacing(6).align_y(Alignment::Center);
        let mut width = 18.0;
        if chat.can_send() {
            row = row.push(glyph_button(
                Symbol::TextBadgePlus,
                15,
                Tone::Accent,
                Some(Message::Send),
                fl!("aggiungi-alla-coda-partir-al-termine-invio"),
            ));
            width += 15.0 + 6.0;
        }
        row = row.push(glyph_button(
            Symbol::StopCircleFill,
            18,
            Tone::Primary,
            Some(Message::Stop),
            fl!("interrompi"),
        ));
        (row.into(), width)
    } else {
        let can_send = chat.can_send();
        (
            glyph_button(
                Symbol::ArrowUpCircleFill,
                18,
                if can_send {
                    Tone::Accent
                } else {
                    Tone::Tertiary
                },
                can_send.then_some(Message::Send),
                fl!("invia-invio"),
            ),
            18.0,
        )
    }
}

// ─── shared bits ───────────────────────────────────────────────────────────

/// An upstream text style on COSMIC's scale: the size from
/// [`crate::typography`] and the line height upstream's SF metrics give
/// that style (16/13 body, 15/12 callout, 13/10 caption, 20/15 title3,
/// 14/11 for 11pt), scaled the same way. The family is the user's.
#[derive(Clone, Copy)]
struct TextStyle {
    size: f32,
    line_height: f32,
}

impl TextStyle {
    fn line_height(self) -> LineHeight {
        LineHeight::Absolute(self.line_height.into())
    }
}

const BODY: TextStyle = TextStyle {
    size: typography::BODY,
    line_height: typography::scaled(16.0),
};
const CALLOUT: TextStyle = TextStyle {
    size: typography::CALLOUT,
    line_height: typography::scaled(15.0),
};
const CAPTION: TextStyle = TextStyle {
    size: typography::CAPTION,
    line_height: typography::scaled(13.0),
};
/// `.buttonStyle(.link).controlSize(.small)` — the small control font.
const CAPTION_LINK: TextStyle = TextStyle {
    size: typography::scaled(11.0),
    line_height: typography::scaled(14.0),
};
const TITLE3: TextStyle = TextStyle {
    size: typography::TITLE3,
    line_height: typography::scaled(20.0),
};
/// The running tool's live preview (ToolActivityRow.swift:45, 10.5pt mono).
const PREVIEW: TextStyle = TextStyle {
    size: typography::scaled(10.5),
    line_height: typography::scaled(13.0),
};

/// The tool input card's `lineLimit(12)` (ToolActivityRow.swift:16).
const TOOL_INPUT_LINES: usize = 12;
/// An AppKit disclosure row's height.
const DISCLOSURE_ROW: f32 = 24.0;
/// The input field's vertical text inset (upstream: 6pt around the field's
/// text line, ChatInputView.swift:51, which lands the field at 32pt with
/// macOS metrics).
const FIELD_PAD_Y: f32 = 8.0;
/// The send glyph's inset inside the field: centered on one body line.
const SEND_PAD_Y: f32 = (BODY.line_height + 2.0 * FIELD_PAD_Y - 18.0) / 2.0;

/// The chat pane sits on the plain window background.
const BACKDROP: Backdrop = Backdrop::Plain;

/// Text at a macOS text style in a tone, wrapping at word boundaries. The
/// owned binding keeps iced's invariant `Text<'a>` at `'static` — a
/// borrowed `&str` would tie the widget to the row reference's lifetime.
fn styled_text<'a>(body: impl Into<String>, style: TextStyle, tone: Tone) -> Text<'a> {
    let owned: String = body.into();
    cosmic::widget::text(owned)
        .size(style.size)
        .line_height(style.line_height())
        .wrapping(Wrapping::Word)
        .class(tone.text(BACKDROP))
}

/// A `.buttonStyle(.link)` button: accent text, no chrome, dimmed and
/// inert when `on_press` is `None`.
fn link_button<'a>(
    label: String,
    style: TextStyle,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let tone = ui::button::content_tone(Tone::Accent, &on_press);
    ui::button::borderless(styled_text(label, style, tone), on_press).into()
}

/// A symbol in a `size` px cell. The chat's glyphs are sized on a 16-unit
/// grid; symbols the registry crops tighter draw at the grid glyph's
/// height, centered in the cell.
fn sf_icon<'a>(symbol: Symbol, size: u16, tone: Tone) -> Element<'a, Message> {
    let cell = f32::from(size);
    let height = cell
        * match symbol {
            // a 9-unit chevron on the grid; the registry's spans 11 of 16
            Symbol::ChevronRight => 9.0 / 11.0,
            // a 12-unit clock on the grid; the registry's spans 0.9 of its box
            Symbol::ClockArrowCirclepath => 12.0 / 16.0 / 0.9,
            // an 8-unit cross on the grid; the registry's spans 9
            Symbol::Xmark => 8.0 / 9.0,
            _ => 1.0,
        };
    ui::icon::in_cell(symbol, height, cell, tone, BACKDROP)
}

/// The focused input field's accent stroke: the focus ring softened so it
/// reads as an edge, not a fill.
const FOCUS_RING_ALPHA: f32 = 0.6;

fn field_style(theme: &cosmic::Theme, status: text_editor::Status) -> text_editor::Style {
    let stroke = match status {
        text_editor::Status::Focused { .. } => Color {
            a: FOCUS_RING_ALPHA,
            ..Tone::Accent.solid(theme)
        },
        _ => Tone::Separator.color(theme, BACKDROP),
    };
    text_editor::Style {
        background: ui::surface::text_background(theme).into(),
        border: cosmic::iced::Border {
            color: stroke,
            width: 1.0,
            radius: Corner::S.radius(theme),
        },
        placeholder: Tone::Tertiary.color(theme, BACKDROP),
        value: Tone::Primary.color(theme, BACKDROP),
        selection: ui::tone::selection(theme),
    }
}

fn warn_style() -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(|theme| {
        let orange = Tone::Warning.solid(theme);
        ContainerStyle {
            background: Some(cosmic::iced::Background::Color(Color { a: 0.08, ..orange })),
            border: cosmic::iced::Border {
                color: Color { a: 0.25, ..orange },
                width: 1.0,
                radius: Corner::S.radius(theme),
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

        // SetDraft replaces the buffer wholesale (the drive harness's
        // keyboard-free entry), and a later sent() still clears it
        let _ = pane.update(Message::SetDraft("animate the cap".into()));
        assert_eq!(pane.draft(), "animate the cap");
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
    /// state), populated, busy, with a queue, with a staged draft strip,
    /// and with sent-row attachment thumbnails over both storage forms —
    /// relative `references/…` copy names and the absolute originals
    /// legacy rows carry. Element construction is pure — but it
    /// instantiates every widget class the runtime would (scrollables,
    /// tooltips, image widgets), which is where tree-shape regressions
    /// surface first.
    #[test]
    fn view_chat_builds_in_every_shape() {
        let root = std::env::temp_dir().join(format!("cad-chatview-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("references")).unwrap();
        // a decodable image the thumbnail widget can load, and a
        // non-image file that takes the file-type tile
        let png = root.join("references").join("1790000000-1-sketch.png");
        let tiny = image::RgbaImage::from_pixel(4, 4, image::Rgba([12, 34, 56, 255]));
        tiny.save(&png).unwrap();
        std::fs::write(root.join("references").join("1790000000-2-notes.txt"), b"x").unwrap();

        let pane = ChatPane::new();
        let empty = ChatState::new(Vec::new());
        let _ = view_chat(&pane, &empty, false, &[], false, &root);
        let _ = view_chat(&pane, &empty, true, &[], false, &root);

        // a user row with both attachment forms: the relative copy names
        // current rows carry, and an absolute original (the legacy shape)
        let mut with_attachments = user_row("model this");
        with_attachments.attachments = vec![
            "references/1790000000-1-sketch.png".into(),
            "references/1790000000-2-notes.txt".into(),
            png.to_string_lossy().into_owned(),
            "/tmp/gone-missing.png".into(),
        ];
        let mut chat = ChatState::new(vec![with_attachments]);
        chat.draft_attachments = vec![
            "references/1790000000-1-sketch.png".into(),
            "/tmp/CAD Studio Attachments/Pasted image x.png".into(),
        ];
        let queue = vec![QueueItem {
            id: Uuid::new_v4(),
            text: "next up".into(),
            images: 1,
        }];
        let _ = view_chat(&pane, &chat, true, &queue, true, &root);
        let _ = view_chat(&pane, &chat, false, &queue, false, &root);

        // the decoded thumbnail cached the real PNG and recorded the
        // missing file as known-failed (the tile, not a retry)
        assert!(decoded_thumbnail(&png).is_some());
        assert!(decoded_thumbnail(Path::new("/tmp/gone-missing.png")).is_none());
    }

    /// The three spinner-bearing rows construct together — the active
    /// thinking row, a running tool row's state mark, and the busy
    /// streaming indicator after a trailing user row — pinning the
    /// construction path each `.size()` call rides on (the sizes themselves
    /// are upstream's controlSize equivalents; the element tree carries no
    /// readable size to assert).
    #[test]
    fn spinner_rows_construct() {
        let root = std::env::temp_dir().join(format!("cad-chatview-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let now = Instant::now();
        let mut chat = ChatState::new(Vec::new());
        chat.apply_at(&started(), now);
        chat.apply_at(&EngineEvent::ThinkingStarted, now);
        chat.apply_at(
            &EngineEvent::ToolStarted {
                id: "tool-1".into(),
                name: "Bash".into(),
                title: engine::agent::tool_summary::ToolAction::ExamineFiles.into(),
            },
            now,
        );
        chat.append(user_row("make the wall 4 mm"));

        let pane = ChatPane::new();
        let _ = view_chat(&pane, &chat, true, &[], false, &root);
    }

    /// Version markers, fenced code cards and the plain summary caption
    /// all build: a transcript holding a current marker, a restorable one
    /// (idle and busy), an unknown one, an interrupted marker, and an
    /// assistant row with a fence.
    #[test]
    fn markers_and_code_cards_construct() {
        let root = std::env::temp_dir().join(format!("cad-chatview-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let summary = |text: &str, detail: Option<&str>| ChatMessage {
            role: Role::Summary,
            detail: detail.map(str::to_string),
            ..user_row(text)
        };
        let assistant = ChatMessage {
            role: Role::Assistant,
            ..user_row("Ecco:\n```python\nprint(1)\n```\nfatto")
        };
        let mut chat = ChatState::new(vec![
            summary("Versione 1", Some("1")),
            summary("Versione 2", Some("2")),
            summary("Versione 9", Some("9")),
            summary("Interrotto", None),
            assistant,
        ]);
        chat.versions = vec![1, 2];
        chat.current_version = Some(2);
        let pane = ChatPane::new();
        let _ = view_chat(&pane, &chat, false, &[], false, &root);
        let _ = view_chat(&pane, &chat, true, &[], false, &root);
    }
}
