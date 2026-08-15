//! Core domain entity structs for Methodus.
//!
//! These map directly to the SQLite schema in `docs/design/03-data-model.md` §3.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::status::{SessionStatus, TaskStatus};

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
