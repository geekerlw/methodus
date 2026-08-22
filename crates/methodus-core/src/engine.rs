//! Methodus's active application service.
//!
//! This layer intentionally contains only the maintainer workflow: a focused
//! Learn conversation, Markdown graph indexing, review actions, and Personal →
//! Team promotion. Ordinary coding tasks, workspaces, handoff sessions, and
//! runtime Skill management are not part of the active product surface.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use chrono::{Local, Utc};
use methodus_domain::{
    AttentionKind, GoalRun, GraphEdge, GraphNode, HumanAttention, LearningGoal, PermissionDenial,
    RuntimeEvent, UsageDelta, WorkKind,
};
use methodus_runtime::{RuntimeAdapter, SessionHandle, SpawnInput};
use methodus_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::agent::{AgentManifest, AgentQuery};
use crate::config::UserConfig;
use crate::error::CoreError;
use crate::learning::{self, GoalForm};

const LEARN_GRAPH_INTEGRATION_CONTRACT: &str =
    include_str!("../../../resources/protocols/learn-graph-integration.md");
const MERGEABLE_FACETS: &[&str] = &["Learn", "Decide", "Execute", "Evidence"];

#[derive(Clone)]
pub struct Engine {
    store: Arc<Store>,
    adapters: HashMap<String, Arc<dyn RuntimeAdapter>>,
    home: PathBuf,
    launch_cwd: PathBuf,
}

