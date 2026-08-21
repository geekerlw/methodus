//! `methodus-domain` — Pure domain types, enums, state machines, and events.

pub mod approval;
pub mod entity;
pub mod error;
pub mod event;
pub mod learning;
pub mod status;
pub mod usage;

// Re-export key types at crate root for convenience.
pub use approval::ApprovalDecision;
pub use entity::{
    Approval, ContextSelection, Experience, GraphEdge, GraphNode, Session, Task, TaskWorkspace,
};
pub use error::DomainError;
pub use learning::{
    usage_month, AttentionKind, AttentionStatus, Cadence, GoalRun, GoalUsage, HumanAttention,
    LearningGoal, QuietHours, ReviewPolicy, WorkKind,
};
pub use event::{PermissionDenial, RuntimeEvent};
pub use status::{
    EvolutionStatus, HypothesisStatus, JobKind, JobStatus, KnowledgeStatus, QuestionStatus,
    SessionStatus, TaskStatus,
};
pub use usage::{UsageDelta, UsageSummary};
