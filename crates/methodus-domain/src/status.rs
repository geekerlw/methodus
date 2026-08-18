//! Status enums and state-machine transition logic for Methodus domain entities.
//!
//! Each enum defines valid transitions per the state machines documented in
//! `docs/design/03-data-model.md` §4.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;

// ─── TaskStatus ──────────────────────────────────────────────────────────────

/// Lifecycle status of a Task.
///
/// State machine:
/// `queued → planning → running → {waiting_user, reviewing} → {completed, failed, cancelled}`
///
/// `reviewing → running` is a follow-up chat turn on the same task.
/// Additionally, `cancelled` is reachable from any non-terminal state.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Planning,
    Running,
    WaitingUser,
    Reviewing,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    /// Returns the list of states this status can legally transition to.
    pub fn transitions(&self) -> Vec<Self> {
        match self {
            Self::Queued => vec![Self::Planning, Self::Cancelled],
            Self::Planning => vec![Self::Running, Self::Failed, Self::Cancelled],
            Self::Running => vec![
                Self::WaitingUser,
                Self::Reviewing,
                Self::Completed,
                Self::Failed,
                Self::Cancelled,
            ],
            Self::WaitingUser => vec![Self::Running, Self::Cancelled],
            Self::Reviewing => vec![
                Self::Running,
                Self::Completed,
                Self::Failed,
                Self::Cancelled,
            ],
            // Terminal states
            Self::Completed => vec![],
            Self::Failed => vec![],
            Self::Cancelled => vec![],
        }
    }

    /// Whether a transition from `self` to `next` is valid.
    pub fn can_transition_to(&self, next: &Self) -> bool {
        self.transitions().contains(next)
    }

    /// Transition to `next`, or return an error if the edge is illegal.
    pub fn checked_transition(&self, next: Self) -> Result<Self, DomainError> {
        if self.can_transition_to(&next) {
            Ok(next)
        } else {
            Err(DomainError::InvalidTransition {
                entity: "task",
                from: self.to_string(),
                to: next.to_string(),
            })
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.transitions().is_empty()
    }
}

impl FromStr for TaskStatus {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(Self::Queued),
            "planning" => Ok(Self::Planning),
            "running" => Ok(Self::Running),
            "waiting_user" => Ok(Self::WaitingUser),
            "reviewing" => Ok(Self::Reviewing),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(DomainError::InvalidStatus {
                entity: "task",
                value: other.to_string(),
            }),
        }
    }
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Queued => "queued",
            Self::Planning => "planning",
            Self::Running => "running",
            Self::WaitingUser => "waiting_user",
            Self::Reviewing => "reviewing",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        };
        write!(f, "{}", s)
    }
}

// ─── SessionStatus ───────────────────────────────────────────────────────────

/// Lifecycle status of an executor Session.
///
/// State machine:
/// `spawning → running → {waiting_user, paused} → {exited, interrupted, failed}`
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Spawning,
    Running,
    WaitingUser,
    Paused,
    Exited,
    Interrupted,
    Failed,
}

impl SessionStatus {
    /// Returns the list of states this status can legally transition to.
    pub fn transitions(&self) -> Vec<Self> {
        match self {
            Self::Spawning => vec![Self::Running, Self::Failed],
            Self::Running => vec![
                Self::WaitingUser,
                Self::Paused,
                Self::Exited,
                Self::Interrupted,
                Self::Failed,
            ],
            Self::WaitingUser => vec![Self::Running, Self::Interrupted, Self::Failed],
            Self::Paused => vec![Self::Running, Self::Exited, Self::Interrupted, Self::Failed],
            // Terminal states
            Self::Exited => vec![],
            Self::Interrupted => vec![],
            Self::Failed => vec![],
        }
    }

    /// Whether a transition from `self` to `next` is valid.
    pub fn can_transition_to(&self, next: &Self) -> bool {
        self.transitions().contains(next)
    }

    /// Transition to `next`, or return an error if the edge is illegal.
    pub fn checked_transition(&self, next: Self) -> Result<Self, DomainError> {
        if self.can_transition_to(&next) {
            Ok(next)
        } else {
            Err(DomainError::InvalidTransition {
                entity: "session",
                from: self.to_string(),
                to: next.to_string(),
            })
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.transitions().is_empty()
    }
}

impl FromStr for SessionStatus {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "spawning" => Ok(Self::Spawning),
            "running" => Ok(Self::Running),
            "waiting_user" => Ok(Self::WaitingUser),
            "paused" => Ok(Self::Paused),
            "exited" => Ok(Self::Exited),
            "interrupted" => Ok(Self::Interrupted),
            "failed" => Ok(Self::Failed),
            other => Err(DomainError::InvalidStatus {
                entity: "session",
                value: other.to_string(),
            }),
        }
    }
}

