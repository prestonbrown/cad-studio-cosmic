//! The version history menu (upstream VersionMenu.swift): the toolbar's
//! clock button whose dropdown lists every recorded version newest-first
//! and restores on selection.
//!
//! VersionMenu.swift:16-21 declares a checkmark `Label` on the current
//! version and a second `Text` (relative · absolute date) under every row,
//! but a macOS `Menu` renders neither: upstream's capture
//! (shots/upstream/version-menu.png) shows single-line "Version N ·
//! prompt" rows with no checkmark and a blank second line. The card draws
//! what users see.
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
//! locale-aware formatters, which no catalog key backs. They label each
//! row's tooltip, where the date stays reachable.

use time::OffsetDateTime;

use engine::project::versions::ProjectVersion;

use crate::fl;
use crate::ui::surface::{self, Corner, Shadow};
use crate::ui::{Backdrop, Symbol, Tone, button as ui_button, icon::icon};

/// The pill sits in the titlebar; the card is a menu material.
const TITLEBAR: Backdrop = Backdrop::Titlebar;
const MENU: Backdrop = Backdrop::Material;

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
/// ago", "yesterday", "4 days ago", in the selected language. Pure in
/// `now` so the formatter is testable; distances are non-negative (a
/// future timestamp reads as "just now"). Each bucket names its own
/// grammatical number: the counted buckets start at 2.
pub fn relative(at: OffsetDateTime, now: OffsetDateTime) -> String {
    let seconds = (now - at).whole_seconds().max(0);
    match seconds {
        0..=59 => fl!("version-relative-now"),
        60..=119 => fl!("version-relative-minute"),
        120..=3_599 => fl!("version-relative-minutes", n = seconds / 60),
        3_600..=7_199 => fl!("version-relative-hour"),
        7_200..=86_399 => fl!("version-relative-hours", n = seconds / 3600),
        86_400..=172_799 => fl!("version-relative-yesterday"),
        _ => fl!("version-relative-days", n = seconds / 86_400),
    }
}

/// The date half of an entry's subtitle (`.formatted(date: .abbreviated,
/// time: .shortened)`) in the system time zone: "Oct 13, 2026 at 3:24 PM",
/// "13 ott 2026, 15:24".
pub fn abbreviated(at: OffsetDateTime) -> String {
    abbreviated_at(at.to_offset(local_offset(at)))
}

/// The system time zone's offset at `at` (daylight saving included); UTC
/// when the zone cannot be read.
fn local_offset(at: OffsetDateTime) -> time::UtcOffset {
    use chrono::{Offset, TimeZone};
    chrono::Local
        .timestamp_opt(at.unix_timestamp(), 0)
        .single()
        .and_then(|local| {
            time::UtcOffset::from_whole_seconds(local.offset().fix().local_minus_utc()).ok()
        })
        .unwrap_or(time::UtcOffset::UTC)
}

/// [`abbreviated`] for a timestamp already in the zone to show.
pub fn abbreviated_at(at: OffsetDateTime) -> String {
    let month = match at.month() {
        time::Month::January => fl!("version-month-jan"),
        time::Month::February => fl!("version-month-feb"),
        time::Month::March => fl!("version-month-mar"),
        time::Month::April => fl!("version-month-apr"),
        time::Month::May => fl!("version-month-may"),
        time::Month::June => fl!("version-month-jun"),
        time::Month::July => fl!("version-month-jul"),
        time::Month::August => fl!("version-month-aug"),
        time::Month::September => fl!("version-month-sep"),
        time::Month::October => fl!("version-month-oct"),
        time::Month::November => fl!("version-month-nov"),
        time::Month::December => fl!("version-month-dec"),
    };
    // the language's clock: `version-hour-cycle` is 12 or 24
    let time = if fl!("version-hour-cycle") == "12" {
        let hour12 = match at.hour() % 12 {
            0 => 12, // midnight and noon both read 12
            hour => hour,
        };
        let meridiem = if at.hour() < 12 { "AM" } else { "PM" };
        format!("{hour12}:{:02} {meridiem}", at.minute())
    } else {
        format!("{}:{:02}", at.hour(), at.minute())
    };
    fl!(
        "version-date-time",
        month = month,
        day = at.day(),
        year = at.year(),
        time = time
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
/// pill's top-left; 5 px top inset, a 23 px section header, 34 px rows
/// (the label on the first 20 px, the undrawn subtitle's line blank under
/// it), 14.5 px text inset.
pub const CARD_OFFSET: (f32, f32) = (-115.0, 38.0);
const CARD_INSET_TOP: f32 = 5.0;
const HEADER_HEIGHT: f32 = 23.0;
const ROW_HEIGHT: f32 = 34.0;
const TEXT_INSET: f32 = 14.5;
/// The pill's `chevron.down` cell and the blank margin its grid leaves
/// on each side of the glyph.
const CHEVRON_CELL: f32 = 13.0;
const CHEVRON_MARGIN: f32 = 2.2;

/// The toolbar's version menu label (VersionMenu.swift:27-35) in its
/// bordered toolbar pill: `clock.arrow.circlepath`, the current version's
/// "v<N>" (monospaced digits), and the menu's `chevron.down` indicator —
/// 70×27, 8 px side insets, the measured 6/8 px gaps. Pressing opens the
/// history card. NOT a menu-bar root: the history grows and shrinks, and
/// its entries are custom shrink-height leaves. Cosmic's menu panics on
/// both an open menu whose items shrink and a shrink-height leaf in any
/// root but the first (docs/research/libcosmic-menu-dynamic-items-bug.md).
/// Anything whose item set changes lives in a popover like this one.
pub fn version_button<Message: Clone + 'static>(
    current: Option<i64>,
    open: bool,
    on_toggle: Message,
) -> cosmic::Element<'static, Message> {
    use cosmic::iced::{Alignment, Length};
    use cosmic::widget::{Row, Space, button, text};
    let mut label = Row::new().align_y(Alignment::Center).push(icon(
        Symbol::ClockArrowCirclepath,
        18.0,
        Tone::Secondary,
        TITLEBAR,
    ));
    if let Some(current) = current {
        label = label.push(Space::new().width(Length::Fixed(6.0))).push(
            text(fl!("v-lld", version = current))
                .size(crate::typography::BODY)
                .class(Tone::Secondary.text(TITLEBAR)),
        );
    }
    // the menu indicator: `chevron.down` on its 16-unit grid at 13 px
    // draws an 8.5 × 5 glyph 2.2 px inside the cell, so the gaps around it
    // shrink by that margin
    let label = label
        .push(Space::new().width(Length::Fixed(8.0 - CHEVRON_MARGIN)))
        .push(icon(
            Symbol::ChevronDown,
            CHEVRON_CELL,
            Tone::Secondary,
            TITLEBAR,
        ))
        .padding([0.0, 8.0 - CHEVRON_MARGIN, 0.0, 8.0])
        .height(Length::Fixed(27.0));
    let root = button::custom(label)
        .padding(0)
        .class(ui_button::held_wash(true, Corner::S, open))
        .on_press(on_toggle)
        .name(fl!("versioni"));
    ui_button::with_help(
        root,
        fl!("storico-delle-versioni"),
        cosmic::widget::tooltip::Position::Bottom,
    )
}

