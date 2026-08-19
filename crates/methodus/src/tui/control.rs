//! Conversation-first shell. A message creates a task; Methodus runs a tiny
//! read-only planner, then gives the terminal to the selected native Agent.

use std::cell::RefCell;
use std::fs;
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::{
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use methodus_core::{at_query, list_from_roots, Engine, MentionCandidate, NativeHandoffPlan, PermissionMode, UserConfig};
use methodus_domain::{GraphNode, Session, Task};
use ratatui::{
    prelude::{Alignment, Constraint, CrosstermBackend, Direction, Frame, Layout, Position, Rect, Terminal},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MARK: &str = "◈";
const WORDMARK: &str = "Methodus";
const CTRL_C_QUIT_WINDOW: Duration = Duration::from_secs(3);
const COMPOSER_MAX_ROWS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SlashCmd { name: &'static str, aliases: &'static [&'static str], summary: &'static str }

const SLASH_COMMANDS: &[SlashCmd] = &[
    SlashCmd { name: "knowledge", aliases: &["graph"], summary: "浏览知识图谱与关联节点" },
    SlashCmd { name: "skill", aliases: &["skills"], summary: "查看可复用技能与说明" },
    SlashCmd { name: "session", aliases: &["sessions"], summary: "查看任务、交接与运行记录" },
    SlashCmd { name: "experience", aliases: &["experiences"], summary: "查看可复盘的任务经验" },
    SlashCmd { name: "review", aliases: &["inbox"], summary: "审核待确认的知识候选" },
    SlashCmd { name: "runtime", aliases: &[], summary: "切换 Claude Code、Codex 与 Cursor" },
    SlashCmd { name: "learn", aliases: &[], summary: "创建学习任务并沉淀 5W2H" },
    SlashCmd { name: "open", aliases: &[], summary: "在文件管理器中打开当前工作区" },
    SlashCmd { name: "help", aliases: &["?"], summary: "查看命令、快捷键与交接流程" },
    SlashCmd { name: "quit", aliases: &["exit"], summary: "退出 Methodus（q 不会退出）" },
];

#[derive(Default)]
struct TranscriptCache { version: u64, width: usize, rows: Vec<String> }
thread_local! { static TRANSCRIPT_CACHE: RefCell<TranscriptCache> = RefCell::new(TranscriptCache::default()); }

/// Retains the previous TUI's warm, restrained terminal palette. Color adds
/// hierarchy only; all state remains readable with NO_COLOR or 16-color themes.
struct Theme { fg: Color, muted: Color, surface: Color, overlay: Color, overlay_fg: Color, border: Color, accent: Color, info: Color, success: Color, warning: Color, error: Color }
impl Theme {
    fn current() -> Self {
        if std::env::var_os("NO_COLOR").is_some() { return Self { fg: Color::Reset, muted: Color::Reset, surface: Color::Reset, overlay: Color::Reset, overlay_fg: Color::Reset, border: Color::Reset, accent: Color::Reset, info: Color::Reset, success: Color::Reset, warning: Color::Reset, error: Color::Reset }; }
        Self { fg: Color::Reset, muted: Color::Rgb(118, 118, 112), surface: Color::Reset, overlay: Color::Rgb(28, 28, 26), overlay_fg: Color::Rgb(245, 241, 234), border: Color::Rgb(72, 72, 68), accent: Color::Rgb(218, 119, 86), info: Color::Rgb(122, 158, 170), success: Color::Rgb(167, 176, 110), warning: Color::Rgb(212, 160, 84), error: Color::Rgb(224, 108, 117) }
    }
    fn text(&self) -> Style { Style::default().fg(self.fg).bg(self.surface) }
    fn dim(&self) -> Style { Style::default().fg(self.muted).bg(self.surface) }
    fn accent(&self) -> Style { Style::default().fg(self.accent).add_modifier(Modifier::BOLD) }
    fn selected(&self) -> Style { Style::default().fg(self.accent).add_modifier(Modifier::BOLD | Modifier::REVERSED) }
    fn border(&self, active: bool) -> Style { Style::default().fg(if active { self.accent } else { self.border }) }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View { Chat, Help, Knowledge, KnowledgeDetail, Skills, SkillDetail, Experience, ExperienceDetail, Sessions, Review, ReviewDetail, Return }

struct App {
    engine: Engine, view: View, input: String, transcript: Vec<String>, nodes: Vec<GraphNode>, tasks: Vec<Task>, sessions: Vec<Session>,
    selected: usize, plan: Option<NativeHandoffPlan>, status: String, runtime: String, permission: String, quit: bool,
    input_cursor: usize, slash_sel: usize, transcript_offset: usize, transcript_version: u64,
    knowledge_content: String, knowledge_scroll: u16, detail_return: View, detail_node_id: String,
    knowledge_filter: String, session_filter: String, session_sort: u8,
    handoff_message: Option<String>, handoff_frame: usize,
    mention_cache: Vec<MentionCandidate>, mention_dynamic: Vec<MentionCandidate>, mention_sel: usize,
    pending_quit_at: Option<Instant>, current_task_id: Option<String>,
}

impl App {
    fn new(engine: Engine) -> Self {
        let config = UserConfig::load(engine.home());
        Self { engine, view: View::Chat, input: String::new(), transcript: vec!["Methodus · 描述你要完成的任务，我会规划上下文后交给原生 Agent。输入 /help 查看管理面板。".into()], nodes: vec![], tasks: vec![], sessions: vec![], selected: 0, plan: None, status: "就绪".into(), runtime: config.default_runtime.unwrap_or_else(|| "claude-code".into()), permission: config.permission_mode.unwrap_or_else(|| "acceptEdits".into()), quit: false, input_cursor: 0, slash_sel: 0, transcript_offset: 0, transcript_version: 1, pending_quit_at: None, current_task_id: None, knowledge_content: String::new(), knowledge_scroll: 0, detail_return: View::Chat, detail_node_id: String::new(), knowledge_filter: String::new(), session_filter: String::new(), session_sort: 0, handoff_message: None, handoff_frame: 0, mention_cache: Vec::new(), mention_dynamic: Vec::new(), mention_sel: 0 }
    }
    fn refresh(&mut self) {
        match self.engine.sync_graph().and_then(|_| self.engine.list_graph_nodes(None)) { Ok(nodes) => self.nodes = nodes, Err(error) => self.status = format!("图谱同步失败：{error}") }
        if let Ok(tasks) = self.engine.store().list_tasks() { self.tasks = tasks; }
        if let Ok(sessions) = self.engine.store().list_sessions() { self.sessions = sessions; }
        self.selected = self.selected.min(self.items_len().saturating_sub(1));
    }
    fn items_len(&self) -> usize { match self.view { View::Knowledge => filtered_knowledge_nodes(&self.nodes, &self.knowledge_filter).len(), View::Skills => self.nodes.iter().filter(|n| n.node_type == "skill").count(), View::Experience => self.nodes.iter().filter(|n| n.node_type == "experience").count(), View::Sessions => filtered_sessions(&self.sessions, &self.tasks, &self.session_filter, self.session_sort).len(), View::Review => self.nodes.iter().filter(|n| n.status.as_deref() == Some("candidate")).count(), _ => 0 } }
    fn panel(&mut self, view: View) { self.view = view; self.selected = 0; self.refresh(); }
    fn say(&mut self, text: impl Into<String>) { self.transcript.push(text.into()); if self.transcript.len() > 200 { self.transcript.remove(0); } self.transcript_offset = 0; self.transcript_version = self.transcript_version.wrapping_add(1); }
    fn clear_input(&mut self) { self.input.clear(); self.input_cursor = 0; self.slash_sel = 0; }
    fn clamp_cursor(&mut self) { self.input_cursor = self.input_cursor.min(self.input.len()); while self.input_cursor > 0 && !self.input.is_char_boundary(self.input_cursor) { self.input_cursor -= 1; } }
    fn insert_str(&mut self, text: &str) { self.clamp_cursor(); self.input.insert_str(self.input_cursor, text); self.input_cursor += text.len(); self.sync_slash_sel(); self.sync_mention_sel(); }
    fn insert_paste(&mut self, text: &str) { self.insert_str(&text.replace("\r\n", "\n").replace('\r', "\n")); }
    fn backspace(&mut self) { self.clamp_cursor(); if self.input_cursor > 0 { let prev = prev_char_boundary(&self.input, self.input_cursor); self.input.replace_range(prev..self.input_cursor, ""); self.input_cursor = prev; self.sync_slash_sel(); self.sync_mention_sel(); } }
    fn delete(&mut self) { self.clamp_cursor(); if self.input_cursor < self.input.len() { let next = next_char_boundary(&self.input, self.input_cursor); self.input.replace_range(self.input_cursor..next, ""); self.sync_slash_sel(); self.sync_mention_sel(); } }
    fn move_cursor(&mut self, direction: isize) { self.clamp_cursor(); if direction < 0 && self.input_cursor > 0 { self.input_cursor = prev_char_boundary(&self.input, self.input_cursor); } else if direction > 0 && self.input_cursor < self.input.len() { self.input_cursor = next_char_boundary(&self.input, self.input_cursor); } }
    fn sync_slash_sel(&mut self) { self.slash_sel = self.slash_sel.min(matching_slash(&self.input).len().saturating_sub(1)); }
    fn move_slash(&mut self, direction: isize) { let count = matching_slash(&self.input).len(); if count == 0 { self.slash_sel = 0; } else if direction < 0 { self.slash_sel = self.slash_sel.saturating_sub(1); } else { self.slash_sel = (self.slash_sel + 1).min(count - 1); } }
    fn complete_slash(&mut self) { if let Some(command) = matching_slash(&self.input).get(self.slash_sel) { let rest = slash_rest(&self.input); self.input = if rest.is_empty() { format!("/{} ", command.name) } else { format!("/{} {rest}", command.name) }; self.input_cursor = self.input.len(); self.slash_sel = 0; } }
    fn sync_mention_sel(&mut self) { if mention_open(&self.input) { self.ensure_mention_cache(); self.mention_dynamic = absolute_path_candidates(at_query(&self.input).unwrap_or_default()); self.mention_sel = self.mention_sel.min(self.matching_mentions().len().saturating_sub(1)); } else { self.mention_dynamic.clear(); self.mention_sel = 0; } }
    fn ensure_mention_cache(&mut self) { if self.mention_cache.is_empty() { self.mention_cache = list_from_roots(&self.engine.context_roots(), 1500); } }
    fn matching_mentions(&self) -> Vec<&MentionCandidate> { let mut all = self.mention_cache.iter().collect::<Vec<_>>(); all.extend(self.mention_dynamic.iter()); let query = at_query(&self.input).unwrap_or_default(); let mut scored = all.into_iter().filter(|candidate| candidate.label.to_lowercase().contains(&query.to_lowercase())).collect::<Vec<_>>(); scored.sort_by_key(|candidate| candidate.label.len()); scored }
    fn move_mention(&mut self, delta: isize) { let n = self.matching_mentions().len(); if n == 0 { self.mention_sel = 0; } else if delta < 0 { self.mention_sel = self.mention_sel.saturating_sub(delta.unsigned_abs()); } else { self.mention_sel = (self.mention_sel + delta as usize).min(n - 1); } }
    fn accept_mention(&mut self) { self.ensure_mention_cache(); let Some(candidate) = self.matching_mentions().get(self.mention_sel).cloned() else { return; }; let Some(start) = mention_start(&self.input) else { return; }; let replacement = if candidate.is_dir { format!("@{}", candidate.label) } else { format!("@{} ", candidate.label) }; self.input.replace_range(start..self.input_cursor, &replacement); self.input_cursor = start + replacement.len(); self.mention_sel = 0; }
    fn handle_ctrl_c(&mut self) { if !self.input.is_empty() { self.clear_input(); self.pending_quit_at = None; self.status = "已清空输入".into(); return; } let now = Instant::now(); if ctrl_c_should_quit(self.pending_quit_at, now) { self.quit = true; } else { self.pending_quit_at = Some(now); self.view = View::Chat; self.status = "再次按 Ctrl+C 退出".into(); } }
    fn scroll_transcript(&mut self, delta: isize) { if delta > 0 { self.transcript_offset = self.transcript_offset.saturating_add(delta as usize); } else { self.transcript_offset = self.transcript_offset.saturating_sub(delta.unsigned_abs()); } }
}

pub fn run_loop(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, engine: Engine) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = App::new(engine); app.refresh();
    loop {
        terminal.draw(|frame| draw(frame, &app))?;
        if app.quit { break; }
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => handle_key(terminal, &mut app, key)?,
                Event::Paste(text) if matches!(app.view, View::Chat | View::Return) => app.insert_paste(&text),
                _ => {}
            }
        }
    }
    Ok(())
}

fn handle_key(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App, key: KeyEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) {
        if key.kind == KeyEventKind::Press { app.handle_ctrl_c(); }
        return Ok(());
    }
    if matches!(key.code, KeyCode::BackTab) || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT)) {
        cycle_permission(app);
        return Ok(());
    }
    app.pending_quit_at = None;
    match app.view {
        View::Chat => match key.code {
            KeyCode::Esc if mention_open(&app.input) => app.mention_sel = 0,
            KeyCode::Esc if slash_menu_open(&app.input) => app.clear_input(),
            KeyCode::Tab if mention_open(&app.input) => app.accept_mention(),
            KeyCode::Tab if slash_menu_open(&app.input) => app.complete_slash(),
            KeyCode::Up if mention_open(&app.input) => app.move_mention(-1),
            KeyCode::Down if mention_open(&app.input) => app.move_mention(1),
            KeyCode::Enter if mention_open(&app.input) => app.accept_mention(),
            KeyCode::Up if slash_menu_open(&app.input) => app.move_slash(-1),
            KeyCode::Down if slash_menu_open(&app.input) => app.move_slash(1),
            KeyCode::PageUp if slash_menu_open(&app.input) => app.move_slash(-1),
            KeyCode::PageDown if slash_menu_open(&app.input) => app.move_slash(1),
            KeyCode::PageUp => app.scroll_transcript(8),
            KeyCode::PageDown => app.scroll_transcript(-8),
            KeyCode::Up => app.scroll_transcript(1),
            KeyCode::Down => app.scroll_transcript(-1),
            KeyCode::Enter | KeyCode::Char('\n') if wants_newline(key) => app.insert_str("\n"),
            KeyCode::Enter if !app.input.trim().is_empty() => submit_chat(terminal, app)?,
            KeyCode::Backspace => app.backspace(),
            KeyCode::Delete => app.delete(),
            KeyCode::Left => app.move_cursor(-1),
            KeyCode::Right => app.move_cursor(1),
            KeyCode::Home => app.input_cursor = 0,
            KeyCode::End => app.input_cursor = app.input.len(),
            KeyCode::Char('a') if ctrl => app.input_cursor = 0,
            KeyCode::Char('e') if ctrl => app.input_cursor = app.input.len(),
            KeyCode::Char(ch) if !ctrl => app.insert_str(&ch.to_string()),
            _ => {}
        },
        View::Return => match key.code {
            KeyCode::Esc => { app.view = View::Chat; app.clear_input(); }
            KeyCode::Enter | KeyCode::Char('\n') if wants_newline(key) => app.insert_str("\n"),
            KeyCode::Enter if !app.input.trim().is_empty() => if let Some(plan) = app.plan.as_ref() { match app.engine.finalize_control_task(&plan.task_id, &app.input) { Ok(()) => { let outcome = app.input.clone(); app.say(format!("Methodus: 已回收任务结果：{outcome}")); app.clear_input(); app.plan = None; app.view = View::Chat; app.refresh(); }, Err(error) => app.status = format!("回收失败：{error}"), } },
            KeyCode::Backspace => app.backspace(), KeyCode::Delete => app.delete(),
            KeyCode::Left => app.move_cursor(-1), KeyCode::Right => app.move_cursor(1),
            KeyCode::Char(ch) if !ctrl => app.insert_str(&ch.to_string()), _ => {}
        },
        View::KnowledgeDetail | View::SkillDetail | View::ExperienceDetail | View::ReviewDetail => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => { app.view = app.detail_return; app.knowledge_scroll = 0; }
            KeyCode::Up => app.knowledge_scroll = app.knowledge_scroll.saturating_sub(1),
            KeyCode::Down => app.knowledge_scroll = app.knowledge_scroll.saturating_add(1),
            KeyCode::PageUp => app.knowledge_scroll = app.knowledge_scroll.saturating_sub(10),
            KeyCode::PageDown => app.knowledge_scroll = app.knowledge_scroll.saturating_add(10),
            KeyCode::Home => app.knowledge_scroll = 0,
            KeyCode::Char('c') if app.view == View::ReviewDetail => commit_candidate(app),
            _ => {}
        },
        View::Help => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => app.view = View::Chat,
            _ => {}
        },
        View::Knowledge => match key.code {
            KeyCode::Esc if !app.knowledge_filter.is_empty() => { app.knowledge_filter.clear(); app.selected = 0; app.status = "已清除 knowledge 筛选".into(); }
            KeyCode::Esc => app.view = View::Chat,
            KeyCode::Enter => open_knowledge_detail(app),
            KeyCode::Up => app.selected = app.selected.saturating_sub(1),
            KeyCode::Down => app.selected = (app.selected + 1).min(app.items_len().saturating_sub(1)),
            KeyCode::Backspace => { app.knowledge_filter.pop(); app.selected = 0; },
            KeyCode::Char(ch) if !ctrl => { app.knowledge_filter.push(ch); app.selected = 0; },
            _ => {}
        },
        View::Skills => match key.code {
            KeyCode::Esc => app.view = View::Chat,
            KeyCode::Enter => open_node_detail(app, View::SkillDetail),
            KeyCode::Up => app.selected = app.selected.saturating_sub(1),
            KeyCode::Down => app.selected = (app.selected + 1).min(app.items_len().saturating_sub(1)),
            _ => {}
        },
        View::Experience => match key.code {
            KeyCode::Esc => app.view = View::Chat,
            KeyCode::Enter => open_node_detail(app, View::ExperienceDetail),
            KeyCode::Up => app.selected = app.selected.saturating_sub(1),
            KeyCode::Down => app.selected = (app.selected + 1).min(app.items_len().saturating_sub(1)),
            _ => {}
        },
        View::Review => match key.code {
            KeyCode::Esc => app.view = View::Chat,
            KeyCode::Enter => open_node_detail(app, View::ReviewDetail),
            KeyCode::Up => app.selected = app.selected.saturating_sub(1),
            KeyCode::Down => app.selected = (app.selected + 1).min(app.items_len().saturating_sub(1)),
            KeyCode::Char('c') => commit_candidate(app),
            _ => {}
        },
        View::Sessions => match key.code {
            KeyCode::Esc => app.view = View::Chat,
            KeyCode::Enter => resume_selected_session(terminal, app)?,
            KeyCode::Up => app.selected = app.selected.saturating_sub(1),
            KeyCode::Down => app.selected = (app.selected + 1).min(app.items_len().saturating_sub(1)),
            KeyCode::Backspace => { app.session_filter.pop(); app.selected = 0; },
            KeyCode::Char('s') if ctrl => { app.session_sort = (app.session_sort + 1) % 3; app.selected = 0; app.status = format!("session 排序：{}", session_sort_label(app.session_sort)); },
            KeyCode::Char(ch) if !ctrl => { app.session_filter.push(ch); app.selected = 0; },
            _ => {}
        },
    }
    Ok(())
}

