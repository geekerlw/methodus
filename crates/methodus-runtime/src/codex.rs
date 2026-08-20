use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use methodus_domain::RuntimeEvent;

use crate::adapter::{RuntimeAdapter, RuntimeError, SessionHandle, SpawnInput};

/// Adapter for the Codex CLI (`codex exec --json` / `codex exec resume`).
pub struct CodexAdapter;

impl CodexAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn codex_args(input: &SpawnInput, resume: Option<&str>) -> Vec<String> {
    let mut args = vec!["exec".to_string()];
    if let Some(ref sandbox) = input.sandbox {
        args.push("--sandbox".to_string());
        args.push(sandbox.clone());
    }
    if let Some(sid) = resume {
        args.push("resume".to_string());
        args.push("--json".to_string());
        args.push(sid.to_string());
    } else {
        args.push("--json".to_string());
        args.push("-C".to_string());
        args.push(input.cwd.to_string_lossy().into_owned());
    }
    args.push(input.prompt.clone());
    args
}

async fn spawn_codex(
    input: SpawnInput,
    resume: Option<&str>,
) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError> {
    let args = codex_args(&input, resume);
    let mut cmd = Command::new("codex");
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
                warn!(target: "codex.stderr", "{line}");
            }
        });
    }

    let (tx, rx) = mpsc::channel(256);
    let fallback_sid = resume
        .map(str::to_owned)
        .unwrap_or_else(|| input.session_id.clone());
    let fallback_sid_for_handle = fallback_sid.clone();

    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut assistant = String::new();
        let mut saw_result = false;

        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            let events = parse_codex_event(&line, &fallback_sid);
            for event in events {
                if let RuntimeEvent::AssistantText { text } = &event {
                    assistant.push_str(text);
                }
                if matches!(event, RuntimeEvent::Result { .. }) {
                    saw_result = true;
                }
                if tx.send(event).await.is_err() {
                    debug!("event receiver dropped, stopping reader");
                    break;
                }
            }
        }

        if !saw_result {
            let _ = tx
                .send(RuntimeEvent::Result {
                    is_error: false,
                    text: assistant,
                    cost_usd: None,
                    usage: None,
                    session_id: Some(fallback_sid.clone()),
                    permission_denials: Vec::new(),
                })
                .await;
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
impl RuntimeAdapter for CodexAdapter {
    async fn spawn(
        &self,
        input: SpawnInput,
    ) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError> {
        spawn_codex(input, None).await
    }

    async fn resume(
        &self,
        executor_sid: &str,
        input: SpawnInput,
    ) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError> {
        spawn_codex(input, Some(executor_sid)).await
    }

    async fn stop(&self, handle: &SessionHandle) -> Result<(), RuntimeError> {
        if let Some(pid) = handle.pid {
            let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            if ret != 0 {
                return Err(RuntimeError::Io(std::io::Error::last_os_error()));
            }
        }
        Ok(())
    }
}

fn parse_codex_event(line: &str, fallback_sid: &str) -> Vec<RuntimeEvent> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        warn!(
            "failed to parse Codex JSONL: {}",
            &line[..line.len().min(200)]
        );
        return Vec::new();
    };
    let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match event_type {
        "thread.started" => {
            let sid = value
                .get("thread_id")
                .and_then(|v| v.as_str())
                .unwrap_or(fallback_sid);
            vec![RuntimeEvent::SessionStarted {
                session_id: sid.to_owned(),
            }]
        }
        "item.started" => parse_codex_item(&value, true),
        "item.completed" => parse_codex_item(&value, false),
        "turn.completed" => {
            let usage = value.get("usage").cloned();
            vec![
                RuntimeEvent::TurnCompleted {
                    stop_reason: Some("completed".to_string()),
                },
                RuntimeEvent::Result {
                    is_error: false,
                    text: String::new(),
                    cost_usd: None,
                    usage,
                    session_id: Some(fallback_sid.to_owned()),
                    permission_denials: Vec::new(),
                },
            ]
        }
        _ => Vec::new(),
    }
}

fn parse_codex_item(value: &serde_json::Value, started: bool) -> Vec<RuntimeEvent> {
    let Some(item) = value.get("item") else {
        return Vec::new();
    };
    let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match item_type {
        "command_execution" => {
            let command = item
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            let id = item
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or(&command)
                .to_owned();
            if started {
                vec![RuntimeEvent::ToolCallStarted {
                    id,
                    name: "command_execution".to_string(),
                    input: serde_json::json!({ "command": command }),
                }]
            } else {
                let output = item
                    .get("aggregated_output")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let exit_code = item
                    .get("exit_code")
                    .and_then(|v| v.as_i64())
                    .map(|n| n as i32);
                vec![RuntimeEvent::ToolCallCompleted {
                    id,
                    output,
                    exit_code,
                }]
            }
        }
        "agent_message" if !started => {
            let text = item
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            if text.is_empty() {
                Vec::new()
            } else {
                vec![RuntimeEvent::AssistantText { text }]
            }
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_input() -> SpawnInput {
        SpawnInput {
            prompt: "hello".to_string(),
            cwd: PathBuf::from("/tmp/ws"),
            session_id: "m-1".to_string(),
            permission_mode: String::new(),
            allowed_tools: Vec::new(),
            sandbox: Some("workspace-write".to_string()),
            extra_dirs: Vec::new(),
            model: None,
        }
    }

    #[test]
    fn spawn_args_include_sandbox_and_cwd() {
        let args = codex_args(&sample_input(), None);
        assert_eq!(args[0], "exec");
        assert!(args.contains(&"--json".to_string()));
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(args.contains(&"workspace-write".to_string()));
        assert!(args.contains(&"-C".to_string()));
        assert!(args.contains(&"/tmp/ws".to_string()));
        assert!(!args.contains(&"resume".to_string()));
    }

    #[test]
    fn resume_args_use_thread_id() {
        let args = codex_args(&sample_input(), Some("019fthread"));
        assert!(args.contains(&"resume".to_string()));
        assert!(args.contains(&"019fthread".to_string()));
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(args.contains(&"workspace-write".to_string()));
    }

    #[test]
    fn parse_thread_started() {
        let events = parse_codex_event(r#"{"type":"thread.started","thread_id":"019fabc"}"#, "fb");
        match &events[0] {
            RuntimeEvent::SessionStarted { session_id } => assert_eq!(session_id, "019fabc"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_command_and_agent_message() {
        let start = parse_codex_event(
            r#"{"type":"item.started","item":{"type":"command_execution","command":"ls","status":"in_progress"}}"#,
            "fb",
        );
        assert!(
            matches!(&start[0], RuntimeEvent::ToolCallStarted { name, .. } if name == "command_execution")
        );

        let msg = parse_codex_event(
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"hi"}}"#,
            "fb",
        );
        match &msg[0] {
            RuntimeEvent::AssistantText { text } => assert_eq!(text, "hi"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_turn_completed_emits_result() {
        let events = parse_codex_event(
            r#"{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":2}}"#,
            "fb",
        );
        assert!(matches!(&events[0], RuntimeEvent::TurnCompleted { .. }));
        assert!(matches!(&events[1], RuntimeEvent::Result { is_error, .. } if !is_error));
    }
}
