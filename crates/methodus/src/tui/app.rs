use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use methodus_core::{
    at_query, filter_candidates, health_checks, list_from_roots, list_packs, list_projects, Engine,
    FaceSummary, HealthCheck, HypothesisReviewAction, KnowledgeReviewAction, MentionCandidate,
    PackInfo, ProjectInfo, RecoveredSession, UserConfig,
};
use methodus_domain::{
    Approval, ApprovalDecision, EvolutionCandidate, EvolutionStatus, Experience, Hypothesis,
    HypothesisStatus, KnowledgeItem, KnowledgeStatus, Question, QuestionStatus, RuntimeEvent, Task,
    TaskStatus, UsageDelta, UsageSummary,
};
use tokio::sync::mpsc;

use crate::notify::NotifyUrgency;

use super::fuzzy::fuzzy_score;
use super::util::summarize_tool_json;

const NOTIFY_DEDUP_TTL: Duration = Duration::from_secs(30);
const TERMINAL_ENGAGED: Duration = Duration::from_secs(30);

/// Full-screen takeover on top of the session. None = the daily driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Setup,
    Inbox,
    Sessions,
    Faces,
    Events,
    Jobs,
}

impl Overlay {
    pub fn is_open(self) -> bool {
        self != Overlay::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Answering,
    ConfirmCancel,
    Prompt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupSection {
    Settings,
    Projects,
    Packs,
}

impl SetupSection {
    fn next(self) -> Self {
        match self {
            Self::Settings => Self::Projects,
            Self::Projects => Self::Packs,
            Self::Packs => Self::Settings,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Settings => Self::Packs,
            Self::Projects => Self::Settings,
            Self::Packs => Self::Projects,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    AddProject,
    AddPack,
    WorkspaceRoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatKind {
    You,
    Assistant,
    Tool,
    Meta,
    Alert,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLine {
    pub kind: ChatKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLevel {
    Info,
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub enum Command {
    None,
    Quit,
    Send {
        task_id: Option<String>,
        text: String,
    },
    Run {
        task_id: String,
        resume: bool,
    },
    Approve {
        id: String,
        decision: ApprovalDecision,
    },
    Cancel {
        task_id: String,
    },
    Delete {
        task_id: String,
    },
    ReviewKnowledge {
        id: String,
        action: KnowledgeReviewAction,
    },
    ReviewEvolution {
        id: String,
        approve: bool,
    },
    AnswerQuestion {
        id: String,
        text: String,
    },
    SnoozeQuestion {
        id: String,
    },
    DismissQuestion {
        id: String,
    },
    CompleteReview {
        task_id: String,
    },
    ReviewHypothesis {
        id: String,
        action: HypothesisReviewAction,
    },
    IngestDocs {
        sources: Vec<String>,
    },
    SurveyRepo,
    CleanupWorkspaces {
        max_age_days: i64,
    },
    CancelLearningJob {
        id: String,
    },
    StudyModule {
        scope: String,
        sources: Vec<String>,
        face: Option<String>,
    },
}

pub struct App {
    pub engine: Engine,
    pub overlay: Overlay,
    pub mode: Mode,
    pub show_help: bool,
    pub should_quit: bool,
    pub status: String,
    pub status_level: StatusLevel,
    pub input: String,
    pub input_cursor: usize,
    pub input_error: Option<String>,
    pub overlay_filter: String,
    pub runtime: Option<String>,
    pub default_face: Option<String>,
    pub context_faces: Vec<String>,
    pub system_events: Vec<methodus_store::EventRecord>,
    pub learning_jobs: Vec<methodus_domain::LearningJob>,
    pub system_sel: usize,
    pub permission_mode: String,
    pub workspace_root: String,
    pub notifications: bool,
    pub tasks: Vec<Task>,
    pub approvals: Vec<Approval>,
    pub faces: Vec<FaceSummary>,
    pub recovered: Vec<RecoveredSession>,
    pub task_sel: usize,
    pub approval_sel: usize,
    pub face_sel: usize,
    pub review_sel: usize,
    pub review_detail_scroll: usize,
    /// Inbox list + summary vs full-page detail with composer actions.
    pub inbox_detail: bool,
    pub inbox_menu_choice: usize,
    pub answering_id: Option<String>,
    pub confirm_task_id: Option<String>,
    pub confirm_delete: bool,
    pub questions: Vec<Question>,
    pub knowledge: Vec<KnowledgeItem>,
    pub hypotheses: Vec<Hypothesis>,
    pub evolutions: Vec<EvolutionCandidate>,
    pub experiences: Vec<Experience>,
    pub transcript: Vec<ChatLine>,
    pub transcript_offset: usize,
    pub session_task_id: Option<String>,
    pub tick: u8,
    pub event_rx: Option<mpsc::Receiver<RuntimeEvent>>,
    pub setup_section: SetupSection,
    pub setup_sel: usize,
    pub packs: Vec<PackInfo>,
    pub projects: Vec<ProjectInfo>,
    pub health: Vec<HealthCheck>,
    pub usage_today: UsageSummary,
    pub usage_all: UsageSummary,
    pub prompt_kind: Option<PromptKind>,
    pub slash_sel: usize,
    pub mention_sel: usize,
    pub approval_choice: usize,
    pub knowledge_pick_id: Option<String>,
    pub knowledge_choice: usize,
    pub evolution_pick_id: Option<String>,
    pub evolution_choice: usize,
    pub hypothesis_pick_id: Option<String>,
    pub hypothesis_choice: usize,
    mention_cache: Vec<MentionCandidate>,
    mention_root_id: Option<String>,
    notify_dedup: HashMap<String, Instant>,
    notified_knowledge: std::collections::HashSet<String>,
    knowledge_notify_ready: bool,
    last_user_input: Instant,
    last_activity: Instant,
    last_idle_ask: Option<Instant>,
    pending_quit_at: Option<Instant>,
}

impl App {
    pub fn new(engine: Engine, recovered: Vec<RecoveredSession>) -> Self {
        let n = recovered.len();
        let (status, status_level) = if n == 0 {
            (
                "type a message, Enter to send".to_string(),
                StatusLevel::Info,
            )
        } else {
            (
                format!("recovered {n} session(s) — Enter to continue, R to resume"),
                StatusLevel::Warn,
            )
        };
        let cfg = UserConfig::load(engine.home());
        Self {
            engine,
            overlay: Overlay::None,
            mode: Mode::Normal,
            show_help: false,
            should_quit: false,
            status,
            status_level,
            input: String::new(),
            input_cursor: 0,
            input_error: None,
            overlay_filter: String::new(),
            runtime: Some(
                cfg.default_runtime
                    .clone()
                    .unwrap_or_else(|| "claude-code".to_string()),
            ),
            default_face: cfg.default_face.clone(),
            context_faces: cfg.context_faces.clone().unwrap_or_default(),
            system_events: Vec::new(),
            learning_jobs: Vec::new(),
            system_sel: 0,
            permission_mode: cfg
                .permission_mode
                .clone()
                .unwrap_or_else(|| "acceptEdits".to_string()),
            workspace_root: cfg.workspace_root.clone().unwrap_or_default(),
            notifications: cfg.notifications_enabled(),
            tasks: Vec::new(),
            approvals: Vec::new(),
            faces: Vec::new(),
            recovered,
            task_sel: 0,
            approval_sel: 0,
            face_sel: 0,
            review_sel: 0,
            review_detail_scroll: 0,
            inbox_detail: false,
            inbox_menu_choice: 0,
            answering_id: None,
            confirm_task_id: None,
            confirm_delete: false,
            questions: Vec::new(),
            knowledge: Vec::new(),
            hypotheses: Vec::new(),
            evolutions: Vec::new(),
            experiences: Vec::new(),
            transcript: Vec::new(),
            transcript_offset: 0,
            session_task_id: None,
            tick: 0,
            event_rx: None,
            setup_section: SetupSection::Settings,
            setup_sel: 0,
            packs: Vec::new(),
            projects: Vec::new(),
            health: Vec::new(),
            usage_today: UsageSummary::default(),
            usage_all: UsageSummary::default(),
            prompt_kind: None,
            slash_sel: 0,
            mention_sel: 0,
            approval_choice: 0,
            knowledge_pick_id: None,
            knowledge_choice: 0,
            evolution_pick_id: None,
            evolution_choice: 0,
            hypothesis_pick_id: None,
            hypothesis_choice: 0,
            mention_cache: Vec::new(),
            mention_root_id: None,
            notify_dedup: HashMap::new(),
            notified_knowledge: std::collections::HashSet::new(),
            knowledge_notify_ready: false,
            last_user_input: Instant::now(),
            last_activity: Instant::now(),
            last_idle_ask: None,
            pending_quit_at: None,
        }
    }

    fn handle_ctrl_c(&mut self) -> Command {
        if !self.input.is_empty() {
            self.input.clear();
            self.input_error = None;
            self.pending_quit_at = None;
            self.set_status(StatusLevel::Info, "cleared");
            return Command::None;
        }
        let now = Instant::now();
        if ctrl_c_should_quit(self.pending_quit_at, now) {
            return Command::Quit;
        }
        self.pending_quit_at = Some(now);
        if self.show_help {
            self.show_help = false;
        }
        if self.mode != Mode::Normal {
            self.mode = Mode::Normal;
            self.prompt_kind = None;
            self.answering_id = None;
            self.confirm_task_id = None;
            self.confirm_delete = false;
        }
        self.set_status(StatusLevel::Warn, "ctrl-c again to quit");
        Command::None
    }

    pub fn touch_user_input(&mut self) {
        self.last_user_input = Instant::now();
        self.last_activity = Instant::now();
    }

    pub fn set_status(&mut self, level: StatusLevel, text: impl Into<String>) {
        self.status_level = level;
        self.status = text.into();
    }

    fn task_tag(&self) -> String {
        self.session_task_id
            .as_deref()
            .map(|id| format!("[{id}] "))
            .unwrap_or_default()
    }

    fn terminal_engaged(&self) -> bool {
        self.last_user_input.elapsed() < TERMINAL_ENGAGED
    }

    fn notify_force_os(&self) -> bool {
        std::env::var("METHODUS_NOTIFY")
            .ok()
            .is_some_and(|v| v.eq_ignore_ascii_case("always"))
    }

    fn should_os_notify(&self, urgency: NotifyUrgency) -> bool {
        if self.notify_force_os() {
            return true;
        }
        if !self.terminal_engaged() {
            return true;
        }
        // Pane looks active — in-app status/composer already handles it.
        match urgency {
            NotifyUrgency::Critical | NotifyUrgency::Normal | NotifyUrgency::Low => false,
        }
    }

    fn notify_recently(&self, key: &str) -> bool {
        self.notify_dedup
            .get(key)
            .is_some_and(|t| t.elapsed() < NOTIFY_DEDUP_TTL)
    }

    fn prune_notify_dedup(&mut self) {
        self.notify_dedup
            .retain(|_, t| t.elapsed() < NOTIFY_DEDUP_TTL);
    }

    pub fn notify_os(&mut self, key: &str, urgency: NotifyUrgency, body: &str) {
        if !self.notifications {
            return;
        }
        self.prune_notify_dedup();
        if self.notify_recently(key) {
            return;
        }
        if !self.should_os_notify(urgency) {
            return;
        }
        self.notify_dedup.insert(key.to_string(), Instant::now());
        crate::notify::send("Methodus", body, urgency);
    }

    pub fn notify(&mut self, key: &str, urgency: NotifyUrgency, body: &str) {
        self.notify_os(key, urgency, body);
    }

    pub fn busy(&self) -> bool {
        self.event_rx.is_some()
    }

    fn idle_for_questions(&self) -> bool {
        if self.busy() || self.show_help {
            return false;
        }
        if self.mode != Mode::Normal {
            return false;
        }
        if !self.input.is_empty() || self.answering_id.is_some() {
            return false;
        }
        if !self.approvals.is_empty() {
            return false;
        }
        !self.tasks.iter().any(|t| {
            matches!(
                t.status,
                TaskStatus::Running | TaskStatus::Planning | TaskStatus::WaitingUser
            )
        })
    }

    fn maybe_idle_prompt(&mut self) {
        const IDLE_AFTER: Duration = Duration::from_secs(30);
        const COOLDOWN: Duration = Duration::from_secs(300);
        if !self.idle_for_questions() {
            return;
        }
        if self.answering_id.is_some() {
            return;
        }
        if let Some(q) = self
            .questions
            .iter()
            .find(|q| q.status == QuestionStatus::Asked)
        {
            if self.overlay == Overlay::None && self.terminal_engaged() {
                let id = q.id.clone();
                let question = q.question.clone();
                self.begin_answer(&id, &question);
            }
            return;
        }
        if !self
            .questions
            .iter()
            .any(|q| q.status == QuestionStatus::Pending)
        {
            return;
        }
        if self.last_activity.elapsed() < IDLE_AFTER {
            return;
        }
        if self.last_idle_ask.is_some_and(|t| t.elapsed() < COOLDOWN) {
            return;
        }
        let Ok(Some(q)) = self.engine.ask_idle_question() else {
            return;
        };
        self.last_idle_ask = Some(Instant::now());
        let body = format!(
            "{}question: {}",
            self.task_tag(),
            ellipsize(&q.question, 80)
        );
        self.notify(
            &format!("question:{}", q.id),
            NotifyUrgency::Normal,
            &body,
        );
        if self.overlay == Overlay::None && self.terminal_engaged() {
            self.begin_answer(&q.id, &q.question);
        }
    }

    fn maybe_notify_new_knowledge(&mut self) {
        if !self.knowledge_notify_ready {
            for k in &self.knowledge {
                self.notified_knowledge.insert(k.id.clone());
            }
            self.knowledge_notify_ready = true;
            return;
        }
        let mut pending = Vec::new();
        for k in &self.knowledge {
            if self.notified_knowledge.contains(&k.id) {
                continue;
            }
            self.notified_knowledge.insert(k.id.clone());
            pending.push((
                k.id.clone(),
                if k.source == methodus_core::learning::SKILL_DRAFT_SOURCE {
                    "skill draft".to_string()
                } else {
                    "knowledge".to_string()
                },
            ));
        }
        let tag = self.task_tag();
        for (id, label) in pending {
            let body = format!("{tag}inbox: {label} ready — /inbox");
            self.notify(
                &format!("knowledge:{id}"),
                NotifyUrgency::Normal,
                &body,
            );
        }
    }

    fn begin_answer(&mut self, id: &str, question: &str) {
        self.mode = Mode::Answering;
        self.answering_id = Some(id.to_string());
        self.knowledge_pick_id = None;
        self.input.clear();
        self.input_error = None;
        let in_inbox = self.inbox_detail_open();
        if !in_inbox {
            self.overlay = Overlay::None;
        }
        if let Some(i) = self.questions.iter().position(|q| q.id == id) {
            self.review_sel = i;
        }
        if in_inbox {
            self.set_status(
                StatusLevel::Info,
                format!(
                    "answer below — {}  [enter]submit [esc]menu",
                    ellipsize(question, 48)
                ),
            );
        } else {
            self.transcript.push(ChatLine {
                kind: ChatKind::Alert,
                text: format!("question — type your answer below, Enter to submit, Esc to later"),
            });
            self.transcript.push(ChatLine {
                kind: ChatKind::Meta,
                text: question.to_string(),
            });
            self.set_status(
                StatusLevel::Info,
                format!(
                    "answer below — {}  [enter]submit [esc]later [d]ismiss",
                    ellipsize(question, 48)
                ),
            );
        }
    }

    pub fn begin_hypothesis_pick(&mut self, id: &str) {
        self.hypothesis_pick_id = Some(id.to_string());
        self.hypothesis_choice = 0;
        self.knowledge_pick_id = None;
        self.knowledge_choice = 0;
        self.evolution_pick_id = None;
        self.evolution_choice = 0;
        self.set_status(
            StatusLevel::Info,
            "hypothesis — [↑↓] [enter]  [y]promote  [v]validate  [d]reject  [esc]later",
        );
    }

    pub fn pending_hypothesis(&self) -> Option<&Hypothesis> {
        let id = self.hypothesis_pick_id.as_deref()?;
        self.hypotheses.iter().find(|h| h.id == id)
    }

    pub fn hypothesis_pick_choices(&self) -> &'static [(&'static str, &'static str)] {
        INBOX_HYPOTHESIS_CHOICES
    }

    pub fn begin_evolution_pick(&mut self, id: &str) {
        self.hypothesis_pick_id = None;
        self.hypothesis_choice = 0;
        self.evolution_pick_id = Some(id.to_string());
        self.evolution_choice = 0;
        self.knowledge_pick_id = None;
        self.knowledge_choice = 0;
        self.set_status(
            StatusLevel::Info,
            "face evolution — [↑↓] [enter]  [y]approve  [d]reject  [esc]later",
        );
    }

    pub fn pending_evolution(&self) -> Option<&EvolutionCandidate> {
        let id = self.evolution_pick_id.as_deref()?;
        self.evolutions.iter().find(|e| e.id == id)
    }

    pub fn evolution_pick_choices(&self) -> &'static [(&'static str, &'static str)] {
        INBOX_EVOLUTION_CHOICES
    }

    pub fn begin_knowledge_pick(&mut self, id: &str) {
        self.hypothesis_pick_id = None;
        self.hypothesis_choice = 0;
        self.evolution_pick_id = None;
        self.evolution_choice = 0;
        self.knowledge_pick_id = Some(id.to_string());
        self.knowledge_choice = 0;
        let in_inbox = self.overlay == Overlay::Inbox;
        if !in_inbox {
            self.overlay = Overlay::None;
        }
        let label = self
            .knowledge
            .iter()
            .find(|k| k.id == id)
            .map(|k| {
                if k.source == methodus_core::learning::SKILL_DRAFT_SOURCE {
                    "skill draft"
                } else {
                    "knowledge"
                }
            })
            .unwrap_or("candidate");
        if in_inbox {
            self.set_status(
                StatusLevel::Info,
                format!("{label} — scroll above · decide below · esc back to list"),
            );
        } else {
            self.transcript.push(ChatLine {
                kind: ChatKind::Alert,
                text: if self
                    .knowledge
                    .iter()
                    .find(|k| k.id == id)
                    .is_some_and(|k| {
                        k.status == KnowledgeStatus::Conflicted
                            && k.source == methodus_core::learning::SKILL_DRAFT_SOURCE
                    })
                {
                    format!("{label} conflict — ↑↓ choose, Enter, 1 replace / 2 reject, Esc later")
                } else {
                    format!("{label} ready — ↑↓ choose, Enter, or y commit / d reject / Esc later")
                },
            });
            self.set_status(
                StatusLevel::Info,
                format!("{label} — [↑↓] [enter]  [y]commit  [d]reject  [esc]later"),
            );
        }
    }

    pub fn refresh_system_lists(&mut self) {
        self.system_events = self
            .engine
            .list_recent_events(80)
            .unwrap_or_default();
        self.learning_jobs = self.engine.list_learning_jobs().unwrap_or_default();
        if self.system_sel >= self.system_list_len() {
            self.system_sel = self.system_list_len().saturating_sub(1);
        }
    }

    pub fn system_list_len(&self) -> usize {
        match self.overlay {
            Overlay::Events => self.system_events.len(),
            Overlay::Jobs => self.learning_jobs.len(),
            _ => 0,
        }
    }

    pub fn refresh(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        let _ = self.engine.tick_learning();
        if let Ok(tasks) = self.engine.store().list_tasks() {
            self.tasks = tasks;
            if self.task_sel >= self.tasks.len() {
                self.task_sel = self.tasks.len().saturating_sub(1);
            }
        }
        if let Ok(approvals) = self.engine.store().list_pending_approvals(None) {
            self.approvals = approvals;
            let n = self.session_approvals().len();
            if n == 0 {
                self.approval_sel = 0;
            } else if self.approval_sel >= n {
                self.approval_sel = n - 1;
            }
        }
        self.faces = methodus_core::list_faces(self.engine.home());
        if self.face_sel >= self.faces.len() {
            self.face_sel = self.faces.len().saturating_sub(1);
        }
        self.questions = self
            .engine
            .store()
            .list_questions(None)
            .unwrap_or_default()
            .into_iter()
            .filter(|q| {
                matches!(
                    q.status,
                    QuestionStatus::Pending | QuestionStatus::Asked | QuestionStatus::Snoozed
                )
            })
            .collect();
        self.knowledge = self
            .engine
            .store()
            .list_knowledge(None)
            .unwrap_or_default()
            .into_iter()
            .filter(|k| {
                matches!(
                    k.status,
                    KnowledgeStatus::Candidate | KnowledgeStatus::Conflicted
                )
            })
            .collect();
        self.hypotheses = self
            .engine
            .store()
            .list_hypotheses(Some(HypothesisStatus::Candidate))
            .unwrap_or_default();
        self.evolutions = self
            .engine
            .store()
            .list_evolution(Some(EvolutionStatus::Candidate))
            .unwrap_or_default();
        self.experiences = self
            .engine
            .store()
            .list_experiences()
            .unwrap_or_default()
            .into_iter()
            .take(40)
            .collect();
        let n = self.review_total();
        if self.review_sel >= n {
            self.review_sel = n.saturating_sub(1);
        }
        self.packs = list_packs(self.engine.home());
        self.projects = list_projects(self.engine.home());
        let roots_sig = mention_roots_sig(&self.projects, self.engine.launch_cwd());
        if self.mention_root_id.as_deref() != Some(roots_sig.as_str()) {
            self.mention_cache.clear();
            self.mention_root_id = None;
        }
        self.usage_today = self.engine.usage_summary(true).unwrap_or_default();
        self.usage_all = self.engine.usage_summary(false).unwrap_or_default();
        if self.overlay == Overlay::Setup {
            self.health = health_checks(self.engine.home());
            let cap = self.setup_list_len();
            if cap > 0 && self.setup_sel >= cap {
                self.setup_sel = cap.saturating_sub(1);
            }
        }
        if self.busy() {
            self.last_activity = Instant::now();
            let spin = ['|', '/', '-', '\\'][(self.tick as usize) % 4];
            let id = self.session_task_id.as_deref().unwrap_or("session");
            self.set_status(StatusLevel::Info, format!("{spin} running {id}"));
        } else if self.pending_approval().is_some() {
            let n = self.session_approvals().len();
            let tool = self
                .pending_approval()
                .map(|a| a.tool_name.as_str())
                .unwrap_or("tool");
            self.set_status(
                StatusLevel::Warn,
                format!("!{n} pending approval: {tool}"),
            );
        } else {
            self.maybe_idle_prompt();
        }
        self.maybe_notify_new_knowledge();
    }

    pub fn restore_recovered(&mut self) {
        let Some(id) = self.recovered.first().map(|r| r.task_id.clone()) else {
            return;
        };
        self.select_task(&id);
        self.session_task_id = Some(id.clone());
        self.load_transcript(&id);
    }

    pub fn selected_task(&self) -> Option<&Task> {
        self.tasks.get(self.task_sel)
    }

    pub fn selected_task_detail(&self) -> String {
        let Some(t) = self.selected_task() else {
            return String::new();
        };
        let runtime = t.runtime.as_deref().unwrap_or("-");
        format!(
            "{}  ·  {}  ·  {}\n{}",
            t.status,
            runtime,
            t.id,
            ellipsize(&t.request, 240)
        )
    }

    pub fn session_task(&self) -> Option<&Task> {
        let id = self.session_task_id.as_deref()?;
        self.tasks.iter().find(|t| t.id == id)
    }

    pub fn select_task(&mut self, id: &str) {
        if let Some(i) = self.tasks.iter().position(|t| t.id == id) {
            self.task_sel = i;
        }
    }

    fn cycle_session(&mut self, dir: isize) {
        if self.tasks.is_empty() {
            return;
        }
        let n = self.tasks.len() as isize;
        let i = (self.task_sel as isize + dir).rem_euclid(n) as usize;
        self.task_sel = i;
        if let Some(task) = self.tasks.get(i) {
            let id = task.id.clone();
            self.session_task_id = Some(id.clone());
            self.load_transcript(&id);
            self.set_status(StatusLevel::Info, format!("viewing {id}"));
        }
    }

    pub fn pending_knowledge(&self) -> Option<&KnowledgeItem> {
        let id = self.knowledge_pick_id.as_deref()?;
        self.knowledge.iter().find(|k| k.id == id)
    }

    pub fn knowledge_preview(&self) -> String {
        let Some(k) = self.pending_knowledge() else {
            return String::new();
        };
        let raw = std::fs::read_to_string(self.engine.home().join(&k.path)).unwrap_or_default();
        let body = raw
            .split("---")
            .nth(2)
            .unwrap_or(raw.as_str())
            .trim();
        body.lines()
            .filter(|l| !l.trim().is_empty())
            .take(4)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn handle_knowledge_key(&mut self, key: KeyEvent) -> Command {
        let choices = self
            .pending_knowledge()
            .map(knowledge_choices_for)
            .unwrap_or(KNOWLEDGE_CHOICES);
        let n = choices.len();
        match key.code {
            KeyCode::Esc => {
                self.knowledge_pick_id = None;
                if self.inbox_detail_open() {
                    self.close_inbox_detail();
                } else {
                    self.set_status(StatusLevel::Info, "later — open /inbox when you want to decide");
                }
                Command::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.knowledge_choice = self.knowledge_choice.saturating_sub(1);
                Command::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if n > 0 {
                    self.knowledge_choice = (self.knowledge_choice + 1).min(n - 1);
                }
                Command::None
            }
            KeyCode::Enter => {
                let i = self.knowledge_choice.min(n.saturating_sub(1));
                self.cmd_knowledge_choice(choices[i].action)
            }
            KeyCode::Char('1') | KeyCode::Char('y') => {
                self.cmd_knowledge_choice(choices[0].action)
            }
            KeyCode::Char('2') | KeyCode::Char('d') | KeyCode::Char('n') | KeyCode::Char('x') => {
                if choices.len() > 1 {
                    self.cmd_knowledge_choice(choices[1].action)
                } else {
                    self.cmd_knowledge_choice(KnowledgeReviewAction::Reject)
                }
            }
            KeyCode::Char('?') => {
                self.show_help = true;
                Command::None
            }
            _ => {
                self.set_status(
                    StatusLevel::Warn,
                    "↑↓ choose · Enter · 1/y · 2/d reject · Esc later",
                );
                Command::None
            }
        }
    }

    pub fn knowledge_pick_choices(&self) -> &'static [KnowledgeChoice] {
        self.pending_knowledge()
            .map(knowledge_choices_for)
            .unwrap_or(KNOWLEDGE_CHOICES)
    }

    fn cmd_knowledge_choice(&mut self, action: KnowledgeReviewAction) -> Command {
        match self.knowledge_pick_id.take() {
            Some(id) => Command::ReviewKnowledge { id, action },
            None => Command::None,
        }
    }

    fn handle_evolution_key(&mut self, key: KeyEvent) -> Command {
        let choices = self.evolution_pick_choices();
        let n = choices.len();
        match key.code {
            KeyCode::Esc => {
                self.evolution_pick_id = None;
                if self.inbox_detail_open() {
                    self.close_inbox_detail();
                } else {
                    self.set_status(StatusLevel::Info, "later — open /inbox when you want to decide");
                }
                Command::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.evolution_choice = self.evolution_choice.saturating_sub(1);
                Command::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if n > 0 {
                    self.evolution_choice = (self.evolution_choice + 1).min(n - 1);
                }
                Command::None
            }
            KeyCode::Enter => {
                let i = self.evolution_choice.min(n.saturating_sub(1));
                self.cmd_evolution_choice(i == 0)
            }
            KeyCode::Char('1') | KeyCode::Char('y') => self.cmd_evolution_choice(true),
            KeyCode::Char('2') | KeyCode::Char('d') | KeyCode::Char('n') | KeyCode::Char('x') => {
                self.cmd_evolution_choice(false)
            }
            KeyCode::Char('?') => {
                self.show_help = true;
                Command::None
            }
            _ => {
                self.set_status(
                    StatusLevel::Warn,
                    "↑↓ choose · Enter · 1/y approve · 2/d reject · Esc later",
                );
                Command::None
            }
        }
    }

    fn cmd_evolution_choice(&mut self, approve: bool) -> Command {
        match self.evolution_pick_id.take() {
            Some(id) => Command::ReviewEvolution { id, approve },
            None => Command::None,
        }
    }

    fn handle_hypothesis_key(&mut self, key: KeyEvent) -> Command {
        let choices = self.hypothesis_pick_choices();
        let n = choices.len();
        match key.code {
            KeyCode::Esc => {
                self.hypothesis_pick_id = None;
                if self.inbox_detail_open() {
                    self.close_inbox_detail();
                } else {
                    self.set_status(StatusLevel::Info, "later — open /inbox when you want to decide");
                }
                Command::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.hypothesis_choice = self.hypothesis_choice.saturating_sub(1);
                Command::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if n > 0 {
                    self.hypothesis_choice = (self.hypothesis_choice + 1).min(n - 1);
                }
                Command::None
            }
            KeyCode::Enter => {
                let i = self.hypothesis_choice.min(n.saturating_sub(1));
                self.cmd_hypothesis_choice(i)
            }
            KeyCode::Char('1') | KeyCode::Char('y') => self.cmd_hypothesis_choice(0),
            KeyCode::Char('2') | KeyCode::Char('v') => self.cmd_hypothesis_choice(1),
            KeyCode::Char('3') | KeyCode::Char('d') | KeyCode::Char('n') | KeyCode::Char('x') => {
                self.cmd_hypothesis_choice(2)
            }
            KeyCode::Char('?') => {
                self.show_help = true;
                Command::None
            }
            _ => {
                self.set_status(
                    StatusLevel::Warn,
                    "↑↓ choose · Enter · 1/y promote · 2/v validate · 3/d reject · Esc later",
                );
                Command::None
            }
        }
    }

    fn cmd_hypothesis_choice(&mut self, i: usize) -> Command {
        let action = match i {
            0 => HypothesisReviewAction::Promote,
            1 => HypothesisReviewAction::Validate,
            _ => HypothesisReviewAction::Reject,
        };
        match self.hypothesis_pick_id.take() {
            Some(id) => Command::ReviewHypothesis { id, action },
            None => Command::None,
        }
    }

    pub fn attach_session(&mut self, task_id: String, rx: mpsc::Receiver<RuntimeEvent>) {
        self.session_task_id = Some(task_id.clone());
        self.select_task(&task_id);
        self.overlay = Overlay::None;
        self.load_transcript(&task_id);
        self.event_rx = Some(rx);
        self.input.clear();
        self.input_error = None;
        self.transcript_offset = 0;
        self.set_status(StatusLevel::Info, format!("running {task_id}"));
    }

    pub fn attach_receiver(&mut self, rx: mpsc::Receiver<RuntimeEvent>) {
        self.overlay = Overlay::None;
        self.event_rx = Some(rx);
    }

    pub fn load_transcript(&mut self, task_id: &str) {
        self.transcript.clear();
        if let Ok(events) = self.engine.store().list_events(Some(task_id), 2000) {
            for ev in events {
                if let Ok(parsed) = serde_json::from_str::<RuntimeEvent>(&ev.payload) {
                    append_transcript_event(&mut self.transcript, &parsed);
                }
            }
        }
        let has_you = self.transcript.iter().any(|l| l.kind == ChatKind::You);
        if !has_you {
            if let Some(request) = self
                .tasks
                .iter()
                .find(|t| t.id == task_id)
                .map(|t| t.request.clone())
            {
                let mut head = format_event(&RuntimeEvent::UserText { text: request });
                head.append(&mut self.transcript);
                self.transcript = head;
            }
        }
        self.transcript_offset = 0;
        self.approval_choice = 0;
        self.approval_sel = 0;
    }

    pub fn push_runtime(&mut self, event: RuntimeEvent) {
        if let RuntimeEvent::ApprovalRequested {
            id,
            tool_name,
            input,
            ..
        } = &event
        {
            self.overlay = Overlay::None;
            self.approval_choice = 0;
            self.approval_sel = 0;
            self.refresh();
            let detail = ellipsize(&summarize_tool_json(input), 72);
            let body = format!(
                "{}needs approval: {tool_name} — {detail}",
                self.task_tag()
            );
            self.notify(
                &format!("approval:{id}"),
                NotifyUrgency::Critical,
                &body,
            );
            self.set_status(
                StatusLevel::Warn,
                format!(
                    "!{} pending approval: {tool_name}",
                    self.session_approvals().len()
                ),
            );
        }
        if let RuntimeEvent::Error { message } = &event {
            self.set_status(StatusLevel::Error, message.clone());
            let body = format!("{}error: {}", self.task_tag(), ellipsize(message, 100));
            self.notify(
                &format!("error:{}", ellipsize(message, 40)),
                NotifyUrgency::Critical,
                &body,
            );
        }
        if let RuntimeEvent::Result {
            is_error: true,
            text,
            ..
        } = &event
        {
            let msg = if text.trim().is_empty() {
                "executor failed (no message) — press r to retry, or ctrl-n for a new session"
                    .to_string()
            } else {
                text.clone()
            };
            self.set_status(StatusLevel::Error, msg.clone());
            let body = format!("{}error: {}", self.task_tag(), ellipsize(&msg, 100));
            self.notify(
                &format!("error:{}", ellipsize(&msg, 40)),
                NotifyUrgency::Critical,
                &body,
            );
        }
        if let RuntimeEvent::Result {
            is_error: false, ..
        } = &event
        {
            self.set_status(
                StatusLevel::Info,
                "your turn — type below · /inbox",
            );
        }
        append_transcript_event(&mut self.transcript, &event);
        const MAX: usize = 2000;
        if self.transcript.len() > MAX {
            let drop_n = self.transcript.len() - MAX;
            self.transcript.drain(0..drop_n);
        }
        if self.transcript_offset == 0 {
            // stay pinned to bottom
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Command {
        if is_ctrl_c(key) {
            if key.kind != KeyEventKind::Press {
                return Command::None;
            }
            return self.handle_ctrl_c();
        }
        self.pending_quit_at = None;
        if self.mode == Mode::Prompt {
            return self.handle_prompt_key(key);
        }
        if self.mode == Mode::Answering {
            return self.handle_answer_key(key);
        }
        if self.mode == Mode::ConfirmCancel {
            return self.handle_confirm_key(key);
        }
        if self.show_help {
            return self.handle_help_key(key);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('n') {
            self.start_new_conversation();
            return Command::None;
        }
        match self.overlay {
            Overlay::Setup => return self.handle_setup_key(key),
            Overlay::Inbox if self.inbox_detail => return self.handle_inbox_detail_key(key),
            Overlay::Inbox => return self.handle_inbox_key(key),
            Overlay::Faces => return self.handle_faces_key(key),
            Overlay::Sessions => return self.handle_sessions_key(key),
            Overlay::Events | Overlay::Jobs => return self.handle_system_key(key),
            Overlay::None => {}
        }
        self.handle_session_key(key)
    }

    fn close_overlay(&mut self) {
        self.inbox_detail = false;
        self.inbox_menu_choice = 0;
        self.knowledge_pick_id = None;
        self.answering_id = None;
        if self.mode == Mode::Answering {
            self.mode = Mode::Normal;
            self.input.clear();
        }
        self.overlay = Overlay::None;
        self.overlay_filter.clear();
        self.set_status(StatusLevel::Info, "session");
    }

    pub fn inbox_detail_open(&self) -> bool {
        self.overlay == Overlay::Inbox && self.inbox_detail
    }

    pub fn inbox_question_menu(&self) -> bool {
        self.inbox_detail_open()
            && self.answering_id.is_none()
            && self.knowledge_pick_id.is_none()
            && self.evolution_pick_id.is_none()
            && self.hypothesis_pick_id.is_none()
            && matches!(self.selected_review(), Some(ReviewItem::Question(_)))
    }

    pub fn inbox_experience_menu(&self) -> bool {
        self.inbox_detail_open()
            && self.answering_id.is_none()
            && self.knowledge_pick_id.is_none()
            && self.evolution_pick_id.is_none()
            && self.hypothesis_pick_id.is_none()
            && matches!(self.selected_review(), Some(ReviewItem::Experience(_)))
    }

    pub fn inbox_evolution_menu(&self) -> bool {
        self.inbox_detail_open()
            && self.answering_id.is_none()
            && self.knowledge_pick_id.is_none()
            && matches!(self.selected_review(), Some(ReviewItem::Evolution(_)))
    }

    fn open_inbox_detail(&mut self) {
        if self.selected_review().is_none() {
            return;
        }
        self.inbox_detail = true;
        self.review_detail_scroll = 0;
        self.inbox_menu_choice = 0;
        self.input.clear();
        self.input_error = None;
        if let Some(ReviewItem::Knowledge(k)) = self.selected_review() {
            let id = k.id.clone();
            self.begin_knowledge_pick(&id);
        } else if let Some(ReviewItem::Hypothesis(h)) = self.selected_review() {
            let id = h.id.clone();
            self.begin_hypothesis_pick(&id);
        } else if let Some(ReviewItem::Evolution(e)) = self.selected_review() {
            let id = e.id.clone();
            self.begin_evolution_pick(&id);
        } else {
            self.knowledge_pick_id = None;
            self.knowledge_choice = 0;
            self.hypothesis_pick_id = None;
            self.hypothesis_choice = 0;
            self.evolution_pick_id = None;
            self.evolution_choice = 0;
        }
        self.set_status(StatusLevel::Info, "inbox detail — scroll · esc back to list");
    }

    pub fn close_inbox_detail(&mut self) {
        self.inbox_detail = false;
        self.review_detail_scroll = 0;
        self.inbox_menu_choice = 0;
        self.knowledge_pick_id = None;
        self.hypothesis_pick_id = None;
        self.hypothesis_choice = 0;
        self.evolution_pick_id = None;
        self.evolution_choice = 0;
        self.answering_id = None;
        self.mode = Mode::Normal;
        self.input.clear();
        self.input_error = None;
        self.set_status(StatusLevel::Info, "inbox — Enter opens full view");
    }

    fn overlay_filter_active(&self) -> bool {
        match self.overlay {
            Overlay::Inbox if self.inbox_detail => false,
            Overlay::Sessions | Overlay::Faces | Overlay::Inbox | Overlay::Events | Overlay::Jobs => true,
            _ => false,
        }
    }

    fn overlay_consume_filter_key(&mut self, key: KeyEvent) -> bool {
        if !self.overlay_filter_active() {
            return false;
        }
        match key.code {
            KeyCode::Esc if !self.overlay_filter.is_empty() => {
                self.overlay_filter.clear();
                true
            }
            KeyCode::Backspace if !self.overlay_filter.is_empty() => {
                self.overlay_filter.pop();
                self.snap_sel_to_filter();
                true
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !overlay_reserved_char(self.overlay, c) =>
            {
                self.overlay_filter.push(c);
                self.snap_sel_to_filter();
                true
            }
            _ => false,
        }
    }

    fn snap_sel_to_filter(&mut self) {
        let vis = self.visible_overlay_indices();
        if vis.is_empty() {
            return;
        }
        if !vis.contains(&self.current_sel()) {
            self.set_sel(vis[0]);
        }
    }

    pub fn visible_overlay_indices(&self) -> Vec<usize> {
        match self.overlay {
            Overlay::Sessions => self.visible_task_indices(),
            Overlay::Faces => self.visible_face_indices(),
            Overlay::Inbox => self.visible_review_indices(),
            Overlay::Events | Overlay::Jobs => (0..self.system_list_len()).collect(),
            Overlay::Setup => (0..self.setup_list_len()).collect(),
            Overlay::None => Vec::new(),
        }
    }

    pub fn visible_task_indices(&self) -> Vec<usize> {
        scored_indices(&self.overlay_filter, self.tasks.len(), |i| {
            let t = &self.tasks[i];
            format!("{} {} {} {}", t.id, t.title, t.status, t.request)
        })
    }

    pub fn visible_face_indices(&self) -> Vec<usize> {
        scored_indices(&self.overlay_filter, self.faces.len(), |i| {
            let f = &self.faces[i];
            format!("{} {} {} {}", f.id, f.name, f.source, f.description)
        })
    }

    pub fn visible_review_indices(&self) -> Vec<usize> {
        let n = self.review_total();
        scored_indices(&self.overlay_filter, n, |i| self.review_hay(i))
    }

    fn review_hypothesis_start(&self) -> usize {
        self.questions.len() + self.knowledge.len()
    }

    fn review_evolution_start(&self) -> usize {
        self.review_hypothesis_start() + self.hypotheses.len()
    }

    fn review_experience_start(&self) -> usize {
        self.review_evolution_start() + self.evolutions.len()
    }

    pub fn review_total(&self) -> usize {
        self.review_experience_start() + self.experiences.len()
    }

    fn review_hay(&self, i: usize) -> String {
        if i < self.questions.len() {
            let q = &self.questions[i];
            return format!("Q {} {}", q.status, q.question);
        }
        let j = i - self.questions.len();
        if j < self.knowledge.len() {
            let k = &self.knowledge[j];
            return format!("K {} {} {}", k.status, k.path, k.source);
        }
        let k = j - self.knowledge.len();
        if k < self.hypotheses.len() {
            let h = &self.hypotheses[k];
            return format!("H {} {} {}", h.status, h.path, h.face_id.as_deref().unwrap_or("-"));
        }
        let e = k - self.hypotheses.len();
        if e < self.evolutions.len() {
            let ev = &self.evolutions[e];
            return format!(
                "F {} face:{} {}",
                ev.status,
                ev.target_id,
                ev.rationale.as_deref().unwrap_or("-")
            );
        }
        self.experiences
            .get(e - self.evolutions.len())
            .map(|e| {
                format!(
                    "E {} {}",
                    e.outcome.as_deref().unwrap_or("-"),
                    e.summary.as_deref().unwrap_or(&e.id)
                )
            })
            .unwrap_or_default()
    }

    pub fn insert_paste(&mut self, raw: &str) {
        let s = raw.replace("\r\n", "\n").replace('\r', "\n");
        if self.overlay_filter_active() {
            self.overlay_filter
                .push_str(&s.chars().filter(|c| *c != '\n').collect::<String>());
            self.snap_sel_to_filter();
            return;
        }
        if self.mode == Mode::Prompt {
            self.insert_str(&s.replace('\n', ""));
            return;
        }
        self.insert_str(&s);
        self.sync_slash_sel();
        self.sync_mention_sel();
    }

    fn insert_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.clamp_cursor();
        self.input.insert_str(self.input_cursor, s);
        self.input_cursor += s.len();
        self.clamp_cursor();
        self.input_error = None;
    }

    fn backspace_char(&mut self) {
        self.clamp_cursor();
        if self.input_cursor == 0 {
            return;
        }
        let prev = prev_char_boundary(&self.input, self.input_cursor);
        self.input.replace_range(prev..self.input_cursor, "");
        self.input_cursor = prev;
        self.input_error = None;
    }

    fn delete_char(&mut self) {
        self.clamp_cursor();
        if self.input_cursor >= self.input.len() {
            return;
        }
        let next = next_char_boundary(&self.input, self.input_cursor);
        self.input.replace_range(self.input_cursor..next, "");
        self.input_error = None;
    }

    fn move_cursor(&mut self, dir: isize) {
        self.clamp_cursor();
        if dir < 0 {
            if self.input_cursor > 0 {
                self.input_cursor = prev_char_boundary(&self.input, self.input_cursor);
            }
        } else if self.input_cursor < self.input.len() {
            self.input_cursor = next_char_boundary(&self.input, self.input_cursor);
        }
    }

    fn cursor_line_home(&mut self) {
        self.clamp_cursor();
        let prefix = &self.input[..self.input_cursor];
        self.input_cursor = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    }

    fn cursor_line_end(&mut self) {
        self.clamp_cursor();
        let rest = &self.input[self.input_cursor..];
        let advance = rest.find('\n').unwrap_or(rest.len());
        self.input_cursor = floor_char_boundary(&self.input, self.input_cursor + advance);
    }

    fn clamp_cursor(&mut self) {
        self.input_cursor = self.input_cursor.min(self.input.len());
        while self.input_cursor > 0 && !self.input.is_char_boundary(self.input_cursor) {
            self.input_cursor -= 1;
        }
    }

    fn snap_cursor_end(&mut self) {
        self.input_cursor = self.input.len();
    }

    fn handle_editor_nav(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Left => self.move_cursor(-1),
            KeyCode::Right => self.move_cursor(1),
            KeyCode::Home => self.cursor_line_home(),
            KeyCode::End => self.cursor_line_end(),
            KeyCode::Delete => self.delete_char(),
            KeyCode::Char('a') if ctrl => self.cursor_line_home(),
            KeyCode::Char('e') if ctrl => self.cursor_line_end(),
            KeyCode::Char('b') if ctrl => self.move_cursor(-1),
            KeyCode::Char('f') if ctrl => self.move_cursor(1),
            _ => return false,
        }
        true
    }

    fn handle_inbox_key(&mut self, key: KeyEvent) -> Command {
        if self.overlay_consume_filter_key(key) {
            return Command::None;
        }
        match key.code {
            KeyCode::Esc => {
                self.close_overlay();
                Command::None
            }
            KeyCode::Char('?') => {
                self.show_help = true;
                Command::None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_sel(1);
                Command::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_sel(-1);
                Command::None
            }
            KeyCode::Char('g') => {
                self.jump_sel(0);
                Command::None
            }
            KeyCode::Char('G') => {
                self.jump_sel(isize::MAX);
                Command::None
            }
            KeyCode::Enter => {
                self.open_inbox_detail();
                Command::None
            }
            _ => Command::None,
        }
    }

    fn handle_inbox_detail_key(&mut self, key: KeyEvent) -> Command {
        if self.pending_knowledge().is_some() {
            return self.handle_knowledge_key(key);
        }
        if self.pending_hypothesis().is_some() {
            return self.handle_hypothesis_key(key);
        }
        if self.pending_evolution().is_some() {
            return self.handle_evolution_key(key);
        }
        if self.inbox_experience_menu() {
            return self.handle_inbox_experience_menu_key(key);
        }
        if self.inbox_question_menu() {
            return self.handle_inbox_question_menu_key(key);
        }
        match key.code {
            KeyCode::Esc => {
                self.close_inbox_detail();
                Command::None
            }
            KeyCode::Char('?') => {
                self.show_help = true;
                Command::None
            }
            KeyCode::Char('[') => {
                self.scroll_review_detail(-3);
                Command::None
            }
            KeyCode::Char(']') => {
                self.scroll_review_detail(3);
                Command::None
            }
            KeyCode::PageUp => {
                self.scroll_review_detail(-8);
                Command::None
            }
            KeyCode::PageDown => {
                self.scroll_review_detail(8);
                Command::None
            }
            _ => Command::None,
        }
    }

    fn handle_inbox_question_menu_key(&mut self, key: KeyEvent) -> Command {
        let n = INBOX_QUESTION_CHOICES.len();
        match key.code {
            KeyCode::Esc => {
                self.close_inbox_detail();
                Command::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.inbox_menu_choice = self.inbox_menu_choice.saturating_sub(1);
                Command::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if n > 0 {
                    self.inbox_menu_choice = (self.inbox_menu_choice + 1).min(n - 1);
                }
                Command::None
            }
            KeyCode::Enter => self.cmd_inbox_question_choice(self.inbox_menu_choice),
            KeyCode::Char('1') => self.cmd_inbox_question_choice(0),
            KeyCode::Char('2') => self.cmd_inbox_question_choice(1),
            KeyCode::Char('3') => self.cmd_inbox_question_choice(2),
            KeyCode::Char('4') => self.cmd_inbox_question_choice(3),
            _ => Command::None,
        }
    }

    fn cmd_inbox_question_choice(&mut self, i: usize) -> Command {
        match i {
            0 => {
                if let Some(ReviewItem::Question(q)) = self.selected_review() {
                    let id = q.id.clone();
                    let question = q.question.clone();
                    self.begin_answer(&id, &question);
                }
                Command::None
            }
            1 => self.cmd_snooze(),
            2 => self.cmd_review_negative(),
            _ => {
                self.close_inbox_detail();
                Command::None
            }
        }
    }

    fn handle_inbox_experience_menu_key(&mut self, key: KeyEvent) -> Command {
        let n = INBOX_EXPERIENCE_CHOICES.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('2') => {
                self.close_inbox_detail();
                Command::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.inbox_menu_choice = self.inbox_menu_choice.saturating_sub(1);
                Command::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if n > 0 {
                    self.inbox_menu_choice = (self.inbox_menu_choice + 1).min(n - 1);
                }
                Command::None
            }
            KeyCode::Enter | KeyCode::Char('1') | KeyCode::Char('y') => {
                let cmd = self.cmd_review(true);
                if matches!(cmd, Command::None) {
                    self.set_status(StatusLevel::Info, "experience is already recorded");
                }
                cmd
            }
            _ => Command::None,
        }
    }

    fn handle_system_key(&mut self, key: KeyEvent) -> Command {
        let n = self.system_list_len();
        match key.code {
            KeyCode::Esc => {
                self.close_overlay();
                Command::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.system_sel = self.system_sel.saturating_sub(1);
                Command::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if n > 0 {
                    self.system_sel = (self.system_sel + 1).min(n - 1);
                }
                Command::None
            }
            KeyCode::Char('c') if self.overlay == Overlay::Jobs => {
                if let Some(job) = self.learning_jobs.get(self.system_sel) {
                    return Command::CancelLearningJob { id: job.id.clone() };
                }
                Command::None
            }
            KeyCode::Char('r') => {
                self.refresh_system_lists();
                Command::None
            }
            _ => Command::None,
        }
    }

    fn handle_faces_key(&mut self, key: KeyEvent) -> Command {
        if self.overlay_consume_filter_key(key) {
            return Command::None;
        }
        match key.code {
            KeyCode::Esc => {
                self.close_overlay();
                Command::None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_sel(1);
                Command::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_sel(-1);
                Command::None
            }
            KeyCode::Enter => {
                self.pin_selected_face();
                self.close_overlay();
                Command::None
            }
            _ => Command::None,
        }
    }

    fn open_sessions(&mut self) {
        self.overlay = Overlay::Sessions;
        self.overlay_filter.clear();
        self.set_status(StatusLevel::Info, "sessions — type to filter, enter to open");
    }

    fn handle_sessions_key(&mut self, key: KeyEvent) -> Command {
        if self.overlay_consume_filter_key(key) {
            return Command::None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Tab | KeyCode::BackTab => {
                self.close_overlay();
                Command::None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_sel(1);
                Command::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_sel(-1);
                Command::None
            }
            KeyCode::Enter => {
                if let Some(task) = self.selected_task() {
                    let id = task.id.clone();
                    self.session_task_id = Some(id.clone());
                    self.load_transcript(&id);
                    self.set_status(StatusLevel::Info, format!("viewing {id}"));
                }
                self.close_overlay();
                Command::None
            }
            KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Delete => self.begin_delete(),
            KeyCode::Char('c') => self.begin_cancel(),
            _ => Command::None,
        }
    }

    fn pin_selected_face(&mut self) {
        if let Some(face) = self.faces.get(self.face_sel) {
            self.default_face = Some(face.id.clone());
            match self.persist_config() {
                Ok(()) => self.set_status(
                    StatusLevel::Ok,
                    format!("default face `{}`", face.id),
                ),
                Err(e) => self.set_status(StatusLevel::Error, e),
            }
        }
    }

    fn pin_face_by_query(&mut self, query: &str) {
        let (primary, context) = methodus_core::multi_face::parse_face_pin(query);
        if let Some(id) = primary {
            if let Some((i, _)) = self.faces.iter().enumerate().find(|(_, f)| f.id == id) {
                self.face_sel = i;
                self.default_face = Some(id);
                self.context_faces = context;
                match self.persist_config() {
                    Ok(()) => {
                        let ctx = if self.context_faces.is_empty() {
                            String::new()
                        } else {
                            format!(" + {}", self.context_faces.join(" + "))
                        };
                        self.set_status(
                            StatusLevel::Ok,
                            format!("face `{}{ctx}`", self.default_face.as_deref().unwrap_or("-")),
                        );
                    }
                    Err(e) => self.set_status(StatusLevel::Error, e),
                }
                return;
            }
        }
        let q = query.to_ascii_lowercase();
        let found = self.faces.iter().enumerate().find(|(_, f)| {
            f.id.to_ascii_lowercase().starts_with(&q)
                || f.id.to_ascii_lowercase() == q
                || f.name.to_ascii_lowercase() == q
        });
        match found {
            Some((i, _)) => {
                self.face_sel = i;
                self.pin_selected_face();
            }
            None => {
                self.input_error = Some(format!("no face matching `{query}` — try /face"));
                self.overlay_filter.clear();
                self.overlay = Overlay::Faces;
            }
        }
    }

    fn handle_session_key(&mut self, key: KeyEvent) -> Command {
        if self.pending_approval().is_some() {
            return self.handle_approval_key(key);
        }
        if self.knowledge_pick_id.is_some() {
            return self.handle_knowledge_key(key);
        }
        if self.evolution_pick_id.is_some() {
            return self.handle_evolution_key(key);
        }
        let empty = self.input.is_empty();
        let slash_open = slash_menu_open(&self.input);
        let mention_open = !slash_open && at_query(&self.input).is_some();
        if mention_open {
            self.ensure_mention_cache();
        }
        match key.code {
            KeyCode::Esc => {
                if slash_open {
                    self.input.clear();
                    self.input_cursor = 0;
                    self.input_error = None;
                    self.slash_sel = 0;
                    return Command::None;
                }
                if mention_open {
                    self.cancel_mention();
                    return Command::None;
                }
                if self.knowledge_pick_id.take().is_some()
                    || self.hypothesis_pick_id.take().is_some()
                    || self.evolution_pick_id.take().is_some()
                {
                    self.set_status(
                        StatusLevel::Info,
                        "later — open /inbox when you want to decide",
                    );
                    return Command::None;
                }
                Command::None
            }
            KeyCode::Tab => {
                if slash_open {
                    self.complete_slash();
                    return Command::None;
                }
                if mention_open {
                    self.accept_mention(false);
                    return Command::None;
                }
                if empty {
                    self.open_sessions();
                }
                Command::None
            }
            KeyCode::BackTab => {
                if empty {
                    self.open_sessions();
                }
                Command::None
            }
            KeyCode::Enter | KeyCode::Char('\n') if wants_newline(key) => {
                self.insert_str("\n");
                Command::None
            }
            KeyCode::Enter => {
                if slash_open {
                    self.dispatch_slash_input()
                } else if mention_open {
                    self.accept_mention(true);
                    Command::None
                } else {
                    self.cmd_send()
                }
            }
            KeyCode::Backspace => {
                self.backspace_char();
                self.sync_slash_sel();
                self.sync_mention_sel();
                Command::None
            }
            KeyCode::Up => {
                if slash_open {
                    self.slash_sel = self.slash_sel.saturating_sub(1);
                    return Command::None;
                }
                if mention_open {
                    self.mention_sel = self.mention_sel.saturating_sub(1);
                    return Command::None;
                }
                self.scroll_transcript(1);
                Command::None
            }
            KeyCode::Down => {
                if slash_open {
                    let n = matching_slash(&self.input).len();
                    if n > 0 {
                        self.slash_sel = (self.slash_sel + 1).min(n - 1);
                    }
                    return Command::None;
                }
                if mention_open {
                    let n = self.matching_mentions().len();
                    if n > 0 {
                        self.mention_sel = (self.mention_sel + 1).min(n - 1);
                    }
                    return Command::None;
                }
                self.scroll_transcript(-1);
                Command::None
            }
            KeyCode::PageUp => {
                self.scroll_transcript(10);
                Command::None
            }
            KeyCode::PageDown => {
                self.scroll_transcript(-10);
                Command::None
            }
            KeyCode::Char('?') if empty => {
                self.show_help = true;
                Command::None
            }
            KeyCode::Char('[') if empty => {
                self.cycle_session(-1);
                Command::None
            }
            KeyCode::Char(']') if empty => {
                self.cycle_session(1);
                Command::None
            }
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_str("\n");
                Command::None
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_str(&c.to_string());
                self.sync_slash_sel();
                self.sync_mention_sel();
                Command::None
            }
            _ if self.handle_editor_nav(key) => Command::None,
            _ => Command::None,
        }
    }

    fn handle_help_key(&mut self, key: KeyEvent) -> Command {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') => {
                self.show_help = false;
                Command::None
            }
            _ => Command::None,
        }
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) -> Command {
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') => {
                self.mode = Mode::Normal;
                self.confirm_task_id = None;
                self.confirm_delete = false;
                self.set_status(StatusLevel::Info, "kept");
                Command::None
            }
            KeyCode::Enter | KeyCode::Char('y') => {
                let id = self.confirm_task_id.clone().unwrap_or_default();
                let delete = self.confirm_delete;
                self.mode = Mode::Normal;
                self.confirm_task_id = None;
                self.confirm_delete = false;
                if delete {
                    Command::Delete { task_id: id }
                } else {
                    Command::Cancel { task_id: id }
                }
            }
            _ => Command::None,
        }
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) -> Command {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.prompt_kind = None;
                self.input.clear();
                self.input_cursor = 0;
                self.input_error = None;
                self.set_status(StatusLevel::Info, "cancelled");
                Command::None
            }
            KeyCode::Enter => {
                let text = self.input.trim().to_string();
                let kind = self.prompt_kind;
                self.submit_prompt(kind, &text);
                Command::None
            }
            KeyCode::Backspace => {
                self.backspace_char();
                Command::None
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_str(&c.to_string());
                Command::None
            }
            _ if self.handle_editor_nav(key) => Command::None,
            _ => Command::None,
        }
    }