/// The history card the version button opens, laid out like upstream's
/// `Menu` as macOS draws it (VersionMenu.swift:7-25, version-menu.png):
/// the section header (busy-titled while working), then one single-line
/// row per version newest-first ("Version N · summary"); pressing —
/// unless busy — restores that version. Each row's tooltip carries the
/// relative · abbreviated date the menu declares but does not draw. `now`
/// is the CALLER-supplied clock, frozen while the card is open: a live
/// per-frame time-ago would rebuild every tooltip every second.
pub fn view_versions<Message: Clone + 'static>(
    entries: &[VersionEntry],
    busy: bool,
    now: OffsetDateTime,
    width: f32,
    on_restore: impl Fn(i64) -> Message + Clone + 'static,
) -> cosmic::Element<'static, Message> {
    use cosmic::iced::alignment::Vertical;
    use cosmic::iced::{Alignment, Length};
    use cosmic::widget::{Column, Row, Space, button, container, scrollable, text};

    let line = |label: String, size: f32, tone: Tone, bold: bool| {
        let mut t = text(label)
            .size(size)
            .wrapping(cosmic::iced::widget::text::Wrapping::None)
            .class(tone.text(MENU));
        if bold {
            t = t.font(crate::typography::font(
                cosmic::iced::font::Weight::Semibold,
            ));
        }
        t
    };

    let mut list = Column::new();
    if entries.is_empty() {
        // `Text("Nessuna versione salvata")` — a disabled menu line
        list = list.push(
            container(line(
                fl!("nessuna-versione-salvata"),
                crate::typography::BODY,
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
                crate::typography::scaled(11.0),
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
            let tone = ui_button::content_tone(Tone::Primary, &on_press);
            let head = Row::new()
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fixed(TEXT_INSET)))
                .push(line(
                    fl!(
                        "versione-lld-2",
                        version = entry.number,
                        summary = entry.prompt_summary
                    ),
                    crate::typography::BODY,
                    tone,
                    false,
                ));
            let body = Column::new()
                .push(
                    container(head)
                        .height(Length::Fixed(20.0))
                        .align_y(Vertical::Center),
                )
                .width(Length::Fill)
                .height(Length::Fixed(ROW_HEIGHT));
            let row = button::custom(body)
                .padding(0)
                .class(ui_button::hover_wash(false, Corner::Xs))
                .on_press_maybe(on_press)
                .width(Length::Fill);
            list = list.push(
                container(ui_button::with_help(
                    row,
                    subtitle(entry, now),
                    cosmic::widget::tooltip::Position::Bottom,
                ))
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
    .class(surface::card(Corner::M, Shadow::Popover))
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
        let _lang = crate::localize::pin_en();
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
        let _lang = crate::localize::pin_en();
        let e = entry(1, 5);
        let s = subtitle(&e, OffsetDateTime::now_utc());
        assert!(s.starts_with("5 minutes ago · "), "{s}");
        assert!(s.contains(", 202"), "abbreviated date present: {s}");
    }

    /// The date reads in the selected language's abbreviated form and
    /// 12- or 24-hour clock.
    #[test]
    fn the_date_follows_the_language() {
        let at = time::macros::datetime!(2026-10-13 15:04 UTC);
        let _lang = crate::localize::pin_en();
        assert_eq!(abbreviated_at(at), "Oct 13, 2026 at 3:04 PM");
        assert_eq!(relative(at - time::Duration::days(3), at), "3 days ago");
        crate::localize::select("it");
        let italian = (
            abbreviated_at(at),
            relative(at - time::Duration::days(3), at),
        );
        crate::localize::select("en");
        assert_eq!(italian.0, "13 ott 2026, 15:04");
        assert_eq!(italian.1, "3 giorni fa");
    }

    /// The card builds in every shape: empty, idle, busy (Element
    /// construction is pure; cosmic's menu panics at view build on a bad item
    /// shape, so construction tests are load-bearing).
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
