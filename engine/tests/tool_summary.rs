use engine::agent::tool_summary::{live_preview, live_title, rel_path, short_command, summarize};
use serde_json::json;

#[test]
fn bash_first_word_classes_in_upstream_order() {
    let cases = [
        ("pip install build123d", "Installing components"),
        // "pip install" anywhere in the text, first word irrelevant
        ("bash -c 'pip install build123d'", "Installing components"),
        ("uv sync", "Installing components"),
        ("python model.py", "Building the model"),
        ("python3.11 model.py --json", "Building the model"),
        // tool is the basename of the first word
        ("/root/.venv/bin/python model.py", "Building the model"),
        // a leading cd-hop is normalized away before classification
        ("cd /root && python model.py", "Building the model"),
        ("python analyze.py", "Analyzing the model"),
        ("python -m build123d", "Analyzing the model"),
        ("rsvg-convert -o out.png schematic.svg", "Preparing a preview"),
        ("/usr/bin/qlmanage -t -s 512 -o . render.png", "Preparing a preview"),
        ("magick convert x.heic x.png", "Preparing a preview"),
        ("ls -la output/", "Looking through files"),
        ("/bin/cat output/manifest.json", "Looking through files"),
        // read-only sed outranks the model.py-edit case (ToolSummary.swift:53)
        ("sed -n 1,5p model.py", "Looking through files"),
        ("sed s/a/b/ model.py", "Looking through files"),
        ("sed -i s/a/b/ model.py", "Editing the model"),
        ("perl -pi -e s/a/b/ model.py", "Editing the model"),
        // in-place but not the model: tidy, not edit
        ("sed -i s/x/y/ helper.txt", "Tidying up files"),
        ("mv out.tmp out.json", "Tidying up files"),
        ("rm -rf scratch/", "Tidying up files"),
        ("mkdir -p output", "Tidying up files"),
        ("touch notes.txt", "Tidying up files"),
        ("echo hello", "Working on the project"),
        ("cargo build --release", "Working on the project"),
        ("", "Working on the project"),
    ];
    for (command, expected) in cases {
        assert_eq!(
            summarize("Bash", &json!({"command": command})),
            expected,
            "command: {command:?}"
        );
    }
}

#[test]
fn write_titles_key_off_the_basename() {
    assert_eq!(
        summarize("Write", &json!({"file_path": "/w/model.py", "content": "x = 1"})),
        "Writing the model"
    );
    // basename match, not suffix: nested model.py counts
    assert_eq!(
        summarize("Write", &json!({"file_path": "/w/scratch/model.py"})),
        "Writing the model"
    );
    // the path-key fallback upstream's file() reads second
    assert_eq!(summarize("Write", &json!({"path": "/w/model.py"})), "Writing the model");
    assert_eq!(
        summarize("Write", &json!({"file_path": "/w/notes.txt", "content": "x"})),
        "Preparing a helper file"
    );
    // no path at all
    assert_eq!(summarize("Write", &json!({})), "Writing a file");
}

#[test]
fn edit_and_multiedit_share_the_model_case() {
    assert_eq!(summarize("Edit", &json!({"file_path": "/w/model.py"})), "Editing the model");
    assert_eq!(
        summarize("MultiEdit", &json!({"file_path": "/w/model.py", "edits": []})),
        "Editing the model"
    );
    assert_eq!(
        summarize("Edit", &json!({"file_path": "/w/output/style.css"})),
        "Editing a helper file"
    );
    assert_eq!(
        summarize("MultiEdit", &json!({"file_path": "/w/output/style.css"})),
        "Editing a helper file"
    );
}

#[test]
fn read_titles_follow_the_upstream_order() {
    // empty first
    assert_eq!(summarize("Read", &json!({})), "Reading a file");
    // references/ outranks model.py (ToolSummary.swift:35-36 checks the prefix first)
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/references/model.py"})),
        "Studying the reference"
    );
    assert_eq!(
        summarize("Read", &json!({"file_path": "references/panel.png"})),
        "Studying the reference"
    );
    assert_eq!(summarize("Read", &json!({"file_path": "/w/model.py"})), "Reviewing the model");
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/schematic-2.pdf"})),
        "Checking the drawing"
    );
    // schematic* prefix and pdf extension both hit, case-insensitively
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/output/SchematicB.PDF"})),
        "Checking the drawing"
    );
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/output/Manifest.JSON"})),
        "Checking the results"
    );
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/output/render.png"})),
        "Checking a preview"
    );
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/output/render.jpeg"})),
        "Checking a preview"
    );
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/output/render.svg"})),
        "Checking a preview"
    );
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/output/render.webp"})),
        "Checking a preview"
    );
    assert_eq!(summarize("Read", &json!({"file_path": "/w/README.md"})), "Reading a file");
}

