//! Text and glyph colors by ROLE, resolved from the active theme.
//!
//! Upstream colors its text and SF Symbols with macOS's semantic label
//! hierarchy (`.primary`, `.secondary`, `.tertiary`, …): the label color at
//! falling opacities. The port resolves each role against the theme's
//! `background.on` color, so a light, dark, custom or high-contrast COSMIC
//! palette carries through, and the status roles against the theme's
//! accent / success / warning / destructive tokens.
//!
//! The opacity of a label role depends on what it sits on, as on macOS:
//! inside a material or the window titlebar, vibrancy draws labels lighter
//! than on a plain window or text background. [`Backdrop`] names those
//! three surfaces; [`Tone::alpha`] is the one table, measured off the
//! upstream light-mode gallery (shots/upstream/project-model.png in sRGB,
//! darkest label pixel against its own background):
//!
//! | backdrop | primary | secondary | tertiary | measured on |
//! |---|---|---|---|---|
//! | plain | 0.85 | 0.50 | 0.25 | the "Front" segment, the size label, chat text, the field placeholder, the schematic footer |
//! | material | 0.70 | 0.44 | 0.25 | the Parts card's names, count, chevron, link and eye glyphs |
//! | titlebar | 0.70 | 0.56 | 0.27 | the activity capsule's name / status / chevron, the toolbar glyphs, "v3", the idle Stop glyph |
//!
//! A high-contrast COSMIC theme reads a second row, the analogue of macOS's
//! Increase Contrast: every label role, hairline and fill is stronger
//! (primary 1.0, secondary 0.75, tertiary 0.55, quaternary and separator
//! 0.30), whatever the backdrop.

use cosmic::iced::Color;
use cosmic::iced::widget::text::Style;

/// What a label is drawn over: the surface decides its vibrancy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backdrop {
    /// A window or text background (the viewer bar, the chat pane, the
    /// schematic footer, plain windows and sheets).
    Plain,
    /// A floating material card or capsule (`.regularMaterial`: the Parts
    /// and Parameters overlays, the section/measure/motion capsules, the
    /// version and issue popovers).
    Material,
    /// The window's titlebar and toolbar.
    Titlebar,
}

/// The theme's contrast setting: which row of the alpha table applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Contrast {
    Standard,
    /// A high-contrast COSMIC palette.
    High,
}

impl Contrast {
    pub fn of(theme: &cosmic::Theme) -> Self {
        if theme.cosmic().is_high_contrast {
            Self::High
        } else {
            Self::Standard
        }
    }
}

/// A text or glyph role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// `.primary` / `labelColor`.
    Primary,
    /// `.secondary` / `secondaryLabelColor`.
    Secondary,
    /// `.tertiary` / `tertiaryLabelColor`; also every disabled control's
    /// label and glyph.
    Tertiary,
    /// `.quaternary` / `quaternaryLabelColor` (fills).
    Quaternary,
    /// `separatorColor`: hairlines and dividers.
    Separator,
    /// The theme accent (`controlAccentColor`).
    Accent,
    /// A glyph or label on an accent fill.
    OnAccent,
    /// A label on a strong neutral fill (the selected segment on a
    /// material): `alternateSelectedControlTextColor`, white in both
    /// appearances.
    OnFill,
    /// `.green` status.
    Success,
    /// `.yellow` / `.orange` status.
    Warning,
    /// `.red` status.
    Danger,
}

impl Tone {
    /// The role's opacity over its solid color on `backdrop` under
    /// `theme` (1 for the solid roles).
    pub fn alpha(self, theme: &cosmic::Theme, backdrop: Backdrop) -> f32 {
        self.alpha_in(Contrast::of(theme), backdrop)
    }

    /// The alpha table: one row per contrast setting.
    pub const fn alpha_in(self, contrast: Contrast, backdrop: Backdrop) -> f32 {
        match (contrast, self, backdrop) {
            (_, Self::Accent | Self::OnAccent | Self::OnFill, _)
            | (_, Self::Success | Self::Warning | Self::Danger, _) => 1.0,
            (Contrast::High, Self::Primary, _) => 1.0,
            (Contrast::High, Self::Secondary, _) => 0.75,
            (Contrast::High, Self::Tertiary, _) => 0.55,
            (Contrast::High, Self::Quaternary | Self::Separator, _) => 0.30,
            (Contrast::Standard, Self::Primary, Backdrop::Plain) => 0.85,
            (Contrast::Standard, Self::Primary, Backdrop::Material | Backdrop::Titlebar) => 0.70,
            (Contrast::Standard, Self::Secondary, Backdrop::Plain) => 0.50,
            (Contrast::Standard, Self::Secondary, Backdrop::Material) => 0.44,
            (Contrast::Standard, Self::Secondary, Backdrop::Titlebar) => 0.56,
            (Contrast::Standard, Self::Tertiary, Backdrop::Plain | Backdrop::Material) => 0.25,
            (Contrast::Standard, Self::Tertiary, Backdrop::Titlebar) => 0.27,
            (Contrast::Standard, Self::Quaternary | Self::Separator, _) => 0.10,
        }
    }

