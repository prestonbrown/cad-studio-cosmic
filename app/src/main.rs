//! The binary: run the CAD Studio application. The welcome window is the
//! main window, so its geometry seeds the application settings; every
//! project window sizes its own `window::open` (upstream
//! `.defaultSize(1320, 840)`, CADStudioApp.swift:25).

use cosmic::app as cosmic_app;
use cosmic::iced::Size;

fn main() -> cosmic::iced::Result {
    // Packaging checks run the binary on hosts without a display.
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("cad-studio {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    // The persisted language override applies at THIS launch boundary —
    // upstream's `AppleLanguages` next-launch semantics: select before any
    // fl! string resolves, never again (the settings window writes the
    // field; a restart picks it up).
    let language = app::settings_store::SettingsStore::global().load().language;
    app::localize::localize_with_override(language.as_deref());
    let settings = cosmic_app::Settings::default().size(Size::new(
        app::windows::welcome::ACTIONS_WIDTH + app::windows::welcome::RECENTS_WIDTH,
        app::windows::welcome::WINDOW_HEIGHT,
    ));
    cosmic_app::run::<app::app::CadStudio>(settings, ())
}
