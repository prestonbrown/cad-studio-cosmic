use engine::project::layout::{Axis, LayoutNode, PaneKind};
use engine::project::*;
use std::path::PathBuf;

fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("cad-proj-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn metadata_roundtrips_upstream_shape() {
    let t = time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
    let m = ProjectMetadata {
        session_id: Some("abc-123".into()),
        current_version: Some(2),
        created_at: t,
        updated_at: t,
        app_version: "0.1.0".into(),
        layout: Some(LayoutNode::Split {
            axis: Axis::Row,
            first: Box::new(LayoutNode::Pane(PaneKind::Chat)),
            second: Box::new(LayoutNode::Pane(PaneKind::Model3D)),
            fraction: 0.34,
        }),
    };
    let json = serde_json::to_string(&m).unwrap();
    for k in [
        "\"session_id\"",
        "\"current_version\"",
        "\"created_at\"",
        "\"updated_at\"",
        "\"app_version\"",
    ] {
        assert!(json.contains(k), "snake_case CodingKeys: missing {k}");
    }
    assert!(json.contains("\"pane\":\"chat\""));
    assert!(json.contains("\"pane\":\"model3D\""), "exact casing");
    assert!(json.contains("\"axis\":\"row\""));
    assert!(json.contains("\"fraction\":0.34"));
    assert!(
        json.contains("2023-11-14T22:13:20Z"),
        "whole-second UTC dates: {json}"
    );
    assert!(!json.contains("22:13:20."), "no fractional seconds: {json}");
    assert!(json.contains('Z'), "UTC");
    let back: ProjectMetadata = serde_json::from_str(&json).unwrap();
    assert_eq!(back.created_at, t, "whole-second round-trip");
    assert_eq!(back.session_id.as_deref(), Some("abc-123"));
    assert_eq!(back.layout, m.layout);
}

#[test]
fn chat_roundtrips_all_roles_and_tools() {
    let t = time::OffsetDateTime::from_unix_timestamp(1_700_000_500).unwrap();
    let msgs = vec![
        ChatMessage {
            id: uuid::Uuid::new_v4(),
            role: Role::User,
            text: "make a box".into(),
            date: t,
            tool: None,
            detail: None,
            action: None,
            attachments: vec!["references/1-sketch.png".into()],
        },
        ChatMessage {
            id: uuid::Uuid::new_v4(),
            role: Role::Tool,
            text: String::new(),
            date: t,
            tool: Some(ToolActivity {
                tool_use_id: "tu_1".into(),
                name: "Bash".into(),
                input: "python model.py".into(),
                output: Some("ok".into()),
                preview: Some("Compiles the model".into()),
                state: ToolState::Succeeded,
            }),
            detail: None,
            action: None,
            attachments: vec![],
        },
        ChatMessage {
            id: uuid::Uuid::new_v4(),
            role: Role::System,
            text: "build failed".into(),
            date: t,
            tool: None,
            detail: Some("Traceback…".into()),
            action: Some(Action::AskClaudeToFix),
            attachments: vec![],
        },
    ];
    let json = serde_json::to_string(&msgs).unwrap();
    assert!(json.contains("\"toolUseID\""), "property-name casing");
    let back: Vec<ChatMessage> = serde_json::from_str(&json).unwrap();
    assert_eq!(back.len(), 3);
    assert_eq!(back[1].tool.as_ref().unwrap().tool_use_id, "tu_1");
    assert_eq!(back[2].action, Some(Action::AskClaudeToFix));
    assert_eq!(back[0].attachments.len(), 1);
}

#[test]
fn saves_are_atomic_no_tmp_residue() {
    let d = tmp();
    let mut m = ProjectMetadata::new("0.1.0");
    m.session_id = Some("s1".into());
    save_metadata(&d, &m).unwrap();
    assert!(d.join(".cadstudio/project.json").exists());
    save_chat(&d, &[]).unwrap();
    assert!(d.join(".cadstudio/chat.json").exists());
    let mut residue = 0;
    for e in std::fs::read_dir(d.join(".cadstudio")).unwrap() {
        if e.unwrap().file_name().to_string_lossy().ends_with(".tmp") {
            residue += 1;
        }
    }
    assert_eq!(residue, 0);
    let m2 = load_metadata(&d).unwrap();
    assert_eq!(m2.session_id.as_deref(), Some("s1"));
}

#[test]
fn project_name_validation_rules() {
    assert!(validate_project_name("Gearbox v2").is_ok());
    assert!(validate_project_name("").is_err());
    assert!(validate_project_name(".hidden").is_err(), "no leading dot");
    assert!(validate_project_name("a/b").is_err());
    assert!(validate_project_name("a:b").is_err());
    assert!(validate_project_name("plain-name 123").is_ok());
}