    /// The role's opaque color: the theme token it resolves to, before
    /// [`Tone::alpha`].
    pub fn solid(self, theme: &cosmic::Theme) -> Color {
        let cosmic = theme.cosmic();
        let opaque = |color: Color| Color { a: 1.0, ..color };
        match self {
            Self::Primary
            | Self::Secondary
            | Self::Tertiary
            | Self::Quaternary
            | Self::Separator => opaque(cosmic.background(false).on.into()),
            Self::Accent => opaque(cosmic.accent_color().into()),
            Self::OnAccent => opaque(cosmic.accent.on.into()),
            Self::OnFill => Color::WHITE,
            Self::Success => opaque(cosmic.success.base.into()),
            Self::Warning => opaque(cosmic.warning.base.into()),
            Self::Danger => opaque(cosmic.destructive.base.into()),
        }
    }

    /// The role's color on `backdrop`, alpha applied.
    pub fn color(self, theme: &cosmic::Theme, backdrop: Backdrop) -> Color {
        Color {
            a: self.alpha(theme, backdrop),
            ..self.solid(theme)
        }
    }

    /// The text class drawing this role on `backdrop`. Every label under
    /// a `Button::Transparent` needs an explicit class: cosmic's
    /// transparent button hands its content a fully transparent inherited
    /// text color.
    pub fn text(self, backdrop: Backdrop) -> cosmic::theme::Text {
        cosmic::theme::Text::Custom(TEXT_STYLES[self as usize][backdrop as usize])
    }
}

/// The on-background color at `alpha`: the translucent washes fills and
/// strokes are drawn in.
pub fn wash(theme: &cosmic::Theme, alpha: f32) -> Color {
    Color {
        a: alpha,
        ..Tone::Primary.solid(theme)
    }
}

/// `a` moved toward `b` by `t`, keeping `a`'s alpha.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a,
    }
}

/// The hover wash of a borderless control.
pub const HOVER: f32 = 0.06;
/// The pressed wash of a borderless control.
pub const PRESS: f32 = 0.12;

/// The selected-text highlight: the accent at a third, so the selected
/// glyphs stay readable through it.
pub fn selection(theme: &cosmic::Theme) -> Color {
    Color {
        a: 0.35,
        ..Tone::Accent.solid(theme)
    }
}

/// The text style for a role: its color plus the selection highlight the
/// selectable rows (chat) draw.
pub fn text_style(theme: &cosmic::Theme, tone: Tone, backdrop: Backdrop) -> Style {
    Style {
        color: Some(tone.color(theme, backdrop)),
        selected_fill: selection(theme),
        selected_text_color: None,
    }
}

const TONES: [Tone; 11] = [
    Tone::Primary,
    Tone::Secondary,
    Tone::Tertiary,
    Tone::Quaternary,
    Tone::Separator,
    Tone::Accent,
    Tone::OnAccent,
    Tone::OnFill,
    Tone::Success,
    Tone::Warning,
    Tone::Danger,
];
const BACKDROPS: [Backdrop; 3] = [Backdrop::Plain, Backdrop::Material, Backdrop::Titlebar];

/// cosmic's `Text::Custom` takes a plain `fn`, so every (role, backdrop)
/// pair gets its own monomorphized style function.
fn style_for<const T: usize, const B: usize>(theme: &cosmic::Theme) -> Style {
    text_style(theme, TONES[T], BACKDROPS[B])
}

macro_rules! row {
    ($t:literal) => {
        [
            style_for::<$t, 0> as fn(&cosmic::Theme) -> Style,
            style_for::<$t, 1>,
            style_for::<$t, 2>,
        ]
    };
}