fn submit_chat(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if app.input.trim_start().starts_with('/') { return command(app); }
    let input = std::mem::take(&mut app.input); app.input_cursor = 0; let text = input.trim();
    app.say(format!("你：{text}"));
    let mode = if text.starts_with("学习：") || text.to_ascii_lowercase().starts_with("learn:") { "learn" } else { "work" };
    let task = match app.engine.create_control_task(text, mode, Some(&app.runtime)) { Ok(task) => task, Err(error) => { app.say(format!("Methodus: 无法创建任务：{error}")); return Ok(()); } };
    app.current_task_id = Some(task.id.clone());
    app.say("Methodus: 正在启动临时规划会话，选择 Skill、Knowledge 与 Experience…");
    app.status = "规划上下文中…".into();
    terminal.draw(|frame| draw(frame, app))?;
    // The app is hosted by Tokio's multi-thread runtime. Planning is a bounded
    // isolated call; `block_in_place` preserves the outer runtime for adapters.
    let planning = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(app.engine.plan_context(&task.id)));
    let plan = match planning {
        Ok(context) => { app.say(format!("Methodus: 规划会话选择了 {} 个上下文节点。", context.selected_node_ids.len())); app.engine.compile_capsule_with_nodes(&task.id, 1_600, &context.selected_node_ids) }
        Err(error) => { app.say(format!("Methodus: 规划会话不可用（{error}）；使用本地匹配继续。")); app.engine.compile_capsule(&task.id, 1_600) }
    };
    match plan {
        Ok(plan) => {
            app.say(format!("Methodus: capsule 已生成，正在切换到 {} 的原生 TUI…", plan.runtime));
            animate_handoff(terminal, app, "正在交接到原生 runtime")?;
            match run_native_handoff(terminal, &app.engine, &plan) {
                Ok(status) => { app.plan = Some(plan); app.status = format!("{} 已返回：{status}", app.runtime); app.input.clear(); app.view = View::Return; }
                Err(error) => app.say(format!("Methodus: 原生交接失败：{error}")),
            }
        }
        Err(error) => app.say(format!("Methodus: 无法编译 capsule：{error}")),
    }
    Ok(())
}

