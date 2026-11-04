//! Parts-overlay widgets — the port of upstream `Viewer3D/PartsOverlay.swift`
//! plus the two ModelPane overlays that travel with it: the measurement
//! badge (ModelPane.swift:163-179) and the section controls
//! (ModelPane.swift:181-200). Pure MAPPING only: every behavioral rule
//! lives in [`crate::viewer_tools`] (the state hub) or the viewer's
//! arrangement math; these views render state verbatim and route every
//! interaction to a [`crate::viewer_pane::Message`].
//!
//! Icons, tones, the card surface, the segmented picker and the small
//! slider come from [`crate::ui`].
//!
//! The views copy every input into owned widget state and return
//! `Element<'static>` — callers may pass short-lived data (the pane
//! computes `groups` fresh per frame).

use cosmic::Element;
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Background, Border, Color, Length};
use cosmic::widget::{Column, Row, Space, container, mouse_area, text, tooltip};
use engine::settings::{MeasurementUnit, PartsLayout};
use viewer::tools::arrangement::PartGroup;
use viewer::tools::section;

use crate::fl;
use crate::typography;
use crate::ui::segmented::{Look, segmented};
use crate::ui::slider::{small_slider, with_rail_ends};
use crate::ui::surface::{self, Corner, Shadow};
use crate::ui::tone::wash;
use crate::ui::{Backdrop, Symbol, Tone, button, icon::icon};
use crate::viewer_pane::{Message, format_length};
use crate::viewer_tools::{EXPLODE_RANGE, ViewerTools};

/// This module's text widget, pinned to cosmic's theme/renderer (the
/// chat pane's invariance note).
type Text<'a> = cosmic::iced::widget::Text<'a, cosmic::theme::Theme, cosmic::Renderer>;

/// The panel's fixed width when expanded (PartsOverlay.swift:47).
const PANEL_WIDTH: f32 = 250.0;
/// The part list's height cap (PartsOverlay.swift:87).
const LIST_MAX_HEIGHT: f32 = 220.0;
/// One group row: a 16 px callout line plus 2 px vertical padding each
/// side (PartsOverlay.swift:131), rows 2 px apart (:81). Fixed, so the
/// list's scroll height is exact.
const ROW_HEIGHT: f32 = 20.0;
const ROW_SPACING: f32 = 2.0;
/// The overlays float on a material.
const BACKDROP: Backdrop = Backdrop::Material;

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
        .class(surface::card(Corner::S, Shadow::Overlay { radius: 6.0 }))
        .into()
}

/// The header button: the layers glyph + semibold "Parts", the group
/// count when more than one group, and the chevron — the whole row
/// toggles `collapsed` (PartsOverlay.swift:57-77).
fn header(tools: &ViewerTools, groups: &[PartGroup]) -> Element<'static, Message> {
    let mut label = Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(icon(Symbol::SquareStack3dUp, 16.0, Tone::Primary, BACKDROP))
        .push(
            text(fl!("parti"))
                .size(typography::CALLOUT)
                .font(crate::typography::font(
                    cosmic::iced::font::Weight::Semibold,
                ))
                .class(Tone::Primary.text(BACKDROP)),
        );
    if groups.len() > 1 {
        label = label.push(count_badge(groups.len()));
    }
    label = label.push(icon(
        if tools.collapsed {
            Symbol::ChevronDown
        } else {
            Symbol::ChevronUp
        },
        14.0,
        Tone::Secondary,
        BACKDROP,
    ));
    let help = if tools.collapsed {
        fl!("mostra-le-parti")
    } else {
        fl!("nascondi-le-parti")
    };
    let header = button::borderless(label, Some(Message::TogglePartsCollapsed)).name(help.clone());
    button::with_help(header, help, cosmic::widget::tooltip::Position::Bottom)
}

