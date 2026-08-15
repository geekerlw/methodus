//! `methodus-domain` — Pure domain types, enums, state machines, and events.

pub mod approval;
pub mod entity;
pub mod error;
pub mod event;
pub mod status;

// Re-export key types at crate root for convenience.
pub use approval::ApprovalDecision;
pub use entity::{Approval, Experience, Session, Task};
pub use error::DomainError;
pub use event::{PermissionDenial, RuntimeEvent};
pub use status::{JobStatus, KnowledgeStatus, QuestionStatus, SessionStatus, TaskStatus};
