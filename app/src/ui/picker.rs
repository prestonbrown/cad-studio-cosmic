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