    fn begin_prompt(&mut self, kind: PromptKind) {
        self.mode = Mode::Prompt;
        self.prompt_kind = Some(kind);
        self.input.clear();
        self.input_cursor = 0;
        self.input_error = None;
        if kind == PromptKind::WorkspaceRoot {
            self.input = self.workspace_root.clone();
            self.snap_cursor_end();
        }
        self.set_status(StatusLevel::Info, prompt_hint(kind));
    }

    fn submit_prompt(&mut self, kind: Option<PromptKind>, text: &str) {
        let home = self.engine.home().to_path_buf();
        let result = match kind {
            Some(PromptKind::AddProject) => {
                if text.is_empty() {
                    Err("path is empty".to_string())
                } else {
                    methodus_core::project::add_project(&home, std::path::Path::new(text))
                        .map(|p| format!("project {} → {}", p.id, p.root.display()))
                        .map_err(|e| e.to_string())
                }
            }
            Some(PromptKind::AddPack) => {
                if text.is_empty() {
                    Err("path is empty".to_string())
                } else {
                    methodus_core::pack::add_pack(&home, std::path::Path::new(text))
                        .map(|p| format!("pack {} registered", p.id))
                        .map_err(|e| e.to_string())
                }
            }
            Some(PromptKind::WorkspaceRoot) => {
                self.workspace_root = text.to_string();
                self.persist_config().map(|_| {
                    if text.is_empty() {
                        "workspace root = default".to_string()
                    } else {
                        format!("workspace root {text}")
                    }
                })
            }
            None => Err("nothing to submit".to_string()),
        };
        match result {
            Ok(msg) => {
                self.mode = Mode::Normal;
                self.prompt_kind = None;
                self.input.clear();
                self.input_error = None;
                self.refresh();
                self.set_status(StatusLevel::Ok, msg);
            }
            Err(e) => {
                self.input_error = Some(e.clone());
                self.set_status(StatusLevel::Error, e);
            }
        }
    }

