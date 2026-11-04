//! packaging tests: the freedesktop metadata under
//! `resources/packaging/` pins to the app's identity. The desktop entry
//! must parse as a keyfile whose Icon and StartupWMClass equal the
//! icon-name law's APP_ID alias (hicolor lookup by APP_ID), with Exec a
//! single BIN placeholder naming the cargo bin that `just install`
//! rewrites to the installed path; the metainfo must parse as XML
//! carrying the same id plus the version placeholder the recipe
//! substitutes from CARGO_PKG_VERSION; the three rasters the recipe
//! installs must decode at their hicolor sizes.

use std::collections::BTreeMap;
use std::path::PathBuf;

use app::sheets::APP_ICON;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

fn resource(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn desktop_file() -> String {
    std::fs::read_to_string(resource(
        "resources/packaging/net.brownhouse.CadStudio.desktop",
    ))
    .expect("the desktop entry ships with the repo")
}

/// Keyfile parse ([group] headers, `key=value` lines, `#` comments, blank
/// lines) — any other shape fails the pin.
fn keyfile(text: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut groups: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut group = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            group = name.to_string();
            groups.entry(group.clone()).or_default();
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .unwrap_or_else(|| panic!("desktop entry line is not key=value: {line:?}"));
        groups
            .entry(group.clone())
            .or_default()
            .insert(key.to_string(), value.to_string());
    }
    groups
}

#[test]
fn desktop_entry_is_one_group_with_the_required_keys() {
    let groups = keyfile(&desktop_file());
    assert_eq!(
        groups.keys().collect::<Vec<_>>(),
        ["Desktop Entry"],
        "exactly the one group"
    );
    let entry = &groups["Desktop Entry"];
    for (key, value) in [
        ("Type", "Application"),
        ("Version", "1.5"),
        ("Name", "CAD Studio"),
        ("Terminal", "false"),
        ("Categories", "Utility;Engineering;"),
        ("Exec", "@BIN@"),
    ] {
        assert_eq!(entry.get(key), Some(&value.to_string()), "key {key}");
    }
    assert!(
        entry.get("Comment").is_some_and(|c| !c.is_empty()),
        "the tooltip line ships"
    );
}

#[test]
fn desktop_entry_identity_keys_equal_the_icon_name_app_id() {
    let entry = &keyfile(&desktop_file())["Desktop Entry"];
    // APP_ICON is the public alias documented "keep in sync with
    // CadStudio::APP_ID"; the sheets' icon-by-name lookups resolve only
    // when the hicolor icon is installed under exactly this name.
    assert_eq!(entry["Icon"], APP_ICON);
    assert_eq!(entry["StartupWMClass"], APP_ICON);
}

#[test]
fn desktop_entry_carries_one_bin_placeholder_for_the_cargo_bin() {
    let text = desktop_file();
    assert_eq!(
        text.matches("@BIN@").count(),
        1,
        "exactly one BIN token for `just install` to rewrite — a second (a \
         comment) would leak the substituted path into the installed file"
    );
    let cargo_toml = std::fs::read_to_string(resource("Cargo.toml")).unwrap();
    assert!(
        cargo_toml.contains("[[bin]]") && cargo_toml.contains("name = \"cad-studio\""),
        "the Exec line names the crate's binary"
    );
}

#[test]
fn the_three_rasters_decode_at_their_hicolor_sizes() {
    for (rel, size) in [
        ("resources/app-icon-128.png", 128u32),
        ("resources/app-icon.png", 256),
        ("resources/app-icon@2x.png", 512),
    ] {
        let bytes = std::fs::read(resource(rel)).unwrap();
        let img = image::load_from_memory(&bytes)
            .unwrap_or_else(|e| panic!("{rel}: not a decodable raster: {e}"));
        assert_eq!((img.width(), img.height()), (size, size), "{rel}");
    }
}

/// The element facts the metainfo test collects from the event stream.
#[derive(Default)]
struct Metainfo {
    component_type: Option<String>,
    release_version: Option<String>,
    release_date: Option<String>,
    /// Element name → trimmed text (the leaf texts the pins read —
    /// launchable's desktop-id is element text, not an attribute).
    texts: BTreeMap<String, String>,
}

impl Metainfo {
    fn note(&mut self, e: &BytesStart<'_>) {
        match e.local_name().as_ref() {
            b"component" => self.component_type = attr(e, b"type"),
            b"release" => {
                self.release_version = attr(e, b"version");
                self.release_date = attr(e, b"date");
            }
            _ => {}
        }
    }
}

/// The engine's threemf idiom (a.key.local_name → utf8).
fn attr(e: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == key)
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

#[test]
fn metainfo_parses_and_carries_the_app_identity() {
    let xml = std::fs::read_to_string(resource(
        "resources/packaging/net.brownhouse.CadStudio.metainfo.xml",
    ))
    .expect("the metainfo ships with the repo");
    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(true);
    let mut meta = Metainfo::default();
    // Open elements, outermost first. Text under <developer> is keyed
    // "developer/<name>" so it cannot shadow the component's own fields.
    let mut open: Vec<String> = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                open.push(String::from_utf8_lossy(e.local_name().as_ref()).into_owned());
                meta.note(&e);
            }
            Ok(Event::End(_)) => {
                open.pop();
            }
            // <release …/> arrives self-closing
            Ok(Event::Empty(e)) => meta.note(&e),
            Ok(Event::Text(t)) => {
                let text = t.xml10_content().expect("metainfo text").into_owned();
                let element = open.last().cloned().unwrap_or_default();
                let key = match open.len().checked_sub(2).map(|i| open[i].as_str()) {
                    Some("developer") => format!("developer/{element}"),
                    _ => element,
                };
                meta.texts.insert(key, text);
            }
            Ok(Event::Eof) => break,
            Err(e) => panic!("metainfo is not well-formed XML: {e}"),
            _ => {}
        }
    }
    assert_eq!(meta.component_type.as_deref(), Some("desktop-application"));
    assert_eq!(meta.texts.get("id").map(String::as_str), Some(APP_ICON));
    assert_eq!(
        meta.texts.get("name").map(String::as_str),
        Some("CAD Studio")
    );
    assert_eq!(
        meta.texts.get("developer/name").map(String::as_str),
        Some("Preston Brown"),
        "the metainfo names its developer"
    );
    assert!(
        meta.texts.get("summary").is_some_and(|s| !s.is_empty()),
        "appstream requires a summary"
    );
    assert!(
        meta.texts.get("p").is_some_and(|p| !p.is_empty()),
        "the description paragraph ships"
    );
    assert_eq!(
        meta.texts.get("launchable").map(String::as_str),
        Some(&*format!("{APP_ICON}.desktop")),
        "the launchable ties metainfo to the desktop entry"
    );
    assert_eq!(
        meta.release_version.as_deref(),
        Some("@CARGO_PKG_VERSION@"),
        "the release version is the placeholder `just install` rewrites"
    );
    assert_eq!(
        xml.matches("@CARGO_PKG_VERSION@").count(),
        1,
        "exactly one version token — a second (a comment) would leak into \
         the installed copy"
    );
    assert!(
        meta.release_date
            .as_deref()
            .is_some_and(|d| d.len() == 10 && d.as_bytes()[4] == b'-' && d.as_bytes()[7] == b'-'),
        "the release date is ISO yyyy-mm-dd: {:?}",
        meta.release_date
    );
    // one identity across both files: the desktop Name and metainfo name
    assert_eq!(
        keyfile(&desktop_file())["Desktop Entry"]["Name"],
        meta.texts["name"]
    );
}
