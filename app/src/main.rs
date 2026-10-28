//! The binary: run the CAD Studio application. The welcome window is the
//! main window, so its geometry seeds the application settings; every
//! project window sizes its own `window::open` (upstream
//! `.defaultSize(1320, 840)`, CADStudioApp.swift:25).

use cosmic::app as cosmic_app;
use cosmic::iced::Size;

fn main() -> cosmic::iced::Result {
    // The persisted language override applies at THIS launch boundary —
    // upstream's `AppleLanguages` next-launch semantics: select before any
    // fl! string resolves, never again (the settings window writes the
    // field; a restart picks it up).
    let language = engine::settings::Settings::load(&engine::settings::settings_path())
        .ok()
        .and_then(|settings| settings.language);
    app::localize::localize_with_override(language.as_deref());
    let settings = cosmic_app::Settings::default().size(Size::new(
        app::windows::welcome::ACTIONS_WIDTH + app::windows::welcome::RECENTS_WIDTH,
        app::windows::welcome::WINDOW_HEIGHT,
    ));
    cosmic_app::run::<app::app::CadStudio>(settings, ())
}
