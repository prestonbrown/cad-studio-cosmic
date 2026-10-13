//! Project store (upstream ProjectStore.swift + ChatMessage.swift): metadata
//! and chat persisted under `<project>/.cadstudio/`, saved atomically via
//! temp-file + rename (same approach as [`crate::settings::Settings::save`]),
//! the recent-projects list, the project-name rules used by the new-project
//! dialog, and [`create_project`]'s new-project scaffolding.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

use crate::project::layout::LayoutNode;

/// Whole-second UTC RFC 3339. Upstream's `.iso8601` date strategy rejects
/// fractional seconds, so writes truncate to whole seconds and force a `Z`
/// suffix; reads stay tolerant (time's Rfc3339 parser accepts fractions).
pub mod iso8601_utc {
    use serde::{Deserialize, Deserializer, Serializer};
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;

    pub fn serialize<S: Serializer>(t: &OffsetDateTime, s: S) -> Result<S::Ok, S::Error> {
        let t = t
            .to_utc()
            .replace_nanosecond(0)
            .map_err(serde::ser::Error::custom)?;
        s.serialize_str(&t.format(&Rfc3339).map_err(serde::ser::Error::custom)?)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<OffsetDateTime, D::Error> {
        let text = String::deserialize(d)?;
        OffsetDateTime::parse(&text, &Rfc3339).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
    Thinking,
    System,
    Summary,
}

/// Upstream rawValues are camelCase ("askClaudeToFix", "openTerminal",
/// "configureClaude").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    AskClaudeToFix,
    OpenTerminal,
    ConfigureClaude,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolState {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolActivity {
    /// Upstream property name keeps the capital ID; camelCase would yield
    /// "toolUseId".
    #[serde(rename = "toolUseID")]
    pub tool_use_id: String,
    pub name: String,
    pub input: String,
    pub output: Option<String>,
    pub preview: Option<String>,
    pub state: ToolState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: uuid::Uuid,
    pub role: Role,
    pub text: String,
    #[serde(with = "iso8601_utc")]
    pub date: OffsetDateTime,
    pub tool: Option<ToolActivity>,
    pub detail: Option<String>,
    pub action: Option<Action>,
    /// Upstream encodes a nil attachment array by omitting the key
    /// (encodeIfPresent), so tolerate its absence on read.
    #[serde(default)]
    pub attachments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMetadata {
    pub session_id: Option<String>,
    pub current_version: Option<i64>,
    #[serde(with = "iso8601_utc")]
    pub created_at: OffsetDateTime,
    #[serde(with = "iso8601_utc")]
    pub updated_at: OffsetDateTime,
    pub app_version: String,
    pub layout: Option<LayoutNode>,
}

impl ProjectMetadata {
    pub fn new(app_version: &str) -> Self {
        let now = OffsetDateTime::now_utc();
        Self {
            session_id: None,
            current_version: None,
            created_at: now,
            updated_at: now,
            app_version: app_version.to_owned(),
            layout: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectStoreError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    InvalidName(#[from] ProjectNameError),
    /// Upstream `ProjectError.notAFolder` (ProjectStore.swift:48-50): the
    /// path a project was asked to open is not a directory.
    #[error("project path is not a folder: {0}")]
    NotAFolder(PathBuf),
}

/// Per-project support directory (upstream ProjectFolder.support) holding
/// project.json and chat.json.
fn support_dir(project_root: &Path) -> PathBuf {
    project_root.join(".cadstudio")
}

/// Write via temp-file + rename so a reader never sees a torn file; the temp
/// name must keep the file extension so tooling still recognizes it.
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| std::io::Error::other("atomic_write: path has no file name"))?;
    let tmp = path.with_file_name(format!("{name}.tmp"));
    {
        let mut file = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn save_metadata(
    project_root: &Path,
    metadata: &ProjectMetadata,
) -> Result<(), ProjectStoreError> {
    let json = serde_json::to_string_pretty(metadata)?;
    atomic_write(
        &support_dir(project_root).join("project.json"),
        json.as_bytes(),
    )?;
    Ok(())
}

pub fn load_metadata(project_root: &Path) -> Result<ProjectMetadata, ProjectStoreError> {
    let json = std::fs::read_to_string(support_dir(project_root).join("project.json"))?;
    Ok(serde_json::from_str(&json)?)
}

pub fn save_chat(project_root: &Path, messages: &[ChatMessage]) -> Result<(), ProjectStoreError> {
    let json = serde_json::to_string_pretty(messages)?;
    atomic_write(
        &support_dir(project_root).join("chat.json"),
        json.as_bytes(),
    )?;
    Ok(())
}

pub fn load_chat(project_root: &Path) -> Result<Vec<ChatMessage>, ProjectStoreError> {
    let json = std::fs::read_to_string(support_dir(project_root).join("chat.json"))?;
    Ok(serde_json::from_str(&json)?)
}

/// The name becomes a folder inside the parent directory, so it must be
/// non-empty and must not hide the project (leading dot) or escape the parent
/// ("/"); ":" is rejected as the classic HFS+/Windows path separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProjectNameError {
    #[error("project name must not be empty")]
    Empty,
    #[error("project name must not start with a period")]
    LeadingDot,
    #[error("project name must not contain {0:?}")]
    BadCharacter(char),
}

pub fn validate_project_name(name: &str) -> Result<(), ProjectNameError> {
    if name.is_empty() {
        Err(ProjectNameError::Empty)
    } else if name.starts_with('.') {
        Err(ProjectNameError::LeadingDot)
    } else if let Some(c) = name.chars().find(|&c| c == '/' || c == ':') {
        Err(ProjectNameError::BadCharacter(c))
    } else {
        Ok(())
    }
}

/// Ensure an EXISTING project's scaffolding is complete (upstream
/// `ProjectStore.prepare`, ProjectStore.swift:46-57 — called by `Project`
/// on open, not only at create): `output/`, `.cadstudio/logs/`, and a
/// default `project.json` when none is present.
pub fn prepare(project_root: &Path) -> Result<(), ProjectStoreError> {
    // upstream guards `fileExists(isDirectory:)` and throws `.notAFolder`:
    // opening a typo'd or file path must not scaffold a tree around it
    if !project_root.is_dir() {
        return Err(ProjectStoreError::NotAFolder(project_root.to_path_buf()));
    }
    std::fs::create_dir_all(project_root.join("output"))?;
    std::fs::create_dir_all(project_root.join(".cadstudio").join("logs"))?;
    if !support_dir(project_root).join("project.json").exists() {
        save_metadata(
            project_root,
            &ProjectMetadata::new(env!("CARGO_PKG_VERSION")),
        )?;
    }
    Ok(())
}

/// Scaffold a new project (upstream `ProjectStore.create` + `prepare`,
/// contract §4:126): validate the name, create the root as a LEAF directory
/// (the parent must already exist and the root must not — a collision
/// surfaces as the io `AlreadyExists` error), then [`prepare`].
/// Returns the project root.
pub fn create_project(parent: &Path, name: &str) -> Result<PathBuf, ProjectStoreError> {
    validate_project_name(name)?;
    let root = parent.join(name);
    std::fs::create_dir(&root)?;
    prepare(&root)?;
    Ok(root)
}

/// Cap shared with upstream's recents list.
pub const MAX_RECENT_PROJECTS: usize = 12;

/// Recently-opened project roots, most recent first. Paths are stored
/// canonicalized so the same project reached through different spellings
/// dedupes to one entry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecentProjects(pub Vec<PathBuf>);

impl RecentProjects {
    pub fn add(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref();
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.0.retain(|p| *p != canonical);
        self.0.insert(0, canonical);
        self.0.truncate(MAX_RECENT_PROJECTS);
    }

    pub fn filter_existing(self) -> Self {
        Self(self.0.into_iter().filter(|p| p.exists()).collect())
    }

    /// Drop one entry (upstream `remove`, RecentProjects.swift:25-28). The
    /// comparison canonicalizes so a different spelling of the same project
    /// still hits.
    pub fn remove(&mut self, path: impl AsRef<Path>) {
        let canonical =
            std::fs::canonicalize(path.as_ref()).unwrap_or_else(|_| path.as_ref().to_path_buf());
        self.0.retain(|p| *p != canonical);
    }

    /// Empty the list (upstream `clear`, RecentProjects.swift:30-33).
    pub fn clear(&mut self) {
        self.0.clear();
    }
}
