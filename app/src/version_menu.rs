//! The version history menu (upstream VersionMenu.swift): the toolbar's
//! clock button whose dropdown lists every recorded version newest-first,
//! checkmarks the current one, and restores on selection.
//!
//! Split like the other widget modules: the entry projection
//! ([`menu_entries`], [`prompt_summary`]) and the subtitle's date
//! formatting ([`relative`], [`abbreviated`]) are pure; [`version_button`]
//! and [`view_versions`] only map that state onto widgets. The shell owns the
//! restore itself — a leaf press emits the shell's own message through the
//! `on_restore` closure, exactly how the parameters overlay's picker
//! reports through its message.
//!
//! User-visible strings read through `fl!` at render time (the key names
//! the en/main.ftl entry); the relative/abbreviated date halves stay
//! hand-formatted English — upstream gets them from SwiftUI's
//! locale-aware formatters, which no catalog key backs. Icons are the
//! project window's SF-Symbol-shaped glyphs (`clock.arrow.circlepath`,
//! `chevron.down`, `checkmark`).

use time::OffsetDateTime;

use engine::project::versions::ProjectVersion;

use crate::fl;

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

/// An entry's subtitle: the relative distance and the abbreviated date
/// (VersionMenu.swift:21) formatted through `version-timestamps` (the
/// renamed format-only `%@ · %@` key).
fn subtitle(entry: &VersionEntry, now: OffsetDateTime) -> String {
    fl!(
        "version-timestamps",
        relative = relative(entry.created_at, now),
        absolute = abbreviated(entry.created_at)
    )
}

// ─── the widget ─────────────────────────────────────────────────────────────

/// The menu card's measured geometry (version-menu.png, halved to the 1x
/// basis): the card's top-left sits 115 px left of and 38 px below the
/// pill's top-left; 5 px top inset, a 23 px section header, 34 px rows,
/// 14.5 px text inset.
pub const CARD_OFFSET: (f32, f32) = (-115.0, 38.0);
const CARD_INSET_TOP: f32 = 5.0;
const HEADER_HEIGHT: f32 = 23.0;
const ROW_HEIGHT: f32 = 34.0;
const TEXT_INSET: f32 = 14.5;

/// The toolbar's version menu label (VersionMenu.swift:27-35) in its
/// bordered toolbar pill: `clock.arrow.circlepath`, the current version's
/// "v<N>" (monospaced digits), and the menu's `chevron.down` indicator —
/// 70×27, 8 px side insets, the measured 6/8 px gaps. Pressing opens the
/// history card. NOT a menu-bar root — cosmic's menu overlay corrupts
/// widget state when the item set changes between rebuilds
/// (docs/research/libcosmic-menu-dynamic-items-bug.md); anything whose
/// items change lives in a popover like this one.
pub fn version_button<Message: Clone + 'static>(
    current: Option<i64>,
    open: bool,
    on_toggle: Message,
) -> cosmic::Element<'static, Message> {
    use crate::windows::project::chrome::{self, Tone};
    use cosmic::iced::{Alignment, Length};
    use cosmic::widget::{Row, Space, button, text};
    let mut label = Row::new().align_y(Alignment::Center).push(chrome::glyph(
        chrome::CLOCK_ARROW,
        (18.0, 18.0),
        Tone::Toolbar,
    ));
    if let Some(current) = current {
        label = label.push(Space::new().width(Length::Fixed(6.0))).push(
            text(fl!("v-lld", version = current))
                .size(13)
                .class(chrome::text_class(Tone::Toolbar)),
        );
    }
    let label = label
        .push(Space::new().width(Length::Fixed(8.0)))
        .push(chrome::glyph(
            chrome::CHEVRON_DOWN,
            (8.0, 5.0),
            Tone::Toolbar,
        ))
        .padding([0, 8])
        .height(Length::Fixed(27.0));
    let root = button::custom(label)
        .padding(0)
        .class(chrome::button_class(true, 8.0))
        .selected(open)
        .on_press(on_toggle)
        .name(fl!("versioni"));
    cosmic::widget::tooltip(
        root,
        text(fl!("storico-delle-versioni")).size(12),
        cosmic::widget::tooltip::Position::Bottom,
    )
    .into()
}

