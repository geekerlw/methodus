//! Core domain entity structs for Methodus.
//!
//! These map directly to the SQLite schema in `docs/03-data-model.md` §3.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::status::{
    EvolutionStatus, HypothesisStatus, JobKind, JobStatus, KnowledgeStatus, QuestionStatus,
    SessionStatus, TaskStatus,
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

/// Indexed hypothesis. Body lives at `path` under Methodus home.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Hypothesis {
    pub id: String,
    pub face_id: Option<String>,
    pub path: String,
    pub content_hash: String,
    pub confidence: Option<f64>,
    pub status: HypothesisStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Proposed upgrade to a Face, Method, Skill, or Knowledge entry (`00-product.md` §3.10).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EvolutionCandidate {
    pub id: String,
    pub target_kind: String, // face|method|skill|knowledge
    pub target_id: String,
    pub diff: String, // JSON payload
    pub rationale: Option<String>,
    pub source: Option<String>,
    pub status: EvolutionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A file-backed node in Methodus's Markdown-first knowledge graph.
///
/// `node_type` intentionally remains a string at this boundary so graph packs can add
/// compatible node kinds without a database migration. Built-ins include knowledge,
/// experience, artifact, face, method, and skill.
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

/// A selected graph fragment and the decision that put it into (or behind) a task capsule.
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

/// The durable metadata for an immutable task context capsule.
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