    fn persist_config(&self) -> Result<(), String> {
        let cfg = UserConfig {
            default_runtime: self.runtime.clone(),
            permission_mode: Some(self.permission_mode.clone()),
            default_face: self.default_face.clone(),
            context_faces: if self.context_faces.is_empty() {
                None
            } else {
                Some(self.context_faces.clone())
            },
            workspace_root: if self.workspace_root.trim().is_empty() {
                None
            } else {
                Some(self.workspace_root.clone())
            },
            notifications: Some(self.notifications),
        };
        cfg.save(self.engine.home()).map_err(|e| e.to_string())
    }

    fn handle_setup_key(&mut self, key: KeyEvent) -> Command {
        match key.code {
            KeyCode::Char('?') => {
                self.show_help = true;
                Command::None
            }
            KeyCode::Esc => {
                self.close_overlay();
                Command::None
            }
            KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => {
                self.setup_section = self.setup_section.next();
                self.setup_sel = 0;
                Command::None
            }
            KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Left => {
                self.setup_section = self.setup_section.prev();
                self.setup_sel = 0;
                Command::None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_setup(1);
                Command::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_setup(-1);
                Command::None
            }
            KeyCode::Char('t') => {
                self.cycle_runtime_persist();
                Command::None
            }
            KeyCode::Char('p') => {
                let mut cfg = UserConfig {
                    permission_mode: Some(self.permission_mode.clone()),
                    ..UserConfig::default()
                };
                cfg.cycle_permission();
                self.permission_mode = cfg
                    .permission_mode
                    .unwrap_or_else(|| "acceptEdits".to_string());
                match self.persist_config() {
                    Ok(()) => self.set_status(
                        StatusLevel::Ok,
                        format!("permission {}", self.permission_mode),
                    ),
                    Err(e) => self.set_status(StatusLevel::Error, e),
                }
                Command::None
            }
            KeyCode::Char('a') => {
                match self.setup_section {
                    SetupSection::Settings => self.begin_prompt(PromptKind::WorkspaceRoot),
                    SetupSection::Projects => self.begin_prompt(PromptKind::AddProject),
                    SetupSection::Packs => self.begin_prompt(PromptKind::AddPack),
                }
                Command::None
            }
            KeyCode::Char(' ') => {
                self.toggle_selected_pack();
                Command::None
            }
            KeyCode::Char('d') | KeyCode::Char('x') => {
                self.remove_selected_setup();
                Command::None
            }
            KeyCode::Enter => {
                self.activate_setup_row();
                Command::None
            }
            _ => Command::None,
        }
    }