const TEXT_STYLES: [[fn(&cosmic::Theme) -> Style; 3]; 11] = [
    row!(0),
    row!(1),
    row!(2),
    row!(3),
    row!(4),
    row!(5),
    row!(6),
    row!(7),
    row!(8),
    row!(9),
    row!(10),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A palette nobody ships: magenta text on a teal window, an orange
    /// accent, so a hard-coded black/white or macOS blue cannot pass.
    fn custom_theme() -> cosmic::Theme {
        use cosmic::cosmic_theme::palette::{Srgb, Srgba};
        let mut builder = cosmic::cosmic_theme::ThemeBuilder::light();
        let palette = builder.palette.as_mut();
        for neutral in [
            &mut palette.neutral_7,
            &mut palette.neutral_8,
            &mut palette.neutral_9,
            &mut palette.neutral_10,
        ] {
            *neutral = Srgba::new(0.55, 0.1, 0.45, 1.0);
        }
        builder.bg_color = Some(Srgba::new(0.6, 0.85, 0.8, 1.0));
        let builder = builder.accent(Srgb::new(0.9, 0.5, 0.1));
        cosmic::Theme::custom(std::sync::Arc::new(builder.build()))
    }

    #[test]
    fn the_custom_palette_is_neither_black_nor_white() {
        let on = Color::from(custom_theme().cosmic().background(false).on);
        assert_ne!((on.r, on.g, on.b), (0.0, 0.0, 0.0));
        assert_ne!((on.r, on.g, on.b), (1.0, 1.0, 1.0));
    }

    #[test]
    fn the_table_is_index_aligned() {
        for (index, tone) in TONES.iter().enumerate() {
            assert_eq!(*tone as usize, index);
        }
        for (index, backdrop) in BACKDROPS.iter().enumerate() {
            assert_eq!(*backdrop as usize, index);
        }
    }

    #[test]
    fn label_roles_follow_a_custom_palette() {
        let theme = custom_theme();
        let on = Color::from(theme.cosmic().background(false).on);
        for backdrop in BACKDROPS {
            for tone in [
                Tone::Primary,
                Tone::Secondary,
                Tone::Tertiary,
                Tone::Separator,
            ] {
                let color = tone.color(&theme, backdrop);
                assert_eq!((color.r, color.g, color.b), (on.r, on.g, on.b), "{tone:?}");
                assert_eq!(color.a, tone.alpha(&theme, backdrop));
            }
        }
        let accent = Color::from(theme.cosmic().accent_color());
        let drawn = Tone::Accent.color(&theme, Backdrop::Plain);
        assert_eq!((drawn.r, drawn.g, drawn.b), (accent.r, accent.g, accent.b));
        assert_eq!(
            Tone::Danger.color(&theme, Backdrop::Plain),
            Color {
                a: 1.0,
                ..Color::from(theme.cosmic().destructive.base)
            }
        );
    }

    #[test]
    fn the_text_class_draws_the_role() {
        let theme = custom_theme();
        for tone in TONES {
            for backdrop in BACKDROPS {
                let cosmic::theme::Text::Custom(style) = tone.text(backdrop) else {
                    panic!("custom class");
                };
                assert_eq!(style(&theme).color, Some(tone.color(&theme, backdrop)));
            }
        }
    }

    #[test]
    fn the_hierarchy_falls_on_every_backdrop() {
        for contrast in [Contrast::Standard, Contrast::High] {
            for backdrop in BACKDROPS {
                let alpha = |tone: Tone| tone.alpha_in(contrast, backdrop);
                assert!(alpha(Tone::Primary) > alpha(Tone::Secondary));
                assert!(alpha(Tone::Secondary) > alpha(Tone::Tertiary));
                assert!(alpha(Tone::Tertiary) > alpha(Tone::Quaternary));
            }
        }
        // vibrancy lightens labels inside materials
        let standard = |backdrop| Tone::Primary.alpha_in(Contrast::Standard, backdrop);
        assert!(standard(Backdrop::Material) < standard(Backdrop::Plain));
    }

    /// A high-contrast palette strengthens the faint roles, not only their
    /// hue: disabled glyphs and hairlines stay legible.
    #[test]
    fn a_high_contrast_theme_strengthens_the_faint_roles() {
        for (builder, standard) in [
            (
                cosmic::cosmic_theme::ThemeBuilder::light_high_contrast(),
                cosmic::cosmic_theme::ThemeBuilder::light(),
            ),
            (
                cosmic::cosmic_theme::ThemeBuilder::dark_high_contrast(),
                cosmic::cosmic_theme::ThemeBuilder::dark(),
            ),
        ] {
            let high = cosmic::Theme::custom(std::sync::Arc::new(builder.build()));
            let plain = cosmic::Theme::custom(std::sync::Arc::new(standard.build()));
            assert_eq!(Contrast::of(&high), Contrast::High);
            assert_eq!(Contrast::of(&plain), Contrast::Standard);
            for backdrop in BACKDROPS {
                for tone in [Tone::Secondary, Tone::Tertiary, Tone::Separator] {
                    assert!(
                        tone.color(&high, backdrop).a > tone.color(&plain, backdrop).a,
                        "{tone:?} on {backdrop:?}"
                    );
                }
                assert!(Tone::Tertiary.color(&high, backdrop).a > 0.25);
            }
        }
    }
}
