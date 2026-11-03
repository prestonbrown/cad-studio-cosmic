//! Parts-overlay widgets — the port of upstream `Viewer3D/PartsOverlay.swift`
//! plus the two ModelPane overlays that travel with it: the measurement
//! badge (ModelPane.swift:163-179) and the section controls
//! (ModelPane.swift:181-200). Pure MAPPING only: every behavioral rule
//! lives in [`crate::viewer_tools`] (the state hub) or the viewer's
//! arrangement math; these views render state verbatim and route every
//! interaction to a [`crate::viewer_pane::Message`].
//!
//! The module also holds the overlay look the parameters panel and the
//! schematic footer share: the SF-Symbol-shaped icons
//! ([`Symbol`], drawn in `app/resources/icons/`), the floating card
//! surface ([`card_style`]: upstream's `.regularMaterial` + separator
//! stroke + drop shadow), the compact segmented picker ([`segmented`]:
//! `.pickerStyle(.segmented)` at `.controlSize(.small)`), and the small
//! slider ([`small_slider_class`]).
//!
//! Every color derives from the cosmic theme (the window's `base` / `on`
//! pair and the accent), mixed at the ratios measured off the upstream
//! light-mode gallery, so dark mode inverts with the theme.
//!
//! The views copy every input into owned widget state and return
//! `Element<'static>` — callers may pass short-lived data (the pane
//! computes `groups` fresh per frame).

use std::rc::Rc;

use cosmic::Element;
use cosmic::iced::widget::{slider, svg, text::Wrapping};
use cosmic::iced::{Alignment, Background, Border, Color, Length, Shadow, Vector};
use cosmic::widget::{Column, Row, Space, button, container, mouse_area, text, tooltip};
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
/// The part list's height cap (PartsOverlay.swift:87).
const LIST_MAX_HEIGHT: f32 = 220.0;
/// One group row: a 16 px callout line plus 2 px vertical padding each
/// side (PartsOverlay.swift:131), rows 2 px apart (:81). Fixed, so the
/// list's scroll height is exact.
const ROW_HEIGHT: f32 = 20.0;
const ROW_SPACING: f32 = 2.0;

// ─── SF-Symbol stand-ins ───────────────────────────────────────────────────

/// The SF Symbols upstream's overlays and pane bars draw, each shipped as
/// a same-shape SVG in `app/resources/icons/<sf-name>.svg` (no Cosmic
/// theme icon has these shapes). The SVGs are single-color masks: every
/// use tints them through [`symbol`]'s color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Symbol {
    SquareStack3dUp,
    ChevronUp,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    ChevronUpChevronDown,
    Xmark,
    Eye,
    EyeSlash,
    Link,
    LinkBadgePlus,
    ArrowsInward,
    ArrowsOutward,
    SliderHorizontal3,
    MinusMagnifyingGlass,
    PlusMagnifyingGlass,
}

impl Symbol {
    /// The glyph's width over its height: the side chevrons ship a
    /// 10 × 16 view box (no dead side margins, matching SF's tight
    /// horizontal metrics); every other symbol is square.
    fn aspect(self) -> f32 {
        match self {
            Self::ChevronLeft | Self::ChevronRight => 10.0 / 16.0,
            _ => 1.0,
        }
    }