    fn setup_list_len(&self) -> usize {
        match self.setup_section {
            SetupSection::Settings => 4,
            SetupSection::Projects => self.projects.len(),
            SetupSection::Packs => self.packs.len(),
        }
    }

    fn move_setup(&mut self, delta: isize) {
        let len = self.setup_list_len();
        if len == 0 {
            return;
        }
        self.setup_sel = (self.setup_sel as isize + delta).rem_euclid(len as isize) as usize;
    }

    fn cycle_runtime_persist(&mut self) {
        self.toggle_runtime();
        match self.persist_config() {
            Ok(()) => {}
            Err(e) => self.set_status(StatusLevel::Error, e),
        }
    }

    fn activate_setup_row(&mut self) {
        let home = self.engine.home().to_path_buf();
        match self.setup_section {
            SetupSection::Settings => match self.setup_sel {
                0 => self.cycle_runtime_persist(),
                1 => {
                    let mut cfg = UserConfig {
                        permission_mode: Some(self.permission_mode.clone()),
                        ..UserConfig::default()
                    };
                    cfg.cycle_permission();
                    self.permission_mode = cfg
                        .permission_mode
                        .unwrap_or_else(|| "acceptEdits".to_string());
                    if let Err(e) = self.persist_config() {
                        self.set_status(StatusLevel::Error, e);
                    } else {
                        self.set_status(
                            StatusLevel::Ok,
                            format!("permission {}", self.permission_mode),
                        );
                    }
                }
                2 => {
                    self.notifications = !self.notifications;
                    match self.persist_config() {
                        Ok(()) => {
                            let state = if self.notifications { "on" } else { "off" };
                            self.set_status(StatusLevel::Ok, format!("notifications {state}"));
                        }
                        Err(e) => self.set_status(StatusLevel::Error, e),
                    }
                }
                _ => self.begin_prompt(PromptKind::WorkspaceRoot),
            },
            SetupSection::Projects => {
                if let Some(p) = self.projects.get(self.setup_sel) {
                    match methodus_core::project::set_focus(&home, &p.id) {
                        Ok(info) => {
                            self.refresh();
                            self.set_status(StatusLevel::Ok, format!("focus project {}", info.id));
                        }
                        Err(e) => self.set_status(StatusLevel::Error, e.to_string()),
                    }
                }
            }
            SetupSection::Packs => {
                if let Some(p) = self.packs.get(self.setup_sel) {
                    match methodus_core::pack::set_focus(&home, &p.id) {
                        Ok(info) => {
                            self.refresh();
                            self.set_status(StatusLevel::Ok, format!("focus pack {}", info.id));
                        }
                        Err(e) => self.set_status(StatusLevel::Error, e.to_string()),
                    }
                }
            }
        }
    }