impl fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Spawning => "spawning",
            Self::Running => "running",
            Self::WaitingUser => "waiting_user",
            Self::Paused => "paused",
            Self::Exited => "exited",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
        };
        write!(f, "{}", s)
    }
}

// ─── KnowledgeStatus ─────────────────────────────────────────────────────────

/// Lifecycle status of a knowledge item.
///
/// State machine: `candidate → {committed, conflicted, rejected}`
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeStatus {
    Candidate,
    Committed,
    Conflicted,
    Rejected,
}

impl KnowledgeStatus {
    /// Returns the list of states this status can legally transition to.
    pub fn transitions(&self) -> Vec<Self> {
        match self {
            Self::Candidate => vec![Self::Committed, Self::Conflicted, Self::Rejected],
            Self::Conflicted => vec![Self::Committed, Self::Rejected],
            // Terminal states
            Self::Committed => vec![],
            Self::Rejected => vec![],
        }
    }

    /// Whether a transition from `self` to `next` is valid.
    pub fn can_transition_to(&self, next: &Self) -> bool {
        self.transitions().contains(next)
    }

    pub fn checked_transition(&self, next: Self) -> Result<Self, DomainError> {
        if self.can_transition_to(&next) {
            Ok(next)
        } else {
            Err(DomainError::InvalidTransition {
                entity: "knowledge",
                from: self.to_string(),
                to: next.to_string(),
            })
        }
    }
}

impl FromStr for KnowledgeStatus {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "candidate" => Ok(Self::Candidate),
            "committed" => Ok(Self::Committed),
            "conflicted" => Ok(Self::Conflicted),
            "rejected" => Ok(Self::Rejected),
            other => Err(DomainError::InvalidStatus {
                entity: "knowledge",
                value: other.to_string(),
            }),
        }
    }
}

impl fmt::Display for KnowledgeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Candidate => "candidate",
            Self::Committed => "committed",
            Self::Conflicted => "conflicted",
            Self::Rejected => "rejected",
        };
        write!(f, "{}", s)
    }
}

// ─── QuestionStatus ──────────────────────────────────────────────────────────

/// Lifecycle status of a proactive question.
///
/// State machine: `pending → {asked → answered, snoozed, dismissed}`
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuestionStatus {
    Pending,
    Asked,
    Answered,
    Snoozed,
    Dismissed,
}

impl QuestionStatus {
    /// Returns the list of states this status can legally transition to.
    pub fn transitions(&self) -> Vec<Self> {
        match self {
            Self::Pending => vec![Self::Asked, Self::Snoozed, Self::Dismissed],
            Self::Asked => vec![Self::Answered, Self::Snoozed, Self::Dismissed],
            Self::Snoozed => vec![Self::Pending, Self::Dismissed],
            // Terminal states
            Self::Answered => vec![],
            Self::Dismissed => vec![],
        }
    }

    /// Whether a transition from `self` to `next` is valid.
    pub fn can_transition_to(&self, next: &Self) -> bool {
        self.transitions().contains(next)
    }

    pub fn checked_transition(&self, next: Self) -> Result<Self, DomainError> {
        if self.can_transition_to(&next) {
            Ok(next)
        } else {
            Err(DomainError::InvalidTransition {
                entity: "question",
                from: self.to_string(),
                to: next.to_string(),
            })
        }
    }
}

impl FromStr for QuestionStatus {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "asked" => Ok(Self::Asked),
            "answered" => Ok(Self::Answered),
            "snoozed" => Ok(Self::Snoozed),
            "dismissed" => Ok(Self::Dismissed),
            other => Err(DomainError::InvalidStatus {
                entity: "question",
                value: other.to_string(),
            }),
        }
    }
}

impl fmt::Display for QuestionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Pending => "pending",
            Self::Asked => "asked",
            Self::Answered => "answered",
            Self::Snoozed => "snoozed",
            Self::Dismissed => "dismissed",
        };
        write!(f, "{}", s)
    }
}

// ─── HypothesisStatus ────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisStatus {
    Candidate,
    Validated,
    Rejected,
    Promoted,
}