/// The layout picker (PartsOverlay.swift:16-23): one segmented control
/// over the available layouts, disabled while animating or comparing.
fn layout_picker(
    tools: &ViewerTools,
    parts_count: usize,
    comparing: bool,
) -> Element<'static, Message> {
    let layouts = ViewerTools::available_layouts(parts_count);
    let selected = layouts.iter().position(|layout| *layout == tools.layout);
    let items = layouts
        .into_iter()
        .map(|layout| {
            (
                layout_label(layout),
                (!comparing).then_some(Message::SetLayout(layout)),
            )
        })
        .collect();
    tooltip(
        segmented(items, selected, Look::Filled, Length::Fill),
        text(fl!("vista-delle-parti")),
        cosmic::widget::tooltip::Position::Bottom,
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
/// layout (PartsOverlay.swift:24-33): the inward-arrows glyph, the slider
/// over `EXPLODE_RANGE` (0 assembled … 2 fully spread), the outward
/// arrows.
fn explode_row(tools: &ViewerTools) -> Element<'static, Message> {
    let distance = small_slider(
        EXPLODE_RANGE,
        tools.explode_amount,
        Message::SetExplode,
        Length::Fill,
    );
    Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(icon(
            Symbol::ArrowDownRightAndArrowUpLeft,
            13.0,
            Tone::Secondary,
            BACKDROP,
        ))
        .push(with_rail_ends(
            tooltip(
                distance,
                text(fl!("distanza-tra-le-parti")),
                cosmic::widget::tooltip::Position::Bottom,
            ),
            Length::Fill,
        ))
        .push(icon(
            Symbol::ArrowUpLeftAndArrowDownRight,
            13.0,
            Tone::Secondary,
            BACKDROP,
        ))
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
    let mut rows = Column::new().spacing(ROW_SPACING);
    for group in groups {
        rows = rows.push(group_row(tools, group, exploded));
    }
    cosmic::widget::scrollable::scrollable(rows.width(Length::Fill))
        .height(Length::Fixed(list_height(groups.len())))
        .width(Length::Fill)
        .into()
}

/// The list's height: every row plus the gaps between them, capped.
fn list_height(rows: usize) -> f32 {
    let rows = rows as f32;
    (rows * ROW_HEIGHT + (rows - 1.0).max(0.0) * ROW_SPACING).min(LIST_MAX_HEIGHT)
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
        .align_y(Alignment::Center)
        .push(color_dot(group.color, 9.0))
        .push(
            text(group.name.clone())
                .size(typography::CALLOUT)
                .wrapping(Wrapping::None)
                .ellipsize(cosmic::iced::core::text::Ellipsize::Middle(
                    cosmic::iced::core::text::EllipsizeHeightLimit::Lines(1),
                ))
                .class(if is_hidden {
                    Tone::Tertiary.text(BACKDROP)
                } else {
                    Tone::Primary.text(BACKDROP)
                }),
        );
    if group.members.len() > 1 {
        cluster = cluster.push(
            text(fl!("lld-3", count = group.members.len()))
                .size(typography::CAPTION)
                .class(Tone::Secondary.text(BACKDROP)),
        );
    }
    let name = group.name.clone();
    let mut row = Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .height(Length::Fixed(ROW_HEIGHT))
        .push(
            mouse_area(
                container(cluster)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_y(Alignment::Center),
            )
            .on_press(Message::RowClicked(name.clone())),
        );
    if exploded {
        row = row.push(button::icon(
            if is_attached {
                Symbol::Link
            } else {
                Symbol::LinkBadgePlus
            },
            16.0,
            if is_attached {
                Tone::Accent
            } else {
                Tone::Secondary
            },
            BACKDROP,
            if is_attached {
                fl!("separa-dal-resto")
            } else {
                fl!("tieni-unita-al-resto")
            },
            Some(Message::ToggleAttachedGroup(name.clone())),
        ));
    }
    // upstream's eye reads about half as dark as the link glyph beside it
    // (176 vs 129 on the card): SF's eye strokes are hairlines, so the
    // port's eye takes the tertiary ink to land at the same weight
    row = row.push(button::icon(
        if is_hidden {
            Symbol::EyeSlash
        } else {
            Symbol::Eye
        },
        18.0,
        Tone::Tertiary,
        BACKDROP,
        if is_hidden {
            fl!("mostra")
        } else {
            fl!("nascondi")
        },
        Some(Message::ToggleHiddenGroup(name)),
    ));
    row.into()
}

/// The caption count beside the header title (upstream
/// `.caption.monospacedDigit()`, secondary).
fn count_badge(count: usize) -> Text<'static> {
    text(count.to_string())
        .size(typography::CAPTION)
        .class(Tone::Secondary.text(BACKDROP))
}

