use std::{
    fs,
    path::{Path, PathBuf},
};

use clap::ValueEnum;

use crate::WorkflowError;

pub const DISPATCHER_CONTENT: &str = "# Gyre workflow dispatcher\n\nExecute `ren workflow run $ARGUMENTS` in the shell, where `$ARGUMENTS` is the complete user-provided argument string. Return the command output.\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Agent {
    Claude,
    Cursor,
    Codex,
    Grok,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeScope {
    Global,
    Project,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeDefinition {
    pub path: PathBuf,
    pub contents: &'static str,
}

#[derive(Clone, Copy)]
struct AgentSpec {
    global_path: &'static str,
    project_path: &'static str,
}

const fn agent_spec(agent: Agent) -> AgentSpec {
    match agent {
        Agent::Claude => AgentSpec {
            global_path: ".claude/commands/ren.md",
            project_path: ".claude/commands/ren.md",
        },
        Agent::Cursor => AgentSpec {
            global_path: ".cursor/commands/ren.md",
            project_path: ".cursor/commands/ren.md",
        },
        Agent::Codex => AgentSpec {
            global_path: ".codex/prompts/ren.md",
            project_path: ".codex/prompts/ren.md",
        },
        Agent::Grok => AgentSpec {
            global_path: ".grok/commands/ren.md",
            project_path: ".grok/commands/ren.md",
        },
    }
}

#[must_use]
pub fn bridge_definition(
    base_dir: &Path,
    agent: Agent,
    scope: BridgeScope,
) -> BridgeDefinition {
    let spec = agent_spec(agent);
    let relative = match scope {
        BridgeScope::Global => spec.global_path,
        BridgeScope::Project => spec.project_path,
    };
    BridgeDefinition {
        path: base_dir.join(relative),
        contents: DISPATCHER_CONTENT,
    }
}

pub fn install_bridge(
    definition: &BridgeDefinition,
    force: bool,
) -> Result<(), WorkflowError> {
    if definition.path.exists() && !force {
        return Err(WorkflowError::BridgeExists(definition.path.clone()));
    }
    if let Some(parent) = definition.path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&definition.path, definition.contents)?;
    Ok(())
}

pub fn uninstall_bridge(definition: &BridgeDefinition) -> Result<bool, WorkflowError> {
    if !definition.path.exists() {
        return Ok(false);
    }
    fs::remove_file(&definition.path)?;
    Ok(true)
}