impl HypothesisStatus {
    pub fn transitions(&self) -> Vec<Self> {
        match self {
            Self::Candidate => vec![Self::Validated, Self::Rejected, Self::Promoted],
            Self::Validated => vec![Self::Promoted, Self::Rejected],
            Self::Rejected | Self::Promoted => vec![],
        }
    }

    pub fn can_transition_to(&self, other: &Self) -> bool {
        self.transitions().contains(other)
    }

    pub fn checked_transition(&self, other: Self) -> Result<Self, DomainError> {
        if self.can_transition_to(&other) {
            Ok(other)
        } else {
            Err(DomainError::InvalidTransition {
                entity: "hypothesis",
                from: self.to_string(),
                to: other.to_string(),
            })
        }
    }
}

impl fmt::Display for HypothesisStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Candidate => "candidate",
            Self::Validated => "validated",
            Self::Rejected => "rejected",
            Self::Promoted => "promoted",
        };
        write!(f, "{s}")
    }
}

impl FromStr for HypothesisStatus {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "candidate" => Ok(Self::Candidate),
            "validated" => Ok(Self::Validated),
            "rejected" => Ok(Self::Rejected),
            "promoted" => Ok(Self::Promoted),
            other => Err(DomainError::InvalidStatus {
                entity: "hypothesis",
                value: other.to_string(),
            }),
        }
    }
}

// ─── EvolutionStatus ─────────────────────────────────────────────────────────

/// Lifecycle of an Evolution candidate (`00-product.md` §3.10).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionStatus {
    Candidate,
    Approved,
    Rejected,
    Active,
}

impl EvolutionStatus {
    pub fn transitions(&self) -> Vec<Self> {
        match self {
            Self::Candidate => vec![Self::Approved, Self::Rejected, Self::Active],
            Self::Approved => vec![Self::Active],
            Self::Rejected | Self::Active => vec![],
        }
    }

    pub fn can_transition_to(&self, other: &Self) -> bool {
        self.transitions().contains(other)
    }

    pub fn checked_transition(&self, other: Self) -> Result<Self, DomainError> {
        if self.can_transition_to(&other) {
            Ok(other)
        } else {
            Err(DomainError::InvalidTransition {
                entity: "evolution",
                from: self.to_string(),
                to: other.to_string(),
            })
        }
    }
}

impl fmt::Display for EvolutionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Candidate => "candidate",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Active => "active",
        };
        write!(f, "{s}")
    }
}

impl FromStr for EvolutionStatus {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "candidate" => Ok(Self::Candidate),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            "active" => Ok(Self::Active),
            other => Err(DomainError::InvalidStatus {
                entity: "evolution",
                value: other.to_string(),
            }),
        }
    }
}

// ─── JobKind ─────────────────────────────────────────────────────────────────

/// MVP learning-queue job kinds (`00-product.md` §7).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    ExtractExperience,
    DetectGaps,
    ProposeKnowledge,
    ProposeSkill,
    SynthesizeKnowledge,
    AnalyzeKnowledgeGaps,
    AutoResearch,
    SynthesizeMethod,
}

impl fmt::Display for JobKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::ExtractExperience => "extract_experience",
            Self::DetectGaps => "detect_gaps",
            Self::ProposeKnowledge => "propose_knowledge",
            Self::ProposeSkill => "propose_skill",
            Self::SynthesizeKnowledge => "synthesize_knowledge",
            Self::AnalyzeKnowledgeGaps => "analyze_knowledge_gaps",
            Self::AutoResearch => "auto_research",
            Self::SynthesizeMethod => "synthesize_method",
        };
        write!(f, "{}", s)
    }
}

impl FromStr for JobKind {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "extract_experience" => Ok(Self::ExtractExperience),
            "detect_gaps" => Ok(Self::DetectGaps),
            "propose_knowledge" => Ok(Self::ProposeKnowledge),
            "propose_skill" => Ok(Self::ProposeSkill),
            "synthesize_knowledge" => Ok(Self::SynthesizeKnowledge),
            "analyze_knowledge_gaps" => Ok(Self::AnalyzeKnowledgeGaps),
            "auto_research" => Ok(Self::AutoResearch),
            "synthesize_method" => Ok(Self::SynthesizeMethod),
            other => Err(DomainError::InvalidStatus {
                entity: "job_kind",
                value: other.to_string(),
            }),
        }
    }
}

// ─── JobStatus ───────────────────────────────────────────────────────────────

