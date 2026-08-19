use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
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
use crate::workspace::{CapsuleSelection, CapsuleSpec, WorkspaceBuilder};

const MAX_AUTO_TURNS: u32 = 12;

fn graph_score(node: &GraphNode, request: &str) -> f64 {
    let haystack = format!("{} {} {}", node.title, node.summary.as_deref().unwrap_or(""), node.scope.as_deref().unwrap_or("")).to_ascii_lowercase();
    let matches = request.split(|c: char| !c.is_alphanumeric())
        .filter(|term| term.len() >= 3)
        .filter(|term| haystack.contains(*term))
        .count() as f64;
    matches + node.confidence.unwrap_or(0.0)
}

fn native_command(runtime: &str, brief: &str) -> (String, Vec<String>) {
    match runtime {
        "claude-code" => ("claude".into(), vec![brief.into()]),
        "codex" => ("codex".into(), vec![brief.into()]),
        "cursor" => ("cursor".into(), vec!["agent".into(), brief.into()]),
        other => (other.into(), vec![brief.into()]),
    }
}

fn yaml_quote(value: &str) -> String {
    value.replace('"', "'").replace('\n', " ").trim().to_string()
}

/// User decision on a candidate / conflicted knowledge or skill draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnowledgeReviewAction {
    /// Promote to committed (install skill when path is free).
    Commit,
    /// Discard the candidate.
    Reject,
    /// Overwrite an existing live skill (conflicted drafts only).
    ReplaceExisting,
}

#[derive(Clone)]
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

/// A native interactive launch prepared by Methodus. The caller owns terminal
/// suspension/pane creation; no Agent TUI output is parsed or proxied.
#[derive(Debug, Clone)]
pub struct NativeHandoffPlan {
    pub task_id: String,
    pub runtime: String,
    pub cwd: PathBuf,
    pub program: String,
    pub args: Vec<String>,
    pub capsule_root: PathBuf,
    pub brief: String,
}

/// Result of the short-lived, read-only context-planning Agent call that precedes
/// native handoff. The planner never executes the user's task.
#[derive(Debug, Clone)]
pub struct ContextPlan {
    pub selected_node_ids: Vec<String>,
    pub rationale: String,
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

    /// Re-index independent Markdown graph nodes. This is deliberately separate from
    /// the legacy Face catalog and can run before every graph/task-control refresh.
    pub fn sync_graph(&self) -> Result<usize, CoreError> {
        crate::graph::sync_graph(&self.store, &self.home)
    }

    pub fn list_graph_nodes(&self, query: Option<&str>) -> Result<Vec<GraphNode>, CoreError> {
        Ok(self.store.list_graph_nodes(query)?)
    }

    pub fn graph_edges_for(&self, node_id: &str) -> Result<Vec<GraphEdge>, CoreError> {
        Ok(self.store.graph_edges_for(node_id)?)
    }

    /// Compile a compact graph-backed Task Workspace without starting a runtime.
    /// This is the default pre-launch step for both native handoff and optional managed
    /// execution. The user's project remains the runtime cwd.
    pub fn compile_capsule(&self, task_id: &str, context_budget_tokens: i64) -> Result<NativeHandoffPlan, CoreError> {
        self.compile_capsule_with_nodes(task_id, context_budget_tokens, &[])
    }

    /// Compile against explicit planner choices. An empty list retains the local
    /// deterministic ranking as an offline fallback.
    pub fn compile_capsule_with_nodes(&self, task_id: &str, context_budget_tokens: i64, preferred_node_ids: &[String]) -> Result<NativeHandoffPlan, CoreError> {
        let task = self.store.get_task(task_id)?
            .ok_or_else(|| CoreError::TaskNotFound(task_id.to_string()))?;
        self.sync_graph()?;
        let runtime = self.preferred_runtime(task.runtime.as_deref());
        let launch_cwd = task.project_id.as_ref()
            .and_then(|id| crate::project::list_projects(&self.home).into_iter().find(|project| project.id == *id))
            .map(|project| project.root)
            .unwrap_or_else(|| self.launch_cwd.clone());
        let roots = self.context_roots().into_iter().map(|(_, path)| path).collect::<Vec<_>>();
        let (request, _) = crate::mentions::prepare_prompt(&task.request, &roots);
        let request_terms = request.to_ascii_lowercase();
        let mut candidates = self.store.list_graph_nodes(None)?;
        candidates.sort_by(|left, right| graph_score(right, &request_terms).total_cmp(&graph_score(left, &request_terms)));
        let mut selections = Vec::new();
        for node in candidates.into_iter()
            .filter(|node| node.node_type == "knowledge" || node.node_type == "skill" || node.node_type == "experience")
            .filter(|node| preferred_node_ids.is_empty() || preferred_node_ids.iter().any(|id| id == &node.id))
            .take(12) {
            let source = self.home.join(&node.path);
            let Ok(document) = crate::graph::read_graph_document(&self.home, &source) else { continue };
            let content = crate::graph::facet(&document.body, "Execute")
                .or_else(|| node.summary.clone())
                .unwrap_or_else(|| document.body.lines().take(8).collect::<Vec<_>>().join("\n"));
            if content.trim().is_empty() { continue; }
            let priority = graph_score(&node, &request_terms);
            selections.push(CapsuleSelection {
                node_id: node.id.clone(), title: node.title.clone(), facet: "Execute".into(),
                content, reference_path: source, rationale: format!("graph match score {:.1}", priority), priority,
            });
        }
        let skill_files = selections.iter()
            .filter(|selection| selection.node_id.starts_with("skill/"))
            .filter_map(|selection| selection.reference_path.parent().map(|parent| (selection.title.clone(), parent.join("SKILL.md"))))
            .filter(|(_, path)| path.is_file())
            .collect();
        let spec = CapsuleSpec {
            task_id: task.id.clone(), title: task.title.clone(), request,
            runtime: runtime.clone(), launch_cwd: launch_cwd.clone(), context_budget_tokens,
            selections: selections.clone(), skills: skill_files,
        };
        let compiled = WorkspaceBuilder::build_capsule(&self.workspace_root(), &spec)?;
        let workspace_id = format!("ws_{}", task.id);
        let now = Utc::now();
        self.store.insert_task_workspace(&TaskWorkspace {
            id: workspace_id.clone(), task_id: task.id.clone(), root_path: compiled.root.display().to_string(),
            launch_cwd: launch_cwd.display().to_string(), status: "compiled".into(),
            manifest_hash: compiled.manifest_hash, context_budget_tokens, created_at: now, updated_at: now,
        })?;
        self.store.update_task_workspace(&task.id, &workspace_id)?;
        let mut consumed = 0i64;
        let selected = selections.into_iter().map(|selection| {
            let estimate = crate::graph::estimated_tokens(&selection.content);
            let disposition = if consumed + estimate <= context_budget_tokens {
                consumed += estimate;
                "injected"
            } else { "lazy" };
            ContextSelection {
                id: format!("ctx_{}", Uuid::new_v4()), workspace_id: workspace_id.clone(), node_id: selection.node_id,
                facet: selection.facet, rationale: selection.rationale, priority: Some(selection.priority),
                estimated_tokens: estimate, disposition: disposition.into(), outcome: None, created_at: now, updated_at: now,
            }
        }).collect::<Vec<_>>();
        self.store.replace_context_selections(&workspace_id, &selected)?;
        self.emit_simple("workspace.compiled", Some(&task.id), &serde_json::json!({
            "workspace_id": workspace_id, "path": compiled.root, "estimated_tokens": compiled.estimated_tokens,
            "budget_tokens": context_budget_tokens,
        }).to_string());
        let (program, args) = native_command(&runtime, &compiled.brief);
        Ok(NativeHandoffPlan { task_id: task.id, runtime, cwd: launch_cwd, program, args, capsule_root: compiled.root, brief: compiled.brief })
    }

