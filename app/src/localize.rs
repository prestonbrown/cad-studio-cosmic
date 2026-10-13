//! The fluent localization scaffold (the cosmic-app-template shape over
//! libcosmic's own `localize.rs`): the M0-extracted catalogs under
//! `i18n/` embedded into the binary, `en` as the fallback language, and
//! the [`fl!`](crate::fl) macro. Only `en` is meaningfully complete in v1
//! (the it/de/fr catalogs ride along from the M0 extraction); landed
//! modules since M6 T9 read their strings through `fl!` at render time,
//! while the pre-M6 modules' string constants still name their keys —
//! their swap rides with the dedicated pass, because the catalog's
//! Swift-style `%@` placeholders must become fluent `{$args}` first (a
//! migration of 52 keys × 4 languages that belongs to the extractor
//! script, not a hand edit).

use std::sync::{LazyLock, OnceLock};

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

static SELECTION: OnceLock<()> = OnceLock::new();

pub fn localizer() -> Box<dyn Localizer> {
    Box::from(DefaultLocalizer::new(&*LANGUAGE_LOADER, &Localizations))
}

/// Select the user's requested languages against the embedded catalogs.
/// Idempotent (the [`fl!`](crate::fl) macro calls it before every lookup,
/// so a first use outside `main` still selects exactly once).
pub fn localize() {
    SELECTION.get_or_init(|| {
        let localizer = localizer();
        let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();
        if let Err(error) = localizer.select(&requested_languages) {
            eprintln!("cad-studio: language selection failed: {error}");
        }
    });
}

/// Select exactly `language` ("en", "it", …), overriding the desktop
/// request. The test seam that pins `fl!` output to one catalog so the
/// host locale cannot decide test outcomes.
pub fn select(language: &str) {
    let tag = language
        .parse::<i18n_embed::unic_langid::LanguageIdentifier>()
        .expect("a valid language tag");
    SELECTION.get_or_init(|| ());
    if let Err(error) = i18n_embed::select(&*LANGUAGE_LOADER, &Localizations, &[tag]) {
        eprintln!("cad-studio: language selection failed: {error}");
    }
}

/// Look up `message_id` in the selected catalog (en fallback). First use
/// selects the desktop's requested languages via [`localize`].
#[macro_export]
macro_rules! fl {
    ($message_id:literal) => {{
        $crate::localize::localize();
        $crate::localize::LANGUAGE_LOADER.get($message_id)
    }};
}