fn command(app: &mut App) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let matches = matching_slash(&app.input);
    let Some(command) = matches.get(app.slash_sel).copied() else { app.status = "未知命令；输入 / 查看可用命令".into(); return Ok(()); };
    let rest = slash_rest(&app.input); app.clear_input();
    match command.name {
        "knowledge" => app.panel(View::Knowledge), "skill" => app.panel(View::Skills), "experience" => app.panel(View::Experience), "session" => app.panel(View::Sessions),
        "review" => app.panel(View::Review),
        "runtime" => cycle_runtime(app),
        "help" => app.panel(View::Help),
        "learn" => { app.input = if rest.is_empty() { "学习：".into() } else { format!("学习：{rest}") }; app.input_cursor = app.input.len(); app.say("Methodus: 补充学习主题后回车；返回时会生成 5W2H 知识候选。"); },
        "open" => open_current_workspace(app),
        "quit" => app.quit = true,
        _ => app.status = format!("未知命令 /{}", command.name),
    }
    Ok(())
}

fn commit_candidate(app: &mut App) {
    if let Some(node) = app.nodes.iter().filter(|node| node.status.as_deref() == Some("candidate")).nth(app.selected) {
        match app.engine.promote_graph_candidate(&node.id) { Ok(()) => { app.status = format!("已提交 {}", node.title); app.refresh(); }, Err(error) => app.status = format!("提交失败：{error}"), }
    }
}