    /// Ask a disposable read-only runtime session to select graph node IDs. The
    /// response is intentionally a tiny JSON contract; file reads and task work
    /// remain outside this planning call. If it cannot provide valid selections,
    /// callers can safely fall back to `compile_capsule`'s local ranker.
    pub async fn plan_context(&self, task_id: &str) -> Result<ContextPlan, CoreError> {
        let task = self.store.get_task(task_id)?.ok_or_else(|| CoreError::TaskNotFound(task_id.to_string()))?;
        self.sync_graph()?;
        let mut nodes = self.store.list_graph_nodes(None)?;
        nodes.retain(|node| matches!(node.node_type.as_str(), "knowledge" | "skill" | "experience"));
        nodes.sort_by(|a, b| graph_score(b, &task.request).total_cmp(&graph_score(a, &task.request)));
        nodes.truncate(24);
        let catalog = nodes.iter().map(|node| format!("- {} | {} | {}", node.id, node.node_type, node.summary.as_deref().unwrap_or(""))).collect::<Vec<_>>().join("\n");
        let prompt = format!(
            "You are Methodus's disposable context planner. Do not solve the task, use tools, or explain the task. Select at most 8 relevant IDs from the catalog for the goal. Return ONLY JSON: {{\"selected_node_ids\":[...],\"rationale\":\"...\"}}.\n\nGoal:\n{}\n\nCatalog:\n{}",
            task.request, catalog
        );
        let runtime = self.preferred_runtime(task.runtime.as_deref());
        let adapter = self.adapter(&runtime)?;
        let cwd = self.workspace_root().join("_context_planner");
        fs::create_dir_all(&cwd)?;
        let (handle, rx) = adapter.spawn(SpawnInput {
            prompt, cwd, session_id: format!("planner_{}", Uuid::new_v4()), permission_mode: "plan".into(),
            allowed_tools: Vec::new(), sandbox: Some("read-only".into()), extra_dirs: Vec::new(), model: None,
        }).await?;
        let text = tokio::time::timeout(Duration::from_secs(45), collect_refine_text(rx)).await
            .map_err(|_| CoreError::Other("context planner timed out".into()))?;
        let _ = adapter.stop(&handle).await;
        let value: serde_json::Value = serde_json::from_str(text.trim())
            .map_err(|_| CoreError::Other("context planner returned invalid JSON".into()))?;
        let valid = nodes.iter().map(|node| node.id.as_str()).collect::<std::collections::HashSet<_>>();
        let selected_node_ids = value.get("selected_node_ids").and_then(serde_json::Value::as_array)
            .into_iter().flatten().filter_map(serde_json::Value::as_str)
            .filter(|id| valid.contains(*id)).take(8).map(str::to_string).collect::<Vec<_>>();
        if selected_node_ids.is_empty() { return Err(CoreError::Other("context planner selected no usable graph nodes".into())); }
        Ok(ContextPlan { selected_node_ids, rationale: value.get("rationale").and_then(serde_json::Value::as_str).unwrap_or("runtime-selected graph context").to_string() })
    }

    /// Persist the fact that the user was handed into their native Agent TUI. The
    /// caller must use `NativeHandoffPlan` with an inherited terminal; Methodus does
    /// not capture or interpret that UI.
    pub fn record_native_handoff(&self, plan: &NativeHandoffPlan) -> Result<String, CoreError> {
        if let Some(task) = self.store.get_task(&plan.task_id)? {
            match task.status {
                TaskStatus::Queued => self.transition_task(&task, TaskStatus::Planning)?,
                TaskStatus::Planning | TaskStatus::Running => {}
                // A returned task can be continued from the Sessions panel.
                // Review is the task's outcome-capture phase, not a terminal
                // state and never a session lifecycle state.
                TaskStatus::Reviewing => self.transition_task(&task, TaskStatus::Running)?,
                other => return Err(CoreError::TaskNotRunnable(plan.task_id.clone(), other.to_string())),
            }
        }
        let task = self.store.get_task(&plan.task_id)?.ok_or_else(|| CoreError::TaskNotFound(plan.task_id.clone()))?;
        if task.status == TaskStatus::Planning {
            self.transition_task(&task, TaskStatus::Running)?;
        }
        let command = std::iter::once(plan.program.as_str()).chain(plan.args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ");
        let id = self.store.record_launch(&plan.task_id, &plan.runtime, "native_handoff", &command)?;
        self.emit_simple("launch.started", Some(&plan.task_id), &serde_json::json!({"launch_id": id, "runtime": plan.runtime, "cwd": plan.cwd, "capsule": plan.capsule_root}).to_string());
        Ok(id)
    }

    /// Return from native Agent interaction into Methodus's review surface. The exit
    /// code is lifecycle evidence only; it never decides whether learned knowledge is
    /// trustworthy or whether the task actually met its acceptance criteria.
    pub fn record_native_return(&self, launch_id: &str, task_id: &str, exit_status: &str) -> Result<(), CoreError> {
        self.store.complete_launch(launch_id, exit_status)?;
        if let Some(task) = self.store.get_task(task_id)? {
            if task.status == TaskStatus::Running {
                self.transition_task(&task, TaskStatus::Reviewing)?;
            }
        }
        self.emit_simple("launch.returned", Some(task_id), &serde_json::json!({"launch_id": launch_id, "exit_status": exit_status}).to_string());
        Ok(())
    }

    /// Drain due learning jobs (extract → detect → propose). Budgeted, no LLM.
    pub fn tick_learning(&self) -> Result<usize, CoreError> {
        scheduler::tick(&self.store, &self.home)
    }

    /// One budgeted executor call to polish a rules note/patch. Never applies.
    pub async fn tick_refine_llm(&self) -> Result<usize, CoreError> {
        let cfg = UserConfig::load(&self.home);
        if !cfg.refine_llm_enabled() {
            return Ok(0);
        }
        if self.has_live_executor_session()? {
            return Ok(0);
        }
        let (used, skip_ids) = self.refine_llm_today()?;
        if used >= cfg.refine_llm_daily_cap() {
            return Ok(0);
        }
        let Some(item) =
            crate::refine::next_unpolished_candidate(&self.store, &self.home, &skip_ids)?
        else {
            return Ok(0);
        };
        match self.polish_one_candidate(&item).await {
            Ok(n) => Ok(n),
            Err(e) => {
                self.emit_refine_llm(&item.id, false, Some(&e.to_string()));
                Ok(0)
            }
        }
    }

    fn has_live_executor_session(&self) -> Result<bool, CoreError> {
        Ok(self.store.list_non_terminal_sessions()?.iter().any(|s| {
            matches!(s.status, SessionStatus::Running | SessionStatus::Spawning)
        }))
    }

    fn refine_llm_today(&self) -> Result<(i64, Vec<String>), CoreError> {
        let today = Utc::now().date_naive();
        let events = self.store.list_events(None, 2000)?;
        let mut ids = Vec::new();
        let mut n = 0i64;
        for ev in events {
            if ev.event_type != crate::refine::REFINE_LLM_EVENT {
                continue;
            }
            if !event_on_date(&ev.occurred_at, today) {
                continue;
            }
            n += 1;
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&ev.payload) {
                if let Some(id) = v.get("knowledge_id").and_then(|x| x.as_str()) {
                    ids.push(id.to_string());
                }
            }
        }
        Ok((n, ids))
    }

    fn emit_refine_llm(&self, knowledge_id: &str, skip: bool, error: Option<&str>) {
        let mut payload = serde_json::json!({
            "knowledge_id": knowledge_id,
            "skip": skip,
        });
        if let Some(err) = error {
            payload["error"] = serde_json::Value::String(err.to_string());
        }
        self.emit_simple(crate::refine::REFINE_LLM_EVENT, None, &payload.to_string());
    }

