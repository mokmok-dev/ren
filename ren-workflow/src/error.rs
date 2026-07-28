use thiserror::Error;

use crate::host::Capability;

/// An infrastructure failure reported by a workflow host.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct HostError {
    message: String,
}

impl HostError {
    /// Creates a host infrastructure error.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// An error produced while compiling or running a workflow.
#[derive(Debug, Error)]
pub enum WorkflowError {
    /// The CLI or run configuration is incomplete or invalid.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    /// The configured agent budget is outside the supported range.
    #[error("agent budget must be between 1 and 1024, got {0}")]
    InvalidBudget(usize),
    /// The workflow metadata declaration is invalid.
    #[error("invalid workflow metadata: {0}")]
    InvalidMeta(String),
    /// Rhai rejected the workflow while compiling it.
    #[error("workflow compilation failed: {0}")]
    Compile(String),
    /// Rhai failed while evaluating the workflow.
    #[error("workflow execution failed: {0}")]
    Runtime(String),
    /// An agent requested more capability than the host grants.
    #[error("requested capability `{requested}` exceeds host-granted `{granted}`")]
    CapabilityDenied {
        /// Capability requested by the agent invocation.
        requested: Capability,
        /// Maximum capability granted by the host.
        granted: Capability,
    },
    /// An agent requested an unrecognized capability mode.
    #[error("invalid capability_mode `{0}`; expected read-only, read-write, execute, or all")]
    InvalidCapabilityMode(String),
    /// The supplied journal does not match the current execution.
    #[error("journal replay diverged: {0}")]
    JournalDivergence(String),
    /// A JSON value could not be represented by Rhai or vice versa.
    #[error("unsupported workflow value: {0}")]
    Value(String),
    /// JSON parsing or serialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// A requested user-store workflow is absent.
    #[error("workflow `{name}` is not present in the user store at {}", path.display())]
    WorkflowNotFound {
        /// Requested workflow name.
        name: String,
        /// Expected user-store path.
        path: std::path::PathBuf,
    },
    /// A workflow name contains unsupported characters.
    #[error("invalid workflow name `{0}` (allowed: lowercase letters, digits, and hyphens)")]
    InvalidWorkflowName(String),
    /// `$HOME` is required for a user-global operation.
    #[error("HOME is not set; cannot locate the user workflow store")]
    HomeUnavailable,
    /// Creating a workflow would replace an existing path.
    #[error("workflow already exists at {}; use --force to replace it", .0.display())]
    WorkflowExists(std::path::PathBuf),
    /// A requested official workflow is not bundled in this binary.
    #[error("no bundled workflow named `{0}`")]
    BundledWorkflowNotFound(String),
    /// An official workflow did not contain its declared name in rewritable form.
    #[error("could not rewrite meta.name in bundled workflow `{bundled}`")]
    BundledNameRewrite {
        /// Name of the inconsistent bundled workflow.
        bundled: String,
    },
    /// A dispatcher already exists and overwrite was not requested.
    #[error("bridge dispatcher already exists at {}; use --force to replace it", .0.display())]
    BridgeExists(std::path::PathBuf),
    /// A skill file already exists and overwrite was not requested.
    #[error("skill file already exists at {}; use --force to replace it", .0.display())]
    SkillExists(std::path::PathBuf),
    /// Reading or writing workflow state failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