fn resume_selected_session(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let sessions = filtered_sessions(&app.sessions, &app.tasks, &app.session_filter, app.session_sort);
    let Some(session) = sessions.get(app.selected).cloned() else {
        app.status = "当前没有可继续的 session".into();
        return Ok(());
    };
    let Some(task) = app.tasks.iter().find(|task| task.id == session.task_id).cloned() else {
        app.status = "session 对应的任务记录不存在".into();
        return Ok(());
    };
    app.current_task_id = Some(task.id.clone());
    app.runtime = session.runtime.clone();
    app.say(format!("Methodus: 正在继续任务「{}」，重新启动 {}…", task.title, session.runtime));
    app.status = "正在编译任务上下文…".into();
    terminal.draw(|frame| draw(frame, app))?;
    let plan = match app.engine.compile_capsule(&task.id, 1_600) {
        Ok(plan) => plan,
        Err(error) => {
            app.status = format!("无法继续任务：{error}");
            return Ok(());
        }
    };
    let plan = resume_plan(plan, &session);
    animate_handoff(terminal, app, "正在恢复 native session")?;
    match run_native_handoff(terminal, &app.engine, &plan) {
        Ok(status) => {
            app.plan = Some(plan);
            app.status = format!("{} 已返回：{status}", app.runtime);
            app.input.clear();
            app.view = View::Return;
            app.refresh();
        }
        Err(error) => app.status = format!("原生交接失败：{error}"),
    }
    Ok(())
}

fn run_native_handoff(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, engine: &Engine, plan: &NativeHandoffPlan) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let launch_id = engine.record_native_handoff(plan)?; disable_raw_mode()?; let _ = stdout().execute(PopKeyboardEnhancementFlags); let _ = stdout().execute(DisableBracketedPaste); stdout().execute(LeaveAlternateScreen)?;
    let result = ProcessCommand::new(&plan.program).args(&plan.args).current_dir(&plan.cwd).status();
    enable_raw_mode()?; stdout().execute(EnterAlternateScreen)?; stdout().execute(EnableBracketedPaste)?; let _ = stdout().execute(PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS)); terminal.clear()?;
    match result { Ok(status) => { let label = status.code().map(|code| format!("exit {code}")).unwrap_or_else(|| "terminated".into()); engine.record_native_return(&launch_id, &plan.task_id, &label)?; Ok(label) }, Err(error) => { let label = format!("launch error: {error}"); let _ = engine.record_native_return(&launch_id, &plan.task_id, &label); Err(Box::new(error)) } }
}

fn resume_plan(mut plan: NativeHandoffPlan, session: &Session) -> NativeHandoffPlan {
    plan.args = resume_args(&plan.runtime, session.executor_sid.as_deref());
    plan
}

fn resume_args(runtime: &str, executor_sid: Option<&str>) -> Vec<String> {
    match (runtime, executor_sid) {
        ("claude-code", Some(sid)) => vec!["--resume".into(), sid.into()],
        ("claude-code", None) => vec!["--continue".into()],
        // Codex and Cursor session identifiers are runtime-specific. Starting
        // their native TUI without the task brief preserves the key invariant:
        // a resumed session must not auto-submit a new task.
        ("codex" | "cursor", _) => Vec::new(),
        _ => Vec::new(),
    }
}

fn animate_handoff(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App, message: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    app.handoff_message = Some(message.to_string());
    for frame in 0..8 {
        app.handoff_frame = frame;
        app.status = message.to_string();
        terminal.draw(|screen| draw(screen, app))?;
        thread::sleep(Duration::from_millis(85));
    }
    app.handoff_message = None;
    Ok(())
}

fn draw_handoff(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let popup = centered(area, 48, 32);
    frame.render_widget(Clear, popup);
    let spinner = ["◐", "◓", "◑", "◒"][app.handoff_frame % 4];
    let current = app.handoff_frame / 3;
    let step = |index: usize, label: &str| {
        if index < current { format!("✓  {label}") } else if index == current { format!("{spinner}  {label}") } else { format!("·  {label}") }
    };
    let runtime_label = match app.runtime.as_str() { "claude-code" => "Claude Code", "codex" => "Codex", "cursor" => "Cursor", other => other };
    let runtime_step = format!("切换到 {runtime_label}");
    let lines = vec![
        Line::from(Span::styled(app.handoff_message.as_deref().unwrap_or("交接中"), theme.accent())),
        Line::default(),
        Line::from(step(0, "整理任务上下文")),
        Line::from(step(1, "准备 capsule 与引用")),
        Line::from(step(2, &runtime_step)),
    ];
    frame.render_widget(Paragraph::new(lines).style(theme.text()).block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(theme.border(true)).title(" Methodus handoff ")), popup);
}

fn draw(frame: &mut Frame, app: &App) {
    let theme = Theme::current();
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(theme.surface)), area);
    if area.width < 80 || area.height < 24 {
        frame.render_widget(Paragraph::new(format!("{MARK}  {WORDMARK}\n\nterminal too small — need 80 × 24")).style(theme.dim()).alignment(Alignment::Center), area);
        return;
    }
    let rows = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(1), Constraint::Min(5), Constraint::Length(1)]).split(area);
    draw_header(frame, rows[0], app, &theme);
    match app.view {
        View::Chat => draw_chat(frame, rows[1], app, &theme),
        View::Return => draw_return(frame, rows[1], app, &theme),
        View::KnowledgeDetail | View::SkillDetail | View::ExperienceDetail | View::ReviewDetail => draw_knowledge_detail(frame, rows[1], app, &theme),
        _ => frame.render_widget(Block::default().style(Style::default().bg(theme.overlay)), rows[1]),
    }
    if !matches!(app.view, View::Chat | View::Return | View::KnowledgeDetail | View::SkillDetail | View::ExperienceDetail | View::ReviewDetail) { draw_overlay(frame, rows[1], app, &theme); }
    if app.handoff_message.is_some() { draw_handoff(frame, area, app, &theme); }
    draw_footer(frame, rows[2], app, &theme);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let active = match app.view { View::Chat => "task", View::Help => "help", View::Knowledge | View::KnowledgeDetail => "knowledge", View::Skills | View::SkillDetail => "skill", View::Experience | View::ExperienceDetail => "experience", View::Sessions => "session", View::Review | View::ReviewDetail => "review", View::Return => "return" };
    let candidates = app.nodes.iter().filter(|node| node.status.as_deref() == Some("candidate")).count();
    let line = Line::from(vec![Span::styled(format!(" {MARK} "), theme.accent()), Span::styled(WORDMARK, theme.accent()), Span::styled(format!("  {} · {active}", app.runtime), theme.dim()), Span::styled(format!("  graph:{}", app.nodes.len()), theme.dim()), Span::styled(if candidates == 0 { String::new() } else { format!("  ▣{candidates}") }, Style::default().fg(theme.warning))]);
    frame.render_widget(Paragraph::new(line).style(theme.text()), area);
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let status_style = if app.status.contains("失败") || app.status.contains("error") { Style::default().fg(theme.error).add_modifier(Modifier::BOLD) } else if app.status.contains("已") { Style::default().fg(theme.success) } else { Style::default().fg(theme.info) };
    frame.render_widget(Paragraph::new(app.status.as_str()).style(status_style), area);
}

