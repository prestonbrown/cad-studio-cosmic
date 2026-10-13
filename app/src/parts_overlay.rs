//! Parts-overlay widgets — the port of upstream `Viewer3D/PartsOverlay.swift`
//! plus the two ModelPane overlays that travel with it: the measurement
//! badge (ModelPane.swift:163-179) and the section controls
//! (ModelPane.swift:181-200). Pure MAPPING only: every behavioral rule
//! lives in [`crate::viewer_tools`] (the state hub) or the viewer's
//! arrangement math; these views render state verbatim and route every
//! interaction to a [`crate::viewer_pane::Message`].
//!
//! Port divergences, all cosmetic and deliberate (function parity over
//! glyph parity — no SF-Symbol icon-name roulette):
//! - upstream's row/picker icons are TEXT buttons and glyph stand-ins here
//!   (the chevron is "⌃"/"⌄", the explode arrows "↘↗"/"↗↘");
//! - `.help(...)` and accessibility labels ride a tooltip (the chat pane's
//!   established stand-in — iced glyphs carry no label of their own);
//! - a disabled layout picker renders buttons without `on_press` (iced's
//!   disabled look); an inert slider cannot exist, and the explode row
//!   never mounts while comparing (comparing forces the effective layout
//!   to Assembled, hiding it upstream too);
//! - no drop shadow (no widget shadows in iced), matching the parameters
//!   panel.
//!
//! The views copy every input into owned widget state and return
//! `Element<'static>` — callers may pass short-lived data (the pane
//! computes `groups` fresh per frame).

use cosmic::Element;
use cosmic::iced::widget::{slider, text::Wrapping};
use cosmic::iced::{Color, Length};
use cosmic::widget::{Column, Row, button, container, mouse_area, text, tooltip};
use engine::settings::{MeasurementUnit, PartsLayout};
use viewer::tools::arrangement::PartGroup;
use viewer::tools::section;

use crate::fl;
use crate::viewer_pane::{Message, format_length};
use crate::viewer_tools::{EXPLODE_RANGE, ViewerTools};

/// This module's text widget, pinned to cosmic's theme/renderer (the
/// chat pane's invariance note).
type Text<'a> = cosmic::iced::widget::Text<'a, cosmic::theme::Theme, cosmic::Renderer>;

/// The panel's fixed width when expanded (PartsOverlay.swift:47).
const PANEL_WIDTH: f32 = 250.0;
/// The part list's height cap (PartsOverlay.swift:87). Upstream's scroll
/// view grows with content up to this; an iced scrollable needs a bounded
/// height up front, so the view estimates one row and clamps to the cap
/// (the parameters panel's approach).
const LIST_MAX_HEIGHT: f32 = 220.0;
const ROW_HEIGHT_ESTIMATE: f32 = 26.0;

// ─── parts panel (PartsOverlay.swift:12-51) ────────────────────────────────

/// The whole parts overlay: header, layout picker, explode slider, the
/// scrollable group list, and the plate note — the last three gated on
/// the EFFECTIVE layout (upstream reads `viewer.effectiveLayout`, which
/// the pane passes as `effective`), while the picker's SELECTION reads
/// the raw `tools.layout`. `plate_ready` is "the agent produced plate
/// parts"; `comparing` disables the picker (PartsOverlay.swift:23).
pub fn view_parts_panel(
    tools: &ViewerTools,
    groups: &[PartGroup],
    effective: PartsLayout,
    plate_ready: bool,
    comparing: bool,
) -> Element<'static, Message> {
    // The picker offers Exploded only with more than one PART
    // (PartsOverlay.swift:53-55) — the sum of group members, since groups
    // collapse same-base parts into one row.
    let parts_count: usize = groups.iter().map(|g| g.members.len()).sum();
    let mut panel = Column::new().spacing(8).push(header(tools, groups));
    if !tools.collapsed {
        panel = panel.push(layout_picker(tools, parts_count, comparing));
        if effective == PartsLayout::Exploded {
            panel = panel.push(explode_row(tools));
        }
        if groups.len() > 1 {
            panel = panel.push(part_list(tools, groups, effective));
        }
        if effective == PartsLayout::Plate && !plate_ready {
            panel = panel.push(plate_note());
        }
    }
    container(panel)
        .padding(10)
        .width(if tools.collapsed {
            Length::Shrink
        } else {
            Length::Fixed(PANEL_WIDTH)
        })
        .class(panel_style(10.0))
        .into()
}