    fn svg(self) -> &'static [u8] {
        match self {
            Self::SquareStack3dUp => include_bytes!("../resources/icons/square.stack.3d.up.svg"),
            Self::ChevronUp => include_bytes!("../resources/icons/chevron.up.svg"),
            Self::ChevronDown => include_bytes!("../resources/icons/chevron.down.svg"),
            Self::ChevronLeft => include_bytes!("../resources/icons/chevron.left.svg"),
            Self::ChevronRight => include_bytes!("../resources/icons/chevron.right.svg"),
            Self::ChevronUpChevronDown => {
                include_bytes!("../resources/icons/chevron.up.chevron.down.svg")
            }
            Self::Xmark => include_bytes!("../resources/icons/xmark.svg"),
            Self::Eye => include_bytes!("../resources/icons/eye.svg"),
            Self::EyeSlash => include_bytes!("../resources/icons/eye.slash.svg"),
            Self::Link => include_bytes!("../resources/icons/link.svg"),
            Self::LinkBadgePlus => include_bytes!("../resources/icons/link.badge.plus.svg"),
            Self::ArrowsInward => {
                include_bytes!("../resources/icons/arrow.down.right.and.arrow.up.left.svg")
            }
            Self::ArrowsOutward => {
                include_bytes!("../resources/icons/arrow.up.left.and.arrow.down.right.svg")
            }
            Self::SliderHorizontal3 => include_bytes!("../resources/icons/slider.horizontal.3.svg"),
            Self::MinusMagnifyingGlass => {
                include_bytes!("../resources/icons/minus.magnifyingglass.svg")
            }
            Self::PlusMagnifyingGlass => {
                include_bytes!("../resources/icons/plus.magnifyingglass.svg")
            }
        }
    }
}

/// How strongly an overlay glyph or label reads against its surface —
/// upstream's `.primary` / `.secondary` / `.tertiary` vibrancy levels
/// inside a material, as fractions of the theme's `on` color (measured:
/// primary text 68/255 on a 228 card ≈ 0.72; secondary glyphs ≈ 0.5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Ink {
    Primary,
    Secondary,
    Tertiary,
    Accent,
    /// Text on a filled neutral control (the selected segment).
    OnFill,
    /// A glyph on an accent fill (the pop-up picker's badge).
    OnAccent,
}

impl Ink {
    /// The ink's opacity over its surface (`OnFill` and `Accent` are
    /// solid).
    const fn alpha(self) -> f32 {
        match self {
            Self::Primary => 0.72,
            Self::Secondary => 0.45,
            Self::Tertiary => 0.25,
            Self::Accent | Self::OnFill | Self::OnAccent => 1.0,
        }
    }

    /// The ink's solid color; [`Ink::color`] applies the alpha.
    fn solid(self, theme: &cosmic::Theme) -> Color {
        match self {
            Self::Primary | Self::Secondary | Self::Tertiary => {
                Color::from(theme.cosmic().background(false).on)
            }
            Self::Accent => {
                let accent = theme.cosmic().accent_color();
                Color::from_rgb(accent.red, accent.green, accent.blue)
            }
            Self::OnFill => Color::WHITE,
            Self::OnAccent => Color::from(theme.cosmic().accent.on),
        }
    }

    pub(crate) fn color(self, theme: &cosmic::Theme) -> Color {
        let solid = self.solid(theme);
        Color {
            a: solid.a * self.alpha(),
            ..solid
        }
    }

    pub(crate) fn text_class(self) -> cosmic::theme::Text {
        cosmic::theme::Text::Custom(match self {
            Self::Primary => |theme| text_style(Ink::Primary.color(theme)),
            Self::Secondary => |theme| text_style(Ink::Secondary.color(theme)),
            Self::Tertiary => |theme| text_style(Ink::Tertiary.color(theme)),
            Self::Accent => |theme| text_style(Ink::Accent.color(theme)),
            Self::OnFill => |theme| text_style(Ink::OnFill.color(theme)),
            Self::OnAccent => |theme| text_style(Ink::OnAccent.color(theme)),
        })
    }
}

fn text_style(color: Color) -> cosmic::iced::widget::text::Style {
    cosmic::iced::widget::text::Style {
        color: Some(color),
        ..Default::default()
    }
}

/// A symbol icon `size` px tall (and [`Symbol::aspect`] as wide), tinted
/// with `ink`. The svg tint replaces color but keeps the glyph's own
/// coverage, so the ink's alpha rides the icon's opacity.
pub(crate) fn symbol(symbol: Symbol, size: u16, ink: Ink) -> cosmic::widget::Icon {
    cosmic::widget::icon::icon(cosmic::widget::icon::from_svg_bytes(symbol.svg()))
        .size(size)
        .width(Length::Fixed(f32::from(size) * symbol.aspect()))
        .height(Length::Fixed(f32::from(size)))
        .opacity(ink.alpha())
        .class(cosmic::theme::Svg::custom(move |theme| svg::Style {
            color: Some(ink.solid(theme)),
        }))
}