fn draw_chat(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let slash_open = slash_menu_open(&app.input);
    let mention_open = !slash_open && mention_open(&app.input);
    let slash_height = if slash_open { (matching_slash(&app.input).len().max(1) as u16 + 2).min(8) } else { 0 };
    let mention_height = if mention_open { (app.matching_mentions().len().max(1) as u16 + 2).min(8) } else { 0 };
    let composer_height = composer_height(&app.input, area.width);
    let mut constraints = vec![Constraint::Min(3)];
    if slash_open { constraints.push(Constraint::Length(slash_height)); }
    if mention_open { constraints.push(Constraint::Length(mention_height)); }
    constraints.push(Constraint::Length(composer_height));
    let rows = Layout::default().direction(Direction::Vertical).constraints(constraints).split(area);
    draw_transcript(frame, rows[0], app, theme);
    let mut index = 1;
    if slash_open { draw_slash_menu(frame, rows[index], app, theme); index += 1; }
    if mention_open { draw_mention_menu(frame, rows[index], app, theme); index += 1; }
    draw_composer(frame, rows[index], app, theme);
}

fn draw_mention_menu(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let matches = app.matching_mentions();
    let items = if matches.is_empty() { vec![ListItem::new("  no matching path").style(theme.dim())] } else { matches.iter().map(|candidate| ListItem::new(format!("  {}{}", if candidate.is_dir { "▸ " } else { "· " }, candidate.label)).style(theme.text())).collect() };
    let mut state = ListState::default(); if !matches.is_empty() { state.select(Some(app.mention_sel)); }
    frame.render_stateful_widget(List::new(items).block(Block::default().borders(Borders::TOP).border_style(theme.border(true)).title(" @  ↑↓ select · Tab complete · Enter attach ")).highlight_style(theme.selected()).highlight_symbol("› "), area, &mut state);
}

fn draw_transcript(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let width = area.width.max(8) as usize;
    let cached = TRANSCRIPT_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.version != app.transcript_version || cache.width != width {
            cache.rows = layout_transcript(&app.transcript, width);
            cache.version = app.transcript_version;
            cache.width = width;
        }
        cache.rows.clone()
    });
    let height = area.height.max(1) as usize;
    let max_offset = cached.len().saturating_sub(height);
    let offset = app.transcript_offset.min(max_offset);
    let end = cached.len().saturating_sub(offset);
    let start = end.saturating_sub(height);
    let lines = if cached.is_empty() {
        vec![Line::default(), Line::from(Span::styled(format!("{MARK}  {WORDMARK}"), theme.accent())), Line::from(Span::styled("Describe a task below; Methodus will plan context and hand off to the native Agent.", theme.dim()))]
    } else {
        cached[start..end].iter().map(|row| transcript_line(row, theme)).collect()
    };
    frame.render_widget(Paragraph::new(lines).style(theme.text()), area);
}

fn open_knowledge_detail(app: &mut App) {
    let Some(node) = filtered_knowledge_nodes(&app.nodes, &app.knowledge_filter).get(app.selected).cloned() else {
        app.status = "当前没有可打开的 knowledge".into();
        return;
    };
    let path = app.engine.home().join(&node.path);
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            app.knowledge_content = content;
            app.knowledge_scroll = 0;
            app.detail_return = View::Knowledge;
            app.detail_node_id = node.id;
            app.view = View::KnowledgeDetail;
            app.status = format!("正在阅读：{}", node.title);
        }
        Err(error) => app.status = format!("无法读取 knowledge：{error}"),
    }
}

fn open_node_detail(app: &mut App, detail_view: View) {
    let nodes: Vec<GraphNode> = match app.view {
        View::Skills => app.nodes.iter().filter(|node| node.node_type == "skill").cloned().collect(),
        View::Experience => app.nodes.iter().filter(|node| node.node_type == "experience").cloned().collect(),
        View::Review => app.nodes.iter().filter(|node| node.status.as_deref() == Some("candidate")).cloned().collect(),
        _ => Vec::new(),
    };
    let Some(node) = nodes.get(app.selected).cloned() else { app.status = "当前没有可打开的内容".into(); return; };
    let path = app.engine.home().join(&node.path);
    match std::fs::read_to_string(&path) {
        Ok(content) => { app.knowledge_content = content; app.knowledge_scroll = 0; app.detail_return = app.view; app.detail_node_id = node.id; app.view = detail_view; app.status = format!("正在阅读：{}", node.title); }
        Err(error) => app.status = format!("无法读取内容：{error}"),
    }
}

fn draw_knowledge_detail(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let node = app.nodes.iter().find(|node| node.id == app.detail_node_id);
    let Some(node) = node else { return; };
    let kind = match app.view { View::SkillDetail => "skill", View::ExperienceDetail => "experience", View::ReviewDetail => "review", _ => "knowledge" };
    let action = if app.view == View::ReviewDetail { "c approve · Esc back" } else { "↑↓ / PgUp/PgDn scroll · Esc back" };
    let title = Line::from(vec![Span::styled(format!(" {} · {} ", kind, node.title), theme.accent()), Span::styled(action, theme.dim())]);
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(theme.border(true)).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(app.knowledge_content.as_str()).style(theme.text()).wrap(Wrap { trim: false }).scroll((app.knowledge_scroll, 0)), inner);
}

fn filtered_knowledge_nodes(nodes: &[GraphNode], filter: &str) -> Vec<GraphNode> {
    let needle = filter.trim().to_lowercase();
    nodes.iter().filter(|node| {
        node.node_type == "knowledge" && (needle.is_empty()
            || node.title.to_lowercase().contains(&needle)
            || node.summary.as_deref().unwrap_or_default().to_lowercase().contains(&needle)
            || node.path.to_lowercase().contains(&needle))
    }).cloned().collect()
}

fn session_sort_label(sort: u8) -> &'static str { match sort { 1 => "名称", 2 => "状态", _ => "最近" } }

fn cycle_runtime(app: &mut App) {
    let mut config = UserConfig::load(app.engine.home());
    config.cycle_runtime();
    app.runtime = config.default_runtime.clone().unwrap_or_else(|| "claude-code".into());
    let _ = config.save(app.engine.home());
    app.status = format!("runtime：{}", app.runtime);
}

fn cycle_permission(app: &mut App) {
    let mut config = UserConfig::load(app.engine.home());
    config.cycle_permission();
    app.permission = config.permission_mode.clone().unwrap_or_else(|| "acceptEdits".into());
    let _ = config.save(app.engine.home());
    app.status = format!("permission：{}", PermissionMode::parse(Some(&app.permission)).label());
}

