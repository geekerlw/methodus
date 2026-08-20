//! Methodus's active application service.
//!
//! This layer intentionally contains only the maintainer workflow: a focused
//! Learn conversation, Markdown graph indexing, review actions, and Personal →
//! Team promotion. Ordinary coding tasks, workspaces, handoff sessions, and
//! runtime Skill management are not part of the active product surface.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use methodus_domain::{GraphEdge, GraphNode, RuntimeEvent};
use methodus_runtime::{RuntimeAdapter, SessionHandle, SpawnInput};
use methodus_store::Store;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::config::UserConfig;
use crate::error::CoreError;

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

fn default_permission_mode() -> String { "plan".into() }

fn permission_profile(mode: &str) -> (&'static str, &'static str) {
    match mode {
        "cautious" => ("cautious", "workspace-write"),
        "acceptEdits" => ("acceptEdits", "workspace-write"),
        _ => ("plan", "read-only"),
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
    candidates: Vec<CandidateDraft>,
    #[serde(default)]
    relations: Vec<CandidateRelation>,
    #[serde(default)]
    unresolved_questions: Vec<String>,
    #[serde(default)]
    contradictions: Vec<String>,
    #[serde(default)]
    runtime_skills: Vec<RuntimeSkillObservation>,
}

#[derive(Debug, Deserialize, Clone)]
struct CandidateRelation {
    from: String,
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
}

fn default_node_type() -> String { "knowledge".into() }

impl Engine {
    pub fn new(store: Arc<Store>, adapter: Arc<dyn RuntimeAdapter>, home: PathBuf) -> Self {
        let mut adapters = HashMap::new();
        adapters.insert("claude-code".into(), adapter);
        Self { store, adapters, home, launch_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")) }
    }

    pub fn with_runtimes(store: Arc<Store>, home: PathBuf, adapters: HashMap<String, Arc<dyn RuntimeAdapter>>) -> Self {
        Self { store, adapters, home, launch_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")) }
    }

    fn adapter(&self, runtime: &str) -> Result<Arc<dyn RuntimeAdapter>, CoreError> {
        self.adapters.get(runtime).cloned().ok_or_else(|| CoreError::UnknownRuntime(runtime.into()))
    }

    fn preferred_runtime(&self, requested: Option<&str>) -> String {
        if let Some(runtime) = requested.filter(|runtime| self.adapters.contains_key(*runtime)) { return runtime.into(); }
        if let Some(runtime) = UserConfig::load(&self.home).default_runtime.filter(|runtime| self.adapters.contains_key(runtime)) { return runtime; }
        ["claude-code", "codex", "cursor"].into_iter().find(|runtime| self.adapters.contains_key(*runtime)).map(str::to_string)
            .or_else(|| self.adapters.keys().next().cloned()).unwrap_or_else(|| "claude-code".into())
    }

    pub fn store(&self) -> &Arc<Store> { &self.store }
    pub fn home(&self) -> &Path { &self.home }
    pub fn launch_cwd(&self) -> &Path { &self.launch_cwd }
    pub fn context_roots(&self) -> Vec<(String, PathBuf)> { crate::mentions::context_roots(&self.home, &self.launch_cwd) }

    pub fn sync_graph(&self) -> Result<usize, CoreError> { Ok(crate::graph::sync_graph(&self.store, &self.home)?) }
    pub fn list_graph_nodes(&self, query: Option<&str>) -> Result<Vec<GraphNode>, CoreError> { Ok(self.store.list_graph_nodes(query)?) }
    pub fn graph_edges_for(&self, node_id: &str) -> Result<Vec<GraphEdge>, CoreError> { Ok(self.store.graph_edges_for(node_id)?) }
    pub fn team_id(&self) -> String { UserConfig::load(&self.home).selected_team().to_string() }
    pub fn team_root(&self) -> PathBuf { self.home.join("teams").join(self.team_id()) }

    pub fn list_learning_runs(&self) -> Result<Vec<LearnRun>, CoreError> {
        let root = self.home.join("runs");
        if !root.is_dir() { return Ok(Vec::new()); }
        let mut runs = Vec::new();
        for entry in fs::read_dir(root)? {
            let path = entry?.path().join("state.yaml");
            if !path.is_file() { continue; }
            let Ok(state) = serde_yaml::from_str::<LearnRunState>(&fs::read_to_string(path)?) else { continue; };
            if state.kind != "learn" { continue; }
            runs.push(LearnRun { run_id: state.run_id, goal: state.goal, runtime: state.runtime, permission_mode: state.permission_mode, status: state.status, executor_sid: state.executor_sid, updated_at: state.updated_at });
        }
        runs.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        Ok(runs)
    }

    pub fn latest_resumable_learning(&self) -> Result<Option<LearnRun>, CoreError> {
        Ok(self.list_learning_runs()?.into_iter().find(|run| matches!(run.status.as_str(), "running" | "awaiting_input" | "failed")))
    }

    fn write_learn_state(&self, run_id: &str, goal: &str, runtime: &str, permission_mode: Option<&str>, status: &str, executor_sid: Option<&str>) -> Result<(), CoreError> {
        let root = self.home.join("runs").join(run_id);
        fs::create_dir_all(&root)?;
        let previous = fs::read_to_string(root.join("state.yaml"))
            .ok()
            .and_then(|raw| serde_yaml::from_str::<LearnRunState>(&raw).ok());
        let state = LearnRunState {
            run_id: run_id.into(), kind: "learn".into(), status: status.into(), goal: goal.into(), runtime: runtime.into(),
            permission_mode: permission_mode.map(|mode| permission_profile(mode).0.to_string()).or_else(|| previous.as_ref().map(|state| state.permission_mode.clone())).unwrap_or_else(default_permission_mode),
            executor_sid: executor_sid.map(str::to_string).or_else(|| previous.as_ref().and_then(|state| state.executor_sid.clone())),
            unresolved_questions: previous.as_ref().map(|state| state.unresolved_questions.clone()).unwrap_or_default(),
            contradictions: previous.as_ref().map(|state| state.contradictions.clone()).unwrap_or_default(),
            updated_at: Utc::now().to_rfc3339(),
        };
        fs::write(root.join("state.yaml"), serde_yaml::to_string(&state).map_err(|error| CoreError::Other(format!("serialize Learn state: {error}")))?)?;
        Ok(())
    }

    pub fn record_learning_event(&self, run_id: &str, role: &str, text: &str) -> Result<(), CoreError> {
        let root = self.home.join("runs").join(run_id);
        fs::create_dir_all(&root)?;
        let event = serde_json::json!({ "at": Utc::now().to_rfc3339(), "role": role, "text": text });
        let mut file = OpenOptions::new().create(true).append(true).open(root.join("events.jsonl"))?;
        writeln!(file, "{}", serde_json::to_string(&event).unwrap_or_else(|_| "{}".into()))?;
        Ok(())
    }

    pub fn learning_events(&self, run_id: &str) -> Result<Vec<LearnEventRecord>, CoreError> {
        let path = self.home.join("runs").join(run_id).join("events.jsonl");
        if !path.is_file() { return Ok(Vec::new()); }
        let mut events = Vec::new();
        for line in fs::read_to_string(path)?.lines() {
            if let Ok(event) = serde_json::from_str::<LearnEventRecord>(line) { events.push(event); }
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
        let mut seen = entries.iter().map(|entry| (entry.locator.clone(), ())).collect::<BTreeMap<_, _>>();
        for token in text.split_whitespace().filter_map(|token| source_token(token, &self.launch_cwd)) {
            if seen.insert(token.clone(), ()).is_some() { continue; }
            let (path, display) = resolve_source_path(&self.launch_cwd, &token);
            let (fingerprint, status) = if path.is_dir() {
                (None, "current")
            } else {
                match fs::read(&path) {
                    Ok(bytes) => (Some(format!("sha256:{:x}", Sha256::digest(bytes))), "current"),
                    Err(_) => (None, "missing"),
                }
            };
            entries.push(SourceManifestEntry { locator: display, path: path.display().to_string(), fingerprint, status: status.into() });
        }
        fs::write(root.join("sources.yaml"), serde_yaml::to_string(&SourceManifest { sources: entries }).map_err(|error| CoreError::Other(format!("serialize source manifest: {error}")))?)?;
        Ok(())
    }

    pub fn mark_learning_status(&self, run_id: &str, status: &str) -> Result<(), CoreError> {
        let Some(run) = self.list_learning_runs()?.into_iter().find(|run| run.run_id == run_id) else { return Err(CoreError::Other(format!("Learn run not found: {run_id}"))); };
        self.write_learn_state(&run.run_id, &run.goal, &run.runtime, Some(&run.permission_mode), status, run.executor_sid.as_deref())
    }

    pub fn team_status(&self) -> Result<TeamStatus, CoreError> {
        let team_id = self.team_id();
        let root = self.team_root();
        let team_prefix = format!("teams/{team_id}/");
        let validation_issues = crate::graph::validate_graph(&self.home)?.into_iter().filter(|issue| issue.path.starts_with(&team_prefix)).collect();
        if !root.is_dir() { return Ok(TeamStatus { team_id, root, is_git: false, branch: None, dirty: false, changes: Vec::new(), validation_issues, diff: String::new() }); }
        let git = |args: &[&str]| Command::new("git").args(args).current_dir(&root).output();
        let Ok(status) = git(&["status", "--porcelain=v1", "--branch"]) else { return Ok(TeamStatus { team_id, root, is_git: false, branch: None, dirty: false, changes: Vec::new(), validation_issues, diff: String::new() }); };
        if !status.status.success() { return Ok(TeamStatus { team_id, root, is_git: false, branch: None, dirty: false, changes: Vec::new(), validation_issues, diff: String::new() }); }
        let lines = String::from_utf8_lossy(&status.stdout).lines().map(str::to_string).collect::<Vec<_>>();
        let branch = lines.first().and_then(|line| line.strip_prefix("## ")).map(str::to_string);
        let changes = lines.into_iter().skip(1).collect::<Vec<_>>();
        let diff = git(&["diff", "--no-ext-diff", "--", "knowledge", "methods", "experiences"]).ok().map(|output| String::from_utf8_lossy(&output.stdout).to_string()).unwrap_or_default();
        Ok(TeamStatus { team_id, root, is_git: true, branch, dirty: !changes.is_empty(), changes, validation_issues, diff })
    }

    /// Write a local, reviewable publish plan. It never commits, pushes, merges,
    /// or discards a Team working tree.
    pub fn create_team_publish_plan(&self) -> Result<PathBuf, CoreError> {
        let status = self.team_status()?;
        let blocking = status.validation_issues.iter().filter(|issue| issue.severity == crate::graph::IssueSeverity::Error).map(|issue| format!("{}: {}", issue.path, issue.message)).collect::<Vec<_>>();
        if !blocking.is_empty() {
            return Err(CoreError::Other(format!("Team publish blocked by validation errors: {}", blocking.join("; "))));
        }
        let run_id = format!("publish_{}", Uuid::new_v4());
        let root = self.home.join("runs").join(&run_id);
        fs::create_dir_all(&root)?;
        let issues = if status.validation_issues.is_empty() { "none".into() } else { status.validation_issues.iter().map(|issue| format!("- [{}] {}: {}", issue.severity.as_str(), issue.path, issue.message)).collect::<Vec<_>>().join("\n") };
        let changes = if status.changes.is_empty() { "none".into() } else { status.changes.join("\n") };
        fs::write(root.join("publish-plan.md"), format!("# Team publish plan\n\nroot: {}\ngit: {}\nbranch: {}\ndirty: {}\n\n## Validation\n\n{}\n\n## Changes\n\n```text\n{}\n```\n\n## Diff\n\n```diff\n{}\n```\n\nThis is a plan only. Review and commit/push with normal Git tooling.\n", status.root.display(), status.is_git, status.branch.as_deref().unwrap_or("unknown"), status.dirty, issues, changes, status.diff))?;
        Ok(root.join("publish-plan.md"))
    }

    /// Start a focused learning conversation with a maintainer-selected permission mode. No task, workspace,
    /// capsule, or native coding session is created.
    pub async fn start_learning(&self, runtime: Option<&str>, permission_mode: &str, goal: &str) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), CoreError> {
        let runtime = self.preferred_runtime(runtime);
        let (permission_mode, sandbox) = permission_profile(permission_mode);
        let adapter = self.adapter(&runtime)?;
        let root_paths = self.context_roots().into_iter().map(|(_, path)| path).collect::<Vec<_>>();
        let (goal_with_mentions, mentioned_dirs) = crate::mentions::prepare_prompt(goal, &root_paths);
        let session_id = format!("learn_{}", Uuid::new_v4());
        let protocol = fs::read_to_string(self.home.join("protocols/deliberate-learning.md"))
            .unwrap_or_else(|_| "Clarify the goal, inspect evidence, challenge assumptions, verify counterexamples, and propose candidates for review.".into());
        let prompt = format!(
            "You are the Methodus deliberate-learning runtime.\n\nLearning goal:\n{goal_with_mentions}\n\nFollow this protocol:\n{protocol}\n\nSeparate facts, inferences, contradictions, and unknowns. Ask consequential maintainer questions. Finish with a CandidateSet only when the evidence is sufficient; otherwise keep asking focused questions. Never claim a draft is canonical. When synthesis is ready, include a fenced `json` block with exactly {{\"candidates\":[{{\"type\":\"knowledge|method|experience\",\"kind\":\"...\",\"title\":\"...\",\"summary\":\"...\",\"learn\":\"...\",\"decide\":\"...\",\"execute\":\"...\",\"evidence\":\"...\",\"outcome\":\"...\",\"occurred_at\":\"...\",\"tags\":[\"...\"]}}],\"relations\":[],\"unresolved_questions\":[],\"contradictions\":[]}}."
        );
        self.write_learn_state(&session_id, goal, &runtime, Some(permission_mode), "running", None)?;
        self.record_learning_sources(&session_id, goal)?;
        let mut extra_dirs = root_paths;
        extra_dirs.extend(mentioned_dirs);
        extra_dirs.extend(source_directories(&self.launch_cwd, goal));
        extra_dirs.sort(); extra_dirs.dedup();
        let spawn = adapter.spawn(SpawnInput {
            prompt,
            cwd: self.launch_cwd.clone(),
            session_id: session_id.clone(),
            permission_mode: permission_mode.into(),
            allowed_tools: vec!["Read".into(), "Glob".into(), "Grep".into(), "WebSearch".into()],
            sandbox: Some(sandbox.into()),
            extra_dirs,
            model: None,
        }).await;
        let (handle, events) = match spawn {
            Ok(result) => result,
            Err(error) => {
                let _ = self.mark_learning_status(&session_id, "failed");
                return Err(error.into());
            }
        };
        self.write_learn_state(&session_id, goal, &runtime, Some(permission_mode), "running", handle.executor_sid.as_deref())?;
        Ok((handle, events))
    }

    /// Continue the same focused Learn conversation using the runtime executor ID.
    pub async fn continue_learning(&self, runtime: &str, permission_mode: &str, executor_sid: &str, session_id: &str, prompt: &str) -> Result<(SessionHandle, mpsc::Receiver<RuntimeEvent>), CoreError> {
        let adapter = self.adapter(runtime)?;
        let (permission_mode, sandbox) = permission_profile(permission_mode);
        let root_paths = self.context_roots().into_iter().map(|(_, path)| path).collect::<Vec<_>>();
        let (prompt_with_mentions, mentioned_dirs) = crate::mentions::prepare_prompt(prompt, &root_paths);
        let mut extra_dirs = root_paths;
        extra_dirs.extend(mentioned_dirs);
        extra_dirs.extend(source_directories(&self.launch_cwd, prompt));
        extra_dirs.sort(); extra_dirs.dedup();
        let resume = adapter.resume(executor_sid, SpawnInput {
            prompt: prompt_with_mentions, cwd: self.launch_cwd.clone(), session_id: session_id.into(),
            permission_mode: permission_mode.into(), allowed_tools: vec!["Read".into(), "Glob".into(), "Grep".into(), "WebSearch".into()],
            sandbox: Some(sandbox.into()), extra_dirs, model: None,
        }).await;
        let (handle, events) = match resume {
            Ok(result) => result,
            Err(error) => {
                let _ = self.mark_learning_status(session_id, "failed");
                return Err(error.into());
            }
        };
        let goal = self.list_learning_runs()?.into_iter().find(|run| run.run_id == session_id).map(|run| run.goal).unwrap_or_else(|| prompt.into());
        self.write_learn_state(session_id, &goal, runtime, Some(permission_mode), "running", handle.executor_sid.as_deref().or(Some(executor_sid)))?;
        self.record_learning_sources(session_id, prompt)?;
        Ok((handle, events))
    }

    /// Write a Learn transcript and a review-only CandidateSet. The candidates
    /// are deliberately separate from canonical Personal/Team content.
    pub fn record_learning_output(&self, goal: &str, output: &str) -> Result<Vec<String>, CoreError> {
        let run_id = format!("learn_{}", Uuid::new_v4());
        self.record_learning_output_for_run(&run_id, goal, output)
    }

    pub fn record_learning_output_for_run(&self, run_id: &str, goal: &str, output: &str) -> Result<Vec<String>, CoreError> {
        let run_root = self.home.join("runs").join(&run_id);
        fs::create_dir_all(&run_root)?;
        let runtime = self.list_learning_runs()?.into_iter().find(|run| run.run_id == run_id).map(|run| run.runtime).unwrap_or_else(|| "unknown".into());
        let existing = self.list_learning_runs()?.into_iter().find(|run| run.run_id == run_id);
        self.write_learn_state(run_id, goal, &runtime, existing.as_ref().map(|run| run.permission_mode.as_str()), "awaiting_review", existing.as_ref().and_then(|run| run.executor_sid.as_deref()))?;
        fs::write(run_root.join("assistant.md"), output.trim())?;
        self.record_learning_sources(run_id, goal)?;

        let slug = slug_for_learning(goal);
        let suffix = run_id.strip_prefix("learn_").unwrap_or(&run_id).chars().take(8).collect::<String>();
        let candidates_root = self.home.join("personal/candidates");
        fs::create_dir_all(&candidates_root)?;
        let set = extract_candidate_set(output).unwrap_or_else(|| CandidateSet {
            candidates: Vec::new(),
            relations: Vec::new(),
            unresolved_questions: vec!["runtime did not return a structured CandidateSet".into()],
            contradictions: Vec::new(),
            runtime_skills: Vec::new(),
        });
        let runtime_skills = set.runtime_skills.clone();
        let drafts = set.candidates;
        let candidate_ids = drafts.iter().enumerate().map(|(index, draft)| {
            let node_type = match draft.node_type.to_ascii_lowercase().as_str() { "method" => "method", "experience" => "experience", _ => "knowledge" };
            (node_type.to_string(), format!("{node_type}/candidate-{slug}-{suffix}-{index}"))
        }).collect::<Vec<_>>();
        let (links, unresolved_relations) = candidate_links(&set.relations, &drafts, &candidate_ids);
        let mut unresolved_questions = set.unresolved_questions.clone();
        unresolved_questions.extend(unresolved_relations);
        let review_notes = format!(
            "### Unresolved questions\n\n{}\n\n### Contradictions\n\n{}",
            if unresolved_questions.is_empty() { "none".into() } else { unresolved_questions.iter().map(|item| format!("- {item}")).collect::<Vec<_>>().join("\n") },
            if set.contradictions.is_empty() { "none".into() } else { set.contradictions.iter().map(|item| format!("- {item}")).collect::<Vec<_>>().join("\n") },
        );
        let state = LearnRunState {
            run_id: run_id.into(), kind: "learn".into(), status: "awaiting_review".into(), goal: goal.trim().into(),
            runtime,
            permission_mode: existing.as_ref().map(|run| run.permission_mode.clone()).unwrap_or_else(default_permission_mode),
            executor_sid: existing.and_then(|run| run.executor_sid),
            unresolved_questions,
            contradictions: set.contradictions.clone(),
            updated_at: Utc::now().to_rfc3339(),
        };
        fs::write(run_root.join("state.yaml"), serde_yaml::to_string(&state).map_err(|error| CoreError::Other(format!("serialize Learn state: {error}")))?)?;
        let mut ids = Vec::new();
        for (index, draft) in drafts.into_iter().enumerate() {
            let node_type = match draft.node_type.to_ascii_lowercase().as_str() { "method" => "method", "experience" => "experience", _ => "knowledge" };
            let kind = yaml_quote(draft.kind.as_deref().unwrap_or(if node_type == "experience" { "case" } else if node_type == "method" { "workflow" } else { "procedure" }));
            let title = yaml_quote(if draft.title.trim().is_empty() { goal } else { &draft.title });
            let summary = yaml_quote(draft.summary.as_deref().unwrap_or("Learning runtime proposal awaiting maintainer review."));
            let id = candidate_ids[index].1.clone();
            let tags = if draft.tags.is_empty() { "[]".into() } else { format!("[{}]", draft.tags.iter().map(|tag| format!("\"{}\"", yaml_quote(tag))).collect::<Vec<_>>().join(", ")) };
            let evidence = draft.evidence.as_deref().unwrap_or("Evidence is recorded in the Learn run and must be checked during Review.").to_string();
            let evidence = if node_type == "experience" && !runtime_skills.is_empty() {
                let observations = runtime_skills.iter().map(|skill| format!("- {} · runtime: {} · outcome: {} · {}", skill.name, skill.runtime.as_deref().unwrap_or("unknown"), skill.outcome.as_deref().unwrap_or("observed"), skill.reason.as_deref().unwrap_or("no reason recorded"))).collect::<Vec<_>>().join("\n");
                format!("{evidence}\n\n### Runtime Skills observed\n\n{observations}")
            } else { evidence };
            let experience_meta = if node_type == "experience" {
                let mut meta = String::new();
                if let Some(outcome) = draft.outcome.as_deref().filter(|value| !value.trim().is_empty()) { meta.push_str(&format!("outcome: \"{}\"\n", yaml_quote(outcome))); }
                if let Some(occurred_at) = draft.occurred_at.as_deref().filter(|value| !value.trim().is_empty()) { meta.push_str(&format!("occurred_at: \"{}\"\n", yaml_quote(occurred_at))); }
                meta
            } else { String::new() };
            let body = format!(
                "---\nid: {id}\ntitle: \"{title}\"\nnode_type: {node_type}\nkind: {kind}\nstatus: candidate\nvisibility: personal\nsummary: \"{summary}\"\ntags: {tags}\n{experience_meta}sources:\n  - path: runs/{run_id}/assistant.md\n    type: learn-run\n  - path: runs/{run_id}/sources.yaml\n    type: learn-source-manifest\n{}---\n\n## Learn\n\n{}\n\n## Decide\n\n{}\n\n## Execute\n\n{}\n\n## Evidence\n\n{}\n\n## Review notes\n\n{}\n\n- Learn run: runs/{run_id}/assistant.md\n- Source manifest: runs/{run_id}/sources.yaml\n- Goal: {}\n",
                links.get(&id).map(|value| format!("links:\n{value}")).unwrap_or_else(|| "links: {}\n".into()),
                draft.learn.as_deref().unwrap_or(output),
                draft.decide.as_deref().unwrap_or("Review applicability, alternatives, boundaries, and contradictions before promotion."),
                draft.execute.as_deref().unwrap_or("Rewrite this section into a compact, safe rule before exposing it to an Agent runtime."),
                evidence,
                review_notes,
                yaml_quote(goal),
            );
            fs::write(candidates_root.join(format!("{node_type}-{slug}-{run_id}-{index}.md")), body)?;
            ids.push(id);
        }
        self.sync_graph()?;
        Ok(ids)
    }

    pub fn promote_graph_candidate(&self, node_id: &str) -> Result<(), CoreError> {
        let node = self.store.graph_node(node_id)?.ok_or_else(|| CoreError::Other(format!("graph node not found: {node_id}")))?;
        if node.status.as_deref() != Some("candidate") { return Err(CoreError::Other(format!("{node_id} is not a candidate"))); }
        let path = self.node_path(&node)?;
        self.ensure_reviewable_path(&node.path)?;
        let raw = fs::read_to_string(&path)?;
        let updated = replace_frontmatter_value(&raw, "status", "candidate", "committed").ok_or_else(|| CoreError::Other(format!("{node_id} has no candidate status")))?;
        let root = if node.visibility == "team" { self.team_root() } else { self.home.join("personal") };
        let dir = root.join(match node.node_type.as_str() { "knowledge" => "knowledge", "method" => "methods", "experience" => "experiences", other => return Err(CoreError::Other(format!("unsupported candidate type: {other}"))) });
        fs::create_dir_all(&dir)?;
        let target = dir.join(path.file_name().ok_or_else(|| CoreError::Other("candidate has no filename".into()))?);
        if target != path && target.exists() { return Err(CoreError::Other(format!("canonical target already exists: {} (use merge instead)", target.display()))); }
        fs::write(&target, updated)?;
        if path != target { fs::remove_file(path)?; }
        self.record_review_action(node_id, "commit", "candidate approved in Methodus Review")?;
        self.sync_graph()?;
        Ok(())
    }

    pub fn reject_graph_candidate(&self, node_id: &str) -> Result<(), CoreError> {
        let node = self.store.graph_node(node_id)?.ok_or_else(|| CoreError::Other(format!("graph node not found: {node_id}")))?;
        if node.status.as_deref() != Some("candidate") { return Err(CoreError::Other(format!("{node_id} is not a candidate"))); }
        let path = self.node_path(&node)?;
        let raw = fs::read_to_string(&path)?;
        let updated = replace_frontmatter_value(&raw, "status", "candidate", "rejected").ok_or_else(|| CoreError::Other(format!("{node_id} has no candidate status")))?;
        fs::write(path, updated)?;
        self.record_review_action(node_id, "reject", "candidate rejected in Methodus Review")?;
        self.sync_graph()?;
        Ok(())
    }

    /// Mark a committed or stale node as historical. Deprecated nodes remain
    /// readable through explicit Agent history queries but are never selected by
    /// normal retrieval.
    pub fn deprecate_graph_node(&self, node_id: &str, rationale: &str) -> Result<(), CoreError> {
        let node = self.store.graph_node(node_id)?.ok_or_else(|| CoreError::Other(format!("graph node not found: {node_id}")))?;
        let status = node.status.as_deref().unwrap_or("committed");
        if !matches!(status, "committed" | "stale") {
            return Err(CoreError::Other(format!("{node_id} cannot be deprecated from status {status}")));
        }
        let path = self.node_path(&node)?;
        let raw = fs::read_to_string(&path)?;
        // `stale` is a projection state derived from source fingerprints; the
        // authored Markdown may still say `committed`. Preserve that source
        // contract while allowing an explicitly reviewed archive action.
        let authored_status = crate::graph::read_graph_document(&self.home, &path)?.node.status.unwrap_or_else(|| "committed".into());
        let updated = replace_frontmatter_value(&raw, "status", &authored_status, "deprecated")
            .ok_or_else(|| CoreError::Other(format!("{node_id} has no editable status frontmatter")))?;
        fs::write(path, updated)?;
        self.record_review_action(node_id, "deprecate", rationale)?;
        self.sync_graph()?;
        Ok(())
    }

    /// Revalidate a stale node against its recorded local source fingerprints.
    /// Revalidation never changes the body or source declaration.
    pub fn revalidate_graph_node(&self, node_id: &str, rationale: &str) -> Result<(), CoreError> {
        let node = self.store.graph_node(node_id)?.ok_or_else(|| CoreError::Other(format!("graph node not found: {node_id}")))?;
        if node.status.as_deref() != Some("stale") {
            return Err(CoreError::Other(format!("{node_id} is not stale")));
        }
        let path = self.node_path(&node)?;
        let document = crate::graph::read_graph_document(&self.home, &path)?;
        if crate::graph::sources_are_stale_now(&self.home, &document.sources) {
            return Err(CoreError::Other(format!("{node_id} still has changed or missing evidence")));
        }
        let raw = fs::read_to_string(&path)?;
        // A stale row is derived at sync time. If the authored file already
        // says committed, revalidation only records the maintainer decision.
        if document.node.status.as_deref() == Some("stale") {
            let updated = replace_frontmatter_value(&raw, "status", "stale", "committed")
                .ok_or_else(|| CoreError::Other(format!("{node_id} has no editable status frontmatter")))?;
            fs::write(path, updated)?;
        }
        self.record_review_action(node_id, "revalidate", rationale)?;
        self.sync_graph()?;
        Ok(())
    }

    pub fn promote_candidate_to_team(&self, node_id: &str) -> Result<(), CoreError> {
        let node = self.store.graph_node(node_id)?.ok_or_else(|| CoreError::Other(format!("graph node not found: {node_id}")))?;
        if node.status.as_deref() != Some("candidate") { return Err(CoreError::Other(format!("{node_id} is not a candidate"))); }
        let path = self.node_path(&node)?;
        self.ensure_reviewable_path(&node.path)?;
        let raw = fs::read_to_string(&path)?;
        let updated = if raw.lines().any(|line| line.trim_start().starts_with("visibility:")) {
            replace_frontmatter_value(&raw, "visibility", "personal", "team").unwrap_or(raw)
        } else {
            raw.replacen("---\n", "---\nvisibility: team\n", 1)
        };
        let dir = self.team_root().join(match node.node_type.as_str() {
            "knowledge" => "knowledge", "method" => "methods", "experience" => "experiences",
            other => return Err(CoreError::Other(format!("unsupported candidate type: {other}"))),
        });
        fs::create_dir_all(&dir)?;
        let target = dir.join(path.file_name().ok_or_else(|| CoreError::Other("candidate has no filename".into()))?);
        if target != path && target.exists() { return Err(CoreError::Other(format!("Team target already exists: {}", target.display()))); }
        fs::write(&target, updated)?;
        if path != target { fs::remove_file(path)?; }
        self.record_review_action(node_id, "mark_team", "candidate marked for Team visibility")?;
        self.sync_graph()?;
        Ok(())
    }

    pub fn merge_graph_candidate(&self, candidate_id: &str, target_id: &str) -> Result<(), CoreError> {
        let candidate = self.store.graph_node(candidate_id)?.ok_or_else(|| CoreError::Other(format!("graph node not found: {candidate_id}")))?;
        let target = self.store.graph_node(target_id)?.ok_or_else(|| CoreError::Other(format!("graph node not found: {target_id}")))?;
        if candidate.status.as_deref() != Some("candidate") || candidate.node_type != "knowledge" || target.node_type != "knowledge" || target.status.as_deref() != Some("committed") { return Err(CoreError::Other("merge requires a candidate Knowledge and a committed Knowledge target".into())); }
        let candidate_path = self.node_path(&candidate)?;
        let target_path = self.node_path(&target)?;
        self.ensure_reviewable_path(&candidate.path)?;
        let candidate_doc = crate::graph::read_graph_document(&self.home, &candidate_path)?;
        let target_raw = fs::read_to_string(&target_path)?;
        fs::write(&target_path, format!("{}\n\n## Merged evidence from {}\n\n{}\n", target_raw.trim_end(), candidate_id, candidate_doc.body.trim()))?;
        let raw = fs::read_to_string(&candidate_path)?;
        let updated = replace_frontmatter_value(&raw, "status", "candidate", "rejected").unwrap_or(raw);
        let updated = if updated.lines().any(|line| line.trim_start().starts_with("merged_into:")) { updated } else { updated.replacen("---\n", &format!("---\nmerged_into: {target_id}\n"), 1) };
        fs::write(candidate_path, updated)?;
        self.record_review_action(candidate_id, "merge", &format!("merged into {target_id}"))?;
        self.sync_graph()?;
        Ok(())
    }

    fn record_review_action(&self, node_id: &str, action: &str, rationale: &str) -> Result<(), CoreError> {
        let path = self.home.join("runs/reviews.jsonl");
        if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
        let record = serde_json::json!({
            "at": Utc::now().to_rfc3339(),
            "node_id": node_id,
            "action": action,
            "rationale": rationale.trim(),
        });
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(file, "{}", serde_json::to_string(&record).unwrap_or_else(|_| "{}".into()))?;
        Ok(())
    }

    fn ensure_reviewable_path(&self, relative_path: &str) -> Result<(), CoreError> {
        let errors = crate::graph::validate_graph(&self.home)?.into_iter()
            .filter(|issue| issue.path == relative_path && issue.severity == crate::graph::IssueSeverity::Error)
            .map(|issue| issue.message)
            .collect::<Vec<_>>();
        if errors.is_empty() { Ok(()) } else { Err(CoreError::Other(format!("Review blocked: {}", errors.join("; ")))) }
    }

    fn node_path(&self, node: &GraphNode) -> Result<PathBuf, CoreError> {
        let relative = Path::new(&node.path);
        if relative.is_absolute() || relative.components().any(|component| component == std::path::Component::ParentDir) { return Err(CoreError::Other(format!("unsafe graph path: {}", node.path))); }
        Ok(self.home.join(relative))
    }
}

fn yaml_quote(value: &str) -> String { value.replace('"', "'").replace('\n', " ").trim().to_string() }

fn source_token(token: &str, cwd: &Path) -> Option<String> {
    let token = token.strip_prefix('@')?.trim_matches(|ch: char| ",.;:!?()[]{}<>".contains(ch));
    if token.is_empty() { return None; }
    let path_like = token.starts_with('/') || token.starts_with("~/") || token == "." || token == ".." || token.contains('/') || token.contains('.');
    let path = if token == "~" || token.starts_with("~/") {
        std::env::var_os("HOME").map(PathBuf::from).map(|home| if token == "~" { home } else { home.join(token.trim_start_matches("~/")) })
    } else {
        Some(Path::new(token).is_absolute().then(|| PathBuf::from(token)).unwrap_or_else(|| cwd.join(token)))
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
    if path.is_absolute() { (path.to_path_buf(), token.to_string()) } else { (cwd.join(path), token.to_string()) }
}

fn source_directories(cwd: &Path, text: &str) -> Vec<PathBuf> {
    text.split_whitespace().filter_map(|token| source_token(token, cwd)).map(|token| {
        let (path, _) = resolve_source_path(cwd, &token);
        if path.is_dir() { path } else { path.parent().unwrap_or(cwd).to_path_buf() }
    }).filter(|path| path.is_dir()).collect()
}

fn extract_candidate_set(answer: &str) -> Option<CandidateSet> {
    let mut cursor = 0;
    while let Some(start) = answer[cursor..].find("```") {
        let start = cursor + start;
        let after_fence = &answer[start + 3..];
        let content_start = after_fence.find('\n').map(|offset| start + 3 + offset + 1)?;
        let end = answer[content_start..].find("```").map(|offset| content_start + offset)?;
        let json = answer[content_start..end].trim();
        if let Ok(set) = serde_json::from_str::<CandidateSet>(json) {
            return Some(set);
        }
        cursor = end + 3;
    }
    None
}

fn candidate_links(relations: &[CandidateRelation], drafts: &[CandidateDraft], ids: &[(String, String)]) -> (BTreeMap<String, String>, Vec<String>) {
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
        if relation_name.is_empty() || !relation_name.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')) {
            unresolved.push(format!("relation between {from} and {to} has an invalid relation type"));
            continue;
        }
        grouped.entry(from).or_default().entry(relation_name.to_string()).or_default().push(to);
    }
    let links = grouped.into_iter().map(|(from, relations)| {
        let yaml = relations.into_iter().map(|(relation, targets)| {
            let values = targets.into_iter().map(|target| format!("    - {target}")).collect::<Vec<_>>().join("\n");
            format!("  {relation}:\n{values}")
        }).collect::<Vec<_>>().join("\n");
        (from, yaml)
    }).collect();
    (links, unresolved)
}

fn resolve_candidate_ref(value: &str, drafts: &[CandidateDraft], ids: &[(String, String)]) -> Option<String> {
    let value = value.trim();
    if value.is_empty() { return None; }
    if let Ok(index) = value.strip_prefix("candidate-").unwrap_or(value).parse::<usize>() {
        return ids.get(index).map(|(_, id)| id.clone());
    }
    if value.contains('/') && value.chars().all(|ch| !ch.is_whitespace() && !matches!(ch, ':' | '[' | ']' | '{' | '}')) { return Some(value.to_string()); }
    drafts.iter().position(|draft| draft.title.eq_ignore_ascii_case(value)).and_then(|index| ids.get(index).map(|(_, id)| id.clone()))
}

fn replace_frontmatter_value(raw: &str, key: &str, from: &str, to: &str) -> Option<String> {
    let rest = raw.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    let front_end = 4 + end;
    let (front, marker_and_body) = raw.split_at(front_end);
    let body = marker_and_body.strip_prefix("\n---\n")?;
    let mut replaced = false;
    let lines = front.lines().map(|line| {
        let prefix = format!("{key}:");
        if line.trim_start().starts_with(&prefix) && line.split_once(':').map(|(_, value)| value.trim().trim_matches('"').trim_matches('\'') == from).unwrap_or(false) {
            replaced = true;
            format!("{key}: {to}")
        } else { line.to_string() }
    }).collect::<Vec<_>>().join("\n");
    replaced.then(|| format!("{lines}\n---\n{body}"))
}

fn slug_for_learning(value: &str) -> String {
    let mut slug = value.chars().filter_map(|ch| if ch.is_ascii_alphanumeric() { Some(ch.to_ascii_lowercase()) } else if ch.is_whitespace() || ch == '-' || ch == '_' { Some('-') } else { None }).collect::<String>();
    while slug.contains("--") { slug = slug.replace("--", "-"); }
    let slug = slug.trim_matches('-');
    if slug.is_empty() { "learning-result".into() } else { slug.chars().take(48).collect() }
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
        assert_eq!(permission_profile("cautious"), ("cautious", "workspace-write"));
        assert_eq!(permission_profile("acceptEdits"), ("acceptEdits", "workspace-write"));
        assert_eq!(permission_profile("bypassPermissions"), ("plan", "read-only"));
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
    fn empty_candidate_set_remains_a_no_publish_learning_result() {
        let set = extract_candidate_set("```json\n{\"candidates\":[],\"unresolved_questions\":[\"scope\"]}\n```").unwrap();
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
    fn learning_output_preserves_run_state_and_materializes_relations() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("personal/candidates")).unwrap();
        let engine = Engine::with_runtimes(Arc::new(Store::open_memory().unwrap()), dir.path().to_path_buf(), HashMap::new());
        let run_id = "learn_test";
        engine.write_learn_state(run_id, "shutdown diagnosis", "claude-code", Some("plan"), "awaiting_input", Some("exec-1")).unwrap();
        let output = "```json\n{\"candidates\":[{\"type\":\"knowledge\",\"kind\":\"signal\",\"title\":\"Reason\",\"summary\":\"Read reason\",\"learn\":\"Explain\"},{\"type\":\"experience\",\"kind\":\"incident\",\"title\":\"Case\",\"summary\":\"A case\",\"learn\":\"Observed\"}],\"relations\":[{\"from\":\"candidate-0\",\"relation\":\"validated_by\",\"to\":\"candidate-1\"}],\"unresolved_questions\":[\"scope?\"],\"contradictions\":[\"old claim\"],\"runtime_skills\":[{\"name\":\"repo-survey\",\"runtime\":\"claude-code\",\"outcome\":\"useful\",\"reason\":\"found the source\"}]}\n```";
        let ids = engine.record_learning_output_for_run(run_id, "shutdown diagnosis", output).unwrap();
        assert_eq!(ids.len(), 2);
        let run = engine.list_learning_runs().unwrap().into_iter().find(|run| run.run_id == run_id).unwrap();
        assert_eq!(run.runtime, "claude-code");
        assert_eq!(run.permission_mode, "plan");
        assert_eq!(run.executor_sid.as_deref(), Some("exec-1"));
        assert_eq!(run.status, "awaiting_review");
        let candidate_files = fs::read_dir(dir.path().join("personal/candidates")).unwrap().count();
        assert_eq!(candidate_files, 2);
        let state = fs::read_to_string(dir.path().join("runs/learn_test/state.yaml")).unwrap();
        assert!(state.contains("scope?"));
        let candidates = fs::read_dir(dir.path().join("personal/candidates")).unwrap().filter_map(Result::ok).filter_map(|entry| fs::read_to_string(entry.path()).ok()).collect::<Vec<_>>();
        assert!(candidates.iter().any(|candidate| candidate.contains("validated_by")));
        assert!(candidates.iter().any(|candidate| candidate.contains("repo-survey")));
    }

    #[test]
    fn team_promotion_moves_candidate_into_selected_team_before_commit() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("personal/candidates")).unwrap();
        fs::create_dir_all(dir.path().join("teams/default")).unwrap();
        let store = Arc::new(Store::open_memory().unwrap());
        let engine = Engine::with_runtimes(store, dir.path().to_path_buf(), HashMap::new());
        engine.record_learning_output("team rule", "```json\n{\"candidates\":[{\"type\":\"knowledge\",\"kind\":\"procedure\",\"title\":\"Team rule\",\"summary\":\"A reviewed rule\",\"learn\":\"Explain\",\"execute\":\"Do it\"}]}\n```").unwrap();
        let candidate = engine.list_graph_nodes(Some("Team rule")).unwrap().into_iter().find(|node| node.status.as_deref() == Some("candidate")).unwrap();
        engine.promote_candidate_to_team(&candidate.id).unwrap();
        let moved = engine.list_graph_nodes(Some("Team rule")).unwrap().into_iter().find(|node| node.id == candidate.id).unwrap();
        assert_eq!(moved.visibility, "team");
        assert!(moved.path.starts_with("teams/default/"));
        engine.promote_graph_candidate(&moved.id).unwrap();
        let committed = engine.list_graph_nodes(Some("Team rule")).unwrap().into_iter().find(|node| node.id == candidate.id).unwrap();
        assert_eq!(committed.status.as_deref(), Some("committed"));
        assert!(committed.path.starts_with("teams/default/"));
    }

    #[test]
    fn projected_stale_nodes_can_be_revalidated_or_archived() {
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
        let stale = engine.list_graph_nodes(Some("Source")).unwrap().pop().unwrap();
        assert_eq!(stale.status.as_deref(), Some("stale"));
        // Restore the evidence without editing the authored status. The
        // projection should return to committed after explicit revalidation.
        fs::write(dir.path().join("evidence.txt"), "one").unwrap();
        engine.revalidate_graph_node(&stale.id, "checked source").unwrap();
        assert_eq!(engine.list_graph_nodes(Some("Source")).unwrap().pop().unwrap().status.as_deref(), Some("committed"));
        fs::write(dir.path().join("evidence.txt"), "two").unwrap();
        engine.sync_graph().unwrap();
        let stale = engine.list_graph_nodes(Some("Source")).unwrap().pop().unwrap();
        engine.deprecate_graph_node(&stale.id, "superseded").unwrap();
        assert_eq!(engine.list_graph_nodes(Some("Source")).unwrap().pop().unwrap().status.as_deref(), Some("deprecated"));
    }
}