#[derive(Debug, Clone)]
pub struct TeamStatus {
    pub team_id: String,
    pub root: PathBuf,
    pub is_git: bool,
    pub branch: Option<String>,
    pub dirty: bool,
    pub changes: Vec<String>,
    pub validation_issues: Vec<crate::graph::GraphIssue>,
    pub diff: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnRun {
    pub run_id: String,
    pub goal: String,
    pub runtime: String,
    pub permission_mode: String,
    pub status: String,
    pub executor_sid: Option<String>,
    pub updated_at: String,
}

/// A prepared native interactive Learn launch. The caller temporarily yields its
/// terminal to this command; Methodus owns the managed workspace but never proxies
/// the runtime conversation.
#[derive(Debug, Clone)]
pub struct NativeLearnHandoff {
    pub run_id: String,
    pub goal: String,
    pub runtime: String,
    pub cwd: PathBuf,
    pub program: String,
    pub args: Vec<String>,
    pub executor_sid: Option<String>,
    pub output_path: PathBuf,
}

/// A prepared native interactive Use launch. The graph and attached sources are
/// protected by contract, while the Methodus-managed workspace follows Learn's
/// native permission mapping; there is no candidate import step.
#[derive(Debug, Clone)]
pub struct NativeUseHandoff {
    pub session_id: String,
    pub question: String,
    pub runtime: String,
    pub cwd: PathBuf,
    pub return_path: PathBuf,
    pub program: String,
    pub args: Vec<String>,
    pub executor_sid: Option<String>,
}

/// What Methodus found after the native runtime returned control.
#[derive(Debug, Clone)]
pub struct NativeLearnReturn {
    pub candidate_ids: Vec<String>,
    pub output_recorded: bool,
    /// A native continuation may itself stop on another maintainer question.
    /// Keeping this in the return value lets every caller route that question
    /// back to the same attention queue instead of treating the turn as done.
    pub attention: Option<HumanAttention>,
    /// A failed handoff/import is deliberately distinct from a resumable
    /// question. Callers use it to keep the previous attention open.
    pub import_error: Option<String>,
}

struct UseEnvironment {
    cwd: PathBuf,
    manifest_path: PathBuf,
    return_path: PathBuf,
    graph_dirs: Vec<PathBuf>,
}

struct LearnEnvironment {
    manifest_path: PathBuf,
    graph_dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnEventRecord {
    pub at: String,
    pub role: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LearnRunState {
    run_id: String,
    kind: String,
    status: String,
    goal: String,
    runtime: String,
    #[serde(default = "default_permission_mode")]
    permission_mode: String,
    #[serde(default)]
    executor_sid: Option<String>,
    #[serde(default)]
    unresolved_questions: Vec<String>,
    #[serde(default)]
    contradictions: Vec<String>,
    updated_at: String,
}

fn default_permission_mode() -> String {
    "plan".into()
}

fn permission_profile(mode: &str) -> (&'static str, &'static str) {
    match mode {
        "cautious" => ("cautious", "workspace-write"),
        "acceptEdits" => ("acceptEdits", "workspace-write"),
        _ => ("plan", "read-only"),
    }
}

fn native_learn_command(
    runtime: &str,
    permission_mode: &str,
    sandbox: &str,
    cwd: &Path,
    extra_dirs: &[PathBuf],
    executor_sid: Option<&str>,
    resume: bool,
    brief: &str,
) -> Result<(String, Vec<String>), CoreError> {
    match runtime {
        "claude-code" => {
            let mut args = Vec::new();
            if resume {
                let sid = executor_sid.ok_or_else(|| {
                    CoreError::Other("missing Claude session id for native Learn resume".into())
                })?;
                args.extend(["--resume".into(), sid.into()]);
            } else {
                let sid = executor_sid.ok_or_else(|| {
                    CoreError::Other("missing Claude session id for native Learn launch".into())
                })?;
                args.extend(["--session-id".into(), sid.into()]);
            }
            args.extend([
                "--permission-mode".into(),
                native_claude_permission_mode(permission_mode).into(),
            ]);
            for dir in extra_dirs {
                args.extend(["--add-dir".into(), dir.to_string_lossy().into_owned()]);
            }
            // `--add-dir` is variadic in Claude's CLI. End option parsing before
            // the initial message or Claude can consume the prompt as another
            // directory and enter plan mode without displaying the question.
            args.push("--".into());
            args.push(brief.into());
            Ok(("claude".into(), args))
        }
        "codex" => {
            // A native Learn needs one explicit, maintainer-approved write for
            // its return artifact. Keep the runtime approval-gated instead of
            // making that artifact impossible under a read-only sandbox.
            let native_sandbox = if permission_mode == "plan" {
                "workspace-write"
            } else {
                sandbox
            };
            let mut args = vec![
                "--cd".into(),
                cwd.to_string_lossy().into_owned(),
                "--sandbox".into(),
                native_sandbox.into(),
            ];
            if permission_mode == "acceptEdits" {
                args.push("--approve-for-me".into());
            } else {
                args.extend(["--ask-for-approval".into(), "on-request".into()]);
            }
            for dir in extra_dirs {
                if dir != cwd {
                    args.extend(["--add-dir".into(), dir.to_string_lossy().into_owned()]);
                }
            }
            args.push(brief.into());
            Ok(("codex".into(), args))
        }
        "cursor" => {
            let mut args = vec![
                "agent".into(),
                "--workspace".into(),
                cwd.to_string_lossy().into_owned(),
            ];
            if permission_mode == "plan" {
                args.push("--plan".into());
            } else {
                args.push("--auto-review".into());
            }
            for dir in extra_dirs {
                if dir != cwd {
                    args.extend(["--add-dir".into(), dir.to_string_lossy().into_owned()]);
                }
            }
            args.push(brief.into());
            Ok(("cursor".into(), args))
        }
        other => Err(CoreError::UnknownRuntime(other.into())),
    }
}

fn native_use_command(
    runtime: &str,
    permission_mode: &str,
    cwd: &Path,
    extra_dirs: &[PathBuf],
    executor_sid: Option<&str>,
    resume: bool,
    brief: &str,
) -> Result<(String, Vec<String>), CoreError> {
    let (permission_mode, sandbox) = permission_profile(permission_mode);
    match runtime {
        "claude-code" => {
            let sid = executor_sid.ok_or_else(|| {
                CoreError::Other("missing Claude session id for native Use launch".into())
            })?;
            let mut args = if resume {
                vec!["--resume".into(), sid.into()]
            } else {
                vec!["--session-id".into(), sid.into()]
            };
            args.extend([
                "--permission-mode".into(),
                native_claude_permission_mode(permission_mode).into(),
            ]);
            for dir in extra_dirs {
                args.extend(["--add-dir".into(), dir.to_string_lossy().into_owned()]);
            }
            // Keep the native Learn and Use launch contracts identical: the
            // initial message must remain a positional prompt after all options.
            args.push("--".into());
            args.push(brief.into());
            Ok(("claude".into(), args))
        }
        "codex" => {
            let native_sandbox = if permission_mode == "plan" {
                "workspace-write"
            } else {
                sandbox
            };
            let mut args = vec![
                "--cd".into(),
                cwd.to_string_lossy().into_owned(),
                "--sandbox".into(),
                native_sandbox.into(),
            ];
            if permission_mode == "acceptEdits" {
                args.push("--approve-for-me".into());
            } else {
                args.extend(["--ask-for-approval".into(), "on-request".into()]);
            }
            for dir in extra_dirs {
                if dir != cwd {
                    args.extend(["--add-dir".into(), dir.to_string_lossy().into_owned()]);
                }
            }
            args.push(brief.into());
            Ok(("codex".into(), args))
        }
        "cursor" => {
            let mut args = vec![
                "agent".into(),
                "--workspace".into(),
                cwd.to_string_lossy().into_owned(),
            ];
            if permission_mode == "plan" {
                args.push("--plan".into());
            } else {
                args.push("--auto-review".into());
            }
            for dir in extra_dirs {
                if dir != cwd {
                    args.extend(["--add-dir".into(), dir.to_string_lossy().into_owned()]);
                }
            }
            args.push(brief.into());
            Ok(("cursor".into(), args))
        }
        other => Err(CoreError::UnknownRuntime(other.into())),
    }
}

fn native_claude_permission_mode(mode: &str) -> &'static str {
    match mode {
        // Source changes remain approval-gated while the maintainer can approve
        // the single Methodus return artifact at finalization time.
        "plan" | "cautious" => "manual",
        "acceptEdits" => "auto",
        _ => "manual",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SourceManifest {
    sources: Vec<SourceManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SourceManifestEntry {
    locator: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    fingerprint: Option<String>,
    status: String,
}

#[derive(Debug, Deserialize)]
struct CandidateSet {
    #[serde(default)]
    graph_review: Option<GraphReview>,
    #[serde(default)]
    candidates: Vec<CandidateDraft>,
    #[serde(default)]
    relations: Vec<CandidateRelation>,
    #[serde(default)]
    unresolved_questions: Vec<String>,
    #[serde(default)]
    contradictions: Vec<Value>,
    #[serde(default)]
    runtime_skills: Vec<RuntimeSkillObservation>,
}

#[derive(Debug, Deserialize, Clone)]
struct GraphReview {
    #[serde(default)]
    searched: bool,
    #[serde(default)]
    relevant_nodes: Vec<GraphReviewNode>,
    #[serde(default)]
    no_match_reason: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct GraphReviewNode {
    id: String,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct CandidateRelation {
    from: String,
    #[serde(alias = "kind", alias = "type")]
    relation: String,
    to: String,
}

#[derive(Debug, Deserialize, Clone)]
struct RuntimeSkillObservation {
    name: String,
    #[serde(default)]
    runtime: Option<String>,
    #[serde(default)]
    outcome: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CandidateDraft {
    #[serde(rename = "type", alias = "node_type", default = "default_node_type")]
    node_type: String,
    #[serde(default)]
    kind: Option<String>,
    title: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    learn: Option<String>,
    #[serde(default)]
    decide: Option<String>,
    #[serde(default)]
    execute: Option<String>,
    #[serde(default)]
    evidence: Option<String>,
    #[serde(default)]
    outcome: Option<String>,
    #[serde(default)]
    occurred_at: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default, alias = "operation", alias = "action")]
    disposition: Option<String>,
    #[serde(default, alias = "target_id")]
    target: Option<String>,
    #[serde(default, alias = "change")]
    patch: Option<String>,
}

fn default_node_type() -> String {
    "knowledge".into()
}

impl Engine {
    pub fn new(store: Arc<Store>, adapter: Arc<dyn RuntimeAdapter>, home: PathBuf) -> Self {
        let mut adapters = HashMap::new();
        adapters.insert("claude-code".into(), adapter);
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
            .ok_or_else(|| CoreError::UnknownRuntime(runtime.into()))
    }

    fn preferred_runtime(&self, requested: Option<&str>) -> String {
        if let Some(runtime) = requested.filter(|runtime| self.adapters.contains_key(*runtime)) {
            return runtime.into();
        }
        if let Some(runtime) = UserConfig::load(&self.home)
            .default_runtime
            .filter(|runtime| self.adapters.contains_key(runtime))
        {
            return runtime;
        }
        ["claude-code", "codex", "cursor"]
            .into_iter()
            .find(|runtime| self.adapters.contains_key(*runtime))
            .map(str::to_string)
            .or_else(|| self.adapters.keys().next().cloned())
            .unwrap_or_else(|| "claude-code".into())
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
    pub fn context_roots(&self) -> Vec<(String, PathBuf)> {
        crate::mentions::context_roots(&self.home, &self.launch_cwd)
    }

    pub fn sync_graph(&self) -> Result<usize, CoreError> {
        Ok(crate::graph::sync_graph(&self.store, &self.home)?)
    }
    pub fn list_graph_nodes(&self, query: Option<&str>) -> Result<Vec<GraphNode>, CoreError> {
        Ok(self.store.list_graph_nodes(query)?)
    }
    pub fn graph_edges_for(&self, node_id: &str) -> Result<Vec<GraphEdge>, CoreError> {
        Ok(self.store.graph_edges_for(node_id)?)
    }

    // ─── Continuous learning ─────────────────────────────────────────────
    // Thin delegation to `crate::learning`, which owns all of the policy.

    pub fn list_goals(&self) -> Result<Vec<LearningGoal>, CoreError> {
        Ok(self.store.list_learning_goals()?)
    }
    pub fn goal(&self, id: &str) -> Result<Option<LearningGoal>, CoreError> {
        Ok(self.store.learning_goal(id)?)
    }
    pub fn delete_goal(&self, id: &str) -> Result<bool, CoreError> {
        Ok(self.store.delete_learning_goal(id)?)
    }

    /// The YAML document handed to `$EDITOR` for a new Goal.
    pub fn new_goal_form(&self) -> Result<String, CoreError> {
        learning::render_form(&GoalForm::default())
    }

    /// The YAML document handed to `$EDITOR` for an existing Goal.
    pub fn goal_form(&self, id: &str) -> Result<String, CoreError> {
        let goal = self.require_goal(id)?;
        learning::render_form(&GoalForm::from_goal(&goal))
    }

    /// Create a Goal from one stretch of natural language. Policy fields take
    /// their defaults; any resolved `@` paths become durable authorized sources.
    pub fn create_goal_from_objective(&self, objective: &str) -> Result<LearningGoal, CoreError> {
        let mut form = GoalForm::from_objective(objective);
        let roots = self.context_roots();
        form.sources = crate::mentions::resolve_named(objective, &roots)
            .into_iter()
            .map(|mention| mention.raw)
            .fold(Vec::new(), |mut sources, source| {
                if !sources.contains(&source) {
                    sources.push(source);
                }
                sources
            });
        self.create_goal(form)
    }

    pub fn create_goal(&self, form: GoalForm) -> Result<LearningGoal, CoreError> {
        let goal = form.into_new_goal(Utc::now())?;
        self.store.upsert_learning_goal(&goal)?;
        Ok(goal)
    }

    pub fn edit_goal(&self, id: &str, form: GoalForm) -> Result<LearningGoal, CoreError> {
        let mut goal = self.require_goal(id)?;
        form.apply_to(&mut goal, Utc::now())?;
        self.store.upsert_learning_goal(&goal)?;
        Ok(goal)
    }

    /// Flip a Goal on or off without going through the editor.
    pub fn set_goal_enabled(&self, id: &str, enabled: bool) -> Result<LearningGoal, CoreError> {
        let mut goal = self.require_goal(id)?;
        let now = Utc::now();
        goal.enabled = enabled;
        goal.reschedule_all(now);
        goal.updated_at = now;
        self.store.upsert_learning_goal(&goal)?;
        Ok(goal)
    }

    /// Bring a Goal's next turn forward so the upcoming tick picks it up. Going
    /// through the schedule rather than launching directly keeps one dispatch
    /// path, so occupancy, attention and budget checks still apply.
    pub fn request_goal_now(&self, id: &str, work: WorkKind) -> Result<LearningGoal, CoreError> {
        let mut goal = self.require_goal(id)?;
        let now = Utc::now();
        goal.set_next_at(work, Some(now));
        goal.updated_at = now;
        self.store.upsert_learning_goal(&goal)?;
        Ok(goal)
    }

    /// Decide which learning turns are due. `occupied_goal_ids` are the Goals
    /// whose runtime session the caller is already running.
    pub fn plan_learning_tick(
        &self,
        occupied_goal_ids: HashSet<String>,
    ) -> Result<learning::TickPlan, CoreError> {
        learning::plan_tick(
            &self.store,
            &learning::TickInput {
                occupied_goal_ids,
                now: Utc::now(),
                local_time: Local::now().time(),
            },
        )
    }

    pub fn record_goal_spend(&self, goal_id: &str, cost_usd: f64) -> Result<f64, CoreError> {
        learning::record_spend(&self.store, goal_id, cost_usd, Utc::now())
    }

    pub fn goal_spend(&self, goal_id: &str) -> Result<f64, CoreError> {
        learning::goal_spend(&self.store, goal_id, Utc::now())
    }

    pub fn link_goal_run(
        &self,
        run_id: &str,
        goal_id: &str,
        work: WorkKind,
    ) -> Result<(), CoreError> {
        Ok(self.store.link_goal_run(&GoalRun {
            run_id: run_id.to_string(),
            goal_id: goal_id.to_string(),
            work,
            created_at: Utc::now(),
        })?)
    }

    pub fn goal_run(&self, run_id: &str) -> Result<Option<GoalRun>, CoreError> {
        Ok(self.store.goal_run(run_id)?)
    }
    pub fn list_goal_runs(&self, goal_id: &str, limit: usize) -> Result<Vec<GoalRun>, CoreError> {
        Ok(self.store.list_goal_runs(goal_id, limit)?)
    }

    pub fn open_attentions(&self) -> Result<Vec<HumanAttention>, CoreError> {
        Ok(self.store.list_open_attentions()?)
    }
    pub fn attention_for_run(&self, run_id: &str) -> Result<Option<HumanAttention>, CoreError> {
        learning::attention_for_run(&self.store, run_id)
    }
    pub fn resolve_attention(&self, id: &str, response: &str) -> Result<bool, CoreError> {
        Ok(self.store.resolve_attention(id, response, Utc::now())?)
    }

    /// Prepare the native turn that carries a maintainer's answer back into the
    /// run that asked the question.
    ///
    /// An answer is only useful inside the session that is blocked on it, and
    /// only the native handoff can resume one, so answering and resuming are the
    /// same act. The caller resolves the attention once the handoff has run, so
    /// a failed launch leaves the question open rather than swallowing the reply.
    pub fn prepare_attention_handoff(
        &self,
        attention: &HumanAttention,
        answer: &str,
    ) -> Result<NativeLearnHandoff, CoreError> {
        let sources = self.persist_attention_sources(attention, answer)?;
        let run = self
            .list_learning_runs()?
            .into_iter()
            .find(|run| run.run_id == attention.run_id)
            .ok_or_else(|| {
                CoreError::Other(format!("Learn run not found: {}", attention.run_id))
            })?;
        let follow_up = format!(
            "Maintainer answer to \"{}\":\n\n{answer}\n\nTreat this as settled and continue the learning turn.",
            attention.title
        );
        self.continue_native_learning_with_sources(
            &run.runtime,
            &run.permission_mode,
            &run.run_id,
            run.executor_sid.as_deref(),
            &follow_up,
            &sources,
        )
    }

    /// Turn an explicitly authorized source mentioned in an attention exchange
    /// into durable Goal context. The next native turn and all later scheduled
    /// turns then receive the same source roots instead of asking the same
    /// permission question again.
    fn persist_attention_sources(
        &self,
        attention: &HumanAttention,
        answer: &str,
    ) -> Result<Vec<String>, CoreError> {
        let Some(goal_id) = attention.goal_id.as_deref() else {
            return Ok(Vec::new());
        };
        let Some(mut goal) = self.store.learning_goal(goal_id)? else {
            return Ok(Vec::new());
        };

        let answer_paths = source_directories(&self.launch_cwd, answer);
        let mut authorized_paths = answer_paths.clone();
        // A short answer such as "yes, you can" authorizes the path contained
        // in the question. If the maintainer supplied a replacement path in
        // the answer, prefer that explicit path and do not retain the old one.
        if answer_paths.is_empty() && answer_is_affirmative(answer) {
            authorized_paths.extend(source_directories(&self.launch_cwd, &attention.prompt));
        }

        let mut changed = false;
        for path in authorized_paths {
            let path = path.canonicalize().unwrap_or(path);
            let already_known = goal.sources.iter().any(|source| {
                let (existing, _) = resolve_source_path(&self.launch_cwd, source);
                existing
                    .canonicalize()
                    .map(|existing| existing == path)
                    .unwrap_or(false)
            });
            if already_known {
                continue;
            }
            goal.sources.push(path.to_string_lossy().into_owned());
            changed = true;
        }
        if changed {
            goal.updated_at = Utc::now();
            self.store.upsert_learning_goal(&goal)?;
        }
        Ok(goal.sources)
    }

    /// Repair runs written by older runtimes before Methodus required a
    /// structured CandidateSet. Such runs were incorrectly marked
    /// `awaiting_review` with no candidates, which made both Review and the
    /// attention queue look empty. Reclassify them as resumable attention and
    /// backfill source authorization from their resolved exchanges.
    pub fn repair_learning_continuations(&self) -> Result<Vec<String>, CoreError> {
        let runs = self.list_learning_runs()?;
        let mut repaired = Vec::new();
        for run in runs {
            for attention in self.store.list_attentions_for_run(&run.run_id)? {
                if let Some(response) = attention.response.as_deref() {
                    let _ = self.persist_attention_sources(&attention, response)?;
                }
            }
            let assistant_path = self.home.join("runs").join(&run.run_id).join("assistant.md");
            if run.status != "awaiting_review" || !assistant_path.is_file() {
                continue;
            }
            let Ok(output) = fs::read_to_string(&assistant_path) else {
                continue;
            };
            if extract_candidate_set(&output).is_some()
                || self.attention_for_run(&run.run_id)?.is_some()
            {
                continue;
            }
            let attention = self.open_learning_follow_up_attention(&run.run_id, Some(&output))?;
            self.record_learning_event(
                &run.run_id,
                "methodus",
                &format!("Reclassified an unstructured Learn return as attention: {}", attention.title),
            )?;
            repaired.push(run.run_id);
        }
        Ok(repaired)
    }

    /// Record a hand-off if a turn's output carries an attention envelope.
    /// Returns `None` when the turn simply finished.
    pub fn record_attention(
        &self,
        run_id: &str,
        output: &str,
    ) -> Result<Option<HumanAttention>, CoreError> {
        let Some(envelope) = learning::parse_envelope(output) else {
            return Ok(None);
        };
        let goal_id = self.store.goal_run(run_id)?.map(|link| link.goal_id);
        learning::open_attention(&self.store, run_id, goal_id, &envelope, Utc::now()).map(Some)
    }

    /// Open the next durable hand-off for a run that returned without a valid
    /// CandidateSet. If the runtime emitted a proper attention envelope, keep
    /// its exact question; otherwise create an actionable protocol mismatch
    /// item so the maintainer can tell the runtime to continue or finalize.
    fn open_learning_follow_up_attention(
        &self,
        run_id: &str,
        output: Option<&str>,
    ) -> Result<HumanAttention, CoreError> {
        let envelope = output
            .and_then(learning::parse_envelope)
            .unwrap_or_else(|| learning::AttentionEnvelope {
                kind: AttentionKind::Question,
                question: "The Learn runtime did not return a structured CandidateSet. Continue the investigation or finalize the synthesis?".into(),
                context: Some(
                    output
                        .map(|text| {
                            format!(
                                "The runtime returned prose or an invalid JSON contract. Return excerpt: {}",
                                single_line(text).chars().take(500).collect::<String>()
                            )
                        })
                        .unwrap_or_else(|| "The native session ended without a return artifact.".into()),
                ),
                tool_name: None,
                tool_input: None,
            });
        self.mark_learning_status(run_id, "awaiting_input")?;
        let goal_id = self.store.goal_run(run_id)?.map(|link| link.goal_id);
        learning::open_attention(&self.store, run_id, goal_id, &envelope, Utc::now())
    }

    fn record_unstructured_learning_artifact(
        &self,
        run_id: &str,
        goal: &str,
        output: &str,
    ) -> Result<(), CoreError> {
        let run_root = self.home.join("runs").join(run_id);
        fs::create_dir_all(&run_root)?;
        fs::write(run_root.join("assistant.md"), output.trim())?;
        self.record_learning_sources(run_id, goal)
    }

    fn require_goal(&self, id: &str) -> Result<LearningGoal, CoreError> {
        self.store
            .learning_goal(id)?
            .ok_or_else(|| CoreError::Other(format!("goal not found: {id}")))
    }

    /// Prepare a native Use handoff. The graph and attached sources are protected
    /// by the protocol; the Methodus-managed Use workspace follows the same
    /// permission mapping as native Learn and is the only place for temporary writes.
    pub fn prepare_native_use(
        &self,
        runtime: Option<&str>,
        permission_mode: &str,
        session_id: Option<&str>,
        executor_sid: Option<&str>,
        question: &str,
    ) -> Result<NativeUseHandoff, CoreError> {
        let runtime = self.preferred_runtime(runtime);
        let session_id = session_id
            .map(str::to_owned)
            .unwrap_or_else(|| format!("use_{}", Uuid::new_v4()));
        // Keep the launch directory available only for resolving explicit @
        // mentions. It must not become an implicit runtime root for Use.
        let mention_roots = self
            .context_roots()
            .into_iter()
            .map(|(_, path)| path)
            .collect::<Vec<_>>();
        let (question_with_mentions, mentioned_dirs) =
            crate::mentions::prepare_prompt(question, &mention_roots);
        let environment = self.prepare_use_environment(&session_id)?;
        let mut extra_dirs = environment.graph_dirs.clone();
        extra_dirs.extend(mentioned_dirs);
        extra_dirs.sort();
        extra_dirs.dedup();
        let graph_roots = environment
            .graph_dirs
            .iter()
            .map(|path| format!("- {}", path.display()))
            .collect::<Vec<_>>()
            .join("\n");
        let prompt = format!(
            "You are Methodus Use in a native runtime. You own this multi-turn conversation and may inspect the supplied Methodus graph before answering. The Use workspace is managed by Methodus and may contain temporary analysis notes or plans; keep those writes inside the workspace.\n\nQuestion:\n{question_with_mentions}\n\nRead this file first:\n{}\n\nThe file is the Methodus Use environment contract and inventory. It lists the authoritative consumer-visible Markdown files, their statuses, paths, facets, evidence references, selected Team, and directory structure. Use Read, Glob, and Grep to inspect the complete relevant files yourself; do not rely on a preselected answer bundle.\n\nGraph directories opened for this turn:\n{graph_roots}\n\nAnswer contract:\n- Answer in the user's language and start with a direct answer.\n- Separate graph facts, inferences, contradictions, and unknowns.\n- Cite the Methodus node ID and relative path for every graph-based claim.\n- Treat committed nodes as current graph knowledge; label stale nodes and do not present them as unqualified current rules.\n- If no relevant committed evidence remains after inspecting the graph, do not invent an answer. Explain the evidence gap, recommend one concrete Learn task, and write exactly one JSON object to the return path in the contract with outcome `learning_recommended`, a `learning_task`, and a concise `context`. Methodus will route that recommendation to /attention.\n- If the graph does support an answer, do not create a CandidateSet or a learning recommendation.\n- You may read explicitly attached @ sources, but distinguish them from Methodus graph evidence.\n- Do not modify graph files, project files, or attached @ sources. Do not create a CandidateSet.\n\nUse runtime workspace:\n{}\n\nUse return path:\n{}",
            environment.manifest_path.display(),
            environment.cwd.display(),
            environment.return_path.display(),
        );
        let resume_sid = (runtime == "claude-code")
            .then_some(executor_sid)
            .flatten()
            .filter(|sid| Uuid::parse_str(sid).is_ok());
        let executor_sid = resume_sid
            .map(str::to_owned)
            .or_else(|| (runtime == "claude-code").then(|| Uuid::new_v4().to_string()));
        let (program, args) = native_use_command(
            &runtime,
            permission_mode,
            &environment.cwd,
            &extra_dirs,
            executor_sid.as_deref(),
            resume_sid.is_some(),
            &prompt,
        )?;
        Ok(NativeUseHandoff {
            session_id,
            question: question.to_string(),
            runtime,
            cwd: environment.cwd,
            return_path: environment.return_path,
            program,
            args,
            executor_sid,
        })
    }

    /// Consume the optional structured return from a native Use turn. A normal
    /// answer has no return file; a `learning_recommended` envelope becomes a
    /// durable Methodus attention item for the maintainer.
    pub fn complete_native_use(
        &self,
        handoff: &NativeUseHandoff,
    ) -> Result<Option<HumanAttention>, CoreError> {
        let output = match fs::read_to_string(&handoff.return_path) {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let Some(envelope) = learning::parse_learning_recommendation(&output) else {
            return Err(CoreError::Other(format!(
                "invalid native Use return contract: {}",
                handoff.return_path.display()
            )));
        };
        let goal_id = self.store.goal_run(&handoff.session_id)?.map(|link| link.goal_id);
        learning::open_attention(
            &self.store,
            &handoff.session_id,
            goal_id,
            &envelope,
            Utc::now(),
        )
        .map(Some)
    }

    fn runtime_workspace(&self, kind: &str, session_id: &str) -> Result<PathBuf, CoreError> {
        let safe_id = session_id
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                    ch
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let safe_id = if safe_id.is_empty() {
            "session"
        } else {
            safe_id.as_str()
        };
        let workspace = self
            .home
            .join("workspaces")
            .join(kind)
            .join(safe_id);
        fs::create_dir_all(&workspace)?;
        Ok(workspace)
    }

    fn use_workspace(&self, session_id: &str) -> Result<PathBuf, CoreError> {
        self.runtime_workspace("use", session_id)
    }

    fn prepare_use_environment(&self, session_id: &str) -> Result<UseEnvironment, CoreError> {
        let cwd = self.use_workspace(session_id)?;
        let manifest = AgentQuery::new(&self.store, &self.home).manifest(&[])?;
        let mut graph_dirs = manifest
            .graph_roots
            .iter()
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        graph_dirs.sort();
        graph_dirs.dedup();
        let manifest_path = cwd.join("METHODUS_USE.md");
        let return_path = cwd.join(format!(
            "METHODUS_USE_RETURN-{}.json",
            Uuid::new_v4().simple()
        ));
        fs::write(
            &manifest_path,
            Self::render_use_manifest(&manifest, &graph_dirs, &return_path),
        )?;
        Ok(UseEnvironment {
            cwd,
            manifest_path,
            return_path,
            graph_dirs,
        })
    }

    fn prepare_learn_environment(&self, cwd: &Path) -> Result<LearnEnvironment, CoreError> {
        let manifest = AgentQuery::new(&self.store, &self.home).manifest(&[])?;
        let mut graph_dirs = manifest
            .graph_roots
            .iter()
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        graph_dirs.sort();
        graph_dirs.dedup();
        let manifest_path = cwd.join("METHODUS_LEARN.md");
        fs::write(
            &manifest_path,
            Self::render_learn_manifest(&manifest, &graph_dirs),
        )?;
        Ok(LearnEnvironment {
            manifest_path,
            graph_dirs,
        })
    }

    fn render_learn_manifest(manifest: &AgentManifest, graph_dirs: &[PathBuf]) -> String {
        let mut out = String::from(
            "# Methodus Learn environment\n\nThis file is the read-only graph snapshot and contract for one Learn turn. The Methodus-managed workspace may hold temporary analysis notes; the listed graph directories and attached source directories are evidence and must not be modified.\n\n",
        );
        out.push_str(&format!(
            "## Graph snapshot\n\n- protocol: {}\n- index revision: {}\n- selected team: {}\n- visible nodes: {}\n",
            manifest.protocol_version,
            manifest.index_revision,
            manifest.selected_team,
            manifest.items.len()
        ));
        out.push_str("\n## Directory structure\n\n");
        for directory in &manifest.directory_structure {
            out.push_str(&format!(
                "- {}{}\n",
                directory.path,
                if directory.exists { "" } else { " (missing)" }
            ));
        }
        out.push_str("\n## Open graph directories\n\n");
        if graph_dirs.is_empty() {
            out.push_str("- none: the current graph has no validated consumer-visible files\n");
        } else {
            for path in graph_dirs {
                out.push_str(&format!("- {}\n", path.display()));
            }
        }
        out.push_str("\n## Consumer-visible inventory\n\n");
        if manifest.items.is_empty() {
            out.push_str(
                "No committed or stale Knowledge, Method, or Experience nodes are indexed.\n",
            );
        } else {
            for item in &manifest.items {
                out.push_str(&format!(
                    "### {} · {}\n\n- id: {}\n- status: {}\n- visibility: {}\n- path: {}\n- kind: {}\n- summary: {}\n- facets: {}\n",
                    item.node_type,
                    item.title,
                    item.id,
                    item.status,
                    item.visibility,
                    item.path,
                    item.kind.as_deref().unwrap_or("unknown"),
                    single_line(&item.summary),
                    if item.facets.is_empty() {
                        "none".into()
                    } else {
                        item.facets.join(", ")
                    }
                ));
                if !item.tags.is_empty() {
                    out.push_str(&format!("- tags: {}\n", item.tags.join(", ")));
                }
                if !item.sources.is_empty() {
                    let sources = item
                        .sources
                        .iter()
                        .map(|source| source.path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push_str(&format!("- evidence references: {sources}\n"));
                }
                out.push('\n');
            }
        }
        if !manifest.warnings.is_empty() {
            out.push_str("## Warnings\n\n");
            for warning in &manifest.warnings {
                out.push_str(&format!("- {warning}\n"));
            }
        }
        out.push_str(
            "\n## Learn reading policy\n\n- Read this file before investigating external sources.\n- Use the inventory to choose likely matches, then read their complete Markdown bodies from the open graph directories.\n- `committed` nodes are current graph knowledge; `stale` nodes require an explicit stale warning. Candidate, rejected, and deprecated files are not canonical evidence.\n- Do not modify graph files, project files, or attached @ sources. Keep temporary notes in the Learn workspace.\n- Cite exact node IDs and relative paths in candidate evidence and integration proposals.\n",
        );
        out
    }

    fn render_use_manifest(
        manifest: &AgentManifest,
        graph_dirs: &[PathBuf],
        return_path: &Path,
    ) -> String {
        let mut out = String::from(
            "# Methodus Use environment\n\nThis file is the contract and inventory for one Use turn. The Methodus-managed workspace may hold temporary analysis artifacts; the listed graph and attached source directories are evidence and must not be modified.\n\n",
        );
        out.push_str(&format!(
            "## Graph snapshot\n\n- protocol: {}\n- index revision: {}\n- selected team: {}\n- visible nodes: {}\n",
            manifest.protocol_version,
            manifest.index_revision,
            manifest.selected_team,
            manifest.items.len()
        ));
        out.push_str("\n## Directory structure\n\n");
        for directory in &manifest.directory_structure {
            out.push_str(&format!(
                "- {}{}\n",
                directory.path,
                if directory.exists { "" } else { " (missing)" }
            ));
        }
        out.push_str("\n## Open graph directories\n\n");
        if graph_dirs.is_empty() {
            out.push_str("- none: the current graph has no validated consumer-visible files\n");
        } else {
            for path in graph_dirs {
                out.push_str(&format!("- {}\n", path.display()));
            }
        }
        out.push_str("\n## Consumer-visible inventory\n\n");
        if manifest.items.is_empty() {
            out.push_str(
                "No committed or stale Knowledge, Method, or Experience nodes are indexed.\n",
            );
        } else {
            for item in &manifest.items {
                out.push_str(&format!(
                    "### {} · {}\n\n- id: {}\n- status: {}\n- visibility: {}\n- path: {}\n- kind: {}\n- summary: {}\n- facets: {}\n",
                    item.node_type,
                    item.title,
                    item.id,
                    item.status,
                    item.visibility,
                    item.path,
                    item.kind.as_deref().unwrap_or("unknown"),
                    single_line(&item.summary),
                    if item.facets.is_empty() {
                        "none".into()
                    } else {
                        item.facets.join(", ")
                    }
                ));
                if !item.tags.is_empty() {
                    out.push_str(&format!("- tags: {}\n", item.tags.join(", ")));
                }
                if !item.sources.is_empty() {
                    let sources = item
                        .sources
                        .iter()
                        .map(|source| source.path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push_str(&format!("- evidence references: {sources}\n"));
                }
                out.push('\n');
            }
        }
        if !manifest.warnings.is_empty() {
            out.push_str("## Warnings\n\n");
            for warning in &manifest.warnings {
                out.push_str(&format!("- {warning}\n"));
            }
        }
        out.push_str(&format!(
            "\n## Use return contract\n\n- If no committed evidence is sufficient to answer, do not invent an answer. Recommend one concrete Learn task and write exactly one JSON object to `{}`.\n- The JSON must have this shape: `{{\"outcome\":\"learning_recommended\",\"learning_task\":\"...\",\"context\":\"...\"}}`.\n- Do not write a CandidateSet or create a Goal directly from the runtime. Methodus routes the recommendation to `/attention`.\n",
            return_path.display()
        ));
        out.push_str(
            "\n## Reading policy\n\n- Only the nodes listed above are consumer-visible for this turn.\n- Candidate, rejected, and deprecated files are not evidence for Use.\n- A stale node must be labeled as stale and not presented as an unqualified current rule.\n- The Use workspace may contain temporary notes or plans; keep all such writes inside that workspace.\n- Do not modify graph files, project files, or attached @ sources.\n- External evidence paths are references; read them only when the user explicitly attaches them with @.\n- Cite the node ID and relative path when relying on a graph claim.\n",
        );
        out
    }

    /// Run one scheduled turn to completion without a terminal.
    ///
    /// Unattended turns are the point of a cadence: they happen while nobody is
    /// watching, so whatever they produce lands in Review or the attention queue
    /// instead of on screen. A maintainer who wants to dig in afterwards resumes
    /// the same executor session through the native handoff.
    pub async fn run_scheduled_turn(
        &self,
        turn: &learning::ScheduledTurn,
    ) -> Result<learning::TurnOutcome, CoreError> {
        let goal = &turn.goal;
        let (handle, mut events) = self
            .start_learning_with_sources(
                Some(&goal.runtime),
                &goal.permission_mode,
                &turn.prompt,
                &goal.sources,
            )
            .await?;
        let run_id = handle.session_id.clone();
        self.link_goal_run(&run_id, &goal.id, turn.work)?;

        let mut transcript = learning::TurnTranscript::default();
        let mut stream_error: Option<String> = None;
        let mut result: Option<(bool, String, Vec<PermissionDenial>)> = None;
        let mut cost_usd = 0.0;
        while let Some(event) = events.recv().await {
            self.record_runtime_event(&run_id, &event);
            match event {
                RuntimeEvent::SessionStarted { session_id } => {
                    let _ = self.update_learning_executor_sid(&run_id, &session_id);
                }
                RuntimeEvent::AssistantText { text } => transcript.push_assistant(&text),
                RuntimeEvent::Error { message } => stream_error = Some(message),
                RuntimeEvent::Result {
                    is_error,
                    text,
                    cost_usd: cost,
                    usage,
                    session_id,
                    permission_denials,
                } => {
                    if let Some(session_id) = session_id.as_deref() {
                        let _ = self.update_learning_executor_sid(&run_id, session_id);
                    }
                    cost_usd = cost.unwrap_or_default();
                    let delta = UsageDelta::from_result(cost, usage.as_ref());
                    if !delta.is_empty() {
                        let _ = self.store.insert_usage(
                            Some(&run_id),
                            Some(&run_id),
                            Some(&goal.runtime),
                            &delta,
                        );
                    }
                    result = Some((is_error, text, permission_denials));
                }
                _ => {}
            }
        }

        let output = transcript.finish(
            result
                .as_ref()
                .map(|(_, text, _)| text.as_str())
                .unwrap_or_default(),
        );
        let disposition = match (stream_error, &result) {
            (Some(message), _) => learning::TurnDisposition::Failed { message },
            (None, Some((is_error, _, denials))) => learning::classify(&output, *is_error, denials),
            (None, None) => learning::TurnDisposition::Failed {
                message: "the runtime exited without returning a result".into(),
            },
        };

        let mut candidate_ids = Vec::new();
        let mut attention = None;
        let mut failure = None;
        match disposition {
            learning::TurnDisposition::Failed { message } => {
                let _ = self.mark_learning_status(&run_id, "failed");
                failure = Some(message);
            }
            learning::TurnDisposition::AwaitingInput { envelope } => {
                let _ = self.mark_learning_status(&run_id, "awaiting_input");
                attention = Some(learning::open_attention(
                    &self.store,
                    &run_id,
                    Some(goal.id.clone()),
                    &envelope,
                    Utc::now(),
                )?);
            }
            learning::TurnDisposition::Completed if output.trim().is_empty() => {
                attention = Some(self.open_learning_follow_up_attention(&run_id, None)?);
            }
            learning::TurnDisposition::Completed => {
                if extract_candidate_set(&output).is_some() {
                    candidate_ids =
                        self.record_learning_output_for_run(&run_id, &turn.prompt, &output)?;
                } else {
                    // A prose-only return is not a successful Learn result.
                    // Keep the run in the same conversation and make the
                    // missing/invalid contract visible through attention.
                    self.record_unstructured_learning_artifact(
                        &run_id,
                        &turn.prompt,
                        &output,
                    )?;
                    attention = Some(
                        self.open_learning_follow_up_attention(&run_id, Some(&output))?,
                    );
                }
            }
        }

        let spent_usd = learning::record_spend(&self.store, &goal.id, cost_usd, Utc::now())?;
        Ok(learning::TurnOutcome {
            run_id,
            goal_id: goal.id.clone(),
            goal_title: goal.title.clone(),
            work: turn.work,
            candidate_ids,
            attention,
            failure,
            cost_usd,
            spent_usd,
            budget_usd: goal.budget_usd,
        })
    }

    /// Append one runtime event to a run transcript. Best effort on purpose: a
    /// transcript write must never abort a turn that is otherwise progressing.
    fn record_runtime_event(&self, run_id: &str, event: &RuntimeEvent) {
        let (role, text) = match event {
            RuntimeEvent::SessionStarted { session_id } => {
                ("runtime", format!("session started: {session_id}"))
            }
            RuntimeEvent::UserText { text } => ("user", text.clone()),
            RuntimeEvent::AssistantText { text } => ("assistant", text.clone()),
            RuntimeEvent::Thinking { text } => ("thinking", text.clone()),
            RuntimeEvent::ToolCallStarted { name, .. } => ("tool", format!("started {name}")),
            RuntimeEvent::ToolCallCompleted { id, exit_code, .. } => (
                "tool",
                match exit_code {
                    Some(code) => format!("completed {id} (exit {code})"),
                    None => format!("completed {id}"),
                },
            ),
            RuntimeEvent::TurnCompleted { stop_reason } => (
                "runtime",
                match stop_reason {
                    Some(reason) => format!("turn completed: {reason}"),
                    None => "turn completed".to_string(),
                },
            ),
            RuntimeEvent::ApprovalRequested { tool_name, .. } => {
                ("runtime", format!("approval requested: {tool_name}"))
            }
            RuntimeEvent::Error { message } => ("runtime", format!("error: {message}")),
            RuntimeEvent::Result { is_error, .. } => {
                ("runtime", format!("result (error: {is_error})"))
            }
        };
        let _ = self.record_learning_event(run_id, role, &text);
    }
    /// Read an indexed Markdown graph document for maintainer-facing detail
    /// views through the same managed-root safety check used by Review.
    pub fn graph_document(&self, node_id: &str) -> Result<crate::graph::GraphDocument, CoreError> {
        let node = self
            .store
            .graph_node(node_id)?
            .ok_or_else(|| CoreError::Other(format!("graph node not found: {node_id}")))?;
        let path = self.node_path(&node)?;
        Ok(crate::graph::read_graph_document(&self.home, &path)?)
    }
    pub fn team_id(&self) -> String {
        UserConfig::load(&self.home).selected_team().to_string()
    }
    pub fn team_root(&self) -> PathBuf {
        self.home.join("teams").join(self.team_id())
    }

    pub fn list_learning_runs(&self) -> Result<Vec<LearnRun>, CoreError> {
        let root = self.home.join("runs");
        if !root.is_dir() {
            return Ok(Vec::new());
        }
        let mut runs = Vec::new();
        for entry in fs::read_dir(root)? {
            let path = entry?.path().join("state.yaml");
            if !path.is_file() {
                continue;
            }
            let Ok(state) = serde_yaml::from_str::<LearnRunState>(&fs::read_to_string(path)?)
            else {
                continue;
            };
            if state.kind != "learn" {
                continue;
            }
            runs.push(LearnRun {
                run_id: state.run_id,
                goal: state.goal,
                runtime: state.runtime,
                permission_mode: state.permission_mode,
                status: state.status,
                executor_sid: state.executor_sid,
                updated_at: state.updated_at,
            });
        }
        runs.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        Ok(runs)
    }

    pub fn latest_resumable_learning(&self) -> Result<Option<LearnRun>, CoreError> {
        Ok(self
            .list_learning_runs()?
            .into_iter()
            .find(|run| matches!(run.status.as_str(), "running" | "awaiting_input" | "failed")))
    }

    fn write_learn_state(
        &self,
        run_id: &str,
        goal: &str,
        runtime: &str,
        permission_mode: Option<&str>,
        status: &str,
        executor_sid: Option<&str>,
    ) -> Result<(), CoreError> {
        let root = self.home.join("runs").join(run_id);
        fs::create_dir_all(&root)?;
        let previous = fs::read_to_string(root.join("state.yaml"))
            .ok()
            .and_then(|raw| serde_yaml::from_str::<LearnRunState>(&raw).ok());
        let state = LearnRunState {
            run_id: run_id.into(),
            kind: "learn".into(),
            status: status.into(),
            goal: goal.into(),
            runtime: runtime.into(),
            permission_mode: permission_mode
                .map(|mode| permission_profile(mode).0.to_string())
                .or_else(|| previous.as_ref().map(|state| state.permission_mode.clone()))
                .unwrap_or_else(default_permission_mode),
            executor_sid: executor_sid.map(str::to_string).or_else(|| {
                previous
                    .as_ref()
                    .and_then(|state| state.executor_sid.clone())
            }),
            unresolved_questions: previous
                .as_ref()
                .map(|state| state.unresolved_questions.clone())
                .unwrap_or_default(),
            contradictions: previous
                .as_ref()
                .map(|state| state.contradictions.clone())
                .unwrap_or_default(),
            updated_at: Utc::now().to_rfc3339(),
        };
        fs::write(
            root.join("state.yaml"),
            serde_yaml::to_string(&state)
                .map_err(|error| CoreError::Other(format!("serialize Learn state: {error}")))?,
        )?;
        Ok(())
    }

    pub fn record_learning_event(
        &self,
        run_id: &str,
        role: &str,
        text: &str,
    ) -> Result<(), CoreError> {
        let root = self.home.join("runs").join(run_id);
        fs::create_dir_all(&root)?;
        let event =
            serde_json::json!({ "at": Utc::now().to_rfc3339(), "role": role, "text": text });
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("events.jsonl"))?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&event).unwrap_or_else(|_| "{}".into())
        )?;
        Ok(())
    }

    pub fn learning_events(&self, run_id: &str) -> Result<Vec<LearnEventRecord>, CoreError> {
        let path = self.home.join("runs").join(run_id).join("events.jsonl");
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let mut events = Vec::new();
        for line in fs::read_to_string(path)?.lines() {
            if let Ok(event) = serde_json::from_str::<LearnEventRecord>(line) {
                events.push(event);
            }
        }
        Ok(events)
    }

    /// Persist source locators mentioned in a Learn prompt. This records only
    /// metadata and fingerprints; it never copies source contents into the run.
    pub fn record_learning_sources(&self, run_id: &str, text: &str) -> Result<(), CoreError> {
        let root = self.home.join("runs").join(run_id);
        fs::create_dir_all(&root)?;
        let mut entries = fs::read_to_string(root.join("sources.yaml"))
            .ok()
            .and_then(|raw| serde_yaml::from_str::<SourceManifest>(&raw).ok())
            .map(|manifest| manifest.sources)
            .unwrap_or_default();
        let mut seen = entries
            .iter()
            .map(|entry| (entry.locator.clone(), ()))
            .collect::<BTreeMap<_, _>>();
        for token in text
            .split_whitespace()
            .filter_map(|token| source_token(token, &self.launch_cwd))
        {
            if seen.insert(token.clone(), ()).is_some() {
                continue;
            }
            let (path, display) = resolve_source_path(&self.launch_cwd, &token);
            let (fingerprint, status) = if path.is_dir() {
                (None, "current")
            } else {
                match fs::read(&path) {
                    Ok(bytes) => (
                        Some(format!("sha256:{:x}", Sha256::digest(bytes))),
                        "current",
                    ),
                    Err(_) => (None, "missing"),
                }
            };
            entries.push(SourceManifestEntry {
                locator: display,
                path: path.display().to_string(),
                fingerprint,
                status: status.into(),
            });
        }
        fs::write(
            root.join("sources.yaml"),
            serde_yaml::to_string(&SourceManifest { sources: entries })
                .map_err(|error| CoreError::Other(format!("serialize source manifest: {error}")))?,
        )?;
        Ok(())
    }

    /// Append Goal-authorized roots to a run's source manifest without relying on
    /// whitespace tokenization (paths may contain spaces, and URLs are preserved).
    pub fn record_learning_authorized_sources(
        &self,
        run_id: &str,
        sources: &[String],
    ) -> Result<(), CoreError> {
        let root = self.home.join("runs").join(run_id);
        fs::create_dir_all(&root)?;
        let mut entries = fs::read_to_string(root.join("sources.yaml"))
            .ok()
            .and_then(|raw| serde_yaml::from_str::<SourceManifest>(&raw).ok())
            .map(|manifest| manifest.sources)
            .unwrap_or_default();
        let mut seen = entries
            .iter()
            .map(|entry| entry.locator.clone())
            .collect::<std::collections::BTreeSet<_>>();
        for source in sources
            .iter()
            .map(|source| source.trim())
            .filter(|source| !source.is_empty())
        {
            if !seen.insert(source.to_string()) {
                continue;
            }
            if source.contains("://") {
                entries.push(SourceManifestEntry {
                    locator: source.to_string(),
                    path: source.to_string(),
                    fingerprint: None,
                    status: "remote".into(),
                });
                continue;
            }
            let (path, display) = resolve_source_path(&self.launch_cwd, source);
            let (fingerprint, status) = if path.is_dir() {
                (None, "current")
            } else {
                match fs::read(&path) {
                    Ok(bytes) => (
                        Some(format!("sha256:{:x}", Sha256::digest(bytes))),
                        "current",
                    ),
                    Err(_) => (None, "missing"),
                }
            };
            entries.push(SourceManifestEntry {
                locator: display,
                path: path.display().to_string(),
                fingerprint,
                status: status.into(),
            });
        }
        fs::write(
            root.join("sources.yaml"),
            serde_yaml::to_string(&SourceManifest { sources: entries })
                .map_err(|error| CoreError::Other(format!("serialize source manifest: {error}")))?,
        )?;
        Ok(())
    }

    pub fn mark_learning_status(&self, run_id: &str, status: &str) -> Result<(), CoreError> {
        let Some(run) = self
            .list_learning_runs()?
            .into_iter()
            .find(|run| run.run_id == run_id)
        else {
            return Err(CoreError::Other(format!("Learn run not found: {run_id}")));
        };
        self.write_learn_state(
            &run.run_id,
            &run.goal,
            &run.runtime,
            Some(&run.permission_mode),
            status,
            run.executor_sid.as_deref(),
        )
    }

    /// Persist the executor-owned session id as soon as a runtime reports it.
    /// Adapters are allowed to negotiate a different id than the one Methodus
    /// requested (notably Codex), so the first `SessionStarted` event is the
    /// source of truth for later app-only resume.
    pub fn update_learning_executor_sid(
        &self,
        run_id: &str,
        executor_sid: &str,
    ) -> Result<(), CoreError> {
        let Some(run) = self
            .list_learning_runs()?
            .into_iter()
            .find(|run| run.run_id == run_id)
        else {
            return Err(CoreError::Other(format!("Learn run not found: {run_id}")));
        };
        self.write_learn_state(
            &run.run_id,
            &run.goal,
            &run.runtime,
            Some(&run.permission_mode),
            &run.status,
            Some(executor_sid),
        )
    }

    pub fn team_status(&self) -> Result<TeamStatus, CoreError> {
        let team_id = self.team_id();
        let root = self.team_root();
        let team_prefix = format!("teams/{team_id}/");
        let validation_issues = crate::graph::validate_graph(&self.home)?
            .into_iter()
            .filter(|issue| issue.path.starts_with(&team_prefix))
            .collect();
        if !root.is_dir() {
            return Ok(TeamStatus {
                team_id,
                root,
                is_git: false,
                branch: None,
                dirty: false,
                changes: Vec::new(),
                validation_issues,
                diff: String::new(),
            });
        }
        let git = |args: &[&str]| Command::new("git").args(args).current_dir(&root).output();
        let Ok(status) = git(&["status", "--porcelain=v1", "--branch"]) else {
            return Ok(TeamStatus {
                team_id,
                root,
                is_git: false,
                branch: None,
                dirty: false,
                changes: Vec::new(),
                validation_issues,
                diff: String::new(),
            });
        };
        if !status.status.success() {
            return Ok(TeamStatus {
                team_id,
                root,
                is_git: false,
                branch: None,
                dirty: false,
                changes: Vec::new(),
                validation_issues,
                diff: String::new(),
            });
        }
        let lines = String::from_utf8_lossy(&status.stdout)
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let branch = lines
            .first()
            .and_then(|line| line.strip_prefix("## "))
            .map(str::to_string);
        let changes = lines.into_iter().skip(1).collect::<Vec<_>>();
        let diff = git(&[
            "diff",
            "--no-ext-diff",
            "--",
            "knowledge",
            "methods",
            "experiences",
        ])
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
        .unwrap_or_default();
        Ok(TeamStatus {
            team_id,
            root,
            is_git: true,
            branch,
            dirty: !changes.is_empty(),
            changes,
            validation_issues,
            diff,
        })
    }

    /// Write a local, reviewable publish plan. It never commits, pushes, merges,
    /// or discards a Team working tree.
    pub fn create_team_publish_plan(&self) -> Result<PathBuf, CoreError> {
        let status = self.team_status()?;
        let blocking = status
            .validation_issues
            .iter()
            .filter(|issue| issue.severity == crate::graph::IssueSeverity::Error)
            .map(|issue| format!("{}: {}", issue.path, issue.message))
            .collect::<Vec<_>>();
        if !blocking.is_empty() {
            return Err(CoreError::Other(format!(
                "Team publish blocked by validation errors: {}",
                blocking.join("; ")
            )));
        }
        let run_id = format!("publish_{}", Uuid::new_v4());
        let root = self.home.join("runs").join(&run_id);
        fs::create_dir_all(&root)?;
        let issues = if status.validation_issues.is_empty() {
            "none".into()
        } else {
            status
                .validation_issues
                .iter()
                .map(|issue| {
                    format!(
                        "- [{}] {}: {}",
                        issue.severity.as_str(),
                        issue.path,
                        issue.message
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let changes = if status.changes.is_empty() {
            "none".into()
        } else {
            status.changes.join("\n")
        };
        fs::write(root.join("publish-plan.md"), format!("# Team publish plan\n\nroot: {}\ngit: {}\nbranch: {}\ndirty: {}\n\n## Validation\n\n{}\n\n## Changes\n\n```text\n{}\n```\n\n## Diff\n\n```diff\n{}\n```\n\nThis is a plan only. Review and commit/push with normal Git tooling.\n", status.root.display(), status.is_git, status.branch.as_deref().unwrap_or("unknown"), status.dirty, issues, changes, status.diff))?;
        Ok(root.join("publish-plan.md"))
    }

    /// Start a focused learning conversation with a maintainer-selected permission mode. No task, workspace,
    /// capsule, or native coding session is created.
    pub async fn start_learning(
        &self,
        runtime: Option<&str>,
        permission_mode: &str,
        goal: &str,
    ) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), CoreError> {
        self.start_learning_with_sources(runtime, permission_mode, goal, &[])
            .await
    }

    /// Start a focused learning conversation with explicit, user-authorized source roots.
    /// The runtime starts in a Methodus-managed workspace; the read-only graph
    /// snapshot and directories plus mentioned or explicitly authorized source
    /// directories are added beside it.
    pub async fn start_learning_with_sources(
        &self,
        runtime: Option<&str>,
        permission_mode: &str,
        goal: &str,
        sources: &[String],
    ) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), CoreError> {
        let runtime = self.preferred_runtime(runtime);
        let (permission_mode, sandbox) = permission_profile(permission_mode);
        let runtime_sandbox = if permission_mode == "plan" {
            "workspace-write"
        } else {
            sandbox
        };
        let adapter = self.adapter(&runtime)?;
        let mention_roots = self
            .context_roots()
            .into_iter()
            .map(|(_, path)| path)
            .collect::<Vec<_>>();
        let (goal_with_mentions, mentioned_dirs) =
            crate::mentions::prepare_prompt(goal, &mention_roots);
        let session_id = format!("learn_{}", Uuid::new_v4());
        let workspace = self.runtime_workspace("learn", &session_id)?;
        let environment = self.prepare_learn_environment(&workspace)?;
        let protocol = fs::read_to_string(self.home.join("protocols/deliberate-learning.md"))
            .unwrap_or_else(|_| "Clarify the goal, inspect evidence, challenge assumptions, verify counterexamples, and propose candidates for review.".into());
        let graph_roots = environment
            .graph_dirs
            .iter()
            .map(|path| format!("- {}", path.display()))
            .collect::<Vec<_>>()
            .join("\n");
        let prompt = format!(
            "You are the Methodus deliberate-learning runtime.\n\nRuntime workspace:\n{}\n\nRead this file first:\n{}\n\nOpen graph directories:\n{}\n\nLearning goal:\n{goal_with_mentions}\n\nFollow this protocol:\n{protocol}\n\nMandatory graph integration contract:\n{graph_contract}\n\nSeparate facts, inferences, contradictions, and unknowns. Ask consequential maintainer questions. Never claim a draft is canonical. If a consequential decision or permission is needed, pause and finish with a fenced `json` block exactly like {{\"outcome\":\"needs_input\",\"question\":\"one focused question for the maintainer\",\"context\":\"why this blocks reliable learning\"}} (or use `permission_required` with `tool_name` and `tool_input`). Do not invent a CandidateSet in that case. When evidence is sufficient, finish with a CandidateSet only according to the graph integration contract.",
            workspace.display(),
            environment.manifest_path.display(),
            graph_roots,
            graph_contract = LEARN_GRAPH_INTEGRATION_CONTRACT,
        );
        self.write_learn_state(
            &session_id,
            goal,
            &runtime,
            Some(permission_mode),
            "running",
            None,
        )?;
        self.record_learning_sources(&session_id, goal)?;
        self.record_learning_authorized_sources(&session_id, sources)?;
        let mut extra_dirs = environment.graph_dirs.clone();
        extra_dirs.extend(mentioned_dirs);
        extra_dirs.extend(source_directories(&self.launch_cwd, goal));
        extra_dirs.extend(authorized_source_directories(&self.launch_cwd, sources));
        extra_dirs.sort();
        extra_dirs.dedup();
        let spawn = adapter
            .spawn(SpawnInput {
                prompt,
                cwd: workspace,
                session_id: session_id.clone(),
                executor_session_id: Some(Uuid::new_v4().to_string()),
                permission_mode: permission_mode.into(),
                allowed_tools: vec![
                    "Read".into(),
                    "Glob".into(),
                    "Grep".into(),
                    "WebSearch".into(),
                ],
                sandbox: Some(runtime_sandbox.into()),
                extra_dirs,
                model: None,
            })
            .await;
        let (handle, events) = match spawn {
            Ok(result) => result,
            Err(error) => {
                let _ = self.mark_learning_status(&session_id, "failed");
                return Err(error.into());
            }
        };
        self.write_learn_state(
            &session_id,
            goal,
            &runtime,
            Some(permission_mode),
            "running",
            handle.executor_sid.as_deref(),
        )?;
        Ok((handle, events))
    }

    /// Continue the same focused Learn conversation using the runtime executor ID.
    pub async fn continue_learning(
        &self,
        runtime: &str,
        permission_mode: &str,
        executor_sid: &str,
        session_id: &str,
        prompt: &str,
    ) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), CoreError> {
        let adapter = self.adapter(runtime)?;
        let (permission_mode, sandbox) = permission_profile(permission_mode);
        let runtime_sandbox = if permission_mode == "plan" {
            "workspace-write"
        } else {
            sandbox
        };
        let mention_roots = self
            .context_roots()
            .into_iter()
            .map(|(_, path)| path)
            .collect::<Vec<_>>();
        let (prompt_with_mentions, mentioned_dirs) =
            crate::mentions::prepare_prompt(prompt, &mention_roots);
        let workspace = self.runtime_workspace("learn", session_id)?;
        let environment = self.prepare_learn_environment(&workspace)?;
        let mut extra_dirs = environment.graph_dirs.clone();
        extra_dirs.extend(mentioned_dirs);
        extra_dirs.extend(source_directories(&self.launch_cwd, prompt));
        extra_dirs.sort();
        extra_dirs.dedup();
        let goal = self
            .list_learning_runs()?
            .into_iter()
            .find(|run| run.run_id == session_id)
            .map(|run| run.goal)
            .unwrap_or_else(|| prompt.into());
        let continuation_prompt = format!(
            "Read the current Methodus Learn environment first: {}\n\nMandatory graph integration contract:\n{}\n\nMaintainer follow-up:\n{}",
            environment.manifest_path.display(),
            LEARN_GRAPH_INTEGRATION_CONTRACT,
            prompt_with_mentions,
        );
        let input = SpawnInput {
            prompt: continuation_prompt.clone(),
            cwd: workspace,
            session_id: session_id.into(),
            executor_session_id: None,
            permission_mode: permission_mode.into(),
            allowed_tools: vec![
                "Read".into(),
                "Glob".into(),
                "Grep".into(),
                "WebSearch".into(),
            ],
            sandbox: Some(runtime_sandbox.into()),
            extra_dirs,
            model: None,
        };
        let restart_with_fresh_claude =
            runtime == "claude-code" && Uuid::parse_str(executor_sid).is_err();
        let fresh_executor_sid = restart_with_fresh_claude.then(|| Uuid::new_v4().to_string());
        let resume = if restart_with_fresh_claude {
            let recovery_prompt = format!(
                "The previous Claude Code session for this Learn run could not be resumed. Continue the same learning task in a fresh session.\n\nOriginal learning goal:\n{goal}\n\n{}",
                continuation_prompt,
            );
            let _ = self.record_learning_event(session_id, "methodus", "Previous Runtime session id was invalid; started a fresh session while preserving this Learn run.");
            adapter
                .spawn(SpawnInput {
                    prompt: recovery_prompt,
                    executor_session_id: fresh_executor_sid.clone(),
                    ..input
                })
                .await
        } else {
            adapter.resume(executor_sid, input).await
        };
        let (handle, events) = match resume {
            Ok(result) => result,
            Err(error) => {
                let _ = self.mark_learning_status(session_id, "failed");
                return Err(error.into());
            }
        };
        let stored_executor_sid = handle
            .executor_sid
            .as_deref()
            .or(fresh_executor_sid.as_deref())
            .or_else(|| (!restart_with_fresh_claude).then_some(executor_sid));
        self.write_learn_state(
            session_id,
            &goal,
            runtime,
            Some(permission_mode),
            "running",
            stored_executor_sid,
        )?;
        self.record_learning_sources(session_id, prompt)?;
        Ok((handle, events))
    }

    /// Stop a background Learn executor while preserving its durable Methodus
    /// run and native executor recovery id for a later resume.
    pub async fn stop_learning(
        &self,
        runtime: &str,
        handle: &SessionHandle,
    ) -> Result<(), CoreError> {
        self.adapter(runtime)?.stop(handle).await?;
        self.mark_learning_status(&handle.session_id, "awaiting_input")?;
        Ok(())
    }

    /// Prepare a fresh focused Learn run for the selected runtime's native TUI.
    /// The runtime owns all multi-turn interaction; Methodus owns only the durable
    /// run record and candidate import after the runtime exits.
    pub fn prepare_native_learning(
        &self,
        runtime: Option<&str>,
        permission_mode: &str,
        goal: &str,
    ) -> Result<NativeLearnHandoff, CoreError> {
        let runtime = self.preferred_runtime(runtime);
        let run_id = format!("learn_{}", Uuid::new_v4());
        self.prepare_native_learning_turn(&runtime, permission_mode, &run_id, goal, goal, None, &[])
    }

    /// Prepare another native TUI turn for an existing Learn run. Claude resumes
    /// the durable UUID it was given; other runtimes start a fresh native chat with
    /// the same Methodus run context when their session ID is not available.
    pub fn continue_native_learning(
        &self,
        runtime: &str,
        permission_mode: &str,
        run_id: &str,
        executor_sid: Option<&str>,
        follow_up: &str,
    ) -> Result<NativeLearnHandoff, CoreError> {
        self.continue_native_learning_with_sources(
            runtime,
            permission_mode,
            run_id,
            executor_sid,
            follow_up,
            &[],
        )
    }

    /// Continue a native Learn turn with explicit source roots authorized by the Goal.
    pub fn continue_native_learning_with_sources(
        &self,
        runtime: &str,
        permission_mode: &str,
        run_id: &str,
        executor_sid: Option<&str>,
        follow_up: &str,
        sources: &[String],
    ) -> Result<NativeLearnHandoff, CoreError> {
        let goal = self
            .list_learning_runs()?
            .into_iter()
            .find(|run| run.run_id == run_id)
            .map(|run| run.goal)
            .unwrap_or_else(|| follow_up.into());
        let resume_sid = (runtime == "claude-code")
            .then_some(executor_sid)
            .flatten()
            .filter(|sid| Uuid::parse_str(sid).is_ok());
        self.prepare_native_learning_turn(
            runtime,
            permission_mode,
            run_id,
            &goal,
            follow_up,
            resume_sid,
            sources,
        )
    }

    /// Import an explicitly returned native Learn synthesis, if the runtime wrote
    /// one. An ordinary exit without that file remains an unfinished Learn run.
    pub fn complete_native_learning(
        &self,
        handoff: &NativeLearnHandoff,
        exit_status: &str,
    ) -> Result<NativeLearnReturn, CoreError> {
        self.record_learning_event(
            &handoff.run_id,
            "methodus",
            &format!("{} native TUI returned: {exit_status}", handoff.runtime),
        )?;
        if let Ok(output) = fs::read_to_string(&handoff.output_path) {
            if !output.trim().is_empty() {
                self.record_learning_event(&handoff.run_id, "assistant", output.trim())?;
                if extract_candidate_set(&output).is_some() {
                    let candidate_ids = self.record_learning_output_for_run(
                        &handoff.run_id,
                        &handoff.goal,
                        &output,
                    )?;
                    return Ok(NativeLearnReturn {
                        candidate_ids,
                        output_recorded: true,
                        attention: None,
                        import_error: None,
                    });
                }
                if exit_status != "exit 0" && learning::parse_envelope(&output).is_none() {
                    let message = format!(
                        "native Learn runtime returned {exit_status} without a structured return"
                    );
                    self.record_unstructured_learning_artifact(
                        &handoff.run_id,
                        &handoff.goal,
                        &output,
                    )?;
                    self.mark_learning_status(&handoff.run_id, "failed")?;
                    return Ok(NativeLearnReturn {
                        candidate_ids: Vec::new(),
                        output_recorded: true,
                        attention: None,
                        import_error: Some(message),
                    });
                }
                self.record_unstructured_learning_artifact(
                    &handoff.run_id,
                    &handoff.goal,
                    &output,
                )?;
                let attention =
                    self.open_learning_follow_up_attention(&handoff.run_id, Some(&output))?;
                return Ok(NativeLearnReturn {
                    candidate_ids: Vec::new(),
                    output_recorded: true,
                    attention: Some(attention),
                    import_error: None,
                });
            }
        }
        if exit_status == "exit 0" {
            self.record_unstructured_learning_artifact(&handoff.run_id, &handoff.goal, "")?;
            let attention = self.open_learning_follow_up_attention(&handoff.run_id, None)?;
            return Ok(NativeLearnReturn {
                candidate_ids: Vec::new(),
                output_recorded: false,
                attention: Some(attention),
                import_error: None,
            });
        }
        self.mark_learning_status(&handoff.run_id, "failed")?;
        Ok(NativeLearnReturn {
            candidate_ids: Vec::new(),
            output_recorded: false,
            attention: None,
            import_error: Some(format!("native Learn runtime returned {exit_status}")),
        })
    }

    /// Recover a final native return that was written before Methodus could import
    /// it (for example, after a parser upgrade or interrupted TUI restoration).
    /// Only output files paired with a Methodus-generated brief are considered;
    /// arbitrary files under a run directory are never imported.
    pub fn recover_pending_native_learning(&self) -> Result<Vec<(String, Vec<String>)>, CoreError> {
        let mut recovered = Vec::new();
        for run in self.list_learning_runs()? {
            if run.status == "awaiting_review"
                || self
                    .home
                    .join("runs")
                    .join(&run.run_id)
                    .join("assistant.md")
                    .is_file()
            {
                continue;
            }
            let root = self.home.join("runs").join(&run.run_id);
            let Ok(entries) = fs::read_dir(&root) else {
                continue;
            };
            let mut outputs = entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let turn = name.strip_prefix("brief-")?.strip_suffix(".md")?;
                    let output = root.join(format!("native-output-{turn}.md"));
                    output.is_file().then_some(output)
                })
                .collect::<Vec<_>>();
            outputs.sort_by_key(|path| fs::metadata(path).and_then(|meta| meta.modified()).ok());
            let Some(output_path) = outputs.pop() else {
                continue;
            };
            let Ok(output) = fs::read_to_string(output_path) else {
                continue;
            };
            if extract_candidate_set(&output).is_some() {
                let ids = self.record_learning_output_for_run(&run.run_id, &run.goal, &output)?;
                self.record_learning_event(
                    &run.run_id,
                    "methodus",
                    "Recovered a native Learn return artifact for Review.",
                )?;
                recovered.push((run.run_id, ids));
            } else {
                // A native return can be a follow-up question as well as a
                // final synthesis. Materialize it before opening attention so
                // restarting Methodus does not lose the conversation state.
                self.record_unstructured_learning_artifact(&run.run_id, &run.goal, &output)?;
                let attention =
                    self.open_learning_follow_up_attention(&run.run_id, Some(&output))?;
                self.record_learning_event(
                    &run.run_id,
                    "methodus",
                    &format!("Recovered a native Learn hand-off: {}", attention.title),
                )?;
            }
        }
        Ok(recovered)
    }

    fn prepare_native_learning_turn(
        &self,
        runtime: &str,
        permission_mode: &str,
        run_id: &str,
        goal: &str,
        maintainer_message: &str,
        resume_sid: Option<&str>,
        sources: &[String],
    ) -> Result<NativeLearnHandoff, CoreError> {
        if !matches!(runtime, "claude-code" | "codex" | "cursor") {
            return Err(CoreError::UnknownRuntime(runtime.into()));
        }
        let (permission_mode, sandbox) = permission_profile(permission_mode);
        let mention_roots = self
            .context_roots()
            .into_iter()
            .map(|(_, path)| path)
            .collect::<Vec<_>>();
        let (message_with_mentions, mentioned_dirs) =
            crate::mentions::prepare_prompt(maintainer_message, &mention_roots);
        let mut extra_dirs = mentioned_dirs;
        extra_dirs.extend(source_directories(&self.launch_cwd, maintainer_message));
        extra_dirs.extend(authorized_source_directories(&self.launch_cwd, sources));
        extra_dirs.sort();
        extra_dirs.dedup();

        let executor_sid = if runtime == "claude-code" {
            resume_sid
                .map(str::to_owned)
                .or_else(|| Some(Uuid::new_v4().to_string()))
        } else {
            None
        };
        self.write_learn_state(
            run_id,
            goal,
            runtime,
            Some(permission_mode),
            "running",
            executor_sid.as_deref(),
        )?;
        self.record_learning_sources(run_id, maintainer_message)?;
        self.record_learning_authorized_sources(run_id, sources)?;
        self.record_learning_event(run_id, "user", maintainer_message)?;

        let run_root = self.home.join("runs").join(run_id);
        fs::create_dir_all(&run_root)?;
        let workspace = self.runtime_workspace("learn", run_id)?;
        let environment = self.prepare_learn_environment(&workspace)?;
        extra_dirs.extend(environment.graph_dirs.clone());
        // The output lives outside the runtime workspace. Give native runtimes
        // explicit access to this one run directory, not the whole Methodus home.
        extra_dirs.push(run_root.clone());
        extra_dirs.sort();
        extra_dirs.dedup();
        let turn_id = Uuid::new_v4();
        let output_path = run_root.join(format!("native-output-{turn_id}.md"));
        let protocol = fs::read_to_string(self.home.join("protocols/deliberate-learning.md"))
            .unwrap_or_else(|_| "Clarify the goal, inspect evidence, challenge assumptions, verify counterexamples, and propose candidates for review.".into());
        let source_roots = extra_dirs
            .iter()
            .map(|path| format!("- {}", path.display()))
            .collect::<Vec<_>>()
            .join("\n");
        let brief = format!(
            "You are the Methodus deliberate-learning runtime in a native interactive terminal. You own this multi-turn conversation: ask the maintainer focused questions, inspect the supplied evidence, challenge assumptions, seek counterexamples, and do not end merely because the first turn is complete.\n\nRuntime workspace:\n{}\n\nRead this file first:\n{}\n\nLearning goal:\n{goal}\n\nCurrent maintainer message:\n{message_with_mentions}\n\nFollow this protocol:\n{protocol}\n\nMandatory graph integration contract:\n{graph_contract}\n\nAvailable source roots:\n{source_roots}\n\nKeep facts, inferences, contradictions, and unknowns distinct. Do not make canonical Methodus graph writes or change project source files for this learning task. When the maintainer explicitly asks to finalize, write the complete synthesis to this exact file using shell tools:\n{output}\n\nIf approval is requested, approve only this return-artifact write. The return file must contain a fenced `json` CandidateSet that includes `graph_review`, and every candidate must include `disposition`, `target`, and `patch` according to the graph integration contract. Tell the maintainer after writing it; they can then exit this runtime to return to Methodus for review.",
            workspace.display(),
            environment.manifest_path.display(),
            graph_contract = LEARN_GRAPH_INTEGRATION_CONTRACT,
            output = output_path.display(),
        );
        fs::write(run_root.join(format!("brief-{turn_id}.md")), &brief)?;
        let (program, args) = native_learn_command(
            runtime,
            permission_mode,
            sandbox,
            &workspace,
            &extra_dirs,
            executor_sid.as_deref(),
            resume_sid.is_some(),
            &brief,
        )?;
        Ok(NativeLearnHandoff {
            run_id: run_id.into(),
            goal: goal.into(),
            runtime: runtime.into(),
            cwd: workspace,
            program,
            args,
            executor_sid,
            output_path,
        })
    }

    /// Write a Learn transcript and a review-only CandidateSet. The candidates
    /// are deliberately separate from canonical Personal/Team content.
    pub fn record_learning_output(
        &self,
        goal: &str,
        output: &str,
    ) -> Result<Vec<String>, CoreError> {
        let run_id = format!("learn_{}", Uuid::new_v4());
        self.record_learning_output_for_run(&run_id, goal, output)
    }

    pub fn record_learning_output_for_run(
        &self,
        run_id: &str,
        goal: &str,
        output: &str,
    ) -> Result<Vec<String>, CoreError> {
        let run_root = self.home.join("runs").join(&run_id);
        fs::create_dir_all(&run_root)?;
        let runtime = self
            .list_learning_runs()?
            .into_iter()
            .find(|run| run.run_id == run_id)
            .map(|run| run.runtime)
            .unwrap_or_else(|| "unknown".into());
        let existing = self
            .list_learning_runs()?
            .into_iter()
            .find(|run| run.run_id == run_id);
        fs::write(run_root.join("assistant.md"), output.trim())?;
        self.record_learning_sources(run_id, goal)?;
        if let Some(goal_id) = self.store.goal_run(run_id)?.map(|link| link.goal_id) {
            if let Some(goal) = self.store.learning_goal(&goal_id)? {
                self.record_learning_authorized_sources(run_id, &goal.sources)?;
            }
        }

        // Never turn an arbitrary runtime paragraph into an empty Review item.
        // The return contract must be structurally present before the run can
        // leave its resumable state.
        let set = extract_candidate_set(output).ok_or_else(|| {
            CoreError::Other("runtime did not return a structured CandidateSet".into())
        })?;

        let slug = slug_for_learning(goal);
        let suffix = run_id
            .strip_prefix("learn_")
            .unwrap_or(&run_id)
            .chars()
            .take(8)
            .collect::<String>();
        let revision = Uuid::new_v4()
            .simple()
            .to_string()
            .chars()
            .take(8)
            .collect::<String>();
        let candidates_root = self.home.join("personal/candidates");
        fs::create_dir_all(&candidates_root)?;
        let runtime_skills = set.runtime_skills.clone();
        let graph_review = set.graph_review.clone();
        let drafts = set.candidates;
        let candidate_ids = drafts
            .iter()
            .enumerate()
            .map(|(index, draft)| {
                let node_type = match draft.node_type.to_ascii_lowercase().as_str() {
                    "method" => "method",
                    "experience" => "experience",
                    _ => "knowledge",
                };
                (
                    node_type.to_string(),
                    format!("{node_type}/candidate-{slug}-{suffix}-{revision}-{index}"),
                )
            })
            .collect::<Vec<_>>();
        let (links, unresolved_relations) =
            candidate_links(&set.relations, &drafts, &candidate_ids);
        let mut unresolved_questions = set.unresolved_questions.clone();
        unresolved_questions.extend(unresolved_relations);
        let review_notes = format!(
            "### Existing graph review\n\n{}\n\n### Unresolved questions\n\n{}\n\n### Contradictions\n\n{}",
            render_graph_review(graph_review.as_ref()),
            if unresolved_questions.is_empty() {
                "none".into()
            } else {
                unresolved_questions
                    .iter()
                    .map(|item| format!("- {item}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            },
            if set.contradictions.is_empty() {
                "none".into()
            } else {
                set.contradictions
                    .iter()
                    .map(|item| format!("- {}", json_value_text(item)))
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        );
        let state = LearnRunState {
            run_id: run_id.into(),
            kind: "learn".into(),
            status: if candidate_ids.is_empty() {
                "closed".into()
            } else {
                "awaiting_review".into()
            },
            goal: goal.trim().into(),
            runtime,
            permission_mode: existing
                .as_ref()
                .map(|run| run.permission_mode.clone())
                .unwrap_or_else(default_permission_mode),
            executor_sid: existing.and_then(|run| run.executor_sid),
            unresolved_questions,
            contradictions: set.contradictions.iter().map(json_value_text).collect(),
            updated_at: Utc::now().to_rfc3339(),
        };
        fs::write(
            run_root.join("state.yaml"),
            serde_yaml::to_string(&state)
                .map_err(|error| CoreError::Other(format!("serialize Learn state: {error}")))?,
        )?;
        let mut ids = Vec::new();
        for (index, draft) in drafts.into_iter().enumerate() {
            let node_type = match draft.node_type.to_ascii_lowercase().as_str() {
                "method" => "method",
                "experience" => "experience",
                _ => "knowledge",
            };
            let kind = yaml_quote(
                draft
                    .kind
                    .as_deref()
                    .unwrap_or(if node_type == "experience" {
                        "case"
                    } else if node_type == "method" {
                        "workflow"
                    } else {
                        "procedure"
                    }),
            );
            let title = yaml_quote(if draft.title.trim().is_empty() {
                goal
            } else {
                &draft.title
            });
            let summary = yaml_quote(
                draft
                    .summary
                    .as_deref()
                    .unwrap_or("Learning runtime proposal awaiting maintainer review."),
            );
            let id = candidate_ids[index].1.clone();
            let tags = if draft.tags.is_empty() {
                "[]".into()
            } else {
                format!(
                    "[{}]",
                    draft
                        .tags
                        .iter()
                        .map(|tag| format!("\"{}\"", yaml_quote(tag)))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let evidence = draft
                .evidence
                .as_deref()
                .unwrap_or(
                    "Evidence is recorded in the Learn run and must be checked during Review.",
                )
                .to_string();
            let evidence = if node_type == "experience" && !runtime_skills.is_empty() {
                let observations = runtime_skills
                    .iter()
                    .map(|skill| {
                        format!(
                            "- {} · runtime: {} · outcome: {} · {}",
                            skill.name,
                            skill.runtime.as_deref().unwrap_or("unknown"),
                            skill.outcome.as_deref().unwrap_or("observed"),
                            skill.reason.as_deref().unwrap_or("no reason recorded")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("{evidence}\n\n### Runtime Skills observed\n\n{observations}")
            } else {
                evidence
            };
            let experience_meta = if node_type == "experience" {
                let mut meta = String::new();
                if let Some(outcome) = draft
                    .outcome
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                {
                    meta.push_str(&format!("outcome: \"{}\"\n", yaml_quote(outcome)));
                }
                if let Some(occurred_at) = draft
                    .occurred_at
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                {
                    meta.push_str(&format!("occurred_at: \"{}\"\n", yaml_quote(occurred_at)));
                }
                meta
            } else {
                String::new()
            };
            let disposition = draft
                .disposition
                .as_deref()
                .unwrap_or("new")
                .trim();
            let target = draft
                .target
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("none")
                .trim();
            let patch = draft
                .patch
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("No patch proposed; Review whether this is genuinely new.");
            let body = format!(
                "---\nid: {id}\ntitle: \"{title}\"\nnode_type: {node_type}\nkind: {kind}\nstatus: candidate\nvisibility: personal\nsummary: \"{summary}\"\ntags: {tags}\n{experience_meta}sources:\n  - path: runs/{run_id}/assistant.md\n    type: learn-run\n  - path: runs/{run_id}/sources.yaml\n    type: learn-source-manifest\n{links}---\n\n## Integration proposal\n\n- disposition: `{disposition}`\n- target: `{target}`\n\n### Proposed patch\n\n{patch}\n\n## Learn\n\n{learn}\n\n## Decide\n\n{decide}\n\n## Execute\n\n{execute}\n\n## Evidence\n\n{evidence}\n\n## Review notes\n\n{review_notes}\n\n- Learn run: runs/{run_id}/assistant.md\n- Source manifest: runs/{run_id}/sources.yaml\n- Goal: {goal}\n",
                links = links
                    .get(&id)
                    .map(|value| format!("links:\n{value}"))
                    .unwrap_or_else(|| "links: {}\n".into()),
                disposition = disposition,
                target = target,
                patch = patch,
                learn = draft.learn.as_deref().unwrap_or(output),
                decide = draft
                    .decide
                    .as_deref()
                    .unwrap_or("Review applicability, alternatives, boundaries, and contradictions before promotion."),
                execute = draft
                    .execute
                    .as_deref()
                    .unwrap_or("Rewrite this section into a compact, safe rule before exposing it to an Agent runtime."),
                evidence = evidence,
                review_notes = review_notes,
                goal = yaml_quote(goal),
            );
            fs::write(
                candidates_root.join(format!("{node_type}-{slug}-{suffix}-{revision}-{index}.md")),
                body,
            )?;
            ids.push(id);
        }
        self.sync_graph()?;
        Ok(ids)
    }

    pub fn promote_graph_candidate(&self, node_id: &str) -> Result<(), CoreError> {
        let node = self
            .store
            .graph_node(node_id)?
            .ok_or_else(|| CoreError::Other(format!("graph node not found: {node_id}")))?;
        if node.status.as_deref() != Some("candidate") {
            return Err(CoreError::Other(format!("{node_id} is not a candidate")));
        }
        let path = self.node_path(&node)?;
        self.ensure_reviewable_path(&node.path)?;
        let raw = fs::read_to_string(&path)?;
        let updated = replace_frontmatter_value(&raw, "status", "candidate", "committed")
            .ok_or_else(|| CoreError::Other(format!("{node_id} has no candidate status")))?;
        let root = if node.visibility == "team" {
            self.team_root()
        } else {
            self.home.join("personal")
        };
        let dir = root.join(match node.node_type.as_str() {
            "knowledge" => "knowledge",
            "method" => "methods",
            "experience" => "experiences",
            other => {
                return Err(CoreError::Other(format!(
                    "unsupported candidate type: {other}"
                )))
            }
        });
        fs::create_dir_all(&dir)?;
        let target = dir.join(
            path.file_name()
                .ok_or_else(|| CoreError::Other("candidate has no filename".into()))?,
        );
        if target != path && target.exists() {
            return Err(CoreError::Other(format!(
                "canonical target already exists: {} (use merge instead)",
                target.display()
            )));
        }
        fs::write(&target, updated)?;
        if path != target {
            fs::remove_file(path)?;
        }
        self.record_review_action(node_id, "commit", "candidate approved in Methodus Review")?;
        self.sync_graph()?;
        Ok(())
    }

    pub fn reject_graph_candidate(&self, node_id: &str) -> Result<(), CoreError> {
        let node = self
            .store
            .graph_node(node_id)?
            .ok_or_else(|| CoreError::Other(format!("graph node not found: {node_id}")))?;
        if node.status.as_deref() != Some("candidate") {
            return Err(CoreError::Other(format!("{node_id} is not a candidate")));
        }
        let path = self.node_path(&node)?;
        fs::remove_file(path)?;
        self.record_review_action(
            node_id,
            "reject",
            "candidate rejected and deleted in Methodus Review",
        )?;
        self.sync_graph()?;
        Ok(())
    }

    /// Permanently remove a reviewed node from managed storage and its graph projection.
    pub fn delete_graph_node(&self, node_id: &str, rationale: &str) -> Result<(), CoreError> {
        let node = self
            .store
            .graph_node(node_id)?
            .ok_or_else(|| CoreError::Other(format!("graph node not found: {node_id}")))?;
        let status = node.status.as_deref().unwrap_or("committed");
        if !matches!(status, "committed" | "stale" | "deprecated" | "rejected") {
            return Err(CoreError::Other(format!(
                "{node_id} cannot be deleted from status {status}; reject a candidate instead"
            )));
        }
        if !(node.path.starts_with("personal/") || node.path.starts_with("teams/")) {
            return Err(CoreError::Other(format!(
                "{node_id} is not a managed Personal or Team node"
            )));
        }
        let path = self.node_path(&node)?;
        fs::remove_file(path)?;
        self.record_review_action(node_id, "delete", rationale)?;
        self.sync_graph()?;
        Ok(())
    }

    /// Revalidate a stale node against its recorded local source fingerprints.
    /// Revalidation never changes the body or source declaration.
    pub fn revalidate_graph_node(&self, node_id: &str, rationale: &str) -> Result<(), CoreError> {
        let node = self
            .store
            .graph_node(node_id)?
            .ok_or_else(|| CoreError::Other(format!("graph node not found: {node_id}")))?;
        if node.status.as_deref() != Some("stale") {
            return Err(CoreError::Other(format!("{node_id} is not stale")));
        }
        let path = self.node_path(&node)?;
        let document = crate::graph::read_graph_document(&self.home, &path)?;
        if crate::graph::sources_are_stale_now(&self.home, &document.sources) {
            return Err(CoreError::Other(format!(
                "{node_id} still has changed or missing evidence"
            )));
        }
        let raw = fs::read_to_string(&path)?;
        // A stale row is derived at sync time. If the authored file already
        // says committed, revalidation only records the maintainer decision.
        if document.node.status.as_deref() == Some("stale") {
            let updated = replace_frontmatter_value(&raw, "status", "stale", "committed")
                .ok_or_else(|| {
                    CoreError::Other(format!("{node_id} has no editable status frontmatter"))
                })?;
            fs::write(path, updated)?;
        }
        self.record_review_action(node_id, "revalidate", rationale)?;
        self.sync_graph()?;
        Ok(())
    }

    pub fn promote_candidate_to_team(&self, node_id: &str) -> Result<(), CoreError> {
        let node = self
            .store
            .graph_node(node_id)?
            .ok_or_else(|| CoreError::Other(format!("graph node not found: {node_id}")))?;
        if node.status.as_deref() != Some("candidate") {
            return Err(CoreError::Other(format!("{node_id} is not a candidate")));
        }
        let path = self.node_path(&node)?;
        self.ensure_reviewable_path(&node.path)?;
        let raw = fs::read_to_string(&path)?;
        let updated = if raw
            .lines()
            .any(|line| line.trim_start().starts_with("visibility:"))
        {
            replace_frontmatter_value(&raw, "visibility", "personal", "team").unwrap_or(raw)
        } else {
            raw.replacen("---\n", "---\nvisibility: team\n", 1)
        };
        let dir = self.team_root().join(match node.node_type.as_str() {
            "knowledge" => "knowledge",
            "method" => "methods",
            "experience" => "experiences",
            other => {
                return Err(CoreError::Other(format!(
                    "unsupported candidate type: {other}"
                )))
            }
        });
        fs::create_dir_all(&dir)?;
        let target = dir.join(
            path.file_name()
                .ok_or_else(|| CoreError::Other("candidate has no filename".into()))?,
        );
        if target != path && target.exists() {
            return Err(CoreError::Other(format!(
                "Team target already exists: {}",
                target.display()
            )));
        }
        fs::write(&target, updated)?;
        if path != target {
            fs::remove_file(path)?;
        }
        self.record_review_action(node_id, "mark_team", "candidate marked for Team visibility")?;
        self.sync_graph()?;
        Ok(())
    }

    pub fn merge_graph_candidate(
        &self,
        candidate_id: &str,
        target_id: &str,
    ) -> Result<(), CoreError> {
        let facets = MERGEABLE_FACETS
            .iter()
            .map(|facet| (*facet).to_string())
            .collect::<Vec<_>>();
        self.merge_graph_candidate_facets(candidate_id, target_id, &facets)
    }

    /// Apply explicitly accepted candidate facets to a committed Knowledge node.
    ///
    /// Review is intentionally facet-level: frontmatter and unselected target
    /// sections remain unchanged, while the candidate file is removed only after
    /// the selected sections have been written. The old all-facet merge entry
    /// point delegates here so callers cannot accidentally append a second copy
    /// of the candidate document to canonical Markdown.
    pub fn merge_graph_candidate_facets(
        &self,
        candidate_id: &str,
        target_id: &str,
        accepted_facets: &[String],
    ) -> Result<(), CoreError> {
        let candidate = self
            .store
            .graph_node(candidate_id)?
            .ok_or_else(|| CoreError::Other(format!("graph node not found: {candidate_id}")))?;
        let target = self
            .store
            .graph_node(target_id)?
            .ok_or_else(|| CoreError::Other(format!("graph node not found: {target_id}")))?;
        if candidate.status.as_deref() != Some("candidate")
            || candidate.node_type != "knowledge"
            || target.node_type != "knowledge"
            || target.status.as_deref() != Some("committed")
        {
            return Err(CoreError::Other(
                "merge requires a candidate Knowledge and a committed Knowledge target".into(),
            ));
        }
        let requested = accepted_facets
            .iter()
            .filter_map(|facet| {
                MERGEABLE_FACETS
                    .iter()
                    .find(|known| known.eq_ignore_ascii_case(facet.trim()))
                    .copied()
            })
            .collect::<Vec<_>>();
        if requested.is_empty() {
            return Err(CoreError::Other(
                "merge requires at least one accepted facet (Learn, Decide, Execute, or Evidence)"
                    .into(),
            ));
        }
        let candidate_path = self.node_path(&candidate)?;
        let target_path = self.node_path(&target)?;
        self.ensure_reviewable_path(&candidate.path)?;
        self.ensure_reviewable_path(&target.path)?;
        let candidate_doc = crate::graph::read_graph_document(&self.home, &candidate_path)?;
        let target_raw = fs::read_to_string(&target_path)?;
        let replacements = requested
            .iter()
            .filter_map(|facet| {
                crate::graph::facet(&candidate_doc.body, facet)
                    .map(|body| (*facet, body))
            })
            .collect::<Vec<_>>();
        if replacements.is_empty() {
            return Err(CoreError::Other(format!(
                "candidate {candidate_id} has no content in the accepted facets"
            )));
        }
        let updated = replace_markdown_facets(&target_raw, &replacements)?;
        fs::write(&target_path, updated)?;
        fs::remove_file(candidate_path)?;
        let applied = replacements
            .iter()
            .map(|(facet, _)| *facet)
            .collect::<Vec<_>>()
            .join(", ");
        self.record_review_action(
            candidate_id,
            "merge",
            &format!("merged facets [{applied}] into {target_id}"),
        )?;
        self.sync_graph()?;
        Ok(())
    }

    fn record_review_action(
        &self,
        node_id: &str,
        action: &str,
        rationale: &str,
    ) -> Result<(), CoreError> {
        let path = self.home.join("runs/reviews.jsonl");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let record = serde_json::json!({
            "at": Utc::now().to_rfc3339(),
            "node_id": node_id,
            "action": action,
            "rationale": rationale.trim(),
        });
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&record).unwrap_or_else(|_| "{}".into())
        )?;
        Ok(())
    }

    fn ensure_reviewable_path(&self, relative_path: &str) -> Result<(), CoreError> {
        let errors = crate::graph::validate_graph(&self.home)?
            .into_iter()
            .filter(|issue| {
                issue.path == relative_path && issue.severity == crate::graph::IssueSeverity::Error
            })
            .map(|issue| issue.message)
            .collect::<Vec<_>>();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(CoreError::Other(format!(
                "Review blocked: {}",
                errors.join("; ")
            )))
        }
    }

    fn node_path(&self, node: &GraphNode) -> Result<PathBuf, CoreError> {
        let relative = Path::new(&node.path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| component == std::path::Component::ParentDir)
        {
            return Err(CoreError::Other(format!(
                "unsafe graph path: {}",
                node.path
            )));
        }
        Ok(self.home.join(relative))
    }
}

fn yaml_quote(value: &str) -> String {
    value
        .replace('"', "'")
        .replace('\n', " ")
        .trim()
        .to_string()
}

fn source_token(token: &str, cwd: &Path) -> Option<String> {
    let token = token
        .strip_prefix('@')?
        .trim_matches(|ch: char| ",.;:!?()[]{}<>`\"'".contains(ch));
    if token.is_empty() {
        return None;
    }
    let path_like = token.starts_with('/')
        || token.starts_with("~/")
        || token == "."
        || token == ".."
        || token.contains('/')
        || token.contains('.');
    let path = if token == "~" || token.starts_with("~/") {
        std::env::var_os("HOME").map(PathBuf::from).map(|home| {
            if token == "~" {
                home
            } else {
                home.join(token.trim_start_matches("~/"))
            }
        })
    } else {
        Some(
            Path::new(token)
                .is_absolute()
                .then(|| PathBuf::from(token))
                .unwrap_or_else(|| cwd.join(token)),
        )
    };
    (path_like || path.is_some_and(|path| path.exists())).then(|| token.to_string())
}

fn resolve_source_path(cwd: &Path, token: &str) -> (PathBuf, String) {
    if token == "~" || token.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            return (home.join(token.trim_start_matches("~/")), token.to_string());
        }
    }
    let path = Path::new(token);
    if path.is_absolute() {
        (path.to_path_buf(), token.to_string())
    } else {
        (cwd.join(path), token.to_string())
    }
}

fn source_directories(cwd: &Path, text: &str) -> Vec<PathBuf> {
    text.split_whitespace()
        .filter_map(|token| {
            if token.starts_with('@') {
                return source_token(token, cwd).and_then(|token| {
                    let (path, _) = resolve_source_path(cwd, &token);
                    source_directory_path(path)
                });
            }
            let token = token.trim_matches(|ch: char| ",.;:!?()[]{}<>`\"'".contains(ch));
            if token.is_empty() || token.contains("://") {
                return None;
            }
            let path_like = token.starts_with('/')
                || token.starts_with("~/")
                || token == "."
                || token == ".."
                || token.starts_with("./")
                || token.starts_with("../")
                || token.contains('/');
            if !path_like {
                return None;
            }
            let (path, _) = resolve_source_path(cwd, token);
            source_directory_path(path)
        })
        .filter(|path| path.is_dir())
        .collect()
}

fn source_directory_path(path: PathBuf) -> Option<PathBuf> {
    if path.is_dir() {
        Some(path)
    } else if path.is_file() {
        path.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

fn authorized_source_directories(cwd: &Path, sources: &[String]) -> Vec<PathBuf> {
    sources
        .iter()
        .filter_map(|source| {
            let source = source.trim();
            if source.is_empty() || source.contains("://") {
                return None;
            }
            let (path, _) = resolve_source_path(cwd, source);
            let path = path.canonicalize().ok()?;
            Some(if path.is_dir() {
                path
            } else {
                path.parent()?.to_path_buf()
            })
        })
        .filter(|path| path.is_dir())
        .collect()
}

fn json_value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| format!("{key}: {}", json_value_text(value)))
            .collect::<Vec<_>>()
            .join("; "),
        other => other.to_string(),
    }
}

fn render_graph_review(review: Option<&GraphReview>) -> String {
    let Some(review) = review else {
        return "- graph review: not reported by the runtime; verify retrieval manually during Review.".into();
    };
    let mut lines = vec![format!(
        "- searched: {}",
        if review.searched { "yes" } else { "no" }
    )];
    if review.relevant_nodes.is_empty() {
        lines.push(format!(
            "- relevant nodes: none{}",
            review
                .no_match_reason
                .as_deref()
                .map(|reason| format!(" ({})", single_line(reason)))
                .unwrap_or_default()
        ));
    } else {
        lines.push("- relevant nodes:".into());
        for node in &review.relevant_nodes {
            lines.push(format!(
                "  - {}{}",
                node.id,
                node.reason
                    .as_deref()
                    .map(|reason| format!(": {}", single_line(reason)))
                    .unwrap_or_default()
            ));
        }
    }
    lines.join("\n")
}

fn single_line(value: &str) -> String {
    value
        .replace(['\n', '\r'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn answer_is_affirmative(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    if matches!(normalized.as_str(), "n" | "no" | "nope" | "deny" | "denied" | "false")
        || ["不可以", "不能", "拒绝", "不允许", "不要"].iter().any(|word| {
            normalized.contains(word)
        })
    {
        return false;
    }
    matches!(
        normalized.as_str(),
        "y" | "yes" | "ok" | "okay" | "sure" | "approved" | "approve" | "true"
    ) || ["yes", "approve", "allow", "可以", "允许", "同意", "是"].iter().any(|word| {
        normalized.contains(word)
    })
}

fn extract_candidate_set(answer: &str) -> Option<CandidateSet> {
    let mut cursor = 0;
    while let Some(start) = answer[cursor..].find("```") {
        let start = cursor + start;
        let after_fence = &answer[start + 3..];
        let content_start = after_fence
            .find('\n')
            .map(|offset| start + 3 + offset + 1)?;
        let end = answer[content_start..]
            .find("```")
            .map(|offset| content_start + offset)?;
        let json = answer[content_start..end].trim();
        if let Some(set) = parse_candidate_set_json(json) {
            return Some(set);
        }
        cursor = end + 3;
    }
    for json in balanced_json_objects(answer) {
        if let Some(set) = parse_candidate_set_json(&json) {
            return Some(set);
        }
    }
    None
}

fn parse_candidate_set_json(json: &str) -> Option<CandidateSet> {
    let value = serde_json::from_str::<Value>(json.trim()).ok()?;
    // `CandidateSet` has defaults for forward compatibility, but an arbitrary
    // JSON object in runtime prose must not count as a successful empty Learn
    // result. The top-level candidates array is the contract discriminator.
    value.get("candidates")?.as_array()?;
    serde_json::from_value(value).ok()
}

fn balanced_json_objects(output: &str) -> Vec<String> {
    let mut objects = Vec::new();
    for (start, character) in output.char_indices() {
        if character != '{' {
            continue;
        }
        let mut depth = 0usize;
        let mut in_string = false;
        let mut escaped = false;
        for (offset, character) in output[start..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    in_string = false;
                }
                continue;
            }
            match character {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        let end = start + offset + character.len_utf8();
                        let candidate = output[start..end].to_string();
                        if serde_json::from_str::<Value>(&candidate).is_ok()
                            && !objects.iter().any(|object| object == &candidate)
                        {
                            objects.push(candidate);
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    objects
}

fn candidate_links(
    relations: &[CandidateRelation],
    drafts: &[CandidateDraft],
    ids: &[(String, String)],
) -> (BTreeMap<String, String>, Vec<String>) {
    let mut grouped = BTreeMap::<String, BTreeMap<String, Vec<String>>>::new();
    let mut unresolved = Vec::new();
    for relation in relations {
        let Some(from) = resolve_candidate_ref(&relation.from, drafts, ids) else {
            unresolved.push(format!("unresolved relation source: {}", relation.from));
            continue;
        };
        let Some(to) = resolve_candidate_ref(&relation.to, drafts, ids) else {
            unresolved.push(format!("unresolved relation target: {}", relation.to));
            continue;
        };
        let relation_name = relation.relation.trim();
        if relation_name.is_empty()
            || !relation_name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        {
            unresolved.push(format!(
                "relation between {from} and {to} has an invalid relation type"
            ));
            continue;
        }
        grouped
            .entry(from)
            .or_default()
            .entry(relation_name.to_string())
            .or_default()
            .push(to);
    }
    let links = grouped
        .into_iter()
        .map(|(from, relations)| {
            let yaml = relations
                .into_iter()
                .map(|(relation, targets)| {
                    let values = targets
                        .into_iter()
                        .map(|target| format!("    - {target}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("  {relation}:\n{values}")
                })
                .collect::<Vec<_>>()
                .join("\n");
            (from, yaml)
        })
        .collect();
    (links, unresolved)
}

fn resolve_candidate_ref(
    value: &str,
    drafts: &[CandidateDraft],
    ids: &[(String, String)],
) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(index) = value
        .strip_prefix("candidate-")
        .unwrap_or(value)
        .parse::<usize>()
    {
        return ids.get(index).map(|(_, id)| id.clone());
    }
    if value.contains('/')
        && value
            .chars()
            .all(|ch| !ch.is_whitespace() && !matches!(ch, ':' | '[' | ']' | '{' | '}'))
    {
        return Some(value.to_string());
    }
    drafts
        .iter()
        .position(|draft| draft.title.eq_ignore_ascii_case(value))
        .and_then(|index| ids.get(index).map(|(_, id)| id.clone()))
}

fn replace_frontmatter_value(raw: &str, key: &str, from: &str, to: &str) -> Option<String> {
    let rest = raw.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    let front_end = 4 + end;
    let (front, marker_and_body) = raw.split_at(front_end);
    let body = marker_and_body.strip_prefix("\n---\n")?;
    let mut replaced = false;
    let lines = front
        .lines()
        .map(|line| {
            let prefix = format!("{key}:");
            if line.trim_start().starts_with(&prefix)
                && line
                    .split_once(':')
                    .map(|(_, value)| value.trim().trim_matches('"').trim_matches('\'') == from)
                    .unwrap_or(false)
            {
                replaced = true;
                format!("{key}: {to}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    replaced.then(|| format!("{lines}\n---\n{body}"))
}

fn replace_markdown_facets(
    raw: &str,
    replacements: &[(&str, String)],
) -> Result<String, CoreError> {
    let rest = raw
        .strip_prefix("---\n")
        .ok_or_else(|| CoreError::Other("target has no editable frontmatter".into()))?;
    let end = rest
        .find("\n---\n")
        .ok_or_else(|| CoreError::Other("target has no editable frontmatter".into()))?;
    let front_end = 4 + end;
    let (front, marker_and_body) = raw.split_at(front_end);
    let body = marker_and_body
        .strip_prefix("\n---\n")
        .ok_or_else(|| CoreError::Other("target has malformed frontmatter".into()))?;

    let mut replaced = HashSet::new();
    let mut active_replacement: Option<&str> = None;
    let mut lines = Vec::new();
    for line in body.lines() {
        if let Some(heading) = line.trim_start().strip_prefix("## ") {
            active_replacement = None;
            if let Some((wanted, replacement)) = replacements.iter().find(|(wanted, _)| {
                heading
                    .trim()
                    .eq_ignore_ascii_case(wanted.trim())
            }) {
                lines.push(line.to_string());
                lines.extend(replacement.lines().map(str::to_string));
                replaced.insert(*wanted);
                active_replacement = Some(wanted);
                continue;
            }
        }
        if active_replacement.is_none() {
            lines.push(line.to_string());
        }
    }

    let mut new_body = lines.join("\n").trim().to_string();
    for (wanted, replacement) in replacements {
        if !replaced.contains(wanted) {
            if !new_body.is_empty() {
                new_body.push_str("\n\n");
            }
            new_body.push_str("## ");
            new_body.push_str(wanted);
            new_body.push('\n');
            new_body.push_str(replacement.trim());
        }
    }
    if !new_body.is_empty() {
        new_body.push('\n');
    }
    Ok(format!("{front}\n---\n\n{new_body}"))
}

fn slug_for_learning(value: &str) -> String {
    let mut slug = value
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() {
                Some(ch.to_ascii_lowercase())
            } else if ch.is_whitespace() || ch == '-' || ch == '_' {
                Some('-')
            } else {
                None
            }
        })
        .collect::<String>();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "learning-result".into()
    } else {
        slug.chars().take(48).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[test]
    fn permission_profiles_are_bounded_and_never_bypass() {
        assert_eq!(permission_profile("plan"), ("plan", "read-only"));
        assert_eq!(
            permission_profile("cautious"),
            ("cautious", "workspace-write")
        );
        assert_eq!(
            permission_profile("acceptEdits"),
            ("acceptEdits", "workspace-write")
        );
        assert_eq!(
            permission_profile("bypassPermissions"),
            ("plan", "read-only")
        );
    }

    #[test]
    fn goal_text_creation_keeps_defaults_and_persists_resolved_mentions() {
        let dir = tempdir().unwrap();
        let engine = Engine::with_runtimes(
            Arc::new(Store::open_memory().unwrap()),
            dir.path().to_path_buf(),
            HashMap::new(),
        );
        let goal = engine
            .create_goal_from_objective("Understand the runtime from @Cargo.toml")
            .unwrap();

        assert_eq!(goal.sources, vec!["Cargo.toml"]);
        assert_eq!(goal.cadence, methodus_domain::Cadence::Weekly);
        assert_eq!(goal.review_cadence, methodus_domain::Cadence::Weekly);
        assert_eq!(goal.summary_cadence, methodus_domain::Cadence::Monthly);
        assert_eq!(goal.source_check_cadence, methodus_domain::Cadence::Daily);
        assert_eq!(goal.budget_usd, 20.0);
    }

    #[test]
    fn native_claude_handoff_is_interactive_and_uses_a_uuid() {
        let sid = "11111111-2222-3333-4444-555555555555";
        let (_, args) = native_learn_command(
            "claude-code",
            "plan",
            "read-only",
            Path::new("/tmp/work"),
            &[PathBuf::from("/tmp/source")],
            Some(sid),
            false,
            "learn this",
        )
        .unwrap();
        assert!(args.contains(&"--session-id".to_string()));
        assert!(args.contains(&sid.to_string()));
        assert!(args.contains(&"--permission-mode".to_string()));
        assert!(!args.contains(&"--print".to_string()));
        assert_eq!(args.get(args.len().saturating_sub(2)).map(String::as_str), Some("--"));
        assert_eq!(args.last().map(String::as_str), Some("learn this"));
    }

    #[test]
    fn native_use_handoff_uses_learn_permissions_and_opens_graph_roots() {
        let sid = "11111111-2222-3333-4444-555555555555";
        let (_, args) = native_use_command(
            "claude-code",
            "plan",
            Path::new("/tmp/methodus-use/session"),
            &[PathBuf::from("/tmp/methodus/personal/knowledge")],
            Some(sid),
            false,
            "answer this",
        )
        .unwrap();
        assert!(args.contains(&"--session-id".to_string()));
        assert!(args.contains(&sid.to_string()));
        assert!(args.contains(&"--permission-mode".to_string()));
        assert!(args.contains(&"manual".to_string()));
        assert!(args.contains(&"--add-dir".to_string()));
        assert!(args.contains(&"/tmp/methodus/personal/knowledge".to_string()));
        assert_eq!(args.get(args.len().saturating_sub(2)).map(String::as_str), Some("--"));
        assert_eq!(args.last().map(String::as_str), Some("answer this"));
    }

    #[test]
    fn native_learn_handoff_uses_a_methodus_managed_workspace() {
        let dir = tempdir().unwrap();
        let engine = Engine::with_runtimes(
            Arc::new(Store::open_memory().unwrap()),
            dir.path().to_path_buf(),
            HashMap::new(),
        );
        let handoff = engine
            .prepare_native_learning(Some("claude-code"), "plan", "learn this")
            .unwrap();
        let expected_root = dir.path().join("workspaces/learn");
        assert!(handoff.cwd.starts_with(&expected_root));
        assert!(handoff.cwd.is_dir());
        assert!(!handoff.cwd.starts_with(engine.launch_cwd()));
        assert!(!handoff
            .args
            .contains(&engine.launch_cwd().display().to_string()));
        assert!(handoff
            .output_path
            .starts_with(dir.path().join("runs").join(&handoff.run_id)));
        assert!(handoff.cwd.join("METHODUS_LEARN.md").is_file());
    }

    #[test]
    fn learn_environment_exposes_existing_graph_to_the_runtime_contract() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("personal/knowledge")).unwrap();
        fs::write(
            dir.path().join("personal/knowledge/existing.md"),
            "---\nid: knowledge/existing\ntitle: Existing rule\nnode_type: knowledge\nkind: procedure\nstatus: committed\nvisibility: personal\nsummary: An existing rule.\n---\n\n## Execute\nUse the existing rule.\n",
        )
        .unwrap();
        let store = Arc::new(Store::open_memory().unwrap());
        crate::graph::sync_graph(&store, dir.path()).unwrap();
        let engine = Engine::with_runtimes(store, dir.path().to_path_buf(), HashMap::new());

        let handoff = engine
            .prepare_native_learning(Some("claude-code"), "plan", "Investigate this rule")
            .unwrap();
        let manifest = fs::read_to_string(handoff.cwd.join("METHODUS_LEARN.md")).unwrap();
        assert!(manifest.contains("knowledge/existing"));
        assert!(manifest.contains("Existing rule"));
        assert!(handoff
            .args
            .contains(&dir.path().join("personal/knowledge").display().to_string()));
        let brief = handoff.args.last().unwrap();
        assert!(brief.contains("METHODUS_LEARN.md"));
        assert!(brief.contains("Mandatory graph integration contract"));
    }

    #[test]
    fn use_environment_lists_consumer_nodes_but_not_candidates() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("personal/knowledge")).unwrap();
        fs::create_dir_all(dir.path().join("personal/candidates")).unwrap();
        fs::write(
            dir.path().join("personal/knowledge/signal.md"),
            "---\nid: knowledge/signal\ntitle: Signal handling\nnode_type: knowledge\nkind: diagnostic-signal\nstatus: committed\nvisibility: personal\nsummary: Handle process signals.\n---\n\n## Execute\nRead the signal log.\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("personal/candidates/draft.md"),
            "---\nid: knowledge/draft\ntitle: Draft signal\nnode_type: knowledge\nkind: diagnostic-signal\nstatus: candidate\nvisibility: personal\nsummary: Draft only.\n---\n\n## Execute\nDo not expose this.\n",
        )
        .unwrap();
        let store = Arc::new(Store::open_memory().unwrap());
        crate::graph::sync_graph(&store, dir.path()).unwrap();
        let engine = Engine::with_runtimes(store, dir.path().to_path_buf(), HashMap::new());
        let environment = engine.prepare_use_environment("use_manifest_test").unwrap();
        assert!(environment
            .cwd
            .starts_with(dir.path().join("workspaces/use")));
        let manifest = fs::read_to_string(environment.manifest_path).unwrap();
        assert!(manifest.contains("knowledge/signal"));
        assert!(manifest.contains("Signal handling"));
        assert!(manifest.contains("facets: Execute"));
        assert!(!manifest.contains("knowledge/draft"));
        assert!(manifest.contains("learning_recommended"));
        assert!(manifest.contains(environment.return_path.to_string_lossy().as_ref()));
        assert!(environment
            .graph_dirs
            .contains(&dir.path().join("personal/knowledge")));
    }

    #[test]
    fn native_use_learning_recommendation_opens_attention() {
        let dir = tempdir().unwrap();
        let store = Arc::new(Store::open_memory().unwrap());
        let engine = Engine::with_runtimes(store, dir.path().to_path_buf(), HashMap::new());
        let return_path = dir
            .path()
            .join("workspaces/use/use_test/METHODUS_USE_RETURN.json");
        fs::create_dir_all(return_path.parent().unwrap()).unwrap();
        fs::write(
            &return_path,
            r#"{"outcome":"learning_recommended","learning_task":"Investigate the shutdown sequence","context":"No committed graph evidence supports this answer."}"#,
        )
        .unwrap();
        let handoff = NativeUseHandoff {
            session_id: "use_test".into(),
            question: "Why did it stop?".into(),
            runtime: "claude-code".into(),
            cwd: return_path.parent().unwrap().to_path_buf(),
            return_path,
            program: "claude".into(),
            args: Vec::new(),
            executor_sid: None,
        };

        let attention = engine.complete_native_use(&handoff).unwrap().unwrap();

        assert_eq!(attention.run_id, "use_test");
        assert_eq!(attention.prompt, "Investigate the shutdown sequence");
        assert_eq!(engine.open_attentions().unwrap().len(), 1);
    }

    #[test]
    fn authorized_source_roots_preserve_spaces_and_ignore_urls() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source with spaces");
        fs::create_dir_all(&source).unwrap();
        let roots = authorized_source_directories(
            dir.path(),
            &[
                "source with spaces".into(),
                "https://example.test/docs".into(),
            ],
        );
        assert_eq!(roots, vec![source.canonicalize().unwrap()]);
    }

    #[test]
    fn native_learn_return_imports_a_structured_candidate_set() {
        let dir = tempdir().unwrap();
        let engine = Engine::with_runtimes(
            Arc::new(Store::open_memory().unwrap()),
            dir.path().to_path_buf(),
            HashMap::new(),
        );
        let run_id = "learn_native";
        engine
            .write_learn_state(
                run_id,
                "signal handling",
                "claude-code",
                Some("plan"),
                "running",
                Some("11111111-2222-3333-4444-555555555555"),
            )
            .unwrap();
        let output_path = dir.path().join("native-output.md");
        fs::write(&output_path, "```json\n{\"candidates\":[{\"type\":\"knowledge\",\"kind\":\"diagnostic-signal\",\"title\":\"Signal marker\",\"learn\":\"Read it.\"}]}\n```").unwrap();
        let handoff = NativeLearnHandoff {
            run_id: run_id.into(),
            goal: "signal handling".into(),
            runtime: "claude-code".into(),
            cwd: dir.path().to_path_buf(),
            program: "claude".into(),
            args: Vec::new(),
            executor_sid: Some("11111111-2222-3333-4444-555555555555".into()),
            output_path,
        };
        let returned = engine.complete_native_learning(&handoff, "exit 0").unwrap();
        assert!(returned.output_recorded);
        assert_eq!(returned.candidate_ids.len(), 1);
        let run = engine
            .list_learning_runs()
            .unwrap()
            .into_iter()
            .find(|run| run.run_id == run_id)
            .unwrap();
        assert_eq!(run.status, "awaiting_review");
    }

    #[test]
    fn native_learn_return_requeues_a_follow_up_attention() {
        let dir = tempdir().unwrap();
        let engine = Engine::with_runtimes(
            Arc::new(Store::open_memory().unwrap()),
            dir.path().to_path_buf(),
            HashMap::new(),
        );
        let run_id = "learn_follow_up";
        engine
            .write_learn_state(
                run_id,
                "investigate the source",
                "claude-code",
                Some("plan"),
                "running",
                Some("11111111-2222-3333-4444-555555555555"),
            )
            .unwrap();
        let output_path = dir.path().join("native-output.md");
        fs::write(
            &output_path,
            "The answer depends on one decision: {\"outcome\":\"needs_input\",\"question\":\"Which source is authoritative?\"}",
        )
        .unwrap();
        let handoff = NativeLearnHandoff {
            run_id: run_id.into(),
            goal: "investigate the source".into(),
            runtime: "claude-code".into(),
            cwd: dir.path().to_path_buf(),
            program: "claude".into(),
            args: Vec::new(),
            executor_sid: Some("11111111-2222-3333-4444-555555555555".into()),
            output_path,
        };

        let returned = engine.complete_native_learning(&handoff, "exit 0").unwrap();

        assert!(returned.attention.is_some());
        assert!(returned.import_error.is_none());
        assert_eq!(engine.open_attentions().unwrap().len(), 1);
        assert_eq!(
            engine
                .list_learning_runs()
                .unwrap()
                .into_iter()
                .find(|run| run.run_id == run_id)
                .unwrap()
                .status,
            "awaiting_input"
        );
    }

    #[test]
    fn repair_reconnects_old_unstructured_runs_and_backfills_authorized_sources() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("nxm");
        fs::create_dir_all(&source).unwrap();
        let store = Arc::new(Store::open_memory().unwrap());
        let engine = Engine::with_runtimes(store.clone(), dir.path().to_path_buf(), HashMap::new());
        let goal = engine
            .create_goal_from_objective("locate the fence implementation")
            .unwrap();
        let run_id = "learn_old_return";
        engine
            .write_learn_state(
                run_id,
                "locate the fence implementation",
                "claude-code",
                Some("plan"),
                "awaiting_review",
                None,
            )
            .unwrap();
        engine.link_goal_run(run_id, &goal.id, WorkKind::Learn).unwrap();
        let run_root = dir.path().join("runs").join(run_id);
        fs::write(
            run_root.join("assistant.md"),
            format!(
                "The runtime stopped. {{\"outcome\":\"needs_input\",\"question\":\"May I use {} as the sole evidence source?\"}}",
                source.display()
            ),
        )
        .unwrap();
        let original = learning::AttentionEnvelope {
            kind: AttentionKind::Question,
            question: format!("May I use {} as the sole evidence source?", source.display()),
            context: None,
            tool_name: None,
            tool_input: None,
        };
        let old_attention = learning::open_attention(
            &store,
            run_id,
            Some(goal.id.clone()),
            &original,
            Utc::now(),
        )
        .unwrap();
        engine
            .resolve_attention(&old_attention.id, "yes, you can")
            .unwrap();

        let repaired = engine.repair_learning_continuations().unwrap();

        assert_eq!(repaired, vec![run_id.to_string()]);
        assert_eq!(engine.open_attentions().unwrap().len(), 1);
        let persisted = engine.goal(&goal.id).unwrap().unwrap();
        assert_eq!(persisted.sources, vec![source.canonicalize().unwrap().display().to_string()]);
    }

    #[test]
    fn extracts_structured_candidate_set_from_runtime_response() {
        let answer = "结论如下：\n```json\n{\"candidates\":[{\"type\":\"knowledge\",\"kind\":\"diagnostic-signal\",\"title\":\"Previous shutdown reason\",\"learn\":\"Read the reason first.\",\"tags\":[\"shutdown\"]}]}\n```";
        let set = extract_candidate_set(answer).unwrap();
        assert_eq!(set.candidates.len(), 1);
        assert_eq!(set.candidates[0].node_type, "knowledge");
        assert_eq!(set.candidates[0].kind.as_deref(), Some("diagnostic-signal"));
    }

    #[test]
    fn candidate_set_accepts_runtime_relation_aliases_and_structured_contradictions() {
        let answer = "```json\n{\"candidates\":[{\"type\":\"knowledge\",\"title\":\"Signal\"}],\"relations\":[{\"from\":\"candidate-0\",\"type\":\"details\",\"to\":\"knowledge/existing\"}],\"contradictions\":[{\"claim_a\":\"old\",\"claim_b\":\"new\"}]}\n```";
        let set = extract_candidate_set(answer).unwrap();
        assert_eq!(set.relations[0].relation, "details");
        assert_eq!(
            json_value_text(&set.contradictions[0]),
            "claim_a: old; claim_b: new"
        );
    }

    #[test]
    fn empty_candidate_set_remains_a_no_publish_learning_result() {
        let set = extract_candidate_set(
            "```json\n{\"candidates\":[],\"unresolved_questions\":[\"scope\"]}\n```",
        )
        .unwrap();
        assert!(set.candidates.is_empty());
        assert_eq!(set.unresolved_questions, vec!["scope"]);
    }

    #[test]
    fn frontmatter_replacement_preserves_markdown_body() {
        let raw = "---\nstatus: candidate\ntitle: Note\n---\n\n## Learn\nBody\n";
        let updated = replace_frontmatter_value(raw, "status", "candidate", "committed").unwrap();
        assert!(updated.contains("status: committed"));
        assert!(updated.contains("## Learn\nBody"));
    }

    #[test]
    fn facet_merge_replaces_only_the_accepted_sections() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("personal/knowledge")).unwrap();
        fs::create_dir_all(dir.path().join("personal/candidates")).unwrap();
        fs::write(
            dir.path().join("personal/knowledge/existing.md"),
            "---\nid: knowledge/existing\ntitle: Existing rule\nnode_type: knowledge\nkind: procedure\nstatus: committed\nvisibility: personal\nsummary: Existing\n---\n\n## Learn\nOld explanation.\n\n## Execute\nKeep this action.\n\n## Evidence\nOriginal evidence.\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("personal/candidates/draft.md"),
            "---\nid: knowledge/draft\ntitle: Draft rule\nnode_type: knowledge\nkind: procedure\nstatus: candidate\nvisibility: personal\nsummary: Draft\n---\n\n## Learn\nNew explanation.\n\n## Execute\nCandidate action.\n",
        )
        .unwrap();
        let engine = Engine::with_runtimes(
            Arc::new(Store::open_memory().unwrap()),
            dir.path().to_path_buf(),
            HashMap::new(),
        );
        engine.sync_graph().unwrap();

        engine
            .merge_graph_candidate_facets(
                "knowledge/draft",
                "knowledge/existing",
                &["Learn".into()],
            )
            .unwrap();

        let updated = fs::read_to_string(dir.path().join("personal/knowledge/existing.md"))
            .unwrap();
        assert!(updated.contains("## Learn\nNew explanation."));
        assert!(updated.contains("## Execute\nKeep this action."));
        assert!(updated.contains("## Evidence\nOriginal evidence."));
        assert!(!dir.path().join("personal/candidates/draft.md").exists());
        let review_log = fs::read_to_string(dir.path().join("runs/reviews.jsonl")).unwrap();
        assert!(review_log.contains("merged facets [Learn] into knowledge/existing"));
    }

    #[test]
    fn learning_output_preserves_run_state_and_materializes_relations() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("personal/candidates")).unwrap();
        let engine = Engine::with_runtimes(
            Arc::new(Store::open_memory().unwrap()),
            dir.path().to_path_buf(),
            HashMap::new(),
        );
        let run_id = "learn_test";
        engine
            .write_learn_state(
                run_id,
                "shutdown diagnosis",
                "claude-code",
                Some("plan"),
                "awaiting_input",
                Some("exec-1"),
            )
            .unwrap();
        let output = "```json\n{\"graph_review\":{\"searched\":true,\"relevant_nodes\":[{\"id\":\"knowledge/existing\",\"reason\":\"same signal\"}]},\"candidates\":[{\"type\":\"knowledge\",\"kind\":\"signal\",\"title\":\"Reason\",\"summary\":\"Read reason\",\"disposition\":\"revise\",\"target\":\"knowledge/existing\",\"patch\":\"Update the Execute facet.\",\"learn\":\"Explain\"},{\"type\":\"experience\",\"kind\":\"incident\",\"title\":\"Case\",\"summary\":\"A case\",\"disposition\":\"new\",\"target\":null,\"patch\":null,\"learn\":\"Observed\"}],\"relations\":[{\"from\":\"candidate-0\",\"relation\":\"validated_by\",\"to\":\"candidate-1\"}],\"unresolved_questions\":[\"scope?\"],\"contradictions\":[\"old claim\"],\"runtime_skills\":[{\"name\":\"repo-survey\",\"runtime\":\"claude-code\",\"outcome\":\"useful\",\"reason\":\"found the source\"}]}\n```";
        let ids = engine
            .record_learning_output_for_run(run_id, "shutdown diagnosis", output)
            .unwrap();
        assert_eq!(ids.len(), 2);
        let run = engine
            .list_learning_runs()
            .unwrap()
            .into_iter()
            .find(|run| run.run_id == run_id)
            .unwrap();
        assert_eq!(run.runtime, "claude-code");
        assert_eq!(run.permission_mode, "plan");
        assert_eq!(run.executor_sid.as_deref(), Some("exec-1"));
        assert_eq!(run.status, "awaiting_review");
        let candidate_files = fs::read_dir(dir.path().join("personal/candidates"))
            .unwrap()
            .count();
        assert_eq!(candidate_files, 2);
        let state = fs::read_to_string(dir.path().join("runs/learn_test/state.yaml")).unwrap();
        assert!(state.contains("scope?"));
        let candidates = fs::read_dir(dir.path().join("personal/candidates"))
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| fs::read_to_string(entry.path()).ok())
            .collect::<Vec<_>>();
        assert!(candidates
            .iter()
            .any(|candidate| candidate.contains("validated_by")));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.contains("repo-survey")));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.contains("disposition: `revise`")));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.contains("Update the Execute facet.")));
    }

