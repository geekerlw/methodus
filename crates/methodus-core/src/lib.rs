//! Methodus core orchestration library — M1+ scope.
//! This crate has NO main() and NO UI. It is a pure library.

pub mod catalog;
pub mod config;
pub mod curiosity;
pub mod engine;
pub mod error;
pub mod face_util;
pub mod hypothesis;
pub mod ingest;
pub mod evolution;
pub mod learn;
pub mod home;
pub mod learning;
pub mod lock;
pub mod mentions;
pub mod multi_face;
pub mod pack;
pub mod policy;
pub mod project;
pub mod refine;
pub mod resolution;
pub mod scheduler;
pub mod workspace;

pub use config::UserConfig;
pub use curiosity::MODULE_EXPERT_METHOD_ID;
pub use engine::{Engine, KnowledgeReviewAction, RecoveredSession};
pub use hypothesis::HypothesisReviewAction;
pub use error::CoreError;
pub use home::{ensure_home, health_checks, methodus_home, HealthCheck};
pub use lock::InstanceLock;
pub use multi_face::parse_face_pin;
pub use mentions::{
    at_query, context_roots, filter_candidates, list_candidates, list_from_roots, MentionCandidate,
};
pub use pack::{list_packs, PackInfo};
pub use policy::{PermissionMode, PolicyVerdict};
pub use project::{list_projects, ProjectInfo};
pub use resolution::{list_faces, FaceSummary, Resolution};
