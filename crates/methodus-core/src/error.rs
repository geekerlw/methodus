//! Errors exposed by the active Methodus core.

use std::path::PathBuf;

use methodus_runtime::RuntimeError;
use methodus_store::StoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("runtime: {0}")]
    Runtime(#[from] RuntimeError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("another Methodus instance holds the lock at {0}")]
    Locked(PathBuf),
    #[error("unknown runtime: {0}")]
    UnknownRuntime(String),
    #[error("{0}")]
    Other(String),
}
