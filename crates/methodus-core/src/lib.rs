//! Methodus core orchestration library — M1+ scope.
//! This crate has NO main() and NO UI. It is a pure library.

pub mod config;
pub mod engine;
pub mod error;
pub mod home;
pub mod learning;
pub mod lock;
pub mod mentions;
pub mod pack;
pub mod policy;
pub mod project;
pub mod resolution;
pub mod scheduler;
pub mod workspace;

pub use config::UserConfig;
pub use engine::{Engine, RecoveredSession};
pub use error::CoreError;
pub use home::{ensure_home, health_checks, methodus_home, HealthCheck};
pub use lock::InstanceLock;
pub use mentions::{
    at_query, context_roots, filter_candidates, list_candidates, list_from_roots, MentionCandidate,
};
pub use pack::{list_packs, PackInfo};
pub use policy::{PermissionMode, PolicyVerdict};
pub use project::{list_projects, ProjectInfo};
pub use resolution::{list_faces, FaceSummary, Resolution};
