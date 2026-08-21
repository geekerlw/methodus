use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Local, NaiveTime, Timelike, Utc};
use methodus_core::{ensure_home, methodus_home, Engine};
use methodus_domain::{GraphEdge, GraphNode, RuntimeEvent, UsageDelta};
use methodus_runtime::{
    ClaudeCodeAdapter, CodexAdapter, CursorAdapter, RuntimeAdapter, SessionHandle,
};
use methodus_store::Store;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, State, WindowEvent,
};
use tokio::time::{sleep, Duration as TokioDuration};
use uuid::Uuid;

mod attention;
use attention::HumanAttention;

#[derive(Debug, thiserror::Error)]
enum AppError {
    #[error("{0}")]
    Message(String),
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl From<methodus_core::CoreError> for AppError {
    fn from(error: methodus_core::CoreError) -> Self {
        Self::Message(error.to_string())
    }
}

impl From<methodus_store::StoreError> for AppError {
    fn from(error: methodus_store::StoreError) -> Self {
        Self::Message(error.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LearningGoal {
    id: String,
    title: String,
    prompt: String,
    #[serde(default)]
    sources: Vec<String>,
    runtime: String,
    permission_mode: String,
    cadence: String,
    #[serde(default = "default_review_cadence")]
    review_cadence: String,
    #[serde(default = "default_summary_cadence")]
    summary_cadence: String,
    #[serde(default = "default_source_check_cadence")]
    source_check_cadence: String,
    #[serde(default)]
    quiet_hours_start: Option<String>,
    #[serde(default)]
    quiet_hours_end: Option<String>,
    #[serde(default = "default_budget_usd")]
    budget_usd: f64,
    #[serde(default = "default_review_policy")]
    review_policy: String,
    enabled: bool,
    next_run_at: Option<String>,
    #[serde(default)]
    next_review_at: Option<String>,
    #[serde(default)]
    next_summary_at: Option<String>,
    #[serde(default)]
    next_source_check_at: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
struct ActiveRun {
    run_id: String,
    runtime: String,
    goal_id: Option<String>,
}

struct ActiveSession {
    handle: SessionHandle,
    runtime: String,
    goal_id: Option<String>,
    output: String,
}

struct EmbeddedTerminal {
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn Child + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    handoff: methodus_core::NativeLearnHandoff,
}

struct AppState {
    engine: Engine,
    sessions: Mutex<HashMap<String, ActiveSession>>,
    terminals: Mutex<HashMap<String, Arc<EmbeddedTerminal>>>,
    attentions: Mutex<HashMap<String, HumanAttention>>,
    goals: Mutex<Vec<LearningGoal>>,
    stale_notices: Mutex<HashSet<String>>,
    goal_usage: Mutex<HashMap<String, GoalUsage>>,
    run_goal_links: Mutex<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GoalUsage {
    month: String,
    spent_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
struct TeamSummary {
    id: String,
    root: String,
    is_git: bool,
    branch: Option<String>,
    dirty: bool,
    changes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct Dashboard {
    home: String,
    nodes: Vec<GraphNode>,
    runs: Vec<methodus_core::LearnRun>,
    goals: Vec<LearningGoal>,
    active_runs: Vec<ActiveRun>,
    team: TeamSummary,
    review_count: usize,
    stale_count: usize,
    goal_usage: HashMap<String, f64>,
    attentions: Vec<HumanAttention>,
}

#[derive(Debug, Clone, Serialize)]
struct RunDetails {
    run: methodus_core::LearnRun,
    events: Vec<methodus_core::LearnEventRecord>,
    attention: Option<HumanAttention>,
}

#[derive(Debug, Clone, Serialize)]
struct NodeDetails {
    node: GraphNode,
    edges: Vec<GraphEdge>,
    kind: Option<String>,
    content: String,
    sources: Vec<methodus_core::graph::SourceEvidence>,
    run_id: Option<String>,
    revisions: Vec<RevisionPreview>,
}

#[derive(Debug, Clone, Serialize)]
struct RevisionPreview {
    id: String,
    path: String,
    status: Option<String>,
    content: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoalInput {
    title: String,
    prompt: String,
    #[serde(default)]
    sources: Vec<String>,
    #[serde(default = "default_runtime")]
    runtime: String,
    #[serde(default = "default_permission")]
    permission_mode: String,
    #[serde(default = "default_cadence")]
    cadence: String,
    #[serde(default = "default_review_cadence")]
    review_cadence: String,
    #[serde(default = "default_summary_cadence")]
    summary_cadence: String,
    #[serde(default = "default_source_check_cadence")]
    source_check_cadence: String,
    #[serde(default)]
    quiet_hours_start: Option<String>,
    #[serde(default)]
    quiet_hours_end: Option<String>,
    #[serde(default = "default_budget_usd")]
    budget_usd: f64,
    #[serde(default = "default_review_policy")]
    review_policy: String,
    #[serde(default = "default_enabled")]
    enabled: bool,
}

fn default_runtime() -> String {
    "claude-code".into()
}
fn default_permission() -> String {
    "plan".into()
}
fn default_cadence() -> String {
    "weekly".into()
}
fn default_review_cadence() -> String {
    "weekly".into()
}
fn default_summary_cadence() -> String {
    "monthly".into()
}
fn default_source_check_cadence() -> String {
    "daily".into()
}
fn default_enabled() -> bool {
    true
}
fn default_budget_usd() -> f64 {
    20.0
}
fn default_review_policy() -> String {
    "human_required".into()
}

fn goal_prompt_for(goal: &LearningGoal, work: &str) -> String {
    let sources = if goal.sources.is_empty() {
        "none specified".into()
    } else {
        goal.sources
            .iter()
            .map(|source| format!("- @{source}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let work_instructions = match work {
        "review" => "Scheduled review: inspect the currently published Methodus knowledge related to this goal, re-check its evidence and source freshness, and propose only narrowly scoped CandidateSet revisions for anything stale, contradicted, or incomplete.",
        "summary" => "Scheduled synthesis: summarize what is now known, what changed, unresolved questions, and the next useful learning step. Create CandidateSet entries only for durable knowledge or methods that deserve human Review.",
        _ => "Scheduled learning: investigate the goal, challenge assumptions, compare evidence, and return a CandidateSet only when the evidence is sufficient.",
    };
    format!(
        "{}\n\n{}\n\nAuthorized evidence sources (inspect these explicitly):\n{}\n\nExecution policy: monthly budget ${:.2}; review cadence {}; summary cadence {}; source checks {}; review policy {}; never publish canonical knowledge without a human decision.",
        goal.prompt,
        work_instructions,
        sources,
        goal.budget_usd,
        goal.review_cadence,
        goal.summary_cadence,
        goal.source_check_cadence,
        goal.review_policy,
    )
}

fn goals_path(engine: &Engine) -> PathBuf {
    engine.home().join("goals.json")
}

fn goal_usage_path(engine: &Engine) -> PathBuf {
    engine.home().join("goal-usage.json")
}

fn run_goal_links_path(engine: &Engine) -> PathBuf {
    engine.home().join("run-goals.json")
}

fn attentions_path(engine: &Engine) -> PathBuf {
    engine.home().join("attentions.json")
}

fn current_month() -> String {
    Utc::now().format("%Y-%m").to_string()
}

fn load_goal_usage(home: &std::path::Path) -> HashMap<String, GoalUsage> {
    fs::read_to_string(home.join("goal-usage.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn load_run_goal_links(home: &std::path::Path) -> HashMap<String, String> {
    fs::read_to_string(home.join("run-goals.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn load_attentions(home: &std::path::Path) -> HashMap<String, HumanAttention> {
    fs::read_to_string(home.join("attentions.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_attentions(state: &AppState) -> Result<(), AppError> {
    let snapshot = state
        .attentions
        .lock()
        .map_err(|_| AppError::Message("attentions lock poisoned".into()))?
        .clone();
    let path = attentions_path(&state.engine);
    let temporary = path.with_extension("json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(&snapshot)
            .map_err(|error| AppError::Message(error.to_string()))?,
    )?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn attention_for_run(state: &AppState, run_id: &str) -> Option<HumanAttention> {
    state.attentions.lock().ok().and_then(|attentions| {
        attentions
            .values()
            .filter(|attention| attention.run_id == run_id && attention.status == "open")
            .max_by(|left, right| left.created_at.cmp(&right.created_at))
            .cloned()
    })
}

fn open_attention(
    state: &AppState,
    app: &AppHandle,
    run_id: &str,
    kind: &str,
    title: &str,
    prompt: &str,
    context: Option<String>,
    tool_name: Option<String>,
    tool_input: Option<String>,
) -> Option<HumanAttention> {
    let mut attentions = state.attentions.lock().ok()?;
    if let Some(existing) = attentions
        .values()
        .find(|attention| attention.run_id == run_id && attention.status == "open")
        .cloned()
    {
        return Some(existing);
    }
    let attention = HumanAttention {
        id: format!("attention_{}", Uuid::new_v4()),
        run_id: run_id.into(),
        kind: kind.into(),
        title: title.into(),
        prompt: prompt.into(),
        context,
        tool_name,
        tool_input,
        status: "open".into(),
        created_at: Utc::now().to_rfc3339(),
        resolved_at: None,
        response: None,
    };
    attentions.insert(attention.id.clone(), attention.clone());
    drop(attentions);
    let _ = save_attentions(state);
    let _ = state.engine.record_learning_event(
        run_id,
        "methodus",
        &format!("Attention required: {} — {}", attention.title, attention.prompt),
    );
    let _ = app.emit("attention-required", &attention);
    Some(attention)
}

fn resolve_attention_for_run(state: &AppState, run_id: &str, response: &str) {
    let Ok(mut attentions) = state.attentions.lock() else { return; };
    let now = Utc::now().to_rfc3339();
    let mut changed = false;
    for attention in attentions.values_mut().filter(|attention| {
        attention.run_id == run_id && attention.status == "open"
    }) {
        attention.status = "resolved".into();
        attention.resolved_at = Some(now.clone());
        attention.response = Some(response.into());
        changed = true;
    }
    drop(attentions);
    if changed {
        let _ = save_attentions(state);
    }
}

fn save_run_goal_links(state: &AppState) -> Result<(), AppError> {
    let links = state
        .run_goal_links
        .lock()
        .map_err(|_| AppError::Message("run goal links lock poisoned".into()))?;
    fs::write(
        run_goal_links_path(&state.engine),
        serde_json::to_vec_pretty(&*links).map_err(|error| AppError::Message(error.to_string()))?,
    )?;
    Ok(())
}

fn save_goal_usage(state: &AppState) -> Result<(), AppError> {
    let usage = state
        .goal_usage
        .lock()
        .map_err(|_| AppError::Message("goal usage lock poisoned".into()))?;
    fs::write(
        goal_usage_path(&state.engine),
        serde_json::to_vec_pretty(&*usage).map_err(|error| AppError::Message(error.to_string()))?,
    )?;
    Ok(())
}

fn dashboard_goal_usage(state: &AppState) -> HashMap<String, f64> {
    let month = current_month();
    state
        .goal_usage
        .lock()
        .ok()
        .map(|usage| {
            usage
                .iter()
                .filter(|(_, value)| value.month == month)
                .map(|(id, value)| (id.clone(), value.spent_usd))
                .collect()
        })
        .unwrap_or_default()
}

fn goal_budget_exhausted(state: &AppState, goal: &LearningGoal) -> bool {
    let month = current_month();
    state
        .goal_usage
        .lock()
        .ok()
        .and_then(|usage| usage.get(&goal.id).cloned())
        .is_some_and(|usage| usage.month == month && usage.spent_usd >= goal.budget_usd)
}

fn record_goal_cost(state: &AppState, goal_id: &str, cost_usd: f64) -> Result<f64, AppError> {
    if !cost_usd.is_finite() || cost_usd <= 0.0 {
        return Ok(0.0);
    }
    let month = current_month();
    let mut usage = state
        .goal_usage
        .lock()
        .map_err(|_| AppError::Message("goal usage lock poisoned".into()))?;
    let entry = usage
        .entry(goal_id.to_string())
        .or_insert_with(|| GoalUsage {
            month: month.clone(),
            spent_usd: 0.0,
        });
    if entry.month != month {
        entry.month = month;
        entry.spent_usd = 0.0;
    }
    entry.spent_usd += cost_usd;
    let total = entry.spent_usd;
    drop(usage);
    save_goal_usage(state)?;
    Ok(total)
}

fn load_goals(home: &std::path::Path) -> Vec<LearningGoal> {
    let mut goals: Vec<LearningGoal> = fs::read_to_string(home.join("goals.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let now = Utc::now();
    for goal in &mut goals {
        if goal.enabled
            && (goal.next_run_at.is_none()
                || goal.next_review_at.is_none()
                || goal.next_summary_at.is_none()
                || goal.next_source_check_at.is_none())
        {
            set_goal_schedules(goal, now);
        }
    }
    goals
}

fn save_goals(state: &AppState) -> Result<(), AppError> {
    let goals = state
        .goals
        .lock()
        .map_err(|_| AppError::Message("goals lock poisoned".into()))?;
    fs::write(
        goals_path(&state.engine),
        serde_json::to_vec_pretty(&*goals).map_err(|e| AppError::Message(e.to_string()))?,
    )?;
    Ok(())
}

impl From<std::io::Error> for AppError {
    fn from(error: std::io::Error) -> Self {
        Self::Message(error.to_string())
    }
}

fn next_run_at(cadence: &str, now: DateTime<Utc>) -> Option<String> {
    let delta = match cadence.trim().to_ascii_lowercase().as_str() {
        "once" | "manual" | "off" | "disabled" => return None,
        "daily" => Duration::days(1),
        "weekly" => Duration::weeks(1),
        "monthly" => Duration::days(30),
        value
            if value
                .strip_prefix("every:")
                .and_then(|hours| hours.parse::<i64>().ok())
                .is_some() =>
        {
            Duration::hours(
                value
                    .strip_prefix("every:")
                    .and_then(|hours| hours.parse::<i64>().ok())
                    .unwrap_or(24)
                    .max(1),
            )
        }
        _ => Duration::weeks(1),
    };
    Some((now + delta).to_rfc3339())
}

fn cadence_is_valid(cadence: &str) -> bool {
    let value = cadence.trim().to_ascii_lowercase();
    matches!(
        value.as_str(),
        "once" | "manual" | "off" | "disabled" | "daily" | "weekly" | "monthly"
    ) || value
        .strip_prefix("every:")
        .is_some_and(|hours| hours.parse::<i64>().is_ok_and(|hours| hours > 0))
}

fn parse_quiet_time(value: Option<&str>) -> Option<NaiveTime> {
    value.and_then(|value| NaiveTime::parse_from_str(value, "%H:%M").ok())
}

fn quiet_hours_are_valid(input: &GoalInput) -> bool {
    input.quiet_hours_start.is_none() == input.quiet_hours_end.is_none()
        && parse_quiet_time(input.quiet_hours_start.as_deref()).is_some()
            == input.quiet_hours_start.is_some()
        && parse_quiet_time(input.quiet_hours_end.as_deref()).is_some()
            == input.quiet_hours_end.is_some()
}

fn in_quiet_hours(goal: &LearningGoal, now: DateTime<Local>) -> bool {
    let Some(start) = parse_quiet_time(goal.quiet_hours_start.as_deref()) else {
        return false;
    };
    let Some(end) = parse_quiet_time(goal.quiet_hours_end.as_deref()) else {
        return false;
    };
    if start == end {
        return true;
    }
    let current = NaiveTime::from_hms_opt(now.hour(), now.minute(), 0).unwrap_or(start);
    if start < end {
        current >= start && current < end
    } else {
        current >= start || current < end
    }
}

fn validate_goal(input: &GoalInput) -> Result<(), AppError> {
    if input.title.trim().is_empty() {
        return Err(AppError::Message("goal title cannot be empty".into()));
    }
    if input.prompt.trim().is_empty() {
        return Err(AppError::Message("goal prompt cannot be empty".into()));
    }
    if !matches!(input.runtime.as_str(), "claude-code" | "codex" | "cursor") {
        return Err(AppError::Message(
            "runtime must be claude-code, codex, or cursor".into(),
        ));
    }
    if !matches!(
        input.permission_mode.as_str(),
        "plan" | "cautious" | "acceptEdits"
    ) {
        return Err(AppError::Message(
            "permission mode must be plan, cautious, or acceptEdits".into(),
        ));
    }
    if !input.budget_usd.is_finite() || input.budget_usd <= 0.0 {
        return Err(AppError::Message("budget must be greater than zero".into()));
    }
    if !matches!(
        input.review_policy.as_str(),
        "human_required" | "maintainer_questions"
    ) {
        return Err(AppError::Message(
            "review policy must require a human decision".into(),
        ));
    }
    for (label, cadence) in [
        ("learning", &input.cadence),
        ("review", &input.review_cadence),
        ("summary", &input.summary_cadence),
        ("source check", &input.source_check_cadence),
    ] {
        if !cadence_is_valid(cadence) {
            return Err(AppError::Message(format!(
                "{label} cadence must be manual, off, daily, weekly, monthly, or every:N hours"
            )));
        }
    }
    if !quiet_hours_are_valid(input) {
        return Err(AppError::Message(
            "quiet hours must be empty or valid matching HH:MM start/end values".into(),
        ));
    }
    Ok(())
}

fn set_goal_schedules(goal: &mut LearningGoal, now: DateTime<Utc>) {
    if goal.enabled {
        goal.next_run_at = next_run_at(&goal.cadence, now);
        goal.next_review_at = next_run_at(&goal.review_cadence, now);
        goal.next_summary_at = next_run_at(&goal.summary_cadence, now);
        goal.next_source_check_at = next_run_at(&goal.source_check_cadence, now);
    } else {
        goal.next_run_at = None;
        goal.next_review_at = None;
        goal.next_summary_at = None;
        goal.next_source_check_at = None;
    }
}

fn dashboard(state: &AppState) -> Result<Dashboard, AppError> {
    let _ = state.engine.sync_graph()?;
    let nodes = state.engine.list_graph_nodes(None)?;
    let runs = state.engine.list_learning_runs()?;
    let team = state.engine.team_status()?;
    let goals = state
        .goals
        .lock()
        .map_err(|_| AppError::Message("goals lock poisoned".into()))?
        .clone();
    let active_runs = state
        .sessions
        .lock()
        .map_err(|_| AppError::Message("sessions lock poisoned".into()))?
        .iter()
        .map(|(run_id, session)| ActiveRun {
            run_id: run_id.clone(),
            runtime: session.runtime.clone(),
            goal_id: session.goal_id.clone(),
        })
        .collect::<Vec<_>>();
    let review_count = nodes
        .iter()
        .filter(|node| node.status.as_deref() == Some("candidate"))
        .count();
    let stale_count = nodes
        .iter()
        .filter(|node| node.status.as_deref() == Some("stale"))
        .count();
    let mut attentions = state
        .attentions
        .lock()
        .map_err(|_| AppError::Message("attentions lock poisoned".into()))?
        .values()
        .filter(|attention| attention.status == "open")
        .cloned()
        .collect::<Vec<_>>();
    attentions.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(Dashboard {
        home: state.engine.home().display().to_string(),
        nodes,
        runs,
        goals,
        active_runs,
        team: TeamSummary {
            id: team.team_id,
            root: team.root.display().to_string(),
            is_git: team.is_git,
            branch: team.branch,
            dirty: team.dirty,
            changes: team.changes,
        },
        review_count,
        stale_count,
        goal_usage: dashboard_goal_usage(state),
        attentions,
    })
}

fn append_event(engine: &Engine, run_id: &str, event: &RuntimeEvent) {
    match event {
        RuntimeEvent::SessionStarted { session_id } => {
            let _ = engine.record_learning_event(run_id, "runtime", &format!("session started: {session_id}"));
        }
        RuntimeEvent::AssistantText { text } => {
            let _ = engine.record_learning_event(run_id, "assistant", text);
        }
        RuntimeEvent::UserText { text } => {
            let _ = engine.record_learning_event(run_id, "user", text);
        }
        RuntimeEvent::Thinking { text } => {
            let _ = engine.record_learning_event(run_id, "thinking", text);
        }
        RuntimeEvent::ToolCallStarted { name, .. } => {
            let _ = engine.record_learning_event(run_id, "tool", &format!("started {name}"));
        }
        RuntimeEvent::ToolCallCompleted { id, exit_code, .. } => {
            let _ = engine.record_learning_event(
                run_id,
                "tool",
                &format!("completed {id} (exit {exit_code:?})"),
            );
        }
        RuntimeEvent::Result { text, .. } if !text.is_empty() => {
            let _ = engine.record_learning_event(run_id, "runtime", text);
        }
        RuntimeEvent::ApprovalRequested { tool_name, input, .. } => {
            let _ = engine.record_learning_event(run_id, "permission", &format!("permission requested for {tool_name}: {input}"));
        }
        RuntimeEvent::Error { message } => {
            let _ = engine.record_learning_event(run_id, "error", message);
        }
        _ => {}
    }
}

fn start_event_worker(
    state: Arc<AppState>,
    app: AppHandle,
    run_id: String,
    mut events: tokio::sync::mpsc::Receiver<RuntimeEvent>,
) {
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            append_event(&state.engine, &run_id, &event);
            match &event {
                RuntimeEvent::SessionStarted { session_id } => {
                    let _ = state.engine.update_learning_executor_sid(&run_id, session_id);
                }
                RuntimeEvent::AssistantText { text } => {
                    if let Ok(mut sessions) = state.sessions.lock() {
                        if let Some(session) = sessions.get_mut(&run_id) {
                            session.output.push_str(text);
                            session.output.push('\n');
                        }
                    }
                }
                RuntimeEvent::ApprovalRequested { tool_name, input, .. } => {
                    let _ = state.engine.mark_learning_status(&run_id, "awaiting_input");
                    let _ = open_attention(
                        &state,
                        &app,
                        &run_id,
                        "permission",
                        "Runtime permission requested",
                        &format!("Allow the runtime to use {tool_name} for this learning run?"),
                        Some("The runtime paused before a consequential tool action.".into()),
                        Some(tool_name.clone()),
                        Some(input.to_string()),
                    );
                }
                RuntimeEvent::Error { message } => {
                    let _ = state.engine.mark_learning_status(&run_id, "failed");
                    let _ = open_attention(
                        &state,
                        &app,
                        &run_id,
                        "question",
                        "Runtime stopped unexpectedly",
                        "Review the runtime error and decide whether to resume the run.",
                        Some(message.clone()),
                        None,
                        None,
                    );
                    if let Ok(mut sessions) = state.sessions.lock() {
                        sessions.remove(&run_id);
                    }
                }
                RuntimeEvent::Result {
                    is_error,
                    text,
                    cost_usd,
                    usage,
                    session_id,
                    permission_denials,
                } => {
                    if let Some(session_id) = session_id.as_deref() {
                        let _ = state.engine.update_learning_executor_sid(&run_id, session_id);
                    }
                    let run = state
                        .engine
                        .list_learning_runs()
                        .ok()
                        .and_then(|runs| runs.into_iter().find(|run| run.run_id == run_id));
                    let delta = UsageDelta::from_result(*cost_usd, usage.as_ref());
                    if !delta.is_empty() {
                        if let Some(run) = run.as_ref() {
                            let _ = state.engine.store().insert_usage(
                                Some(&run_id),
                                Some(&run_id),
                                Some(&run.runtime),
                                &delta,
                            );
                        }
                        let goal_id = state.sessions.lock().ok().and_then(|sessions| {
                            sessions.get(&run_id).and_then(|session| session.goal_id.clone())
                        });
                        if let Some(goal_id) = goal_id {
                            if let Ok(total) = record_goal_cost(&state, &goal_id, cost_usd.unwrap_or_default()) {
                                let budget = state.goals.lock().ok().and_then(|goals| {
                                    goals.iter().find(|goal| goal.id == goal_id).map(|goal| goal.budget_usd)
                                });
                                if budget.is_some_and(|budget| total >= budget) {
                                    let _ = app.emit("budget-exhausted", serde_json::json!({
                                        "goalId": goal_id,
                                        "spentUsd": total,
                                        "budgetUsd": budget,
                                    }));
                                }
                            }
                        }
                    }
                    let output = state.sessions.lock().ok().and_then(|mut sessions| {
                        sessions.get_mut(&run_id).map(|session| {
                            // Claude's stream emits the final assistant text and
                            // repeats it in the terminal `result` envelope. Keep
                            // one canonical copy for CandidateSet parsing and
                            // persisted run artifacts.
                            if !text.is_empty() && !session.output.ends_with(text) {
                                if !session.output.is_empty() {
                                    session.output.push('\n');
                                }
                                session.output.push_str(text);
                            }
                            session.output.clone()
                        })
                    }).unwrap_or_else(|| text.clone());
                    if let Some(run) = run.as_ref() {
                        if !permission_denials.is_empty() {
                            let denial = permission_denials.first().expect("non-empty permission denials");
                            let _ = state.engine.mark_learning_status(&run_id, "awaiting_input");
                            let _ = open_attention(
                                &state,
                                &app,
                                &run_id,
                                "permission",
                                "Runtime permission denied",
                                &format!("The runtime needs a decision about {} before it can continue.", denial.tool_name),
                                Some("Claude reported a permission denial while the run was unattended.".into()),
                                Some(denial.tool_name.clone()),
                                Some(denial.tool_input.to_string()),
                            );
                        } else if let Some((outcome, question, context, tool_name, tool_input)) = attention::parse_structured(&output) {
                            let _ = state.engine.mark_learning_status(&run_id, "awaiting_input");
                            let kind = if outcome == "permission_required" { "permission" } else { "question" };
                            let title = if kind == "permission" { "Runtime permission requested" } else { "Runtime needs your input" };
                            let _ = open_attention(&state, &app, &run_id, kind, title, &question, context, tool_name, tool_input);
                        } else if output.contains("```") && output.contains("candidates") {
                            match state.engine.record_learning_output_for_run(&run_id, &run.goal, &output) {
                                Ok(candidate_ids) => {
                                    if candidate_ids.is_empty() {
                                        let _ = state.engine.mark_learning_status(&run_id, "awaiting_input");
                                        let _ = open_attention(
                                            &state,
                                            &app,
                                            &run_id,
                                            "question",
                                            "No candidate memory was returned",
                                            "The runtime finished without a durable candidate. Add a narrower direction if this investigation should continue.",
                                            None,
                                            None,
                                            None,
                                        );
                                    } else {
                                        let _ = app.emit(
                                            "review-ready",
                                            serde_json::json!({ "runId": run_id, "count": candidate_ids.len() }),
                                        );
                                    }
                                }
                                Err(error) => {
                                    let _ = state.engine.mark_learning_status(&run_id, "failed");
                                    let _ = open_attention(
                                        &state,
                                        &app,
                                        &run_id,
                                        "question",
                                        "Candidate import failed",
                                        "The runtime returned a synthesis, but Methodus could not stage it for Review.",
                                        Some(error.to_string()),
                                        None,
                                        None,
                                    );
                                }
                            }
                        } else if *is_error {
                            let _ = state.engine.mark_learning_status(&run_id, "failed");
                            let _ = open_attention(
                                &state,
                                &app,
                                &run_id,
                                "question",
                                "Runtime returned an error",
                                "Review the recorded output and decide whether to resume the run.",
                                Some(output.clone()),
                                None,
                                None,
                            );
                        } else {
                            let _ = state.engine.mark_learning_status(&run_id, "awaiting_input");
                            let _ = open_attention(
                                &state,
                                &app,
                                &run_id,
                                "question",
                                "Runtime needs your direction",
                                "The runtime paused without a structured CandidateSet. Add a focused follow-up to continue.",
                                (!output.trim().is_empty()).then_some(output.clone()),
                                None,
                                None,
                            );
                        }
                    }
                    if let Ok(mut sessions) = state.sessions.lock() {
                        sessions.remove(&run_id);
                    }
                    let _ = app.emit("run-updated", run_id.clone());
                }
                _ => {}
            }
            let _ = app.emit(
                "runtime-event",
                serde_json::json!({ "runId": run_id, "event": event }),
            );
        }
    });
}

fn spawn_embedded_terminal(
    state: Arc<AppState>,
    app: AppHandle,
    handoff: methodus_core::NativeLearnHandoff,
) -> Result<(), AppError> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 42,
            cols: 128,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| AppError::Message(format!("open PTY: {error}")))?;
    let mut command = CommandBuilder::new(&handoff.program);
    command.args(&handoff.args);
    command.cwd(&handoff.cwd);
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| AppError::Message(format!("spawn native runtime: {error}")))?;
    drop(pair.slave);
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| AppError::Message(format!("open PTY writer: {error}")))?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| AppError::Message(format!("open PTY reader: {error}")))?;
    let terminal = Arc::new(EmbeddedTerminal {
        writer: Mutex::new(writer),
        child: Mutex::new(child),
        master: Mutex::new(pair.master),
        handoff: handoff.clone(),
    });
    state
        .terminals
        .lock()
        .map_err(|_| AppError::Message("terminals lock poisoned".into()))?
        .insert(handoff.run_id.clone(), terminal);
    let run_id = handoff.run_id.clone();
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        let mut pending_utf8 = Vec::new();
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(size) => {
                    emit_terminal_bytes(&app, &run_id, &mut pending_utf8, &buffer[..size]);
                }
                Err(error) => {
                    let _ = app.emit("terminal-output", serde_json::json!({ "runId": run_id, "text": format!("\n[PTY read error: {error}]\n") }));
                    break;
                }
            }
        }
        if !pending_utf8.is_empty() {
            let text = String::from_utf8_lossy(&pending_utf8).into_owned();
            let _ = app.emit("terminal-output", serde_json::json!({ "runId": run_id, "text": text }));
        }
        let terminal = state
            .terminals
            .lock()
            .ok()
            .and_then(|terminals| terminals.get(&run_id).cloned());
        let status = terminal
            .as_ref()
            .and_then(|terminal| {
                terminal
                    .child
                    .lock()
                    .ok()
                    .and_then(|mut child| child.wait().ok())
            })
            .map(|status| format!("native terminal exited ({status:?})"))
            .unwrap_or_else(|| "native terminal exited".into());
        if let Ok(mut terminals) = state.terminals.lock() {
            terminals.remove(&run_id);
        }
        if let Some(terminal) = terminal {
            let _ = state
                .engine
                .complete_native_learning(&terminal.handoff, &status);
        } else {
            let _ = state.engine.mark_learning_status(&run_id, "awaiting_input");
        }
        let _ = app.emit(
            "terminal-exit",
            serde_json::json!({ "runId": run_id, "status": status }),
        );
    });
    Ok(())
}

/// PTYs are byte streams; a UTF-8 code point may straddle two reads. Keep the
/// incomplete suffix until the next chunk so the optional diagnostics surface
/// never renders replacement glyphs for valid non-ASCII output.
fn emit_terminal_bytes(app: &AppHandle, run_id: &str, pending: &mut Vec<u8>, bytes: &[u8]) {
    pending.extend_from_slice(bytes);
    loop {
        match std::str::from_utf8(pending) {
            Ok(text) => {
                if !text.is_empty() {
                    let _ = app.emit("terminal-output", serde_json::json!({ "runId": run_id, "text": text }));
                }
                pending.clear();
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    let text = String::from_utf8_lossy(&pending[..valid]).into_owned();
                    let _ = app.emit("terminal-output", serde_json::json!({ "runId": run_id, "text": text }));
                    pending.drain(..valid);
                    continue;
                }
                if error.error_len().is_some() {
                    let _ = app.emit("terminal-output", serde_json::json!({ "runId": run_id, "text": "�" }));
                    pending.drain(..1);
                    continue;
                }
                break;
            }
        }
    }
}

async fn launch_learning(
    state: &Arc<AppState>,
    app: &AppHandle,
    goal: String,
    runtime: Option<String>,
    permission_mode: String,
    goal_id: Option<String>,
    sources: &[String],
) -> Result<methodus_core::LearnRun, AppError> {
    let (handle, events) = state
        .engine
        .start_learning_with_sources(runtime.as_deref(), &permission_mode, &goal, sources)
        .await
        .map_err(AppError::from)?;
    let run_id = handle.session_id.clone();
    let persisted_run = state
        .engine
        .list_learning_runs()?
        .into_iter()
        .find(|run| run.run_id == run_id)
        .ok_or_else(|| AppError::Message("runtime started without a persisted Learn run".into()))?;
    let runtime_name = persisted_run.runtime.clone();
    let goal_id_for_link = goal_id.clone();
    state
        .sessions
        .lock()
        .map_err(|_| AppError::Message("sessions lock poisoned".into()))?
        .insert(
            run_id.clone(),
            ActiveSession {
                handle: handle.clone(),
                runtime: runtime_name,
                goal_id,
                output: String::new(),
            },
        );
    if let Some(goal_id) = goal_id_for_link {
        if let Ok(mut links) = state.run_goal_links.lock() {
            links.insert(run_id.clone(), goal_id);
        }
        let _ = save_run_goal_links(state);
    }
    state.engine.record_learning_event(&run_id, "user", &goal)?;
    start_event_worker(state.clone(), app.clone(), run_id.clone(), events);
    Ok(persisted_run)
}

#[tauri::command]
fn get_dashboard(state: State<'_, Arc<AppState>>) -> Result<Dashboard, AppError> {
    dashboard(&state)
}

#[tauri::command]
fn list_goals(state: State<'_, Arc<AppState>>) -> Result<Vec<LearningGoal>, AppError> {
    Ok(state
        .goals
        .lock()
        .map_err(|_| AppError::Message("goals lock poisoned".into()))?
        .clone())
}

#[tauri::command]
fn save_goal(state: State<'_, Arc<AppState>>, input: GoalInput) -> Result<LearningGoal, AppError> {
    validate_goal(&input)?;
    let now = Utc::now().to_rfc3339();
    let sources = input
        .sources
        .into_iter()
        .map(|source| source.trim().to_string())
        .filter(|source| !source.is_empty())
        .collect::<Vec<_>>();
    let goal = LearningGoal {
        id: format!("goal_{}", Uuid::new_v4()),
        title: input.title.trim().into(),
        prompt: input.prompt.trim().into(),
        sources,
        runtime: input.runtime,
        permission_mode: input.permission_mode,
        cadence: input.cadence,
        review_cadence: input.review_cadence,
        summary_cadence: input.summary_cadence,
        source_check_cadence: input.source_check_cadence,
        quiet_hours_start: input.quiet_hours_start,
        quiet_hours_end: input.quiet_hours_end,
        budget_usd: input.budget_usd,
        review_policy: input.review_policy,
        enabled: input.enabled,
        next_run_at: None,
        next_review_at: None,
        next_summary_at: None,
        next_source_check_at: None,
        created_at: now.clone(),
        updated_at: now,
    };
    let mut goal = goal;
    set_goal_schedules(&mut goal, Utc::now());
    state
        .goals
        .lock()
        .map_err(|_| AppError::Message("goals lock poisoned".into()))?
        .push(goal.clone());
    save_goals(&state)?;
    Ok(goal)
}

#[tauri::command]
fn update_goal(
    state: State<'_, Arc<AppState>>,
    goal_id: String,
    input: GoalInput,
) -> Result<LearningGoal, AppError> {
    validate_goal(&input)?;
    let sources = input
        .sources
        .into_iter()
        .map(|source| source.trim().to_string())
        .filter(|source| !source.is_empty())
        .collect::<Vec<_>>();
    let mut goals = state
        .goals
        .lock()
        .map_err(|_| AppError::Message("goals lock poisoned".into()))?;
    let goal = goals
        .iter_mut()
        .find(|goal| goal.id == goal_id)
        .ok_or_else(|| AppError::Message(format!("goal not found: {goal_id}")))?;
    goal.title = input.title.trim().into();
    goal.prompt = input.prompt.trim().into();
    goal.sources = sources;
    goal.runtime = input.runtime;
    goal.permission_mode = input.permission_mode;
    goal.cadence = input.cadence;
    goal.review_cadence = input.review_cadence;
    goal.summary_cadence = input.summary_cadence;
    goal.source_check_cadence = input.source_check_cadence;
    goal.quiet_hours_start = input.quiet_hours_start;
    goal.quiet_hours_end = input.quiet_hours_end;
    goal.budget_usd = input.budget_usd;
    goal.review_policy = input.review_policy;
    goal.enabled = input.enabled;
    goal.updated_at = Utc::now().to_rfc3339();
    set_goal_schedules(goal, Utc::now());
    let result = goal.clone();
    drop(goals);
    save_goals(&state)?;
    Ok(result)
}

#[tauri::command]
fn set_goal_enabled(
    state: State<'_, Arc<AppState>>,
    goal_id: String,
    enabled: bool,
) -> Result<Vec<LearningGoal>, AppError> {
    let mut goals = state
        .goals
        .lock()
        .map_err(|_| AppError::Message("goals lock poisoned".into()))?;
    let goal = goals
        .iter_mut()
        .find(|goal| goal.id == goal_id)
        .ok_or_else(|| AppError::Message(format!("goal not found: {goal_id}")))?;
    goal.enabled = enabled;
    set_goal_schedules(goal, Utc::now());
    goal.updated_at = Utc::now().to_rfc3339();
    let result = goals.clone();
    drop(goals);
    save_goals(&state)?;
    Ok(result)
}

#[tauri::command]
fn delete_goal(
    state: State<'_, Arc<AppState>>,
    goal_id: String,
) -> Result<Vec<LearningGoal>, AppError> {
    let active = state
        .sessions
        .lock()
        .map_err(|_| AppError::Message("sessions lock poisoned".into()))?
        .values()
        .any(|session| session.goal_id.as_deref() == Some(&goal_id));
    if active {
        return Err(AppError::Message(
            "pause or stop the goal's active run before deleting it".into(),
        ));
    }
    let mut goals = state
        .goals
        .lock()
        .map_err(|_| AppError::Message("goals lock poisoned".into()))?;
    let before = goals.len();
    goals.retain(|goal| goal.id != goal_id);
    if goals.len() == before {
        return Err(AppError::Message(format!("goal not found: {goal_id}")));
    }
    let result = goals.clone();
    drop(goals);
    save_goals(&state)?;
    Ok(result)
}

#[tauri::command]
async fn start_learning(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    goal: String,
    runtime: Option<String>,
    permission_mode: Option<String>,
) -> Result<methodus_core::LearnRun, AppError> {
    launch_learning(
        &state,
        &app,
        goal,
        runtime,
        permission_mode.unwrap_or_else(default_permission),
        None,
        &[],
    )
    .await
}

#[tauri::command]
async fn open_embedded_learning(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    run_id: String,
    prompt: String,
) -> Result<(), AppError> {
    if prompt.trim().is_empty() {
        return Err(AppError::Message(
            "native terminal prompt cannot be empty".into(),
        ));
    }
    if state
        .terminals
        .lock()
        .map_err(|_| AppError::Message("terminals lock poisoned".into()))?
        .contains_key(&run_id)
    {
        return Err(AppError::Message(
            "this run already has an embedded terminal".into(),
        ));
    }
    let background = state
        .sessions
        .lock()
        .map_err(|_| AppError::Message("sessions lock poisoned".into()))?
        .remove(&run_id);
    if let Some(session) = background {
        if let Err(error) = state
            .engine
            .stop_learning(&session.runtime, &session.handle)
            .await
        {
            state
                .sessions
                .lock()
                .map_err(|_| AppError::Message("sessions lock poisoned".into()))?
                .insert(run_id.clone(), session);
            return Err(error.into());
        }
    }
    let run = state
        .engine
        .list_learning_runs()?
        .into_iter()
        .find(|run| run.run_id == run_id)
        .ok_or_else(|| AppError::Message(format!("run not found: {run_id}")))?;
    let handoff = state.engine.continue_native_learning(
        &run.runtime,
        &run.permission_mode,
        &run.run_id,
        run.executor_sid.as_deref(),
        &prompt,
    )?;
    spawn_embedded_terminal(state.inner().clone(), app, handoff)
}

#[tauri::command]
fn write_embedded_input(
    state: State<'_, Arc<AppState>>,
    run_id: String,
    input: String,
) -> Result<(), AppError> {
    let terminal = state
        .terminals
        .lock()
        .map_err(|_| AppError::Message("terminals lock poisoned".into()))?
        .get(&run_id)
        .cloned()
        .ok_or_else(|| AppError::Message("embedded terminal is not active".into()))?;
    let mut writer = terminal
        .writer
        .lock()
        .map_err(|_| AppError::Message("PTY writer lock poisoned".into()))?;
    writer.write_all(input.as_bytes())?;
    writer.flush()?;
    Ok(())
}

#[tauri::command]
fn resize_embedded_terminal(
    state: State<'_, Arc<AppState>>,
    run_id: String,
    rows: u16,
    cols: u16,
) -> Result<(), AppError> {
    let terminal = state
        .terminals
        .lock()
        .map_err(|_| AppError::Message("terminals lock poisoned".into()))?
        .get(&run_id)
        .cloned()
        .ok_or_else(|| AppError::Message("embedded terminal is not active".into()))?;
    terminal
        .master
        .lock()
        .map_err(|_| AppError::Message("PTY master lock poisoned".into()))?
        .resize(PtySize {
            rows: rows.max(2),
            cols: cols.max(20),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| AppError::Message(format!("resize PTY: {error}")))?;
    Ok(())
}

#[tauri::command]
fn stop_embedded_learning(state: State<'_, Arc<AppState>>, run_id: String) -> Result<(), AppError> {
    let terminal = state
        .terminals
        .lock()
        .map_err(|_| AppError::Message("terminals lock poisoned".into()))?
        .remove(&run_id)
        .ok_or_else(|| AppError::Message("embedded terminal is not active".into()))?;
    terminal
        .child
        .lock()
        .map_err(|_| AppError::Message("PTY child lock poisoned".into()))?
        .kill()
        .map_err(|error| AppError::Message(format!("stop PTY: {error}")))?;
    state
        .engine
        .mark_learning_status(&run_id, "awaiting_input")?;
    Ok(())
}

#[tauri::command]
async fn run_goal(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    goal_id: String,
) -> Result<Dashboard, AppError> {
    let goal = state
        .goals
        .lock()
        .map_err(|_| AppError::Message("goals lock poisoned".into()))?
        .iter()
        .find(|goal| goal.id == goal_id)
        .cloned()
        .ok_or_else(|| AppError::Message(format!("goal not found: {goal_id}")))?;
    if goal_budget_exhausted(&state, &goal) {
        return Err(AppError::Message(format!(
            "goal monthly budget of ${:.2} has been reached",
            goal.budget_usd
        )));
    }
    let _ = launch_learning(
        &state,
        &app,
        goal_prompt_for(&goal, "learn"),
        Some(goal.runtime.clone()),
        goal.permission_mode.clone(),
        Some(goal.id.clone()),
        &goal.sources,
    )
    .await?;
    dashboard(&state)
}

#[tauri::command]
async fn continue_learning(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    run_id: String,
    prompt: String,
    attention_id: Option<String>,
) -> Result<methodus_core::LearnRun, AppError> {
    if prompt.trim().is_empty() {
        return Err(AppError::Message("follow-up cannot be empty".into()));
    }
    if let Some(attention_id) = attention_id.as_deref() {
        let valid = state
            .attentions
            .lock()
            .map_err(|_| AppError::Message("attentions lock poisoned".into()))?
            .get(attention_id)
            .is_some_and(|attention| attention.run_id == run_id && attention.status == "open");
        if !valid {
            return Err(AppError::Message("attention is no longer open for this run".into()));
        }
    }
    if state
        .sessions
        .lock()
        .map_err(|_| AppError::Message("sessions lock poisoned".into()))?
        .contains_key(&run_id)
    {
        return Err(AppError::Message("this Learn run is still active".into()));
    }
    let run = state
        .engine
        .list_learning_runs()?
        .into_iter()
        .find(|run| run.run_id == run_id)
        .ok_or_else(|| AppError::Message(format!("run not found: {run_id}")))?;
    let sid = run
        .executor_sid
        .ok_or_else(|| AppError::Message("run has no resumable executor session".into()))?;
    let (handle, events) = state
        .engine
        .continue_learning(&run.runtime, &run.permission_mode, &sid, &run_id, &prompt)
        .await
        .map_err(AppError::from)?;
    let goal_id = state
        .run_goal_links
        .lock()
        .ok()
        .and_then(|links| links.get(&run_id).cloned());
    state
        .sessions
        .lock()
        .map_err(|_| AppError::Message("sessions lock poisoned".into()))?
        .insert(
            run_id.clone(),
            ActiveSession {
                handle,
                runtime: run.runtime.clone(),
                goal_id,
                output: String::new(),
            },
        );
    state
        .engine
        .record_learning_event(&run_id, "user", &prompt)?;
    resolve_attention_for_run(state.inner(), &run_id, &prompt);
    start_event_worker(state.inner().clone(), app, run_id.clone(), events);
    state
        .engine
        .list_learning_runs()?
        .into_iter()
        .find(|item| item.run_id == run_id)
        .ok_or_else(|| AppError::Message("run disappeared after resume".into()))
}

#[tauri::command]
async fn stop_learning(
    state: State<'_, Arc<AppState>>,
    run_id: String,
) -> Result<Dashboard, AppError> {
    let session = state
        .sessions
        .lock()
        .map_err(|_| AppError::Message("sessions lock poisoned".into()))?
        .remove(&run_id)
        .ok_or_else(|| AppError::Message("this Learn run is not active".into()))?;
    state
        .engine
        .stop_learning(&session.runtime, &session.handle)
        .await?;
    dashboard(&state)
}

#[tauri::command]
fn get_run(state: State<'_, Arc<AppState>>, run_id: String) -> Result<RunDetails, AppError> {
    let run = state
        .engine
        .list_learning_runs()?
        .into_iter()
        .find(|run| run.run_id == run_id)
        .ok_or_else(|| AppError::Message(format!("run not found: {run_id}")))?;
    Ok(RunDetails {
        events: state.engine.learning_events(&run_id)?,
        attention: attention_for_run(&state, &run_id),
        run,
    })
}

#[tauri::command]
fn review_candidate(
    state: State<'_, Arc<AppState>>,
    node_id: String,
    action: String,
    target_id: Option<String>,
    rationale: Option<String>,
) -> Result<Dashboard, AppError> {
    let rationale = rationale.unwrap_or_else(|| "reviewed in Methodus Desktop".into());
    match action.as_str() {
        "approve" => state.engine.promote_graph_candidate(&node_id)?,
        "team" => state.engine.promote_candidate_to_team(&node_id)?,
        "reject" => state.engine.reject_graph_candidate(&node_id)?,
        "revalidate" => state.engine.revalidate_graph_node(&node_id, &rationale)?,
        "merge" => state.engine.merge_graph_candidate(
            &node_id,
            target_id
                .as_deref()
                .ok_or_else(|| AppError::Message("merge requires target_id".into()))?,
        )?,
        _ => {
            return Err(AppError::Message(format!(
                "unsupported review action: {action}"
            )))
        }
    }
    dashboard(&state)
}

#[tauri::command]
fn get_node(state: State<'_, Arc<AppState>>, node_id: String) -> Result<NodeDetails, AppError> {
    let document = state.engine.graph_document(&node_id)?;
    let run_id = document.sources.iter().find_map(|source| {
        let path = source.path.replace('\\', "/");
        let suffix = path
            .strip_prefix("runs/")
            .or_else(|| path.split_once("/runs/").map(|(_, suffix)| suffix))?;
        suffix
            .split('/')
            .next()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    });
    let revisions = state
        .engine
        .list_graph_nodes(Some(&document.node.title))?
        .into_iter()
        .filter(|node| node.id != node_id && node.title == document.node.title)
        .filter_map(|node| {
            let revision = state.engine.graph_document(&node.id).ok()?;
            Some(RevisionPreview {
                id: node.id,
                path: node.path,
                status: node.status,
                content: revision.body.chars().take(8_000).collect(),
            })
        })
        .collect();
    Ok(NodeDetails {
        node: document.node.clone(),
        edges: state.engine.graph_edges_for(&node_id)?,
        kind: document.kind,
        content: document.body,
        sources: document.sources,
        run_id,
        revisions,
    })
}

#[tauri::command]
fn sync_graph(state: State<'_, Arc<AppState>>) -> Result<Dashboard, AppError> {
    dashboard(&state)
}

fn is_due(value: Option<&str>, now: DateTime<Utc>) -> bool {
    value
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .is_some_and(|at| at.with_timezone(&Utc) <= now)
}

async fn run_due_goals(state: Arc<AppState>, app: AppHandle) {
    #[derive(Clone, Copy)]
    enum WorkKind {
        Learn,
        Review,
        Summary,
    }

    let (due, source_checks) = {
        let occupied_goal_ids = state
            .sessions
            .lock()
            .ok()
            .map(|sessions| {
                sessions
                    .values()
                    .filter_map(|session| session.goal_id.clone())
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        let attention_goal_ids = {
            let links = state.run_goal_links.lock().ok();
            state
                .attentions
                .lock()
                .ok()
                .map(|attentions| {
                    attentions
                        .values()
                        .filter(|attention| attention.status == "open")
                        .filter_map(|attention| links.as_ref().and_then(|links| links.get(&attention.run_id)))
                        .cloned()
                        .collect::<HashSet<_>>()
                })
                .unwrap_or_default()
        };
        let mut goals = match state.goals.lock() {
            Ok(goals) => goals,
            Err(_) => return,
        };
        let now = Utc::now();
        let mut due = Vec::new();
        let mut source_checks = false;
        for goal in goals.iter_mut().filter(|goal| goal.enabled) {
            if in_quiet_hours(goal, Local::now()) {
                continue;
            }
            let mut advanced = false;
            // A Goal owns one executor session at a time. Select one due turn and
            // leave other due cadences untouched so they retry after this run.
            if !occupied_goal_ids.contains(&goal.id) && !attention_goal_ids.contains(&goal.id) {
                if is_due(goal.next_run_at.as_deref(), now) {
                    goal.next_run_at = next_run_at(&goal.cadence, now);
                    due.push((goal.clone(), WorkKind::Learn));
                    advanced = true;
                } else if is_due(goal.next_review_at.as_deref(), now) {
                    goal.next_review_at = next_run_at(&goal.review_cadence, now);
                    due.push((goal.clone(), WorkKind::Review));
                    advanced = true;
                } else if is_due(goal.next_summary_at.as_deref(), now) {
                    goal.next_summary_at = next_run_at(&goal.summary_cadence, now);
                    due.push((goal.clone(), WorkKind::Summary));
                    advanced = true;
                }
            }
            if is_due(goal.next_source_check_at.as_deref(), now) {
                goal.next_source_check_at = next_run_at(&goal.source_check_cadence, now);
                source_checks = true;
                advanced = true;
            }
            if advanced {
                goal.updated_at = now.to_rfc3339();
            }
        }
        (due, source_checks)
    };
    if due.is_empty() && !source_checks {
        return;
    }
    let _ = save_goals(&state);
    if source_checks {
        let _ = state.engine.sync_graph();
        if let Ok(nodes) = state.engine.list_graph_nodes(None) {
            let stale = nodes
                .into_iter()
                .filter(|node| node.status.as_deref() == Some("stale"))
                .map(|node| node.id)
                .collect::<HashSet<_>>();
            if let Ok(mut notified) = state.stale_notices.lock() {
                let newly_stale = stale.difference(&*notified).cloned().collect::<Vec<_>>();
                *notified = stale;
                if !newly_stale.is_empty() {
                    let _ = app.emit(
                        "source-stale",
                        serde_json::json!({ "nodeIds": newly_stale, "count": newly_stale.len() }),
                    );
                }
            }
        }
    }
    for (goal, work) in due {
        if goal_budget_exhausted(&state, &goal) {
            let _ = app.emit(
                "budget-exhausted",
                serde_json::json!({ "goalId": goal.id, "spentUsd": dashboard_goal_usage(&state).get(&goal.id).copied().unwrap_or_default(), "budgetUsd": goal.budget_usd }),
            );
            continue;
        }
        let occupied = state
            .sessions
            .lock()
            .ok()
            .map(|sessions| {
                sessions
                    .values()
                    .any(|session| session.goal_id.as_deref() == Some(&goal.id))
            })
            .unwrap_or(true);
        if occupied {
            continue;
        }
        if let Err(error) = launch_learning(
            &state,
            &app,
            goal_prompt_for(
                &goal,
                match work {
                    WorkKind::Learn => "learn",
                    WorkKind::Review => "review",
                    WorkKind::Summary => "summary",
                },
            ),
            Some(goal.runtime.clone()),
            goal.permission_mode.clone(),
            Some(goal.id.clone()),
            &goal.sources,
        )
        .await
        {
            let _ = app.emit(
                "scheduler-error",
                serde_json::json!({ "goalId": goal.id, "message": error.to_string() }),
            );
        }
    }
}

fn build_state() -> Result<Arc<AppState>, AppError> {
    let home = methodus_home()?;
    ensure_home(&home)?;
    let store = Arc::new(Store::open(&home.join("state.db"))?);
    let mut adapters: HashMap<String, Arc<dyn RuntimeAdapter>> = HashMap::new();
    adapters.insert("claude-code".into(), Arc::new(ClaudeCodeAdapter::new()));
    adapters.insert("codex".into(), Arc::new(CodexAdapter::new()));
    adapters.insert("cursor".into(), Arc::new(CursorAdapter::new()));
    let engine = Engine::with_runtimes(store, home.clone(), adapters);
    let _ = engine.recover_pending_native_learning();
    // A desktop process owns its child runtimes. After a restart those
    // processes are gone, so make the persisted state explicit instead of
    // presenting phantom "running" sessions in Today. The executor id remains
    // intact and the run can still be resumed from the app.
    for run in engine.list_learning_runs()? {
        if run.status == "running" {
            let _ = engine.mark_learning_status(&run.run_id, "disconnected");
            let _ = engine.record_learning_event(
                &run.run_id,
                "methodus",
                "Methodus restarted; the previous runtime process is detached and can be resumed.",
            );
        }
    }
    Ok(Arc::new(AppState {
        engine,
        sessions: Mutex::new(HashMap::new()),
        terminals: Mutex::new(HashMap::new()),
        attentions: Mutex::new(load_attentions(&home)),
        goals: Mutex::new(load_goals(&home)),
        stale_notices: Mutex::new(HashSet::new()),
        goal_usage: Mutex::new(load_goal_usage(&home)),
        run_goal_links: Mutex::new(load_run_goal_links(&home)),
    }))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = build_state().expect("failed to initialize Methodus home");
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .manage(state)
        .setup(|app| {
            let show = MenuItem::with_id(app, "show", "Open Methodus", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Methodus", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))?;
            TrayIconBuilder::with_id("methodus-tray")
                .icon(icon)
                .icon_as_template(false)
                .menu(&menu)
                .tooltip("Methodus continuous learning")
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            let state = app.state::<Arc<AppState>>().inner().clone();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    sleep(TokioDuration::from_secs(30)).await;
                    run_due_goals(state.clone(), handle.clone()).await;
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_dashboard,
            list_goals,
            save_goal,
            update_goal,
            set_goal_enabled,
            delete_goal,
            start_learning,
            open_embedded_learning,
            write_embedded_input,
            resize_embedded_terminal,
            stop_embedded_learning,
            run_goal,
            continue_learning,
            stop_learning,
            get_run,
            review_candidate,
            get_node,
            sync_graph
        ])
        .run(tauri::generate_context!())
        .expect("error while running Methodus Desktop");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_goal() -> GoalInput {
        GoalInput {
            title: "Shutdown recovery".into(),
            prompt: "Understand the recovery path".into(),
            sources: vec!["docs/runbook".into()],
            runtime: default_runtime(),
            permission_mode: default_permission(),
            cadence: default_cadence(),
            review_cadence: default_review_cadence(),
            summary_cadence: default_summary_cadence(),
            source_check_cadence: default_source_check_cadence(),
            quiet_hours_start: None,
            quiet_hours_end: None,
            budget_usd: default_budget_usd(),
            review_policy: default_review_policy(),
            enabled: true,
        }
    }

    #[test]
    fn cadence_is_deterministic_and_manual_goals_do_not_schedule() {
        let now = DateTime::parse_from_rfc3339("2026-08-21T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            next_run_at("daily", now).as_deref(),
            Some("2026-08-22T00:00:00+00:00")
        );
        assert_eq!(
            next_run_at("every:6", now).as_deref(),
            Some("2026-08-21T06:00:00+00:00")
        );
        assert!(next_run_at("manual", now).is_none());
    }

    #[test]
    fn goal_validation_rejects_unsafe_or_unbounded_policy() {
        assert!(validate_goal(&valid_goal()).is_ok());

        let mut invalid = valid_goal();
        invalid.budget_usd = 0.0;
        assert!(validate_goal(&invalid).is_err());

        let mut invalid = valid_goal();
        invalid.review_policy = "auto_publish".into();
        assert!(validate_goal(&invalid).is_err());

        let mut invalid = valid_goal();
        invalid.quiet_hours_start = Some("23:00".into());
        invalid.quiet_hours_end = None;
        assert!(!quiet_hours_are_valid(&invalid));

        let mut invalid = valid_goal();
        invalid.quiet_hours_start = Some("25:00".into());
        invalid.quiet_hours_end = Some("07:00".into());
        assert!(validate_goal(&invalid).is_err());
    }

    #[test]
    fn scheduled_prompt_keeps_sources_and_review_gate_explicit() {
        let mut goal = LearningGoal {
            id: "goal_test".into(),
            title: "Test".into(),
            prompt: "Investigate recovery".into(),
            sources: vec!["docs/runbook".into(), "https://example.test".into()],
            runtime: default_runtime(),
            permission_mode: default_permission(),
            cadence: default_cadence(),
            review_cadence: default_review_cadence(),
            summary_cadence: default_summary_cadence(),
            source_check_cadence: default_source_check_cadence(),
            quiet_hours_start: None,
            quiet_hours_end: None,
            budget_usd: 12.5,
            review_policy: default_review_policy(),
            enabled: true,
            next_run_at: None,
            next_review_at: None,
            next_summary_at: None,
            next_source_check_at: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let prompt = goal_prompt_for(&goal, "learn");
        assert!(prompt.contains("@docs/runbook"));
        assert!(prompt.contains("@https://example.test"));
        assert!(prompt.contains("$12.50"));
        assert!(prompt.contains("never publish canonical knowledge"));
        assert!(goal_prompt_for(&goal, "review").contains("Scheduled review"));
        assert!(goal_prompt_for(&goal, "summary").contains("Scheduled synthesis"));
        goal.sources.clear();
        assert!(goal_prompt_for(&goal, "learn").contains("none specified"));
    }

    #[test]
    fn structured_attention_requires_a_focused_question() {
        let output = "Before I continue, I need a decision.\n```json\n{\"outcome\":\"needs_input\",\"question\":\"Should the recovery path prefer a bounded retry?\",\"context\":\"The source leaves both policies possible.\"}\n```";
        let parsed = attention::parse_structured(output).expect("attention envelope");
        assert_eq!(parsed.0, "needs_input");
        assert_eq!(parsed.1, "Should the recovery path prefer a bounded retry?");
        assert_eq!(parsed.2.as_deref(), Some("The source leaves both policies possible."));
        assert!(attention::parse_structured("```json\n{\"candidates\":[]}\n```").is_none());
    }

    #[test]
    fn permission_attention_preserves_tool_context() {
        let parsed = attention::parse_structured(r#"{"outcome":"permission_required","question":"May I inspect the generated trace?","tool_name":"Read","tool_input":{"path":"trace.log"}}"#).expect("permission envelope");
        assert_eq!(parsed.0, "permission_required");
        assert_eq!(parsed.3.as_deref(), Some("Read"));
        assert_eq!(parsed.4.as_deref(), Some("{\"path\":\"trace.log\"}"));
    }
}
