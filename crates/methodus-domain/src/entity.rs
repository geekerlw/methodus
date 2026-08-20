//! Core domain entity structs for Methodus.
//!
//! These map directly to the SQLite schema in `docs/03-data-model.md` §3.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::status::{SessionStatus, TaskStatus};

/// Legacy task row retained only so old SQLite homes can be inspected/migrated.
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

/// Legacy execution row. Canonical Experience nodes are Markdown graph files.
///
/// The body lives in a Markdown file (`path`, relative to Methodus home);
/// SQLite holds the index row (`path` + `content_hash` + lifecycle).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Experience {
    pub id: String,
    pub task_id: String,
    pub path: String,
    pub content_hash: String,
    pub outcome: Option<String>, // success|partial|failed
    pub summary: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Legacy executor session row; active Learn state is runtime-owned.
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

/// Legacy permission approval row. Active runtime permissions remain runtime-owned.
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

/// A file-backed node in Methodus's Markdown-first knowledge graph.
///
/// `node_type` intentionally remains a string at this boundary so Markdown graph files
/// can add compatible node kinds without a database migration. Active built-ins are
/// knowledge, method, and experience; legacy rows may contain older values.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct GraphNode {
    pub id: String,
    pub node_type: String,
    pub title: String,
    pub path: String,
    pub content_hash: String,
    pub status: Option<String>,
    pub summary: Option<String>,
    pub scope: Option<String>,
    pub visibility: String,
    pub tags: Vec<String>,
    pub confidence: Option<f64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A typed, directed relationship between two graph nodes.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct GraphEdge {
    pub id: String,
    pub from_id: String,
    pub relation: String,
    pub to_id: String,
    pub source: String,
    pub confidence: Option<f64>,
    pub evidence_refs: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Legacy context-selection row retained for migration compatibility.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ContextSelection {
    pub id: String,
    pub workspace_id: String,
    pub node_id: String,
    pub facet: String,
    pub rationale: String,
    pub priority: Option<f64>,
    pub estimated_tokens: i64,
    pub disposition: String,
    pub outcome: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Legacy task-workspace metadata retained for migration compatibility.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TaskWorkspace {
    pub id: String,
    pub task_id: String,
    pub root_path: String,
    pub launch_cwd: String,
    pub status: String,
    pub manifest_hash: String,
    pub context_budget_tokens: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
