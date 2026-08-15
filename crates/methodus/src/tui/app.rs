use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use methodus_core::{Engine, FaceSummary, RecoveredSession};
use methodus_domain::{Approval, ApprovalDecision, RuntimeEvent, Task, TaskStatus};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Dashboard,
    Tasks,
    Session,
    Faces,
}

impl Page {
    pub fn all() -> [Page; 4] {
        [Page::Dashboard, Page::Tasks, Page::Session, Page::Faces]
    }

    pub fn title(self) -> &'static str {
        match self {
            Page::Dashboard => "dashboard",
            Page::Tasks => "tasks",
            Page::Session => "session",
            Page::Faces => "faces",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Page::Dashboard => Page::Tasks,
            Page::Tasks => Page::Session,
            Page::Session => Page::Faces,
            Page::Faces => Page::Dashboard,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Page::Dashboard => Page::Faces,
            Page::Tasks => Page::Dashboard,
            Page::Session => Page::Tasks,
            Page::Faces => Page::Session,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Creating,
}

#[derive(Debug, Clone)]
pub enum Command {
    None,
    Quit,
    Create {
        goal: String,
        face: Option<String>,
    },
    Run {
        task_id: String,
        resume: bool,
    },
    Approve {
        id: String,
        decision: ApprovalDecision,
    },
}

pub struct App {
    pub engine: Engine,
    pub page: Page,
    pub mode: Mode,
    pub should_quit: bool,
    pub status: String,
    pub input: String,
    pub runtime: Option<String>,
    pub default_face: Option<String>,
    pub tasks: Vec<Task>,
    pub approvals: Vec<Approval>,
    pub faces: Vec<FaceSummary>,
    pub recovered: Vec<RecoveredSession>,
    pub task_sel: usize,
    pub approval_sel: usize,
    pub face_sel: usize,
    pub transcript: Vec<String>,
    pub session_task_id: Option<String>,
    pub event_rx: Option<mpsc::Receiver<RuntimeEvent>>,
}

impl App {
    pub fn new(engine: Engine, recovered: Vec<RecoveredSession>) -> Self {
        let n = recovered.len();
        let status = if n == 0 {
            "ready  n new task  r run  1-4 pages  q quit".to_string()
        } else {
            format!("recovered {n} session(s) — open Tasks and press R to resume")
        };
        Self {
            engine,
            page: Page::Dashboard,
            mode: Mode::Normal,
            should_quit: false,
            status,
            input: String::new(),
            runtime: Some("claude-code".to_string()),
            default_face: None,
            tasks: Vec::new(),
            approvals: Vec::new(),
            faces: Vec::new(),
            recovered,
            task_sel: 0,
            approval_sel: 0,
            face_sel: 0,
            transcript: Vec::new(),
            session_task_id: None,
            event_rx: None,
        }
    }

    pub fn refresh(&mut self) {
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
    }

    pub fn selected_task(&self) -> Option<&Task> {
        self.tasks.get(self.task_sel)
    }

    pub fn selected_approval(&self) -> Option<&Approval> {
        self.approvals.get(self.approval_sel)
    }

    pub fn select_task(&mut self, id: &str) {
        if let Some(i) = self.tasks.iter().position(|t| t.id == id) {
            self.task_sel = i;
        }
    }

    pub fn attach_session(&mut self, task_id: String, rx: mpsc::Receiver<RuntimeEvent>) {
        self.session_task_id = Some(task_id.clone());
        self.page = Page::Session;
        self.load_transcript(&task_id);
        self.event_rx = Some(rx);
        self.status = format!("running {task_id}");
    }

    pub fn attach_receiver(&mut self, rx: mpsc::Receiver<RuntimeEvent>) {
        self.page = Page::Session;
        self.event_rx = Some(rx);
    }

    pub fn load_transcript(&mut self, task_id: &str) {
        self.transcript.clear();
        if let Ok(events) = self.engine.store().list_events(Some(task_id), 200) {
            for ev in events {
                if let Ok(parsed) = serde_json::from_str::<RuntimeEvent>(&ev.payload) {
                    self.transcript.extend(format_event(&parsed));
                }
            }
        }
    }

    pub fn push_runtime(&mut self, event: RuntimeEvent) {
        if matches!(event, RuntimeEvent::ApprovalRequested { .. }) {
            self.refresh();
        }
        self.transcript.extend(format_event(&event));
        const MAX: usize = 2000;
        if self.transcript.len() > MAX {
            let drop_n = self.transcript.len() - MAX;
            self.transcript.drain(0..drop_n);
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Command {
        if self.mode == Mode::Creating {
            return self.handle_create_key(key);
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Command::Quit,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Command::Quit,
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                self.page = self.page.next();
                Command::None
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                self.page = self.page.prev();
                Command::None
            }
            KeyCode::Char('1') => {
                self.page = Page::Dashboard;
                Command::None
            }
            KeyCode::Char('2') => {
                self.page = Page::Tasks;
                Command::None
            }
            KeyCode::Char('3') => {
                self.page = Page::Session;
                Command::None
            }
            KeyCode::Char('4') => {
                self.page = Page::Faces;
                Command::None
            }
            KeyCode::Char('n') => {
                self.mode = Mode::Creating;
                self.input.clear();
                self.page = Page::Tasks;
                self.status =
                    "new task — type the goal, Enter to create, Esc to cancel".to_string();
                Command::None
            }
            KeyCode::Char('t') => {
                self.toggle_runtime();
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
            KeyCode::Char('r') => self.cmd_run(false),
            KeyCode::Char('R') => self.cmd_run(true),
            KeyCode::Enter => self.cmd_enter(),
            KeyCode::Char('y') => self.cmd_approve(ApprovalDecision::Once),
            KeyCode::Char('s') => self.cmd_approve(ApprovalDecision::Session),
            KeyCode::Char('d') => self.cmd_approve(ApprovalDecision::Deny),
            KeyCode::Char('x') => self.cmd_approve(ApprovalDecision::Abort),
            _ => Command::None,
        }
    }

    fn handle_create_key(&mut self, key: KeyEvent) -> Command {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.input.clear();
                self.status = "cancelled".to_string();
                Command::None
            }
            KeyCode::Enter => {
                let goal = self.input.trim().to_string();
                if goal.is_empty() {
                    self.status = "goal is empty".to_string();
                    return Command::None;
                }
                Command::Create {
                    goal,
                    face: self.default_face.clone(),
                }
            }
            KeyCode::Backspace => {
                self.input.pop();
                Command::None
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.push(c);
                Command::None
            }
            _ => Command::None,
        }
    }