/// A borderless icon button (upstream `.buttonStyle(.borderless)` /
/// `.plain` around an `Image(systemName:)`): the bare glyph, no padding,
/// no fill, the help string as tooltip and accessible name. `None`
/// renders it disabled with the glyph at tertiary ink.
pub(crate) fn icon_button<M: Clone + 'static>(
    glyph: Symbol,
    size: u16,
    ink: Ink,
    help: String,
    on_press: Option<M>,
) -> Element<'static, M> {
    let ink = if on_press.is_some() {
        ink
    } else {
        Ink::Tertiary
    };
    let body = button::custom(symbol(glyph, size, ink))
        .padding(0)
        .class(cosmic::theme::Button::Transparent)
        .on_press_maybe(on_press)
        .name(help.clone());
    tooltip(body, text(help), cosmic::widget::tooltip::Position::Bottom).into()
}

/// A borderless text button (upstream `Button("…")` under
/// `.buttonStyle(.borderless)`): the label at `ink`, no padding or fill;
/// disabled shows tertiary ink.
pub(crate) fn text_button<M: Clone + 'static>(
    label: String,
    size: u16,
    ink: Ink,
    on_press: Option<M>,
) -> Element<'static, M> {
    let ink = if on_press.is_some() {
        ink
    } else {
        Ink::Tertiary
    };
    button::custom(text(label).size(size).class(ink.text_class()))
        .padding(0)
        .class(cosmic::theme::Button::Transparent)
        .on_press_maybe(on_press)
        .into()
}

// ─── surfaces ──────────────────────────────────────────────────────────────

/// `a` moved toward `b` by `t` (alpha kept from `a`).
pub(crate) fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a,
    }
}

/// The material fill: the window base nudged 5 % toward its `on` color —
/// darker in light mode (upstream's card reads 228 over a 239 viewer),
/// lighter in dark mode, as macOS's `.regularMaterial` does. Opaque: the
/// 3D scene behind must not show through without the blur.
pub(crate) fn material(theme: &cosmic::Theme) -> Color {
    let bg = theme.cosmic().background(false);
    mix(Color::from(bg.base), Color::from(bg.on), 0.05)
}

/// Upstream's `.separator` stroke on a material: the `on` color at 10 %
/// (207 on the 228 card).
pub(crate) fn separator(theme: &cosmic::Theme) -> Color {
    let on = Color::from(theme.cosmic().background(false).on);
    Color { a: 0.1, ..on }
}

/// The floating card: material fill, hairline separator border, and the
/// drop shadow (`.shadow(color: .black.opacity(0.15), radius:, y: 2)` —
/// SwiftUI's radius is a Gaussian sigma, iced's blur a diameter).
pub(crate) fn card_style(radius: f32, shadow_radius: f32) -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(move |theme| cosmic::iced::widget::container::Style {
        background: Some(Background::Color(material(theme))),
        border: Border {
            color: separator(theme),
            width: 1.0,
            radius: radius.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.15),
            offset: Vector::new(0.0, 2.0),
            blur_radius: shadow_radius * 2.0,
        },
        ..Default::default()
    })
}

