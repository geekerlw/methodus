use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use methodus_domain::RuntimeEvent;

use crate::adapter::{RuntimeAdapter, RuntimeError, SessionHandle, SpawnInput};

/// Adapter for the Cursor Agent CLI (`cursor agent --print`).
pub struct CursorAdapter;

impl CursorAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CursorAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// Map Methodus permission mode to a Cursor CLI flag.
/// Never `--force`: that bypasses the classifier. Daily work uses `--auto-review`;
/// `plan` is read-only. Cursor has no mid-turn denial + re-grant loop.
pub(crate) fn cursor_permission_flag(mode: &str) -> &'static str {
    match mode {
        "plan" => "--plan",
        _ => "--auto-review",
    }
}

/// Build CLI args for `cursor agent`.
pub(crate) fn cursor_args(input: &SpawnInput, resume: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "agent".to_string(),
        "--print".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
    ];
    args.push("--workspace".to_string());
    args.push(input.cwd.to_string_lossy().into_owned());
    if let Some(sid) = resume {
        args.push("--resume".to_string());
        args.push(sid.to_string());
    }
    args.push(cursor_permission_flag(&input.permission_mode).to_string());
    for dir in &input.extra_dirs {
        if dir != &input.cwd {
            args.push("--add-dir".to_string());
            args.push(dir.to_string_lossy().into_owned());
        }
    }
    if let Some(ref model) = input.model {
        args.push("--model".to_string());
        args.push(model.clone());
    }
    args.push(input.prompt.clone());
    args
}

async fn spawn_cursor(
    input: SpawnInput,
    resume: Option<&str>,
) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError> {
    let args = cursor_args(&input, resume);
    let mut cmd = Command::new("cursor");
    cmd.args(&args);
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

    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                warn!(target: "cursor.stderr", "{line}");
            }
        });
    }

    let (tx, rx) = mpsc::channel(256);
    let fallback_sid = resume
        .map(str::to_owned)
        .unwrap_or_else(|| {
            input
                .executor_session_id
                .clone()
                .unwrap_or_else(|| input.session_id.clone())
        });
    let fallback_sid_for_handle = fallback_sid.clone();

    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            let events = parse_cursor_event(&line, &fallback_sid);
            for event in events {
                if tx.send(event).await.is_err() {
                    debug!("event receiver dropped, stopping reader");
                    break;
                }
            }
        }

        let _ = child.wait().await;
    });

    Ok((
        SessionHandle {
            session_id: input.session_id,
            executor_sid: Some(fallback_sid_for_handle),
            pid,
        },
        rx,
    ))
}

#[async_trait::async_trait]
impl RuntimeAdapter for CursorAdapter {
    async fn spawn(
        &self,
        input: SpawnInput,
    ) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError> {
        spawn_cursor(input, None).await
    }

    async fn resume(
        &self,
        executor_sid: &str,
        input: SpawnInput,
    ) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError> {
        spawn_cursor(input, Some(executor_sid)).await
    }

    async fn stop(&self, handle: &SessionHandle) -> Result<(), RuntimeError> {
        if let Some(pid) = handle.pid {
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

/// Parse a single JSONL line from Cursor's `stream-json` output.
pub(crate) fn parse_cursor_event(line: &str, fallback_session_id: &str) -> Vec<RuntimeEvent> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        warn!(
            "failed to parse Cursor JSONL line: {}",
            &line[..line.len().min(200)]
        );
        return Vec::new();
    };

    let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let subtype = value.get("subtype").and_then(|v| v.as_str()).unwrap_or("");

    match event_type {
        "system" if subtype == "init" => {
            let sid = value
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or(fallback_session_id);
            vec![RuntimeEvent::SessionStarted {
                session_id: sid.to_owned(),
            }]
        }
        "thinking" => {
            let text = value
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            if text.is_empty() {
                Vec::new()
            } else {
                vec![RuntimeEvent::Thinking { text }]
            }
        }
        "assistant" => parse_assistant_content(&value),
        "tool_call" => parse_tool_call(&value, subtype),
        "result" => {
            let is_error = value
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(subtype == "error");
            let text = value
                .get("result")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            let session_id = value
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            vec![RuntimeEvent::Result {
                is_error,
                text,
                cost_usd: None,
                usage: value.get("usage").cloned(),
                session_id,
                permission_denials: Vec::new(),
            }]
        }
        _ => Vec::new(),
    }
}

fn parse_assistant_content(value: &serde_json::Value) -> Vec<RuntimeEvent> {
    let content = value
        .get("message")
        .and_then(|m| m.get("content"))
        .or_else(|| value.get("content"))
        .and_then(|v| v.as_array());
    let Some(content) = content else {
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
                    .or_else(|| item.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                if !text.is_empty() {
                    events.push(RuntimeEvent::Thinking { text });
                }
            }
            _ => {}
        }
    }
    events
}

fn parse_tool_call(value: &serde_json::Value, subtype: &str) -> Vec<RuntimeEvent> {
    let Some(tool_call) = value.get("tool_call") else {
        return Vec::new();
    };
    let (id, name, inner) = cursor_tool_parts(tool_call);
    match subtype {
        "started" => {
            let input = inner.get("args").cloned().unwrap_or_else(|| inner.clone());
            vec![RuntimeEvent::ToolCallStarted { id, name, input }]
        }
        "completed" => {
            let failed = inner.get("result").and_then(|r| r.get("error")).is_some();
            vec![RuntimeEvent::ToolCallCompleted {
                id,
                output: inner,
                exit_code: Some(if failed { 1 } else { 0 }),
            }]
        }
        _ => Vec::new(),
    }
}