    fn toggle_selected_pack(&mut self) {
        let home = self.engine.home().to_path_buf();
        let Some(p) = self.packs.get(self.setup_sel) else {
            return;
        };
        if self.setup_section != SetupSection::Packs {
            return;
        }
        match methodus_core::pack::set_active(&home, &p.id, !p.active) {
            Ok(info) => {
                self.refresh();
                let state = if info.active { "active" } else { "off" };
                self.set_status(StatusLevel::Ok, format!("{} {state}", info.id));
            }
            Err(e) => self.set_status(StatusLevel::Error, e.to_string()),
        }
    }

    fn remove_selected_setup(&mut self) {
        let home = self.engine.home().to_path_buf();
        match self.setup_section {
            SetupSection::Settings => {}
            SetupSection::Projects => {
                if let Some(p) = self.projects.get(self.setup_sel) {
                    match methodus_core::project::remove_project(&home, &p.id) {
                        Ok(()) => {
                            self.refresh();
                            self.set_status(StatusLevel::Ok, "project unregistered");
                        }
                        Err(e) => self.set_status(StatusLevel::Error, e.to_string()),
                    }
                }
            }
            SetupSection::Packs => {
                if let Some(p) = self.packs.get(self.setup_sel) {
                    match methodus_core::pack::remove_pack(&home, &p.id) {
                        Ok(()) => {
                            self.refresh();
                            self.set_status(StatusLevel::Ok, "pack unregistered");
                        }
                        Err(e) => self.set_status(StatusLevel::Error, e.to_string()),
                    }
                }
            }
        }
    }

