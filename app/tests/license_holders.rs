//! The About line names every copyright holder of the repository LICENSE,
//! in every catalog.

use std::path::PathBuf;

use app::windows::settings::UPSTREAM_BASE;

const HOLDERS: [&str; 2] = ["Filippo Finke", "Preston Brown"];

fn read(rel: &str) -> String {
    std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn about_copyright_names_both_license_holders_in_every_catalog() {
    let license = read("../LICENSE");
    for holder in HOLDERS {
        assert!(license.contains(holder), "LICENSE lacks {holder}");
    }
    for lang in ["en", "it", "de", "fr"] {
        let ftl = read(&format!("i18n/{lang}/main.ftl"));
        let start = ftl
            .lines()
            .position(|l| l.starts_with("about-copyright ="))
            .unwrap_or_else(|| panic!("{lang}: no about-copyright entry"));
        // The entry runs through its indented continuation lines.
        let entry: Vec<&str> = ftl
            .lines()
            .skip(start)
            .enumerate()
            .take_while(|(i, l)| *i == 0 || l.starts_with(' '))
            .map(|(_, l)| l)
            .collect();
        let line = entry.join("\n");
        for holder in HOLDERS {
            assert!(line.contains(holder), "{lang}: {line:?} lacks {holder}");
        }
        assert!(line.contains("MIT License"), "{lang}: {line:?}");
    }
}

/// Both names ride in the credit line, and the upstream base version is
/// stated once (UPSTREAM_BASE) and appears in every About catalog entry and
/// in the metainfo description.
#[test]
fn about_credits_original_author_and_port_author_and_names_the_upstream_base() {
    assert_eq!(UPSTREAM_BASE, "1.2.0");
    let credit = [
        (
            "en",
            "Original app by Filippo Finke. COSMIC/Linux port by Preston Brown.",
        ),
        (
            "it",
            "App originale di Filippo Finke. Port COSMIC/Linux di Preston Brown.",
        ),
        (
            "de",
            "Ursprüngliche App von Filippo Finke. COSMIC/Linux-Portierung von Preston Brown.",
        ),
        (
            "fr",
            "Application originale de Filippo Finke. Portage COSMIC/Linux par Preston Brown.",
        ),
    ];
    for (lang, line) in credit {
        let ftl = read(&format!("i18n/{lang}/main.ftl"));
        assert!(ftl.contains(line), "{lang}: credit line missing");
        let base = ftl
            .lines()
            .find(|l| l.starts_with("about-upstream-base ="))
            .unwrap_or_else(|| panic!("{lang}: no about-upstream-base entry"));
        assert!(
            base.contains("cad-studio") && base.contains("$base"),
            "{lang}: {base:?}"
        );
    }
    let metainfo = read("resources/packaging/net.brownhouse.CadStudio.metainfo.xml");
    assert!(
        metainfo.contains(&format!("Based on cad-studio {UPSTREAM_BASE}")),
        "the metainfo description states the upstream base"
    );
}
