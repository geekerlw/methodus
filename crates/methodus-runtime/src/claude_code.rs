use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use methodus_domain::{PermissionDenial, RuntimeEvent};

use crate::adapter::{LiveAgent, RuntimeAdapter, RuntimeError, SessionHandle, SpawnInput};

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

/// Build CLI args for a spawn (`--session-id`) or resume (`--resume`) invocation.
pub(crate) fn claude_args(input: &SpawnInput, resume: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "--print".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
    ];
    if let Some(sid) = resume {
        args.push("--resume".to_string());
        args.push(sid.to_string());
    } else {
        args.push("--session-id".to_string());
        args.push(input.executor_session_id.clone().unwrap_or_else(|| input.session_id.clone()));
    }
    args.push("--permission-mode".to_string());
    args.push(claude_permission_mode(&input.permission_mode).to_string());
    for tool in &input.allowed_tools {
        args.push("--allowed-tools".to_string());
        args.push(tool.clone());
    }
    if let Some(ref model) = input.model {
        args.push("--model".to_string());
        args.push(model.clone());
    }
    for dir in &input.extra_dirs {
        args.push("--add-dir".to_string());
        args.push(dir.to_string_lossy().into_owned());
    }
    // `--add-dir` / `--allowed-tools` are variadic; without `--` the prompt can be
    // swallowed as another flag value and Claude exits with no conversation.
    args.push("--".to_string());
    args.push(input.prompt.clone());
    args
}

/// Map Methodus permission mode to Claude `--permission-mode`.
/// Never `bypassPermissions`. `AcceptEdits` maps to Claude `auto` (goal-mode feel).
pub(crate) fn claude_permission_mode(mode: &str) -> &'static str {
    match mode {
        "plan" => "plan",
        "cautious" | "manual" | "default" => "manual",
        "acceptEdits" => "auto",
        _ => "auto",
    }
}

async fn spawn_claude(
    input: SpawnInput,
    resume: Option<&str>,
) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError> {
    let args = claude_args(&input, resume);
    let mut cmd = Command::new("claude");
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

    let stderr_buf = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
    if let Some(stderr) = child.stderr.take() {
        let stderr_buf = stderr_buf.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                warn!(target: "claude.stderr", "{line}");
                let mut guard = stderr_buf.lock().await;
                if guard.len() < 20 {
                    guard.push(line);
                }
            }
        });
    }

    let (tx, rx) = mpsc::channel(256);
    let fallback_sid = resume.map(str::to_owned).unwrap_or_else(|| {
        input.executor_session_id.clone().unwrap_or_else(|| input.session_id.clone())
    });
    // A fresh spawn receives an explicit UUID from Methodus. Keep it on the
    // handle immediately so a failed/empty first turn never falls back to the
    // Methodus run id (for example, `learn_<uuid>`) on the next turn.
    let handle_executor_sid = Some(fallback_sid.clone());

    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut saw_result = false;

        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            let events = parse_claude_event(&line, &fallback_sid);
            for event in events {
                if matches!(event, RuntimeEvent::Result { .. }) {
                    saw_result = true;
                }
                if tx.send(event).await.is_err() {
                    debug!("event receiver dropped, stopping reader");
                    let _ = child.wait().await;
                    return;
                }
            }
        }

        let status = child.wait().await;
        let code = status.ok().and_then(|s| s.code()).unwrap_or(1);
        if !saw_result {
            let stderr_tail = stderr_buf.lock().await.join(" | ");
            let detail = if stderr_tail.is_empty() {
                format!("claude exited without a result (code {code})")
            } else {
                format!("claude exited without a result (code {code}): {stderr_tail}")
            };
            let _ = tx
                .send(RuntimeEvent::Result {
                    is_error: true,
                    text: detail,
                    cost_usd: None,
                    usage: None,
                    session_id: None,
                    permission_denials: Vec::new(),
                })
                .await;
        }
    });

    let handle = SessionHandle {
        session_id: input.session_id,
        executor_sid: handle_executor_sid,
        pid,
    };
    Ok((handle, rx))
}

#[async_trait::async_trait]
impl RuntimeAdapter for ClaudeCodeAdapter {
    async fn spawn(
        &self,
        input: SpawnInput,
    ) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError> {
        spawn_claude(input, None).await
    }

    async fn resume(
        &self,
        executor_sid: &str,
        input: SpawnInput,
    ) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError> {
        spawn_claude(input, Some(executor_sid)).await
    }

