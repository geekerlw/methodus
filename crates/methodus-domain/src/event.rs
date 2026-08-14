//! Normalized runtime events — all adapters map their native streams into this.

use serde::{Deserialize, Serialize};

/// Normalized runtime event — all adapters map their native streams into this.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    SessionStarted {
        session_id: String,
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
    },
    Error {
        message: String,
    },
}