/// Lifecycle status of a learning job.
///
/// State machine: `queued → running → {done, failed, cancelled}`
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobStatus {
    /// Returns the list of states this status can legally transition to.
    pub fn transitions(&self) -> Vec<Self> {
        match self {
            Self::Queued => vec![Self::Running, Self::Cancelled],
            Self::Running => vec![Self::Done, Self::Failed, Self::Cancelled],
            // Terminal states
            Self::Done => vec![],
            Self::Failed => vec![],
            Self::Cancelled => vec![],
        }
    }

    /// Whether a transition from `self` to `next` is valid.
    pub fn can_transition_to(&self, next: &Self) -> bool {
        self.transitions().contains(next)
    }

    pub fn checked_transition(&self, next: Self) -> Result<Self, DomainError> {
        if self.can_transition_to(&next) {
            Ok(next)
        } else {
            Err(DomainError::InvalidTransition {
                entity: "job",
                from: self.to_string(),
                to: next.to_string(),
            })
        }
    }
}

impl FromStr for JobStatus {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "done" => Ok(Self::Done),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(DomainError::InvalidStatus {
                entity: "job",
                value: other.to_string(),
            }),
        }
    }
}

impl fmt::Display for JobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        };
        write!(f, "{}", s)
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── TaskStatus ───────────────────────────────────────────────────────

    #[test]
    fn task_queued_to_planning() {
        assert!(TaskStatus::Queued.can_transition_to(&TaskStatus::Planning));
    }

    #[test]
    fn task_queued_to_cancelled() {
        assert!(TaskStatus::Queued.can_transition_to(&TaskStatus::Cancelled));
    }

    #[test]
    fn task_queued_cannot_skip_to_running() {
        assert!(!TaskStatus::Queued.can_transition_to(&TaskStatus::Running));
    }

    #[test]
    fn task_running_to_waiting_user() {
        assert!(TaskStatus::Running.can_transition_to(&TaskStatus::WaitingUser));
    }

    #[test]
    fn task_running_to_reviewing() {
        assert!(TaskStatus::Running.can_transition_to(&TaskStatus::Reviewing));
    }

    #[test]
    fn task_reviewing_to_completed() {
        assert!(TaskStatus::Reviewing.can_transition_to(&TaskStatus::Completed));
    }

    #[test]
    fn task_reviewing_to_running_follow_up() {
        assert!(TaskStatus::Reviewing.can_transition_to(&TaskStatus::Running));
    }

    #[test]
    fn task_checked_transition_rejects_skip() {
        let err = TaskStatus::Queued
            .checked_transition(TaskStatus::Running)
            .unwrap_err();
        assert!(matches!(err, DomainError::InvalidTransition { .. }));
    }

    #[test]
    fn task_completed_is_terminal() {
        assert!(!TaskStatus::Completed.can_transition_to(&TaskStatus::Running));
        assert!(TaskStatus::Completed.transitions().is_empty());
        assert!(TaskStatus::Completed.is_terminal());
    }

    #[test]
    fn task_failed_is_terminal() {
        assert!(TaskStatus::Failed.transitions().is_empty());
    }

    #[test]
    fn task_display() {
        assert_eq!(TaskStatus::WaitingUser.to_string(), "waiting_user");
        assert_eq!(TaskStatus::Queued.to_string(), "queued");
    }

    // ── SessionStatus ────────────────────────────────────────────────────

    #[test]
    fn session_spawning_to_running() {
        assert!(SessionStatus::Spawning.can_transition_to(&SessionStatus::Running));
    }

    #[test]
    fn session_spawning_to_failed() {
        assert!(SessionStatus::Spawning.can_transition_to(&SessionStatus::Failed));
    }

    #[test]
    fn session_spawning_cannot_go_to_exited() {
        assert!(!SessionStatus::Spawning.can_transition_to(&SessionStatus::Exited));
    }

    #[test]
    fn session_running_to_waiting_user() {
        assert!(SessionStatus::Running.can_transition_to(&SessionStatus::WaitingUser));
    }

    #[test]
    fn session_running_to_paused() {
        assert!(SessionStatus::Running.can_transition_to(&SessionStatus::Paused));
    }

    #[test]
    fn session_paused_to_running() {
        assert!(SessionStatus::Paused.can_transition_to(&SessionStatus::Running));
    }

    #[test]
    fn session_exited_is_terminal() {
        assert!(SessionStatus::Exited.transitions().is_empty());
    }

    #[test]
    fn session_display() {
        assert_eq!(SessionStatus::WaitingUser.to_string(), "waiting_user");
        assert_eq!(SessionStatus::Spawning.to_string(), "spawning");
    }

    // ── KnowledgeStatus ──────────────────────────────────────────────────

    #[test]
    fn knowledge_candidate_to_committed() {
        assert!(KnowledgeStatus::Candidate.can_transition_to(&KnowledgeStatus::Committed));
    }

    #[test]
    fn knowledge_candidate_to_conflicted() {
        assert!(KnowledgeStatus::Candidate.can_transition_to(&KnowledgeStatus::Conflicted));
    }

    #[test]
    fn knowledge_candidate_to_rejected() {
        assert!(KnowledgeStatus::Candidate.can_transition_to(&KnowledgeStatus::Rejected));
    }

    #[test]
    fn knowledge_conflicted_to_committed() {
        assert!(KnowledgeStatus::Conflicted.can_transition_to(&KnowledgeStatus::Committed));
    }

    #[test]
    fn knowledge_committed_is_terminal() {
        assert!(KnowledgeStatus::Committed.transitions().is_empty());
    }

    #[test]
    fn knowledge_committed_cannot_go_to_candidate() {
        assert!(!KnowledgeStatus::Committed.can_transition_to(&KnowledgeStatus::Candidate));
    }

    #[test]
    fn knowledge_display() {
        assert_eq!(KnowledgeStatus::Candidate.to_string(), "candidate");
        assert_eq!(KnowledgeStatus::Conflicted.to_string(), "conflicted");
    }

    // ── QuestionStatus ───────────────────────────────────────────────────

    #[test]
    fn question_pending_to_asked() {
        assert!(QuestionStatus::Pending.can_transition_to(&QuestionStatus::Asked));
    }

    #[test]
    fn question_asked_to_answered() {
        assert!(QuestionStatus::Asked.can_transition_to(&QuestionStatus::Answered));
    }

    #[test]
    fn question_pending_to_snoozed() {
        assert!(QuestionStatus::Pending.can_transition_to(&QuestionStatus::Snoozed));
    }

    #[test]
    fn question_snoozed_to_pending() {
        assert!(QuestionStatus::Snoozed.can_transition_to(&QuestionStatus::Pending));
    }

    #[test]
    fn question_answered_is_terminal() {
        assert!(QuestionStatus::Answered.transitions().is_empty());
    }

    #[test]
    fn question_answered_cannot_go_to_pending() {
        assert!(!QuestionStatus::Answered.can_transition_to(&QuestionStatus::Pending));
    }

    #[test]
    fn question_display() {
        assert_eq!(QuestionStatus::Pending.to_string(), "pending");
        assert_eq!(QuestionStatus::Snoozed.to_string(), "snoozed");
    }

    // ── JobStatus ────────────────────────────────────────────────────────

    #[test]
    fn job_queued_to_running() {
        assert!(JobStatus::Queued.can_transition_to(&JobStatus::Running));
    }

    #[test]
    fn job_queued_to_cancelled() {
        assert!(JobStatus::Queued.can_transition_to(&JobStatus::Cancelled));
    }

    #[test]
    fn job_running_to_done() {
        assert!(JobStatus::Running.can_transition_to(&JobStatus::Done));
    }

    #[test]
    fn job_running_to_failed() {
        assert!(JobStatus::Running.can_transition_to(&JobStatus::Failed));
    }

    #[test]
    fn job_done_is_terminal() {
        assert!(JobStatus::Done.transitions().is_empty());
    }

    #[test]
    fn job_done_cannot_go_to_queued() {
        assert!(!JobStatus::Done.can_transition_to(&JobStatus::Queued));
    }

    #[test]
    fn job_display() {
        assert_eq!(JobStatus::Queued.to_string(), "queued");
        assert_eq!(JobStatus::Done.to_string(), "done");
        assert_eq!(JobStatus::Cancelled.to_string(), "cancelled");
    }

    #[test]
    fn job_kind_roundtrip() {
        assert_eq!(JobKind::ExtractExperience.to_string(), "extract_experience");
        assert_eq!(
            "propose_knowledge".parse::<JobKind>().unwrap(),
            JobKind::ProposeKnowledge
        );
        assert_eq!(
            "propose_skill".parse::<JobKind>().unwrap(),
            JobKind::ProposeSkill
        );
    }

    #[test]
    fn knowledge_and_question_parse() {
        assert_eq!(
            "conflicted".parse::<KnowledgeStatus>().unwrap(),
            KnowledgeStatus::Conflicted
        );
        assert_eq!(
            "snoozed".parse::<QuestionStatus>().unwrap(),
            QuestionStatus::Snoozed
        );
    }
}
