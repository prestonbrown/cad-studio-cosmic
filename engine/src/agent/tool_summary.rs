//! Tool-use summaries: chat-row titles for agent tool calls.
//!
//! Classification transcribed from the vendored
//! `fixtures/upstream-source/ToolSummary.swift`; title strings are the
//! `app/i18n/en/main.ftl` catalog values (each constant below names its key).
//! [`summarize`] and [`live_title`] classify root-free — no project root or
//! venv path at hand — so model and extension checks work on the basename and
//! reference material is recognized by a `references/` path segment.
//! [`rel_path`] and [`short_command`] carry the root/venv display
//! normalization for callers that know the project folder.

/// `installa-i-componenti`
const INSTALL_COMPONENTS: &str = "Installing components";
/// `compila-il-modello`
const BUILD_MODEL: &str = "Building the model";
/// `analizza-il-modello`
const ANALYZE_MODEL: &str = "Analyzing the model";
/// `prepara-un-anteprima`
const PREPARE_PREVIEW: &str = "Preparing a preview";
/// `esamina-i-file`
const EXAMINE_FILES: &str = "Looking through files";
/// `modifica-il-modello`
const EDIT_MODEL: &str = "Editing the model";
/// `sistema-i-file`
const TIDY_FILES: &str = "Tidying up files";
/// `lavora-sul-progetto`
const WORK_PROJECT: &str = "Working on the project";
/// `scrive-un-file`
const WRITE_FILE: &str = "Writing a file";
/// `scrive-il-modello`
const WRITE_MODEL: &str = "Writing the model";
/// `prepara-un-file-di-supporto`
const WRITE_HELPER: &str = "Preparing a helper file";
/// `modifica-un-file-di-supporto`
const EDIT_HELPER: &str = "Editing a helper file";
/// `legge-un-file`
const READ_FILE: &str = "Reading a file";
/// `studia-il-riferimento`
const STUDY_REFERENCE: &str = "Studying the reference";
/// `rilegge-il-modello`
const REREAD_MODEL: &str = "Reviewing the model";
/// `controlla-il-disegno`
const CHECK_DRAWING: &str = "Checking the drawing";
/// `controlla-i-risultati`
const CHECK_RESULTS: &str = "Checking the results";
/// `controlla-un-anteprima`
const CHECK_PREVIEW: &str = "Checking a preview";
/// `cerca-nel-progetto`
const SEARCH_PROJECT: &str = "Searching the project";
/// `aggiorna-il-piano-di-lavoro`
const UPDATE_PLAN: &str = "Updates the task plan";
/// `cerca-sul-web`
const SEARCH_WEB: &str = "Searching the web";

/// Title for a completed tool use, upstream `ToolSummary.title` minus the
/// root/python display context.
pub fn summarize(name: &str, input: &serde_json::Value) -> String {
    match name {
        "Bash" => command_title(&short_command(input_str(input, "command"), "", "")),
        "Write" => {
            let path = file_path(input);
            if path.is_empty() {
                WRITE_FILE.to_string()
            } else if is_model(path) {
                WRITE_MODEL.to_string()
            } else {
                WRITE_HELPER.to_string()
            }
        }
        "Edit" | "MultiEdit" => {
            if is_model(file_path(input)) {
                EDIT_MODEL.to_string()
            } else {
                EDIT_HELPER.to_string()
            }
        }
        "Read" => read_title(file_path(input)),
        "Glob" | "Grep" => SEARCH_PROJECT.to_string(),
        "TodoWrite" => UPDATE_PLAN.to_string(),
        "WebSearch" | "WebFetch" => SEARCH_WEB.to_string(),
        _ => WORK_PROJECT.to_string(),
    }
}

/// Title for a tool use whose input JSON is still streaming in. Garbage or
/// truncated prefixes never error: keys are string-scanned, and whatever is
/// missing simply does not classify.
///
/// While the prefix ends inside an unterminated JSON string (odd count of
/// unescaped quotes) the input is still arriving, so the title gets a
/// trailing `…`; a Write with parseable content additionally reports the
/// line count so far (`lld-righe` format).
pub fn live_title(name: &str, partial_json: &str) -> String {
    let mut fields = serde_json::Map::new();
    for key in ["file_path", "pattern"] {
        if let Some(value) = partial_string(key, partial_json) {
            // upstream re-checks that the scanned value really sits after the
            // key as its value before trusting it
            let tight = format!("\"{key}\":\"{value}\"");
            let spaced = format!("\"{key}\": \"{value}\"");
            if partial_json.contains(&tight) || partial_json.contains(&spaced) {
                fields.insert(key.to_string(), serde_json::Value::String(value));
            }
        }
    }
    if let Some(command) = partial_string("command", partial_json) {
        fields.insert("command".to_string(), serde_json::Value::String(command));
    }
    let mut title = summarize(name, &serde_json::Value::Object(fields));
    if name == "Write" {
        if let Some(content) = partial_string("content", partial_json) {
            title = format!("{title} · {} lines", content.split('\n').count());
        }
    }
    if ends_inside_string(partial_json) {
        title.push('…');
    }
    title
}

/// Last four non-empty lines of the `content` (Write) or `new_string` (Edit)
/// value scanned out of a streaming input prefix; `None` when neither key has
/// a value. A found-but-empty value yields `Some("")`, as upstream does.
pub fn live_preview(partial_json: &str) -> Option<String> {
    let content = partial_string("content", partial_json)
        .or_else(|| partial_string("new_string", partial_json))?;
    let mut lines: Vec<&str> = content
        .split('\n')
        .filter(|l| !l.trim().is_empty())
        .collect();
    let kept = lines.split_off(lines.len().saturating_sub(4));
    Some(kept.join("\n"))
}

