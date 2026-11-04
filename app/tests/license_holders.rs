//! The About line names every copyright holder of the repository LICENSE,
//! in every catalog.

use std::path::PathBuf;

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