/// The history card the version button opens, laid out like upstream's
/// `Menu` (VersionMenu.swift:7-25): the section header (busy-titled while
/// working), then one row per version newest-first — the label line
/// ("Version N · summary", the current one checkmarked in the leading
/// gutter) over the subtitle (relative · abbreviated date); pressing —
/// unless busy — restores that version. `now` is the CALLER-supplied
/// clock, frozen while the card is open: a live per-frame time-ago would
/// rebuild every row's text every second.
pub fn view_versions<Message: Clone + 'static>(
    entries: &[VersionEntry],
    busy: bool,
    now: OffsetDateTime,
    width: f32,
    on_restore: impl Fn(i64) -> Message + Clone + 'static,
) -> cosmic::Element<'static, Message> {
    use crate::windows::project::chrome::{self, Tone};
    use cosmic::iced::alignment::Vertical;
    use cosmic::iced::{Alignment, Length};
    use cosmic::widget::{Column, Row, Space, button, container, scrollable, text};

    let line = |label: String, size: u16, tone: Tone, bold: bool| {
        let mut t = text(label)
            .size(size)
            .wrapping(cosmic::iced::widget::text::Wrapping::None)
            .class(chrome::text_class(tone));
        if bold {
            t = t.font(cosmic::font::semibold());
        }
        t
    };

    let mut list = Column::new();
    if entries.is_empty() {
        // `Text("Nessuna versione salvata")` — a disabled menu line
        list = list.push(
            container(line(
                fl!("nessuna-versione-salvata"),
                13,
                Tone::Tertiary,
                false,
            ))
            .padding([0.0, TEXT_INSET])
            .height(Length::Fixed(ROW_HEIGHT - 12.0))
            .align_y(Vertical::Center),
        );
    } else {
        list = list.push(
            container(line(
                if busy {
                    fl!("disponibili-al-termine-della-generazione")
                } else {
                    fl!("torna-a-una-versione")
                },
                11,
                Tone::Tertiary,
                true,
            ))
            .padding([0.0, TEXT_INSET])
            .height(Length::Fixed(HEADER_HEIGHT))
            .align_y(Vertical::Center),
        );
        for entry in entries {
            let restore = on_restore.clone();
            let on_press = (!busy).then(move || restore(entry.number));
            let tone = if busy { Tone::Tertiary } else { Tone::Primary };
            let gutter: cosmic::Element<'static, Message> = if entry.is_current {
                chrome::glyph(chrome::CHECKMARK, (10.0, 9.0), tone)
            } else {
                Space::new().width(Length::Fixed(10.0)).into()
            };
            let head = Row::new()
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fixed(TEXT_INSET - 12.0)))
                .push(gutter)
                .push(Space::new().width(Length::Fixed(2.0)))
                .push(line(
                    fl!(
                        "versione-lld-2",
                        version = entry.number,
                        summary = entry.prompt_summary
                    ),
                    13,
                    tone,
                    false,
                ));
            let body = Column::new()
                .push(
                    container(head)
                        .height(Length::Fixed(20.0))
                        .align_y(Vertical::Center),
                )
                .push(
                    container(line(subtitle(entry, now), 11, Tone::Tertiary, false))
                        .padding([0.0, TEXT_INSET])
                        .height(Length::Fixed(14.0)),
                )
                .width(Length::Fill)
                .height(Length::Fixed(ROW_HEIGHT));
            list = list.push(
                container(
                    button::custom(body)
                        .padding(0)
                        .class(chrome::button_class(false, 5.0))
                        .on_press_maybe(on_press)
                        .width(Length::Fill),
                )
                .padding([0, 5]),
            );
        }
    }
    container(
        scrollable(
            list.push(Space::new().height(Length::Fixed(4.0)))
                .width(Length::Fixed(width)),
        )
        .height(Length::Shrink),
    )
    .padding([CARD_INSET_TOP, 0.0, 0.0, 0.0])
    .max_height(360.0)
    .class(chrome::card_class())
    .into()
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
        assert_eq!(
            relative(now - time::Duration::minutes(5), now),
            "5 minutes ago"
        );
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
        let _ = view_versions(&[], false, now, 340.0, |n| n);
        let _ = view_versions(&[], true, now, 340.0, |n| n);
        let mut current = entry(2, 1);
        current.is_current = true;
        let _ = view_versions(&[current.clone(), entry(1, 300)], false, now, 340.0, |n| n);
        let _ = view_versions(&[current, entry(1, 300)], true, now, 340.0, |n| n);
        let _ = version_button(None, false, ());
        let _ = version_button(Some(2), true, ());
    }
}