fn filtered_sessions(sessions: &[Session], tasks: &[Task], filter: &str, sort: u8) -> Vec<Session> {
    let needle = filter.trim().to_lowercase();
    let mut result: Vec<Session> = sessions.iter().filter(|session| {
        let task = tasks.iter().find(|task| task.id == session.task_id);
        let title = task.map(|task| task.title.as_str()).unwrap_or_default();
        needle.is_empty() || title.to_lowercase().contains(&needle) || session.task_id.to_lowercase().contains(&needle) || session.runtime.to_lowercase().contains(&needle) || session.status.to_string().to_lowercase().contains(&needle) || session.cwd.to_lowercase().contains(&needle)
    }).cloned().collect();
    match sort {
        1 => result.sort_by_key(|session| tasks.iter().find(|task| task.id == session.task_id).map(|task| task.title.to_lowercase()).unwrap_or_else(|| session.task_id.to_lowercase())),
        2 => result.sort_by_key(|session| (session.status.to_string(), std::cmp::Reverse(session.updated_at))),
        _ => result.sort_by_key(|session| std::cmp::Reverse(session.updated_at)),
    }
    result
}

fn draw_slash_menu(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let matches = matching_slash(&app.input);
    let items = if matches.is_empty() { vec![ListItem::new("  no matching command").style(theme.dim())] } else { matches.iter().map(|command| ListItem::new(format!("  /{}  {}", command.name, command.summary)).style(theme.text())).collect() };
    let mut state = ListState::default(); if !matches.is_empty() { state.select(Some(app.slash_sel)); }
    frame.render_stateful_widget(List::new(items).block(Block::default().borders(Borders::TOP).border_style(theme.border(true)).title(" /  ↑↓ select · Tab complete · Enter run ")).highlight_style(theme.selected()).highlight_symbol("› "), area, &mut state);
}

fn draw_composer(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let inner_width = area.width.saturating_sub(4).max(8) as usize;
    let (shown, cursor_col, cursor_row) = composer_view(&app.input, app.input_cursor, inner_width, COMPOSER_MAX_ROWS);
    let lines = if shown.is_empty() { vec![Line::from(vec![Span::styled("› ", theme.dim()), Span::styled("describe a task or type /", theme.dim())])] } else { shown.into_iter().map(Line::from).collect() };
    let title = Line::from(vec![Span::styled(" task ", theme.accent()), Span::styled("Enter handoff · Shift+Enter newline", theme.dim()), Span::styled(format!("  {} · {} ", app.runtime, PermissionMode::parse(Some(&app.permission)).label()), theme.dim())]);
    frame.render_widget(Paragraph::new(lines).style(theme.text()).block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(theme.border(true)).title(title)), area);
    let x = area.x.saturating_add(1).saturating_add(cursor_col);
    let y = area.y.saturating_add(1).saturating_add(cursor_row);
    if x < area.right().saturating_sub(1) && y < area.bottom().saturating_sub(1) { frame.set_cursor_position(Position { x, y }); }
}

fn draw_return(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let composer_height = composer_height(&app.input, area.width);
    let rows = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(4), Constraint::Length(composer_height)]).split(area);
    frame.render_widget(Paragraph::new(vec![Line::from(Span::styled("Native Agent returned", theme.accent())), Line::default(), Line::from("Write the outcome, evidence, limitation, or next step. Methodus stores an Experience; learning tasks also create a 5W2H candidate.")]).wrap(Wrap { trim: true }).style(theme.text()), rows[0]);
    let inner_width = rows[1].width.saturating_sub(4).max(8) as usize;
    let (shown, cursor_col, cursor_row) = composer_view(&app.input, app.input_cursor, inner_width, COMPOSER_MAX_ROWS);
    frame.render_widget(Paragraph::new(shown.into_iter().map(Line::from).collect::<Vec<_>>()).style(theme.text()).block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(theme.border(true)).title(format!(" outcome · Enter save · Shift+Enter newline · {} ", PermissionMode::parse(Some(&app.permission)).label()))), rows[1]);
    let x = rows[1].x.saturating_add(1).saturating_add(cursor_col); let y = rows[1].y.saturating_add(1).saturating_add(cursor_row);
    if x < rows[1].right().saturating_sub(1) && y < rows[1].bottom().saturating_sub(1) { frame.set_cursor_position(Position { x, y }); }
}

fn draw_overlay(frame: &mut Frame, base: Rect, app: &App, theme: &Theme) {
    let popup = centered(base, 86, 78); frame.render_widget(Clear, popup);
    let title = match app.view { View::Help => " help  ·  commands & controls ".into(), View::Knowledge => format!(" knowledge  ·  filter: {} ", if app.knowledge_filter.is_empty() { "type to filter" } else { &app.knowledge_filter }), View::Skills => " skill  ·  reusable procedures ".into(), View::Experience => " experience  ·  task learnings ".into(), View::Sessions => format!(" session  ·  filter: {}  ·  sort: {} ", if app.session_filter.is_empty() { "all" } else { &app.session_filter }, session_sort_label(app.session_sort)), View::Review => " review  ·  candidates ".into(), _ => return };
    let inner = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(theme.border(true)).style(Style::default().bg(theme.overlay).fg(theme.overlay_fg)).title(Span::styled(title.clone(), theme.accent())).inner(popup);
    frame.render_widget(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(theme.border(true)).style(Style::default().bg(theme.overlay).fg(theme.overlay_fg)).title(Span::styled(title, theme.accent())), popup);
    match app.view { View::Help => draw_help(frame, inner, theme), View::Knowledge | View::Skills | View::Experience | View::Review => draw_nodes(frame, inner, app, theme), View::Sessions => draw_sessions(frame, inner, app, theme), _ => {} }
}

fn draw_nodes(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let nodes: Vec<GraphNode> = match app.view { View::Knowledge => filtered_knowledge_nodes(&app.nodes, &app.knowledge_filter), View::Skills => app.nodes.iter().filter(|n| n.node_type == "skill").cloned().collect(), View::Experience => app.nodes.iter().filter(|n| n.node_type == "experience").cloned().collect(), _ => app.nodes.iter().filter(|n| n.status.as_deref() == Some("candidate")).cloned().collect() };
    let cols = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(42), Constraint::Percentage(58)]).split(area);
    let items = nodes.iter().map(|n| ListItem::new(Line::from(vec![Span::styled(format!("{}  ", n.status.as_deref().unwrap_or("—")), theme.dim()), Span::styled(&n.title, theme.text())]))).collect::<Vec<_>>();
    let mut state = ListState::default(); if !nodes.is_empty() { state.select(Some(app.selected)); }
    frame.render_stateful_widget(List::new(items).highlight_style(theme.selected()).highlight_symbol("› "), cols[0], &mut state);
    let filter_hint = if app.view == View::Knowledge && !app.knowledge_filter.is_empty() { format!("filter     {}\n\n", app.knowledge_filter) } else { String::new() };
    let detail = nodes.get(app.selected).map(|node| format!("{}{}\n\n{}\n\nstatus      {}\nconfidence  {}\npath        {}\n\n{}", filter_hint, node.title, node.summary.as_deref().unwrap_or("No summary available."), node.status.as_deref().unwrap_or("—"), node.confidence.map(|v| format!("{v:.2}")).unwrap_or_else(|| "—".into()), node.path, if app.view == View::Review { "Press c to commit this candidate." } else { "Press Enter to read the complete Markdown note." })).unwrap_or_else(|| if app.view == View::Knowledge && !app.knowledge_filter.is_empty() { format!("No knowledge matches ‘{}’.\n\nType to change the filter or press Esc to clear it.", app.knowledge_filter) } else { "Nothing here yet.".into() });
    frame.render_widget(Paragraph::new(detail).style(Style::default().fg(theme.overlay_fg).bg(theme.overlay)).wrap(Wrap { trim: true }).block(Block::default().borders(Borders::LEFT).border_style(theme.border(false))), cols[1]);
}