/// The measurement badge / section capsule: material at a pill radius,
/// no stroke, no shadow (ModelPane.swift:176, :197).
fn capsule_style() -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(|theme| cosmic::iced::widget::container::Style {
        background: Some(Background::Color(material(theme))),
        border: Border {
            radius: 999.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
}

// ─── segmented picker ──────────────────────────────────────────────────────

/// The segmented control's height at `.controlSize(.small)` (measured
/// 18 px; the selected pill insets 1 px).
const SEGMENT_HEIGHT: f32 = 18.0;

/// The track: a lighter inset on the material (242 on the 228 card in
/// light mode; a faint lift in dark mode).
fn segment_track(theme: &cosmic::Theme) -> Color {
    let lift = if theme.cosmic().is_dark { 0.08 } else { 0.3 };
    mix(material(theme), Color::WHITE, lift)
}

/// The selected pill: the track pressed toward `on` (178 on 242 in light
/// mode), white label.
fn segment_selected(theme: &cosmic::Theme) -> Color {
    let on = Color::from(theme.cosmic().background(false).on);
    mix(segment_track(theme), on, 0.26)
}

/// A compact segmented picker: equal-width segments on a rounded track,
/// the selected one a filled pill with a semibold white label. With
/// `enabled` false the segments carry no press and the labels dim
/// (upstream `.disabled`). `width` is the whole control's width
/// (`Length::Fill` inside a fixed panel, `Shrink` for `.fixedSize()`).
pub(crate) fn segmented<M: Clone + 'static>(
    items: Vec<(String, M)>,
    selected: Option<usize>,
    enabled: bool,
    width: Length,
) -> Element<'static, M> {
    let count = items.len();
    let mut bar = Row::new()
        .spacing(0)
        .align_y(Alignment::Center)
        .height(Length::Fixed(SEGMENT_HEIGHT));
    for (index, (label, message)) in items.into_iter().enumerate() {
        let is_selected = selected == Some(index);
        let ink = match (is_selected, enabled) {
            (true, _) => Ink::OnFill,
            (false, true) => Ink::Primary,
            (false, false) => Ink::Tertiary,
        };
        let mut label = text(label).size(11).class(ink.text_class());
        if is_selected {
            label = label.font(cosmic::font::semibold());
        }
        let pill = container(label)
            .padding([0.0, 6.0])
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .class(if is_selected {
                cosmic::theme::Container::custom(|theme| cosmic::iced::widget::container::Style {
                    background: Some(Background::Color(segment_selected(theme))),
                    border: Border {
                        radius: 4.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
            } else {
                cosmic::theme::Container::Transparent
            });
        let mut segment = mouse_area(container(pill).padding(1));
        if enabled {
            segment = segment.on_press(message);
        }
        bar = bar.push(container(segment).width(if width == Length::Shrink {
            Length::Shrink
        } else {
            Length::FillPortion(1)
        }));
        // a hairline divides two neighbouring unselected segments; next to
        // the selected pill the pill itself is the boundary
        let next_selected = selected == Some(index + 1);
        if index + 1 < count && !is_selected && !next_selected {
            bar = bar.push(segment_divider());
        }
    }
    container(bar)
        .width(width)
        .class(cosmic::theme::Container::custom(|theme| {
            cosmic::iced::widget::container::Style {
                background: Some(Background::Color(segment_track(theme))),
                border: Border {
                    radius: 5.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }))
        .into()
}

/// The 1 px divider between two unselected segments (207 on the 242
/// track: the separator ink).
fn segment_divider<M: 'static>() -> Element<'static, M> {
    container(Space::new().width(1.0).height(10.0))
        .class(cosmic::theme::Container::custom(|theme| {
            cosmic::iced::widget::container::Style {
                background: Some(Background::Color(separator(theme))),
                ..Default::default()
            }
        }))
        .into()
}

// ─── small slider ──────────────────────────────────────────────────────────

/// The small macOS slider: a 4 px rounded rail (accent fill, faint `on`
/// remainder) and a 16 px round knob — white with a hairline ring in
/// light mode, a light neutral in dark mode.
pub(crate) fn small_slider_class() -> cosmic::theme::iced::Slider {
    let style = Rc::new(|theme: &cosmic::Theme| {
        let on = Color::from(theme.cosmic().background(false).on);
        let knob = if theme.cosmic().is_dark {
            mix(on, Color::BLACK, 0.15)
        } else {
            Color::WHITE
        };
        slider::Style {
            rail: slider::Rail {
                backgrounds: (
                    Background::Color(Ink::Accent.color(theme)),
                    Background::Color(Color { a: 0.1, ..on }),
                ),
                width: 4.0,
                // square ends: the rounded caps come from `with_rail_ends`
                border: Border::default(),
            },
            handle: slider::Handle {
                shape: slider::HandleShape::Circle { radius: 8.0 },
                background: Background::Color(knob),
                border_width: 1.0,
                border_color: Color { a: 0.18, ..on },
            },
            breakpoint: slider::Breakpoint { color: on },
        }
    });
    cosmic::theme::iced::Slider::Custom {
        active: style.clone(),
        hovered: style.clone(),
        dragging: style,
    }
}

/// The small slider's knob diameter (and row height).
pub(crate) const KNOB: f32 = 16.0;

/// A small slider over `range`: the [`small_slider_class`] look, a
/// [`KNOB`]-sized knob, and the rail reaching the full width. iced insets
/// its rail by half the knob at both ends (the knob's travel), where
/// macOS runs the rail edge to edge; the two half-knob end caps are
/// drawn underneath — the leading cap in the fill color (the value never
/// sits left of the range start), the trailing one in the track color.
pub(crate) fn small_slider<'a, T, M>(
    range: std::ops::RangeInclusive<T>,
    value: T,
    on_change: impl Fn(T) -> M + 'a,
    width: Length,
) -> cosmic::iced::widget::Slider<'a, T, M, cosmic::Theme>
where
    T: Copy + From<u8> + PartialOrd,
    M: Clone + 'a,
{
    cosmic::iced::widget::Slider::new(range, value, on_change)
        .width(width)
        .height(KNOB)
        .handle_width(KNOB)
        .handle_height(KNOB)
        .class(small_slider_class())
}

/// Wrap a [`small_slider`] (or its tooltip) with the rail end caps.
pub(crate) fn with_rail_ends<'a, M: 'a>(
    track: impl Into<Element<'a, M>>,
    width: Length,
) -> Element<'a, M> {
    let cap = |leading: bool| {
        container(Space::new().width(KNOB / 2.0).height(4.0)).class(
            cosmic::theme::Container::custom(move |theme| {
                let on = Color::from(theme.cosmic().background(false).on);
                cosmic::iced::widget::container::Style {
                    background: Some(Background::Color(if leading {
                        Ink::Accent.color(theme)
                    } else {
                        Color { a: 0.1, ..on }
                    })),
                    border: Border {
                        radius: if leading {
                            [2.0, 0.0, 0.0, 2.0].into()
                        } else {
                            [0.0, 2.0, 2.0, 0.0].into()
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                }
            }),
        )
    };
    let ends = Row::new()
        .height(KNOB)
        .align_y(Alignment::Center)
        .push(cap(true))
        .push(Space::new().width(Length::Fill))
        .push(cap(false));
    cosmic::iced::widget::stack![ends, track.into()]
        .width(width)
        .into()
}

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
        .class(card_style(10.0, 6.0))
        .into()
}

/// The header button: the layers glyph + semibold "Parts", the group
/// count when more than one group, and the chevron — the whole row
/// toggles `collapsed` (PartsOverlay.swift:57-77).
fn header(tools: &ViewerTools, groups: &[PartGroup]) -> Element<'static, Message> {
    let mut label = Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(symbol(Symbol::SquareStack3dUp, 16, Ink::Primary))
        .push(
            text(fl!("parti"))
                .size(12)
                .font(cosmic::font::semibold())
                .class(Ink::Primary.text_class()),
        );
    if groups.len() > 1 {
        label = label.push(count_badge(groups.len()));
    }
    label = label.push(symbol(
        if tools.collapsed {
            Symbol::ChevronDown
        } else {
            Symbol::ChevronUp
        },
        14,
        Ink::Secondary,
    ));
    let help = if tools.collapsed {
        fl!("mostra-le-parti")
    } else {
        fl!("nascondi-le-parti")
    };
    let header = button::custom(label)
        .padding(0)
        .class(cosmic::theme::Button::Transparent)
        .on_press(Message::TogglePartsCollapsed)
        .name(help.clone());
    tooltip(
        header,
        text(help),
        cosmic::widget::tooltip::Position::Bottom,
    )
    .into()
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
        .map(|layout| (layout_label(layout), Message::SetLayout(layout)))
        .collect();
    tooltip(
        segmented(items, selected, !comparing, Length::Fill),
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
        .push(symbol(Symbol::ArrowsInward, 13, Ink::Secondary))
        .push(with_rail_ends(
            tooltip(
                distance,
                text(fl!("distanza-tra-le-parti")),
                cosmic::widget::tooltip::Position::Bottom,
            ),
            Length::Fill,
        ))
        .push(symbol(Symbol::ArrowsOutward, 13, Ink::Secondary))
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
                .size(12)
                .wrapping(Wrapping::None)
                .ellipsize(cosmic::iced::core::text::Ellipsize::Middle(
                    cosmic::iced::core::text::EllipsizeHeightLimit::Lines(1),
                ))
                .class(if is_hidden {
                    Ink::Tertiary.text_class()
                } else {
                    Ink::Primary.text_class()
                }),
        );
    if group.members.len() > 1 {
        cluster = cluster.push(
            text(fl!("lld-3", count = group.members.len()))
                .size(10)
                .class(Ink::Secondary.text_class()),
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
        row = row.push(icon_button(
            if is_attached {
                Symbol::Link
            } else {
                Symbol::LinkBadgePlus
            },
            16,
            if is_attached {
                Ink::Accent
            } else {
                Ink::Secondary
            },
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
    row = row.push(icon_button(
        if is_hidden {
            Symbol::EyeSlash
        } else {
            Symbol::Eye
        },
        18,
        Ink::Tertiary,
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
        .size(10)
        .class(Ink::Secondary.text_class())
}

/// The plate note (PartsOverlay.swift:37-42): shown in the effective
/// Plate layout while the agent has produced no plate parts.
fn plate_note() -> Element<'static, Message> {
    text(fl!(
        "l-agente-non-ha-ancora-preparato-il-piatto-di-stampa-le-parti-sono-affiancate-senza-ruotarle-chiedi-di-prepararlo-nella-chat"
    ))
    .size(10)
    .class(Ink::Secondary.text_class())
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
        .class(Ink::Primary.text_class());
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

/// The bottom-center section capsule: the X/Y/Z segmented picker
/// (`.fixedSize()`) and the position slider over T3's
/// `SLIDER_MIN..=SLIDER_MAX`, width 220, 10 apart, in a material capsule
/// padded 12 × 6.
pub fn view_section_controls(tools: &ViewerTools) -> Element<'static, Message> {
    let selected = section::ALL
        .iter()
        .position(|axis| *axis == tools.section_axis);
    let axes = segmented(
        section::ALL
            .iter()
            .map(|axis| (axis.title().to_string(), Message::SetSectionAxis(*axis)))
            .collect(),
        selected,
        true,
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
        .class(capsule_style())
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
        let fill = fill.unwrap_or_else(|| {
            let mut fallback = Color::from(theme.cosmic().background(false).on);
            fallback.a *= 0.4;
            fallback
        });
        cosmic::iced::widget::container::Style {
            background: Some(Background::Color(fill)),
            border: Border {
                color: separator(theme),
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

    /// Every symbol's SVG is shipped and parses as an SVG document.
    #[test]
    fn every_symbol_svg_is_shipped() {
        use Symbol::*;
        for symbol in [
            SquareStack3dUp,
            ChevronUp,
            ChevronDown,
            ChevronLeft,
            ChevronRight,
            ChevronUpChevronDown,
            Xmark,
            Eye,
            EyeSlash,
            Link,
            LinkBadgePlus,
            ArrowsInward,
            ArrowsOutward,
            SliderHorizontal3,
            MinusMagnifyingGlass,
            PlusMagnifyingGlass,
        ] {
            let svg = std::str::from_utf8(symbol.svg()).unwrap();
            assert!(
                svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"),
                "{symbol:?}"
            );
        }
    }
}