    async fn polish_one_candidate(&self, item: &KnowledgeItem) -> Result<usize, CoreError> {
        let body = fs::read_to_string(self.home.join(&item.path)).unwrap_or_default();
        let proposal = crate::refine::parse_proposal(&body).ok_or_else(|| {
            CoreError::Other("refine llm: candidate missing proposal JSON".into())
        })?;
        let task_id = proposal
            .evidence_refs
            .first()
            .cloned()
            .unwrap_or_default();
        let title = self
            .store
            .get_task(&task_id)
            .ok()
            .flatten()
            .map(|t| t.title)
            .unwrap_or_else(|| proposal.target_id.clone());
        let digest = crate::refine::trajectory_digest(&self.store, &task_id, &title);
        let draft_json = serde_json::to_string_pretty(&proposal).unwrap_or_else(|_| "{}".into());
        let prompt = crate::refine::polish_prompt(&digest, &draft_json);
        let runtime = self.preferred_runtime(None);
        let adapter = self.adapter(&runtime)?;
        let cwd = self.workspace_root().join("_refine_llm");
        fs::create_dir_all(&cwd)?;
        let session_id = Uuid::new_v4().to_string();
        let (handle, rx) = adapter
            .spawn(SpawnInput {
                prompt,
                cwd,
                session_id,
                permission_mode: "plan".into(),
                allowed_tools: Vec::new(),
                sandbox: Some("read-only".into()),
                extra_dirs: Vec::new(),
                model: None,
            })
            .await?;
        let collected = tokio::time::timeout(Duration::from_secs(45), collect_refine_text(rx)).await;
        let _ = adapter.stop(&handle).await;
        let text = match collected {
            Ok(s) => s,
            Err(_) => {
                self.emit_refine_llm(&item.id, false, Some("timeout"));
                return Ok(0);
            }
        };
        if text.trim().is_empty() {
            self.emit_refine_llm(&item.id, false, Some("empty"));
            return Ok(0);
        }
        let Some(out) = crate::refine::parse_llm_refine_output(&text) else {
            self.emit_refine_llm(&item.id, false, Some("parse"));
            return Ok(0);
        };
        if out.skip {
            let _ = self.review_knowledge(&item.id, KnowledgeReviewAction::Reject);
            self.emit_refine_llm(&item.id, true, None);
            return Ok(1);
        }
        crate::refine::apply_llm_polish(&self.store, &self.home, item, &out)?;
        self.emit_refine_llm(&item.id, false, None);
        Ok(1)
    }

    pub fn list_learning_jobs(&self) -> Result<Vec<methodus_domain::LearningJob>, CoreError> {
        Ok(self.store.list_jobs()?)
    }

    pub fn cancel_learning_job(&self, id: &str) -> Result<bool, CoreError> {
        Ok(self.store.cancel_learning_job(id)?)
    }

