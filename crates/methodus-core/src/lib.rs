//! Methodus core orchestration library — M1+ scope.
//! This crate has NO main() and NO UI. It is a pure library.

pub mod engine;
pub mod error;
pub mod lock;
pub mod policy;
pub mod resolution;
pub mod workspace;

pub use engine::{Engine, RecoveredSession};
pub use error::CoreError;
pub use lock::InstanceLock;
pub use resolution::Resolution;