    fn handle_answer_key(&mut self, key: KeyEvent) -> Command {
        let empty = self.input.is_empty();
        match key.code {
            KeyCode::Esc => {
                if self.inbox_detail_open() {
                    self.mode = Mode::Normal;
                    self.input.clear();
                    self.input_error = None;
                    self.answering_id = None;
                    Command::None
                } else {
                    self.cmd_snooze_answering("later — Methodus will ask again when idle")
                }
            }
            KeyCode::Char('z') if empty => {
                self.cmd_snooze_answering("later — Methodus will ask again when idle")
            }
            KeyCode::Char('d') | KeyCode::Char('x') if empty => {
                let id = self.answering_id.clone();
                self.mode = Mode::Normal;
                self.input.clear();
                self.input_error = None;
                self.answering_id = None;
                match id {
                    Some(id) => Command::DismissQuestion { id },
                    None => Command::None,
                }
            }
            KeyCode::Char('?') if empty => {
                self.show_help = true;
                Command::None
            }
            KeyCode::Enter | KeyCode::Char('\n') if wants_newline(key) => {
                self.insert_str("\n");
                Command::None
            }
            KeyCode::Enter => {
                let text = self.input.trim().to_string();
                if text.is_empty() {
                    self.input_error =
                        Some("type an answer here, then Enter — Esc = later".to_string());
                    return Command::None;
                }
                let id = self.answering_id.clone().unwrap_or_default();
                Command::AnswerQuestion { id, text }
            }
            KeyCode::Backspace => {
                self.backspace_char();
                Command::None
            }
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_str("\n");
                Command::None
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_str(&c.to_string());
                Command::None
            }
            _ if self.handle_editor_nav(key) => Command::None,
            _ => {
                self.set_status(
                    StatusLevel::Warn,
                    "type an answer · Enter submit · Esc later · d dismiss",
                );
                Command::None
            }
        }
    }

    fn cmd_snooze_answering(&mut self, why: &str) -> Command {
        let id = self.answering_id.clone();
        self.mode = Mode::Normal;
        self.input.clear();
        self.input_error = None;
        self.answering_id = None;
        self.set_status(StatusLevel::Info, why);
        match id {
            Some(id) => Command::SnoozeQuestion { id },
            None => Command::None,
        }
    }

    fn start_new_conversation(&mut self) {
        let was_busy = self.busy();
        self.event_rx = None;
        self.session_task_id = None;
        self.transcript.clear();
        self.transcript_offset = 0;
        self.input.clear();
        self.input_error = None;
        self.slash_sel = 0;
        self.mention_sel = 0;
        self.overlay = Overlay::None;
        self.knowledge_pick_id = None;
        if was_busy {
            self.set_status(
                StatusLevel::Warn,
                "cleared — previous run may still finish; next message is a new executor session",
            );
        } else {
            self.set_status(
                StatusLevel::Info,
                "cleared — next message starts a new executor session (no resume)",
            );
        }
    }

    pub fn scroll_session(&mut self, delta: isize) {
        self.scroll_transcript(delta);
    }

    fn scroll_transcript(&mut self, delta: isize) {
        if delta >= 0 {
            self.transcript_offset = self.transcript_offset.saturating_add(delta as usize);
        } else {
            self.transcript_offset = self.transcript_offset.saturating_sub((-delta) as usize);
        }
    }

    fn toggle_runtime(&mut self) {
        let mut cfg = UserConfig {
            default_runtime: self.runtime.clone(),
            permission_mode: Some(self.permission_mode.clone()),
            default_face: self.default_face.clone(),
            context_faces: if self.context_faces.is_empty() {
                None
            } else {
                Some(self.context_faces.clone())
            },
            workspace_root: if self.workspace_root.trim().is_empty() {
                None
            } else {
                Some(self.workspace_root.clone())
            },
            notifications: Some(self.notifications),
        };
        cfg.cycle_runtime();
        self.runtime = cfg.default_runtime.clone();
        if let Err(e) = self.persist_config() {
            self.set_status(StatusLevel::Error, e);
            return;
        }
        self.set_status(
            StatusLevel::Info,
            format!("runtime {}", self.runtime.as_deref().unwrap_or("-")),
        );
    }

    fn current_sel(&self) -> usize {
        match self.overlay {
            Overlay::Sessions => self.task_sel,
            Overlay::Faces => self.face_sel,
            Overlay::Inbox => self.review_sel,
            Overlay::Events | Overlay::Jobs => self.system_sel,
            Overlay::Setup => self.setup_sel,
            Overlay::None => 0,
        }
    }

    fn set_sel(&mut self, sel: usize) {
        match self.overlay {
            Overlay::Sessions => self.task_sel = sel,
            Overlay::Faces => self.face_sel = sel,
            Overlay::Inbox => {
                self.review_sel = sel;
                self.review_detail_scroll = 0;
            }
            Overlay::Events | Overlay::Jobs => self.system_sel = sel,
            Overlay::Setup => self.setup_sel = sel,
            Overlay::None => {}
        }
    }

    pub fn scroll_review_detail(&mut self, delta: isize) {
        if delta < 0 {
            self.review_detail_scroll = self.review_detail_scroll.saturating_sub((-delta) as usize);
        } else {
            self.review_detail_scroll = self.review_detail_scroll.saturating_add(delta as usize);
        }
    }

    fn move_sel(&mut self, delta: isize) {
        let vis = self.visible_overlay_indices();
        if vis.is_empty() {
            return;
        }
        let pos = vis
            .iter()
            .position(|&i| i == self.current_sel())
            .unwrap_or(0);
        let next = vis[(pos as isize + delta).rem_euclid(vis.len() as isize) as usize];
        self.set_sel(next);
    }

    fn jump_sel(&mut self, to: isize) {
        let vis = self.visible_overlay_indices();
        if vis.is_empty() {
            return;
        }
        if to == isize::MAX {
            self.set_sel(*vis.last().unwrap());
        } else {
            self.set_sel(vis[0]);
        }
    }

    fn action_task(&self) -> Option<&Task> {
        self.session_task().or_else(|| self.selected_task())
    }

    fn cmd_run(&mut self, resume: bool) -> Command {
        let Some(task) = self.action_task().cloned() else {
            return Command::None;
        };
        if matches!(
            task.status,
            TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Completed
        ) {
            if resume {
                self.set_status(
                    StatusLevel::Warn,
                    format!(
                        "{} is {} — cannot resume; ctrl-n then send a new message",
                        task.id, task.status
                    ),
                );
                return Command::None;
            }
            // Terminal tasks cannot be re-run; start a fresh task with the same request.
            self.session_task_id = None;
            self.event_rx = None;
            self.set_status(
                StatusLevel::Info,
                format!("retrying as a new task (was {})", task.status),
            );
            return Command::Send {
                task_id: None,
                text: task.request.clone(),
            };
        }
        Command::Run {
            task_id: task.id.clone(),
            resume,
        }
    }

    fn sync_slash_sel(&mut self) {
        let n = matching_slash(&self.input).len();
        if n == 0 {
            self.slash_sel = 0;
        } else {
            self.slash_sel = self.slash_sel.min(n - 1);
        }
    }

    fn complete_slash(&mut self) {
        let matches = matching_slash(&self.input);
        let Some(cmd) = matches.get(self.slash_sel) else {
            return;
        };
        let rest = slash_rest(&self.input);
        self.input = if rest.is_empty() {
            format!("/{} ", cmd.name)
        } else {
            format!("/{} {rest}", cmd.name)
        };
        self.snap_cursor_end();
        self.slash_sel = 0;
    }

    fn ensure_mention_cache(&mut self) {
        let roots = self.engine.context_roots();
        let key = mention_roots_sig(&self.projects, self.engine.launch_cwd());
        if self.mention_root_id.as_deref() == Some(key.as_str()) && !self.mention_cache.is_empty() {
            return;
        }
        self.mention_root_id = Some(key);
        self.mention_cache = list_from_roots(&roots, 2000);
        self.mention_sel = 0;
    }

    pub fn mention_menu_open(&self) -> bool {
        !slash_menu_open(&self.input) && at_query(&self.input).is_some()
    }

    pub fn mention_cache_empty(&self) -> bool {
        self.mention_cache.is_empty()
    }

    pub fn matching_mentions(&self) -> Vec<&MentionCandidate> {
        let q = at_query(&self.input).unwrap_or("");
        filter_candidates(&self.mention_cache, q)
    }

    fn sync_mention_sel(&mut self) {
        if at_query(&self.input).is_some() {
            self.ensure_mention_cache();
        }
        let n = self.matching_mentions().len();
        if n == 0 {
            self.mention_sel = 0;
        } else {
            self.mention_sel = self.mention_sel.min(n - 1);
        }
    }

    fn cancel_mention(&mut self) {
        if let Some(i) = self.input.rfind('@') {
            self.input.truncate(i);
        }
        self.snap_cursor_end();
        self.mention_sel = 0;
        self.input_error = None;
    }

    fn accept_mention(&mut self, commit: bool) {
        self.ensure_mention_cache();
        let matches: Vec<MentionCandidate> =
            self.matching_mentions().into_iter().cloned().collect();
        let Some(cand) = matches.get(self.mention_sel).cloned() else {
            self.input_error = Some(
                "no matching file — launch from a folder, or register a project in Setup"
                    .to_string(),
            );
            return;
        };
        let Some(at) = self.input.rfind('@') else {
            return;
        };
        let prefix = self.input[..at].to_string();
        let path = cand.label.trim_end_matches('/');
        self.input = if cand.is_dir && !commit {
            format!("{prefix}@{path}/")
        } else if cand.is_dir {
            format!("{prefix}@{path}/ ")
        } else {
            format!("{prefix}@{path} ")
        };
        self.snap_cursor_end();
        self.mention_sel = 0;
        self.input_error = None;
        self.sync_mention_sel();
    }

    fn dispatch_slash_input(&mut self) -> Command {
        let matches = matching_slash(&self.input);
        if matches.is_empty() {
            self.input_error =
                Some("unknown command — try /help /setup /inbox /face /quit".to_string());
            return Command::None;
        }
        let cmd = matches[self.slash_sel.min(matches.len() - 1)];
        let rest = slash_rest(&self.input);
        self.run_slash(cmd.name, rest)
    }

    fn run_slash(&mut self, name: &str, rest: String) -> Command {
        match name {
            "help" => {
                self.input.clear();
                self.slash_sel = 0;
                self.show_help = true;
                Command::None
            }
            "clear" => {
                self.start_new_conversation();
                Command::None
            }
            "quit" => {
                self.input.clear();
                Command::Quit
            }
            "setup" => {
                self.input.clear();
                self.input_cursor = 0;
                self.slash_sel = 0;
                self.overlay_filter.clear();
                self.overlay = Overlay::Setup;
                self.health = health_checks(self.engine.home());
                self.set_status(StatusLevel::Info, "setup — esc back to session");
                Command::None
            }
            "inbox" => {
                self.input.clear();
                self.input_cursor = 0;
                self.slash_sel = 0;
                self.overlay_filter.clear();
                self.overlay = Overlay::Inbox;
                self.set_status(
                    StatusLevel::Info,
                    "inbox — type to filter, enter to act, esc back",
                );
                Command::None
            }
            "session" => {
                self.input.clear();
                self.input_cursor = 0;
                self.slash_sel = 0;
                self.open_sessions();
                Command::None
            }
            "face" => {
                self.input.clear();
                self.input_cursor = 0;
                self.slash_sel = 0;
                if rest.trim().is_empty() {
                    self.overlay_filter.clear();
                    self.overlay = Overlay::Faces;
                    self.set_status(
                        StatusLevel::Info,
                        "faces — type to filter, enter to pin as default",
                    );
                } else {
                    self.pin_face_by_query(rest.trim());
                }
                Command::None
            }
            "cancel" => {
                self.input.clear();
                self.slash_sel = 0;
                self.begin_cancel()
            }
            "delete" => {
                self.input.clear();
                self.slash_sel = 0;
                self.begin_delete()
            }
            "retry" => {
                self.input.clear();
                self.slash_sel = 0;
                self.cmd_run(false)
            }
            "study" => {
                self.input.clear();
                self.slash_sel = 0;
                if self.busy() {
                    self.input_error = Some("wait until this turn ends".to_string());
                    return Command::None;
                }
                let (scope, sources) = methodus_core::curiosity::parse_study_invocation(rest.trim());
                if sources.is_empty() {
                    self.input_error = Some(
                        "/study needs sources — e.g. /study nxm @~/docs/nxm https://wiki.example.com/x".to_string(),
                    );
                    return Command::None;
                }
                Command::StudyModule {
                    scope,
                    sources,
                    face: self.default_face.clone(),
                }
            }
            "ingest" => {
                self.input.clear();
                self.slash_sel = 0;
                if self.busy() {
                    self.input_error = Some("wait until this turn ends".to_string());
                    return Command::None;
                }
                let (_, sources) = methodus_core::curiosity::parse_study_invocation(rest.trim());
                if sources.is_empty() {
                    self.input_error = Some(
                        "/ingest needs sources — e.g. /ingest @~/docs/standard.pdf".to_string(),
                    );
                    return Command::None;
                }
                Command::IngestDocs { sources }
            }
            "survey" => {
                self.input.clear();
                self.slash_sel = 0;
                if self.busy() {
                    self.input_error = Some("wait until this turn ends".to_string());
                    return Command::None;
                }
                Command::SurveyRepo
            }
            "cleanup" => {
                self.input.clear();
                self.slash_sel = 0;
                let days = rest
                    .trim()
                    .parse::<i64>()
                    .unwrap_or(30)
                    .clamp(1, 3650);
                Command::CleanupWorkspaces { max_age_days: days }
            }
            "events" => {
                self.input.clear();
                self.slash_sel = 0;
                self.overlay_filter.clear();
                self.refresh_system_lists();
                self.system_sel = 0;
                self.overlay = Overlay::Events;
                self.set_status(StatusLevel::Info, "events — esc back");
                Command::None
            }
            "jobs" => {
                self.input.clear();
                self.slash_sel = 0;
                self.overlay_filter.clear();
                self.refresh_system_lists();
                self.system_sel = 0;
                self.overlay = Overlay::Jobs;
                self.set_status(StatusLevel::Info, "jobs — [c] cancel selected · esc back");
                Command::None
            }
            _ => {
                self.input_error = Some("unknown command".to_string());
                Command::None
            }
        }
    }

    fn cmd_send(&mut self) -> Command {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            self.input_error = Some("type a message, then Enter".to_string());
            return Command::None;
        }
        if text.starts_with('/') {
            return self.dispatch_slash_input();
        }
        if self.busy() {
            self.input_error = Some("wait until this turn ends".to_string());
            return Command::None;
        }
        if let Some(task) = self.session_task() {
            if self.approvals.iter().any(|a| a.task_id == task.id) {
                self.input_error = Some("approve or deny first".to_string());
                return Command::None;
            }
            if matches!(
                task.status,
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
            ) {
                return Command::Send {
                    task_id: None,
                    text,
                };
            }
            return Command::Send {
                task_id: Some(task.id.clone()),
                text,
            };
        }
        Command::Send {
            task_id: None,
            text,
        }
    }

    pub fn selected_review(&self) -> Option<ReviewItem<'_>> {
        let i = self.review_sel;
        if i < self.questions.len() {
            return self.questions.get(i).map(ReviewItem::Question);
        }
        let j = i - self.questions.len();
        if j < self.knowledge.len() {
            return self.knowledge.get(j).map(ReviewItem::Knowledge);
        }
        let h = j - self.knowledge.len();
        if h < self.hypotheses.len() {
            return self.hypotheses.get(h).map(ReviewItem::Hypothesis);
        }
        let k = h - self.hypotheses.len();
        if k < self.evolutions.len() {
            return self.evolutions.get(k).map(ReviewItem::Evolution);
        }
        self.experiences
            .get(k - self.evolutions.len())
            .map(ReviewItem::Experience)
    }

    pub fn answering_question(&self) -> Option<&Question> {
        let id = self.answering_id.as_deref()?;
        self.questions.iter().find(|q| q.id == id)
    }

    pub fn review_summary(&self) -> String {
        match self.selected_review() {
            Some(ReviewItem::Question(q)) => {
                let reason = q.reason.as_deref().unwrap_or("-");
                let mentor = reason.starts_with("mentor:");
                let kind = if mentor { "mentor question" } else { "question" };
                format!(
                    "{kind} · {}\n\n{}\n\nreason: {reason}\nstatus: {}  value: {:.1}\n\nEnter → read & answer",
                    if mentor { "for you (domain expert)" } else { "idle ask" },
                    ellipsize(&q.question, 160),
                    q.status,
                    q.value
                )
            }
            Some(ReviewItem::Knowledge(k)) => {
                let kind = if k.source == methodus_core::learning::SKILL_DRAFT_SOURCE {
                    "skill draft"
                } else {
                    "knowledge"
                };
                let mut head = format!("{kind} · {status}\n{path}\n", status = k.status, path = k.path);
                if k.status == KnowledgeStatus::Conflicted {
                    head.push_str("conflicts with committed version\n");
                }
                let body = std::fs::read_to_string(self.engine.home().join(&k.path)).unwrap_or_default();
                let snippet = body
                    .split("---")
                    .nth(2)
                    .unwrap_or(body.as_str())
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .take(4)
                    .collect::<Vec<_>>()
                    .join("\n");
                if !snippet.is_empty() {
                    head.push_str(&ellipsize(&snippet, 200));
                    head.push('\n');
                }
                head.push_str("\nEnter → full view & decide");
                head
            }
            Some(ReviewItem::Hypothesis(h)) => {
                format!(
                    "hypothesis · {}\n{}\n\nEnter → read & promote / validate / reject",
                    h.status,
                    ellipsize(&h.path, 160)
                )
            }
            Some(ReviewItem::Evolution(e)) => {
                format!(
                    "face evolution · {}\ntarget: face `{}`\n\n{}\n\nEnter → diff & approve",
                    e.status,
                    e.target_id,
                    ellipsize(e.rationale.as_deref().unwrap_or("-"), 160)
                )
            }
            Some(ReviewItem::Experience(e)) => {
                format!(
                    "outcome: {}\ntask: {}\n{}\n\nEnter → full log",
                    e.outcome.as_deref().unwrap_or("-"),
                    e.task_id,
                    ellipsize(e.summary.as_deref().unwrap_or(&e.id), 180)
                )
            }
            None => String::new(),
        }
    }

    pub fn review_detail(&self) -> String {
        match self.selected_review() {
            Some(ReviewItem::Question(q)) => {
                let reason = q.reason.as_deref().unwrap_or("-");
                format!(
                    "{}\n\nreason  {reason}\nstatus  {}\nvalue   {:.2}\nfreq    {:.0}\n",
                    q.question, q.status, q.value, q.frequency
                )
            }
            Some(ReviewItem::Knowledge(k)) => self.knowledge_review_body(k),
            Some(ReviewItem::Hypothesis(h)) => std::fs::read_to_string(self.engine.home().join(&h.path))
                .unwrap_or_else(|_| format!("(missing file)\n{}", h.path)),
            Some(ReviewItem::Evolution(e)) => {
                methodus_core::evolution::format_evolution_detail(e)
            }
            Some(ReviewItem::Experience(e)) => std::fs::read_to_string(self.engine.home().join(&e.path))
                .unwrap_or_else(|_| {
                    e.summary
                        .clone()
                        .unwrap_or_else(|| e.path.clone())
                }),
            None => String::new(),
        }
    }

    fn knowledge_review_body(&self, k: &KnowledgeItem) -> String {
        let home = self.engine.home();
        let candidate = std::fs::read_to_string(home.join(&k.path))
            .unwrap_or_else(|_| format!("(missing file)\n{}", k.path));
        if k.status != KnowledgeStatus::Conflicted {
            return candidate;
        }
        let mut out = String::from("## Conflict\n\nThis candidate differs from committed knowledge.\n\n");
        if let Some(ref cid) = k.conflict_of {
            if let Ok(Some(existing)) = self.engine.store().get_knowledge(cid) {
                let existing_body = std::fs::read_to_string(home.join(&existing.path))
                    .unwrap_or_else(|_| format!("{}\n", existing.path));
                out.push_str("### Existing (committed)\n\n");
                out.push_str(&existing_body);
                out.push_str("\n\n---\n\n");
            }
        }
        out.push_str("### Candidate (this item)\n\n");
        out.push_str(&candidate);
        if k.source == methodus_core::learning::SKILL_DRAFT_SOURCE {
            out.push_str("\n\n---\n\n**Resolve:** choose in the composer below\n");
        } else {
            out.push_str("\n\n---\n\n**Resolve:** choose in the composer below\n");
        }
        out
    }

    fn cmd_review(&self, positive: bool) -> Command {
        match self.selected_review() {
            Some(ReviewItem::Hypothesis(h)) if positive => Command::ReviewHypothesis {
                id: h.id.clone(),
                action: HypothesisReviewAction::Promote,
            },
            Some(ReviewItem::Evolution(e)) => Command::ReviewEvolution {
                id: e.id.clone(),
                approve: positive,
            },
            Some(ReviewItem::Knowledge(k)) => {
                let action = if positive {
                    if k.source == methodus_core::learning::SKILL_DRAFT_SOURCE
                        && k.status == KnowledgeStatus::Conflicted
                    {
                        KnowledgeReviewAction::ReplaceExisting
                    } else {
                        KnowledgeReviewAction::Commit
                    }
                } else {
                    KnowledgeReviewAction::Reject
                };
                Command::ReviewKnowledge {
                    id: k.id.clone(),
                    action,
                }
            }
            Some(ReviewItem::Experience(e)) if positive => {
                if self
                    .tasks
                    .iter()
                    .any(|t| t.id == e.task_id && t.status == TaskStatus::Reviewing)
                {
                    Command::CompleteReview {
                        task_id: e.task_id.clone(),
                    }
                } else {
                    Command::None
                }
            }
            _ => Command::None,
        }
    }

    fn cmd_review_negative(&self) -> Command {
        match self.selected_review() {
            Some(ReviewItem::Hypothesis(h)) => Command::ReviewHypothesis {
                id: h.id.clone(),
                action: HypothesisReviewAction::Reject,
            },
            Some(ReviewItem::Evolution(e)) => Command::ReviewEvolution {
                id: e.id.clone(),
                approve: false,
            },
            Some(ReviewItem::Knowledge(k)) => Command::ReviewKnowledge {
                id: k.id.clone(),
                action: KnowledgeReviewAction::Reject,
            },
            Some(ReviewItem::Question(q)) => Command::DismissQuestion { id: q.id.clone() },
            Some(ReviewItem::Experience(_)) | None => Command::None,
        }
    }

    fn cmd_snooze(&self) -> Command {
        match self.selected_review() {
            Some(ReviewItem::Question(q)) => Command::SnoozeQuestion { id: q.id.clone() },
            _ => Command::None,
        }
    }

    fn begin_cancel(&mut self) -> Command {
        let Some(task) = self.task_for_list_action() else {
            self.input_error = Some("open a conversation first, then /cancel".to_string());
            return Command::None;
        };
        if task.status.is_terminal() {
            self.set_status(
                StatusLevel::Info,
                format!("{} is already {} — d to delete", task.id, task.status),
            );
            return Command::None;
        }
        let id = task.id.clone();
        self.mode = Mode::ConfirmCancel;
        self.confirm_delete = false;
        self.confirm_task_id = Some(id.clone());
        self.set_status(StatusLevel::Warn, format!("cancel {id}? [y]es  [n]/esc no"));
        Command::None
    }

    fn begin_delete(&mut self) -> Command {
        let Some(task) = self.task_for_list_action() else {
            self.input_error = Some("open a conversation first, then /delete".to_string());
            return Command::None;
        };
        if !task.status.is_terminal() {
            self.set_status(
                StatusLevel::Warn,
                format!(
                    "{} is {} — c to cancel it first, then d to delete",
                    task.id, task.status
                ),
            );
            return Command::None;
        }
        let id = task.id.clone();
        let status = task.status.clone();
        self.mode = Mode::ConfirmCancel;
        self.confirm_delete = true;
        self.confirm_task_id = Some(id.clone());
        self.set_status(
            StatusLevel::Warn,
            format!("delete {id} ({status})? [y]es  [n]/esc no"),
        );
        Command::None
    }

    fn task_for_list_action(&self) -> Option<&Task> {
        if self.overlay == Overlay::Sessions {
            self.selected_task()
        } else {
            self.action_task()
        }
    }

    fn cmd_approve(&self, decision: ApprovalDecision) -> Command {
        match self.pending_approval() {
            Some(a) => Command::Approve {
                id: a.id.clone(),
                decision,
            },
            None => Command::None,
        }
    }

    /// Pending permission for the open conversation (shown in the composer).
    pub fn session_approvals(&self) -> Vec<&Approval> {
        let Some(id) = self.session_task_id.as_deref() else {
            return Vec::new();
        };
        self.approvals.iter().filter(|a| a.task_id == id).collect()
    }

    pub fn pending_approval(&self) -> Option<&Approval> {
        let list = self.session_approvals();
        if list.is_empty() {
            return None;
        }
        Some(list[self.approval_sel.min(list.len() - 1)])
    }

    fn handle_approval_key(&mut self, key: KeyEvent) -> Command {
        let n = APPROVAL_CHOICES.len();
        match key.code {
            KeyCode::Esc => {
                self.set_status(
                    StatusLevel::Info,
                    "permission still waiting — 1 yes / 2 session / 3 no / 4 abort",
                );
                Command::None
            }
            KeyCode::Tab | KeyCode::BackTab => Command::None,
            KeyCode::Up | KeyCode::Char('k') => {
                self.approval_choice = self.approval_choice.saturating_sub(1);
                Command::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if n > 0 {
                    self.approval_choice = (self.approval_choice + 1).min(n - 1);
                }
                Command::None
            }
            KeyCode::Left | KeyCode::Char('[') => {
                let m = self.session_approvals().len();
                if m > 0 {
                    self.approval_sel = self.approval_sel.saturating_sub(1).min(m - 1);
                }
                Command::None
            }
            KeyCode::Right | KeyCode::Char(']') => {
                let m = self.session_approvals().len();
                if m > 1 {
                    self.approval_sel = (self.approval_sel + 1).min(m - 1);
                }
                Command::None
            }
            KeyCode::Enter => {
                let i = self.approval_choice.min(n.saturating_sub(1));
                self.cmd_approve(APPROVAL_CHOICES[i].decision)
            }
            KeyCode::Char('1') => self.cmd_approve(ApprovalDecision::Once),
            KeyCode::Char('2') => self.cmd_approve(ApprovalDecision::Session),
            KeyCode::Char('3') | KeyCode::Char('n') => self.cmd_approve(ApprovalDecision::Deny),
            KeyCode::Char('4') => self.cmd_approve(ApprovalDecision::Abort),
            KeyCode::Char('y') => self.cmd_approve(ApprovalDecision::Once),
            KeyCode::Char('s') => self.cmd_approve(ApprovalDecision::Session),
            KeyCode::Char('d') => self.cmd_approve(ApprovalDecision::Deny),
            KeyCode::Char('x') => self.cmd_approve(ApprovalDecision::Abort),
            _ => Command::None,
        }
    }
}