/// `path` relative to `root` when it lives under it, otherwise as given.
pub fn rel_path(path: &str, root: &str) -> String {
    path.strip_prefix(&format!("{root}/"))
        .unwrap_or(path)
        .to_string()
}

/// Display form of a Bash command, upstream `shortCommand`: the venv
/// interpreter becomes `python` (its quoted form first, so paths with spaces
/// collapse whole), only the first line counts, and a leading
/// `cd <into root> && …` (or `; …`) hop is stripped. With an empty `root`
/// any cd target is accepted — [`summarize`] uses that to classify
/// root-free.
pub fn short_command(command: &str, root: &str, python: &str) -> String {
    let mut command = command.to_string();
    if !python.is_empty() {
        if python.contains(' ') {
            command = command.replace(&format!("\"{python}\""), "python");
        }
        command = command.replace(python, "python");
    }
    let mut command = command.split('\n').next().unwrap_or("").to_string();
    if command.starts_with("cd ") && command.contains(root) {
        let separator = command
            .find("&&")
            .map(|i| i + 2)
            .or_else(|| command.find(';').map(|i| i + 1));
        if let Some(end) = separator {
            command = command[end..].trim().to_string();
        }
    }
    command
}

/// Upstream `commandTitle`: classes keyed off the first word (by basename).
fn command_title(command: &str) -> String {
    let text = command.trim();
    if text.is_empty() {
        return WORK_PROJECT.to_string();
    }
    let words: Vec<&str> = text.split([' ', '\t']).filter(|w| !w.is_empty()).collect();
    let tool = words.first().map(|w| base_name(w)).unwrap_or("");
    if text.contains("pip install") || tool == "uv" {
        return INSTALL_COMPONENTS.to_string();
    }
    if tool.starts_with("python") {
        return if words.get(1) == Some(&"model.py") {
            BUILD_MODEL.to_string()
        } else {
            ANALYZE_MODEL.to_string()
        };
    }
    if matches!(
        tool,
        "rsvg-convert" | "sips" | "magick" | "convert" | "qlmanage"
    ) {
        return PREPARE_PREVIEW.to_string();
    }
    // read-only sed is checked here, BEFORE the model.py-edit case below
    if matches!(
        tool,
        "ls" | "cat" | "head" | "tail" | "grep" | "rg" | "find" | "wc" | "file" | "stat"
    ) || (tool == "sed" && !text.contains(" -i"))
    {
        return EXAMINE_FILES.to_string();
    }
    if matches!(tool, "sed" | "perl") && text.contains("model.py") {
        return EDIT_MODEL.to_string();
    }
    if matches!(
        tool,
        "sed" | "mv" | "cp" | "rm" | "mkdir" | "touch" | "perl"
    ) {
        return TIDY_FILES.to_string();
    }
    WORK_PROJECT.to_string()
}

/// Upstream `readTitle`, same check order: empty, references/, model.py,
/// schematic*/pdf, manifest.json, image extensions, default.
fn read_title(path: &str) -> String {
    if path.is_empty() {
        return READ_FILE.to_string();
    }
    if path.starts_with("references/") || path.contains("/references/") {
        return STUDY_REFERENCE.to_string();
    }
    if is_model(path) {
        return REREAD_MODEL.to_string();
    }
    let name = base_name(path).to_lowercase();
    let ext = file_ext(&name);
    if name.starts_with("schematic") || ext == "pdf" {
        return CHECK_DRAWING.to_string();
    }
    if name == "manifest.json" {
        return CHECK_RESULTS.to_string();
    }
    if matches!(ext, "png" | "jpg" | "jpeg" | "svg" | "webp") {
        return CHECK_PREVIEW.to_string();
    }
    READ_FILE.to_string()
}

/// The file a tool input points at: `file_path`, falling back to `path` —
/// a present-but-non-string `file_path` falls back too, like upstream's
/// nil-coalescing read.
fn file_path(input: &serde_json::Value) -> &str {
    match input.get("file_path").and_then(serde_json::Value::as_str) {
        Some(path) => path,
        None => input
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(""),
    }
}

fn input_str<'a>(input: &'a serde_json::Value, key: &str) -> &'a str {
    input
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

/// Last path component — upstream uses NSString.lastPathComponent, which
/// also ignores a trailing slash.
fn base_name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// Extension of an already-lowercased basename, "" when there is none
/// (matching NSString.pathExtension: no extension for ".png"-style names).
fn file_ext(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => &name[i + 1..],
        _ => "",
    }
}

fn is_model(path: &str) -> bool {
    base_name(path) == "model.py"
}

/// Scans `"<key>"` in a partial-JSON prefix and hand-decodes the string value
/// that follows (`\n`/`\t` escapes respected), tolerating a missing closing
/// quote — upstream `partialString`. Returns `None` when the key is absent or
/// not followed by a string value.
fn partial_string(key: &str, json: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let after_key = json.find(&needle)? + needle.len();
    let rest = json[after_key..].trim_start_matches([' ', ':']);
    let mut value = String::new();
    let mut escaped = false;
    for c in rest.strip_prefix('"')?.chars() {
        if escaped {
            match c {
                'n' => value.push('\n'),
                't' => value.push('\t'),
                _ => value.push(c),
            }
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            break;
        } else {
            value.push(c);
        }
    }
    Some(value)
}

/// True when the prefix ends inside a string literal: an odd number of
/// unescaped double quotes means more input is on the way.
fn ends_inside_string(json: &str) -> bool {
    let mut escaped = false;
    let mut quotes = 0u32;
    for c in json.chars() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            quotes += 1;
        }
    }
    quotes % 2 == 1
}
