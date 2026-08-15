use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use uuid::Uuid;

use methodus_domain::*;
use methodus_runtime::{RuntimeAdapter, SpawnInput};
use methodus_store::Store;

use crate::config::UserConfig;
use crate::error::CoreError;
use crate::learning;
use crate::lock::process_is_alive;
use crate::policy;
use crate::resolution::{self, Resolution};
use crate::scheduler;
use crate::workspace::WorkspaceBuilder;

const MAX_AUTO_TURNS: u32 = 12;

pub struct Engine {
    store: Arc<Store>,
    adapters: HashMap<String, Arc<dyn RuntimeAdapter>>,
    home: PathBuf,
    /// Directory Methodus was launched from. Source trees stay here; they are not copied.
    launch_cwd: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RecoveredSession {
    pub task_id: String,
    pub session_id: String,
    pub executor_sid: Option<String>,
    pub still_live: bool,
}

impl Engine {
    pub fn new(store: Arc<Store>, adapter: Arc<dyn RuntimeAdapter>, home: PathBuf) -> Self {
        let mut adapters = HashMap::new();
        adapters.insert("claude-code".to_string(), adapter);
        Self {
            store,
            adapters,
            home,
            launch_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    pub fn with_runtimes(
        store: Arc<Store>,
        home: PathBuf,
        adapters: HashMap<String, Arc<dyn RuntimeAdapter>>,
    ) -> Self {
        Self {
            store,
            adapters,
            home,
            launch_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    fn adapter(&self, runtime: &str) -> Result<Arc<dyn RuntimeAdapter>, CoreError> {
        self.adapters
            .get(runtime)
            .cloned()
            .ok_or_else(|| CoreError::UnknownRuntime(runtime.to_string()))
    }

    fn preferred_runtime(&self, requested: Option<&str>) -> String {
        if let Some(name) = requested {
            if self.adapters.contains_key(name) {
                return name.to_string();
            }
        }
        let cfg = UserConfig::load(&self.home);
        if let Some(name) = cfg.default_runtime {
            if self.adapters.contains_key(&name) {
                return name;
            }
        }
        for name in ["claude-code", "cursor", "codex"] {
            if self.adapters.contains_key(name) {
                return name.to_string();
            }
        }
        self.adapters
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "claude-code".to_string())
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn launch_cwd(&self) -> &Path {
        &self.launch_cwd
    }

    /// Directories the executor may read in place (launch cwd ∪ registered projects).
    pub fn context_roots(&self) -> Vec<(String, PathBuf)> {
        crate::mentions::context_roots(&self.home, &self.launch_cwd)
    }

    /// Root directory for per-task executor sandboxes (Claude/Codex cwd).
    pub fn workspace_root(&self) -> PathBuf {
        UserConfig::load(&self.home).resolve_workspace_root(&self.home)
    }

    /// Drain due learning jobs (extract → detect → propose). Budgeted, no LLM.
    pub fn tick_learning(&self) -> Result<usize, CoreError> {
        scheduler::tick(&self.store, &self.home)
    }

    /// Promote the highest-value pending question to Asked when the user is idle.
    /// No-op if a question is already Asked, or none clear the value floor.
    pub fn ask_idle_question(&self) -> Result<Option<Question>, CoreError> {
        learning::promote_idle_question(&self.store)
    }

    pub fn usage_summary(&self, today_only: bool) -> Result<UsageSummary, CoreError> {
        let since = if today_only {
            Utc::now()
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .map(|d| d.and_utc())
        } else {
            None
        };
        Ok(self.store.usage_summary(since)?)
    }

    pub fn review_knowledge(&self, id: &str, commit: bool) -> Result<KnowledgeItem, CoreError> {
        let mut item = self
            .store
            .get_knowledge(id)?
            .ok_or_else(|| CoreError::KnowledgeNotFound(id.to_string()))?;
        if commit && item.source == learning::SKILL_DRAFT_SOURCE {
            match learning::install_skill_draft(&self.home, &item) {
                Ok(live_path) => {
                    item.path = live_path;
                }
                Err(e) => {
                    if item.status == KnowledgeStatus::Candidate {
                        item.status = item
                            .status
                            .checked_transition(KnowledgeStatus::Conflicted)?;
                        item.updated_at = Utc::now();
                        self.store.update_knowledge(&item)?;
                    }
                    return Err(e);
                }
            }
        }
        let next = if commit {
            KnowledgeStatus::Committed
        } else {
            KnowledgeStatus::Rejected
        };
        item.status = item.status.checked_transition(next)?;
        item.updated_at = Utc::now();
        self.store.update_knowledge(&item)?;
        let ev = if commit {
            "knowledge.committed"
        } else {
            "knowledge.rejected"
        };
        let _ = self.store.insert_event(
            &format!("ev_{}", Uuid::new_v4().to_string().replace('-', "")),
            ev,
            &Utc::now().to_rfc3339(),
            None,
            None,
            &serde_json::json!({"knowledge_id": id, "kind": item.source}).to_string(),
            None,
        );
        Ok(item)
    }

    /// Distill the current task into a candidate skill (`/learn`). Never writes a live skill.
    pub fn learn_skill(
        &self,
        task_id: &str,
        hint: Option<&str>,
    ) -> Result<KnowledgeItem, CoreError> {
        learning::propose_skill_from_task(&self.store, &self.home, task_id, hint)?
            .ok_or_else(|| CoreError::Other("could not draft a skill from this task".into()))
    }

    pub fn answer_question(&self, id: &str, answer: &str) -> Result<Question, CoreError> {
        let mut q = self
            .store
            .get_question(id)?
            .ok_or_else(|| CoreError::QuestionNotFound(id.to_string()))?;
        if q.status == QuestionStatus::Pending {
            q.status = q.status.checked_transition(QuestionStatus::Asked)?;
        }
        q.status = q.status.checked_transition(QuestionStatus::Answered)?;
        q.answer = Some(answer.to_string());
        q.updated_at = Utc::now();
        self.store.update_question(&q)?;
        let _ = self.store.insert_event(
            &format!("ev_{}", Uuid::new_v4().to_string().replace('-', "")),
            "question.answered",
            &Utc::now().to_rfc3339(),
            q.task_id.as_deref(),
            None,
            &serde_json::json!({"question_id": id}).to_string(),
            None,
        );
        let refs = learning::JobRefs {
            experience_id: None,
            task_id: q.task_id.clone(),
            face_id: q.face_id.clone(),
            question_id: Some(q.id.clone()),
            source: Some("user_answer".to_string()),
        };
        learning::enqueue_job(
            &self.store,
            JobKind::ProposeKnowledge,
            &format!("propose:q:{}", q.id),
            &refs,
            20,
        )?;
        let _ = self.tick_learning()?;
        Ok(q)
    }

    pub fn snooze_question(&self, id: &str) -> Result<Question, CoreError> {
        let mut q = self
            .store
            .get_question(id)?
            .ok_or_else(|| CoreError::QuestionNotFound(id.to_string()))?;
        q.status = q.status.checked_transition(QuestionStatus::Snoozed)?;
        q.not_before = Some(Utc::now() + learning::snooze_hours());
        q.updated_at = Utc::now();
        self.store.update_question(&q)?;
        Ok(q)
    }

    pub fn dismiss_question(&self, id: &str) -> Result<Question, CoreError> {
        let mut q = self
            .store
            .get_question(id)?
            .ok_or_else(|| CoreError::QuestionNotFound(id.to_string()))?;
        q.status = q.status.checked_transition(QuestionStatus::Dismissed)?;
        q.updated_at = Utc::now();
        self.store.update_question(&q)?;
        Ok(q)
    }

    pub fn create_task(
        &self,
        title: &str,
        request: &str,
        face: Option<&str>,
        runtime: Option<&str>,
    ) -> Result<Task, CoreError> {
        let resolution = resolution::resolve(resolution::ResolveOpts {
            methodus_home: &self.home,
            request,
            requested_face: face,
        })?;
        let now = Utc::now();
        let id_raw = Uuid::new_v4().to_string().replace('-', "");
        let project_id = crate::project::focus_project(&self.home).map(|p| p.id);
        let task = Task {
            id: format!("task_{}", &id_raw[..12]),
            title: title.to_string(),
            request: request.to_string(),
            project_id,
            status: TaskStatus::Queued,
            runtime: Some(self.preferred_runtime(runtime)),
            workspace_id: None,
            resolution: Some(resolution.to_json()),
            version: 1,
            created_at: now,
            updated_at: now,
        };
        self.store.insert_task(&task)?;
        Ok(task)
    }

    /// Reconcile every non-terminal session against pid liveness + `claude agents --json`.
    pub async fn recover(&self) -> Result<Vec<RecoveredSession>, CoreError> {
        let _ = scheduler::recover_jobs(&self.store)?;
        let sessions = self.store.list_non_terminal_sessions()?;
        let mut out = Vec::new();
        for session in sessions {
            if let Some(rec) = self.reconcile_session(&session).await? {
                out.push(rec);
            }
        }
        Ok(out)
    }

    /// Cancel a task and interrupt any non-terminal sessions it owns.
    pub fn cancel_task(&self, task_id: &str) -> Result<(), CoreError> {
        let task = self
            .store
            .get_task(task_id)?
            .ok_or_else(|| CoreError::TaskNotFound(task_id.to_string()))?;
        if !task.status.can_transition_to(&TaskStatus::Cancelled) {
            return Err(CoreError::TaskNotCancellable(
                task_id.to_string(),
                task.status.to_string(),
            ));
        }
        for session in self.store.list_sessions_for_task(task_id)? {
            self.interrupt_session(&session)?;
        }
        self.store
            .update_task_status(task_id, TaskStatus::Cancelled)?;
        Ok(())
    }

    /// Cancel one session and, if the parent task is still open, cancel it too.
    pub fn cancel_session(&self, session_id: &str) -> Result<(), CoreError> {
        let session = self
            .store
            .get_session(session_id)?
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;
        if session.status.is_terminal() {
            return Err(CoreError::SessionNotCancellable(
                session_id.to_string(),
                session.status.to_string(),
            ));
        }
        self.interrupt_session(&session)?;
        if let Some(task) = self.store.get_task(&session.task_id)? {
            if task.status.can_transition_to(&TaskStatus::Cancelled) {
                self.store
                    .update_task_status(&task.id, TaskStatus::Cancelled)?;
            }
        }
        Ok(())
    }

    fn interrupt_session(&self, session: &Session) -> Result<(), CoreError> {
        if session.status.is_terminal() {
            return Ok(());
        }
        if let Some(pid) = session.pid {
            terminate_pid(pid);
        }
        let next = if session
            .status
            .can_transition_to(&SessionStatus::Interrupted)
        {
            SessionStatus::Interrupted
        } else if session.status.can_transition_to(&SessionStatus::Failed) {
            SessionStatus::Failed
        } else {
            return Ok(());
        };
        self.store.update_session_status(&session.id, next)?;
        Ok(())
    }

    /// Persist a user chat line so transcripts reload as a conversation.
    pub fn record_user_message(&self, task_id: &str, text: &str) -> Result<(), CoreError> {
        let _ = self
            .store
            .get_task(task_id)?
            .ok_or_else(|| CoreError::TaskNotFound(task_id.to_string()))?;
        let payload = serde_json::to_string(&RuntimeEvent::UserText {
            text: text.to_string(),
        })
        .unwrap_or_default();
        self.store.insert_event(
            &format!("ev_user_{}", Uuid::new_v4().to_string().replace('-', "")),
            "user.message",
            &Utc::now().to_rfc3339(),
            Some(task_id),
            None,
            &payload,
            None,
        )?;
        Ok(())
    }

    /// Send a user turn: first run on queued tasks, follow-up `--resume` on reviewing ones.
    pub async fn send_turn(
        &self,
        task_id: &str,
        prompt: &str,
    ) -> Result<mpsc::Receiver<RuntimeEvent>, CoreError> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(CoreError::EmptyMessage);
        }
        let task = self
            .store
            .get_task(task_id)?
            .ok_or_else(|| CoreError::TaskNotFound(task_id.to_string()))?;
        self.record_user_message(task_id, prompt)?;
        let resume = matches!(
            task.status,
            TaskStatus::Reviewing | TaskStatus::WaitingUser | TaskStatus::Running
        );
        self.run_task_inner(task_id, resume, Some(prompt.to_string()))
            .await
    }

    /// Run a task: build workspace, spawn or resume executor, stream events, persist, save experience.
    pub async fn run_task(
        &self,
        task_id: &str,
        resume: bool,
    ) -> Result<mpsc::Receiver<RuntimeEvent>, CoreError> {
        self.run_task_inner(task_id, resume, None).await
    }

    async fn run_task_inner(
        &self,
        task_id: &str,
        resume: bool,
        follow_up: Option<String>,
    ) -> Result<mpsc::Receiver<RuntimeEvent>, CoreError> {
        let mut task = self
            .store
            .get_task(task_id)?
            .ok_or_else(|| CoreError::TaskNotFound(task_id.to_string()))?;

        if matches!(
            task.status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        ) {
            return Err(CoreError::TaskNotRunnable(
                task_id.to_string(),
                task.status.to_string(),
            ));
        }

        if let Some(pending) = self
            .store
            .list_pending_approvals(Some(task_id))?
            .into_iter()
            .next()
        {
            return Err(CoreError::NeedsApproval(task_id.to_string(), pending.id));
        }

        let resume_sid = self.prepare_run(&task, resume).await?;

        // Reload after reconcile may have moved the task to waiting_user.
        task = self
            .store
            .get_task(task_id)?
            .ok_or_else(|| CoreError::TaskNotFound(task_id.to_string()))?;

        match task.status {
            TaskStatus::Queued => self.transition_task(&task, TaskStatus::Planning)?,
            TaskStatus::Planning
            | TaskStatus::WaitingUser
            | TaskStatus::Running
            | TaskStatus::Reviewing => {}
            other => {
                return Err(CoreError::TaskNotRunnable(
                    task_id.to_string(),
                    other.to_string(),
                ));
            }
        }

        let resolution = resolution_from_task(&task, &self.home)?;
        let snippets = learning::select_committed_knowledge(
            &self.store,
            &self.home,
            &resolution.face_id,
            &task.request,
        )?;
        let mut context = resolution.to_context_markdown(&task.title, &task.request);
        if let Some(ref pid) = task.project_id {
            if let Some(proj) = crate::project::list_projects(&self.home)
                .into_iter()
                .find(|p| p.id == *pid)
            {
                context.push_str(&format!(
                    "\n## Project\n\nRoot: `{}`\nWrites inside this tree are in-scope when the user asks.\n",
                    proj.root.display()
                ));
            }
        }
        context.push_str(&learning::render_knowledge_context(&snippets));
        let named_roots = self.context_roots();
        context.push_str(&crate::mentions::render_readable_dirs(&named_roots));
        let mention_source = follow_up.as_deref().unwrap_or(&task.request);
        let mentions = crate::mentions::resolve_named(mention_source, &named_roots);
        context.push_str(&crate::mentions::render_attached(&mentions));
        let extra_dirs = crate::mentions::readable_dirs(&named_roots);
        let ws_root = WorkspaceBuilder::build(&self.workspace_root(), task_id, &context)?;
        let knowledge_files: Vec<(String, PathBuf)> = snippets
            .iter()
            .map(|s| (s.dest_name.clone(), s.src_path.clone()))
            .collect();
        WorkspaceBuilder::materialize_knowledge(&ws_root, &knowledge_files)?;

        let face_yaml = if !resolution.face_dir.is_empty() {
            PathBuf::from(&resolution.face_dir).join("face.yaml")
        } else {
            self.home
                .join("faces")
                .join(&resolution.face_id)
                .join("face.yaml")
        };
        if face_yaml.is_file() {
            let dest = ws_root.join("face-context");
            fs::create_dir_all(&dest)?;
            fs::copy(&face_yaml, dest.join("face.yaml"))?;
        }

        let method_src = resolution.method.as_ref().map(|m| PathBuf::from(&m.path));
        let skill_files: Vec<(String, PathBuf)> = resolution
            .skills
            .iter()
            .map(|s| (s.name.clone(), PathBuf::from(&s.path)))
            .collect();
        WorkspaceBuilder::materialize_resolution(&ws_root, method_src.as_deref(), &skill_files)?;

        let ws_id = format!("ws_{task_id}");
        self.store.insert_workspace(
            &ws_id,
            task_id,
            ws_root.to_str().unwrap_or(""),
            "active",
            &Utc::now().to_rfc3339(),
        )?;
        self.store.update_task_workspace(task_id, &ws_id)?;

        task = self
            .store
            .get_task(task_id)?
            .ok_or_else(|| CoreError::TaskNotFound(task_id.to_string()))?;
        match task.status {
            TaskStatus::Planning | TaskStatus::WaitingUser | TaskStatus::Reviewing => {
                self.transition_task(&task, TaskStatus::Running)?;
            }
            TaskStatus::Running => {}
            other => {
                return Err(CoreError::TaskNotRunnable(
                    task_id.to_string(),
                    other.to_string(),
                ));
            }
        }

        let session_id = Uuid::new_v4().to_string();
        let session = Session {
            id: session_id.clone(),
            task_id: task_id.to_string(),
            runtime: task
                .runtime
                .clone()
                .unwrap_or_else(|| self.preferred_runtime(None)),
            executor_sid: resume_sid.clone(),
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

        let spawn_input_prompt = {
            let body = if let Some(text) = follow_up {
                text
            } else if resume_sid.is_some() {
                format!(
                    "The previous session was interrupted. Continue from where you left off.\n\n\
                     Original request:\n{}",
                    task.request
                )
            } else {
                task.request.clone()
            };
            if resume_sid.is_none() {
                format!(
                    "{body}\n\n\
                     Follow `.methodus/selected-context.md`. \
                     Load listed skills from `.claude/skills/` with the Skill tool, \
                     or read each `SKILL.md` and follow it."
                )
            } else {
                body
            }
        };

        let runtime_name = task
            .runtime
            .clone()
            .unwrap_or_else(|| self.preferred_runtime(None));
        let adapter = self.adapter(&runtime_name)?;
        let allowed_tools = policy::baseline_allowed_tools();
        let _ = self
            .store
            .set_session_allowed_tools(&session_id, &allowed_tools);

        self.launch_turns(LaunchTurns {
            adapter,
            runtime: runtime_name,
            task_id: task_id.to_string(),
            session_id,
            workspace: ws_root,
            face_id: Some(resolution.face_id),
            task_title: task.title.clone(),
            task_request: task.request.clone(),
            prompt: spawn_input_prompt,
            executor_sid: resume_sid.clone(),
            allowed_tools,
            next_is_resume: resume_sid.is_some(),
            extra_dirs,
        })
    }

    /// Resolve a pending approval and, when none remain, resume the executor turn.
    pub async fn approve(
        &self,
        approval_id: &str,
        decision: ApprovalDecision,
        actor: &str,
    ) -> Result<mpsc::Receiver<RuntimeEvent>, CoreError> {
        let approval = self
            .store
            .get_approval(approval_id)?
            .ok_or_else(|| CoreError::ApprovalNotFound(approval_id.to_string()))?;
        if approval.decision.is_some() {
            return Err(CoreError::ApprovalResolved(approval_id.to_string()));
        }

        self.store
            .resolve_approval(approval_id, &decision.to_string(), actor)?;
        let _ = self.store.insert_event(
            &format!("evt_{approval_id}_resolved"),
            "approval.resolved",
            &Utc::now().to_rfc3339(),
            Some(&approval.task_id),
            Some(&approval.session_id),
            &serde_json::json!({ "id": approval_id, "decision": decision.to_string() }).to_string(),
            None,
        );

        let task = self
            .store
            .get_task(&approval.task_id)?
            .ok_or_else(|| CoreError::TaskNotFound(approval.task_id.clone()))?;
        let session = self
            .store
            .get_session(&approval.session_id)?
            .ok_or_else(|| {
                CoreError::Other(format!("session not found: {}", approval.session_id))
            })?;

        if decision == ApprovalDecision::Abort {
            if task.status.can_transition_to(&TaskStatus::Cancelled) {
                self.store
                    .update_task_status(&task.id, TaskStatus::Cancelled)?;
            }
            if session
                .status
                .can_transition_to(&SessionStatus::Interrupted)
            {
                self.store
                    .update_session_status(&session.id, SessionStatus::Interrupted)?;
            }
            let (tx, rx) = mpsc::channel(1);
            drop(tx);
            return Ok(rx);
        }

        let mut allowed = self.store.get_session_allowed_tools(&session.id)?;
        match decision {
            ApprovalDecision::Once | ApprovalDecision::Session => {
                policy::grant_tools(&mut allowed, [approval.tool_name.clone()]);
                if decision == ApprovalDecision::Session {
                    self.store
                        .set_session_allowed_tools(&session.id, &allowed)?;
                }
            }
            ApprovalDecision::Deny => {}
            ApprovalDecision::Abort => {}
        }

        let remaining = self.store.list_pending_approvals(Some(&task.id))?;
        if !remaining.is_empty() {
            let (tx, rx) = mpsc::channel(1);
            drop(tx);
            return Ok(rx);
        }

        let runtime_name = session.runtime.clone();
        let adapter = self.adapter(&runtime_name)?;
        let prompt = match decision {
            ApprovalDecision::Deny => format!(
                "The user denied tool `{}`. Continue the original task without it.\n\nOriginal request:\n{}",
                approval.tool_name, task.request
            ),
            _ => format!(
                "The user approved additional tools. Continue the original task.\n\nOriginal request:\n{}",
                task.request
            ),
        };

        if task.status.can_transition_to(&TaskStatus::Running) {
            self.store
                .update_task_status(&task.id, TaskStatus::Running)?;
        }
        if session.status.can_transition_to(&SessionStatus::Running) {
            self.store
                .update_session_status(&session.id, SessionStatus::Running)?;
        }

        self.launch_turns(LaunchTurns {
            adapter,
            runtime: runtime_name,
            task_id: task.id.clone(),
            session_id: session.id.clone(),
            workspace: PathBuf::from(&session.cwd),
            face_id: resolution_from_task(&task, &self.home)
                .ok()
                .map(|r| r.face_id),
            task_title: task.title.clone(),
            task_request: task.request.clone(),
            prompt,
            executor_sid: session.executor_sid.clone(),
            allowed_tools: allowed,
            next_is_resume: session.executor_sid.is_some(),
            extra_dirs: crate::mentions::readable_dirs(&self.context_roots()),
        })
    }

    fn launch_turns(&self, launch: LaunchTurns) -> Result<mpsc::Receiver<RuntimeEvent>, CoreError> {
        let (caller_tx, caller_rx) = mpsc::channel(256);
        let permission_mode =
            policy::PermissionMode::parse(UserConfig::load(&self.home).permission_mode.as_deref());
        let runner = TurnRunner {
            store: self.store.clone(),
            adapter: launch.adapter,
            home: self.home.clone(),
            workspace: launch.workspace,
            task_id: launch.task_id,
            session_id: launch.session_id,
            face_id: launch.face_id,
            task_title: launch.task_title,
            task_request: launch.task_request,
            caller_tx,
            permission_mode,
            runtime: launch.runtime,
            extra_dirs: launch.extra_dirs,
        };
        tokio::spawn(runner.run(
            launch.prompt,
            launch.executor_sid,
            launch.allowed_tools,
            launch.next_is_resume,
        ));
        Ok(caller_rx)
    }

    fn transition_task(&self, task: &Task, next: TaskStatus) -> Result<(), CoreError> {
        if task.status == next {
            return Ok(());
        }
        task.status.checked_transition(next.clone())?;
        self.store.update_task_status(&task.id, next)?;
        Ok(())
    }

    async fn prepare_run(&self, task: &Task, resume: bool) -> Result<Option<String>, CoreError> {
        let sessions = self.store.list_sessions_for_task(&task.id)?;
        let mut latest_executor_sid: Option<String> = None;
        let mut had_interrupted = false;

        for session in &sessions {
            if !session.status.is_terminal() {
                if let Some(rec) = self.reconcile_session(session).await? {
                    if rec.still_live {
                        let pid = session.pid.unwrap_or(0);
                        return Err(CoreError::SessionLive(pid));
                    }
                    had_interrupted = true;
                    if rec.executor_sid.is_some() {
                        latest_executor_sid = rec.executor_sid;
                    }
                }
            } else if session.status == SessionStatus::Interrupted {
                had_interrupted = true;
                if session.executor_sid.is_some() && latest_executor_sid.is_none() {
                    latest_executor_sid = session.executor_sid.clone();
                }
            } else if latest_executor_sid.is_none() {
                latest_executor_sid = session.executor_sid.clone();
            }
        }

        if resume {
            return latest_executor_sid
                .clone()
                .ok_or_else(|| CoreError::NothingToResume(task.id.clone()))
                .map(Some);
        }

        if had_interrupted && latest_executor_sid.is_some() {
            return Err(CoreError::NeedsResume);
        }

        // A finished turn (reviewing) still has an executor thread to continue.
        if matches!(task.status, TaskStatus::Reviewing) && latest_executor_sid.is_some() {
            return Ok(latest_executor_sid);
        }

        Ok(None)
    }

    async fn reconcile_session(
        &self,
        session: &Session,
    ) -> Result<Option<RecoveredSession>, CoreError> {
        if session.status.is_terminal() {
            return Ok(None);
        }

        if session.status == SessionStatus::WaitingUser {
            return Ok(None);
        }

        let pid_alive = session.pid.map(process_is_alive).unwrap_or(false);
        let agent_alive = self
            .agent_is_live(&session.runtime, session.executor_sid.as_deref())
            .await;

        if pid_alive || agent_alive {
            return Ok(Some(RecoveredSession {
                task_id: session.task_id.clone(),
                session_id: session.id.clone(),
                executor_sid: session.executor_sid.clone(),
                still_live: true,
            }));
        }

        session
            .status
            .checked_transition(SessionStatus::Interrupted)?;
        self.store
            .update_session_status(&session.id, SessionStatus::Interrupted)?;

        if let Some(task) = self.store.get_task(&session.task_id)? {
            if matches!(task.status, TaskStatus::Running | TaskStatus::Planning)
                && task.status.can_transition_to(&TaskStatus::WaitingUser)
            {
                self.store
                    .update_task_status(&task.id, TaskStatus::WaitingUser)?;
            }
        }

        Ok(Some(RecoveredSession {
            task_id: session.task_id.clone(),
            session_id: session.id.clone(),
            executor_sid: session.executor_sid.clone(),
            still_live: false,
        }))
    }

    async fn agent_is_live(&self, runtime: &str, executor_sid: Option<&str>) -> bool {
        let Some(sid) = executor_sid else {
            return false;
        };
        let Ok(adapter) = self.adapter(runtime) else {
            return false;
        };
        match adapter.list_live_agents().await {
            Ok(agents) => agents.iter().any(|a| a.session_id == sid),
            Err(_) => false,
        }
    }
}

fn terminate_pid(pid: u32) {
    if pid <= 1 || pid == std::process::id() || !process_is_alive(pid) {
        return;
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
}

fn resolution_from_task(task: &Task, home: &std::path::Path) -> Result<Resolution, CoreError> {
    if let Some(raw) = &task.resolution {
        if let Some(res) = Resolution::parse_json(raw) {
            if !res.face_id.is_empty() {
                return Ok(res);
            }
        }
    }
    resolution::resolve(resolution::ResolveOpts {
        methodus_home: home,
        request: &task.request,
        requested_face: None,
    })
}

fn write_session_json(
    ws_root: &std::path::Path,
    session_id: &str,
    executor_sid: Option<&str>,
    pid: Option<u32>,
) -> Result<(), CoreError> {
    let body = serde_json::json!({
        "methodus_session_id": session_id,
        "executor_sid": executor_sid,
        "pid": pid,
    });
    fs::write(
        ws_root.join(".methodus/session.json"),
        serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".to_string()),
    )?;
    Ok(())
}

struct LaunchTurns {
    adapter: Arc<dyn RuntimeAdapter>,
    runtime: String,
    task_id: String,
    session_id: String,
    workspace: PathBuf,
    face_id: Option<String>,
    task_title: String,
    task_request: String,
    prompt: String,
    executor_sid: Option<String>,
    allowed_tools: Vec<String>,
    next_is_resume: bool,
    extra_dirs: Vec<PathBuf>,
}

struct TurnRunner {
    store: Arc<Store>,
    adapter: Arc<dyn RuntimeAdapter>,
    home: PathBuf,
    workspace: PathBuf,
    task_id: String,
    session_id: String,
    face_id: Option<String>,
    task_title: String,
    task_request: String,
    caller_tx: mpsc::Sender<RuntimeEvent>,
    permission_mode: policy::PermissionMode,
    runtime: String,
    extra_dirs: Vec<PathBuf>,
}

struct TurnOutcome {
    result_text: String,
    is_error: bool,
    denials: Vec<PermissionDenial>,
    executor_sid: Option<String>,
}

impl TurnRunner {
    async fn run(
        self,
        mut prompt: String,
        mut executor_sid: Option<String>,
        mut allowed_tools: Vec<String>,
        mut next_is_resume: bool,
    ) {
        for _turn in 0..MAX_AUTO_TURNS {
            let permission_mode = self.permission_mode.as_str().to_string();
            let sandbox = if self.runtime == "codex" {
                Some(self.permission_mode.codex_sandbox().to_string())
            } else {
                None
            };
            let spawn_input = SpawnInput {
                prompt: prompt.clone(),
                cwd: self.workspace.clone(),
                session_id: self.session_id.clone(),
                permission_mode,
                allowed_tools: allowed_tools.clone(),
                sandbox,
                extra_dirs: self.extra_dirs.clone(),
                model: None,
            };

            let spawn_res = if next_is_resume {
                if let Some(ref sid) = executor_sid {
                    self.adapter.resume(sid, spawn_input).await
                } else {
                    self.adapter.spawn(spawn_input).await
                }
            } else {
                self.adapter.spawn(spawn_input).await
            };

            let (handle, event_rx) = match spawn_res {
                Ok(v) => v,
                Err(e) => {
                    let _ = self
                        .caller_tx
                        .send(RuntimeEvent::Error {
                            message: e.to_string(),
                        })
                        .await;
                    let _ = self
                        .store
                        .update_session_status(&self.session_id, SessionStatus::Failed);
                    if let Ok(Some(task)) = self.store.get_task(&self.task_id) {
                        if task.status.can_transition_to(&TaskStatus::Failed) {
                            let _ = self
                                .store
                                .update_task_status(&self.task_id, TaskStatus::Failed);
                        }
                    }
                    return;
                }
            };

            let _ = self.store.set_session_pid(&self.session_id, handle.pid);
            if let Some(ref sid) = handle.executor_sid {
                let _ = self.store.set_executor_sid(&self.session_id, sid);
                executor_sid = Some(sid.clone());
            }
            let _ = write_session_json(
                &self.workspace,
                &self.session_id,
                handle.executor_sid.as_deref(),
                handle.pid,
            );

            let ctx = RelayCtx {
                caller_tx: self.caller_tx.clone(),
                store: self.store.clone(),
                workspace: self.workspace.clone(),
                task_id: self.task_id.clone(),
                session_id: self.session_id.clone(),
                pid: handle.pid,
                runtime: self.runtime.clone(),
            };
            let outcome = persist_turn(event_rx, ctx).await;
            if let Some(sid) = outcome.executor_sid.clone() {
                executor_sid = Some(sid);
            }

            if outcome.denials.is_empty() {
                finish_task(&self, &outcome.result_text, outcome.is_error);
                return;
            }

            let (auto, user) = policy::split_denials(&outcome.denials);
            policy::grant_tools(&mut allowed_tools, auto.into_iter().map(|d| d.tool_name));
            let _ = self
                .store
                .set_session_allowed_tools(&self.session_id, &allowed_tools);

            if user.is_empty() {
                next_is_resume = executor_sid.is_some();
                prompt =
                    "Continue. Previously blocked read-only tools are now allowed.".to_string();
                continue;
            }

            pause_for_approval(&self, user).await;
            return;
        }

        let _ = self
            .caller_tx
            .send(RuntimeEvent::Error {
                message: "too many auto-resume turns without completion".to_string(),
            })
            .await;
        if let Ok(Some(task)) = self.store.get_task(&self.task_id) {
            if task.status.can_transition_to(&TaskStatus::Failed) {
                let _ = self
                    .store
                    .update_task_status(&self.task_id, TaskStatus::Failed);
            }
        }
    }
}

async fn pause_for_approval(runner: &TurnRunner, denials: Vec<PermissionDenial>) {
    if let Ok(Some(task)) = runner.store.get_task(&runner.task_id) {
        if task.status.can_transition_to(&TaskStatus::WaitingUser) {
            let _ = runner
                .store
                .update_task_status(&runner.task_id, TaskStatus::WaitingUser);
        }
    }
    if let Ok(Some(session)) = runner.store.get_session(&runner.session_id) {
        if session
            .status
            .can_transition_to(&SessionStatus::WaitingUser)
        {
            let _ = runner
                .store
                .update_session_status(&runner.session_id, SessionStatus::WaitingUser);
        }
    }

    for denial in denials {
        let id_raw = Uuid::new_v4().to_string().replace('-', "");
        let id = format!("appr_{}", &id_raw[..12]);
        let input_json = denial.tool_input.to_string();
        let approval = Approval {
            id: id.clone(),
            session_id: runner.session_id.clone(),
            task_id: runner.task_id.clone(),
            subject: format!("{} {}", denial.tool_name, input_json),
            tool_name: denial.tool_name.clone(),
            tool_use_id: denial.tool_use_id.clone(),
            tool_input: input_json,
            decision: None,
            actor: None,
            requested_at: Utc::now(),
            resolved_at: None,
        };
        let _ = runner.store.insert_approval(&approval);
        let payload = serde_json::to_string(&approval).unwrap_or_default();
        let _ = runner.store.insert_event(
            &format!("evt_{id}_req"),
            "approval.requested",
            &Utc::now().to_rfc3339(),
            Some(&runner.task_id),
            Some(&runner.session_id),
            &payload,
            None,
        );
        let _ = runner
            .caller_tx
            .send(RuntimeEvent::ApprovalRequested {
                id,
                tool_name: denial.tool_name,
                input: denial.tool_input,
            })
            .await;
    }
}

fn finish_task(runner: &TurnRunner, result_text: &str, is_error: bool) {
    let final_status = if is_error {
        SessionStatus::Failed
    } else {
        SessionStatus::Exited
    };
    let _ = runner
        .store
        .update_session_status(&runner.session_id, final_status);

    if let Ok(Some(task)) = runner.store.get_task(&runner.task_id) {
        if is_error {
            if task.status.can_transition_to(&TaskStatus::Failed) {
                let _ = runner
                    .store
                    .update_task_status(&runner.task_id, TaskStatus::Failed);
            }
        } else if task.status.can_transition_to(&TaskStatus::Reviewing) {
            let _ = runner
                .store
                .update_task_status(&runner.task_id, TaskStatus::Reviewing);
        }
    }

    let _ = save_experience_direct(runner, result_text, is_error);
}

struct RelayCtx {
    caller_tx: mpsc::Sender<RuntimeEvent>,
    store: Arc<Store>,
    workspace: PathBuf,
    task_id: String,
    session_id: String,
    pid: Option<u32>,
    runtime: String,
}

async fn persist_turn(mut event_rx: mpsc::Receiver<RuntimeEvent>, ctx: RelayCtx) -> TurnOutcome {
    let mut seq: i64 = 0;
    let mut result_text = String::new();
    let mut is_error = false;
    let mut denials = Vec::new();
    let mut executor_sid = None;
    let transcript = ctx.workspace.join("transcript/events.jsonl");

    while let Some(event) = event_rx.recv().await {
        seq += 1;
        let evt_id = format!(
            "evt_{}_{seq}",
            &ctx.session_id[..8.min(ctx.session_id.len())]
        );
        let event_type = match &event {
            RuntimeEvent::SessionStarted { .. } => "session.started",
            RuntimeEvent::UserText { .. } => "user.message",
            RuntimeEvent::AssistantText { .. } => "session.output",
            RuntimeEvent::Thinking { .. } => "session.thinking",
            RuntimeEvent::ToolCallStarted { .. } => "session.tool_start",
            RuntimeEvent::ToolCallCompleted { .. } => "session.tool_done",
            RuntimeEvent::TurnCompleted { .. } => "session.turn_done",
            RuntimeEvent::Result { .. } => "session.result",
            RuntimeEvent::ApprovalRequested { .. } => "approval.requested",
            RuntimeEvent::Error { .. } => "session.error",
        };
        let payload = serde_json::to_string(&event).unwrap_or_default();
        let _ = ctx.store.insert_event(
            &evt_id,
            event_type,
            &Utc::now().to_rfc3339(),
            Some(&ctx.task_id),
            Some(&ctx.session_id),
            &payload,
            Some(seq),
        );
        let _ = append_transcript(&transcript, &payload);

        match &event {
            RuntimeEvent::SessionStarted { session_id } => {
                let _ = ctx.store.set_executor_sid(&ctx.session_id, session_id);
                let _ =
                    write_session_json(&ctx.workspace, &ctx.session_id, Some(session_id), ctx.pid);
                executor_sid = Some(session_id.clone());
            }
            RuntimeEvent::Result {
                text,
                is_error: err,
                session_id,
                permission_denials,
                cost_usd,
                usage,
            } => {
                result_text = text.clone();
                is_error = *err;
                denials = permission_denials.clone();
                if let Some(sid) = session_id {
                    let _ = ctx.store.set_executor_sid(&ctx.session_id, sid);
                    executor_sid = Some(sid.clone());
                }
                let delta = UsageDelta::from_result(*cost_usd, usage.as_ref());
                let _ = ctx.store.insert_usage(
                    Some(&ctx.task_id),
                    Some(&ctx.session_id),
                    Some(&ctx.runtime),
                    &delta,
                );
            }
            _ => {}
        }

        if ctx.caller_tx.send(event).await.is_err() {
            break;
        }
    }

    TurnOutcome {
        result_text,
        is_error,
        denials,
        executor_sid,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn append_transcript(path: &std::path::Path, line: &str) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{line}")?;
    Ok(())
}

fn save_experience_direct(
    runner: &TurnRunner,
    result_text: &str,
    is_error: bool,
) -> Result<(), CoreError> {
    let now = Utc::now();
    let exp_id_raw = Uuid::new_v4().to_string().replace('-', "");
    let exp_id = format!("exp_{}", &exp_id_raw[..12]);
    let face = runner
        .face_id
        .clone()
        .unwrap_or_else(|| "general".to_string());
    let rel = format!("faces/{face}/experiences/{exp_id}.md");
    let abs = runner.home.join(&rel);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent)?;
    }

    let outcome = if is_error { "failed" } else { "success" };
    let summary = if result_text.len() > 500 {
        result_text[..500].to_string()
    } else {
        result_text.to_string()
    };

    let body = format!(
        "# Experience `{exp_id}`\n\n\
         - task: `{task}`\n\
         - face: `{face}`\n\
         - outcome: {outcome}\n\
         - created: {ts}\n\n\
         ## Request\n\n\
         {title}\n\n\
         {request}\n\n\
         ## Result\n\n\
         {result}\n",
        task = runner.task_id,
        ts = now.to_rfc3339(),
        title = runner.task_title,
        request = runner.task_request,
        result = result_text,
    );
    fs::write(&abs, &body)?;
    let hash = sha256_hex(body.as_bytes());

    let exp = Experience {
        id: exp_id,
        task_id: runner.task_id.clone(),
        face_id: Some(face),
        path: rel,
        content_hash: hash,
        outcome: Some(outcome.to_string()),
        summary: Some(summary),
        created_at: now,
        updated_at: now,
    };
    runner.store.insert_experience(&exp)?;
    let _ = learning::enqueue_extract(&runner.store, &exp);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use methodus_runtime::{LiveAgent, RuntimeError, SessionHandle};
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use tempfile::tempdir;

    struct MockAdapter {
        turns: Mutex<VecDeque<Vec<RuntimeEvent>>>,
        live: Vec<LiveAgent>,
    }

    impl MockAdapter {
        fn take_turn(&self) -> Vec<RuntimeEvent> {
            self.turns.lock().unwrap().pop_front().unwrap_or_default()
        }
    }

    #[async_trait]
    impl RuntimeAdapter for MockAdapter {
        async fn spawn(
            &self,
            input: SpawnInput,
        ) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError> {
            let (tx, rx) = mpsc::channel(16);
            for event in self.take_turn() {
                let _ = tx.send(event).await;
            }
            drop(tx);
            Ok((
                SessionHandle {
                    session_id: input.session_id,
                    executor_sid: Some("exec-sid-1".to_string()),
                    pid: None,
                },
                rx,
            ))
        }

        async fn resume(
            &self,
            executor_sid: &str,
            input: SpawnInput,
        ) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError> {
            let (handle, rx) = self.spawn(input).await?;
            Ok((
                SessionHandle {
                    session_id: handle.session_id,
                    executor_sid: Some(executor_sid.to_string()),
                    pid: handle.pid,
                },
                rx,
            ))
        }

        async fn stop(&self, _handle: &SessionHandle) -> Result<(), RuntimeError> {
            Ok(())
        }

        async fn list_live_agents(&self) -> Result<Vec<LiveAgent>, RuntimeError> {
            Ok(self.live.clone())
        }

        fn uses_manual_permissions(&self) -> bool {
            true
        }
    }

    fn ok_result(text: &str) -> RuntimeEvent {
        RuntimeEvent::Result {
            is_error: false,
            text: text.to_string(),
            cost_usd: None,
            usage: None,
            session_id: Some("exec-sid-1".to_string()),
            permission_denials: Vec::new(),
        }
    }

    fn engine_with(events: Vec<RuntimeEvent>) -> (Engine, tempfile::TempDir) {
        engine_with_turns(vec![events])
    }

    fn engine_with_turns(turns: Vec<Vec<RuntimeEvent>>) -> (Engine, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("state.db")).unwrap());
        let adapter = Arc::new(MockAdapter {
            turns: Mutex::new(turns.into()),
            live: vec![],
        });
        (Engine::new(store, adapter, dir.path().to_path_buf()), dir)
    }

    #[tokio::test]
    async fn run_task_persists_experience_file_and_completes() {
        let events = vec![
            RuntimeEvent::SessionStarted {
                session_id: "exec-sid-1".to_string(),
            },
            RuntimeEvent::AssistantText {
                text: "hello".to_string(),
            },
            RuntimeEvent::Result {
                is_error: false,
                text: "done".to_string(),
                cost_usd: None,
                usage: None,
                session_id: Some("exec-sid-1".to_string()),
                permission_denials: Vec::new(),
            },
        ];
        let (engine, _dir) = engine_with(events);
        let task = engine.create_task("goal", "goal", None, None).unwrap();
        let mut rx = engine.run_task(&task.id, false).await.unwrap();
        while rx.recv().await.is_some() {}

        let done = engine.store().get_task(&task.id).unwrap().unwrap();
        assert_eq!(done.status, TaskStatus::Reviewing);

        let sessions = engine.store().list_sessions_for_task(&task.id).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].executor_sid.as_deref(), Some("exec-sid-1"));
        assert_eq!(sessions[0].status, SessionStatus::Exited);