fn cursor_tool_parts(tool_call: &serde_json::Value) -> (String, String, serde_json::Value) {
    let Some(map) = tool_call.as_object() else {
        return ("tool".into(), "tool".into(), serde_json::Value::Null);
    };
    let Some((key, inner)) = map.iter().next() else {
        return ("tool".into(), "tool".into(), serde_json::Value::Null);
    };
    let name = match key.as_str() {
        "shellToolCall" => "Bash",
        "readToolCall" => "Read",
        "writeToolCall" | "editToolCall" | "applyPatchToolCall" => "Write",
        "grepToolCall" => "Grep",
        "globToolCall" => "Glob",
        "lsToolCall" => "LS",
        other => other.trim_end_matches("ToolCall"),
    };
    let id = inner
        .get("callId")
        .or_else(|| inner.get("toolCallId"))
        .or_else(|| inner.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or(key)
        .to_owned();
    (id, name.to_owned(), inner.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_input() -> SpawnInput {
        SpawnInput {
            prompt: "do the thing".to_string(),
            cwd: PathBuf::from("/tmp/ws"),
            session_id: "m-sid".to_string(),
            executor_session_id: Some("executor-1".to_string()),
            permission_mode: "acceptEdits".to_string(),
            allowed_tools: Vec::new(),
            sandbox: None,
            extra_dirs: Vec::new(),
            model: None,
        }
    }

    #[test]
    fn spawn_args_use_workspace_and_auto_review() {
        let args = cursor_args(&sample_input(), None);
        assert_eq!(args[0], "agent");
        assert!(args.contains(&"--print".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
        assert!(args.contains(&"--workspace".to_string()));
        assert!(args.contains(&"/tmp/ws".to_string()));
        assert!(args.contains(&"--auto-review".to_string()));
        assert!(!args.contains(&"--force".to_string()));
        assert!(!args.contains(&"--resume".to_string()));
        assert!(args.contains(&"do the thing".to_string()));
    }

    #[test]
    fn plan_mode_uses_plan_flag() {
        let mut input = sample_input();
        input.permission_mode = "plan".to_string();
        let args = cursor_args(&input, None);
        assert!(args.contains(&"--plan".to_string()));
        assert!(!args.contains(&"--auto-review".to_string()));
        assert!(!args.contains(&"--force".to_string()));
    }

    #[test]
    fn cautious_still_auto_review_never_force() {
        let mut input = sample_input();
        input.permission_mode = "cautious".to_string();
        let args = cursor_args(&input, None);
        assert!(args.contains(&"--auto-review".to_string()));
        assert!(!args.contains(&"--force".to_string()));
        assert!(!args.contains(&"--yolo".to_string()));
    }

    #[test]
    fn resume_args_use_resume_flag() {
        let args = cursor_args(&sample_input(), Some("cur-sid-1"));
        assert!(args.contains(&"--resume".to_string()));
        assert!(args.contains(&"cur-sid-1".to_string()));
    }

    #[test]
    fn spawn_args_include_extra_read_roots() {
        let mut input = sample_input();
        input.extra_dirs.push(PathBuf::from("/tmp/methodus/knowledge"));
        let args = cursor_args(&input, None);
        assert!(args.contains(&"--add-dir".to_string()));
        assert!(args.contains(&"/tmp/methodus/knowledge".to_string()));
    }

    #[test]
    fn parse_system_init() {
        let line = r#"{"type":"system","subtype":"init","session_id":"real-sid","permissionMode":"default"}"#;
        let events = parse_cursor_event(line, "fallback");
        match &events[0] {
            RuntimeEvent::SessionStarted { session_id } => assert_eq!(session_id, "real-sid"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_thinking_delta() {
        let line = r#"{"type":"thinking","subtype":"delta","text":"hmm"}"#;
        let events = parse_cursor_event(line, "s1");
        match &events[0] {
            RuntimeEvent::Thinking { text } => assert_eq!(text, "hmm"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_assistant_nested() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Four"}]}}"#;
        let events = parse_cursor_event(line, "s1");
        match &events[0] {
            RuntimeEvent::AssistantText { text } => assert_eq!(text, "Four"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_shell_tool_started() {
        let line = r#"{"type":"tool_call","subtype":"started","tool_call":{"shellToolCall":{"args":{"command":"ls"}}}}"#;
        let events = parse_cursor_event(line, "s1");
        match &events[0] {
            RuntimeEvent::ToolCallStarted { name, input, .. } => {
                assert_eq!(name, "Bash");
                assert_eq!(input["command"], "ls");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_read_tool_completed() {
        let line = r#"{"type":"tool_call","subtype":"completed","tool_call":{"readToolCall":{"args":{"path":"/tmp/x"},"result":{"success":{"content":"hi"}}}}}"#;
        let events = parse_cursor_event(line, "s1");
        match &events[0] {
            RuntimeEvent::ToolCallCompleted { id, exit_code, .. } => {
                assert_eq!(id, "readToolCall");
                assert_eq!(*exit_code, Some(0));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_result_success() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"Done","session_id":"sid-9","usage":{"input_tokens":1}}"#;
        let events = parse_cursor_event(line, "s1");
        match &events[0] {
            RuntimeEvent::Result {
                is_error,
                text,
                session_id,
                permission_denials,
                ..
            } => {
                assert!(!is_error);
                assert_eq!(text, "Done");
                assert_eq!(session_id.as_deref(), Some("sid-9"));
                assert!(permission_denials.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn never_emits_force_flag() {
        for mode in ["acceptEdits", "plan", "cautious", "default", "manual"] {
            assert_ne!(cursor_permission_flag(mode), "--force");
        }
    }
}
