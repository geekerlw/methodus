use std::path::PathBuf;

use methodus_domain::RuntimeEvent;
use tokio::sync::mpsc;

/// Input parameters for spawning an executor session.
#[derive(Debug, Clone)]
pub struct SpawnInput {
    pub prompt: String,
    pub cwd: PathBuf,
    pub session_id: String,
    /// Permission mode — e.g. "acceptEdits" for M1.
    pub permission_mode: String,
    pub model: Option<String>,
}

/// Handle to a running executor session.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub session_id: String,
    pub pid: Option<u32>,
}

/// Trait implemented by each executor backend (Claude Code, Codex, Cursor).
#[async_trait::async_trait]
pub trait RuntimeAdapter: Send + Sync {
    /// Spawn an executor session and return a handle + event stream.
    async fn spawn(
        &self,
        input: SpawnInput,
    ) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError>;

    /// Stop a running session (send SIGTERM or equivalent).
    async fn stop(&self, handle: &SessionHandle) -> Result<(), RuntimeError>;
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