    fn toggle_runtime(&mut self) {
        self.runtime = match self.runtime.as_deref() {
            Some("codex") => Some("claude-code".to_string()),
            _ => Some("codex".to_string()),
        };
        self.status = format!("runtime {}", self.runtime.as_deref().unwrap_or("-"));
    }

    fn move_sel(&mut self, delta: isize) {
        let (len, sel) = match self.page {
            Page::Dashboard | Page::Session => (self.approvals.len(), self.approval_sel),
            Page::Tasks => (self.tasks.len(), self.task_sel),
            Page::Faces => (self.faces.len(), self.face_sel),
        };
        if len == 0 {
            return;
        }
        let next = (sel as isize + delta).rem_euclid(len as isize) as usize;
        match self.page {
            Page::Dashboard | Page::Session => self.approval_sel = next,
            Page::Tasks => self.task_sel = next,
            Page::Faces => self.face_sel = next,
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

    fn cmd_enter(&mut self) -> Command {
        match self.page {
            Page::Tasks => {
                if let Some(task) = self.selected_task() {
                    let id = task.id.clone();
                    self.session_task_id = Some(id.clone());
                    self.load_transcript(&id);
                    self.page = Page::Session;
                    self.status = format!("viewing {id}");
                }
                Command::None
            }
            Page::Faces => {
                if let Some(face) = self.faces.get(self.face_sel) {
                    self.default_face = Some(face.id.clone());
                    self.status =
                        format!("default face `{}` — new tasks will pin this Face", face.id);
                }
                Command::None
            }
            Page::Dashboard => self.cmd_run(false),
            Page::Session => self.cmd_approve(ApprovalDecision::Once),
        }
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

pub fn format_event(event: &RuntimeEvent) -> Vec<String> {
    match event {
        RuntimeEvent::SessionStarted { session_id } => {
            vec![format!("▶ session {session_id}")]
        }
        RuntimeEvent::AssistantText { text } => split_lines(" ", text),
        RuntimeEvent::Thinking { text } => split_lines("💭 ", text),
        RuntimeEvent::ToolCallStarted { name, .. } => vec![format!("🔧 {name}")],
        RuntimeEvent::ToolCallCompleted { id, .. } => vec![format!("  ✓ {id}")],
        RuntimeEvent::TurnCompleted { stop_reason } => {
            vec![format!(
                "── turn ({}) ──",
                stop_reason.as_deref().unwrap_or("end")
            )]
        }
        RuntimeEvent::Result { is_error, text, .. } => {
            let tag = if *is_error { "✗" } else { "✓" };
            split_lines(&format!("{tag} "), text)
        }
        RuntimeEvent::ApprovalRequested {
            id,
            tool_name,
            input,
        } => vec![
            format!("⚠ approval {id}  tool={tool_name}"),
            format!("  {input}"),
            "  y once  s session  d deny  x abort".to_string(),
        ],
        RuntimeEvent::Error { message } => vec![format!("✗ {message}")],
    }
}

fn split_lines(prefix: &str, text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    text.lines().map(|line| format!("{prefix}{line}")).collect()
}

pub fn status_counts(tasks: &[Task]) -> (usize, usize, usize, usize) {
    let mut queued = 0;
    let mut running = 0;
    let mut waiting = 0;
    let mut done = 0;
    for t in tasks {
        match t.status {
            TaskStatus::Queued | TaskStatus::Planning => queued += 1,
            TaskStatus::Running | TaskStatus::Reviewing => running += 1,
            TaskStatus::WaitingUser => waiting += 1,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled => done += 1,
        }
    }
    (queued, running, waiting, done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn page_cycle() {
        assert_eq!(Page::Dashboard.next(), Page::Tasks);
        assert_eq!(Page::Faces.next(), Page::Dashboard);
        assert_eq!(Page::Dashboard.prev(), Page::Faces);
        assert_eq!(Page::all().len(), 4);
    }

    #[test]
    fn quit_and_tab_without_engine_state() {
        // route via a tiny stand-in: constructing App needs Engine, so test Page + format only
        let k = key(KeyCode::Char('q'));
        assert_eq!(k.kind, KeyEventKind::Press);
        assert_eq!(Page::Tasks.title(), "tasks");
    }

    #[test]
    fn format_approval_event_lists_keys() {
        let lines = format_event(&RuntimeEvent::ApprovalRequested {
            id: "appr_1".into(),
            tool_name: "Write".into(),
            input: serde_json::json!({"path": "a.rs"}),
        });
        assert!(lines.iter().any(|l| l.contains("appr_1")));
        assert!(lines.iter().any(|l| l.contains("y once")));
    }

    #[test]
    fn counts_empty() {
        assert_eq!(status_counts(&[]), (0, 0, 0, 0));
    }
}