/// The header button: "Parts", the group-count badge when more than one
/// group, and the chevron — the whole row toggles `collapsed`
/// (PartsOverlay.swift:57-77).
fn header(tools: &ViewerTools, groups: &[PartGroup]) -> Element<'static, Message> {
    let mut label = Row::new().spacing(6).push(text(fl!("parti")).size(14));
    if groups.len() > 1 {
        label = label.push(count_badge(groups.len()));
    }
    label = label.push(
        secondary_text(if tools.collapsed { "⌃" } else { "⌄" }).size(10), // chevron stand-in
    );
    let help = if tools.collapsed {
        fl!("mostra-le-parti")
    } else {
        fl!("nascondi-le-parti")
    };
    let header = button::custom(label)
        .class(cosmic::theme::Button::Text)
        .on_press(Message::TogglePartsCollapsed);
    tooltip(
        header,
        text(help),
        cosmic::widget::tooltip::Position::FollowCursor,
    )
    .into()
}

/// The layout picker as one button per layout, the selected one in the
/// suggested style (the preset picker's pattern — a segmented control's
/// model borrows its owner, which a per-view rebuild cannot satisfy).
/// Disabled while comparing: buttons without `on_press`
/// (PartsOverlay.swift:16-23).
fn layout_picker(
    tools: &ViewerTools,
    parts_count: usize,
    comparing: bool,
) -> Element<'static, Message> {
    let mut bar = Row::new().spacing(4);
    for layout in ViewerTools::available_layouts(parts_count) {
        let mut choice = button::text(layout_label(layout)).class(if tools.layout == layout {
            cosmic::theme::Button::Suggested
        } else {
            cosmic::theme::Button::Standard
        });
        if !comparing {
            choice = choice.on_press(Message::SetLayout(layout));
        }
        bar = bar.push(choice);
    }
    tooltip(
        bar,
        text(fl!("vista-delle-parti")),
        cosmic::widget::tooltip::Position::FollowCursor,
    )
    .into()
}

fn layout_label(layout: PartsLayout) -> String {
    match layout {
        PartsLayout::Assembled => fl!("montato"),
        PartsLayout::Exploded => fl!("esploso"),
        PartsLayout::Plate => fl!("piatto"),
    }
}

/// The explode-distance slider row, only in the effective Exploded
/// layout (PartsOverlay.swift:24-33): the range is T8's `EXPLODE_RANGE`
/// (0 assembled … 2 fully spread); the arrows are glyph stand-ins.
fn explode_row(tools: &ViewerTools) -> Element<'static, Message> {
    let distance =
        slider(EXPLODE_RANGE, tools.explode_amount, Message::SetExplode).width(Length::Fill);
    Row::new()
        .spacing(6)
        .push(secondary_text("↘↗").size(12))
        .push(tooltip(
            distance,
            text(fl!("distanza-tra-le-parti")),
            cosmic::widget::tooltip::Position::FollowCursor,
        ))
        .push(secondary_text("↗↘").size(12))
        .into()
}

/// The scrollable group list (PartsOverlay.swift:79-89): one row per
/// group, capped at [`LIST_MAX_HEIGHT`].
fn part_list(
    tools: &ViewerTools,
    groups: &[PartGroup],
    effective: PartsLayout,
) -> Element<'static, Message> {
    let exploded = effective == PartsLayout::Exploded;
    let mut rows = Column::new().spacing(2);
    for group in groups {
        rows = rows.push(group_row(tools, group, exploded));
    }
    let height = (groups.len() as f32 * ROW_HEIGHT_ESTIMATE + 4.0).min(LIST_MAX_HEIGHT);
    cosmic::widget::scrollable::scrollable(rows.width(Length::Fill))
        .height(Length::Fixed(height))
        .width(Length::Fill)
        .into()
}

