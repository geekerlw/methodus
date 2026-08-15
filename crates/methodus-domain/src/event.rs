//! Normalized runtime events — all adapters map their native streams into this.

use serde::{Deserialize, Serialize};

/// Normalized runtime event — all adapters map their native streams into this.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    SessionStarted {
        session_id: String,
    },
    /// Injected by Methodus when the user sends a chat turn (not from adapters).
    UserText {
        text: String,
    },
    AssistantText {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolCallStarted {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolCallCompleted {
        id: String,
        output: serde_json::Value,
        exit_code: Option<i32>,
    },
    TurnCompleted {
        stop_reason: Option<String>,
    },
    Result {
        is_error: bool,
        text: String,
        cost_usd: Option<f64>,
        usage: Option<serde_json::Value>,
        /// Executor-issued session id when the native event carries one (recovery key).
        session_id: Option<String>,
        /// Claude `--permission-mode manual` denials (empty for other runtimes).
        #[serde(default)]
        permission_denials: Vec<PermissionDenial>,
    },
    ApprovalRequested {
        id: String,
        tool_name: String,
        input: serde_json::Value,
    },
    Error {
        message: String,
    },
}

/// One blocked tool call from an executor permission layer.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PermissionDenial {
    pub tool_name: String,
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub tool_input: serde_json::Value,
}
