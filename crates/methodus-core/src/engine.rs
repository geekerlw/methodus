use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::mpsc;
use uuid::Uuid;

use methodus_domain::*;
use methodus_runtime::{RuntimeAdapter, SpawnInput};
use methodus_store::Store;

use crate::workspace::WorkspaceBuilder;

pub struct Engine {
    store: Arc<Store>,
    adapter: Arc<dyn RuntimeAdapter>,
    home: PathBuf,
}

impl Engine {
    pub fn new(store: Arc<Store>, adapter: Arc<dyn RuntimeAdapter>, home: PathBuf) -> Self {
        Self {
            store,
            adapter,
            home,
        }
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn create_task(
        &self,
        title: &str,
        request: &str,
        face: Option<&str>,
        runtime: Option<&str>,
    ) -> Result<Task, Box<dyn std::error::Error + Send + Sync>> {
        let now = Utc::now();
        let id_raw = Uuid::new_v4().to_string().replace('-', "");
        let task = Task {
            id: format!("task_{}", &id_raw[..12]),
            title: title.to_string(),
            request: request.to_string(),
            project_id: None,
            status: TaskStatus::Queued,
            runtime: Some(runtime.unwrap_or("claude-code").to_string()),
            workspace_id: None,
            resolution: face.map(|f| format!("{{\"face\":\"{f}\"}}")),
            version: 1,
            created_at: now,
            updated_at: now,
        };
        self.store.insert_task(&task)?;
        Ok(task)
    }

    /// Run a task: build workspace, spawn executor, stream events, persist, save experience.
    /// Returns a receiver of RuntimeEvents for the caller to consume (print to terminal).
    pub async fn run_task(
        &self,
        task_id: &str,
    ) -> Result<mpsc::Receiver<RuntimeEvent>, Box<dyn std::error::Error + Send + Sync>> {
        // 1. Load task
        let task = self
            .store
            .get_task(task_id)?
            .ok_or_else(|| format!("task not found: {task_id}"))?;

        // 2. Update status to Planning
        self.store
            .update_task_status(task_id, TaskStatus::Planning)?;

        // 3. Build workspace
        let ws_base = self.home.join("workspaces");
        let context = format!(
            "# Task: {}\n\n{}\n\nResolution: {:?}",
            task.title, task.request, task.resolution
        );
        let ws_root = WorkspaceBuilder::build(&ws_base, task_id, &context)?;

        let ws_id = format!("ws_{task_id}");
        self.store.insert_workspace(
            &ws_id,
            task_id,
            ws_root.to_str().unwrap_or(""),
            "active",
            &Utc::now().to_rfc3339(),
        )?;
        self.store
            .update_task_status(task_id, TaskStatus::Running)?;

        // 4. Spawn session
        let session_id = Uuid::new_v4().to_string();
        let session = Session {
            id: session_id.clone(),
            task_id: task_id.to_string(),
            runtime: task
                .runtime
                .clone()
                .unwrap_or_else(|| "claude-code".to_string()),
            executor_sid: None,
            transport: "subprocess".to_string(),
            pid: None,
            cwd: ws_root.to_str().unwrap_or("").to_string(),
            status: SessionStatus::Spawning,
            last_turn: None,
            started_at: Utc::now(),
            ended_at: None,
            updated_at: Utc::now(),
        };
        self.store.insert_session(&session)?;

        let spawn_input = SpawnInput {
            prompt: task.request.clone(),
            cwd: ws_root.clone(),
            session_id: session_id.clone(),
            permission_mode: "acceptEdits".to_string(),
            model: None,
        };

        let (handle, event_rx) = self
            .adapter
            .spawn(spawn_input)
            .await
            .map_err(|e| format!("spawn failed: {e}"))?;

        // Update session with executor info
        self.store.update_session_status(
            &session_id,
            SessionStatus::Running,
            handle.pid.map(|p| p.to_string()).as_deref(),
        )?;

        // 5. Relay events: persist each event and forward to caller
        let (caller_tx, caller_rx) = mpsc::channel(256);
        let store = self.store.clone();
        let task_id_owned = task_id.to_string();
        let session_id_owned = session_id.clone();

        tokio::spawn(relay_events(
            event_rx,
            caller_tx,
            store,
            task_id_owned,
            session_id_owned,
        ));

        Ok(caller_rx)
    }
}

async fn relay_events(
    mut event_rx: mpsc::Receiver<RuntimeEvent>,
    caller_tx: mpsc::Sender<RuntimeEvent>,
    store: Arc<Store>,
    task_id: String,
    session_id: String,
) {
    let mut seq: i64 = 0;
    let mut result_text = String::new();
    let mut is_error = false;

    while let Some(event) = event_rx.recv().await {
        seq += 1;
        // Persist event
        let evt_id = format!("evt_{}_{seq}", &session_id[..8.min(session_id.len())]);
        let event_type = match &event {
            RuntimeEvent::SessionStarted { .. } => "session.started",
            RuntimeEvent::AssistantText { .. } => "session.output",
            RuntimeEvent::Thinking { .. } => "session.thinking",
            RuntimeEvent::ToolCallStarted { .. } => "session.tool_start",
            RuntimeEvent::ToolCallCompleted { .. } => "session.tool_done",
            RuntimeEvent::TurnCompleted { .. } => "session.turn_done",
            RuntimeEvent::Result { .. } => "session.result",
            RuntimeEvent::Error { .. } => "session.error",
        };
        let payload = serde_json::to_string(&event).unwrap_or_default();
        let _ = store.insert_event(
            &evt_id,
            event_type,
            &Utc::now().to_rfc3339(),
            Some(&task_id),
            Some(&session_id),
            &payload,
            Some(seq),
        );

        // Track result for experience
        if let RuntimeEvent::Result {
            text,
            is_error: err,
            ..
        } = &event
        {
            result_text = text.clone();
            is_error = *err;
        }

        // Forward to caller
        if caller_tx.send(event).await.is_err() {
            break;
        }
    }

    // Session ended — update status and save experience
    let final_status = if is_error {
        SessionStatus::Failed
    } else {
        SessionStatus::Exited
    };
    let _ = store.update_session_status(&session_id, final_status, None);

    let task_status = if is_error {
        TaskStatus::Failed
    } else {
        TaskStatus::Completed
    };
    let _ = store.update_task_status(&task_id, task_status);

    // Save experience
    let exp_id_raw = Uuid::new_v4().to_string().replace('-', "");
    let exp = Experience {
        id: format!("exp_{}", &exp_id_raw[..12]),
        task_id: task_id.clone(),
        face_id: None,
        outcome: Some(if is_error { "failed" } else { "success" }.to_string()),
        summary: Some(if result_text.len() > 500 {
            result_text[..500].to_string()
        } else {
            result_text
        }),
        created_at: Utc::now(),
    };
    let _ = store.insert_experience(&exp);
}