/// One group row (PartsOverlay.swift:91-135): color dot, name (dimmed
/// when hidden), `×N` for multi-member groups, then the attach button
/// (exploded layout only) and the show/hide button. The name cluster
/// reports [`Message::RowClicked`] on every press — the pane decodes the
/// double-click (T8's `is_double_click`) and isolates the group.
fn group_row(tools: &ViewerTools, group: &PartGroup, exploded: bool) -> Element<'static, Message> {
    let is_hidden = tools.hidden.contains(&group.name);
    let is_attached = tools.attached.contains(&group.name);
    let mut cluster = Row::new()
        .spacing(6)
        .push(color_dot(group.color, 9.0))
        .push(
            text(group.name.clone())
                .size(12)
                .width(Length::Fill)
                .wrapping(Wrapping::None)
                .ellipsize(cosmic::iced::core::text::Ellipsize::Middle(
                    cosmic::iced::core::text::EllipsizeHeightLimit::Lines(1),
                ))
                .class(if is_hidden {
                    tertiary_text_class()
                } else {
                    cosmic::theme::Text::Default
                }),
        );
    if group.members.len() > 1 {
        cluster = cluster.push(count_badge(group.members.len()));
    }
    let name = group.name.clone();
    let mut row = Row::new()
        .spacing(6)
        .push(mouse_area(cluster.width(Length::Fill)).on_press(Message::RowClicked(name)));
    if exploded {
        row = row.push(action_button(
            if is_attached {
                fl!("separa-dal-resto")
            } else {
                fl!("tieni-unita-al-resto")
            },
            is_attached,
            Message::ToggleAttachedGroup(group.name.clone()),
        ));
    }
    row = row.push(action_button(
        if is_hidden {
            fl!("mostra")
        } else {
            fl!("nascondi")
        },
        false,
        Message::ToggleHiddenGroup(group.name.clone()),
    ));
    row.into()
}

/// A small text action button; `active` tints the label with the theme
/// accent (upstream colors the attached link icon with `accentColor`).
fn action_button(label: String, active: bool, message: Message) -> Element<'static, Message> {
    let body = text(label).size(10).class(if active {
        accent_text_class()
    } else {
        secondary_text_class()
    });
    button::custom(body)
        .class(cosmic::theme::Button::Text)
        .on_press(message)
        .into()
}

/// The caption digit badge (upstream `.caption.monospacedDigit()`).
fn count_badge(count: usize) -> Text<'static> {
    text(count.to_string())
        .font(cosmic::font::mono())
        .size(10)
        .class(secondary_text_class())
}

/// The plate note (PartsOverlay.swift:37-42): shown in the effective
/// Plate layout while the agent has produced no plate parts.
fn plate_note() -> Element<'static, Message> {
    secondary_text(fl!(
        "l-agente-non-ha-ancora-preparato-il-piatto-di-stampa-le-parti-sono-affiancate-senza-ruotarle-chiedi-di-prepararlo-nella-chat"
    ))
    .size(12)
    .wrapping(Wrapping::Word)
    .width(Length::Fill)
    .into()
}

// ─── measurement badge (ModelPane.swift:163-179) ───────────────────────────

/// The badge's text: with two points `total · ΔX dx · ΔY dy · ΔZ dz`
/// (Δ a literal glyph, matching upstream's badge), every term through
/// [`format_length`] WITH its unit symbol; otherwise the next-step prompt.
pub fn badge_text(points: &[[f32; 3]], unit: MeasurementUnit) -> String {
    match points.len() {
        0 => fl!("fai-clic-su-un-punto-del-modello"),
        1 => fl!("fai-clic-sul-secondo-punto"),
        _ => {
            // delta = abs(p1 - p0) per MODEL component; the total is the
            // delta's length (simd_length(abs(delta)) == simd_length(delta))
            let (p0, p1) = (points[0], points[1]);
            let delta = [
                (p1[0] - p0[0]).abs(),
                (p1[1] - p0[1]).abs(),
                (p1[2] - p0[2]).abs(),
            ];
            let total = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
            format!(
                "{} · ΔX {} · ΔY {} · ΔZ {}",
                format_length(total as f64, unit, true),
                format_length(delta[0] as f64, unit, true),
                format_length(delta[1] as f64, unit, true),
                format_length(delta[2] as f64, unit, true),
            )
        }
    }
}

/// The floating measurement badge: a capsule with the two-point
/// measurement in monospace (upstream `.fontDesign(.monospaced)`, on the
/// measurement arm only) or the next-step prompt.
pub fn view_measure_badge(points: &[[f32; 3]], unit: MeasurementUnit) -> Element<'static, Message> {
    let body = text(badge_text(points, unit)).size(14);
    let body = if points.len() == 2 {
        body.font(cosmic::font::mono())
    } else {
        body
    };
    container(body)
        .padding([5.0, 10.0])
        .class(capsule_style())
        .into()
}

