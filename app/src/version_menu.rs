//! The version history menu (upstream VersionMenu.swift): the toolbar's
//! clock button whose dropdown lists every recorded version newest-first,
//! checkmarks the current one, and restores on selection.
//!
//! Split like the other widget modules: the entry projection
//! ([`menu_entries`], [`prompt_summary`]) and the subtitle's date
//! formatting ([`relative`], [`abbreviated`]) are pure; [`version_tree`]
//! only maps that state onto a libcosmic menu-bar root. The shell owns the
//! restore itself — a leaf press emits the shell's own message through the
//! `on_restore` closure, exactly how the parameters overlay's picker
//! reports through its message.
//!
//! User-visible strings are the `app/i18n/en/main.ftl` catalog values; each
//! constant names its key (the repo-wide `fl!()` swap is pending, the
//! landed modules' convention). Icons are freedesktop-theme approximations
//! of upstream SF Symbols (`clock.arrow.circlepath` →
//! `document-open-recent`, `checkmark` → `object-select`).

use time::OffsetDateTime;

use engine::project::versions::ProjectVersion;

// ─── catalog strings (each constant names its en/main.ftl key) ─────────────

/// `versione-lld-lld` (VersionMenu.swift:17/19).
const VERSION_LABEL: &str = "Version %lld · %@";
/// `nessuna-versione-salvata` (VersionMenu.swift:9).
const NO_SAVED_VERSIONS: &str = "No Saved Versions";
/// `disponibili-al-termine-della-generazione` (VersionMenu.swift:11).
const AVAILABLE_WHEN_DONE: &str = "Available when generation finishes";
/// `torna-a-una-versione` (VersionMenu.swift:11).
const GO_BACK_TO_A_VERSION: &str = "Go back to a version";
/// `versioni` — the button's accessibility label (VersionMenu.swift:35).
const VERSIONS: &str = "Versions";
/// `storico-delle-versioni` (VersionMenu.swift:37).
const VERSION_HISTORY: &str = "Version history";

/// Upstream's `clock.arrow.circlepath` (VersionMenu.swift:29).
const CLOCK_ICON: &str = "document-open-recent";
/// Upstream's `checkmark` (VersionMenu.swift:17).
const CHECK_ICON: &str = "object-select";

/// The dropdown's width — the two-line entries (label + subtitle) need
/// more than the menu bar's 150 px default.
const MENU_WIDTH: u16 = 340;

// ─── the entry projection (pure) ────────────────────────────────────────────

/// One version as the menu renders it (upstream renders the
/// `ProjectVersion` rows directly; the projection exists so the pure half
/// is testable without widgets).
#[derive(Debug, Clone, PartialEq)]
pub struct VersionEntry {
    pub number: i64,
    pub created_at: OffsetDateTime,
    pub prompt_summary: String,
    pub is_current: bool,
}

/// The first line of a version's prompt, capped at 60 characters with an
/// ellipsis (VersionMenu.swift:40-43). Character-wise, like Swift's
/// `prefix(60)`.
pub fn prompt_summary(prompt: &str) -> String {
    let line = prompt.split('\n').next().unwrap_or_default();
    if line.chars().count() > 60 {
        let capped: String = line.chars().take(60).collect();
        format!("{capped}…")
    } else {
        line.to_string()
    }
}

/// The menu's rows: versions newest-first (upstream `.reversed()` over the
/// ascending history), each with its summarized prompt and the current one
/// marked.
pub fn menu_entries(versions: &[ProjectVersion], current: Option<i64>) -> Vec<VersionEntry> {
    versions
        .iter()
        .rev()
        .map(|version| VersionEntry {
            number: version.number,
            created_at: version.created_at,
            prompt_summary: prompt_summary(&version.prompt),
            is_current: Some(version.number) == current,
        })
        .collect()
}

