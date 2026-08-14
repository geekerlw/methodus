use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use methodus_domain::RuntimeEvent;

use crate::adapter::{RuntimeAdapter, RuntimeError, SessionHandle, SpawnInput};

/// Adapter for the Claude Code CLI (`claude` binary).
pub struct ClaudeCodeAdapter;

impl ClaudeCodeAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ClaudeCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl RuntimeAdapter for ClaudeCodeAdapter {
    async fn spawn(
        &self,
        input: SpawnInput,
    ) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError> {
        // Build command:
        // claude --print --output-format stream-json --verbose
        //        --session-id <id> --permission-mode <mode> [--model <model>] "<prompt>"
        let mut cmd = Command::new("claude");
        cmd.arg("--print")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .arg("--session-id")
            .arg(&input.session_id)
            .arg("--permission-mode")
            .arg(&input.permission_mode);

        if let Some(ref model) = input.model {
            cmd.arg("--model").arg(model);
        }

        cmd.arg(&input.prompt);
        cmd.current_dir(&input.cwd);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.stdin(std::process::Stdio::null());

        let mut child = cmd
            .spawn()
            .map_err(|e| RuntimeError::SpawnFailed(e.to_string()))?;

        let pid = child.id();

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RuntimeError::SpawnFailed("no stdout".into()))?;

        let (tx, rx) = mpsc::channel(256);
        let session_id = input.session_id.clone();

        // Spawn a background task to read stdout JSONL and emit RuntimeEvents.
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let events = parse_claude_event(&line, &session_id);
                for event in events {
                    if tx.send(event).await.is_err() {
                        // Receiver dropped — stop reading.
                        debug!("event receiver dropped, stopping reader");
                        break;
                    }
                }
            }

            // Wait for child to exit so we don't leave zombies.
            let _ = child.wait().await;
        });

        let handle = SessionHandle {
            session_id: input.session_id,
            pid,
        };
        Ok((handle, rx))
    }

    async fn stop(&self, handle: &SessionHandle) -> Result<(), RuntimeError> {
        if let Some(pid) = handle.pid {
            // Send SIGTERM to gracefully stop the executor process.
            // SAFETY: pid is a valid child process id from tokio::process::Child.
            let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            if ret != 0 {
                let err = std::io::Error::last_os_error();
                warn!(pid, %err, "failed to send SIGTERM");
                return Err(RuntimeError::Io(err));
            }
        }
        Ok(())
    }
}

/// Parse a single JSONL line from Claude Code's `stream-json` output into zero or
/// more `RuntimeEvent`s. A single assistant message can contain multiple content
/// items (text, thinking, tool_use), so we return a Vec.
fn parse_claude_event(line: &str, session_id: &str) -> Vec<RuntimeEvent> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        warn!(
            "failed to parse JSONL line: {}",
            &line[..line.len().min(200)]
        );
        return Vec::new();
    };

    let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let subtype = value.get("subtype").and_then(|v| v.as_str()).unwrap_or("");

    match event_type {
        // system/init → SessionStarted
        "system" if subtype == "init" => {
            vec![RuntimeEvent::SessionStarted {
                session_id: session_id.to_owned(),
            }]
        }

        // assistant message — iterate content array
        "assistant" => parse_assistant_content(&value),

        // result → Result
        "result" => {
            let is_error = value
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let text = value
                .get("result")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            let cost_usd = value.get("total_cost_usd").and_then(|v| v.as_f64());
            let usage = value.get("usage").cloned();

            vec![RuntimeEvent::Result {
                is_error,
                text,
                cost_usd,
                usage,
            }]
        }

        _ => Vec::new(),
    }
}

/// Parse content items from an assistant-type message.
fn parse_assistant_content(value: &serde_json::Value) -> Vec<RuntimeEvent> {
    let Some(content) = value.get("content").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let mut events = Vec::new();

    for item in content {
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match item_type {
            "text" => {
                let text = item
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                if !text.is_empty() {
                    events.push(RuntimeEvent::AssistantText { text });
                }
            }
            "thinking" => {
                let text = item
                    .get("thinking")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                if !text.is_empty() {
                    events.push(RuntimeEvent::Thinking { text });
                }
            }
            "tool_use" => {
                let id = item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                let name = item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                let input = item
                    .get("input")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                events.push(RuntimeEvent::ToolCallStarted { id, name, input });
            }
            _ => {}
        }
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_system_init() {
        let line = r#"{"type":"system","subtype":"init","session_id":"abc-123"}"#;
        let events = parse_claude_event(line, "abc-123");
        assert_eq!(events.len(), 1);
        match &events[0] {
            RuntimeEvent::SessionStarted { session_id } => {
                assert_eq!(session_id, "abc-123");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parse_assistant_text() {
        let line = r#"{"type":"assistant","content":[{"type":"text","text":"Hello world"}]}"#;
        let events = parse_claude_event(line, "s1");
        assert_eq!(events.len(), 1);
        match &events[0] {
            RuntimeEvent::AssistantText { text } => assert_eq!(text, "Hello world"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_assistant_thinking() {
        let line =
            r#"{"type":"assistant","content":[{"type":"thinking","thinking":"Let me think..."}]}"#;
        let events = parse_claude_event(line, "s1");
        assert_eq!(events.len(), 1);
        match &events[0] {
            RuntimeEvent::Thinking { text } => assert_eq!(text, "Let me think..."),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_assistant_tool_use() {
        let line = r#"{"type":"assistant","content":[{"type":"tool_use","id":"tu_1","name":"read_file","input":{"path":"/tmp/x"}}]}"#;
        let events = parse_claude_event(line, "s1");
        assert_eq!(events.len(), 1);
        match &events[0] {
            RuntimeEvent::ToolCallStarted { id, name, input } => {
                assert_eq!(id, "tu_1");
                assert_eq!(name, "read_file");
                assert_eq!(input["path"], "/tmp/x");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_assistant_multiple_content_items() {
        let line = r#"{"type":"assistant","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"answer"},{"type":"tool_use","id":"t1","name":"bash","input":{"cmd":"ls"}}]}"#;
        let events = parse_claude_event(line, "s1");
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0], RuntimeEvent::Thinking { .. }));
        assert!(matches!(&events[1], RuntimeEvent::AssistantText { .. }));
        assert!(matches!(&events[2], RuntimeEvent::ToolCallStarted { .. }));
    }

    #[test]
    fn parse_result() {
        let line = r#"{"type":"result","is_error":false,"result":"Done","total_cost_usd":0.05,"usage":{"input_tokens":100,"output_tokens":50}}"#;
        let events = parse_claude_event(line, "s1");
        assert_eq!(events.len(), 1);
        match &events[0] {
            RuntimeEvent::Result {
                is_error,
                text,
                cost_usd,
                usage,
            } => {
                assert!(!is_error);
                assert_eq!(text, "Done");
                assert_eq!(*cost_usd, Some(0.05));
                assert!(usage.is_some());
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_unknown_type_returns_empty() {
        let line = r#"{"type":"unknown","data":"foo"}"#;
        let events = parse_claude_event(line, "s1");
        assert!(events.is_empty());
    }

    #[test]
    fn parse_invalid_json_returns_empty() {
        let events = parse_claude_event("not json at all", "s1");
        assert!(events.is_empty());
    }
}