    pub fn list_recent_events(&self, limit: usize) -> Result<Vec<methodus_store::EventRecord>, CoreError> {
        Ok(self.store.list_events(None, limit)?)
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

    pub fn review_knowledge(
        &self,
        id: &str,
        action: KnowledgeReviewAction,
    ) -> Result<KnowledgeItem, CoreError> {
        let mut item = self
            .store
            .get_knowledge(id)?
            .ok_or_else(|| CoreError::KnowledgeNotFound(id.to_string()))?;
        if action == KnowledgeReviewAction::Commit && item.source == learning::SKILL_DRAFT_SOURCE {
            match learning::install_skill_draft(&self.home, &item, false) {
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
        if action == KnowledgeReviewAction::Commit || action == KnowledgeReviewAction::ReplaceExisting
        {
            if item.source == crate::refine::SKILL_PATCH_SOURCE {
                item.path = crate::refine::apply_skill_patch(&self.home, &item)?;
            }
            if item.source == crate::refine::HARNESS_NOTE_SOURCE {
                let (live, _hits) =
                    crate::refine::apply_harness_note(&self.store, &self.home, &item)?;
                item.path = live;
            }
        }
        if action == KnowledgeReviewAction::ReplaceExisting {
            if item.source != learning::SKILL_DRAFT_SOURCE
                && item.source != crate::refine::SKILL_PATCH_SOURCE
            {
                return Err(CoreError::Other(
                    "replace only applies to skill drafts or patches".into(),
                ));
            }
            if item.source == learning::SKILL_DRAFT_SOURCE {
                item.path = learning::install_skill_draft(&self.home, &item, true)?;
            }
        }
        let next = match action {
            KnowledgeReviewAction::Commit | KnowledgeReviewAction::ReplaceExisting => {
                KnowledgeStatus::Committed
            }
            KnowledgeReviewAction::Reject => KnowledgeStatus::Rejected,
        };
        let committed = next == KnowledgeStatus::Committed;
        item.status = item.status.checked_transition(next)?;
        item.updated_at = Utc::now();
        self.store.update_knowledge(&item)?;
        let ev = match action {
            KnowledgeReviewAction::Reject => "knowledge.rejected",
            KnowledgeReviewAction::ReplaceExisting => "knowledge.replaced",
            KnowledgeReviewAction::Commit => "knowledge.committed",
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
        if committed {
            if item.source == learning::SKILL_DRAFT_SOURCE {
                if let (Some(fid), Some(skill)) = (
                    item.face_id.as_deref(),
                    crate::evolution::skill_name_from_path(&item.path),
                ) {
                    let _ = crate::evolution::maybe_propose_skill_evolution(
                        &self.store,
                        &self.home,
                        fid,
                        &skill,
                    );
                }
            }
            if item.source == learning::MODULE_STUDY_SOURCE {
                if let Some(fid) = item.face_id.as_deref() {
                    let _ = crate::evolution::maybe_propose_face_evolution(
                        &self.store,
                        &self.home,
                        fid,
                    );
                }
            }
            if let Some(fid) = item.face_id.as_deref() {
                let _ = crate::evolution::maybe_propose_method_evolution(
                    &self.store,
                    &self.home,
                    fid,
                );
            }
        }
        Ok(item)
    }

    pub fn review_evolution(&self, id: &str, approve: bool) -> Result<EvolutionCandidate, CoreError> {
        crate::evolution::review_evolution(&self.store, &self.home, id, approve)
    }

    pub async fn revise_knowledge_with_feedback(
        &self,
        id: &str,
        feedback: &str,
    ) -> Result<KnowledgeItem, CoreError> {
        let mut item = self
            .store
            .get_knowledge(id)?
            .ok_or_else(|| CoreError::KnowledgeNotFound(id.to_string()))?;
        if !matches!(item.status, KnowledgeStatus::Candidate | KnowledgeStatus::Conflicted) {
            return Err(CoreError::Other(format!(
                "knowledge {id} is {} — only candidate/conflicted can be optimized",
                item.status
            )));
        }
        let src = self.home.join(&item.path);
        let current = fs::read_to_string(&src)
            .map_err(|e| CoreError::Other(format!("read candidate failed: {e}")))?;
        let runtime = self.preferred_runtime(None);
        let adapter = self.adapter(&runtime)?;
        let prompt = format!(
            "You are revising a Methodus inbox candidate.\n\
Return only the full revised markdown content.\n\
Keep frontmatter keys and metadata unless obviously invalid.\n\
Do not wrap in code fences.\n\n\
Reviewer feedback:\n{feedback}\n\n\
Current candidate markdown:\n\n{current}"
        );
        let cwd = self.workspace_root().join("_inbox_feedback");
        fs::create_dir_all(&cwd)?;
        let session_id = Uuid::new_v4().to_string();
        let (handle, rx) = adapter
            .spawn(SpawnInput {
                prompt,
                cwd,
                session_id,
                permission_mode: "plan".into(),
                allowed_tools: Vec::new(),
                sandbox: Some("read-only".into()),
                extra_dirs: Vec::new(),
                model: None,
            })
            .await?;
        let collected = tokio::time::timeout(Duration::from_secs(60), collect_refine_text(rx)).await;
        let _ = adapter.stop(&handle).await;
        let revised_raw = match collected {
            Ok(s) => s,
            Err(_) => return Err(CoreError::Other("feedback optimize timeout".into())),
        };
        let revised = strip_markdown_fences(&revised_raw);
        if revised.trim().is_empty() {
            return Err(CoreError::Other("optimizer returned empty content".into()));
        }
        fs::write(&src, &revised)
            .map_err(|e| CoreError::Other(format!("write candidate failed: {e}")))?;
        item.content_hash = sha256_hex(revised.as_bytes());
        item.updated_at = Utc::now();
        self.store.update_knowledge(&item)?;
        let _ = self.store.insert_event(
            &format!("ev_{}", Uuid::new_v4().to_string().replace('-', "")),
            "knowledge.feedback_optimized",
            &Utc::now().to_rfc3339(),
            None,
            None,
            &serde_json::json!({"knowledge_id": id, "feedback": feedback}).to_string(),
            None,
        );
        Ok(item)
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

    /// Human finished looking at a reviewing task (experience + candidates).
    pub fn complete_review(&self, task_id: &str) -> Result<methodus_domain::Task, CoreError> {
        let task = self
            .store
            .get_task(task_id)?
            .ok_or_else(|| CoreError::TaskNotFound(task_id.to_string()))?;
        if task.status != TaskStatus::Reviewing {
            return Err(CoreError::Other(format!(
                "task {task_id} is {} — only reviewing tasks can be marked done",
                task.status
            )));
        }
        self.store
            .update_task_status(task_id, TaskStatus::Completed)?;
        let _ = self.store.insert_event(
            &format!("ev_{}", Uuid::new_v4().to_string().replace('-', "")),
            "task.completed",
            &Utc::now().to_rfc3339(),
            Some(task_id),
            None,
            &serde_json::json!({"task_id": task_id}).to_string(),
            None,
        );
        self.store
            .get_task(task_id)?
            .ok_or_else(|| CoreError::TaskNotFound(task_id.to_string()))
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
        let cfg = UserConfig::load(&self.home);
        let ctx = cfg.context_faces.as_deref().unwrap_or(&[]);
        let resolution = resolution::resolve(resolution::ResolveOpts {
            methodus_home: &self.home,
            request,
            requested_face: face,
            requested_context_faces: Some(ctx),
            requested_method: None,
        })?;
        if resolution.context_faces.len() >= 1 {
            let _ = crate::multi_face::detect_cross_face_debates(
                &self.store,
                &self.home,
                &resolution.all_face_ids(),
                request,
            )?;
        }
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
        self.emit_simple("task.created", Some(&task.id), &serde_json::json!({"title": title}).to_string());
        Ok(task)
    }

    /// Create a graph-control task without resolving a Face or starting a managed
    /// executor. This is the product's default task entry point: the runtime is
    /// selected only when the capsule is compiled and then owns its native TUI.
    pub fn create_control_task(
        &self,
        title: &str,
        mode: &str,
        runtime: Option<&str>,
    ) -> Result<Task, CoreError> {
        let now = Utc::now();
        let id_raw = Uuid::new_v4().to_string().replace('-', "");
        let task = Task {
            id: format!("task_{}", &id_raw[..12]),
            title: title.trim().to_string(),
            request: title.trim().to_string(),
            project_id: crate::project::focus_project(&self.home).map(|p| p.id),
            status: TaskStatus::Queued,
            runtime: Some(self.preferred_runtime(runtime)),
            workspace_id: None,
            resolution: Some(serde_json::json!({"mode": mode, "control_plane": "graph"}).to_string()),
            version: 1,
            created_at: now,
            updated_at: now,
        };
        if task.title.is_empty() {
            return Err(CoreError::Other("task title cannot be empty".into()));
        }
        self.store.insert_task(&task)?;
        self.emit_simple("task.created", Some(&task.id), &serde_json::json!({"title": title, "mode": mode}).to_string());
        Ok(task)
    }

    /// Close a native handoff with a human-authored outcome. Work tasks become
    /// experience nodes; learn tasks additionally create an explicitly-reviewable
    /// 5W2H knowledge candidate. Neither path asks Methodus to parse the Agent UI.
    pub fn finalize_control_task(&self, task_id: &str, outcome: &str) -> Result<(), CoreError> {
        let task = self.store.get_task(task_id)?
            .ok_or_else(|| CoreError::TaskNotFound(task_id.to_string()))?;
        if task.status == TaskStatus::Reviewing {
            self.transition_task(&task, TaskStatus::Completed)?;
        }
        let is_learn = task.resolution.as_deref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .and_then(|value| value.get("mode").and_then(|value| value.as_str()).map(str::to_string))
            .as_deref() == Some("learn");
        let slug = task.id.trim_start_matches("task_");
        let now = Utc::now();
        let links = task.workspace_id.as_deref()
            .map(|id| self.store.list_context_selections(id))
            .transpose()?
            .unwrap_or_default()
            .into_iter().map(|s| s.node_id).collect::<Vec<_>>();
        let related = if links.is_empty() { "[]".to_string() } else { format!("[{}]", links.iter().map(|id| format!("\"{id}\"")).collect::<Vec<_>>().join(", ")) };
        let experience_path = self.home.join("graph/experiences").join(format!("{slug}.md"));
        fs::create_dir_all(experience_path.parent().expect("experience parent"))?;
        fs::write(&experience_path, format!(
            "---\nid: experience/{slug}\ntitle: \"{}\"\nnode_type: experience\nstatus: committed\nsummary: \"{}\"\nlinks:\n  learned_from: {related}\n---\n\n## Outcome\n\n{}\n\n## Evidence\n\n- Native runtime: {}\n- Capsule: {}\n",
            yaml_quote(&task.title), yaml_quote(outcome), outcome.trim(), task.runtime.as_deref().unwrap_or("unknown"), task.workspace_id.as_deref().unwrap_or("none")
        ))?;
        if is_learn {
            let knowledge_path = self.home.join("graph/knowledge").join(format!("candidate-{slug}.md"));
            fs::create_dir_all(knowledge_path.parent().expect("knowledge parent"))?;
            fs::write(&knowledge_path, format!(
                "---\nid: knowledge/candidate-{slug}\ntitle: \"{}\"\nnode_type: knowledge\nstatus: candidate\nsummary: \"{}\"\nlinks:\n  derived_from: [\"experience/{slug}\"]\n---\n\n# 5W2H\n\n## What\n\n{}\n\n## Why\n\n待复盘确认其适用价值。\n\n## Who\n\n面向后续相关任务的 Agent 与操作者。\n\n## When\n\n当任务目标与本主题匹配时。\n\n## Where\n\n由 Methodus graph capsule 选择后注入。\n\n## How\n\n根据证据和复盘补充为可执行步骤。\n\n## How much\n\n按 capsule token 预算按需注入。\n\n## Evidence\n\n- experience/{slug}\n",
                yaml_quote(&task.title), yaml_quote(outcome), outcome.trim()
            ))?;
        }
        self.sync_graph()?;
        self.emit_simple("task.reviewed", Some(task_id), &serde_json::json!({"outcome": outcome, "learn": is_learn, "at": now}).to_string());
        Ok(())
    }

    /// Promote a Markdown graph candidate after an explicit review. The Markdown
    /// file remains the source of truth; SQLite is refreshed only as its index.
    pub fn promote_graph_candidate(&self, node_id: &str) -> Result<(), CoreError> {
        let node = self.store.graph_node(node_id)?
            .ok_or_else(|| CoreError::Other(format!("graph node not found: {node_id}")))?;
        let path = self.home.join(&node.path);
        let raw = fs::read_to_string(&path)?;
        let updated = raw.replacen("status: candidate", "status: committed", 1);
        if updated == raw {
            return Err(CoreError::Other(format!("{node_id} is not a candidate")));
        }
        fs::write(path, updated)?;
        self.sync_graph()?;
        self.emit_simple("graph.promoted", None, &serde_json::json!({"node_id": node_id}).to_string());
        Ok(())
    }

    /// Ingest docs/standards into the focus project's knowledge corpus.
    pub fn create_ingest_task(&self, sources: &[String]) -> Result<Task, CoreError> {
        if sources.is_empty() {
            return Err(CoreError::Other(
                "/learn needs document sources — e.g. /learn @~/docs/standard.pdf".into(),
            ));
        }
        let project = crate::project::focus_project(&self.home)
            .ok_or_else(|| CoreError::Other("set a focus project in /setup first".into()))?;
        let sources_block = sources
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n");
        let request = format!(
            "Document ingest for project `{}`\n\n\
             ## Sources\n\n{sources_block}\n\n\
             Read sources in place. Extract stable facts with citations. \
             Output ## Knowledge subsections for review.",
            project.id
        );
        self.create_method_task(
            "doc ingest",
            &request,
            crate::ingest::DOC_INGEST_METHOD_ID,
            Some(project.id.as_str()),
            None,
        )
    }

    /// Survey the focus project repository layout into project notes.
    pub fn create_survey_task(&self) -> Result<Task, CoreError> {
        let project = crate::project::focus_project(&self.home)
            .ok_or_else(|| CoreError::Other("set a focus project in /setup first".into()))?;
        let root = project.root.display();
        let request = format!(
            "Repository survey: project `{}`\n\n\
             Survey `{root}` only. Map layout, modules, build entry points. \
             Write ## Project Notes — do not dump into global Faces.",
            project.id
        );
        self.create_method_task(
            "repo survey",
            &request,
            crate::ingest::REPO_SURVEY_METHOD_ID,
            Some(project.id.as_str()),
            None,
        )
    }

    fn create_method_task(
        &self,
        title: &str,
        request: &str,
        method_id: &str,
        project_id: Option<&str>,
        study_sources: Option<Vec<String>>,
    ) -> Result<Task, CoreError> {
        let mut resolution = resolution::resolve(resolution::ResolveOpts {
            methodus_home: &self.home,
            request,
            requested_face: None,
            requested_context_faces: None,
            requested_method: Some(method_id),
        })?;
        if resolution.method.as_ref().is_none_or(|m| m.id != method_id) {
            return Err(CoreError::Other(format!("method `{method_id}` not installed")));
        }
        if let Some(src) = study_sources {
            resolution.study_sources = src;
        }
        let now = Utc::now();
        let id_raw = Uuid::new_v4().to_string().replace('-', "");
        let task = Task {
            id: format!("task_{}", &id_raw[..12]),
            title: title.to_string(),
            request: request.to_string(),
            project_id: project_id.map(str::to_string),
            status: TaskStatus::Queued,
            runtime: Some(self.preferred_runtime(None)),
            workspace_id: None,
            resolution: Some(resolution.to_json()),
            version: 1,
            created_at: now,
            updated_at: now,
        };
        self.store.insert_task(&task)?;
        self.emit_simple("task.created", Some(&task.id), &serde_json::json!({"title": title}).to_string());
        Ok(task)
    }

    pub fn review_hypothesis(
        &self,
        id: &str,
        action: crate::hypothesis::HypothesisReviewAction,
    ) -> Result<Hypothesis, CoreError> {
        crate::hypothesis::review_hypothesis(&self.store, &self.home, id, action)
    }

    /// Remove workspace dirs for terminal tasks older than `max_age_days`.
    pub fn cleanup_workspaces(&self, max_age_days: i64) -> Result<usize, CoreError> {
        use std::time::SystemTime;
        let cutoff = Utc::now() - chrono::Duration::days(max_age_days);
        let mut removed = 0usize;
        let ws_root = self.home.join("workspaces");
        let Ok(entries) = fs::read_dir(&ws_root) else {
            return Ok(0);
        };
        for entry in entries.flatten() {
            let task_id = entry.file_name().to_string_lossy().into_owned();
            let Ok(Some(task)) = self.store.get_task(&task_id) else {
                continue;
            };
            if !task.status.is_terminal() {
                continue;
            }
            if task.updated_at > cutoff {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                let _ = fs::remove_dir_all(&path);
                removed += 1;
                let _ = self.store.insert_event(
                    &format!("ev_{}", Uuid::new_v4().to_string().replace('-', "")),
                    "workspace.cleaned",
                    &Utc::now().to_rfc3339(),
                    Some(&task_id),
                    None,
                    &serde_json::json!({"path": path.display().to_string()}).to_string(),
                    None,
                );
            }
        }
        let _ = SystemTime::now();
        Ok(removed)
    }

    fn emit_simple(&self, event_type: &str, task_id: Option<&str>, payload: &str) {
        let _ = self.store.insert_event(
            &format!("ev_{}", Uuid::new_v4().to_string().replace('-', "")),
            event_type,
            &Utc::now().to_rfc3339(),
            task_id,
            None,
            payload,
            None,
        );
    }

    /// Start a module-expert study task from user-specified paths/URLs (not the task workspace).
    pub fn create_study_task(
        &self,
        scope: &str,
        sources: &[String],
        face: Option<&str>,
    ) -> Result<Task, CoreError> {
        if sources.is_empty() {
            return Err(CoreError::Other(
                "learn needs at least one path or URL — e.g. /learn nxm @~/docs/nxm https://…"
                    .into(),
            ));
        }
        let scope = scope.trim();
        let title = if scope.is_empty() {
            sources[0].chars().take(72).collect::<String>()
        } else if scope.chars().count() > 72 {
            format!("{}…", scope.chars().take(71).collect::<String>())
        } else {
            scope.to_string()
        };
        let sources_block = sources
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n");
        let topic = if scope.is_empty() {
            title.clone()
        } else {
            scope.to_string()
        };
        let request = format!(
            "Module expert study: {topic}\n\n\
             ## Sources to read\n\n\
             {sources_block}\n\n\
             Read only the sources above (in place). Do not survey the task workspace. \
             Synthesize structured knowledge and list open questions for your human mentor. \
             Follow the module-expert-learning output format."
        );
        let face_id = crate::face_util::ensure_study_face(&self.home, scope, face)?;
        let mut resolution = resolution::resolve(resolution::ResolveOpts {
            methodus_home: &self.home,
            request: &request,
            requested_face: Some(&face_id),
            requested_context_faces: None,
            requested_method: Some(crate::curiosity::MODULE_EXPERT_METHOD_ID),
        })?;
        if resolution.method.as_ref().is_none_or(|m| {
            m.id != crate::curiosity::MODULE_EXPERT_METHOD_ID
        }) {
            return Err(CoreError::Other(
                "module-expert-learning method not installed — restart Methodus or check ~/.methodus/methods".into(),
            ));
        }
        resolution.study_sources = sources.to_vec();
        let now = Utc::now();
        let id_raw = Uuid::new_v4().to_string().replace('-', "");
        let task = Task {
            id: format!("task_{}", &id_raw[..12]),
            title,
            request,
            project_id: None,
            status: TaskStatus::Queued,
            runtime: Some(self.preferred_runtime(None)),
            workspace_id: None,
            resolution: Some(resolution.to_json()),
            version: 1,
            created_at: now,
            updated_at: now,
        };
        self.store.insert_task(&task)?;
        self.emit_simple("task.created", Some(&task.id), &serde_json::json!({"title": task.title}).to_string());
        Ok(task)
    }

    /// Unified learn: user supplies sources; Methodus picks survey / ingest / module-expert.
    pub fn create_learn_task(
        &self,
        hint: &str,
        sources: &[String],
        face: Option<&str>,
    ) -> Result<(Task, crate::learn::LearnMode), CoreError> {
        let (mode, scope) = crate::learn::plan_learn(&self.home, sources, hint)?;
        let task = match mode {
            crate::learn::LearnMode::RepoSurvey => self.create_survey_task()?,
            crate::learn::LearnMode::DocIngest => self.create_ingest_task(sources)?,
            crate::learn::LearnMode::ModuleExpert => {
                self.create_study_task(&scope, sources, face)?
            }
        };
        Ok((task, mode))
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

    /// Drop a terminal task from the list (failed / cancelled / completed).
    pub fn delete_task(&self, task_id: &str) -> Result<(), CoreError> {
        let task = self
            .store
            .get_task(task_id)?
            .ok_or_else(|| CoreError::TaskNotFound(task_id.to_string()))?;
        if !task.status.is_terminal() {
            return Err(CoreError::TaskNotDeletable(
                task_id.to_string(),
                task.status.to_string(),
            ));
        }
        let paths = self.store.delete_task(task_id)?;
        for path in paths {
            let p = PathBuf::from(&path);
            if p.is_dir() {
                let _ = fs::remove_dir_all(&p);
            }
        }
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
        let is_study = !resolution.study_sources.is_empty();
        let face_ids = resolution.all_face_ids();
        let face_refs: Vec<&str> = face_ids.iter().map(String::as_str).collect();
        let snippets = if is_study {
            Vec::new()
        } else {
            learning::select_committed_knowledge_multi(
                &self.store,
                &self.home,
                &face_refs,
                &task.request,
            )?
        };
        let notes = if is_study {
            Vec::new()
        } else {
            crate::refine::select_committed_notes(
                &self.store,
                &self.home,
                &face_refs,
                &task.request,
            )?
        };
        let inventory = learning::render_injected_inventory(&notes, &snippets);
        if !is_study {
            let _ = learning::record_injections(
                &self.store,
                &self.home,
                task_id,
                &notes,
                &snippets,
            )?;
        }
        let mut context = resolution.to_context_markdown(&task.title, &task.request);
        if !is_study {
            context.push_str(&inventory);
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
            context.push_str(&crate::refine::render_notes_context(&notes));
            context.push_str(&learning::render_knowledge_context(&snippets));
        } else {
            context.push_str(&crate::curiosity::render_study_sources(
                &resolution.study_sources,
            ));
        }
        let named_roots = if is_study {
            crate::curiosity::study_named_roots(&self.home, &resolution.study_sources)?
        } else {
            self.context_roots()
        };
        context.push_str(&crate::mentions::render_readable_dirs(&named_roots));
        let mention_source = follow_up.as_deref().unwrap_or(&task.request);
        let mentions = crate::mentions::resolve_named(mention_source, &named_roots);
        context.push_str(&crate::mentions::render_attached(&mentions));
        let extra_dirs = crate::mentions::readable_dirs(&named_roots);
        let ws_root = WorkspaceBuilder::build(&self.workspace_root(), task_id, &context)?;
        WorkspaceBuilder::write_injected(&ws_root, &inventory)?;
        WorkspaceBuilder::write_plan(&ws_root, &resolution.to_plan_markdown(&task.title))?;
        let knowledge_files: Vec<(String, PathBuf)> = notes
            .iter()
            .chain(snippets.iter())
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
            let dest = ws_root.join("face-context").join(&resolution.face_id);
            fs::create_dir_all(&dest)?;
            fs::copy(&face_yaml, dest.join("face.yaml"))?;
        }
        for ctx in &resolution.context_faces {
            if ctx.face_dir.is_empty() {
                continue;
            }
            let ctx_yaml = PathBuf::from(&ctx.face_dir).join("face.yaml");
            if ctx_yaml.is_file() {
                let dest = ws_root.join("face-context").join(&ctx.id);
                fs::create_dir_all(&dest)?;
                let _ = fs::copy(&ctx_yaml, dest.join("face.yaml"));
            }
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
        self.emit_simple(
            "workspace.created",
            Some(task_id),
            &serde_json::json!({"workspace_id": ws_id, "path": ws_root.to_string_lossy()}).to_string(),
        );

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
        let permission_mode =
            policy::PermissionMode::parse(UserConfig::load(&self.home).permission_mode.as_deref());
        let allowed_tools = policy::spawn_allowed_tools(permission_mode);
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
            method_id: resolution.method.as_ref().map(|m| m.id.clone()),
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
            method_id: resolution_from_task(&task, &self.home)
                .ok()
                .and_then(|r| r.method.map(|m| m.id)),
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
            method_id: launch.method_id,
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
        requested_context_faces: None,
        requested_method: None,
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
    method_id: Option<String>,
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
    method_id: Option<String>,
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

            let (auto, user) = policy::split_denials(&outcome.denials, self.permission_mode);
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
    let _ = crate::scheduler::tick(&runner.store, &runner.home);
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

fn event_on_date(occurred_at: &str, day: chrono::NaiveDate) -> bool {
    DateTime::parse_from_rfc3339(occurred_at)
        .map(|d| d.date_naive() == day)
        .unwrap_or(false)
}

async fn collect_refine_text(mut rx: mpsc::Receiver<RuntimeEvent>) -> String {
    let mut last = String::new();
    while let Some(ev) = rx.recv().await {
        match ev {
            RuntimeEvent::Result {
                is_error: true, ..
            } => return String::new(),
            RuntimeEvent::Result { text, .. } => {
                if !text.trim().is_empty() {
                    return text;
                }
                return last;
            }
            RuntimeEvent::AssistantText { text } => last = text,
            RuntimeEvent::Error { .. } => return String::new(),
            _ => {}
        }
    }
    last
}

fn strip_markdown_fences(text: &str) -> String {
    let t = text.trim();
    if !t.starts_with("```") {
        return t.to_string();
    }
    let mut lines = t.lines();
    let _ = lines.next();
    let mut out = Vec::new();
    for line in lines {
        if line.trim_start().starts_with("```") {
            break;
        }
        out.push(line);
    }
    out.join("\n").trim().to_string()
}

fn append_transcript(path: &std::path::Path, line: &str) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{line}")?;
    Ok(())
}

fn truncate_utf8_prefix(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
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
    let summary = truncate_utf8_prefix(result_text, 500);
    let mode_line = if runner.method_id.as_deref()
        == Some(crate::curiosity::MODULE_EXPERT_METHOD_ID)
    {
        "         - mode: module_expert\n"
    } else {
        ""
    };

    let body = format!(
        "# Experience `{exp_id}`\n\n\
         - task: `{task}`\n\
         - face: `{face}`\n\
         - outcome: {outcome}\n\
{mode_line}\
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
        mode_line = mode_line,
    );
    fs::write(&abs, &body)?;
    let hash = sha256_hex(body.as_bytes());

    let exp = Experience {
        id: exp_id,
        task_id: runner.task_id.clone(),
        face_id: Some(face.clone()),
        path: rel,
        content_hash: hash,
        outcome: Some(outcome.to_string()),
        summary: Some(summary),
        created_at: now,
        updated_at: now,
    };
    runner.store.insert_experience(&exp)?;
    let _ = runner.store.insert_event(
        &format!("ev_{}", Uuid::new_v4().to_string().replace('-', "")),
        "experience.created",
        &now.to_rfc3339(),
        Some(&runner.task_id),
        None,
        &serde_json::json!({"experience_id": exp.id, "face_id": face}).to_string(),
        None,
    );
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

    #[test]
    fn truncate_utf8_prefix_respects_char_boundary() {
        let s = "中".repeat(200); // 600 bytes
        let out = truncate_utf8_prefix(&s, 500);
        assert!(out.len() <= 500);
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

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

    #[test]
    fn graph_control_learn_task_returns_experience_and_5w2h_candidate() {
        let (engine, dir) = engine_with(vec![]);
        fs::create_dir_all(dir.path().join("graph/knowledge")).unwrap();
        fs::write(dir.path().join("graph/knowledge/retries.md"), "---\nid: knowledge/retries\ntitle: Retry safety\nnode_type: knowledge\nstatus: committed\nsummary: Retry with idempotency keys\n---\n\n## Execute\nUse a stable key.\n").unwrap();
        let task = engine.create_control_task("Learn retry safety", "learn", Some("claude-code")).unwrap();
        let plan = engine.compile_capsule(&task.id, 1600).unwrap();
        let launch = engine.record_native_handoff(&plan).unwrap();
        engine.record_native_return(&launch, &task.id, "exit 0").unwrap();
        engine.finalize_control_task(&task.id, "Validated an idempotency-key approach.").unwrap();
        let nodes = engine.list_graph_nodes(None).unwrap();
        assert!(nodes.iter().any(|node| node.id == format!("experience/{}", task.id.trim_start_matches("task_"))));
        assert!(nodes.iter().any(|node| node.status.as_deref() == Some("candidate")));
        assert_eq!(engine.store().get_task(&task.id).unwrap().unwrap().status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn context_planner_accepts_only_catalog_node_ids() {
        let (engine, dir) = engine_with(vec![ok_result(r#"{"selected_node_ids":["knowledge/retries","outside/catalog"],"rationale":"retry task"}"#)]);
        fs::create_dir_all(dir.path().join("graph/knowledge")).unwrap();
        fs::write(dir.path().join("graph/knowledge/retries.md"), "---\nid: knowledge/retries\ntitle: Retry safety\nnode_type: knowledge\nstatus: committed\nsummary: Retry with idempotency keys\n---\n\n## Execute\nUse a stable key.\n").unwrap();
        let task = engine.create_control_task("Handle retries", "work", Some("claude-code")).unwrap();
        let plan = engine.plan_context(&task.id).await.unwrap();
        assert_eq!(plan.selected_node_ids, vec!["knowledge/retries"]);
        assert_eq!(plan.rationale, "retry task");
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
        let closed = engine.complete_review(&task.id).unwrap();
        assert_eq!(closed.status, TaskStatus::Completed);
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
        assert!(ctx.contains("## Injected this turn"));
        assert!(ctx.contains("**knowledge**"));
        let injected = fs::read_to_string(
            dir.path()
                .join("workspaces")
                .join(&task.id)
                .join(".methodus/injected.md"),
        )
        .unwrap();
        assert!(injected.contains("latch"));
        let evs = engine.store().list_events(Some(&task.id), 80).unwrap();
        assert!(
            evs.iter()
                .any(|e| e.event_type == crate::learning::INJECTED_EVENT),
            "missing learning.injected: {evs:?}"
        );
        assert!(dir
            .path()
            .join("workspaces")
            .join(&task.id)
            .join("face-context/knowledge/latch.md")
            .is_file());
    }

    #[tokio::test]
    async fn inject_increments_note_hits_and_promotes() {
        let (engine, dir) = engine_with_turns(vec![
            vec![ok_result("ok1")],
            vec![ok_result("ok2")],
            vec![ok_result("ok3")],
        ]);
        let now = Utc::now();
        let rel = "faces/general/notes/latch-gpio.md";
        fs::create_dir_all(dir.path().join("faces/general/notes")).unwrap();
        fs::write(
            dir.path().join(rel),
            "---\nkind: note\nhits: 0\nstatus: committed\n---\n\n# latch gpio\n\n- probe gpio 4 first\n",
        )
        .unwrap();
        engine
            .store()
            .insert_knowledge(&KnowledgeItem {
                id: "know_note".into(),
                face_id: Some("general".into()),
                project_id: None,
                path: rel.into(),
                content_hash: "h".into(),
                source: crate::refine::HARNESS_NOTE_SOURCE.into(),
                confidence: Some(0.55),
                scope: Some("note".into()),
                status: KnowledgeStatus::Committed,
                conflict_of: None,
                version: 1,
                created_at: now,
                updated_at: now,
            })
            .unwrap();

        for title in ["debug latch a", "debug latch b", "debug latch c"] {
            let task = engine
                .create_task(title, "debug the latch gpio", None, None)
                .unwrap();
            let mut rx = engine.run_task(&task.id, false).await.unwrap();
            while rx.recv().await.is_some() {}
        }
        let body = fs::read_to_string(dir.path().join(rel)).unwrap();
        assert!(body.contains("hits: 3"), "{body}");
        engine.tick_learning().unwrap();
        let jobs = engine.store().list_jobs().unwrap();
        assert!(
            jobs.iter().any(|j| j.kind == JobKind::ProposeSkill),
            "3 injections should enqueue skill promote: {jobs:?}"
        );
    }

    #[tokio::test]
    async fn injection_miss_lowers_confidence() {
        let (engine, dir) = engine_with(vec![ok_result(
            "unknown: latch protocol still broken on gpio",
        )]);
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
        engine.tick_learning().unwrap();

        let item = engine.store().get_knowledge("know_latch").unwrap().unwrap();
        assert!(
            item.confidence.unwrap() < 0.8,
            "miss should downrank: {:?}",
            item.confidence
        );
        let qs = engine.store().list_questions(None).unwrap();
        assert!(
            qs.iter()
                .any(|q| q.question.contains("Injected") && q.question.contains("latch")),
            "expected mentor question: {qs:?}"
        );
        let evs = engine.store().list_events(Some(&task.id), 200).unwrap();
        assert!(evs
            .iter()
            .any(|e| e.event_type == crate::learning::INJECTION_MISSED_EVENT));
    }

    #[tokio::test]
    async fn trajectory_skill_skips_experience_knowledge() {
        let (engine, _dir) = engine_with(vec![
            tool_start_with("Bash", serde_json::json!({"command": "ps aux | grep nginx"})),
            tool_start_with("Read", serde_json::json!({"path": "/proc/1/stat"})),
            tool_start_with("Grep", serde_json::json!({"pattern": "cpu"})),
            ok_result(
                "The latch on the carrier board uses gpio 4 with a 3.3V pull-up.",
            ),
        ]);
        let task = engine
            .create_task("sample cpu", "sample cpu of nginx", None, None)
            .unwrap();
        let mut rx = engine.run_task(&task.id, false).await.unwrap();
        while rx.recv().await.is_some() {}
        engine.tick_learning().unwrap();
        let experience_cands: Vec<_> = engine
            .store()
            .list_knowledge(Some(KnowledgeStatus::Candidate))
            .unwrap()
            .into_iter()
            .filter(|k| k.source == "experience")
            .collect();
        assert!(
            experience_cands.is_empty(),
            "trajectory distill should not also mint knowledge: {experience_cands:?}"
        );
        assert!(engine
            .store()
            .list_knowledge(Some(KnowledgeStatus::Candidate))
            .unwrap()
            .iter()
            .any(|k| k.source == crate::learning::SKILL_DRAFT_SOURCE));
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
    async fn destructive_bash_denial_pauses_for_approval_then_approve_continues() {
        let deny = vec![
            RuntimeEvent::SessionStarted {
                session_id: "exec-sid-1".to_string(),
            },
            RuntimeEvent::Result {
                is_error: false,
                text: "need rm".to_string(),
                cost_usd: None,
                usage: None,
                session_id: Some("exec-sid-1".to_string()),
                permission_denials: vec![PermissionDenial {
                    tool_name: "Bash".to_string(),
                    tool_use_id: Some("tu1".to_string()),
                    tool_input: serde_json::json!({"command": "rm -rf build/"}),
                }],
            },
        ];
        let (engine, _dir) = engine_with_turns(vec![deny, vec![ok_result("ran it")]]);
        let task = engine
            .create_task("clean build", "clean build", None, None)
            .unwrap();
        let mut rx = engine.run_task(&task.id, false).await.unwrap();
        let mut saw_approval = None;
        while let Some(ev) = rx.recv().await {
            if let RuntimeEvent::ApprovalRequested { id, tool_name, .. } = ev {
                assert_eq!(tool_name, "Bash");
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
    async fn auto_allow_bash_git_does_not_pause() {
        let first = vec![RuntimeEvent::Result {
            is_error: false,
            text: "need shell".to_string(),
            cost_usd: None,
            usage: None,
            session_id: Some("exec-sid-1".to_string()),
            permission_denials: vec![PermissionDenial {
                tool_name: "Bash".to_string(),
                tool_use_id: None,
                tool_input: serde_json::json!({"command": "git status"}),
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
        engine.delete_task(&task.id).unwrap();
        assert!(engine.store().get_task(&task.id).unwrap().is_none());
    }

    #[tokio::test]
    async fn task_complete_enqueues_learning_jobs() {
        let (engine, _dir) = engine_with(vec![ok_result("unknown: latch protocol")]);
        let task = engine.create_task("g", "g", None, None).unwrap();
        let mut rx = engine.run_task(&task.id, false).await.unwrap();
        while rx.recv().await.is_some() {}
        let jobs = engine.store().list_jobs().unwrap();
        assert!(jobs.iter().any(|j| j.kind == JobKind::ExtractExperience));
        assert!(
            jobs.iter().any(|j| j.status == JobStatus::Done),
            "finish_task should drain extract/detect/propose: {jobs:?}"
        );
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
        let committed = engine
            .review_knowledge(&cands[0].id, KnowledgeReviewAction::Commit)
            .unwrap();
        assert_eq!(committed.status, KnowledgeStatus::Committed);
        let committed_body = fs::read_to_string(engine.home().join(&committed.path)).unwrap();
        assert!(committed_body.contains("gpio 4"));

        let t2 = engine.create_task("two", "two", None, None).unwrap();
        let mut rx = engine.run_task(&t2.id, false).await.unwrap();
        while rx.recv().await.is_some() {}
        engine.tick_learning().unwrap();

        let qs = engine.store().list_questions(None).unwrap();
        let gap_q = qs
            .iter()
            .find(|q| q.question.starts_with("What should we know about"))
            .expect("gap question");
        assert!(
            gap_q.frequency >= 2.0,
            "gap frequency: {} qs={qs:?}",
            gap_q.frequency
        );
        let conflicts = engine
            .store()
            .list_knowledge(Some(KnowledgeStatus::Conflicted))
            .unwrap();
        assert!(!conflicts.is_empty());
        let still = fs::read_to_string(engine.home().join(&committed.path)).unwrap();
        assert!(still.contains("gpio 4"));
        assert!(!still.contains("gpio 7"));

        let answered = engine
            .answer_question(&gap_q.id, "the latch uses 3.3V pull-up")
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
        tool_start_with(name, serde_json::json!({}))
    }

    fn tool_start_with(name: &str, input: serde_json::Value) -> RuntimeEvent {
        RuntimeEvent::ToolCallStarted {
            id: name.to_string(),
            name: name.to_string(),
            input,
        }
    }

    #[tokio::test]
    async fn auto_skill_draft_then_review_installs_live_skill() {
        let (engine, _dir) = engine_with(vec![
            tool_start_with("Bash", serde_json::json!({"command": "ps aux | grep nginx"})),
            tool_start_with("Read", serde_json::json!({"path": "/proc/1/stat"})),
            tool_start_with("Grep", serde_json::json!({"pattern": "cpu"})),
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

        let draft = auto_drafts.into_iter().next().unwrap();
        assert_eq!(draft.source, crate::learning::SKILL_DRAFT_SOURCE);
        assert!(draft.path.contains(".candidates"));

        let committed = engine
            .review_knowledge(&draft.id, KnowledgeReviewAction::Commit)
            .unwrap();
        assert_eq!(committed.status, KnowledgeStatus::Committed);
        assert!(committed.path.starts_with("skills/"));
        assert!(!committed.path.contains(".candidates"));
        assert!(engine.home().join(&committed.path).exists());
        let catalog = crate::resolution::scan_skills(engine.home());
        assert!(catalog.iter().any(|s| s.name.contains("sample")));

        // Same trajectory against a live skill → incremental patch, not a parallel draft.
        let again = crate::learning::propose_skill_from_task(
            engine.store(),
            engine.home(),
            &task.id,
            None,
        )
        .unwrap();
        if let Some(patch) = again {
            assert_eq!(patch.source, crate::refine::SKILL_PATCH_SOURCE);
            let applied = engine
                .review_knowledge(&patch.id, KnowledgeReviewAction::Commit)
                .unwrap();
            assert_eq!(applied.status, KnowledgeStatus::Committed);
            assert!(applied.path.starts_with("skills/"));
        }
    }

    #[tokio::test]
    async fn empty_tool_calls_do_not_draft_skill() {
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
            .filter(|k| {
                k.source == crate::learning::SKILL_DRAFT_SOURCE
                    || k.source == crate::refine::HARNESS_NOTE_SOURCE
                    || k.source == crate::refine::SKILL_PATCH_SOURCE
            })
            .collect();
        assert!(
            auto_drafts.is_empty(),
            "bare Use Tool steps must not distill"
        );
    }

    #[tokio::test]
    async fn tick_refine_llm_polishes_note() {
        let polished = r#"{"skip":false,"rationale":"prefer ss -tnp","note_body":"- run ss -tnp before blaming the app"}"#;
        let (engine, dir) = engine_with(vec![ok_result(polished)]);
        let now = Utc::now();
        let proposal = crate::refine::RefinementProposal {
            target_kind: crate::refine::RefineTargetKind::Note,
            target_id: "tcp-note".into(),
            op: crate::refine::RefineOp::Create,
            add_procedure: Vec::new(),
            add_pitfalls: Vec::new(),
            note_body: Some("- Use `Read`".into()),
            evidence_refs: vec!["task_refine".into()],
            rationale: "raw".into(),
            hits: 1,
            planner: "rules".into(),
        };
        let json = serde_json::to_string_pretty(&proposal).unwrap();
        let rel = "faces/general/notes/.candidates/tcp-note.md";
        let body = format!(
            "---\nkind: note\nplanner: rules\nsource_task: task_refine\n---\n\n# tcp-note\n\n- Use `Read`\n\n## Proposal\n\n```json\n{json}\n```\n"
        );
        fs::create_dir_all(dir.path().join("faces/general/notes/.candidates")).unwrap();
        fs::write(dir.path().join(rel), &body).unwrap();
        engine
            .store()
            .insert_knowledge(&KnowledgeItem {
                id: "know_refine".into(),
                face_id: Some("general".into()),
                project_id: None,
                path: rel.into(),
                content_hash: "h".into(),
                source: crate::refine::HARNESS_NOTE_SOURCE.into(),
                confidence: Some(0.5),
                scope: Some("note".into()),
                status: KnowledgeStatus::Candidate,
                conflict_of: None,
                version: 1,
                created_at: now,
                updated_at: now,
            })
            .unwrap();

        let n = engine.tick_refine_llm().await.unwrap();
        assert_eq!(n, 1);
        let rewritten = fs::read_to_string(dir.path().join(rel)).unwrap();
        assert!(rewritten.contains("planner: llm"), "{rewritten}");
        assert!(rewritten.contains("prefer ss -tnp"), "{rewritten}");
        assert!(rewritten.contains("ss -tnp before blaming"));
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
