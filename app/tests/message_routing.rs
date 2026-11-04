//! Every message-routing function matches its message enum exhaustively: no
//! wildcard arm (`_ =>`, `_msg =>`, `| _ =>`, `_ if …`, anywhere on a line)
//! that would let a new variant fall through unhandled. Adding a variant must
//! then fail to compile in each router until someone decides where it goes.
//!
//! A binding arm that forwards the rest to a child (`msg => child.update(msg)`)
//! is allowed: that is a typed forward. A binding arm whose body never uses
//! the binding (`other => {}`) is a swallow, and so is a no-op arm over a
//! variant that carries a child message enum (`Message::Viewer(_) => {}`).

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
    ("app.rs", "fn update_agent_sheet("),
    ("app.rs", "fn update_setup_sheet("),
    ("app.rs", "fn update_python_sheet("),
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
    ("windows/settings.rs", "pub fn persists("),
    ("windows/welcome.rs", "pub fn update_sheet("),
    ("windows/welcome.rs", "pub fn update(sheet: &mut NewProject"),
    ("chat_view.rs", "pub fn update(&mut self, message: Message)"),
    ("schematic.rs", "pub fn update(&mut self, message: Message)"),
    (
        "viewer_pane.rs",
        "pub fn update(&mut self, message: Message)",
    ),
];

/// The source with every comment (`//`, nested `/* */`) removed and the
/// contents of every string, raw string and char literal blanked to spaces,
/// so braces and arrows inside them never count. Newlines are kept, so line
/// structure survives.
fn sanitize(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0usize;
            while i < chars.len() {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(chars[i]));
                    i += 1;
                }
            }
        } else if c == 'r'
            && matches!(next, Some('"' | '#'))
            && !chars
                .get(i.wrapping_sub(1))
                .is_some_and(|p| p.is_alphanumeric() || *p == '_')
        {
            // r"…" / r#"…"#
            let mut hashes = 0;
            let mut j = i + 1;
            while chars.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if chars.get(j) != Some(&'"') {
                out.push(c);
                i += 1;
                continue;
            }
            out.push_str("r\"");
            j += 1;
            while j < chars.len() {
                if chars[j] == '"' && (0..hashes).all(|k| chars.get(j + 1 + k) == Some(&'#')) {
                    j += 1 + hashes;
                    break;
                }
                out.push(blank(chars[j]));
                j += 1;
            }
            out.push('"');
            i = j;
        } else if c == '"' {
            out.push('"');
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' {
                    out.push(' ');
                    i += 1;
                }
                if let Some(&ch) = chars.get(i) {
                    out.push(blank(ch));
                }
                i += 1;
            }
            out.push('"');
            i += 1;
        } else if c == '\''
            && (chars.get(i + 2) == Some(&'\'')
                || (next == Some('\\') && chars[i + 2..].iter().take(10).any(|&ch| ch == '\'')))
        {
            // a char literal ('x', '\n', '\u{..}'); a lifetime has no closing quote
            let mut j = i + 1;
            if chars[j] == '\\' {
                j += 1;
            }
            j += 1;
            while j < chars.len() && chars[j] != '\'' {
                j += 1;
            }
            out.push_str("' '");
            i = j + 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// The body of the first fn whose line contains `signature`, braces
/// balanced (in sanitized source).
fn fn_body<'a>(source: &'a str, signature: &str) -> Option<&'a str> {
    let start = source.find(signature)?;
    let open = start + source[start..].find('{')?;
    let mut depth = 0usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
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

/// One match arm: its pattern (with any guard) and its body text.
struct Arm {
    pattern: String,
    body: String,
}

/// Every `pattern => body` arm in `body`: the pattern runs back from `=>`
/// to the line start or the previous arm's `,`; the body runs to the
/// next `,` at the arm's own brace depth, or to the end of its block.
fn arms(text: &str) -> Vec<Arm> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut search = 0;
    while let Some(at) = text[search..].find("=>") {
        let arrow = search + at;
        let line_start = text[..arrow].rfind('\n').map_or(0, |n| n + 1);
        let mut pattern_start = line_start;
        let mut depth = 0i32;
        for (k, ch) in text[line_start..arrow].char_indices().rev() {
            match ch {
                ')' | ']' | '}' => depth += 1,
                '(' | '[' | '{' => depth -= 1,
                ',' if depth == 0 => {
                    pattern_start = line_start + k + 1;
                    break;
                }
                _ => {}
            }
        }
        let mut depth = 0i32;
        let mut end = text.len();
        for (k, &b) in bytes[arrow + 2..].iter().enumerate() {
            match b {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    if depth == 0 {
                        end = arrow + 2 + k;
                        break;
                    }
                    depth -= 1;
                    if depth == 0 && b == b'}' {
                        end = arrow + 3 + k;
                        let rest = text[end..].trim_start();
                        if !rest.starts_with('.') && !rest.starts_with('?') {
                            break;
                        }
                    }
                }
                b',' if depth == 0 => {
                    end = arrow + 2 + k;
                    break;
                }
                _ => {}
            }
        }
        found.push(Arm {
            pattern: text[pattern_start..arrow].trim().to_string(),
            body: text[arrow + 2..end].trim().to_string(),
        });
        search = arrow + 2;
    }
    found
}