    async fn stop(&self, handle: &SessionHandle) -> Result<(), RuntimeError> {
        if let Some(pid) = handle.pid {
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

    async fn list_live_agents(&self) -> Result<Vec<LiveAgent>, RuntimeError> {
        let output = Command::new("claude")
            .arg("agents")
            .arg("--json")
            .output()
            .await
            .map_err(|e| RuntimeError::SpawnFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(%stderr, "claude agents --json failed");
            return Ok(Vec::new());
        }

        parse_agents_json(&output.stdout)
    }

    fn uses_manual_permissions(&self) -> bool {
        true
    }
}

fn parse_agents_json(bytes: &[u8]) -> Result<Vec<LiveAgent>, RuntimeError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| RuntimeError::Parse(format!("claude agents --json: {e}")))?;

    let items = if let Some(arr) = value.as_array() {
        arr.clone()
    } else if let Some(arr) = value.get("agents").and_then(|v| v.as_array()) {
        arr.clone()
    } else {
        return Ok(Vec::new());
    };

    let mut agents = Vec::new();
    for item in items {
        let session_id = item
            .get("sessionId")
            .or_else(|| item.get("session_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        if session_id.is_empty() {
            continue;
        }
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or(&session_id)
            .to_owned();
        let pid = item.get("pid").and_then(|v| {
            v.as_u64()
                .map(|n| n as u32)
                .or_else(|| v.as_i64().map(|n| n as u32))
        });
        let status = item
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        agents.push(LiveAgent {
            id,
            session_id,
            pid,
            status,
        });
    }
    Ok(agents)
}

/// Parse a single JSONL line from Claude Code's `stream-json` output into zero or
/// more `RuntimeEvent`s. A single assistant message can contain multiple content
/// items (text, thinking, tool_use), so we return a Vec.
fn parse_claude_event(line: &str, fallback_session_id: &str) -> Vec<RuntimeEvent> {
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
        "system" if subtype == "init" => {
            let sid = value
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or(fallback_session_id);
            vec![RuntimeEvent::SessionStarted {
                session_id: sid.to_owned(),
            }]
        }

        "assistant" => parse_assistant_content(&value),

        "result" => {
            let is_error = value
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let mut text = value
                .get("result")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            if text.trim().is_empty() {
                if let Some(errs) = value.get("errors").and_then(|v| v.as_array()) {
                    let joined: Vec<&str> = errs.iter().filter_map(|e| e.as_str()).collect();
                    if !joined.is_empty() {
                        text = joined.join("; ");
                    }
                }
            }
            if text.trim().is_empty() {
                if let Some(subtype) = value.get("subtype").and_then(|v| v.as_str()) {
                    if subtype != "success" {
                        text = format!("claude {subtype}");
                    }
                }
            }
            let cost_usd = value.get("total_cost_usd").and_then(|v| v.as_f64());
            let usage = value.get("usage").cloned();
            let session_id = value
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned);

            vec![RuntimeEvent::Result {
                is_error,
                text,
                cost_usd,
                usage,
                session_id,
                permission_denials: parse_permission_denials(&value),
            }]
        }

        _ => Vec::new(),
    }
}

