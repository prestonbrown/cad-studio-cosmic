//! The claude runner's pure half: argv construction, system-prompt rendering
//! and turn-prompt assembly, transcribed from the vendored Swift —
//! fixtures/upstream-source/ClaudeRunner.swift for argv and the template
//! substitutions, AgentSession.swift:197-217 for the {{ENVIRONMENT}} bullets,
//! Project.swift:245-256 for the two prompt notes (the attachment COPIES
//! themselves are the bridge's turn-start job, [`crate::agent::attachments`]).
//! Nothing here spawns a process: the session pairs [`build_args`] with
//! [`crate::processes::spawn_streaming`].

use std::path::{Path, PathBuf};

use crate::agent::locator::AgentPath;
use crate::project::folder;
use crate::project::outputs::{MissingCtx, missing_requirements};
use crate::settings::Settings;

/// One user turn: the raw prompt text plus the attachment facts the bridge
/// established at turn start — the `references/<stamp>-<n>-<name>` names of
/// the copies [`crate::agent::attachments::copy_attachments`] made (upstream
/// threads the same list into the row, the prompt, and `images:`,
/// Project.swift:201-206) and the root-resolved copies the codex argv
/// filters for `-i`.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnRequest {
    pub prompt: String,
    /// The prompt-relative names of this turn's attachment copies. The
    /// prompt lists exactly these (upstream `prompt(text:references:)`,
    /// Project.swift:203-206); the engine never re-derives them from disk.
    pub references: Vec<String>,
    /// Image attachments the CODEX argv carries as `-i` flags (upstream
    /// `ClaudeRequest.images`, Project.swift:206: the `references/` copies,
    /// root-resolved; the argv filters them to images). Claude's argv
    /// consumes none — upstream ClaudeRunner.swift:28-51 never reads the
    /// field.
    pub images: Vec<PathBuf>,
}

/// Everything [`render_system_prompt`] substitutes into the template.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptCtx {
    pub project_name: String,
    /// The python line's display text: the verified versions line, or
    /// upstream's "build123d, matplotlib, numpy" fallback
    /// (AgentSession.swift:206).
    pub python_display: String,
    /// The interpreter path used in the restricted-bash allowlist and
    /// `{{PYTHON}}`.
    pub venv_python: PathBuf,
    /// The `{{ENVIRONMENT}}` bullets, from [`environment_bullets`].
    pub environment: Vec<String>,
    pub units: String,
    pub printer: String,
    pub guidelines: String,
    /// Numeric date without time (upstream `formatted(date: .numeric,
    /// time: .omitted)`).
    pub date: String,
}

/// The argv of one claude turn, in upstream order (ClaudeRunner.swift:28-51).
/// `_claude` is the executable the caller spawns alongside these arguments —
/// upstream hands `executable` and `arguments` to ProcessRunner separately,
/// so the argv itself never names it.
pub fn build_args(
    _claude: &AgentPath,
    prompt: &str,
    system_prompt: &str,
    s: &Settings,
    session_id: Option<&str>,
    venv_python: &Path,
) -> Vec<String> {
    let mut args = vec!["-p".to_string(), prompt.to_string()];
    if s.streaming_output {
        args.extend(
            [
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
            ]
            .map(String::from),
        );
    } else {
        args.extend(["--output-format", "json"].map(String::from));
    }
    args.extend(["--append-system-prompt", system_prompt].map(String::from));
    args.extend(["--permission-mode", "acceptEdits"].map(String::from));
    args.push("--allowedTools".to_string());
    args.extend(allowed_tools(s.restricted_bash, venv_python));
    if s.isolates_claude {
        args.push("--strict-mcp-config".to_string());
        args.extend(["--setting-sources", "project,local"].map(String::from));
    }
    if let Some(session_id) = session_id {
        args.extend(["--resume", session_id, "--fork-session"].map(String::from));
    }
    if let Some(model) = &s.claude_model {
        args.extend(["--model", model.as_str()].map(String::from));
    }
    if let Some(effort) = &s.claude_effort {
        args.extend(["--effort", effort.as_str()].map(String::from));
    }
    args
}