/// A pattern's leading binding name when the whole pattern (before any
/// guard) is a lone lowercase or underscore-led identifier.
fn binding(pattern: &str) -> Option<&str> {
    let pattern = pattern.trim_start_matches('|').trim();
    let name = pattern.split(" if ").next()?.trim();
    let first = name.chars().next()?;
    ((first.is_lowercase() || first == '_')
        && name.chars().all(|c| c.is_alphanumeric() || c == '_'))
    .then_some(name)
}

/// Whether `body` does nothing with what it matched.
fn is_noop(body: &str) -> bool {
    let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    matches!(
        compact.as_str(),
        "{}" | "()" | "Task::none()" | "{Task::none()}" | "CosmicTask::none()" | "false" | "true"
    )
}

/// Whether `variant` (its last path segment) is declared in `source` as
/// carrying one child message enum: `Variant(…Message)`.
fn carries_message(source: &str, variant: &str) -> bool {
    let needle = format!("{variant}(");
    source.lines().map(str::trim).any(|line| {
        line.strip_prefix(&needle)
            .and_then(|rest| rest.strip_suffix("),").or_else(|| rest.strip_suffix(')')))
            .is_some_and(|ty| !ty.contains(',') && ty.ends_with("Message"))
    })
}

/// The arms of `body` that let a message fall through unhandled.
fn swallowing_arms(body: &str, file_source: &str) -> Vec<String> {
    arms(body)
        .into_iter()
        .filter(|arm| {
            if let Some(name) = binding(&arm.pattern) {
                return name.starts_with('_') || !mentions(&arm.body, name);
            }
            let alternatives = arm.pattern.split('|').map(str::trim);
            for alternative in alternatives {
                if let Some(head) = alternative.strip_suffix("(_)") {
                    let variant = head.rsplit("::").next().unwrap_or(head);
                    if is_noop(&arm.body) && carries_message(file_source, variant) {
                        return true;
                    }
                }
            }
            false
        })
        .map(|arm| format!("{} => {}", arm.pattern, arm.body))
        .collect()
}

/// Whether `text` uses the identifier `name` (whole-word).
fn mentions(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + name.len()..].chars().next();
        let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        !word(before) && !word(after)
    })
}

#[test]
fn message_routers_have_no_wildcard_arms() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut failures = Vec::new();
    for (file, signature) in ROUTERS {
        let raw = std::fs::read_to_string(src.join(file)).expect("router source readable");
        let source = sanitize(&raw);
        let Some(body) = fn_body(&source, signature) else {
            failures.push(format!("{file}: `{signature}` not found"));
            continue;
        };
        // a router that matches must yield arms, or the scan proves nothing
        if body.contains("match ") && arms(body).is_empty() {
            failures.push(format!("{file} `{signature}`: no match arms parsed"));
        }
        for arm in swallowing_arms(body, &source) {
            failures.push(format!("{file} `{signature}`: swallowing arm `{arm}`"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_scanner_sees_every_swallow_shape() {
    let enums = "enum Message {\n    Viewer(viewer::Message),\n    Tab(Tab),\n}\n";
    let check = |router: &str| {
        let source = sanitize(&format!("{enums}{router}"));
        let body = fn_body(&source, "fn update(").expect("found");
        swallowing_arms(body, &source)
    };
    let wrap = |arms: &str| {
        format!(
            "fn update(&mut self, message: Message) {{\n    match message {{\n{arms}\n    }}\n}}\n"
        )
    };

    assert_eq!(
        check(&wrap(
            "        Message::A => {} // a \"{\" in a comment\n        _ => {}"
        ))
        .len(),
        1
    );
    assert_eq!(
        check(&wrap("        Message::A => {}\n        _msg => {}")).len(),
        1
    );
    assert_eq!(
        check(&wrap("        Message::A => {}\n        | _ => {}")).len(),
        1
    );
    assert_eq!(
        check(&wrap("        Message::A => x, _ => y,")).len(),
        1,
        "two arms on a line"
    );
    assert_eq!(
        check(&wrap("        other => Task::none(),")).len(),
        1,
        "binding dropped"
    );
    assert_eq!(
        check(&wrap("        Message::Viewer(_) => {}")).len(),
        1,
        "nested catch-all"
    );
    assert_eq!(
        check(&wrap("        _ if ready => go(),")).len(),
        1,
        "guarded wildcard"
    );

    assert!(
        check(&wrap("        msg => self.child.update(msg),")).is_empty(),
        "typed forward"
    );
    assert!(
        check(&wrap("        Message::Tab(_) => {}")).is_empty(),
        "a data payload"
    );
    assert!(
        check(&wrap(
            "        Message::A => { let c = '{'; let s = r#\"_ => {\"#; /* _ => {} */ }"
        ))
        .is_empty(),
        "literals and block comments are not code"
    );
    assert!(
        check(&wrap(
            "        Message::A => {\n            go();\n        }\n        Message::B => stop(),"
        ))
        .is_empty(),
        "block arms end at their brace"
    );
}
