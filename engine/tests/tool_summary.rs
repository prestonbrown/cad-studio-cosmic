use engine::agent::tool_summary::{
    ToolAction, ToolTitle, live_preview, live_title, rel_path, short_command, summarize,
};
use serde_json::json;

#[test]
fn bash_first_word_classes_in_upstream_order() {
    let cases = [
        ("pip install build123d", ToolAction::InstallComponents),
        // "pip install" anywhere in the text, first word irrelevant
        (
            "bash -c 'pip install build123d'",
            ToolAction::InstallComponents,
        ),
        ("uv sync", ToolAction::InstallComponents),
        ("python model.py", ToolAction::BuildModel),
        ("python3.11 model.py --json", ToolAction::BuildModel),
        // tool is the basename of the first word
        ("/root/.venv/bin/python model.py", ToolAction::BuildModel),
        // a leading cd-hop is normalized away before classification
        ("cd /root && python model.py", ToolAction::BuildModel),
        ("python analyze.py", ToolAction::AnalyzeModel),
        ("python -m build123d", ToolAction::AnalyzeModel),
        (
            "rsvg-convert -o out.png schematic.svg",
            ToolAction::PreparePreview,
        ),
        (
            "/usr/bin/qlmanage -t -s 512 -o . render.png",
            ToolAction::PreparePreview,
        ),
        ("magick convert x.heic x.png", ToolAction::PreparePreview),
        ("ls -la output/", ToolAction::ExamineFiles),
        ("/bin/cat output/manifest.json", ToolAction::ExamineFiles),
        // read-only sed outranks the model.py-edit case (ToolSummary.swift:53)
        ("sed -n 1,5p model.py", ToolAction::ExamineFiles),
        ("sed s/a/b/ model.py", ToolAction::ExamineFiles),
        ("sed -i s/a/b/ model.py", ToolAction::EditModel),
        ("perl -pi -e s/a/b/ model.py", ToolAction::EditModel),
        // in-place but not the model: tidy, not edit
        ("sed -i s/x/y/ helper.txt", ToolAction::TidyFiles),
        ("mv out.tmp out.json", ToolAction::TidyFiles),
        ("rm -rf scratch/", ToolAction::TidyFiles),
        ("mkdir -p output", ToolAction::TidyFiles),
        ("touch notes.txt", ToolAction::TidyFiles),
        ("echo hello", ToolAction::WorkProject),
        ("cargo build --release", ToolAction::WorkProject),
        ("", ToolAction::WorkProject),
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
        summarize(
            "Write",
            &json!({"file_path": "/w/model.py", "content": "x = 1"})
        ),
        ToolAction::WriteModel
    );
    // basename match, not suffix: nested model.py counts
    assert_eq!(
        summarize("Write", &json!({"file_path": "/w/scratch/model.py"})),
        ToolAction::WriteModel
    );
    // the path-key fallback upstream's file() reads second
    assert_eq!(
        summarize("Write", &json!({"path": "/w/model.py"})),
        ToolAction::WriteModel
    );
    assert_eq!(
        summarize(
            "Write",
            &json!({"file_path": "/w/notes.txt", "content": "x"})
        ),
        ToolAction::WriteHelper
    );
    // no path at all
    assert_eq!(summarize("Write", &json!({})), ToolAction::WriteFile);
}

#[test]
fn edit_and_multiedit_share_the_model_case() {
    assert_eq!(
        summarize("Edit", &json!({"file_path": "/w/model.py"})),
        ToolAction::EditModel
    );
    assert_eq!(
        summarize(
            "MultiEdit",
            &json!({"file_path": "/w/model.py", "edits": []})
        ),
        ToolAction::EditModel
    );
    assert_eq!(
        summarize("Edit", &json!({"file_path": "/w/output/style.css"})),
        ToolAction::EditHelper
    );
    assert_eq!(
        summarize("MultiEdit", &json!({"file_path": "/w/output/style.css"})),
        ToolAction::EditHelper
    );
}

#[test]
fn read_titles_follow_the_upstream_order() {
    // empty first
    assert_eq!(summarize("Read", &json!({})), ToolAction::ReadFile);
    // references/ outranks model.py (ToolSummary.swift:35-36 checks the prefix first)
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/references/model.py"})),
        ToolAction::StudyReference
    );
    assert_eq!(
        summarize("Read", &json!({"file_path": "references/panel.png"})),
        ToolAction::StudyReference
    );
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/model.py"})),
        ToolAction::RereadModel
    );
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/schematic-2.pdf"})),
        ToolAction::CheckDrawing
    );
    // schematic* prefix and pdf extension both hit, case-insensitively
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/output/SchematicB.PDF"})),
        ToolAction::CheckDrawing
    );
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/output/Manifest.JSON"})),
        ToolAction::CheckResults
    );
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/output/render.png"})),
        ToolAction::CheckPreview
    );
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/output/render.jpeg"})),
        ToolAction::CheckPreview
    );
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/output/render.svg"})),
        ToolAction::CheckPreview
    );
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/output/render.webp"})),
        ToolAction::CheckPreview
    );
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/README.md"})),
        ToolAction::ReadFile
    );
}