pub fn format_event(event: &RuntimeEvent) -> Vec<ChatLine> {
    match event {
        RuntimeEvent::SessionStarted { session_id } => {
            vec![line(ChatKind::Meta, format!("session {session_id}"))]
        }
        RuntimeEvent::UserText { text } => blob(ChatKind::You, text),
        RuntimeEvent::AssistantText { text } => blob(ChatKind::Assistant, text),
        RuntimeEvent::Thinking { .. } => Vec::new(),
        RuntimeEvent::ToolCallStarted { name, input, .. } => tool_started_lines(name, input),
        RuntimeEvent::ToolCallCompleted {
            output, exit_code, ..
        } => tool_completed_lines(output, exit_code),
        RuntimeEvent::TurnCompleted { stop_reason } => {
            vec![line(
                ChatKind::Meta,
                format!("turn ({})", stop_reason.as_deref().unwrap_or("end")),
            )]
        }
        RuntimeEvent::Result {
            is_error: true,
            text,
            cost_usd,
            usage,
            ..
        } => {
            let kind = ChatKind::Alert;
            let body = if text.trim().is_empty() {
                "executor failed (no message) — press r to retry, or ctrl-n for a new session"
                    .to_string()
            } else {
                text.clone()
            };
            let mut lines = vec![line(kind, body)];
            let delta = UsageDelta::from_result(*cost_usd, usage.as_ref());
            if !delta.is_empty() {
                lines.push(line(ChatKind::Meta, format!("usage  {}", delta.compact())));
            }
            lines
        }
        RuntimeEvent::Result {
            is_error: false,
            cost_usd,
            usage,
            ..
        } => {
            let delta = UsageDelta::from_result(*cost_usd, usage.as_ref());
            if delta.is_empty() {
                Vec::new()
            } else {
                vec![line(
                    ChatKind::Meta,
                    format!("usage  {}", delta.compact()),
                )]
            }
        }
        RuntimeEvent::ApprovalRequested {
            tool_name,
            input,
            ..
        } => vec![
            line(ChatKind::Alert, format!("permission  {tool_name}")),
            line(ChatKind::Alert, compact_json(input)),
        ],
        RuntimeEvent::Error { message } => vec![line(ChatKind::Alert, message.clone())],
    }
}

fn line(kind: ChatKind, text: String) -> ChatLine {
    ChatLine { kind, text }
}

fn blob(kind: ChatKind, text: &str) -> Vec<ChatLine> {
    if text.is_empty() {
        return Vec::new();
    }
    vec![ChatLine {
        kind,
        text: text.to_string(),
    }]
}

fn append_transcript_event(transcript: &mut Vec<ChatLine>, event: &RuntimeEvent) {
    for line in format_event(event) {
        append_transcript_line(transcript, line);
    }
}

fn append_transcript_line(transcript: &mut Vec<ChatLine>, line: ChatLine) {
    if line.kind == ChatKind::Assistant {
        if let Some(last) = transcript.last_mut() {
            if last.kind == ChatKind::Assistant {
                coalesce_assistant_text(&mut last.text, &line.text);
                return;
            }
        }
    }
    transcript.push(line);
}

fn coalesce_assistant_text(acc: &mut String, chunk: &str) {
    if chunk.is_empty() {
        return;
    }
    if acc.is_empty() {
        *acc = chunk.to_string();
        return;
    }
    if chunk.starts_with(acc.as_str()) {
        *acc = chunk.to_string();
        return;
    }
    if acc.starts_with(chunk) {
        return;
    }
    if !acc.ends_with(chunk) {
        acc.push_str(chunk);
    }
}

fn compact_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn tool_started_lines(name: &str, input: &serde_json::Value) -> Vec<ChatLine> {
    let summary = tool_input_summary(input);
    let mut out = vec![line(
        ChatKind::Tool,
        format!("→ {name}  {summary}"),
    )];
    for snippet in tool_diff_lines(input).into_iter().take(8) {
        out.push(line(ChatKind::Tool, snippet));
    }
    out
}

fn tool_completed_lines(output: &serde_json::Value, exit_code: &Option<i32>) -> Vec<ChatLine> {
    let code = exit_code.unwrap_or(0);
    let head = if code == 0 {
        "← ok".to_string()
    } else {
        format!("← exit {code}")
    };
    let preview = tool_output_preview(output);
    if preview.is_empty() {
        vec![line(ChatKind::Tool, head)]
    } else {
        vec![line(ChatKind::Tool, format!("{head}  {preview}"))]
    }
}

fn tool_input_summary(value: &serde_json::Value) -> String {
    if let Some(obj) = value.as_object() {
        if let Some(p) = obj
            .get("file_path")
            .or_else(|| obj.get("path"))
            .and_then(|v| v.as_str())
        {
            return ellipsize(p, 72);
        }
        if let Some(cmd) = obj.get("command").and_then(|v| v.as_str()) {
            return ellipsize(cmd, 72);
        }
        if let Some(q) = obj
            .get("query")
            .or_else(|| obj.get("pattern"))
            .and_then(|v| v.as_str())
        {
            return ellipsize(q, 72);
        }
    }
    ellipsize(&compact_json(value), 72)
}

fn tool_diff_lines(value: &serde_json::Value) -> Vec<String> {
    let Some(obj) = value.as_object() else {
        return Vec::new();
    };
    let old = obj.get("old_string").and_then(|v| v.as_str());
    let new = obj.get("new_string").and_then(|v| v.as_str());
    match (old, new) {
        (Some(o), Some(n)) => {
            let mut lines = Vec::new();
            for l in o.lines().take(4) {
                lines.push(format!("- {l}"));
            }
            for l in n.lines().take(4) {
                lines.push(format!("+ {l}"));
            }
            lines
        }
        _ => Vec::new(),
    }
}

fn tool_output_preview(value: &serde_json::Value) -> String {
    let s = compact_json(value);
    let s = s.trim();
    if s.is_empty() || s == "null" {
        return String::new();
    }
    ellipsize(s.lines().next().unwrap_or(""), 80)
}

fn overlay_reserved_char(overlay: Overlay, c: char) -> bool {
    match overlay {
        Overlay::Sessions => matches!(c, 'j' | 'k' | 'g' | 'G' | '?' | 'd' | 'x' | 'c'),
        Overlay::Faces => matches!(c, 'j' | 'k' | '?'),
        Overlay::Inbox => matches!(c, 'j' | 'k' | 'g' | 'G' | '?' | 'y' | 'd' | 'x' | 'z'),
        Overlay::Events | Overlay::Jobs => matches!(c, 'j' | 'k' | 'r' | 'c' | '?'),
        Overlay::Setup | Overlay::None => true,
    }
}

fn scored_indices(needle: &str, n: usize, hay: impl Fn(usize) -> String) -> Vec<usize> {
    if needle.is_empty() {
        return (0..n).collect();
    }
    let mut ranked: Vec<(i32, usize)> = (0..n)
        .filter_map(|i| fuzzy_score(needle, &hay(i)).map(|s| (s, i)))
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    ranked.into_iter().map(|(_, i)| i).collect()
}

fn floor_char_boundary(s: &str, byte_idx: usize) -> usize {
    let mut i = byte_idx.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn prev_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.saturating_sub(1);
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.saturating_add(1);
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i.min(s.len())
}

pub fn status_counts(tasks: &[Task]) -> (usize, usize, usize, usize) {
    let mut queued = 0;
    let mut running = 0;
    let mut waiting = 0;
    let mut done = 0;
    for t in tasks {
        match t.status {
            TaskStatus::Queued | TaskStatus::Planning => queued += 1,
            TaskStatus::Running => running += 1,
            TaskStatus::WaitingUser | TaskStatus::Reviewing => waiting += 1,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled => done += 1,
        }
    }
    (queued, running, waiting, done)
}

pub fn ellipsize(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    if max <= 3 {
        return s.chars().take(max).collect();
    }
    let take = max - 3;
    let mut out: String = s.chars().take(take).collect();
    out.push_str("...");
    out
}

fn is_ctrl_c(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
}

fn wants_newline(key: KeyEvent) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('j') {
        return true;
    }
    let enter = matches!(key.code, KeyCode::Enter | KeyCode::Char('\n'));
    enter && key.modifiers.contains(KeyModifiers::SHIFT)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApprovalChoice {
    pub decision: ApprovalDecision,
    pub key: &'static str,
    pub label: &'static str,
}