/// Upstream RecentProjects.remove (RecentProjects.swift:25-28): the entry
/// leaves the list; everything else keeps its order. Canonicalized removal
/// so a different spelling of the same project still hits.
#[test]
fn recents_remove_drops_only_the_named_entry() {
    let d = tmp();
    let mut r = RecentProjects::default();
    for name in ["a", "b", "c"] {
        let p = d.join(name);
        std::fs::create_dir_all(&p).unwrap();
        r.add(p.clone());
    }
    let b = d.join("b").canonicalize().unwrap();
    r.remove(d.join("b"));
    assert!(!r.0.contains(&b), "the named entry is gone");
    assert_eq!(r.0.len(), 2, "only that entry leaves");
    assert_eq!(r.0[0], d.join("c").canonicalize().unwrap(), "order kept");
    r.remove(d.join("not-in-the-list"));
    assert_eq!(r.0.len(), 2, "removing an absent entry is a no-op");
}

/// Upstream RecentProjects.clear (RecentProjects.swift:30-33).
#[test]
fn recents_clear_empties_the_list() {
    let d = tmp();
    let mut r = RecentProjects::default();
    let p = d.join("a");
    std::fs::create_dir_all(&p).unwrap();
    r.add(p);
    r.clear();
    assert!(r.0.is_empty());
}

#[test]
fn recents_dedupe_evict_and_filter_missing() {
    let d = tmp();
    let mut r = RecentProjects::default();
    for i in 0..14 {
        let p = d.join(format!("p{i}"));
        std::fs::create_dir_all(&p).unwrap();
        r.add(p.clone());
    }
    assert_eq!(r.0.len(), 12, "max 12");
    r.add(d.join("p5"));
    assert_eq!(r.0.len(), 12);
    assert_eq!(r.0[0], d.join("p5").canonicalize().unwrap());
    let p0 = d.join("p0").canonicalize().unwrap();
    std::fs::remove_dir_all(d.join("p0")).unwrap();
    // p6 is in the 12-entry list (p0/p1 were evicted by the cap); deleting it
    // makes filter_existing observable.
    let p6 = d.join("p6").canonicalize().unwrap();
    std::fs::remove_dir_all(d.join("p6")).unwrap();
    let filtered = r.filter_existing();
    assert!(!filtered.0.contains(&p0));
    assert!(
        !filtered.0.contains(&p6),
        "deleted projects are filtered out"
    );
    assert!(
        filtered.0.contains(&d.join("p4").canonicalize().unwrap()),
        "existing projects are kept"
    );
    assert_eq!(filtered.0.len(), 11);
}

// ─── project creation (M4 task 0) ────────────────────────────────────────────

/// `create_project` scaffolds the upstream tree (ProjectStore.create +
/// prepare, contract §4:126): the root as a leaf of the parent, `output/`,
/// `.cadstudio/logs/`, and a default `project.json` that loads back.
#[test]
fn create_project_scaffolds_tree() {
    let parent = tmp();
    let root = create_project(&parent, "gearbox").unwrap();
    assert_eq!(root, parent.join("gearbox"));
    assert!(root.join("output").is_dir(), "output/ scaffolded");
    assert!(
        root.join(".cadstudio").join("logs").is_dir(),
        ".cadstudio/logs/ scaffolded"
    );
    let metadata = load_metadata(&root).unwrap();
    assert_eq!(
        metadata.session_id, None,
        "a fresh project has no session yet"
    );
    assert!(!metadata.app_version.is_empty());
    // upstream refuses to clobber a folder that already exists
    assert!(
        create_project(&parent, "gearbox").is_err(),
        "second create must fail"
    );
}

/// An invalid name fails before anything is created — no partial scaffold.
#[test]
fn create_project_rejects_invalid_name() {
    let parent = tmp();
    for name in ["", ".hidden", "a/b", "a:b"] {
        assert!(create_project(&parent, name).is_err(), "{name:?}");
    }
    assert_eq!(
        std::fs::read_dir(&parent).unwrap().count(),
        0,
        "nothing scaffolded for an invalid name"
    );
}

/// Upstream `ProjectStore.prepare` (ProjectStore.swift:46-57): opening a
/// project re-creates any missing scaffolding and leaves a present
/// project.json untouched.
#[test]
fn prepare_restores_scaffolding_without_clobbering_metadata() {
    let d = tmp();
    let root = d.join("opened");
    std::fs::create_dir_all(&root).unwrap();
    prepare(&root).unwrap();
    for path in ["output", ".cadstudio/logs"] {
        assert!(root.join(path).is_dir(), "{path} exists after prepare");
    }
    assert!(root.join(".cadstudio/project.json").is_file());

    // a second prepare over an already-prepared project changes nothing
    let before = std::fs::read_to_string(root.join(".cadstudio/project.json")).unwrap();
    prepare(&root).unwrap();
    let after = std::fs::read_to_string(root.join(".cadstudio/project.json")).unwrap();
    assert_eq!(before, after, "an existing manifest is never rewritten");
}
