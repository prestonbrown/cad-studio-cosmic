//! A grouped Form's `Picker` (macOS's menu-style pop-up in a form row):
//! the selected value in primary text, then the up/down chevron
//! (`chevron.up.chevron.down`) in a small rounded box, hugging the row's
//! trailing edge. A press opens the option list.
//!
//! Geometry off upstream (shots/upstream/settings-general.png, 1x): the
//! chevron box 16 x 16, 6 px after the value, filled one step darker than
//! the form box; the chevrons about 9 px tall.
//!
//! libcosmic's dropdown draws a fixed `pan-down-symbolic` glyph from the
//! icon theme (src/widget/dropdown/widget.rs:420 at the pinned rev), so
//! the picker is iced's `pick_list` with its handle hidden and its own
//! text left empty: the list keeps the hit area (as wide as the widest
//! option) and the option menu, and the face drawn over it, right-aligned,
//! shows the value and the chevron box.

use cosmic::Element;
use cosmic::iced::widget::pick_list::{Handle, PickList};
use cosmic::iced::widget::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{Row, container, text};

use super::icon::{self, Symbol};
use super::surface::{self, Corner};
use super::tone::{Backdrop, Tone};
use crate::typography;

/// The chevron box's side.
const BOX: f32 = 16.0;
/// The chevron symbol's height (its drawing fills about 9 px of it).
const GLYPH: f32 = BOX;
/// Between the value and the chevron box.
const GAP: f32 = 6.0;
/// The chevron box fill against [`Tone::Quaternary`] (upstream's box
/// reads 13 levels under the 232 form box).
const BOX_FILL: f32 = 0.7;

/// A form picker over `options`, showing `selected`; `on_select` gets the
/// chosen option's index.
pub fn picker<'a, M: Clone + 'a>(
    options: Vec<String>,
    selected: Option<usize>,
    on_select: impl Fn(usize) -> M + 'a,
) -> Element<'a, M> {
    let value = selected
        .and_then(|index| options.get(index))
        .cloned()
        .unwrap_or_default();
    let choices: Vec<Choice> = options
        .into_iter()
        .enumerate()
        .map(|(index, label)| Choice { index, label })
        .collect();
    // the list's own selection stays empty: a selected label would draw
    // in the list's face under the right-aligned one, and the pinned
    // theme's pick_list class is `()` (no text color to hide it with)
    let list = PickList::new(choices, None::<Choice>, move |chosen: Choice| {
        on_select(chosen.index)
    })
    .placeholder("")
    .text_size(typography::BODY)
    // the list reserves one text size for its own (hidden) handle
    .padding([0.0, GAP + BOX - typography::BODY, 0.0, 0.0])
    .handle(Handle::None);
    let face = Row::new()
        .spacing(GAP)
        .align_y(Alignment::Center)
        .push(
            text(value)
                .size(typography::BODY)
                .wrapping(Wrapping::None)
                .class(Tone::Primary.text(Backdrop::Plain)),
        )
        .push(chevron_box());
    cosmic::iced::widget::stack![
        list,
        container(face)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::End)
            .align_y(Alignment::Center),
    ]
    .into()
}

/// A label glyph's average advance in ems, for placing the pull-down's
/// chevron after a label the list is wider than (iced measures no text at
/// view time; a digit's advance in the interface families is ~0.57 em).
const ADVANCE: f32 = 0.57;

/// A borderless pull-down (`.menuStyle(.borderlessButton)`): `label` at
/// `size` followed by a `chevron` px `chevron.down`, opening `items`;
/// `on_select` gets the chosen item's index. The label is the list's
/// placeholder, so the closed face can read differently from the items
/// (the schematic's "n/N · title" over "n. title" entries, the motion
/// capsule's speed over its checked speed list). The list is as wide as
/// its widest entry or the label; the chevron, drawn over it and taking no
/// input, follows the label ([`chevron_offset`]).
pub fn pulldown<'a, M: Clone + 'a>(
    items: Vec<String>,
    label: String,
    size: f32,
    chevron: f32,
    backdrop: Backdrop,
    on_select: impl Fn(usize) -> M + 'a,
) -> Element<'a, M> {
    let offset = chevron_offset(&items, &label, size);
    let choices: Vec<Choice> = items
        .into_iter()
        .enumerate()
        .map(|(index, label)| Choice { index, label })
        .collect();
    let list = PickList::new(choices, None::<Choice>, move |chosen: Choice| {
        on_select(chosen.index)
    })
    .placeholder(label)
    .text_size(size)
    // the list reserves one text size for its own (hidden) handle
    .padding([0.0, chevron + 2.0 - size, 0.0, PULLDOWN_LEAD])
    .handle(Handle::None);
    let glyph = icon::icon(Symbol::ChevronDown, chevron, Tone::Primary, backdrop);
    let face = match offset {
        Some(offset) => container(glyph)
            .padding([0.0, 0.0, 0.0, offset])
            .align_x(Alignment::Start),
        None => container(glyph).align_x(Alignment::End),
    };
    cosmic::iced::widget::stack![
        list,
        face.width(Length::Fill)
            .height(Length::Fill)
            .align_y(Alignment::Center),
    ]
    .into()
}

/// The pull-down list's leading padding before its label.
const PULLDOWN_LEAD: f32 = 4.0;

/// Where the pull-down's chevron starts: `None` (the list's trailing
/// edge, exact) when the label is the list's widest text, else just past
/// the label's estimated advance, since an entry wider than the label
/// stretches the list beyond it.
fn chevron_offset(items: &[String], label: &str, size: f32) -> Option<f32> {
    let chars = |text: &str| text.chars().count();
    let widest = items.iter().map(|item| chars(item)).max().unwrap_or(0);
    (widest > chars(label))
        .then(|| PULLDOWN_LEAD + chars(label) as f32 * size * ADVANCE + 2.0)
}

/// One option: its label, shown in the list, and its index, which the
/// selection reports (so a label never has to be looked up again).
#[derive(Debug, Clone, PartialEq)]
struct Choice {
    index: usize,
    label: String,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// The rounded box holding the up/down chevron.
fn chevron_box<'a, M: 'a>() -> Element<'a, M> {
    container(icon::icon(
        Symbol::ChevronUpChevronDown,
        GLYPH,
        Tone::Primary,
        Backdrop::Plain,
    ))
    .width(Length::Fixed(BOX))
    .height(Length::Fixed(BOX))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .class(surface::fill(Tone::Quaternary, BOX_FILL, Corner::Xs, None))
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chevron rides the trailing edge while the label is the widest
    /// text, and follows the label once an entry is wider.
    #[test]
    fn the_chevron_follows_a_label_narrower_than_the_list() {
        let pages = vec!["1. Assembly".to_string(), "2. Base".to_string()];
        assert_eq!(chevron_offset(&pages, "1/2 · Assembly", 11.0), None);
        let speeds = vec!["✓\u{2002}1×".to_string(), "\u{2002}\u{2002}Loop".to_string()];
        let offset = chevron_offset(&speeds, "1×", 12.0).unwrap();
        assert!((offset - (PULLDOWN_LEAD + 2.0 * 12.0 * ADVANCE + 2.0)).abs() < 1e-4);
    }

    #[test]
    fn a_choice_shows_its_label_and_keeps_its_index() {
        let choice = Choice {
            index: 2,
            label: "Deutsch".to_string(),
        };
        assert_eq!(choice.to_string(), "Deutsch");
        assert_eq!(choice.index, 2);
    }
}
