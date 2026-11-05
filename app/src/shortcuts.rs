//! Keyboard bindings as data. A binding is libcosmic's [`KeyBind`]: the key
//! handlers match against it (with the physical-key fallback for non-Latin
//! layouts), the menu bar builds its accelerator column from the same
//! values, and every tooltip that names a key renders the `KeyBind` the menu
//! shows, so a hint cannot differ from its binding.

use cosmic::iced::keyboard::Key;
use cosmic::iced::keyboard::key::Named;
pub use cosmic::widget::menu::key_bind::{KeyBind, Modifier};

use crate::fl;
use crate::windows::project::{Command, binding, bindings};

/// `key` held with `modifiers`. A character key is its lowercase
/// character; the named keys the app binds are spelled `Escape`, `F9`,
/// `F11`.
pub fn bind(modifiers: &[Modifier], key: &str) -> KeyBind {
    let key = match key {
        "Escape" => Key::Named(Named::Escape),
        "F9" => Key::Named(Named::F9),
        "F11" => Key::Named(Named::F11),
        character => Key::Character(character.into()),
    };
    KeyBind {
        modifiers: modifiers.to_vec(),
        key,
    }
}

/// `key` alone.
pub fn bare(key: &str) -> KeyBind {
    bind(&[], key)
}

/// `Ctrl` + `key`.
pub fn ctrl(key: &str) -> KeyBind {
    bind(&[Modifier::Ctrl], key)
}

/// `Ctrl` + `Shift` + `key`.
pub fn ctrl_shift(key: &str) -> KeyBind {
    bind(&[Modifier::Ctrl, Modifier::Shift], key)
}

/// What must be true of a window for a binding to fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Always,
    /// A turn or build is running.
    Running,
    /// The 3D viewer pane holds the keyboard focus.
    ViewerFocused,
}

/// The window state [`When`] reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Context {
    pub running: bool,
    pub viewer_focused: bool,
}

impl When {
    pub fn holds(self, context: Context) -> bool {
        match self {
            Self::Always => true,
            Self::Running => context.running,
            Self::ViewerFocused => context.viewer_focused,
        }
    }
}

/// The system paste binding the chat input honours.
pub fn paste() -> KeyBind {
    ctrl("v")
}

fn keys(command: Command) -> String {
    binding(command).to_string()
}

/// Every binding of `command`, joined.
fn all_keys(command: Command) -> String {
    bindings(command)
        .map(|shortcut| shortcut.to_string())
        .collect::<Vec<_>>()
        .join(" / ")
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

/// The stop tooltip lists every binding that stops: Esc while running
/// and the always-available shortcut.
pub fn stop() -> String {
    fl!("interrompi", keys = all_keys(Command::Stop))
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
        keys = paste().to_string()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic::iced::keyboard::Modifiers;
    use cosmic::iced::keyboard::key::{Code, Physical};

    #[test]
    fn labels_spell_each_modifier_combination() {
        assert_eq!(ctrl("b").to_string(), "Ctrl + B");
        assert_eq!(ctrl_shift("e").to_string(), "Ctrl + Shift + E");
        assert_eq!(
            bind(&[Modifier::Ctrl, Modifier::Alt], "i").to_string(),
            "Ctrl + Alt + I"
        );
        assert_eq!(ctrl("0").to_string(), "Ctrl + 0");
        assert_eq!(ctrl(".").to_string(), "Ctrl + .");
        assert_eq!(bare("F9").to_string(), "F9");
        assert_eq!(bare("Escape").to_string(), "Escape");
    }

    #[test]
    fn matching_requires_exactly_the_modifiers() {
        let key = Key::Character("E".into());
        let shortcut = ctrl_shift("e");
        let shift_ctrl = Modifiers::CTRL | Modifiers::SHIFT;
        assert!(shortcut.matches(shift_ctrl, &key, None));
        assert!(!shortcut.matches(Modifiers::CTRL, &key, None));
        assert!(!shortcut.matches(shift_ctrl | Modifiers::ALT, &key, None));
        assert!(!shortcut.matches(shift_ctrl, &Key::Character("r".into()), None));
    }

    /// A Cyrillic layout reports `ц` for the physical W key; Ctrl+W still
    /// closes the window.
    #[test]
    fn a_non_latin_layout_matches_by_physical_key() {
        let close = ctrl("w");
        let cyrillic = Key::Character("ц".into());
        assert!(close.matches(
            Modifiers::CTRL,
            &cyrillic,
            Some(&Physical::Code(Code::KeyW))
        ));
        assert!(!close.matches(
            Modifiers::CTRL,
            &cyrillic,
            Some(&Physical::Code(Code::KeyQ))
        ));
        assert!(!close.matches(Modifiers::CTRL, &cyrillic, None));
    }

    /// Each hint names its command's registered binding, the text the menu
    /// shows for it.
    #[test]
    fn every_hint_shows_its_commands_binding() {
        let _lang = crate::localize::pin_en();
        for (command, hint) in HINTS {
            let label = binding(*command).to_string();
            let text = hint();
            assert!(text.contains(&label), "{command:?}: {text:?} lacks {label}");
        }
        assert!(attach().contains("Ctrl + V"), "{}", attach());
        assert!(hide_parameters().contains(&binding(Command::ToggleParameters).to_string()));
    }

    /// The accelerator the menu draws for a command is the text its
    /// tooltip carries.
    #[test]
    fn menu_accelerators_equal_the_hint_labels() {
        let binds = crate::windows::project::menu_key_binds();
        for (command, _) in HINTS {
            let menu_label = binds
                .iter()
                .find(|(_, bound)| bound == &command)
                .map(|(key, _)| key.to_string())
                .unwrap_or_else(|| panic!("{command:?} has no menu accelerator"));
            assert_eq!(menu_label, binding(*command).to_string());
        }
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

    #[test]
    fn named_keys_match() {
        let escape = Key::Named(Named::Escape);
        assert!(bare("Escape").matches(Modifiers::empty(), &escape, None));
        assert!(!bare("Escape").matches(Modifiers::CTRL, &escape, None));
        let f9 = Key::Named(Named::F9);
        assert!(bare("F9").matches(Modifiers::empty(), &f9, None));
    }

    #[test]
    fn the_stop_hint_lists_escape_and_the_always_on_binding() {
        let _lang = crate::localize::pin_en();
        let hint = stop();
        assert!(
            hint.contains("Escape") && hint.contains("Ctrl + ."),
            "{hint}"
        );
    }
}