        let exps = engine.store().list_experiences().unwrap();
        assert_eq!(exps.len(), 1);
        let abs = engine.home().join(&exps[0].path);
        assert!(abs.exists(), "experience file missing: {}", abs.display());
        let body = fs::read_to_string(&abs).unwrap();
        assert!(body.contains("done"));
    }

    #[tokio::test]
    async fn run_task_records_executor_usage() {
        let events = vec![
            RuntimeEvent::SessionStarted {
                session_id: "exec-sid-1".to_string(),
            },
            RuntimeEvent::Result {
                is_error: false,
                text: "done".to_string(),
                cost_usd: Some(0.04),
                usage: Some(serde_json::json!({"input_tokens": 120, "output_tokens": 30})),
                session_id: Some("exec-sid-1".to_string()),
                permission_denials: Vec::new(),
            },
        ];
        let (engine, _dir) = engine_with(events);
        let task = engine.create_task("goal", "goal", None, None).unwrap();
        let mut rx = engine.run_task(&task.id, false).await.unwrap();
        while rx.recv().await.is_some() {}
        let all = engine.usage_summary(false).unwrap();
        assert_eq!(all.input_tokens, 120);
        assert_eq!(all.output_tokens, 30);
        assert!((all.cost_usd - 0.04).abs() < 1e-9);
        assert_eq!(all.turns, 1);
        let task_u = engine.store().usage_for_task(&task.id).unwrap();
        assert_eq!(task_u.input_tokens, 120);
    }