    #[test]
    fn team_promotion_moves_candidate_into_selected_team_before_commit() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("personal/candidates")).unwrap();
        fs::create_dir_all(dir.path().join("teams/default")).unwrap();
        let store = Arc::new(Store::open_memory().unwrap());
        let engine = Engine::with_runtimes(store, dir.path().to_path_buf(), HashMap::new());
        engine.record_learning_output("team rule", "```json\n{\"candidates\":[{\"type\":\"knowledge\",\"kind\":\"procedure\",\"title\":\"Team rule\",\"summary\":\"A reviewed rule\",\"learn\":\"Explain\",\"execute\":\"Do it\"}]}\n```").unwrap();
        let candidate = engine
            .list_graph_nodes(Some("Team rule"))
            .unwrap()
            .into_iter()
            .find(|node| node.status.as_deref() == Some("candidate"))
            .unwrap();
        engine.promote_candidate_to_team(&candidate.id).unwrap();
        let moved = engine
            .list_graph_nodes(Some("Team rule"))
            .unwrap()
            .into_iter()
            .find(|node| node.id == candidate.id)
            .unwrap();
        assert_eq!(moved.visibility, "team");
        assert!(moved.path.starts_with("teams/default/"));
        engine.promote_graph_candidate(&moved.id).unwrap();
        let committed = engine
            .list_graph_nodes(Some("Team rule"))
            .unwrap()
            .into_iter()
            .find(|node| node.id == candidate.id)
            .unwrap();
        assert_eq!(committed.status.as_deref(), Some("committed"));
        assert!(committed.path.starts_with("teams/default/"));
    }

    #[test]
    fn projected_stale_nodes_can_be_revalidated() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("personal/knowledge")).unwrap();
        fs::write(dir.path().join("evidence.txt"), "one").unwrap();
        let fingerprint = format!("{:x}", Sha256::digest(b"one"));
        fs::write(dir.path().join("personal/knowledge/source.md"), format!(
            "---\nid: knowledge/source\ntitle: Source\nnode_type: knowledge\nkind: signal\nstatus: committed\nvisibility: personal\nsummary: Source-backed rule\nsources:\n  - path: evidence.txt\n    fingerprint: sha256:{fingerprint}\n---\n\n## Execute\nUse it.\n"
        )).unwrap();
        let store = Arc::new(Store::open_memory().unwrap());
        let engine = Engine::with_runtimes(store, dir.path().to_path_buf(), HashMap::new());
        engine.sync_graph().unwrap();
        fs::write(dir.path().join("evidence.txt"), "two").unwrap();
        engine.sync_graph().unwrap();
        let stale = engine
            .list_graph_nodes(Some("Source"))
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(stale.status.as_deref(), Some("stale"));
        // Restore the evidence without editing the authored status. The
        // projection should return to committed after explicit revalidation.
        fs::write(dir.path().join("evidence.txt"), "one").unwrap();
        engine
            .revalidate_graph_node(&stale.id, "checked source")
            .unwrap();
        assert_eq!(
            engine
                .list_graph_nodes(Some("Source"))
                .unwrap()
                .pop()
                .unwrap()
                .status
                .as_deref(),
            Some("committed")
        );
    }

    #[test]
    fn deleting_a_canonical_node_removes_its_projection() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("personal/knowledge")).unwrap();
        fs::write(dir.path().join("personal/knowledge/remove.md"), "---\nid: knowledge/remove\ntitle: Remove me\nnode_type: knowledge\nkind: rule\nstatus: committed\nvisibility: personal\nsummary: Temporary\n---\n\n## Learn\nTemporary\n").unwrap();
        let engine = Engine::with_runtimes(
            Arc::new(Store::open_memory().unwrap()),
            dir.path().to_path_buf(),
            HashMap::new(),
        );
        engine.sync_graph().unwrap();
        engine
            .delete_graph_node("knowledge/remove", "test deletion")
            .unwrap();
        assert!(!dir.path().join("personal/knowledge/remove.md").exists());
        assert!(engine
            .list_graph_nodes(Some("Remove me"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rejecting_a_candidate_deletes_its_source_and_projection() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("personal/candidates")).unwrap();
        let engine = Engine::with_runtimes(
            Arc::new(Store::open_memory().unwrap()),
            dir.path().to_path_buf(),
            HashMap::new(),
        );
        engine.record_learning_output("Discard me", "```json\n{\"candidates\":[{\"type\":\"knowledge\",\"kind\":\"rule\",\"title\":\"Discard me\",\"summary\":\"Temporary\",\"learn\":\"No\",\"execute\":\"No\"}]}\n```").unwrap();
        let candidate = engine
            .list_graph_nodes(Some("Discard me"))
            .unwrap()
            .into_iter()
            .find(|node| node.status.as_deref() == Some("candidate"))
            .unwrap();
        let source = dir.path().join(&candidate.path);
        assert!(source.exists());
        engine.reject_graph_candidate(&candidate.id).unwrap();
        assert!(!source.exists());
        assert!(engine
            .list_graph_nodes(Some("Discard me"))
            .unwrap()
            .is_empty());
    }
}