/// The `--allowedTools` values: normally one comma-joined argument; with
/// restricted bash the allowlist as 15 separate entries, the interpreter
/// first and then the read-only commands in upstream order
/// (ClaudeRunner.swift:57-65).
fn allowed_tools(restricted_bash: bool, venv_python: &Path) -> Vec<String> {
    if !restricted_bash {
        return vec!["Read,Write,Edit,Glob,Grep,Bash".to_string()];
    }
    let read_only_commands = [
        "ls", "cat", "head", "tail", "wc", "file", "test", "pwd", "stat",
    ];
    let mut tools: Vec<String> = ["Read", "Write", "Edit", "Glob", "Grep"]
        .map(String::from)
        .into_iter()
        .collect();
    tools.push(format!("Bash({}:*)", quoted_python(venv_python)));
    tools.extend(read_only_commands.map(|command| format!("Bash({command}:*)")));
    tools
}

/// The interpreter as it appears inside `Bash(...)` and `{{PYTHON}}`:
/// double-quoted when the path contains a space, the quotes living inside
/// the value (ClaudeRunner.swift:53-55).
fn quoted_python(python: &Path) -> String {
    let path = python.to_string_lossy();
    if path.contains(' ') {
        format!("\"{path}\"")
    } else {
        path.into_owned()
    }
}

/// Substitute the seven template placeholders in upstream replacement order
/// (ClaudeRunner.swift:67-79); the bullets join with newlines because the
/// template gives `{{ENVIRONMENT}}` a line of its own.
pub fn render_system_prompt(tpl: &str, ctx: &PromptCtx) -> String {
    tpl.replace("{{PYTHON}}", &quoted_python(&ctx.venv_python))
        .replace("{{PROJECT_NAME}}", &ctx.project_name)
        .replace("{{DATE}}", &ctx.date)
        .replace("{{UNITS}}", &ctx.units)
        .replace("{{PRINTER}}", &ctx.printer)
        .replace("{{PRINTING_GUIDELINES}}", &ctx.guidelines)
        .replace("{{ENVIRONMENT}}", &ctx.environment.join("\n"))
}

/// The `{{ENVIRONMENT}}` bullets: the python line, the model.py status, the
/// output/ listing and — only when references/ exists — its listing
/// (AgentSession.swift:197-217). The python line's display text comes from
/// the caller: verification state is process-global (the shared versions
/// cell), not derivable from the project root.
pub fn environment_bullets(root: &Path, python_display: &str) -> Vec<String> {
    let mut lines = vec![format!("- Python packages: {python_display}")];
    match std::fs::read_to_string(folder::model_script(root)) {
        // Swift's split(omittingEmptySubsequences: false) keeps the empty
        // tail, so a trailing newline still counts as a line
        Ok(script) => {
            lines.push(format!(
                "- model.py exists ({} lines)",
                script.split('\n').count()
            ));
        }
        Err(_) => lines.push("- model.py does not exist yet: create it".to_string()),
    }
    lines.push(format!("- output/: {}", listing(&folder::output_dir(root))));
    let references = folder::references_dir(root);
    if references.exists() {
        lines.push(format!("- references/: {}", listing(&references)));
    }
    lines
}

/// The non-hidden entry names of `dir`, sorted and comma-joined; "empty"
/// when the directory is absent, unreadable or holds nothing
/// (AgentSession.swift:200-205).
fn listing(dir: &Path) -> String {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| !name.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    if names.is_empty() {
        "empty".to_string()
    } else {
        names.join(", ")
    }
}

/// The turn prompt: the text — or the attachment fallback when the text is
/// empty — followed by the attachment list and the missing-output note,
/// each separated by a blank line exactly as upstream does
/// (Project.swift:245-256). The attachment list IS the request's
/// `references` — the copies the bridge made at turn start, threaded
/// straight through like upstream threads `copyReferences`' return
/// (Project.swift:203). `missing` reaches [`missing_requirements`]
/// verbatim.
pub fn build_prompt(req: &TurnRequest, missing: &MissingCtx) -> String {
    let mut prompt = if req.prompt.is_empty() && !req.references.is_empty() {
        "Create a model based on the attached files.".to_string()
    } else {
        req.prompt.clone()
    };
    if !req.references.is_empty() {
        prompt.push_str("\n\nAttached reference files:\n");
        prompt.push_str(&bullets(&req.references));
    }
    let missing = missing_requirements(missing);
    if !missing.is_empty() {
        prompt.push_str(
            "\n\nNote from CAD Studio: model.py does not produce these required outputs yet. \
             Add them in this turn as well, following your instructions:\n",
        );
        prompt.push_str(&bullets(&missing));
    }
    prompt
}

fn bullets(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}