/// The plate note (PartsOverlay.swift:37-42): shown in the effective
/// Plate layout while the agent has produced no plate parts.
fn plate_note() -> Element<'static, Message> {
    text(fl!(
        "l-agente-non-ha-ancora-preparato-il-piatto-di-stampa-le-parti-sono-affiancate-senza-ruotarle-chiedi-di-prepararlo-nella-chat"
    ))
    .size(typography::CAPTION)
    .class(Tone::Secondary.text(BACKDROP))
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
            fl!(
                "x-y-z",
                total = format_length(total as f64, unit, true),
                dx = format_length(delta[0] as f64, unit, true),
                dy = format_length(delta[1] as f64, unit, true),
                dz = format_length(delta[2] as f64, unit, true),
            )
        }
    }
}

/// The floating measurement badge: a material capsule with the two-point
/// measurement in monospace (upstream `.fontDesign(.monospaced)`, on the
/// measurement arm only) or the next-step prompt, at `.callout` (12).
pub fn view_measure_badge(points: &[[f32; 3]], unit: MeasurementUnit) -> Element<'static, Message> {
    let body = text(badge_text(points, unit))
        .size(12)
        .class(Tone::Primary.text(BACKDROP));
    let body = if points.len() == 2 {
        body.font(cosmic::font::mono())
    } else {
        body
    };
    container(body)
        .padding([5.0, 10.0])
        .class(surface::material_fill(Corner::Capsule))
        .into()
}

// ─── section controls (ModelPane.swift:181-200) ────────────────────────────

/// The bottom-center section capsule: the X/Y/Z segmented picker
/// (`.fixedSize()`) and the position slider over
/// `section::SLIDER_MIN..=SLIDER_MAX`, width 220, 10 apart, in a material capsule
/// padded 12 × 6.
pub fn view_section_controls(tools: &ViewerTools) -> Element<'static, Message> {
    let selected = section::ALL
        .iter()
        .position(|axis| *axis == tools.section_axis);
    let axes = segmented(
        section::ALL
            .iter()
            .map(|axis| {
                (
                    axis.title().to_string(),
                    Some(Message::SetSectionAxis(*axis)),
                )
            })
            .collect(),
        selected,
        Look::Filled,
        Length::Shrink,
    );
    let width = Length::Fixed(220.0);
    let position = small_slider(
        section::SLIDER_MIN..=section::SLIDER_MAX,
        tools.section_position,
        Message::SetSectionPosition,
        width,
    );
    let row = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(axes)
        .push(with_rail_ends(
            tooltip(
                position,
                text(fl!("posizione-della-sezione")),
                cosmic::widget::tooltip::Position::Top,
            ),
            width,
        ));
    container(row.padding([6.0, 12.0]))
        .class(surface::material_fill(Corner::Capsule))
        .into()
}

// ─── color dot ─────────────────────────────────────────────────────────────

/// A `size`-pixel color swatch with a hairline separator ring (upstream's
/// stroked circles); `None` falls back to the dimmed secondary fill
/// (PartsOverlay.swift:95-98).
pub(crate) fn color_dot(color: Option<[u8; 3]>, size: f32) -> Element<'static, Message> {
    container(
        Space::new()
            .width(Length::Fixed(size))
            .height(Length::Fixed(size)),
    )
    .width(Length::Fixed(size))
    .height(Length::Fixed(size))
    .class(dot_style(
        color.map(|c| Color::from_rgb8(c[0], c[1], c[2])),
        size,
    ))
    .into()
}

fn dot_style(fill: Option<Color>, size: f32) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| {
        let fill = fill.unwrap_or_else(|| wash(theme, 0.4));
        cosmic::iced::widget::container::Style {
            background: Some(Background::Color(fill)),
            border: Border {
                color: Tone::Separator.color(theme, BACKDROP),
                width: 1.0,
                radius: (size / 2.0).into(),
            },
            ..Default::default()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The list is sized to exactly its rows and gaps, capped at the
    /// upstream maximum (PartsOverlay.swift:87).
    #[test]
    fn list_height_fits_rows_and_caps() {
        assert_eq!(list_height(1), ROW_HEIGHT);
        assert_eq!(list_height(2), 2.0 * ROW_HEIGHT + ROW_SPACING);
        assert_eq!(list_height(100), LIST_MAX_HEIGHT);
    }
}
