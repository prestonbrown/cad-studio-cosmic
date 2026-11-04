//! Every message-routing function matches its message enum exhaustively: no
//! wildcard arm (`_ =>`, `_ if …`) that would let a new variant fall through
//! unhandled. Adding a variant must then fail to compile in each router until
//! someone decides where it goes.
//!
//! A binding arm that forwards the rest to a child (`msg => child.update(msg)`)
//! is allowed: that is a typed forward, not a swallow.

use std::path::PathBuf;

/// (source file under app/src, the router's signature prefix).
const ROUTERS: &[(&str, &str)] = &[
    ("app.rs", "fn update(&mut self, message: Message)"),
    ("app.rs", "fn update_welcome("),
    ("app.rs", "fn update_new_project_sheet("),
    ("app.rs", "fn update_project("),
    ("app.rs", "fn app_request("),
    ("app.rs", "fn app_command("),
    ("app.rs", "fn update_settings_any("),
    (
        "windows/project.rs",
        "pub fn update(&mut self, message: Message)",
    ),
    ("windows/project.rs", "fn update_viewer("),
    ("windows/project.rs", "fn update_chat("),
    ("windows/project.rs", "fn update_bridge("),
    ("windows/project.rs", "fn update_command("),
    (
        "windows/settings.rs",
        "pub fn update(&mut self, message: &Message)",
    ),
    ("windows/settings.rs", "pub fn stage("),
    ("windows/welcome.rs", "pub fn update_sheet("),
    ("windows/welcome.rs", "pub fn update(sheet: &mut NewProject"),
    ("chat_view.rs", "pub fn update(&mut self, message: Message)"),
    ("schematic.rs", "pub fn update(&mut self, message: Message)"),
    (
        "viewer_pane.rs",
        "pub fn update(&mut self, message: Message)",
    ),
];

/// The source with `//` comments blanked (string literals kept intact, so a
/// `//` inside a string does not cut the line).
fn strip_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            let mut in_string = false;
            let mut escaped = false;
            let bytes = line.as_bytes();
            for i in 0..bytes.len() {
                let b = bytes[i];
                if in_string {
                    match (escaped, b) {
                        (true, _) => escaped = false,
                        (false, b'\\') => escaped = true,
                        (false, b'"') => in_string = false,
                        _ => {}
                    }
                } else if b == b'"' {
                    in_string = true;
                } else if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
                    return &line[..i];
                }
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of the first fn whose line contains `signature`, braces
/// balanced (strings skipped).
fn fn_body<'a>(source: &'a str, signature: &str) -> Option<&'a str> {
    let start = source.find(signature)?;
    let open = start + source[start..].find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in source[open..].char_indices() {
        if in_string {
            match (escaped, ch) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&source[open..=open + offset]);
                }
            }
            _ => {}
        }
    }
    None
}

/// The lines of `body` that open a wildcard match arm.
fn wildcard_arms(body: &str) -> Vec<String> {
    body.lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with("_ =>") || line.starts_with("_ if "))
        .map(str::to_string)
        .collect()
}

#[test]
fn message_routers_have_no_wildcard_arms() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut failures = Vec::new();
    for (file, signature) in ROUTERS {
        let raw = std::fs::read_to_string(src.join(file)).expect("router source readable");
        let source = strip_comments(&raw);
        let Some(body) = fn_body(&source, signature) else {
            failures.push(format!("{file}: `{signature}` not found"));
            continue;
        };
        for arm in wildcard_arms(body) {
            failures.push(format!("{file} `{signature}`: wildcard arm `{arm}`"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_scanner_sees_a_wildcard_arm() {
    let source = strip_comments(
        "fn update(&mut self, message: Message) {\n    match message {\n        \
         Message::A => {} // a \"{\" in a comment\n        _ => {}\n    }\n}\n",
    );
    let body = fn_body(&source, "fn update(").expect("found");
    assert_eq!(wildcard_arms(body), vec!["_ => {}".to_string()]);
    let forward = "fn update() {\n    match m {\n        msg => child(msg),\n    }\n}";
    assert!(wildcard_arms(fn_body(forward, "fn update(").unwrap()).is_empty());
}
