//! Orchestration errors. Typed so the binary can print a useful message.

use std::path::PathBuf;

use methodus_domain::DomainError;
use methodus_runtime::RuntimeError;
use methodus_store::StoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("{0}")]
    Store(#[from] StoreError),

    #[error("{0}")]
    Runtime(#[from] RuntimeError),

    #[error("{0}")]
    Domain(#[from] DomainError),

    #[error("task not found: {0}")]
    TaskNotFound(String),

    #[error("unsafe task id: {0}")]
    InvalidTaskId(String),

    #[error("task {0} is {1} and cannot be run")]
    TaskNotRunnable(String, String),

    #[error("session still running (pid {0}); wait for it to finish, or kill it and run `methodus recover`")]
    SessionLive(u32),

    #[error("session was interrupted; re-run with --resume to continue the executor session")]
    NeedsResume,

    #[error("nothing to resume for task {0} (no stored executor session id)")]
    NothingToResume(String),

    #[error("another Methodus instance holds the lock at {0}; read-only commands (task list, task show) still work")]
    Locked(PathBuf),

    #[error("face not found: {0}")]
    FaceNotFound(String),

    #[error("unknown runtime: {0}")]
    UnknownRuntime(String),

    #[error("approval not found: {0}")]
    ApprovalNotFound(String),

    #[error("approval {0} is already resolved")]
    ApprovalResolved(String),

    #[error("task {0} is waiting for approval {1}; run `methodus approve {1} --decision once|session|deny|abort`")]
    NeedsApproval(String, String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}