pub const APPROVAL_CHOICES: &[ApprovalChoice] = &[
    ApprovalChoice {
        decision: ApprovalDecision::Once,
        key: "1",
        label: "Yes",
    },
    ApprovalChoice {
        decision: ApprovalDecision::Session,
        key: "2",
        label: "Yes, don't ask again this session",
    },
    ApprovalChoice {
        decision: ApprovalDecision::Deny,
        key: "3",
        label: "No — tell the agent what to do",
    },
    ApprovalChoice {
        decision: ApprovalDecision::Abort,
        key: "4",
        label: "Abort this task",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnowledgeChoice {
    pub action: KnowledgeReviewAction,
    pub key: &'static str,
    pub label: &'static str,
}

pub const INBOX_QUESTION_CHOICES: &[(&str, &str)] = &[
    ("1", "Answer"),
    ("2", "Later (snooze)"),
    ("3", "Dismiss"),
    ("4", "Back to list"),
];

pub const INBOX_EXPERIENCE_CHOICES: &[(&str, &str)] = &[
    ("1", "Mark review done"),
    ("2", "Back to list"),
];

pub const KNOWLEDGE_CHOICES: &[KnowledgeChoice] = &[
    KnowledgeChoice {
        action: KnowledgeReviewAction::Commit,
        key: "1",
        label: "Yes — commit (install skill / keep knowledge)",
    },
    KnowledgeChoice {
        action: KnowledgeReviewAction::Reject,
        key: "2",
        label: "No — reject",
    },
];

pub const KNOWLEDGE_CONFLICT_CHOICES: &[KnowledgeChoice] = &[
    KnowledgeChoice {
        action: KnowledgeReviewAction::ReplaceExisting,
        key: "1",
        label: "Replace existing skill with this draft",
    },
    KnowledgeChoice {
        action: KnowledgeReviewAction::Reject,
        key: "2",
        label: "Reject draft",
    },
];

pub fn knowledge_choices_for(item: &KnowledgeItem) -> &'static [KnowledgeChoice] {
    if item.source == methodus_core::learning::SKILL_DRAFT_SOURCE
        && item.status == KnowledgeStatus::Conflicted
    {
        KNOWLEDGE_CONFLICT_CHOICES
    } else {
        KNOWLEDGE_CHOICES
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashCmd {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub summary: &'static str,
}

pub const SLASH_COMMANDS: &[SlashCmd] = &[
    SlashCmd {
        name: "cancel",
        aliases: &[],
        summary: "cancel the open conversation",
    },
    SlashCmd {
        name: "clear",
        aliases: &["new"],
        summary: "new conversation — next turn is a fresh executor session",
    },
    SlashCmd {
        name: "delete",
        aliases: &[],
        summary: "delete a finished conversation",
    },
    SlashCmd {
        name: "face",
        aliases: &[],
        summary: "pin a Face, or open the Face list",
    },
    SlashCmd {
        name: "help",
        aliases: &[],
        summary: "keyboard and commands",
    },
    SlashCmd {
        name: "cleanup",
        aliases: &[],
        summary: "remove old task workspace dirs (default 30 days)",
    },
    SlashCmd {
        name: "events",
        aliases: &[],
        summary: "recent audit events",
    },
    SlashCmd {
        name: "jobs",
        aliases: &[],
        summary: "learning queue — cancel with [c] in overlay",
    },
    SlashCmd {
        name: "ingest",
        aliases: &[],
        summary: "ingest docs into focus project knowledge",
    },
    SlashCmd {
        name: "inbox",
        aliases: &[],
        summary: "questions, candidate knowledge, experience",
    },
    SlashCmd {
        name: "survey",
        aliases: &[],
        summary: "survey focus project repo layout into project notes",
    },
    SlashCmd {
        name: "study",
        aliases: &[],
        summary: "module expert study — paths/URLs → knowledge, skill, mentor Qs",
    },
    SlashCmd {
        name: "quit",
        aliases: &["exit"],
        summary: "quit Methodus",
    },
    SlashCmd {
        name: "retry",
        aliases: &[],
        summary: "retry the open conversation",
    },
    SlashCmd {
        name: "session",
        aliases: &[],
        summary: "conversations overlay (also Tab)",
    },
    SlashCmd {
        name: "setup",
        aliases: &[],
        summary: "runtime, projects, packs",
    },
];

fn mention_roots_sig(projects: &[ProjectInfo], cwd: &Path) -> String {
    let mut s = cwd.display().to_string();
    for p in projects {
        s.push('|');
        s.push_str(&p.id);
        s.push(':');
        s.push_str(&p.root.display().to_string());
    }
    s
}

pub fn slash_menu_open(input: &str) -> bool {
    input.trim_start().starts_with('/')
}

fn slash_token(input: &str) -> Option<String> {
    let t = input.trim_start();
    let rest = t.strip_prefix('/')?;
    let token = rest.split_whitespace().next().unwrap_or("");
    Some(token.to_ascii_lowercase())
}

pub fn matching_slash(input: &str) -> Vec<&'static SlashCmd> {
    let Some(token) = slash_token(input) else {
        return Vec::new();
    };
    SLASH_COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(&token) || c.aliases.iter().any(|a| a.starts_with(&token)))
        .collect()
}

fn slash_rest(input: &str) -> String {
    let t = input.trim_start();
    let Some(rest) = t.strip_prefix('/') else {
        return String::new();
    };
    rest.split_once(char::is_whitespace)
        .map(|(_, r)| r.trim().to_string())
        .unwrap_or_default()
}

const CTRL_C_QUIT_WINDOW: Duration = Duration::from_secs(3);

fn ctrl_c_should_quit(pending: Option<Instant>, now: Instant) -> bool {
    pending.is_some_and(|t| now.duration_since(t) <= CTRL_C_QUIT_WINDOW)
}

/// Context-sensitive footer. Only what is actionable right now.
pub fn help_line(app: &App) -> String {
    if app.show_help {
        return " [esc/?] close help   [ctrl-c twice]quit ".to_string();
    }
    match app.mode {
        Mode::Answering => " [enter] submit   [esc] later   [d]ismiss ".to_string(),
        Mode::Prompt => " [enter] save path   [esc] cancel ".to_string(),
        Mode::ConfirmCancel => {
            if app.confirm_delete {
                " [y]es delete task   [n]/[esc] keep ".to_string()
            } else {
                " [y]es cancel task   [n]/[esc] keep ".to_string()
            }
        }
        Mode::Normal => match app.overlay {
            Overlay::Setup => {
                " [tab]section [enter]set [a]dd path [d]rop [space]toggle pack  [esc]session "
                    .to_string()
            }
            Overlay::Inbox if app.inbox_detail_open() => {
                if app.mode == Mode::Answering {
                    " [enter]submit  [esc]menu  scroll [[]/wheel] ".to_string()
                } else if app.pending_knowledge().is_some() {
                    " [↑↓]choose  [enter]  [y]/[d]  scroll [[]/wheel]  [esc]list ".to_string()
                } else if app.inbox_question_menu() {
                    " [↑↓]choose  [enter]  [1-4]  scroll [[]/wheel]  [esc]list ".to_string()
                } else if app.inbox_experience_menu() {
                    " [enter]/[1] mark done  [2]/[esc] back ".to_string()
                } else {
                    " scroll [[]/wheel]  [esc]list ".to_string()
                }
            }
            Overlay::Inbox => match app.selected_review() {
                Some(_) => {
                    " [↑↓]select  [enter]full view  type filter  [esc]session ".to_string()
                }
                None => " type to filter  [esc]session  [?]help ".to_string(),
            },
            Overlay::Faces => {
                " [enter] pin  type to filter  [esc] session ".to_string()
            }
            Overlay::Sessions => {
                " [enter] open  [d]elete  [c]ancel  type to filter  [tab]/[esc] back "
                    .to_string()
            }
            Overlay::Events => {
                " [↑↓]select  [r]efresh  [esc]session ".to_string()
            }
            Overlay::Jobs => {
                " [↑↓]select  [c] cancel job  [r]efresh  [esc]session ".to_string()
            }
            Overlay::None => {
                if app.pending_approval().is_some() {
                    let n = app.session_approvals().len();
                    format!(
                        " !{n} pending approval · [↑↓]choose [enter] [1]yes [2]session [3]no [4]abort "
                    )
                } else if app.pending_knowledge().is_some() {
                    " [↑↓]choose  [enter]  [y]commit  [d]reject  [esc]later ".to_string()
                } else {
                    " [enter]send  [shift-enter]newline  [/]cmds  [@]files  [tab]sessions  [?]help "
                        .to_string()
                }
            }
        },
    }
}

fn prompt_hint(kind: PromptKind) -> &'static str {
    match kind {
        PromptKind::AddProject => "paste a project directory path",
        PromptKind::AddPack => "paste a pack folder (must contain pack.yaml)",
        PromptKind::WorkspaceRoot => "workspace root (empty = ~/.methodus/workspaces)",
    }
}

pub enum ReviewItem<'a> {
    Question(&'a Question),
    Knowledge(&'a KnowledgeItem),
    Hypothesis(&'a Hypothesis),
    Evolution(&'a EvolutionCandidate),
    Experience(&'a Experience),
}

pub const INBOX_HYPOTHESIS_CHOICES: &[(&str, &str)] = &[
    ("1", "Promote → knowledge candidate"),
    ("2", "Validate — keep as hypothesis"),
    ("3", "Reject"),
];

pub const INBOX_EVOLUTION_CHOICES: &[(&str, &str)] = &[
    ("1", "Yes — apply face.yaml updates"),
    ("2", "No — reject"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    #[test]
    fn slash_commands_cover_overlays() {
        let names: Vec<_> = matching_slash("/")
            .iter()
            .map(|c| c.name)
            .collect();
        for need in ["setup", "inbox", "face", "session", "clear", "help", "quit", "study", "ingest", "survey", "cleanup"] {
            assert!(names.contains(&need), "missing /{need} in {names:?}");
        }
        assert_eq!(
            matching_slash("/setup")
                .iter()
                .map(|c| c.name)
                .collect::<Vec<_>>(),
            vec!["setup"]
        );
        assert_eq!(
            matching_slash("/in")
                .iter()
                .map(|c| c.name)
                .collect::<Vec<_>>(),
            vec!["ingest", "inbox"]
        );
        assert_eq!(
            matching_slash("/new")
                .iter()
                .map(|c| c.name)
                .collect::<Vec<_>>(),
            vec!["clear"]
        );
    }

    #[test]
    fn help_line_session_mentions_send() {
        let line = " [enter]send  [shift-enter]newline  [/]cmds  [@]files  [tab]sessions  [?]help ";
        assert!(line.contains("[enter]send"));
        assert!(line.contains("[/]cmds"));
        assert!(line.contains("[@]files"));
        assert!(line.contains("[shift-enter]newline"));
        assert!(!line.contains("[q]uit"));
        assert!(!line.contains("1-5 pages"));
    }

    #[test]
    fn shift_enter_is_newline_plain_enter_is_not() {
        assert!(wants_newline(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::SHIFT
        )));
        assert!(!wants_newline(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE
        )));
        assert!(wants_newline(KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::CONTROL
        )));
    }

    #[test]
    fn format_result_empty_error_is_visible() {
        let lines = format_event(&RuntimeEvent::Result {
            is_error: true,
            text: String::new(),
            cost_usd: None,
            usage: None,
            session_id: None,
            permission_denials: Vec::new(),
        });
        assert!(!lines.is_empty());
        assert_eq!(lines[0].kind, ChatKind::Alert);
        assert!(lines[0].text.contains("executor failed"));
    }

    #[test]
    fn format_success_result_skips_duplicate_body() {
        let lines = format_event(&RuntimeEvent::Result {
            is_error: false,
            text: "same as assistant".to_string(),
            cost_usd: Some(0.01),
            usage: Some(serde_json::json!({"input_tokens": 1, "output_tokens": 2})),
            session_id: None,
            permission_denials: Vec::new(),
        });
        assert!(lines.iter().all(|l| l.kind == ChatKind::Meta));
        assert!(lines[0].text.starts_with("usage"));
        assert!(!lines.iter().any(|l| l.text.contains("same as assistant")));
    }

    #[test]
    fn thinking_is_hidden_from_transcript() {
        assert!(format_event(&RuntimeEvent::Thinking {
            text: "internal".into()
        })
        .is_empty());
    }

    #[test]
    fn assistant_stream_coalesces_cumulative_chunks() {
        let mut t = Vec::new();
        append_transcript_event(
            &mut t,
            &RuntimeEvent::AssistantText {
                text: "hel".into(),
            },
        );
        append_transcript_event(
            &mut t,
            &RuntimeEvent::AssistantText {
                text: "hello".into(),
            },
        );
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].text, "hello");
    }

    #[test]
    fn tool_start_shows_path_and_diff_snippet() {
        let input = serde_json::json!({
            "file_path": "src/main.rs",
            "old_string": "fn a() {}",
            "new_string": "fn a() { 1 }"
        });
        let lines = format_event(&RuntimeEvent::ToolCallStarted {
            id: "1".into(),
            name: "Edit".into(),
            input,
        });
        assert!(lines[0].text.contains("Edit"));
        assert!(lines[0].text.contains("src/main.rs"));
        assert!(lines.iter().any(|l| l.text.starts_with('-')));
        assert!(lines.iter().any(|l| l.text.starts_with('+')));
    }

    #[test]
    fn tool_done_shows_exit_and_preview() {
        let lines = format_event(&RuntimeEvent::ToolCallCompleted {
            id: "1".into(),
            output: serde_json::json!("wrote 12 lines"),
            exit_code: Some(0),
        });
        assert_eq!(lines.len(), 1);
        assert!(lines[0].text.contains("ok"));
        assert!(lines[0].text.contains("wrote 12 lines"));
    }

    #[test]
    fn quit_needs_slash_or_double_ctrl_c() {
        assert_eq!(
            matching_slash("/quit")
                .iter()
                .map(|c| c.name)
                .collect::<Vec<_>>(),
            vec!["quit"]
        );
        assert_eq!(
            matching_slash("  /Exit  ")
                .iter()
                .map(|c| c.name)
                .collect::<Vec<_>>(),
            vec!["quit"]
        );
        assert!(matching_slash("quit").is_empty());
        let now = Instant::now();
        assert!(!ctrl_c_should_quit(None, now));
        assert!(ctrl_c_should_quit(Some(now), now));
        assert!(!ctrl_c_should_quit(Some(now - Duration::from_secs(4)), now));
        let k = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(k.kind, KeyEventKind::Press);
        assert!(!is_ctrl_c(k));
        assert!(is_ctrl_c(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
    }

    #[test]
    fn slash_palette_filters_and_keeps_unknown_out() {
        assert_eq!(matching_slash("/").len(), SLASH_COMMANDS.len());
        assert_eq!(
            matching_slash("/cl")
                .iter()
                .map(|c| c.name)
                .collect::<Vec<_>>(),
            vec!["clear", "cleanup"]
        );
        assert_eq!(
            matching_slash("/exit")
                .iter()
                .map(|c| c.name)
                .collect::<Vec<_>>(),
            vec!["quit"]
        );
        assert!(matching_slash("/nope").is_empty());
        assert!(!slash_menu_open("hello"));
        assert!(slash_menu_open("/clear"));
        assert_eq!(slash_rest("/clear"), "");
    }

    #[test]
    fn format_approval_event_lists_keys() {
        let lines = format_event(&RuntimeEvent::ApprovalRequested {
            id: "appr_1".into(),
            tool_name: "Write".into(),
            input: serde_json::json!({"path": "a.rs"}),
        });
        assert!(lines.iter().any(|l| l.text.contains("Write")));
        assert!(lines.iter().any(|l| l.text.contains("a.rs")));
        assert!(lines
            .iter()
            .all(|l| !l.text.contains('⚠') && !l.text.contains('🔧')));
    }

    #[test]
    fn format_user_text() {
        let lines = format_event(&RuntimeEvent::UserText {
            text: "write hello.txt".into(),
        });
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].kind, ChatKind::You);
        assert_eq!(lines[0].text, "write hello.txt");
    }

    #[test]
    fn counts_empty() {
        assert_eq!(status_counts(&[]), (0, 0, 0, 0));
    }

    #[test]
    fn ellipsize_short() {
        assert_eq!(ellipsize("hello", 10), "hello");
        assert_eq!(ellipsize("hello world", 8), "hello...");
    }
}
