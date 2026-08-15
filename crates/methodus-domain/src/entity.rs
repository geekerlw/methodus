//! Core domain entity structs for Methodus.
//!
//! These map directly to the SQLite schema in `docs/design/03-data-model.md` §3.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::status::{
    JobKind, JobStatus, KnowledgeStatus, QuestionStatus, SessionStatus, TaskStatus,
};

/// A user-submitted task that Methodus orchestrates through an executor.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub request: String,
    pub project_id: Option<String>,
    pub status: TaskStatus,
    pub runtime: Option<String>,
    pub workspace_id: Option<String>,
    pub resolution: Option<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A structured record of one task execution.
///
/// The body lives in a Markdown file (`path`, relative to Methodus home);
/// SQLite holds the index row (`path` + `content_hash` + lifecycle).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Experience {
    pub id: String,
    pub task_id: String,
    pub face_id: Option<String>,
    pub path: String,
    pub content_hash: String,
    pub outcome: Option<String>, // success|partial|failed
    pub summary: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// An executor session spawned for a task.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Session {
    pub id: String,
    pub task_id: String,
    pub runtime: String,
    pub executor_sid: Option<String>,
    pub transport: String,
    pub pid: Option<u32>,
    pub cwd: String,
    pub status: SessionStatus,
    pub last_turn: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// A pending or resolved permission approval (body in SQLite; see `03-data-model.md`).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Approval {
    pub id: String,
    pub session_id: String,
    pub task_id: String,
    pub subject: String,
    pub tool_name: String,
    pub tool_use_id: Option<String>,
    pub tool_input: String,
    pub decision: Option<String>,
    pub actor: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// Indexed knowledge item. Body lives at `path` under Methodus home.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct KnowledgeItem {
    pub id: String,
    pub face_id: Option<String>,
    pub project_id: Option<String>,
    pub path: String,
    pub content_hash: String,
    pub source: String, // experience|user_answer|doc|research
    pub confidence: Option<f64>,
    pub scope: Option<String>,
    pub status: KnowledgeStatus,
    pub conflict_of: Option<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Proactive question raised by the curiosity loop.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Question {
    pub id: String,
    pub question: String,
    pub reason: Option<String>,
    pub task_id: Option<String>,
    pub face_id: Option<String>,
    pub importance: f64,
    pub frequency: f64,
    pub impact: f64,
    pub uncertainty: f64,
    pub value: f64,
    pub status: QuestionStatus,
    pub not_before: Option<DateTime<Utc>>,
    pub answer: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Budgeted, retryable, cancelable learning-queue job.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LearningJob {
    pub id: String,
    pub kind: JobKind,
    pub priority: i64,
    pub dedupe_key: Option<String>,
    pub input_refs: String, // JSON
    pub status: JobStatus,
    pub attempts: i64,
    pub not_before: Option<DateTime<Utc>>,
    pub budget: Option<String>,
    pub requires_approval: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
