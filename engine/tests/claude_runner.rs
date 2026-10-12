//! The claude runner's pure surface, transcribed from the vendored Swift:
//! argv construction (fixtures/upstream-source/ClaudeRunner.swift:28-65),
//! system-prompt rendering (:67-79), the {{ENVIRONMENT}} bullets
//! (AgentSession.swift:197-217) and turn-prompt assembly with the attachment
//! and missing-output notes (Project.swift:210-256).

use engine::agent::claude::{
    PromptCtx, TurnRequest, build_args, build_prompt, copy_attachments, environment_bullets,
    render_system_prompt,
};
use engine::agent::locator::AgentPath;
use std::path::{Path, PathBuf};

fn fake_path() -> AgentPath {
    AgentPath {
        path: "/usr/bin/claude".into(),
        version: String::new(),
    }
}

fn golden() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden")
}

fn fixture_upstream(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/upstream")
        .join(p)
}

/// A temp project with the real on-disk layout: model.py at the root when
/// asked, the named golden files under output/.
fn mk_project(root_files: &[&str], output_files: &[&str]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("cad-claude-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("output")).unwrap();
    if root_files.contains(&"model.py") {
        std::fs::write(root.join("model.py"), "# model\n").unwrap();
    }
    for file in output_files {
        std::fs::copy(golden().join(file), root.join("output").join(file)).unwrap();
    }
    root
}

#[test]
fn argv_default_isolation() {
    let claude = fake_path();
    let s = engine::settings::Settings::default(); // isolates_claude = true
    let args = build_args(
        &claude,
        "make a box",
        "SYSTEM",
        &s,
        None,
        std::path::Path::new("/v/python"),
    );
    assert_eq!(args[0], "-p");
    let joined = args.join("\u{1f}");
    assert!(joined.contains("--output-format\u{1f}stream-json"));
    assert!(joined.contains("--verbose"));
    assert!(joined.contains("--include-partial-messages"));
    assert!(joined.contains("--permission-mode\u{1f}acceptEdits"));
    assert!(joined.contains("--allowedTools\u{1f}Read,Write,Edit,Glob,Grep,Bash"));
    assert!(joined.contains("--strict-mcp-config"));
    assert!(joined.contains("--setting-sources\u{1f}project,local"));
    assert!(!joined.contains("--resume"));
}

#[test]
fn argv_default_full_vector() {
    // the whole argv, in upstream order, for the default flag combination
    let s = engine::settings::Settings::default();
    let args = build_args(
        &fake_path(),
        "make a box",
        "SYS",
        &s,
        None,
        Path::new("/v/python"),
    );
    assert_eq!(
        args,
        [
            "-p",
            "make a box",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--append-system-prompt",
            "SYS",
            "--permission-mode",
            "acceptEdits",
            "--allowedTools",
            "Read,Write,Edit,Glob,Grep,Bash",
            "--strict-mcp-config",
            "--setting-sources",
            "project,local",
        ]
    );
}

#[test]
fn argv_restricted_bash_allowlist_is_15_separate_entries() {
    let mut s = engine::settings::Settings::default();
    s.restricted_bash = true;
    let args = build_args(
        &fake_path(),
        "p",
        "SYS",
        &s,
        None,
        std::path::Path::new("/v/python"),
    );
    let expected = [
        "Read",
        "Write",
        "Edit",
        "Glob",
        "Grep",
        "Bash(/v/python:*)",
        "Bash(ls:*)",
        "Bash(cat:*)",
        "Bash(head:*)",
        "Bash(tail:*)",
        "Bash(wc:*)",
        "Bash(file:*)",
        "Bash(test:*)",
        "Bash(pwd:*)",
        "Bash(stat:*)",
    ];
    // 15 consecutive separate argv entries after --allowedTools
    let i = args
        .iter()
        .position(|a| a == "--allowedTools")
        .expect("flag present");
    assert_eq!(
        &args[i + 1..i + 16],
        expected,
        "15 separate entries, exact order"
    );
    // spacey python path → the entry itself carries quotes
    let args2 = build_args(
        &fake_path(),
        "p",
        "SYS",
        &s,
        None,
        std::path::Path::new("/opt/my venv/python"),
    );
    assert!(
        args2.contains(&"Bash(\"/opt/my venv/python\":*)".to_string()),
        "quoted inside the entry"
    );
}

#[test]
fn argv_isolation_off_omits_flags() {
    let mut s = engine::settings::Settings::default();
    s.isolates_claude = false;
    let args = build_args(
        &fake_path(),
        "p",
        "SYS",
        &s,
        None,
        std::path::Path::new("/v/python"),
    );
    let joined = args.join("\u{1f}");
    assert!(!joined.contains("--strict-mcp-config"));
    assert!(!joined.contains("--setting-sources"));
}

#[test]
fn argv_resume_fork_model_effort() {
    let mut s = engine::settings::Settings::default();
    s.claude_model = Some("opus".into());
    s.claude_effort = Some("high".into());
    let args = build_args(
        &fake_path(),
        "p",
        "SYS",
        &s,
        Some("sid-123"),
        std::path::Path::new("/v/python"),
    );
    let joined = args.join("\u{1f}");
    assert!(joined.contains("--resume\u{1f}sid-123"));
    assert!(joined.contains("--fork-session"));
    assert!(joined.contains("--model\u{1f}opus"));
    assert!(joined.contains("--effort\u{1f}high"));
}

#[test]
fn argv_nonstreaming_json() {
    let mut s = engine::settings::Settings::default();
    s.streaming_output = false;
    let args = build_args(
        &fake_path(),
        "p",
        "SYS",
        &s,
        None,
        std::path::Path::new("/v/python"),
    );
    let joined = args.join("\u{1f}");
    assert!(joined.contains("--output-format\u{1f}json"));
    assert!(!joined.contains("--include-partial-messages"));
    assert!(!joined.contains("stream-json"));
}

#[test]
fn system_prompt_all_placeholders_rendered() {
    let tpl = std::fs::read_to_string(fixture_upstream("CADSystemPrompt.md")).unwrap();
    let ctx = PromptCtx {
        project_name: "Gearbox".into(),
        python_display: "python".into(),
        environment: vec!["model.py does not exist yet: create it".into()],
        units: "millimeters (mm)".into(),
        printer: "Bambu Lab P1S".into(),
        guidelines: "walls".into(),
        date: "2026-10-12".into(),
        venv_python: "/v/python".into(),
    };
    let out = render_system_prompt(&tpl, &ctx);
    assert!(!out.contains("{{"), "no placeholder left: {out}");
    assert!(out.contains("Gearbox"));
    assert!(out.contains("model.py does not exist yet: create it"));
}

#[test]
fn environment_bullets_match_agent_session() {
    // fresh project: no model.py, empty output/, no references/ yet
    let root = mk_project(&[], &[]);
    let bullets = environment_bullets(&root, "python interpreter not verified yet");
    assert_eq!(
        bullets[0],
        "- Python packages: python interpreter not verified yet"
    );
    assert_eq!(bullets[1], "- model.py does not exist yet: create it");
    assert_eq!(bullets[2], "- output/: empty");
    assert_eq!(
        bullets.len(),
        3,
        "references/ bullet only when the dir exists: {bullets:?}"
    );

    // model.py with a trailing newline still counts the empty tail as a
    // line (Swift split(omittingEmptySubsequences: false)); a non-empty
    // output/ lists sorted names; an existing empty references/ says empty
    std::fs::write(root.join("model.py"), "a\nb\n").unwrap();
    std::fs::copy(
        golden().join("model.stl"),
        root.join("output").join("model.stl"),
    )
    .unwrap();
    std::fs::create_dir_all(root.join("references")).unwrap();
    let bullets = environment_bullets(&root, "venv line");
    assert_eq!(bullets[0], "- Python packages: venv line");
    assert_eq!(bullets[1], "- model.py exists (3 lines)");
    assert_eq!(bullets[2], "- output/: model.stl");
    assert_eq!(bullets[3], "- references/: empty");
}

#[test]
fn prompt_assembly_with_attachments_and_missing_note() {
    // temp project: model.py present, output/ has model.3mf only
    let root = mk_project(&["model.py"], &["model.3mf"]);
    std::fs::write("/tmp/sketch.png", b"png").unwrap();
    let req = TurnRequest {
        prompt: String::new(),
        attachments: vec![PathBuf::from("/tmp/sketch.png")],
    };
    // copy_attachments first (writes references/), then build_prompt
    let rels = copy_attachments(&req, &root);
    assert_eq!(rels.len(), 1);
    // a ctx over a root without model.py reports nothing missing (the
    // missing_requirements gate), so the attachment sections stand alone
    let no_missing = engine::project::outputs::MissingCtx {
        project_root: Path::new("/nonexistent"),
        output_dir: Path::new("/nonexistent/output"),
        parts_count: 2,
        printer_model: None,
        manifest: None,
    };
    let p = build_prompt(&req, &root, &no_missing);
    assert!(p.contains("Create a model based on the attached files."));
    assert!(p.contains(&rels[0]));
    let missing = engine::project::outputs::MissingCtx {
        project_root: &root,
        output_dir: &root.join("output"),
        parts_count: 2,
        printer_model: Some("P1S"),
        manifest: None,
    };
    let p2 = build_prompt(
        &TurnRequest {
            prompt: "go".into(),
            attachments: vec![],
        },
        &root,
        &missing,
    );
    assert!(
        p2.contains("Note from CAD Studio: model.py does not produce these required outputs yet.")
    );
    assert!(p2.contains("model.stl"));
    assert!(
        p2.contains("one drawing page per piece"),
        "parts_count>1 note flows through"
    );
}

#[test]
fn attachments_copied_with_timestamp_names() {
    let root = mk_project(&[], &[]);
    std::fs::write("/tmp/cad-att-sketch.png", b"png").unwrap();
    let req = TurnRequest {
        prompt: "x".into(),
        attachments: vec![PathBuf::from("/tmp/cad-att-sketch.png")],
    };
    let rels = copy_attachments(&req, &root);
    let refs_dir = root.join("references");
    let names: Vec<_> = std::fs::read_dir(&refs_dir).unwrap().flatten().collect();
    assert_eq!(names.len(), 1);
    let name = names[0].file_name().into_string().unwrap();
    assert!(
        name.ends_with("-1-cad-att-sketch.png"),
        "unixts-1-name (upstream numbers from 1): {name}"
    );
    assert_eq!(rels[0], format!("references/{name}"));
}
