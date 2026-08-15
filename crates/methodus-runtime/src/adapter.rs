use std::path::PathBuf;

use methodus_domain::RuntimeEvent;
use tokio::sync::mpsc;

/// Input parameters for spawning or resuming an executor session.
#[derive(Debug, Clone)]
pub struct SpawnInput {
    pub prompt: String,
    pub cwd: PathBuf,
    /// Methodus-side session id. Also passed as Claude `--session-id` on a fresh spawn.
    pub session_id: String,
    /// Permission mode — e.g. "manual" for M2 Claude approval, "acceptEdits" for tests.
    pub permission_mode: String,
    /// Claude `--allowed-tools` entries (e.g. "Write", "Bash"). Ignored by Codex.
    pub allowed_tools: Vec<String>,
    /// Codex `--sandbox` value (`read-only` | `workspace-write` | `danger-full-access`).
    pub sandbox: Option<String>,
    /// Additional directories the executor may read in place (`claude --add-dir`).
    /// Launch cwd ∪ registered projects; source is not copied into `cwd`.
    pub extra_dirs: Vec<PathBuf>,
    pub model: Option<String>,
}

/// Handle to a running executor session.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    /// Methodus session id.
    pub session_id: String,
    /// Executor-issued recovery key (Claude session uuid / Codex thread_id).
    pub executor_sid: Option<String>,
    pub pid: Option<u32>,
}

/// A live executor-side agent, as reported by the executor's own listing API.
#[derive(Debug, Clone)]
pub struct LiveAgent {
    pub id: String,
    pub session_id: String,
    pub pid: Option<u32>,
    pub status: String,
}

/// Trait implemented by each executor backend (Claude Code, Codex, Cursor).
#[async_trait::async_trait]
pub trait RuntimeAdapter: Send + Sync {
    /// Spawn an executor session and return a handle + event stream.
    async fn spawn(
        &self,
        input: SpawnInput,
    ) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError>;

    /// Continue an existing executor session with a new user turn (`--resume`).
    async fn resume(
        &self,
        executor_sid: &str,
        input: SpawnInput,
    ) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError>;

    /// Stop a running session (send SIGTERM or equivalent).
    async fn stop(&self, handle: &SessionHandle) -> Result<(), RuntimeError>;

    /// List agents still alive on the executor side (Claude: `agents --json`).
    /// Default: none — adapters that have no listing API leave this empty.
    async fn list_live_agents(&self) -> Result<Vec<LiveAgent>, RuntimeError> {
        Ok(Vec::new())
    }

    /// Whether this adapter surfaces structured permission denials (Claude `manual`).
    fn uses_manual_permissions(&self) -> bool {
        false
    }
}

/// Errors that can occur during adapter operations.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("spawn failed: {0}")]
    SpawnFailed(String),
    #[error("executor not found: {0}")]
    NotFound(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(String),
}
