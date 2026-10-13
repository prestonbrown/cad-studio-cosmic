//! The binary: run the CAD Studio application. The welcome window is the
//! main window, so its geometry seeds the application settings; every
//! project window sizes its own `window::open` (upstream
//! `.defaultSize(1320, 840)`, CADStudioApp.swift:25).

use cosmic::app as cosmic_app;
use cosmic::iced::Size;

fn main() -> cosmic::iced::Result {
    app::localize::localize();
    let settings = cosmic_app::Settings::default().size(Size::new(
        app::windows::welcome::ACTIONS_WIDTH + app::windows::welcome::RECENTS_WIDTH,
        app::windows::welcome::WINDOW_HEIGHT,
    ));
    cosmic_app::run::<app::app::CadStudio>(settings, ())
}
