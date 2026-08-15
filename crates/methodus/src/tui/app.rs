use std::path::Path;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use methodus_core::{
    at_query, filter_candidates, health_checks, list_from_roots, list_packs, list_projects, Engine,
    FaceSummary, HealthCheck, MentionCandidate, PackInfo, ProjectInfo, RecoveredSession,
    UserConfig,
};
use methodus_domain::{
    Approval, ApprovalDecision, KnowledgeItem, KnowledgeStatus, Question, QuestionStatus,
    RuntimeEvent, Task, TaskStatus, UsageDelta, UsageSummary,
};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// Daily driver: tasks (left) + session (right). Always the home view.
    Work,
    Faces,
    Review,
    Setup,
}

impl Page {
    pub fn all() -> [Page; 4] {
        [Page::Work, Page::Faces, Page::Review, Page::Setup]
    }

    pub fn title(self) -> &'static str {
        match self {
            Page::Work => "work",
            Page::Faces => "faces",
            Page::Review => "review",
            Page::Setup => "setup",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Page::Work => Page::Faces,
            Page::Faces => Page::Review,
            Page::Review => Page::Setup,
            Page::Setup => Page::Work,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Page::Work => Page::Setup,
            Page::Faces => Page::Work,
            Page::Review => Page::Faces,
            Page::Setup => Page::Review,
        }
    }
}

/// Keyboard target inside the Work page (lazygit-style spatial panels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tasks,
    Session,
    Inbox,
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
    Thinking,
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
    ReviewKnowledge {
        id: String,
        commit: bool,
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
    LearnSkill {
        task_id: String,
        hint: Option<String>,
    },
}

