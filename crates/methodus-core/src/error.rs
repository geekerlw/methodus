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

    #[error("message is empty")]
    EmptyMessage,

    #[error("session still running (pid {0}); wait for it to finish, or cancel it in the TUI")]
    SessionLive(u32),

    #[error("session was interrupted; resume it from the TUI (R)")]
    NeedsResume,

    #[error("nothing to resume for task {0} (no stored executor session id)")]
    NothingToResume(String),

    #[error("another Methodus instance holds the lock at {0}")]
    Locked(PathBuf),

    #[error("face not found: {0}")]
    FaceNotFound(String),

    #[error("unknown runtime: {0}")]
    UnknownRuntime(String),

    #[error("approval not found: {0}")]
    ApprovalNotFound(String),

    #[error("approval {0} is already resolved")]
    ApprovalResolved(String),

    #[error("task {0} is waiting for approval {1}; approve it in the TUI")]
    NeedsApproval(String, String),

    #[error("task {0} is {1} and cannot be cancelled")]
    TaskNotCancellable(String, String),

    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[error("session {0} is {1} and cannot be cancelled")]
    SessionNotCancellable(String, String),

    #[error("knowledge not found: {0}")]
    KnowledgeNotFound(String),

    #[error("question not found: {0}")]
    QuestionNotFound(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("pack not found: {0}")]
    PackNotFound(String),

    #[error("invalid pack at {0}: {1}")]
    InvalidPack(PathBuf, String),

    #[error("project not found: {0}")]
    ProjectNotFound(String),

    #[error("invalid project at {0}: {1}")]
    InvalidProject(PathBuf, String),

    #[error("{0}")]
    Other(String),
}