fn draw_sessions(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let cols = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(48), Constraint::Percentage(52)]).split(area);
    let sessions = filtered_sessions(&app.sessions, &app.tasks, &app.session_filter, app.session_sort);
    let items = sessions.iter().map(|session| { let name = app.tasks.iter().find(|task| task.id == session.task_id).map(|task| task.title.as_str()).unwrap_or(session.task_id.as_str()); ListItem::new(Line::from(vec![Span::styled(format!("{}  ", session.status), theme.dim()), Span::styled(session.runtime.as_str(), theme.accent()), Span::styled(format!("  {}", name), theme.text())])).style(theme.text()) }).collect::<Vec<_>>(); let mut state = ListState::default(); if !sessions.is_empty() { state.select(Some(app.selected)); }
    frame.render_stateful_widget(List::new(items).highlight_style(theme.selected()).highlight_symbol("› "), cols[0], &mut state);
    let detail = sessions.get(app.selected).map(|session| {
        let task = app.tasks.iter().find(|task| task.id == session.task_id);
        format!("{}\n\n{}\n\nruntime   {}\nstatus    {}\nstarted   {}\nworkspace {}\n\nEnter continues this task in a new native runtime session.", task.map(|item| item.title.as_str()).unwrap_or("Unknown task"), task.map(|item| item.request.as_str()).unwrap_or("Task record is unavailable."), session.runtime, session.status, session.started_at.format("%Y-%m-%d %H:%M"), session.cwd)
    }).unwrap_or_else(|| "No sessions yet.\n\nCompleted and active task runs will appear here.".into());
    frame.render_widget(Paragraph::new(detail).style(Style::default().fg(theme.overlay_fg).bg(theme.overlay)).wrap(Wrap { trim: true }).block(Block::default().borders(Borders::LEFT).border_style(theme.border(false))), cols[1]);
}

fn draw_help(frame: &mut Frame, area: Rect, theme: &Theme) {
    let lines = vec![
        Line::from(Span::styled("Commands", theme.accent())),
        Line::from("/knowledge   浏览、筛选并阅读知识图谱"),
        Line::from("/skill       浏览并阅读可复用技能"),
        Line::from("/experience  浏览任务经验与复盘记录"),
        Line::from("/session     继续历史任务的 native session"),
        Line::from("/review      查看候选内容并审批晋升"),
        Line::from("/runtime     切换 Claude Code / Codex / Cursor"),
        Line::from("/learn       创建学习任务并生成 5W2H 候选"),
        Line::from("/open        打开当前 capsule 工作区"),
        Line::from("/quit        退出 Methodus；q 不会退出"),
        Line::default(),
        Line::from(Span::styled("Navigation", theme.accent())),
        Line::from("/             打开命令面板；↑↓ 选择；Tab 补全；Enter 执行"),
        Line::from("Knowledge     直接输入筛选；Enter 阅读；Esc 清除/返回"),
        Line::from("Skill/Exp.    ↑↓ 选择；Enter 阅读完整文档；Esc 返回"),
        Line::from("Review        Enter 阅读；c 审批候选；Esc 返回"),
        Line::from("Composer      Enter 交接；Shift+Enter 换行；PgUp/PgDn 滚动"),
        Line::from("Exit          Ctrl+C 清空输入；空输入时三秒内再次按退出"),
    ];
    frame.render_widget(Paragraph::new(lines).style(theme.text()).wrap(Wrap { trim: false }), area);
}

fn centered(area: Rect, width_pct: u16, height_pct: u16) -> Rect { let width = area.width.saturating_mul(width_pct).saturating_div(100).max(50).min(area.width); let height = area.height.saturating_mul(height_pct).saturating_div(100).max(10).min(area.height); Rect { x: area.x + area.width.saturating_sub(width) / 2, y: area.y + area.height.saturating_sub(height) / 2, width, height } }

fn slash_menu_open(input: &str) -> bool { input.trim_start().starts_with('/') }
fn mention_open(input: &str) -> bool { at_query(input).is_some() }
fn mention_start(input: &str) -> Option<usize> {
    let query = at_query(input)?;
    let end = input.len();
    Some(end.saturating_sub(query.len() + 1))
}
fn absolute_path_candidates(query: &str) -> Vec<MentionCandidate> {
    let (display_prefix, path_query) = if query == "~" || query.starts_with("~/") {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        ("~".to_string(), home.map(|home| home.join(query.trim_start_matches("~/").trim_start_matches('~'))))
    } else if query.starts_with('/') {
        (String::new(), Some(std::path::PathBuf::from(query)))
    } else { return Vec::new(); };
    let path_query = path_query.unwrap_or_default();
    let (parent, partial) = if path_query.is_dir() { (path_query.clone(), String::new()) } else { (path_query.parent().unwrap_or(std::path::Path::new("/")).to_path_buf(), path_query.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default()) };
    let Ok(entries) = fs::read_dir(&parent) else { return Vec::new(); };
    entries.flatten().filter_map(|entry| {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || (!partial.is_empty() && !name.to_lowercase().contains(&partial)) { return None; }
        let path = entry.path(); let is_dir = path.is_dir();
        let label = if display_prefix == "~" { format!("~/{}{}", path.strip_prefix(std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default()).ok()?.display(), if is_dir { "/" } else { "" }) } else { format!("{}{}", path.display(), if is_dir { "/" } else { "" }) };
        Some(MentionCandidate { rel: label.clone(), label, is_dir, abs: path })
    }).take(80).collect()
}
fn slash_token(input: &str) -> Option<String> { let rest = input.trim_start().strip_prefix('/')?; Some(rest.split_whitespace().next().unwrap_or("").to_ascii_lowercase()) }
fn matching_slash(input: &str) -> Vec<&'static SlashCmd> { let Some(token) = slash_token(input) else { return Vec::new() }; SLASH_COMMANDS.iter().filter(|command| command.name.starts_with(&token) || command.aliases.iter().any(|alias| alias.starts_with(&token))).collect() }
fn slash_rest(input: &str) -> String { input.trim_start().strip_prefix('/').and_then(|rest| rest.split_once(char::is_whitespace)).map(|(_, rest)| rest.trim().to_string()).unwrap_or_default() }

fn prev_char_boundary(text: &str, index: usize) -> usize { text[..index.min(text.len())].char_indices().next_back().map(|(index, _)| index).unwrap_or(0) }
fn next_char_boundary(text: &str, index: usize) -> usize { let index = index.min(text.len()); text[index..].char_indices().nth(1).map(|(offset, _)| index + offset).unwrap_or(text.len()) }
fn floor_char_boundary(text: &str, mut index: usize) -> usize { index = index.min(text.len()); while index > 0 && !text.is_char_boundary(index) { index -= 1; } index }
fn display_cols(text: &str) -> u16 { UnicodeWidthStr::width(text) as u16 }
fn wants_newline(key: KeyEvent) -> bool { key.modifiers.contains(KeyModifiers::SHIFT) && matches!(key.code, KeyCode::Enter | KeyCode::Char('\n')) }