pub struct App {
    pub engine: Engine,
    pub page: Page,
    pub focus: Focus,
    pub mode: Mode,
    pub show_help: bool,
    pub should_quit: bool,
    pub status: String,
    pub status_level: StatusLevel,
    pub input: String,
    pub input_error: Option<String>,
    pub runtime: Option<String>,
    pub default_face: Option<String>,
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
    pub answering_id: Option<String>,
    pub confirm_task_id: Option<String>,
    pub questions: Vec<Question>,
    pub knowledge: Vec<KnowledgeItem>,
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
    mention_cache: Vec<MentionCandidate>,
    mention_root_id: Option<String>,
    last_notify: Option<String>,
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
            page: Page::Work,
            focus: Focus::Session,
            mode: Mode::Normal,
            show_help: false,
            should_quit: false,
            status,
            status_level,
            input: String::new(),
            input_error: None,
            runtime: Some(
                cfg.default_runtime
                    .clone()
                    .unwrap_or_else(|| "claude-code".to_string()),
            ),
            default_face: cfg.default_face.clone(),
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
            answering_id: None,
            confirm_task_id: None,
            questions: Vec::new(),
            knowledge: Vec::new(),
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
            mention_cache: Vec::new(),
            mention_root_id: None,
            last_notify: None,
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
        }
        self.set_status(StatusLevel::Warn, "ctrl-c again to quit");
        Command::None
    }

    pub fn set_status(&mut self, level: StatusLevel, text: impl Into<String>) {
        self.status_level = level;
        self.status = text.into();
    }

    pub fn notify(&mut self, key: &str, body: &str) {
        if !self.notifications {
            return;
        }
        if self.last_notify.as_deref() == Some(key) {
            return;
        }
        self.last_notify = Some(key.to_string());
        crate::notify::send("Methodus", body);
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
        if self
            .questions
            .iter()
            .any(|q| q.status == QuestionStatus::Asked)
        {
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
        self.page = Page::Review;
        self.mode = Mode::Answering;
        self.answering_id = Some(q.id.clone());
        self.input.clear();
        self.input_error = None;
        let preview = ellipsize(&q.question, 80);
        self.notify(
            &format!("question:{}", q.id),
            &format!("a question: {preview}"),
        );
        self.set_status(
            StatusLevel::Info,
            format!("idle question {} — Enter submit, Esc later", q.id),
        );
        self.questions = self
            .engine
            .store()
            .list_questions(None)
            .unwrap_or_default()
            .into_iter()
            .filter(|item| {
                matches!(
                    item.status,
                    QuestionStatus::Pending | QuestionStatus::Asked | QuestionStatus::Snoozed
                )
            })
            .collect();
        if let Some(i) = self.questions.iter().position(|item| item.id == q.id) {
            self.review_sel = i;
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
            if self.approval_sel >= self.approvals.len() {
                self.approval_sel = self.approvals.len().saturating_sub(1);
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
        let n = self.questions.len() + self.knowledge.len();
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
        if self.page == Page::Setup {
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
        } else {
            self.maybe_idle_prompt();
        }
    }

    pub fn restore_recovered(&mut self) {
        let Some(id) = self.recovered.first().map(|r| r.task_id.clone()) else {
            return;
        };
        self.select_task(&id);
        self.session_task_id = Some(id.clone());
        self.load_transcript(&id);
        self.focus = Focus::Session;
    }

    pub fn selected_task(&self) -> Option<&Task> {
        self.tasks.get(self.task_sel)
    }

    pub fn selected_approval(&self) -> Option<&Approval> {
        self.approvals.get(self.approval_sel)
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

    pub fn attach_session(&mut self, task_id: String, rx: mpsc::Receiver<RuntimeEvent>) {
        self.session_task_id = Some(task_id.clone());
        self.page = Page::Work;
        self.focus = Focus::Session;
        self.load_transcript(&task_id);
        self.event_rx = Some(rx);
        self.input.clear();
        self.input_error = None;
        self.transcript_offset = 0;
        self.set_status(StatusLevel::Info, format!("running {task_id}"));
    }

    pub fn attach_receiver(&mut self, rx: mpsc::Receiver<RuntimeEvent>) {
        self.page = Page::Work;
        self.focus = Focus::Session;
        self.event_rx = Some(rx);
    }

    pub fn load_transcript(&mut self, task_id: &str) {
        self.transcript.clear();
        if let Ok(events) = self.engine.store().list_events(Some(task_id), 400) {
            for ev in events {
                if let Ok(parsed) = serde_json::from_str::<RuntimeEvent>(&ev.payload) {
                    self.transcript.extend(format_event(&parsed));
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
    }

    pub fn push_runtime(&mut self, event: RuntimeEvent) {
        if let RuntimeEvent::ApprovalRequested { id, tool_name, .. } = &event {
            self.refresh();
            self.notify(
                &format!("approval:{id}"),
                &format!("needs approval: {tool_name}"),
            );
        }
        self.transcript.extend(format_event(&event));
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
        self.last_activity = Instant::now();
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
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('n') => {
                    self.start_new_conversation();
                    return Command::None;
                }
                _ => {}
            }
        }
        if self.page == Page::Setup && self.mode == Mode::Normal {
            return self.handle_setup_key(key);
        }
        if self.page == Page::Work && self.focus == Focus::Session && self.mode == Mode::Normal {
            return self.handle_session_key(key);
        }
        match key.code {
            KeyCode::Esc => {
                if self.page != Page::Work {
                    self.page = Page::Work;
                    self.focus = Focus::Session;
                    self.set_status(StatusLevel::Info, "work");
                }
                Command::None
            }
            KeyCode::Char('?') => {
                self.show_help = true;
                Command::None
            }
            KeyCode::Tab => {
                if self.page == Page::Work {
                    self.focus_next();
                }
                Command::None
            }
            KeyCode::BackTab => {
                if self.page == Page::Work {
                    self.focus_prev();
                }
                Command::None
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if self.page == Page::Work {
                    self.focus_next();
                }
                Command::None
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if self.page == Page::Work {
                    self.focus_prev();
                }
                Command::None
            }
            KeyCode::Char('[') => {
                self.page = self.page.prev();
                Command::None
            }
            KeyCode::Char(']') => {
                self.page = self.page.next();
                Command::None
            }
            KeyCode::Char('1') => {
                self.page = Page::Work;
                Command::None
            }
            KeyCode::Char('2') => {
                self.page = Page::Faces;
                Command::None
            }
            KeyCode::Char('3') => {
                self.page = Page::Review;
                Command::None
            }
            KeyCode::Char('4') => {
                self.page = Page::Setup;
                Command::None
            }
            KeyCode::Char('n') => {
                self.start_new_conversation();
                Command::None
            }
            KeyCode::Char('t') => {
                if self.page == Page::Work && self.focus == Focus::Tasks {
                    self.toggle_runtime();
                }
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
            KeyCode::Char('c') => {
                if self.page == Page::Work && self.focus == Focus::Tasks {
                    self.begin_cancel()
                } else {
                    Command::None
                }
            }
            KeyCode::Char('r') => {
                if self.page == Page::Work && self.focus == Focus::Tasks {
                    self.cmd_run(false)
                } else {
                    Command::None
                }
            }
            KeyCode::Char('R') => {
                if self.page == Page::Work && self.focus == Focus::Tasks {
                    self.cmd_run(true)
                } else {
                    Command::None
                }
            }
            KeyCode::Enter => self.cmd_enter(),
            KeyCode::Char('y') => {
                if self.page == Page::Review {
                    self.cmd_review(true)
                } else if self.page == Page::Work && self.focus == Focus::Inbox {
                    self.cmd_approve(ApprovalDecision::Once)
                } else {
                    Command::None
                }
            }
            KeyCode::Char('s') => {
                if self.page == Page::Work && self.focus == Focus::Inbox {
                    self.cmd_approve(ApprovalDecision::Session)
                } else {
                    Command::None
                }
            }
            KeyCode::Char('d') => {
                if self.page == Page::Review {
                    self.cmd_review_negative()
                } else if self.page == Page::Work && self.focus == Focus::Inbox {
                    self.cmd_approve(ApprovalDecision::Deny)
                } else {
                    Command::None
                }
            }
            KeyCode::Char('x') => {
                if self.page == Page::Review {
                    self.cmd_review_negative()
                } else if self.page == Page::Work && self.focus == Focus::Inbox {
                    self.cmd_approve(ApprovalDecision::Abort)
                } else {
                    Command::None
                }
            }
            KeyCode::Char('z') => self.cmd_snooze(),
            _ => Command::None,
        }
    }

    fn handle_session_key(&mut self, key: KeyEvent) -> Command {
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
                    self.input_error = None;
                    self.slash_sel = 0;
                    return Command::None;
                }
                if mention_open {
                    self.cancel_mention();
                    return Command::None;
                }
                self.focus = Focus::Tasks;
                self.set_status(StatusLevel::Info, "tasks");
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
                self.focus_next();
                Command::None
            }
            KeyCode::BackTab => {
                self.focus_prev();
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
                self.input.pop();
                self.input_error = None;
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
            KeyCode::Char('y') if empty && !self.approvals.is_empty() => {
                self.cmd_approve(ApprovalDecision::Once)
            }
            KeyCode::Char('s') if empty && !self.approvals.is_empty() => {
                self.cmd_approve(ApprovalDecision::Session)
            }
            KeyCode::Char('d') if empty && !self.approvals.is_empty() => {
                self.cmd_approve(ApprovalDecision::Deny)
            }
            KeyCode::Char('x') if empty && !self.approvals.is_empty() => {
                self.cmd_approve(ApprovalDecision::Abort)
            }
            KeyCode::Char('[') if empty => {
                self.page = self.page.prev();
                Command::None
            }
            KeyCode::Char(']') if empty => {
                self.page = self.page.next();
                Command::None
            }
            KeyCode::Char('1') if empty => {
                self.page = Page::Work;
                Command::None
            }
            KeyCode::Char('2') if empty => {
                self.page = Page::Faces;
                Command::None
            }
            KeyCode::Char('3') if empty => {
                self.page = Page::Review;
                Command::None
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.push(c);
                self.input_error = None;
                self.sync_slash_sel();
                self.sync_mention_sel();
                Command::None
            }
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
                self.set_status(StatusLevel::Info, "cancelled");
                Command::None
            }
            KeyCode::Enter | KeyCode::Char('y') => {
                let id = self.confirm_task_id.clone().unwrap_or_default();
                self.mode = Mode::Normal;
                self.confirm_task_id = None;
                Command::Cancel { task_id: id }
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
                self.input.pop();
                self.input_error = None;
                Command::None
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.push(c);
                self.input_error = None;
                Command::None
            }
            _ => Command::None,
        }
    }

    fn begin_prompt(&mut self, kind: PromptKind) {
        self.mode = Mode::Prompt;
        self.prompt_kind = Some(kind);
        self.input.clear();
        self.input_error = None;
        if kind == PromptKind::WorkspaceRoot {
            self.input = self.workspace_root.clone();
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
                self.page = Page::Work;
                self.focus = Focus::Session;
                Command::None
            }
            KeyCode::Char('1') => {
                self.page = Page::Work;
                Command::None
            }
            KeyCode::Char('2') => {
                self.page = Page::Faces;
                Command::None
            }
            KeyCode::Char('3') => {
                self.page = Page::Review;
                Command::None
            }
            KeyCode::Char('4') => Command::None,
            KeyCode::Char('[') => {
                self.page = self.page.prev();
                Command::None
            }
            KeyCode::Char(']') => {
                self.page = self.page.next();
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
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.input.clear();
                self.input_error = None;
                self.answering_id = None;
                self.set_status(StatusLevel::Info, "cancelled");
                Command::None
            }
            KeyCode::Enter => {
                let text = self.input.trim().to_string();
                if text.is_empty() {
                    self.input_error = Some("answer is empty — type a reply".to_string());
                    return Command::None;
                }
                let id = self.answering_id.clone().unwrap_or_default();
                Command::AnswerQuestion { id, text }
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.input_error = None;
                Command::None
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.push(c);
                self.input_error = None;
                Command::None
            }
            _ => Command::None,
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
        self.page = Page::Work;
        self.focus = Focus::Session;
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

    fn scroll_transcript(&mut self, delta: isize) {
        let max = self.transcript.len();
        let next = (self.transcript_offset as isize + delta).clamp(0, max as isize);
        self.transcript_offset = next as usize;
    }

    fn toggle_runtime(&mut self) {
        let mut cfg = UserConfig {
            default_runtime: self.runtime.clone(),
            permission_mode: Some(self.permission_mode.clone()),
            default_face: self.default_face.clone(),
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

    fn focus_next(&mut self) {
        self.focus = match self.focus {
            Focus::Tasks => Focus::Session,
            Focus::Session => {
                if self.approvals.is_empty() {
                    Focus::Tasks
                } else {
                    Focus::Inbox
                }
            }
            Focus::Inbox => Focus::Tasks,
        };
    }

    fn focus_prev(&mut self) {
        self.focus = match self.focus {
            Focus::Tasks => {
                if self.approvals.is_empty() {
                    Focus::Session
                } else {
                    Focus::Inbox
                }
            }
            Focus::Session => Focus::Tasks,
            Focus::Inbox => Focus::Session,
        };
    }

    fn list_len(&self) -> usize {
        match (self.page, self.focus) {
            (Page::Work, Focus::Tasks) => self.tasks.len(),
            (Page::Work, Focus::Inbox) => self.approvals.len(),
            (Page::Work, Focus::Session) => 0,
            (Page::Faces, _) => self.faces.len(),
            (Page::Review, _) => self.questions.len() + self.knowledge.len(),
            (Page::Setup, _) => self.setup_list_len(),
        }
    }

    fn current_sel(&self) -> usize {
        match (self.page, self.focus) {
            (Page::Work, Focus::Tasks) => self.task_sel,
            (Page::Work, Focus::Inbox) => self.approval_sel,
            (Page::Work, Focus::Session) => 0,
            (Page::Faces, _) => self.face_sel,
            (Page::Review, _) => self.review_sel,
            (Page::Setup, _) => self.setup_sel,
        }
    }

    fn set_sel(&mut self, sel: usize) {
        match (self.page, self.focus) {
            (Page::Work, Focus::Tasks) => self.task_sel = sel,
            (Page::Work, Focus::Inbox) => self.approval_sel = sel,
            (Page::Work, Focus::Session) => {}
            (Page::Faces, _) => self.face_sel = sel,
            (Page::Review, _) => self.review_sel = sel,
            (Page::Setup, _) => self.setup_sel = sel,
        }
    }

    fn move_sel(&mut self, delta: isize) {
        let len = self.list_len();
        if len == 0 {
            return;
        }
        let next = (self.current_sel() as isize + delta).rem_euclid(len as isize) as usize;
        self.set_sel(next);
        if self.page == Page::Work && self.focus == Focus::Tasks {
            if let Some(task) = self.selected_task() {
                let id = task.id.clone();
                if self.session_task_id.as_deref() != Some(id.as_str()) && !self.busy() {
                    self.session_task_id = Some(id.clone());
                    self.load_transcript(&id);
                }
            }
        }
    }

    fn jump_sel(&mut self, to: isize) {
        let len = self.list_len();
        if len == 0 {
            return;
        }
        if to == isize::MAX {
            self.set_sel(len - 1);
        } else {
            self.set_sel(0);
        }
    }

    fn cmd_run(&self, resume: bool) -> Command {
        match self.selected_task() {
            Some(task) => Command::Run {
                task_id: task.id.clone(),
                resume,
            },
            None => Command::None,
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
        self.mention_sel = 0;
        self.input_error = None;
        self.sync_mention_sel();
    }

    fn dispatch_slash_input(&mut self) -> Command {
        let matches = matching_slash(&self.input);
        if matches.is_empty() {
            self.input_error = Some("unknown command — try /help /clear /learn /quit".to_string());
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
            "learn" => {
                self.input.clear();
                self.slash_sel = 0;
                if self.busy() {
                    self.input_error = Some("wait until this turn ends".to_string());
                    return Command::None;
                }
                let Some(task) = self.session_task() else {
                    self.input_error = Some("open a conversation first, then /learn".to_string());
                    return Command::None;
                };
                Command::LearnSkill {
                    task_id: task.id.clone(),
                    hint: if rest.is_empty() { None } else { Some(rest) },
                }
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
        if methodus_core::learning::is_learn_request(&text) {
            if self.busy() {
                self.input_error = Some("wait until this turn ends".to_string());
                return Command::None;
            }
            let Some(task) = self.session_task() else {
                self.input_error = Some("open a conversation first, then /learn".to_string());
                return Command::None;
            };
            return Command::LearnSkill {
                task_id: task.id.clone(),
                hint: methodus_core::learning::learn_hint(&text),
            };
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

    fn cmd_enter(&mut self) -> Command {
        match self.page {
            Page::Work => match self.focus {
                Focus::Tasks => {
                    if let Some(task) = self.selected_task() {
                        let id = task.id.clone();
                        self.session_task_id = Some(id.clone());
                        self.load_transcript(&id);
                        self.focus = Focus::Session;
                        self.set_status(StatusLevel::Info, format!("viewing {id}"));
                    }
                    Command::None
                }
                Focus::Inbox => self.cmd_approve(ApprovalDecision::Once),
                Focus::Session => self.cmd_send(),
            },
            Page::Faces => {
                if let Some(face) = self.faces.get(self.face_sel) {
                    self.default_face = Some(face.id.clone());
                    match self.persist_config() {
                        Ok(()) => self.set_status(
                            StatusLevel::Ok,
                            format!("default face `{}` — new messages pin this Face", face.id),
                        ),
                        Err(e) => self.set_status(StatusLevel::Error, e),
                    }
                }
                Command::None
            }
            Page::Review => self.cmd_review_enter(),
            Page::Setup => Command::None,
        }
    }

    fn selected_review(&self) -> Option<ReviewItem<'_>> {
        let i = self.review_sel;
        if i < self.questions.len() {
            return self.questions.get(i).map(ReviewItem::Question);
        }
        self.knowledge
            .get(i - self.questions.len())
            .map(ReviewItem::Knowledge)
    }

    fn cmd_review_enter(&mut self) -> Command {
        let qid = match self.selected_review() {
            Some(ReviewItem::Question(q)) => Some(q.id.clone()),
            Some(ReviewItem::Knowledge(k)) => {
                return Command::ReviewKnowledge {
                    id: k.id.clone(),
                    commit: true,
                };
            }
            None => None,
        };
        if let Some(id) = qid {
            self.mode = Mode::Answering;
            self.answering_id = Some(id.clone());
            self.input.clear();
            self.input_error = None;
            self.set_status(
                StatusLevel::Info,
                format!("answer {id} — Enter submit, Esc cancel"),
            );
        }
        Command::None
    }

    fn cmd_review(&self, commit: bool) -> Command {
        match self.selected_review() {
            Some(ReviewItem::Knowledge(k)) => Command::ReviewKnowledge {
                id: k.id.clone(),
                commit,
            },
            _ => Command::None,
        }
    }

    fn cmd_review_negative(&self) -> Command {
        match self.selected_review() {
            Some(ReviewItem::Knowledge(k)) => Command::ReviewKnowledge {
                id: k.id.clone(),
                commit: false,
            },
            Some(ReviewItem::Question(q)) => Command::DismissQuestion { id: q.id.clone() },
            None => Command::None,
        }
    }

    fn cmd_snooze(&self) -> Command {
        match self.selected_review() {
            Some(ReviewItem::Question(q)) => Command::SnoozeQuestion { id: q.id.clone() },
            _ => Command::None,
        }
    }

    fn begin_cancel(&mut self) -> Command {
        let Some(id) = self.selected_task().map(|t| t.id.clone()) else {
            return Command::None;
        };
        self.mode = Mode::ConfirmCancel;
        self.confirm_task_id = Some(id.clone());
        self.set_status(StatusLevel::Warn, format!("cancel {id}? [y]es  [n]/esc no"));
        Command::None
    }

    fn cmd_approve(&self, decision: ApprovalDecision) -> Command {
        match self.selected_approval() {
            Some(a) => Command::Approve {
                id: a.id.clone(),
                decision,
            },
            None => Command::None,
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
        RuntimeEvent::Thinking { text } => blob(ChatKind::Thinking, text),
        RuntimeEvent::ToolCallStarted { name, .. } => {
            vec![line(ChatKind::Tool, format!("· {name}"))]
        }
        RuntimeEvent::ToolCallCompleted { id, .. } => {
            vec![line(ChatKind::Tool, format!("ok {id}"))]
        }
        RuntimeEvent::TurnCompleted { stop_reason } => {
            vec![line(
                ChatKind::Meta,
                format!("turn ({})", stop_reason.as_deref().unwrap_or("end")),
            )]
        }
        RuntimeEvent::Result {
            is_error,
            text,
            cost_usd,
            usage,
            ..
        } => {
            let kind = if *is_error {
                ChatKind::Alert
            } else {
                ChatKind::Meta
            };
            let mut lines = blob(kind, text);
            let delta = UsageDelta::from_result(*cost_usd, usage.as_ref());
            if !delta.is_empty() {
                lines.push(line(ChatKind::Meta, format!("usage  {}", delta.compact())));
            }
            lines
        }
        RuntimeEvent::ApprovalRequested {
            id,
            tool_name,
            input,
        } => vec![
            line(ChatKind::Alert, format!("{tool_name}  {id}")),
            line(ChatKind::Alert, input.to_string()),
            line(
                ChatKind::Alert,
                "[y]once  [s]ession  [d]eny  [x]abort".to_string(),
            ),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashCmd {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub summary: &'static str,
}

pub const SLASH_COMMANDS: &[SlashCmd] = &[
    SlashCmd {
        name: "clear",
        aliases: &[],
        summary: "new conversation — next turn is a fresh executor session",
    },
    SlashCmd {
        name: "help",
        aliases: &[],
        summary: "keyboard and commands",
    },
    SlashCmd {
        name: "learn",
        aliases: &[],
        summary: "draft a skill from this conversation",
    },
    SlashCmd {
        name: "quit",
        aliases: &["exit"],
        summary: "quit Methodus",
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
        Mode::Answering => " [enter] submit   [esc] cancel ".to_string(),
        Mode::Prompt => " [enter] save path   [esc] cancel ".to_string(),
        Mode::ConfirmCancel => " [y]es cancel task   [n]/[esc] keep ".to_string(),
        Mode::Normal => match app.page {
            Page::Work => match app.focus {
                Focus::Inbox => {
                    " [enter]approve [y]once [s]ession [d]eny [x]abort  [tab]focus [?]help "
                        .to_string()
                }
                Focus::Tasks => {
                    " [n]ew [enter]chat [r]un [c]ancel  [tab]session  [?]help ".to_string()
                }
                Focus::Session => {
                    if app.input.is_empty() && !app.approvals.is_empty() {
                        " [enter]send  [y]once [s]ession [d]eny  [esc]tasks [?]help ".to_string()
                    } else {
                        " [enter]send  [/]cmds  [@]files  [esc]tasks  [ctrl-n]new  [?]help "
                            .to_string()
                    }
                }
            },
            Page::Faces => {
                " [enter] pin default face   [esc] work   [?]help  [ctrl-c twice]quit ".to_string()
            }
            Page::Review => {
                " [enter] answer/commit  [y]commit [d/x]reject [z]snooze  [esc] work  [?]help "
                    .to_string()
            }
            Page::Setup => {
                " [tab]section [enter]set [a]dd path [d]rop [space]toggle pack  [esc]work [?]help "
                    .to_string()
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    #[test]
    fn page_cycle() {
        assert_eq!(Page::Work.next(), Page::Faces);
        assert_eq!(Page::Review.next(), Page::Setup);
        assert_eq!(Page::Setup.next(), Page::Work);
        assert_eq!(Page::Work.prev(), Page::Setup);
        assert_eq!(Page::all().len(), 4);
    }

    #[test]
    fn help_line_session_mentions_send() {
        let line = " [enter]send  [/]cmds  [@]files  [esc]tasks  [ctrl-n]new  [?]help ";
        assert!(line.contains("[enter]send"));
        assert!(line.contains("[/]cmds"));
        assert!(line.contains("[@]files"));
        assert!(!line.contains("[q]uit"));
        assert!(!line.contains("1-5 pages"));
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
        assert_eq!(
            matching_slash("/learn")
                .iter()
                .map(|c| c.name)
                .collect::<Vec<_>>(),
            vec!["learn"]
        );
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
            vec!["clear"]
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
        assert_eq!(slash_rest("/learn cpu-sample"), "cpu-sample");
        assert_eq!(slash_rest("/clear"), "");
    }

    #[test]
    fn format_approval_event_lists_keys() {
        let lines = format_event(&RuntimeEvent::ApprovalRequested {
            id: "appr_1".into(),
            tool_name: "Write".into(),
            input: serde_json::json!({"path": "a.rs"}),
        });
        assert!(lines.iter().any(|l| l.text.contains("appr_1")));
        assert!(lines.iter().any(|l| l.text.contains("[y]once")));
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
