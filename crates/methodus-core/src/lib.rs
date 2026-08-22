//! Methodus core orchestration library — M1+ scope.
//! This crate has NO main() and NO UI. It is a pure library.

pub mod agent;
pub mod config;
pub mod engine;
pub mod error;
pub mod graph;
pub mod home;
pub mod learning;
pub mod lock;
pub mod mentions;

pub use config::UserConfig;
pub use agent::{
    index_revision, AgentDirectory, AgentItem, AgentManifest, AgentManifestItem, AgentQuery,
    AgentSearchResult, AGENT_PROTOCOL_VERSION,
};
pub use engine::{
    Engine, LearnEventRecord, LearnRun, NativeLearnHandoff, NativeLearnReturn, NativeUseHandoff,
    TeamStatus,
};
pub use error::CoreError;
pub use home::{ensure_home, health_checks, methodus_home, HealthCheck};
pub use graph::{estimated_tokens, facet, read_graph_document, sources_are_stale_now, sync_graph, validate_graph, GraphDocument, GraphIssue, IssueSeverity, SourceEvidence};
pub use lock::InstanceLock;
pub use mentions::{
    at_query, context_roots, filter_candidates, list_candidates, list_from_roots, MentionCandidate,
};