/// Wrap by terminal display columns, so Chinese/fullwidth glyphs occupy two cells.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 { return vec![text.to_string()]; }
    let mut output = Vec::new();
    for raw in text.split('\n') {
        if raw.is_empty() { output.push(String::new()); continue; }
        let mut line = String::new(); let mut columns = 0usize;
        for character in raw.chars() {
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if columns > 0 && columns + character_width > width { output.push(std::mem::take(&mut line)); columns = 0; }
            line.push(character); columns = columns.saturating_add(character_width);
        }
        output.push(line);
    }
    if output.is_empty() { output.push(String::new()); }
    output
}

fn composer_height(input: &str, width: u16) -> u16 { let inner = width.saturating_sub(4).max(8) as usize; let rows = wrap_text(input, inner.saturating_sub(2).max(1)).len().clamp(1, COMPOSER_MAX_ROWS); (rows as u16 + 2).clamp(3, 10) }

/// Multiline composer that tracks a UTF-8 byte cursor while positioning the
/// hardware cursor in terminal columns. This is the key CJK/IME invariant.
fn composer_view(input: &str, cursor: usize, width: usize, max_rows: usize) -> (Vec<String>, u16, u16) {
    let cursor = floor_char_boundary(input, cursor);
    let inner = width.saturating_sub(2).max(1);
    let mut visual = Vec::new(); let mut cursor_row = 0u16; let mut cursor_col = 2u16; let mut found = false; let mut global = 0usize;
    let logical: Vec<&str> = if input.is_empty() { vec![""] } else { input.split('\n').collect() };
    for (line_index, raw) in logical.iter().enumerate() {
        let line_start = global; let wrapped = wrap_text(raw, inner); let mut local = 0usize;
        for (wrap_index, piece) in wrapped.iter().enumerate() {
            visual.push(format!("{}{}", if line_index == 0 && wrap_index == 0 { "› " } else { "  " }, piece));
            let start = line_start + local; let end = start + piece.len();
            if !found && cursor >= start && cursor <= end { found = true; cursor_row = visual.len().saturating_sub(1) as u16; let offset = floor_char_boundary(piece, cursor.saturating_sub(start)); cursor_col = 2 + display_cols(&piece[..offset]); }
            local += piece.len();
        }
        global = line_start + raw.len();
        if line_index + 1 < logical.len() { if !found && cursor == global { found = true; cursor_row = visual.len().saturating_sub(1) as u16; cursor_col = visual.last().map(|line| display_cols(line)).unwrap_or(2); } global += 1; }
    }
    if visual.is_empty() { visual.push("› ".into()); }
    let max_rows = max_rows.max(1);
    if visual.len() > max_rows { let row = cursor_row as usize; let start = row.saturating_add(1).saturating_sub(max_rows); return (visual[start..start + max_rows].to_vec(), cursor_col, (row - start) as u16); }
    (visual, cursor_col, cursor_row)
}

fn layout_transcript(entries: &[String], width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for entry in entries {
        let (label, body) = if let Some(body) = entry.strip_prefix("你：") { ("you", body) } else if let Some(body) = entry.strip_prefix("Methodus:") { ("methodus", body) } else { ("methodus", entry.as_str()) };
        let prefix = format!("{label:<9}"); let continuation = " ".repeat(9); let body_width = width.saturating_sub(9).max(8);
        for (index, line) in wrap_text(body.trim(), body_width).into_iter().enumerate() { rows.push(format!("{}{}", if index == 0 { &prefix } else { &continuation }, line)); }
        rows.push(String::new());
    }
    rows
}

fn transcript_line(row: &str, theme: &Theme) -> Line<'static> {
    if row.is_empty() { return Line::default(); }
    if let Some(body) = row.strip_prefix("you      ") { return Line::from(vec![Span::styled("you      ", theme.accent()), Span::styled(body.to_string(), theme.text())]); }
    if let Some(body) = row.strip_prefix("methodus ") { return Line::from(vec![Span::styled("methodus ", theme.dim().add_modifier(Modifier::BOLD)), Span::styled(body.to_string(), theme.dim())]); }
    Line::from(Span::styled(row.to_string(), theme.dim()))
}

fn open_current_workspace(app: &mut App) {
    let root = app.engine.workspace_root();
    let stored = app.current_task_id.as_deref().and_then(|task_id| app.engine.store().workspace_path_for_task(task_id).ok().flatten()).map(PathBuf::from);
    let path = app.plan.as_ref().map(|plan| plan.capsule_root.clone()).filter(|path| path.is_dir()).or_else(|| stored.filter(|path| path.is_dir())).unwrap_or(root);
    if let Err(error) = std::fs::create_dir_all(&path) { app.status = format!("无法创建 {}：{error}", path.display()); return; }
    match spawn_file_manager(&path) { Ok(()) => app.status = format!("已打开 {}", path.display()), Err(error) => app.status = error }
}

fn spawn_file_manager(path: &Path) -> Result<(), String> {
    let binary = if cfg!(target_os = "macos") { "open" } else if cfg!(target_os = "windows") { "explorer" } else { "xdg-open" };
    ProcessCommand::new(binary).arg(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|_| ()).map_err(|error| format!("无法打开 {}：{error}", path.display()))
}

fn ctrl_c_should_quit(pending: Option<Instant>, now: Instant) -> bool { pending.is_some_and(|then| now.duration_since(then) <= CTRL_C_QUIT_WINDOW) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_palette_exposes_management_and_lifecycle_commands() {
        let names = matching_slash("/").into_iter().map(|command| command.name).collect::<Vec<_>>();
        for required in ["knowledge", "skill", "experience", "session", "runtime", "review", "learn", "open", "quit"] { assert!(names.contains(&required), "missing /{required}"); }
        assert_eq!(matching_slash("/exit")[0].name, "quit");
        assert_eq!(matching_slash("/op")[0].name, "open");
    }

    #[test]
    fn only_shift_enter_creates_multiline_input() {
        assert!(wants_newline(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)));
        assert!(!wants_newline(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL)));
        assert!(!wants_newline(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    }

    #[test]
    fn resumed_native_plan_does_not_submit_a_new_brief() {
        assert!(resume_args("codex", None).is_empty());
    }

    #[test]
    fn composer_cursor_uses_cjk_display_columns_and_byte_boundaries() {
        let (rows, column, row) = composer_view("你好", "你".len(), 20, 8);
        assert_eq!(rows, vec!["› 你好"]);
        assert_eq!(column, 4);
        assert_eq!(row, 0);
        assert_eq!(prev_char_boundary("你好", "你好".len()), "你".len());
        assert_eq!(next_char_boundary("你好", 0), "你".len());
    }

    #[test]
    fn transcript_wraps_by_terminal_width() {
        assert_eq!(wrap_text("一二三四", 4), vec!["一二", "三四"]);
        assert_eq!(UnicodeWidthStr::width("一二"), 4);
    }

    #[test]
    fn ctrl_c_requires_second_press_inside_window() {
        let now = Instant::now();
        assert!(!ctrl_c_should_quit(None, now));
        assert!(ctrl_c_should_quit(Some(now), now));
        assert!(!ctrl_c_should_quit(Some(now - Duration::from_secs(4)), now));
    }
}