#[test]
fn fixed_tool_names_need_no_input() {
    assert_eq!(
        summarize("Glob", &json!({"pattern": "*.py"})),
        ToolAction::SearchProject
    );
    assert_eq!(
        summarize("Grep", &json!({"pattern": "build123d"})),
        ToolAction::SearchProject
    );
    assert_eq!(
        summarize("TodoWrite", &json!({"todos": []})),
        ToolAction::UpdatePlan
    );
    assert_eq!(
        summarize("WebSearch", &json!({"query": "build123d docs"})),
        ToolAction::SearchWeb
    );
    assert_eq!(
        summarize("WebFetch", &json!({"url": "https://example.com"})),
        ToolAction::SearchWeb
    );
    // unknown tools fall through to the generic title
    assert_eq!(
        summarize("NotebookEdit", &json!({})),
        ToolAction::WorkProject
    );
}

#[test]
fn live_title_marks_streaming_partials() {
    let title = |action, lines, streaming| ToolTitle {
        action,
        lines,
        streaming,
    };
    // file_path classifies, the content key is still truncated: title plus ellipsis
    assert_eq!(
        live_title("Write", r#"{"file_path": "/tmp/p/model.py", "conte"#),
        title(ToolAction::WriteModel, None, true)
    );
    // complete content value: line count, no ellipsis
    assert_eq!(
        live_title(
            "Write",
            r#"{"file_path": "/tmp/p/model.py", "content": "a\nb\nc"}"#
        ),
        title(ToolAction::WriteModel, Some(3), false)
    );
    // content still streaming: lines so far plus the ellipsis
    assert_eq!(
        live_title(
            "Write",
            r#"{"file_path": "/tmp/p/model.py", "content": "a\nb"#
        ),
        title(ToolAction::WriteModel, Some(2), true)
    );
    // bare prefix and garbage never panic: the tool's generic title comes back
    assert_eq!(live_title("Write", "{"), ToolAction::WriteFile.into());
    assert_eq!(
        live_title("Bash", "not json"),
        ToolAction::WorkProject.into()
    );
    // the line suffix is Write-only; Edit keeps its plain title
    assert_eq!(
        live_title(
            "Edit",
            r#"{"file_path": "/tmp/p/model.py", "new_string": "x = 1"#
        ),
        title(ToolAction::EditModel, None, true)
    );
    // command partials classify too
    assert_eq!(
        live_title("Bash", r#"{"command": "python model.py"#),
        title(ToolAction::BuildModel, None, true)
    );
}

#[test]
fn live_title_ignores_keys_quoted_inside_values() {
    // "file_path" appears inside the command string, not as a key: it must not
    // leak into the classification, and the command still parses around it
    // (the partial is complete JSON — escaped quotes don't count toward the
    // streaming marker — so no ellipsis)
    let partial = r#"{"command": "grep \"file_path\":\"x\" model.py"}"#;
    assert_eq!(live_title("Bash", partial), ToolAction::ExamineFiles.into());
    // Read never reads the command key, so nothing is left to classify
    assert_eq!(live_title("Read", partial), ToolAction::ReadFile.into());
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
    assert_eq!(
        live_preview(r#"{"content": "c", "new_string": "n"}"#).as_deref(),
        Some("c")
    );
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
    assert_eq!(
        short_command("cd /root && python model.py", "/root", py),
        "python model.py"
    );
    assert_eq!(
        short_command("/root/.venv/bin/python model.py", "/root", py),
        "python model.py"
    );
    // replacement happens before the cd strip, like upstream
    assert_eq!(
        short_command("cd /root && /root/.venv/bin/python model.py", "/root", py),
        "python model.py"
    );
    // quoted venv paths (spaces) are replaced first, then the bare path
    assert_eq!(
        short_command(
            "\"/opt/my venv/python\" model.py",
            "/w",
            "/opt/my venv/python"
        ),
        "python model.py"
    );
    // only the first line counts
    assert_eq!(
        short_command("/root/.venv/bin/python model.py\nls -la", "/root", py),
        "python model.py"
    );
    // ';' separates when no "&&" is present
    assert_eq!(
        short_command("cd /root; python x.py", "/root", py),
        "python x.py"
    );
    // the cd target must contain the root, otherwise nothing is stripped
    assert_eq!(
        short_command("cd /elsewhere && python x.py", "/root", py),
        "cd /elsewhere && python x.py"
    );
    assert_eq!(
        short_command("python model.py", "/root", py),
        "python model.py"
    );
}

#[test]
fn short_command_empty_root_strips_any_cd() {
    // Carry-forward divergence: upstream requires the cd target to contain
    // the project root; the port's root-free classification (summarize's
    // Bash path) strips ANY leading cd, so a foreign-cd command classifies
    // one class up from upstream. Preserved deliberately.
    assert_eq!(
        short_command("cd /elsewhere && python x.py", "", ""),
        "python x.py"
    );
    assert_eq!(
        summarize(
            "Bash",
            &json!({"command": "cd /elsewhere && python model.py"})
        ),
        ToolAction::BuildModel // upstream: WorkProject
    );
    // The other two preserved divergences, restated where they live:
    assert_eq!(
        live_preview(r#"{"new_string": "a\nb"}"#).as_deref(),
        Some("a\nb")
    );
    assert_eq!(
        summarize("Read", &json!({"file_path": "/w/references/model.py"})),
        ToolAction::StudyReference // segment match, not just prefix
    );
}