#[test]
fn fixed_tool_names_need_no_input() {
    assert_eq!(summarize("Glob", &json!({"pattern": "*.py"})), "Searching the project");
    assert_eq!(summarize("Grep", &json!({"pattern": "build123d"})), "Searching the project");
    assert_eq!(summarize("TodoWrite", &json!({"todos": []})), "Updates the task plan");
    assert_eq!(summarize("WebSearch", &json!({"query": "build123d docs"})), "Searching the web");
    assert_eq!(
        summarize("WebFetch", &json!({"url": "https://example.com"})),
        "Searching the web"
    );
    // unknown tools fall through to the generic title
    assert_eq!(summarize("NotebookEdit", &json!({})), "Working on the project");
}

#[test]
fn live_title_marks_streaming_partials() {
    // file_path classifies, the content key is still truncated: title plus ellipsis
    assert_eq!(
        live_title("Write", r#"{"file_path": "/tmp/p/model.py", "conte"#),
        "Writing the model…"
    );
    // complete content value: line count, no ellipsis
    assert_eq!(
        live_title("Write", r#"{"file_path": "/tmp/p/model.py", "content": "a\nb\nc"}"#),
        "Writing the model · 3 lines"
    );
    // content still streaming: lines so far plus the ellipsis
    assert_eq!(
        live_title("Write", r#"{"file_path": "/tmp/p/model.py", "content": "a\nb"#),
        "Writing the model · 2 lines…"
    );
    // bare prefix and garbage never panic: the tool's generic title comes back
    assert_eq!(live_title("Write", "{"), "Writing a file");
    assert_eq!(live_title("Bash", "not json"), "Working on the project");
    // the line suffix is Write-only; Edit keeps its plain title
    assert_eq!(
        live_title("Edit", r#"{"file_path": "/tmp/p/model.py", "new_string": "x = 1"#),
        "Editing the model…"
    );
    // command partials classify too
    assert_eq!(live_title("Bash", r#"{"command": "python model.py"#), "Building the model…");
}

#[test]
fn live_title_ignores_keys_quoted_inside_values() {
    // "file_path" appears inside the command string, not as a key: it must not
    // leak into the classification, and the command still parses around it
    // (the partial is complete JSON — escaped quotes don't count toward the
    // streaming marker — so no ellipsis)
    let partial = r#"{"command": "grep \"file_path\":\"x\" model.py"}"#;
    assert_eq!(live_title("Bash", partial), "Looking through files");
    // Read never reads the command key, so nothing is left to classify
    assert_eq!(live_title("Read", partial), "Reading a file");
}

#[test]
fn live_preview_shows_last_four_nonempty_lines() {
    let write = r#"{"file_path": "/tmp/p/model.py", "content": "1\n2\n\n3\n4\n5\n6"#;
    assert_eq!(live_preview(write).as_deref(), Some("3\n4\n5\n6"));
    // blank and whitespace-only lines drop out; fewer than four survive whole
    let edit = r#"{"file_path": "/tmp/p/model.py", "new_string": "a\n\nb\n \nc"#;
    assert_eq!(live_preview(edit).as_deref(), Some("a\nb\nc"));
    // no content/new_string key: no preview
    assert_eq!(live_preview(r#"{"command": "python model.py"#), None);
    // key present with an empty value: empty preview, not None (upstream returns "")
    assert_eq!(live_preview(r#"{"content": ""}"#).as_deref(), Some(""));
    // content wins when both keys are present (Write carries content, Edit new_string)
    assert_eq!(live_preview(r#"{"content": "c", "new_string": "n"}"#).as_deref(), Some("c"));
}

#[test]
fn rel_path_strips_only_the_root_prefix() {
    assert_eq!(rel_path("/w/model.py", "/w"), "model.py");
    assert_eq!(rel_path("/w/output/model.py", "/w"), "output/model.py");
    // outside the root: as given
    assert_eq!(rel_path("/elsewhere/model.py", "/w"), "/elsewhere/model.py");
    // already relative: as given
    assert_eq!(rel_path("model.py", "/w"), "model.py");
}

#[test]
fn short_command_normalizes_venv_and_cd() {
    let py = "/root/.venv/bin/python";
    assert_eq!(short_command("cd /root && python model.py", "/root", py), "python model.py");
    assert_eq!(short_command("/root/.venv/bin/python model.py", "/root", py), "python model.py");
    // replacement happens before the cd strip, like upstream
    assert_eq!(
        short_command("cd /root && /root/.venv/bin/python model.py", "/root", py),
        "python model.py"
    );
    // quoted venv paths (spaces) are replaced first, then the bare path
    assert_eq!(
        short_command("\"/opt/my venv/python\" model.py", "/w", "/opt/my venv/python"),
        "python model.py"
    );
    // only the first line counts
    assert_eq!(
        short_command("/root/.venv/bin/python model.py\nls -la", "/root", py),
        "python model.py"
    );
    // ';' separates when no "&&" is present
    assert_eq!(short_command("cd /root; python x.py", "/root", py), "python x.py");
    // the cd target must contain the root, otherwise nothing is stripped
    assert_eq!(
        short_command("cd /elsewhere && python x.py", "/root", py),
        "cd /elsewhere && python x.py"
    );
    assert_eq!(short_command("python model.py", "/root", py), "python model.py");
}