/// The relative half of an entry's subtitle (SwiftUI
/// `.formatted(.relative(presentation: .named))`): "just now", "5 minutes
/// ago", "yesterday", "4 days ago". Pure in `now` so the formatter is
/// testable; distances are non-negative (a future timestamp reads as
/// "just now").
pub fn relative(at: OffsetDateTime, now: OffsetDateTime) -> String {
    let seconds = (now - at).whole_seconds().max(0);
    match seconds {
        0..=59 => "just now".to_string(),
        60..=119 => "1 minute ago".to_string(),
        120..=3_599 => format!("{} minutes ago", seconds / 60),
        3_600..=7_199 => "1 hour ago".to_string(),
        7_200..=86_399 => format!("{} hours ago", seconds / 3600),
        86_400..=172_799 => "yesterday".to_string(),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

/// The date half of an entry's subtitle (`.formatted(date: .abbreviated,
/// time: .shortened)`): "Oct 13, 2026 at 3:24 PM" — abbreviated month,
/// 12-hour clock. UTC: the port has no local-offset lookup, and the
/// history stores UTC stamps.
pub fn abbreviated(at: OffsetDateTime) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = MONTHS[usize::from(u8::from(at.month())) - 1];
    let hour12 = match at.hour() % 12 {
        0 => 12, // midnight and noon both read 12
        hour => hour,
    };
    let meridiem = if at.hour() < 12 { "AM" } else { "PM" };
    format!(
        "{} {}, {} at {}:{:02} {}",
        month,
        at.day(),
        at.year(),
        hour12,
        at.minute(),
        meridiem
    )
}

/// An entry's subtitle: the relative distance and the abbreviated date,
/// joined by upstream's `·` (VersionMenu.swift:21).
fn subtitle(entry: &VersionEntry, now: OffsetDateTime) -> String {
    format!(
        "{} · {}",
        relative(entry.created_at, now),
        abbreviated(entry.created_at)
    )
}

// ─── the widget ─────────────────────────────────────────────────────────────

/// The toolbar's version menu as a menu-bar root (VersionMenu.swift): a
/// clock button labeled `v{current}` when one is set, whose dropdown is
/// the empty notice or the section (busy-titled while working) with one
/// two-line entry per version — label line, checkmark on the current one,
/// subtitle below — each inert while busy (upstream `.disabled(isBusy)`).
///
/// A menu-bar root rather than a hand-rolled popover so the dropdown
/// anchors under the button and closes on outside click for free. The
/// shell passes its own message constructor; the returned tree is dropped
/// into the same menu bar as the command roots (with
/// [`cosmic::widget::menu::ItemHeight::Dynamic`] — the two-line leaves
/// carry their own heights).
pub fn version_tree<Message>(
    entries: &[VersionEntry],
    current: Option<i64>,
    busy: bool,
    on_restore: impl Fn(i64) -> Message + Clone + 'static,
) -> cosmic::widget::menu::Tree<Message>
where
    Message: Clone + 'static,
{
    use cosmic::widget::menu::Tree;
    use cosmic::widget::{button, container, text};

    let children: Vec<Tree<Message>> = if entries.is_empty() {
        let empty: cosmic::Element<'static, Message> =
            container(text(NO_SAVED_VERSIONS.to_string()).size(12))
                .padding([8, 16])
                .into();
        vec![Tree::new(empty)]
    } else {
        let header: cosmic::Element<'static, Message> = container(text(
            if busy {
                AVAILABLE_WHEN_DONE
            } else {
                GO_BACK_TO_A_VERSION
            }
            .to_string(),
        ))
        .padding([8, 16])
        .into();
        let mut list = vec![Tree::new(header)];
        list.extend(
            entries
                .iter()
                .map(|entry| entry_leaf(entry, busy, utc_now(), on_restore.clone())),
        );
        list
    };

    let mut label = cosmic::widget::Row::new()
        .spacing(4)
        .push(cosmic::widget::icon::from_name(CLOCK_ICON).size(16).icon());
    if let Some(current) = current {
        label = label.push(
            text(format!("v{current}"))
                .size(12)
                .font(cosmic::font::mono()),
        );
    }
    let root_button = button::custom(container(label).padding([4, 10]))
        .class(cosmic::theme::Button::MenuRoot)
        .name(VERSIONS);
    let root: cosmic::Element<'static, Message> = cosmic::widget::tooltip(
        root_button,
        text(VERSION_HISTORY),
        cosmic::widget::tooltip::Position::FollowCursor,
    )
    .into();
    Tree::with_children(root, children).width(MENU_WIDTH)
}

/// One version's leaf: the label line (checkmark + "Version N · summary")
/// over the subtitle (relative · abbreviated date), pressing — unless busy
/// — restores that version.
fn entry_leaf<Message>(
    entry: &VersionEntry,
    busy: bool,
    now: OffsetDateTime,
    on_restore: impl Fn(i64) -> Message + Clone + 'static,
) -> cosmic::widget::menu::Tree<Message>
where
    Message: Clone + 'static,
{
    use cosmic::widget::menu::Tree;
    use cosmic::widget::{button, text};

    let restore = on_restore.clone();
    let on_press = (!busy).then(move || restore(entry.number));
    let mut head = cosmic::widget::Row::new().spacing(6);
    if entry.is_current {
        head = head.push(cosmic::widget::icon::from_name(CHECK_ICON).size(12).icon());
    }
    head = head.push(text(
        VERSION_LABEL
            .replace("%lld", &entry.number.to_string())
            .replace("%@", &entry.prompt_summary),
    ));
    let body = cosmic::widget::Column::new()
        .spacing(2)
        .push(head)
        .push(text(subtitle(entry, now)).size(11).class(dim_subtitle()))
        .padding([6, 16]);
    let leaf: cosmic::Element<'static, Message> = button::custom(body)
        .class(cosmic::theme::Button::MenuItem)
        .on_press_maybe(on_press)
        .width(cosmic::iced::Length::Fill)
        .into();
    Tree::new(leaf)
}

/// The subtitle's dimmed text (the pane modules' shared secondary style;
/// kept local so modules stay self-contained, the house note).
fn dim_subtitle() -> cosmic::theme::Text {
    cosmic::theme::Text::Custom(|theme| {
        let mut color = cosmic::iced::Color::from(theme.cosmic().background(false).on);
        color.a *= 0.65;
        cosmic::iced::widget::text::Style {
            color: Some(color),
            ..Default::default()
        }
    })
}

fn utc_now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}