// ─── section controls (ModelPane.swift:181-200) ────────────────────────────

/// The bottom-center section capsule: the X/Y/Z axis picker (one button
/// per axis, selected = suggested — the preset picker's pattern) and the
/// position slider over T3's `SLIDER_MIN..=SLIDER_MAX`, width 220.
pub fn view_section_controls(tools: &ViewerTools) -> Element<'static, Message> {
    let mut axes = Row::new().spacing(4);
    for axis in section::ALL {
        axes = axes.push(
            button::text(axis.title().to_string())
                .class(if tools.section_axis == axis {
                    cosmic::theme::Button::Suggested
                } else {
                    cosmic::theme::Button::Standard
                })
                .on_press(Message::SetSectionAxis(axis)),
        );
    }
    let position = slider(
        section::SLIDER_MIN..=section::SLIDER_MAX,
        tools.section_position,
        Message::SetSectionPosition,
    )
    .width(Length::Fixed(220.0));
    let row = Row::new().spacing(10).push(axes).push(tooltip(
        position,
        text(fl!("posizione-della-sezione")),
        cosmic::widget::tooltip::Position::FollowCursor,
    ));
    container(row.padding([6.0, 12.0]))
        .class(capsule_style())
        .into()
}

// ─── shared bits (private twins; modules stay self-contained — the chat
// pane's convention) ─────────────────────────────────────────────────────────

fn secondary_text(body: impl Into<String>) -> Text<'static> {
    text(body.into()).class(secondary_text_class())
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

/// Dimmer than secondary (upstream's `.tertiary` on a hidden row's name).
fn tertiary_text_class() -> cosmic::theme::Text {
    cosmic::theme::Text::Custom(|theme| {
        let mut color = Color::from(theme.cosmic().background(false).on);
        color.a *= 0.45;
        cosmic::iced::widget::text::Style {
            color: Some(color),
            ..Default::default()
        }
    })
}

fn accent_text_class() -> cosmic::theme::Text {
    cosmic::theme::Text::Custom(|theme| {
        let accent = theme.cosmic().accent_color();
        cosmic::iced::widget::text::Style {
            color: Some(Color::from_rgb(accent.red, accent.green, accent.blue)),
            ..Default::default()
        }
    })
}

/// A solid theme-base surface with a hairline border (upstream's
/// `.regularMaterial` + separator stroke; the drop shadow has no iced
/// port) — the parameters panel's style at the parts panel's radius 10.
fn panel_style(radius: f32) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| {
        let base = Color::from(theme.cosmic().background(false).base);
        let mut border = Color::from(theme.cosmic().background(false).on);
        border.a = 0.15;
        cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(base)),
            border: cosmic::iced::Border {
                color: border,
                width: 1.0,
                radius: radius.into(),
            },
            ..Default::default()
        }
    })
}

/// The badge/section capsule: the panel surface at a pill radius.
fn capsule_style() -> cosmic::theme::Container<'static> {
    panel_style(999.0)
}

/// A `size`-pixel color swatch with a hairline ring (upstream's stroked
/// circles); `None` falls back to the dimmed secondary fill
/// (PartsOverlay.swift:95-98).
pub(crate) fn color_dot(color: Option<[u8; 3]>, size: f32) -> Element<'static, Message> {
    container(dot_filler(size))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .class(dot_style(
            color.map(|c| Color::from_rgb8(c[0], c[1], c[2])),
            size,
        ))
        .into()
}

/// A zero-content filler that gives the dot its square bounds.
fn dot_filler(size: f32) -> cosmic::widget::Space {
    cosmic::widget::Space::new()
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
}

fn dot_style(fill: Option<Color>, size: f32) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| {
        let fill = fill.unwrap_or_else(|| {
            let mut fallback = Color::from(theme.cosmic().background(false).on);
            fallback.a *= 0.4;
            fallback
        });
        let mut border = Color::from(theme.cosmic().background(false).on);
        border.a = 0.15;
        cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(fill)),
            border: cosmic::iced::Border {
                color: border,
                width: 1.0,
                radius: (size / 2.0).into(),
            },
            ..Default::default()
        }
    })
}
