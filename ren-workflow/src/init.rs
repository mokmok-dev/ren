use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Write as _},
    path::{Path, PathBuf},
};

use crate::{WorkflowError, bridge::Agent};

/// Directory name every skill-capable agent uses under its config root.
const SKILL_ROOT: &str = "skills";
/// Folder name for this specific skill.
const SKILL_NAME: &str = "ren-workflow";

/// The embedded skill entrypoint, following the open Agent Skills standard.
///
/// This is a thin bootstrap: it points agents at the binary, whose `--help` and
/// injected `agent_protocol` are the version-matched source of truth. Rich,
/// version-sensitive guidance lives in [`crate::guide`], not in this file.
pub const SKILL_MD: &str = include_str!("../assets/skill/SKILL.md");

/// A single embedded skill file and its path relative to the skill folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkillFile {
    /// Path relative to the skill folder, using `/` separators.
    pub relative: &'static str,
    /// File contents.
    pub contents: &'static str,
}

/// Every file that makes up the embedded skill.
pub const SKILL_FILES: &[SkillFile] = &[SkillFile {
    relative: "SKILL.md",
    contents: SKILL_MD,
}];

/// Whether a skill is installed globally (user scope) or in a project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitScope {
    /// The agent's user-global config directory (the default).
    User,
    /// The current repository's config directory.
    Project,
}

/// The resolved plan for installing the skill for one agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillDefinition {
    /// The `<agent>/skills/ren-workflow` folder that receives the skill.
    pub dir: PathBuf,
    /// Absolute paths and contents of every file to write.
    pub files: Vec<(PathBuf, &'static str)>,
}

/// Returns the agent config directory name (e.g. `.grok`) for `agent`.
const fn agent_config_dir(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => ".claude",
        Agent::Cursor => ".cursor",
        Agent::Codex => ".codex",
        Agent::Grok => ".grok",
    }
}

/// Every agent the skill installer supports.
#[must_use]
pub const fn supported_agents() -> [Agent; 4] {
    [Agent::Claude, Agent::Cursor, Agent::Codex, Agent::Grok]
}

/// Builds the install plan for one agent rooted at `base_dir`.
///
/// `base_dir` is the user's home directory for [`InitScope::User`] or the
/// repository root for [`InitScope::Project`]; both resolve to the same
/// `<agent>/skills/ren-workflow` layout.
#[must_use]
pub fn skill_definition(
    base_dir: &Path,
    agent: Agent,
) -> SkillDefinition {
    let dir = base_dir
        .join(agent_config_dir(agent))
        .join(SKILL_ROOT)
        .join(SKILL_NAME);
    let files = SKILL_FILES
        .iter()
        .map(|file| (join_relative(&dir, file.relative), file.contents))
        .collect();
    SkillDefinition { dir, files }
}

/// Joins a `/`-separated relative path onto `base` in a platform-correct way.
fn join_relative(
    base: &Path,
    relative: &str,
) -> PathBuf {
    let mut path = base.to_path_buf();
    for segment in relative.split('/') {
        path.push(segment);
    }
    path
}

/// Writes every file in `definition`, creating parent directories as needed.
///
/// # Errors
///
/// Returns [`WorkflowError::SkillExists`] when a target file already exists and
/// `force` is false, or [`WorkflowError::Io`] when a filesystem operation fails.
pub fn install_skill(
    definition: &SkillDefinition,
    force: bool,
) -> Result<(), WorkflowError> {
    for (path, contents) in &definition.files {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if force {
            fs::write(path, contents)?;
            continue;
        }
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => file.write_all(contents.as_bytes())?,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                return Err(WorkflowError::SkillExists(path.clone()));
            },
            Err(error) => return Err(WorkflowError::Io(error)),
        }
    }
    Ok(())
}
