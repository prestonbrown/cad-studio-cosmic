//! Agent executable discovery: custom path → login-shell PATH → fixed
//! candidates, each validated by running `--version` (AgentLocator.swift).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Which agent CLI to locate (upstream `AgentEngine`); Claude is the M2 agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
}

impl AgentKind {
    /// The executable name searched for in PATH directories.
    pub fn command(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
        }
    }
}

/// A located agent: a validated executable and its `--version` output
/// (upstream `AgentLocator.State.found`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPath {
    pub path: PathBuf,
    pub version: String,
}

/// Expand a bare `~` or `~/` prefix against `home`; any other path returns
/// unchanged (`~user` forms never expand — `NSString.expandingTildeInPath`).
pub fn expand_tilde(path: &str, home: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else if path == "~" {
        home.to_string()
    } else {
        path.to_string()
    }
}

/// Fixed candidate executables in upstream order: `~/.claude/local/claude`
/// first, then the user-local install dirs and system locations. Relative
/// entries resolve against `home`.
pub fn home_candidates(home: &Path) -> Vec<PathBuf> {
    [
        ".claude/local/claude",
        ".local/bin/claude",
        ".cargo/bin/claude",
        ".npm-global/bin/claude",
        "/usr/local/bin/claude",
        "/usr/bin/claude",
    ]
    .into_iter()
    .map(|rel| {
        if rel.starts_with('/') {
            PathBuf::from(rel)
        } else {
            home.join(rel)
        }
    })
    .collect()
}

/// Locate the agent: custom path (tilde-expanded, validated) → merged
/// login-shell PATH dirs in order → [`home_candidates`] in order. The first
/// candidate whose `--version` exits 0 wins (upstream `locate()`).
pub fn find_agent(kind: AgentKind, custom_path: Option<&str>) -> Option<AgentPath> {
    let home = std::env::var("HOME").unwrap_or_default();
    let env = shell_env();
    let name = kind.command();

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(custom) = custom_path.filter(|c| !c.is_empty()) {
        candidates.push(PathBuf::from(expand_tilde(custom, &home)));
    }
    if let Some(path) = env.get("PATH") {
        candidates.extend(path.split(':').map(|dir| Path::new(dir).join(name)));
    }
    candidates.extend(home_candidates(Path::new(&home)));

    candidates
        .into_iter()
        .filter(|c| is_executable_file(c))
        .find_map(|c| validate(&c, &env).map(|version| AgentPath { path: c, version }))
}

/// The directories executable discovery searches: the env's PATH dirs in
/// order, with `~/.local/bin` appended when absent (the fallback dir
/// upstream's uv install relies on that a default PATH may lack).
pub(crate) fn executable_dirs(env: &HashMap<String, String>) -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = env
        .get("PATH")
        .map(|path| path.split(':').map(PathBuf::from).collect())
        .unwrap_or_default();
    let local = Path::new(&home).join(".local/bin");
    if !dirs.contains(&local) {
        dirs.push(local);
    }
    dirs
}

/// First searchable directory holding an executable file `name` (upstream
/// `ShellEnvironment.locate`, which PATH-searches without validating).
pub fn find_executable(name: &str, env: &HashMap<String, String>) -> Option<PathBuf> {
    executable_dirs(env)
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable_file(candidate))
}

/// The environment agents are spawned with: the current environment with
/// `PATH` replaced by the login-shell PATH plus the fallback dirs appended
/// (ShellEnvironment.swift).
pub fn shell_env() -> HashMap<String, String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("PATH".to_string(), merged_path(&home));
    env
}

/// Login-shell PATH dirs with the fallback dirs appended when absent
/// (ShellEnvironment's `path()`).
fn merged_path(home: &str) -> String {
    let mut dirs: Vec<String> = login_path_output()
        .map(|path| path.split(':').map(str::to_string).collect())
        .unwrap_or_default();
    for dir in fallback_dirs(home) {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs.join(":")
}

fn fallback_dirs(home: &str) -> Vec<String> {
    [
        "~/.local/bin",
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ]
    .iter()
    .map(|dir| expand_tilde(dir, home))
    .collect()
}

/// The PATH a fresh login shell computes for the current HOME. `PATH` is
/// stripped from the login shell's environment so the output is rebuilt from
/// the profile files rather than inherited — upstream gets the same effect by
/// running the login shell from a GUI app whose inherited PATH is minimal.
/// `$SHELL` runs when present, `/bin/bash` otherwise; profile noise before
/// the echo is skipped by taking the last non-empty line.
fn login_path_output() -> Option<String> {
    let shells = [std::env::var("SHELL").ok(), Some("/bin/bash".to_string())];
    for shell in shells.into_iter().flatten() {
        let Ok(out) = Command::new(&shell)
            .args(["-lc", "echo $PATH"])
            .env_remove("PATH")
            .output()
        else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        if let Some(line) = text.lines().rev().map(str::trim).find(|l| !l.is_empty()) {
            return Some(line.to_string());
        }
    }
    None
}

/// `FileManager.isExecutableFile`: a regular file with an execute bit.
pub(crate) fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

/// Run `--version`; the version is the whole trimmed stdout when the exit
/// status is 0 (upstream `version(of:)`).
fn validate(path: &Path, env: &HashMap<String, String>) -> Option<String> {
    let out = Command::new(path)
        .arg("--version")
        .envs(env)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