    #[tokio::test]
    async fn send_turn_resumes_after_reviewing() {
        let (engine, _dir) =
            engine_with_turns(vec![vec![ok_result("first")], vec![ok_result("second")]]);
        let task = engine.create_task("goal", "goal", None, None).unwrap();
        let mut rx = engine.run_task(&task.id, false).await.unwrap();
        while rx.recv().await.is_some() {}

        let mut rx = engine
            .send_turn(&task.id, "change it to methodus")
            .await
            .unwrap();
        while rx.recv().await.is_some() {}

        let done = engine.store().get_task(&task.id).unwrap().unwrap();
        assert_eq!(done.status, TaskStatus::Reviewing);
        let sessions = engine.store().list_sessions_for_task(&task.id).unwrap();
        assert_eq!(sessions.len(), 2);
        let events = engine.store().list_events(Some(&task.id), 80).unwrap();
        assert!(events.iter().any(|e| e.event_type == "user.message"));
    }

    #[tokio::test]
    async fn run_task_materializes_resolved_skill() {
        let events = vec![ok_result("done")];
        let (engine, dir) = engine_with(events);
        fs::create_dir_all(dir.path().join("faces/general")).unwrap();
        fs::write(
            dir.path().join("faces/general/face.yaml"),
            "id: general\nname: General\nintent_tags: [general]\nmethods: [general-software]\nskills: [workspace-hygiene]\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("methods")).unwrap();
        fs::write(
            dir.path().join("methods/general-software.yaml"),
            "id: general-software\nname: General software\nrecommended_skills: [workspace-hygiene]\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("skills/workspace-hygiene")).unwrap();
        fs::write(
            dir.path().join("skills/workspace-hygiene/SKILL.md"),
            "---\nname: workspace-hygiene\ndescription: stay in workspace\n---\n",
        )
        .unwrap();

        let task = engine
            .create_task("hygiene", "keep the workspace clean", None, None)
            .unwrap();
        let res = Resolution::parse_json(task.resolution.as_deref().unwrap()).unwrap();
        assert!(res.skills.iter().any(|s| s.name == "workspace-hygiene"));

        let mut rx = engine.run_task(&task.id, false).await.unwrap();
        while rx.recv().await.is_some() {}

        let skill = dir
            .path()
            .join("workspaces")
            .join(&task.id)
            .join(".claude/skills/workspace-hygiene/SKILL.md");
        assert!(skill.is_file(), "missing {}", skill.display());
        assert!(dir
            .path()
            .join("workspaces")
            .join(&task.id)
            .join(".methodus/method.yaml")
            .is_file());
    }

    #[tokio::test]
    async fn run_task_injects_committed_face_knowledge() {
        let (engine, dir) = engine_with(vec![ok_result("done")]);
        let now = Utc::now();
        let rel = "faces/general/knowledge/latch.md";
        fs::create_dir_all(dir.path().join("faces/general/knowledge")).unwrap();
        fs::write(
            dir.path().join(rel),
            "# Latch protocol\n\nThe latch uses gpio 4.\n",
        )
        .unwrap();
        engine
            .store()
            .insert_knowledge(&KnowledgeItem {
                id: "know_latch".into(),
                face_id: Some("general".into()),
                project_id: None,
                path: rel.into(),
                content_hash: "h".into(),
                source: "experience".into(),
                confidence: Some(0.8),
                scope: None,
                status: KnowledgeStatus::Committed,
                conflict_of: None,
                version: 1,
                created_at: now,
                updated_at: now,
            })
            .unwrap();

        let task = engine
            .create_task("debug latch", "debug the latch gpio", None, None)
            .unwrap();
        let mut rx = engine.run_task(&task.id, false).await.unwrap();
        while rx.recv().await.is_some() {}

        let ctx = fs::read_to_string(
            dir.path()
                .join("workspaces")
                .join(&task.id)
                .join(".methodus/selected-context.md"),
        )
        .unwrap();
        assert!(ctx.contains("gpio 4"), "context missing knowledge: {ctx}");
        assert!(ctx.contains("Face knowledge (committed)"));
        assert!(dir
            .path()
            .join("workspaces")
            .join(&task.id)
            .join("face-context/knowledge/latch.md")
            .is_file());
    }

    #[tokio::test]
    async fn run_task_injects_pack_knowledge() {
        let (engine, dir) = engine_with(vec![ok_result("done")]);
        let pack = dir.path().join("team-pack");
        fs::create_dir_all(pack.join("knowledge")).unwrap();
        fs::write(pack.join("pack.yaml"), "id: team-x\nname: Team X\n").unwrap();
        fs::write(
            pack.join("knowledge/latch.md"),
            "# Latch protocol\n\nThe latch uses gpio 4.\n",
        )
        .unwrap();
        crate::pack::add_pack(dir.path(), &pack).unwrap();

        let task = engine
            .create_task("debug latch", "debug the latch gpio", None, None)
            .unwrap();
        let mut rx = engine.run_task(&task.id, false).await.unwrap();
        while rx.recv().await.is_some() {}

        let ctx = fs::read_to_string(
            dir.path()
                .join("workspaces")
                .join(&task.id)
                .join(".methodus/selected-context.md"),
        )
        .unwrap();
        assert!(ctx.contains("gpio 4"), "missing pack knowledge: {ctx}");
        assert!(ctx.contains("team:team-x"));
        assert!(dir
            .path()
            .join("workspaces")
            .join(&task.id)
            .join("face-context/knowledge/latch.md")
            .is_file());
    }

    #[tokio::test]
    async fn workspace_root_honors_config_yaml() {
        let events = vec![ok_result("done")];
        let (engine, dir) = engine_with(events);
        drop(engine);
        let runs = dir.path().join("custom-runs");
        fs::write(
            dir.path().join("config.yaml"),
            format!("workspace_root: {}\n", runs.display()),
        )
        .unwrap();
        let store = Arc::new(Store::open(&dir.path().join("state.db")).unwrap());
        let adapter = Arc::new(MockAdapter {
            turns: Mutex::new(vec![vec![ok_result("done")]].into()),
            live: vec![],
        });
        let engine = Engine::new(store, adapter, dir.path().to_path_buf());
        assert_eq!(engine.workspace_root(), runs);
        let task = engine.create_task("g", "g", None, None).unwrap();
        let mut rx = engine.run_task(&task.id, false).await.unwrap();
        while rx.recv().await.is_some() {}
        assert!(runs
            .join(&task.id)
            .join(".methodus/selected-context.md")
            .is_file());
        assert!(!dir.path().join("workspaces").join(&task.id).exists());
    }

    #[tokio::test]
    async fn recover_marks_dead_running_session_interrupted() {
        let (engine, _dir) = engine_with(vec![]);
        let task = engine.create_task("g", "g", None, None).unwrap();
        engine
            .store()
            .update_task_status(&task.id, TaskStatus::Planning)
            .unwrap();
        engine
            .store()
            .update_task_status(&task.id, TaskStatus::Running)
            .unwrap();

        let now = Utc::now();
        engine
            .store()
            .insert_session(&Session {
                id: "sess-1".to_string(),
                task_id: task.id.clone(),
                runtime: "claude-code".to_string(),
                executor_sid: Some("exec-sid-1".to_string()),
                transport: "subprocess".to_string(),
                pid: Some(u32::MAX - 7),
                cwd: "/tmp".to_string(),
                status: SessionStatus::Running,
                last_turn: None,
                started_at: now,
                ended_at: None,
                updated_at: now,
            })
            .unwrap();

        let rec = engine.recover().await.unwrap();
        assert_eq!(rec.len(), 1);
        assert!(!rec[0].still_live);

        let session = engine.store().get_session("sess-1").unwrap().unwrap();
        assert_eq!(session.status, SessionStatus::Interrupted);
        let task = engine.store().get_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::WaitingUser);
    }

    #[tokio::test]
    async fn run_without_resume_after_interrupt_asks_for_flag() {
        let (engine, _dir) = engine_with(vec![]);
        let task = engine.create_task("g", "g", None, None).unwrap();
        let now = Utc::now();
        engine
            .store()
            .insert_session(&Session {
                id: "sess-1".to_string(),
                task_id: task.id.clone(),
                runtime: "claude-code".to_string(),
                executor_sid: Some("exec-sid-1".to_string()),
                transport: "subprocess".to_string(),
                pid: None,
                cwd: "/tmp".to_string(),
                status: SessionStatus::Interrupted,
                last_turn: None,
                started_at: now,
                ended_at: Some(now),
                updated_at: now,
            })
            .unwrap();

        let err = engine.run_task(&task.id, false).await.unwrap_err();
        assert!(matches!(err, CoreError::NeedsResume));
    }

    #[tokio::test]
    async fn run_with_resume_starts_new_session_row() {
        let events = vec![
            RuntimeEvent::SessionStarted {
                session_id: "exec-sid-1".to_string(),
            },
            RuntimeEvent::Result {
                is_error: false,
                text: "continued".to_string(),
                cost_usd: None,
                usage: None,
                session_id: Some("exec-sid-1".to_string()),
                permission_denials: Vec::new(),
            },
        ];
        let (engine, _dir) = engine_with(events);
        let task = engine.create_task("g", "g", None, None).unwrap();
        let now = Utc::now();
        engine
            .store()
            .insert_session(&Session {
                id: "sess-old".to_string(),
                task_id: task.id.clone(),
                runtime: "claude-code".to_string(),
                executor_sid: Some("exec-sid-1".to_string()),
                transport: "subprocess".to_string(),
                pid: None,
                cwd: "/tmp".to_string(),
                status: SessionStatus::Interrupted,
                last_turn: None,
                started_at: now,
                ended_at: Some(now),
                updated_at: now,
            })
            .unwrap();

        let mut rx = engine.run_task(&task.id, true).await.unwrap();
        while rx.recv().await.is_some() {}

        let sessions = engine.store().list_sessions_for_task(&task.id).unwrap();
        assert_eq!(sessions.len(), 2);
        let newest = sessions
            .iter()
            .find(|s| s.id != "sess-old")
            .expect("new session row");
        assert_eq!(newest.executor_sid.as_deref(), Some("exec-sid-1"));
        assert_eq!(newest.status, SessionStatus::Exited);
        let done = engine.store().get_task(&task.id).unwrap().unwrap();
        assert_eq!(done.status, TaskStatus::Reviewing);
    }

    #[tokio::test]
    async fn write_denial_pauses_for_approval_then_approve_continues() {
        let deny = vec![
            RuntimeEvent::SessionStarted {
                session_id: "exec-sid-1".to_string(),
            },
            RuntimeEvent::Result {
                is_error: false,
                text: "need write".to_string(),
                cost_usd: None,
                usage: None,
                session_id: Some("exec-sid-1".to_string()),
                permission_denials: vec![PermissionDenial {
                    tool_name: "Write".to_string(),
                    tool_use_id: Some("tu1".to_string()),
                    tool_input: serde_json::json!({"file_path": "/tmp/x"}),
                }],
            },
        ];
        let (engine, _dir) = engine_with_turns(vec![deny, vec![ok_result("wrote it")]]);
        let task = engine
            .create_task("write a file", "write a file", None, None)
            .unwrap();
        let mut rx = engine.run_task(&task.id, false).await.unwrap();
        let mut saw_approval = None;
        while let Some(ev) = rx.recv().await {
            if let RuntimeEvent::ApprovalRequested { id, tool_name, .. } = ev {
                assert_eq!(tool_name, "Write");
                saw_approval = Some(id);
            }
        }
        let appr_id = saw_approval.expect("approval event");
        let task = engine.store().get_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::WaitingUser);

        let mut rx = engine
            .approve(&appr_id, ApprovalDecision::Once, "user")
            .await
            .unwrap();
        while rx.recv().await.is_some() {}
        let task = engine.store().get_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Reviewing);
        let appr = engine.store().get_approval(&appr_id).unwrap().unwrap();
        assert_eq!(appr.decision.as_deref(), Some("once"));
    }

    #[tokio::test]
    async fn deny_abort_cancels_task() {
        let deny = vec![RuntimeEvent::Result {
            is_error: false,
            text: "need bash".to_string(),
            cost_usd: None,
            usage: None,
            session_id: Some("exec-sid-1".to_string()),
            permission_denials: vec![PermissionDenial {
                tool_name: "Bash".to_string(),
                tool_use_id: None,
                tool_input: serde_json::json!({"command": "rm -rf /"}),
            }],
        }];
        let (engine, _dir) = engine_with(deny);
        let task = engine.create_task("g", "g", None, None).unwrap();
        let mut rx = engine.run_task(&task.id, false).await.unwrap();
        let mut appr_id = None;
        while let Some(ev) = rx.recv().await {
            if let RuntimeEvent::ApprovalRequested { id, .. } = ev {
                appr_id = Some(id);
            }
        }
        engine
            .approve(&appr_id.unwrap(), ApprovalDecision::Abort, "user")
            .await
            .unwrap();
        let task = engine.store().get_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Cancelled);
    }

    #[tokio::test]
    async fn auto_allow_read_does_not_pause() {
        let first = vec![RuntimeEvent::Result {
            is_error: false,
            text: "need read".to_string(),
            cost_usd: None,
            usage: None,
            session_id: Some("exec-sid-1".to_string()),
            permission_denials: vec![PermissionDenial {
                tool_name: "Read".to_string(),
                tool_use_id: None,
                tool_input: serde_json::json!({"path": "/tmp/a"}),
            }],
        }];
        let (engine, _dir) = engine_with_turns(vec![first, vec![ok_result("ok")]]);
        let task = engine.create_task("g", "g", None, None).unwrap();
        let mut rx = engine.run_task(&task.id, false).await.unwrap();
        while rx.recv().await.is_some() {}
        let task = engine.store().get_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Reviewing);
        assert!(engine
            .store()
            .list_pending_approvals(Some(&task.id))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn cancel_queued_task() {
        let (engine, _dir) = engine_with(vec![]);
        let task = engine.create_task("g", "g", None, None).unwrap();
        engine.cancel_task(&task.id).unwrap();
        let task = engine.store().get_task(&task.id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Cancelled);
        assert!(engine.cancel_task(&task.id).is_err());
    }

    #[tokio::test]
    async fn task_complete_enqueues_learning_jobs() {
        let (engine, _dir) = engine_with(vec![ok_result("unknown: latch protocol")]);
        let task = engine.create_task("g", "g", None, None).unwrap();
        let mut rx = engine.run_task(&task.id, false).await.unwrap();
        while rx.recv().await.is_some() {}
        let jobs = engine.store().list_jobs().unwrap();
        assert!(jobs
            .iter()
            .any(|j| j.kind == JobKind::ExtractExperience && j.status == JobStatus::Queued));
    }

    #[tokio::test]
    async fn learning_repeated_unknown_question_answer_and_conflict() {
        let (engine, dir) = engine_with_turns(vec![
            vec![ok_result("unknown: latch protocol\nuse gpio 4")],
            vec![ok_result("unknown: latch protocol\nuse gpio 7")],
        ]);
        fs::create_dir_all(dir.path().join("faces/general/knowledge")).unwrap();

        let t1 = engine.create_task("one", "one", None, None).unwrap();
        let mut rx = engine.run_task(&t1.id, false).await.unwrap();
        while rx.recv().await.is_some() {}
        engine.tick_learning().unwrap();

        let qs = engine.store().list_questions(None).unwrap();
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].frequency, 1.0);
        let cands = engine
            .store()
            .list_knowledge(Some(KnowledgeStatus::Candidate))
            .unwrap()
            .into_iter()
            .filter(|k| k.source != crate::learning::SKILL_DRAFT_SOURCE)
            .collect::<Vec<_>>();
        assert!(!cands.is_empty());
        let committed = engine.review_knowledge(&cands[0].id, true).unwrap();
        assert_eq!(committed.status, KnowledgeStatus::Committed);
        let committed_body = fs::read_to_string(engine.home().join(&committed.path)).unwrap();
        assert!(committed_body.contains("gpio 4"));

        let t2 = engine.create_task("two", "two", None, None).unwrap();
        let mut rx = engine.run_task(&t2.id, false).await.unwrap();
        while rx.recv().await.is_some() {}
        engine.tick_learning().unwrap();

        let qs = engine.store().list_questions(None).unwrap();
        assert_eq!(qs.len(), 1);
        assert!(qs[0].frequency >= 2.0);
        let conflicts = engine
            .store()
            .list_knowledge(Some(KnowledgeStatus::Conflicted))
            .unwrap();
        assert!(!conflicts.is_empty());
        let still = fs::read_to_string(engine.home().join(&committed.path)).unwrap();
        assert!(still.contains("gpio 4"));
        assert!(!still.contains("gpio 7"));

        let answered = engine
            .answer_question(&qs[0].id, "the latch uses 3.3V pull-up")
            .unwrap();
        assert_eq!(answered.status, QuestionStatus::Answered);
        let from_answer = engine
            .store()
            .list_knowledge(None)
            .unwrap()
            .into_iter()
            .find(|k| k.source == "user_answer")
            .expect("candidate from answer");
        assert_eq!(from_answer.status, KnowledgeStatus::Candidate);
    }

    fn tool_start(name: &str) -> RuntimeEvent {
        RuntimeEvent::ToolCallStarted {
            id: name.to_string(),
            name: name.to_string(),
            input: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn learn_skill_then_review_installs_live_skill() {
        let (engine, _dir) = engine_with(vec![
            tool_start("Bash"),
            tool_start("Read"),
            tool_start("Grep"),
            ok_result("sampled"),
        ]);
        let task = engine
            .create_task("sample cpu", "sample cpu of nginx", None, None)
            .unwrap();
        let mut rx = engine.run_task(&task.id, false).await.unwrap();
        while rx.recv().await.is_some() {}

        engine.tick_learning().unwrap();
        let auto_drafts: Vec<_> = engine
            .store()
            .list_knowledge(Some(KnowledgeStatus::Candidate))
            .unwrap()
            .into_iter()
            .filter(|k| k.source == crate::learning::SKILL_DRAFT_SOURCE)
            .collect();
        assert!(
            !auto_drafts.is_empty(),
            "expected propose_skill draft after 3 tool calls"
        );

        let explicit = engine.learn_skill(&task.id, Some("cpu-sample")).unwrap();
        assert_eq!(explicit.source, crate::learning::SKILL_DRAFT_SOURCE);
        assert!(explicit.path.contains(".candidates"));

        let committed = engine.review_knowledge(&explicit.id, true).unwrap();
        assert_eq!(committed.status, KnowledgeStatus::Committed);
        assert!(committed.path.starts_with("skills/"));
        assert!(!committed.path.contains(".candidates"));
        assert!(engine.home().join(&committed.path).exists());
        let catalog = crate::resolution::scan_skills(engine.home());
        assert!(catalog.iter().any(|s| s.name.contains("cpu-sample")));

        let again = engine.learn_skill(&task.id, Some("cpu-sample")).unwrap();
        assert_eq!(again.status, KnowledgeStatus::Conflicted);
        assert!(engine.review_knowledge(&again.id, true).is_err());
        let still = engine.store().get_knowledge(&again.id).unwrap().unwrap();
        assert_eq!(still.status, KnowledgeStatus::Conflicted);
    }

    #[test]
    fn recover_requeues_running_learning_job() {
        let (engine, _dir) = engine_with(vec![]);
        let now = Utc::now();
        engine
            .store()
            .enqueue_job(&LearningJob {
                id: "job_stuck".to_string(),
                kind: JobKind::DetectGaps,
                priority: 1,
                dedupe_key: Some("detect:x".to_string()),
                input_refs: "{}".to_string(),
                status: JobStatus::Running,
                attempts: 1,
                not_before: None,
                budget: None,
                requires_approval: false,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
        let n = crate::scheduler::recover_jobs(engine.store()).unwrap();
        assert_eq!(n, 1);
        let job = engine.store().get_job("job_stuck").unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Queued);
    }
}
