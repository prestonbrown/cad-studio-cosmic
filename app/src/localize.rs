//! The fluent localization scaffold (the cosmic-app-template shape over
//! libcosmic's own `localize.rs`): the M0-extracted catalogs under
//! `i18n/` embedded into the binary, `en` as the fallback language, and
//! the [`fl!`](crate::fl) macro for future wiring. Only `en` is
//! meaningfully complete in v1 (the it/de/fr catalogs ride along from the
//! M0 extraction); the landed modules' string constants document their
//! keys and swap to `fl!()` in a dedicated pass — the catalog's
//! Swift-style `%@` placeholders must become fluent `{$args}` first (a
//! migration of 52 keys × 4 languages that belongs to the extractor
//! script, not a hand edit).

use std::sync::LazyLock;

use i18n_embed::fluent::{FluentLanguageLoader, fluent_language_loader};
use i18n_embed::{DefaultLocalizer, LanguageLoader, Localizer};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

pub static LANGUAGE_LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader: FluentLanguageLoader = fluent_language_loader!();
    loader
        .load_fallback_language(&Localizations)
        .expect("the embedded en catalog must load");
    loader
});

pub fn localizer() -> Box<dyn Localizer> {
    Box::from(DefaultLocalizer::new(&*LANGUAGE_LOADER, &Localizations))
}

/// Select the user's requested languages against the embedded catalogs.
/// Called once from `main` before the first view renders.
pub fn localize() {
    let localizer = localizer();
    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();
    if let Err(error) = localizer.select(&requested_languages) {
        eprintln!("cad-studio: language selection failed: {error}");
    }
}
