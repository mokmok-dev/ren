use std::{
    fs::{self, OpenOptions},
    io::{self, ErrorKind, Write as _},
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

/// An embedded skill that can be installed into every supported agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmbeddedSkill {
    /// Folder name under the agent's skill root.
    pub name: &'static str,
    /// Files to install relative to the skill folder.
    pub files: &'static [SkillFile],
}

/// Every file that makes up the embedded skill.
pub const SKILL_FILES: &[SkillFile] = &[SkillFile {
    relative: "SKILL.md",
    contents: SKILL_MD,
}];

/// The embedded `ren-workflow` skill.
pub const WORKFLOW_SKILL: EmbeddedSkill = EmbeddedSkill {
    name: SKILL_NAME,
    files: SKILL_FILES,
};

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
    /// The `<agent>/skills/<skill-name>` folder that receives the skill.
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
    skill_definition_for(base_dir, agent, WORKFLOW_SKILL)
}

/// Builds the install plan for any embedded `skill` rooted at `base_dir`.
#[must_use]
pub fn skill_definition_for(
    base_dir: &Path,
    agent: Agent,
    skill: EmbeddedSkill,
) -> SkillDefinition {
    let dir = base_dir
        .join(agent_config_dir(agent))
        .join(SKILL_ROOT)
        .join(skill.name);
    let files = skill
        .files
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
/// Existing byte-identical files are left unchanged, making repeated installs
/// idempotent. All files are checked before the first write so a later conflict
/// cannot leave a partially installed skill.
///
/// # Errors
///
/// Returns [`WorkflowError::SkillExists`] when a target file has different
/// contents and `force` is false, or [`WorkflowError::Io`] when a filesystem
/// operation fails.
pub fn install_skill(
    definition: &SkillDefinition,
    force: bool,
) -> Result<(), WorkflowError> {
    install_skills(std::slice::from_ref(definition), force)
}

/// Writes every file in `definitions` after preflighting the complete batch.
///
/// This is used by top-level initialization so conflicts in later agents,
/// skills, or files are reported before any earlier target is written.
///
/// # Errors
///
/// Returns the same installation errors as [`install_skill`].
pub fn install_skills(
    definitions: &[SkillDefinition],
    force: bool,
) -> Result<(), WorkflowError> {
    let pending = preflight_install(definitions, force)?;
    let mut completed = Vec::new();
    for file in &pending {
        if let Some(parent) = file.path.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            return Err(rollback_after(error, &completed));
        }

        if file.previous.is_some() {
            completed.push(file);
            if let Err(error) = fs::write(&file.path, file.contents) {
                return Err(rollback_after(error, &completed));
            }
            continue;
        }

        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file.path)
        {
            Ok(mut target) => {
                completed.push(file);
                if let Err(error) = target.write_all(file.contents.as_bytes()) {
                    return Err(rollback_after(error, &completed));
                }
            },
            Err(error) => return Err(rollback_after(error, &completed)),
        }
    }
    Ok(())
}

#[derive(Debug)]
struct PendingSkillFile {
    path: PathBuf,
    contents: &'static str,
    previous: Option<Vec<u8>>,
}

/// Returns only the files that need writing after checking every target.
fn preflight_install(
    definitions: &[SkillDefinition],
    force: bool,
) -> Result<Vec<PendingSkillFile>, WorkflowError> {
    let mut pending = Vec::new();
    for definition in definitions {
        for (path, contents) in &definition.files {
            match fs::read(path) {
                Ok(installed) if installed == contents.as_bytes() => {},
                Ok(_) if !force => return Err(WorkflowError::SkillExists(path.clone())),
                Ok(installed) => pending.push(PendingSkillFile {
                    path: path.clone(),
                    contents,
                    previous: Some(installed),
                }),
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    pending.push(PendingSkillFile {
                        path: path.clone(),
                        contents,
                        previous: None,
                    });
                },
                Err(error) => return Err(WorkflowError::Io(error)),
            }
        }
    }
    Ok(pending)
}

/// Restores all targets touched by the failed batch in reverse order.
fn rollback_after(
    install_error: io::Error,
    completed: &[&PendingSkillFile],
) -> WorkflowError {
    let mut rollback_error = None;
    for file in completed.iter().rev() {
        let result = file.previous.as_ref().map_or_else(
            || {
                fs::remove_file(&file.path).or_else(|error| {
                    if error.kind() == ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })
            },
            |contents| fs::write(&file.path, contents),
        );
        if let Err(error) = result
            && rollback_error.is_none()
        {
            rollback_error = Some(error);
        }
    }

    match rollback_error {
        None => WorkflowError::Io(install_error),
        Some(rollback_error) => WorkflowError::Io(io::Error::new(
            install_error.kind(),
            format!(
                "skill installation failed: {install_error}; rollback also failed: \
                     {rollback_error}"
            ),
        )),
    }
}
