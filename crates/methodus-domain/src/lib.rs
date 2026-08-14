//! `methodus-domain` — Pure domain types, enums, state machines, and events.

pub mod entity;
pub mod event;
pub mod status;

// Re-export key types at crate root for convenience.
pub use entity::{Experience, Session, Task};
pub use event::RuntimeEvent;
pub use status::{JobStatus, KnowledgeStatus, QuestionStatus, SessionStatus, TaskStatus};