fn parse_permission_denials(value: &serde_json::Value) -> Vec<PermissionDenial> {
    let Some(arr) = value.get("permission_denials").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        let tool_name = item
            .get("tool_name")
            .or_else(|| item.get("toolName"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        if tool_name.is_empty() {
            continue;
        }
        let tool_use_id = item
            .get("tool_use_id")
            .or_else(|| item.get("toolUseId"))
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let tool_input = item
            .get("tool_input")
            .or_else(|| item.get("toolInput"))
            .or_else(|| item.get("input"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        out.push(PermissionDenial {
            tool_name,
            tool_use_id,
            tool_input,
        });
    }
    out
}

/// Parse content items from an assistant-type message.
/// Verified Claude stream-json nests content under `message.content`; older/test
/// fixtures may put `content` at the top level.
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

    fn sample_input() -> SpawnInput {
        SpawnInput {
            prompt: "do the thing".to_string(),
            cwd: "/tmp".into(),
            session_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
            executor_session_id: Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string()),
            permission_mode: "acceptEdits".to_string(),
            allowed_tools: Vec::new(),
            sandbox: None,
            extra_dirs: Vec::new(),
            model: None,
        }
    }

    #[test]
    fn spawn_args_use_session_id() {
        let args = claude_args(&sample_input(), None);
        assert!(args.contains(&"--session-id".to_string()));
        assert!(!args.contains(&"--resume".to_string()));
        assert!(args.contains(&"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string()));
    }

    #[test]
    fn fresh_spawn_prefers_executor_session_id_over_methodus_run_id() {
        let mut input = sample_input();
        input.session_id = "learn_legacy-run".into();
        input.executor_session_id = Some("11111111-2222-3333-4444-555555555555".into());
        let args = claude_args(&input, None);
        assert!(args.contains(&"11111111-2222-3333-4444-555555555555".to_string()));
        assert!(!args.contains(&"learn_legacy-run".to_string()));
    }

    #[test]
    fn resume_args_use_resume_flag() {
        let args = claude_args(&sample_input(), Some("exec-sid-1"));
        assert!(args.contains(&"--resume".to_string()));
        assert!(args.contains(&"exec-sid-1".to_string()));
        assert!(!args.contains(&"--session-id".to_string()));
    }

    #[test]
    fn spawn_args_pass_add_dir() {
        let mut input = sample_input();
        input.extra_dirs = vec!["/tmp/proj".into()];
        let args = claude_args(&input, None);
        assert!(args.contains(&"--add-dir".to_string()));
        assert!(args.contains(&"/tmp/proj".to_string()));
        assert!(args.iter().any(|a| a == "--"));
        let prompt_at = args.iter().position(|a| a == "do the thing").unwrap();
        let sep_at = args.iter().position(|a| a == "--").unwrap();
        assert!(sep_at < prompt_at);
        assert_eq!(args.last().map(String::as_str), Some("do the thing"));
    }

    #[test]
    fn parse_result_surfaces_errors_array() {
        let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"","session_id":"sid","errors":["No conversation found with session ID: sid"],"permission_denials":[]}"#;
        let events = parse_claude_event(line, "fallback");
        match &events[0] {
            RuntimeEvent::Result {
                is_error,
                text,
                ..
            } => {
                assert!(*is_error);
                assert!(text.contains("No conversation found"));
            }
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[test]
    fn parse_system_init_prefers_json_session_id() {
        let line = r#"{"type":"system","subtype":"init","session_id":"real-sid"}"#;
        let events = parse_claude_event(line, "fallback");
        assert_eq!(events.len(), 1);
        match &events[0] {
            RuntimeEvent::SessionStarted { session_id } => {
                assert_eq!(session_id, "real-sid");
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
    fn parse_assistant_text_nested_message() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Four"}]}}"#;
        let events = parse_claude_event(line, "s1");
        assert_eq!(events.len(), 1);
        match &events[0] {
            RuntimeEvent::AssistantText { text } => assert_eq!(text, "Four"),
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
        let line = r#"{"type":"result","is_error":false,"result":"Done","total_cost_usd":0.05,"session_id":"sid-9","usage":{"input_tokens":100,"output_tokens":50}}"#;
        let events = parse_claude_event(line, "s1");
        assert_eq!(events.len(), 1);
        match &events[0] {
            RuntimeEvent::Result {
                is_error,
                text,
                cost_usd,
                usage,
                session_id,
                permission_denials,
            } => {
                assert!(!is_error);
                assert_eq!(text, "Done");
                assert_eq!(*cost_usd, Some(0.05));
                assert!(usage.is_some());
                assert_eq!(session_id.as_deref(), Some("sid-9"));
                assert!(permission_denials.is_empty());
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_result_permission_denials() {
        let line = r#"{"type":"result","is_error":false,"result":"blocked","permission_denials":[{"tool_name":"Write","tool_use_id":"tu1","tool_input":{"file_path":"/tmp/x"}}]}"#;
        let events = parse_claude_event(line, "s1");
        match &events[0] {
            RuntimeEvent::Result {
                permission_denials, ..
            } => {
                assert_eq!(permission_denials.len(), 1);
                assert_eq!(permission_denials[0].tool_name, "Write");
                assert_eq!(permission_denials[0].tool_use_id.as_deref(), Some("tu1"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn allowed_tools_appear_in_args() {
        let mut input = sample_input();
        input.allowed_tools = vec!["Write".into(), "Read".into()];
        let args = claude_args(&input, None);
        assert!(args.contains(&"--allowed-tools".to_string()));
        assert!(args.contains(&"Write".to_string()));
        assert!(args.contains(&"Read".to_string()));
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

    #[test]
    fn parse_agents_array() {
        let json = r#"[{"pid":42,"id":"ag-1","sessionId":"sid-1","status":"busy"}]"#;
        let agents = parse_agents_json(json.as_bytes()).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].session_id, "sid-1");
        assert_eq!(agents[0].pid, Some(42));
    }

    #[test]
    fn parse_agents_wrapped_object() {
        let json = r#"{"agents":[{"id":"ag-2","session_id":"sid-2","status":"idle"}]}"#;
        let agents = parse_agents_json(json.as_bytes()).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].session_id, "sid-2");
        assert_eq!(agents[0].id, "ag-2");
    }

    #[test]
    fn maps_methodus_modes_never_bypass() {
        assert_eq!(claude_permission_mode("acceptEdits"), "auto");
        assert_eq!(claude_permission_mode("plan"), "plan");
        assert_eq!(claude_permission_mode("cautious"), "manual");
        assert_eq!(claude_permission_mode("default"), "manual");
        let mut input = sample_input();
        input.permission_mode = "cautious".to_string();
        let args = claude_args(&input, None);
        assert!(args.contains(&"manual".to_string()));
        assert!(!args.iter().any(|a| a.contains("bypass")));
    }
}
