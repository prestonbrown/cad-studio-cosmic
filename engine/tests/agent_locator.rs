use engine::agent::locator::{AgentKind, expand_tilde, find_agent, home_candidates};

fn mk_bin(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

#[test]
fn tilde_expansion_is_pure() {
    assert_eq!(
        expand_tilde("~/bin/claude", "/home/p"),
        "/home/p/bin/claude"
    );
    assert_eq!(expand_tilde("/abs/claude", "/home/p"), "/abs/claude");
    assert_eq!(
        expand_tilde("~x/claude", "/home/p"),
        "~x/claude",
        "only bare ~ expands"
    );
}

/// AgentEngine's per-engine surface (AgentLocator.swift:3-61): display
/// name, command, login command; unknown agentEngine strings read claude
/// (`AgentEngine(rawValue:) ?? .claude`).
#[test]
fn kinds_carry_the_upstream_surface() {
    use engine::agent::locator::AgentKind;
    assert_eq!(AgentKind::Claude.display_name(), "Claude Code");
    assert_eq!(AgentKind::Codex.display_name(), "Codex");
    assert_eq!(AgentKind::Claude.command(), "claude");
    assert_eq!(AgentKind::Codex.command(), "codex");
    assert_eq!(AgentKind::Claude.login_command(), "claude");
    assert_eq!(AgentKind::Codex.login_command(), "codex login");
    assert_eq!(AgentKind::from_raw("claude"), AgentKind::Claude);
    assert_eq!(AgentKind::from_raw("codex"), AgentKind::Codex);
    assert_eq!(AgentKind::from_raw(""), AgentKind::Claude);
    assert_eq!(AgentKind::from_raw("gpt-agent"), AgentKind::Claude);
    assert_eq!(AgentKind::default(), AgentKind::Claude);
}

/// The codex candidate set (AgentLocator.swift:57-58), adapted like the
/// claude list was: home-relative dirs first, then the system locations —
/// /opt/homebrew is macOS-only and does not port; /usr/bin joins it.
#[test]
fn codex_candidates_under_fake_home() {
    let home = std::env::temp_dir().join(format!("cad-loc-{}", uuid::Uuid::new_v4()));
    let want: Vec<std::path::PathBuf> = [
        ".local/bin/codex",
        ".npm-global/bin/codex",
        ".cargo/bin/codex",
        "/usr/local/bin/codex",
        "/usr/bin/codex",
    ]
    .iter()
    .map(|r| {
        if r.starts_with('/') {
            r.into()
        } else {
            home.join(r)
        }
    })
    .collect();
    assert_eq!(home_candidates(AgentKind::Codex, &home), want);
}

/// find_agent locates a codex binary by name with the same validation the
/// claude arm runs (one --version probe, first working candidate wins).
/// HOME mutators serialize: the harness runs this file's tests in
/// parallel, and two set_var("HOME") windows racing fall through to real
/// PATH binaries (the codex_runner ENV_LOCK pattern).
static HOME_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

#[test]
fn codex_discovery_validates_version() {
    let _home = HOME_LOCK.lock().unwrap();
    // serial under HOME_LOCK across this file's tests
    unsafe {
        let home = std::env::temp_dir().join(format!("cad-loc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        let good = mk_bin(
            &home.join(".local/bin"),
            "codex",
            "#!/bin/sh\necho 'codex-cli 0.45.0'\n",
        );
        std::env::set_var("HOME", &home);
        let found = find_agent(AgentKind::Codex, None).expect("codex found in ~/.local/bin");
        assert_eq!(found.path, good);
        assert!(found.version.contains("0.45.0"));
        // a custom path wins over the candidates, per engine
        let custom = mk_bin(&home, "my-codex", "#!/bin/sh\necho '0.9.0'\n");
        let found2 = find_agent(AgentKind::Codex, Some(custom.to_str().unwrap())).unwrap();
        assert_eq!(found2.path, custom);
        std::env::remove_var("HOME");
    }
}

#[test]
fn candidates_follow_upstream_order_under_fake_home() {
    let home = std::env::temp_dir().join(format!("cad-loc-{}", uuid::Uuid::new_v4()));
    // upstream AgentLocator.swift order: ~/.claude/local/claude FIRST, then ~/.local/bin/claude, …
    let want: Vec<std::path::PathBuf> = [
        ".claude/local/claude",
        ".local/bin/claude",
        ".cargo/bin/claude",
        ".npm-global/bin/claude",
        "/usr/local/bin/claude",
        "/usr/bin/claude",
    ]
    .iter()
    .map(|r| {
        if r.starts_with('/') {
            r.into()
        } else {
            home.join(r)
        }
    })
    .collect();
    assert_eq!(home_candidates(AgentKind::Claude, &home), want);
}

#[test]
fn discovery_uses_custom_path_then_candidates() {
    let _home = HOME_LOCK.lock().unwrap();
    // serial under HOME_LOCK across this file's tests
    unsafe {
        let home = std::env::temp_dir().join(format!("cad-loc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(home.join(".claude/local")).unwrap();
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        let good = mk_bin(
            &home.join(".claude/local"),
            "claude",
            "#!/bin/sh\necho '2.0.0 (fake)'\n",
        );
        let _bad = mk_bin(&home.join(".local/bin"), "claude", "#!/bin/sh\nexit 3\n");
        std::env::set_var("HOME", &home);
        // candidate order: ~/.claude/local wins over the failing ~/.local/bin binary
        let found = find_agent(AgentKind::Claude, None).expect("first valid candidate found");
        assert_eq!(found.path, good);
        assert!(
            found.version.contains("2.0.0"),
            "version = whole trimmed --version output"
        );
        // invalid custom path is rejected (falls through to candidates, none valid → None
        // here because we did not put anything on PATH discovery)
        let bad_custom = mk_bin(&home, "failing-claude", "#!/bin/sh\nexit 3\n");
        let found2 = find_agent(AgentKind::Claude, Some(bad_custom.to_str().unwrap()));
        assert_ne!(
            found2.as_ref().map(|p| p.path.clone()),
            Some(bad_custom),
            "--version exit != 0 must be rejected"
        );
        // custom good path beats candidates
        let custom = mk_bin(&home, "my-claude", "#!/bin/sh\necho '1.0.0 (fake)'\n");
        let found3 = find_agent(AgentKind::Claude, Some(custom.to_str().unwrap())).unwrap();
        assert_eq!(found3.path, custom);
        std::env::remove_var("HOME");
    }
}
