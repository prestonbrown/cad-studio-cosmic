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

/// `versione-lld-2` (VersionMenu.swift:17/19).
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
/// The toolbar's clock button (upstream's VersionMenu trigger): clock icon
/// plus the current version's tag, opening the history card. NOT a
/// menu-bar root — cosmic's menu overlay corrupts widget state when the
/// item set changes between rebuilds (a version landing mid-turn turned
/// the closed menu's item texts into stateless shells; the next overlay
/// layout panicked "Downcast widget state" at iced tree.rs:507 — the
/// same corruption class as the text-editor id bug, through menu_inner's
/// RcWrapper). The static command roots are safe in the bar; anything
/// whose items change lives in a popover like this one.
pub fn version_button<Message: Clone + 'static>(
    current: Option<i64>,
    open: bool,
    on_toggle: Message,
) -> cosmic::Element<'static, Message> {
    use cosmic::widget::{button, text};
    let mut label = cosmic::widget::Row::new()
        .spacing(4)
        .push(cosmic::widget::icon::from_name(CLOCK_ICON).size(14).icon());
    if let Some(current) = current {
        label = label.push(
            text(format!("v{current}"))
                .size(12)
                .font(cosmic::font::mono()),
        );
    }
    let root = button::custom(label.padding([4, 10]))
        .class(if open {
            cosmic::theme::Button::Suggested
        } else {
            cosmic::theme::Button::Standard
        })
        .on_press(on_toggle)
        .name(VERSIONS);
    cosmic::widget::tooltip(
        root,
        cosmic::widget::text(VERSION_HISTORY),
        cosmic::widget::tooltip::Position::FollowCursor,
    )
    .into()
}

/// The history card the popover shows: header, then one row per version
/// newest-first — the label line (checkmark + "Version N · summary") over
/// the subtitle (relative · abbreviated date), pressing — unless busy —
/// restores that version. `now` is the CALLER-supplied clock, frozen
/// while the card is open: a live per-frame time-ago would rebuild every
/// row's text every second.
pub fn view_versions<Message: Clone + 'static>(
    entries: &[VersionEntry],
    busy: bool,
    now: OffsetDateTime,
    on_restore: impl Fn(i64) -> Message + Clone + 'static,
) -> cosmic::Element<'static, Message> {
    use cosmic::iced::Length;
    use cosmic::widget::{button, container, scrollable, text};

    let mut list = cosmic::widget::Column::new().spacing(2);
    if entries.is_empty() {
        list = list.push(
            container(
                cosmic::widget::Column::new()
                    .spacing(4)
                    .push(text(NO_SAVED_VERSIONS.to_string()).size(12))
                    .push(
                        text(AVAILABLE_WHEN_DONE.to_string())
                            .size(11)
                            .class(dim_subtitle()),
                    ),
            )
            .padding([10, 16]),
        );
    } else {
        list = list.push(
            container(text(
                if busy {
                    AVAILABLE_WHEN_DONE
                } else {
                    GO_BACK_TO_A_VERSION
                }
                .to_string(),
            ))
            .padding([10, 16, 6, 16]),
        );
        for entry in entries {
            let restore = on_restore.clone();
            let on_press = (!busy).then(move || restore(entry.number));
            let mut head = cosmic::widget::Row::new().spacing(6);
            if entry.is_current {
                head =
                    head.push(cosmic::widget::icon::from_name(CHECK_ICON).size(12).icon());
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
            list = list.push(
                button::custom(body)
                    .class(cosmic::theme::Button::Standard)
                    .on_press_maybe(on_press)
                    .width(Length::Fill),
            );
        }
    }
    container(
        scrollable(
            cosmic::widget::Column::new()
                .push(list)
                .width(Length::Fixed(MENU_WIDTH as f32)),
        )
        .height(Length::Shrink),
    )
    .max_height(360.0)
    .class(cosmic::theme::Container::Dialog(false))
    .into()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(number: i64, minutes_ago: i64) -> VersionEntry {
        VersionEntry {
            number,
            created_at: OffsetDateTime::now_utc() - time::Duration::minutes(minutes_ago),
            prompt_summary: "a bracket".into(),
            is_current: false,
        }
    }

    /// SwiftUI's relative presentation buckets: "just now" under a minute,
    /// named units above (VersionMenu.swift renders `.relative(presentation:
    /// .named)`).
    #[test]
    fn relative_buckets_like_swiftui() {
        let now = OffsetDateTime::now_utc();
        assert_eq!(relative(now - time::Duration::seconds(30), now), "just now");
        assert_eq!(relative(now - time::Duration::minutes(5), now), "5 minutes ago");
        assert_eq!(relative(now - time::Duration::hours(2), now), "2 hours ago");
        // a future timestamp clamps to "just now" (a clock skew guard)
        assert_eq!(relative(now + time::Duration::minutes(3), now), "just now");
    }

    /// The subtitle carries both halves: "5 minutes ago · Oct 13, 2026 at
    /// 3:24 PM" (VersionMenu.swift:19-21).
    #[test]
    fn subtitle_joins_relative_and_abbreviated() {
        let e = entry(1, 5);
        let s = subtitle(&e, OffsetDateTime::now_utc());
        assert!(s.starts_with("5 minutes ago · "), "{s}");
        assert!(s.contains(", 202"), "abbreviated date present: {s}");
    }

    /// The card builds in every shape: empty, idle, busy (Element
    /// construction is pure; the tree-corruption history makes view-build
    /// tests load-bearing).
    #[test]
    fn card_builds_in_every_shape() {
        let now = OffsetDateTime::now_utc();
        let _ = view_versions(&[], false, now, |n| n);
        let _ = view_versions(&[], true, now, |n| n);
        let mut current = entry(2, 1);
        current.is_current = true;
        let _ = view_versions(&[current.clone(), entry(1, 300)], false, now, |n| n);
        let _ = view_versions(&[current, entry(1, 300)], true, now, |n| n);
        let _ = version_button(None, false, ());
        let _ = version_button(Some(2), true, ());
    }
}
