//! Keyboard bindings as data. The key handlers match against [`Shortcut`]
//! values and every tooltip that names a key renders the same value through
//! [`Shortcut::label`], so a hint cannot differ from its binding.

use cosmic::iced::keyboard::{Key, Modifiers};

use crate::fl;
use crate::windows::project::{Command, binding};

/// A key plus the modifiers that must be held with it (and only them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    ctrl: bool,
    alt: bool,
    shift: bool,
    key: &'static str,
}

impl Shortcut {
    /// `Ctrl` + `key`; `key` is the lowercase character the key produces.
    pub const fn ctrl(key: &'static str) -> Self {
        Self {
            ctrl: true,
            alt: false,
            shift: false,
            key,
        }
    }

    pub const fn with_alt(mut self) -> Self {
        self.alt = true;
        self
    }

    pub const fn with_shift(mut self) -> Self {
        self.shift = true;
        self
    }

    /// Whether the press of `key` with `modifiers` is this shortcut.
    pub fn matches(&self, key: &Key, modifiers: &Modifiers) -> bool {
        let Key::Character(character) = key else {
            return false;
        };
        modifiers.control() == self.ctrl
            && modifiers.alt() == self.alt
            && modifiers.shift() == self.shift
            && character.to_lowercase() == self.key
    }

    /// The binding as the desktop writes it: `Ctrl+Shift+E`, `Ctrl+Plus`.
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_owned());
        }
        if self.alt {
            parts.push("Alt".to_owned());
        }
        if self.shift {
            parts.push("Shift".to_owned());
        }
        parts.push(match self.key {
            "+" => "Plus".to_owned(),
            "-" => "Minus".to_owned(),
            key => key.to_uppercase(),
        });
        parts.join("+")
    }
}

/// The system paste binding the chat input honours.
pub const PASTE: Shortcut = Shortcut::ctrl("v");

fn keys(command: Command) -> String {
    binding(command).label()
}

/// Tooltips that name a project-window command's key, each paired with the
/// command it names.
pub const HINTS: &[(Command, fn() -> String)] = &[
    (Command::Build, build),
    (Command::Stop, stop),
    (Command::ShowChat, show_chat),
    (Command::Export, export),
    (Command::ToggleParameters, toggle_parameters),
    (Command::SchematicZoomIn, zoom_in),
    (Command::SchematicZoomOut, zoom_out),
];

pub fn build() -> String {
    fl!("compila-modello-b", keys = keys(Command::Build))
}

pub fn stop() -> String {
    fl!("interrompi", keys = keys(Command::Stop))
}

pub fn show_chat() -> String {
    fl!(
        "mostra-o-nascondi-la-chat-0",
        keys = keys(Command::ShowChat)
    )
}

pub fn export() -> String {
    fl!(
        "esporta-step-3mf-stl-tavola-pdf-e-model-py-in-un-archivio-zip-e",
        keys = keys(Command::Export)
    )
}

/// The toolbar's show/hide tooltip.
pub fn toggle_parameters() -> String {
    fl!(
        "mostra-o-nascondi-i-parametri-i",
        keys = keys(Command::ToggleParameters)
    )
}

/// The parameters overlay's close button tooltip.
pub fn hide_parameters() -> String {
    fl!(
        "nascondi-parametri-i",
        keys = keys(Command::ToggleParameters)
    )
}

pub fn zoom_in() -> String {
    fl!("ingrandisci-2", keys = keys(Command::SchematicZoomIn))
}

pub fn zoom_out() -> String {
    fl!("riduci-2", keys = keys(Command::SchematicZoomOut))
}

pub fn attach() -> String {
    fl!(
        "allega-immagini-o-file-di-riferimento-puoi-anche-incollarli-con-v",
        keys = PASTE.label()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_spell_each_modifier_combination() {
        assert_eq!(Shortcut::ctrl("b").label(), "Ctrl+B");
        assert_eq!(Shortcut::ctrl("e").with_shift().label(), "Ctrl+Shift+E");
        assert_eq!(Shortcut::ctrl("i").with_alt().label(), "Ctrl+Alt+I");
        assert_eq!(
            Shortcut::ctrl("i").with_alt().with_shift().label(),
            "Ctrl+Alt+Shift+I"
        );
        assert_eq!(Shortcut::ctrl("0").label(), "Ctrl+0");
        assert_eq!(Shortcut::ctrl(".").label(), "Ctrl+.");
        assert_eq!(Shortcut::ctrl("+").label(), "Ctrl+Plus");
        assert_eq!(Shortcut::ctrl("-").label(), "Ctrl+Minus");
    }

    #[test]
    fn matching_requires_exactly_the_modifiers() {
        let key = Key::Character("E".into());
        let shortcut = Shortcut::ctrl("e").with_shift();
        assert!(shortcut.matches(&key, &(Modifiers::CTRL | Modifiers::SHIFT)));
        assert!(!shortcut.matches(&key, &Modifiers::CTRL));
        assert!(!shortcut.matches(&key, &(Modifiers::CTRL | Modifiers::SHIFT | Modifiers::ALT)));
        assert!(!shortcut.matches(
            &Key::Character("r".into()),
            &(Modifiers::CTRL | Modifiers::SHIFT)
        ));
    }

    /// Each hint names its command's registered binding.
    #[test]
    fn every_hint_shows_its_commands_binding() {
        let _lang = crate::localize::pin_en();
        for (command, hint) in HINTS {
            let label = binding(*command).label();
            let text = hint();
            assert!(text.contains(&label), "{command:?}: {text:?} lacks {label}");
        }
        assert!(attach().contains("Ctrl+V"), "{}", attach());
        assert!(hide_parameters().contains(&binding(Command::ToggleParameters).label()));
    }

    #[test]
    fn no_catalog_value_carries_a_mac_modifier_glyph() {
        for (lang, ftl) in [
            ("en", include_str!("../i18n/en/main.ftl")),
            ("it", include_str!("../i18n/it/main.ftl")),
            ("de", include_str!("../i18n/de/main.ftl")),
            ("fr", include_str!("../i18n/fr/main.ftl")),
        ] {
            for line in ftl.lines() {
                assert!(!line.contains(['⌘', '⌥', '⇧', '⌃']), "{lang}: {line}");
            }
        }
    }
}
