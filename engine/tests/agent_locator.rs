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
    assert_eq!(home_candidates(&home), want);
}

#[test]
fn discovery_uses_custom_path_then_candidates() {
    // ONE unsafe block, serial by construction (single test):
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
