//! Maintainer-facing Methodus studio.
//!
//! The main surface is a Learn conversation. Slash commands open the small set
//! of maintenance panels; ordinary coding work is intentionally left to the
//! user's native Agent runtime.

use std::cell::RefCell;
use std::fs;
use std::io::stdout;
use std::process::Command;
use std::time::{Duration, Instant};

use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use methodus_core::{at_query, list_from_roots, Engine, MentionCandidate, UserConfig};
use methodus_domain::{GraphEdge, GraphNode};
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
struct RuntimeOption { id: &'static str, label: &'static str, detail: &'static str }

const RUNTIMES: &[RuntimeOption] = &[
    RuntimeOption { id: "claude-code", label: "Claude Code", detail: "session · native approvals" },
    RuntimeOption { id: "codex", label: "Codex", detail: "thread · sandbox" },
    RuntimeOption { id: "cursor", label: "Cursor Agent", detail: "session · auto-review" },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SlashCmd { name: &'static str, aliases: &'static [&'static str], summary: &'static str }

const SLASH_COMMANDS: &[SlashCmd] = &[
    SlashCmd { name: "knowledge", aliases: &[], summary: "Browse the knowledge graph and related nodes" },
    SlashCmd { name: "method", aliases: &["methods"], summary: "Browse reusable working methods" },
    SlashCmd { name: "experience", aliases: &["experiences"], summary: "Review validated task experience" },
    SlashCmd { name: "review", aliases: &["inbox"], summary: "Review pending knowledge candidates" },
    SlashCmd { name: "team", aliases: &[], summary: "Inspect the Team knowledge root and publish scope" },
    SlashCmd { name: "health", aliases: &[], summary: "Check sources, indexes, and connector status" },
    SlashCmd { name: "runtime", aliases: &[], summary: "Switch the runtime used by Learn" },
    SlashCmd { name: "new", aliases: &[], summary: "Close the current context and start a new Learn" },
    SlashCmd { name: "open", aliases: &[], summary: "Open the current node or a path" },
    SlashCmd { name: "help", aliases: &["?"], summary: "Show maintainer commands and controls" },
    SlashCmd { name: "quit", aliases: &["exit"], summary: "Exit Methodus (q does not quit)" },
];

#[derive(Default)]
struct TranscriptCache { version: u64, width: usize, rows: Vec<String> }
thread_local! { static TRANSCRIPT_CACHE: RefCell<TranscriptCache> = RefCell::new(TranscriptCache::default()); }

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
    fn overlay_text(&self) -> Style { Style::default().fg(self.overlay_fg).bg(self.overlay) }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Chat, Help, Knowledge, Method, Experience, Review, Team, Health, Runtime,
    KnowledgeDetail, ExperienceDetail, ReviewDetail, KnowledgeGraph, MergeTarget,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfirmAction { Commit, Reject, MarkTeam, Delete, Revalidate, Merge }

struct App {
    engine: Engine,
    view: View,
    input: String,
    transcript: Vec<String>,
    show_welcome: bool,
    nodes: Vec<GraphNode>,
    selected: usize,
    status: String,
    runtime: String,
    runtime_selected: usize,
    permission_mode: String,
    quit: bool,
    input_cursor: usize,
    slash_sel: usize,
    transcript_offset: usize,
    transcript_version: u64,
    knowledge_content: String,
    knowledge_scroll: u16,
    detail_return: View,
    detail_node_id: String,
    knowledge_filter: String,
    method_filter: String,
    experience_filter: String,
    review_filter: String,
    merge_filter: String,
    filtering: bool,
    merge_candidate_id: Option<String>,
    mention_cache: Vec<MentionCandidate>,
    mention_dynamic: Vec<MentionCandidate>,
    mention_sel: usize,
    graph_nodes: Vec<GraphNode>,
    graph_edges: Vec<GraphEdge>,
    graph_selected: usize,
    graph_return: View,
    pending_quit_at: Option<Instant>,
    learning_session_id: Option<String>,
    learning_executor_sid: Option<String>,
    learning_goal: Option<String>,
    team: Option<methodus_core::TeamStatus>,
    confirmation: Option<ConfirmAction>,
}

impl App {
    fn new(engine: Engine) -> Self {
        let recovered = engine.recover_pending_native_learning().unwrap_or_default();
        let config = UserConfig::load(engine.home());
        let resumable = engine.latest_resumable_learning().ok().flatten();
        let runtime = resumable.as_ref().map(|run| run.runtime.clone()).or_else(|| config.default_runtime.clone()).unwrap_or_else(|| "claude-code".into());
        let permission_mode = resumable.as_ref().map(|run| run.permission_mode.clone()).unwrap_or_else(|| config.permission_mode().to_string());
        let runtime_selected = runtime_index(&runtime);
        let has_resumable = resumable.is_some();
        let mut transcript = Vec::new();
        if let Some(run) = &resumable {
            transcript.push(format!("Methodus: Resumed Learn run {} · {} · reply to continue", run.run_id, run.goal));
            if let Ok(events) = engine.learning_events(&run.run_id) {
                for event in events {
                    let line = match event.role.as_str() {
                        "user" => format!("You: {}", event.text),
                        "assistant" => format!("Methodus: {}", event.text),
                        _ => format!("Methodus: [{}] {}", event.role, event.text),
                    };
                    if !line.trim().is_empty() { transcript.push(line); }
                }
            }
        }
        if !recovered.is_empty() {
            let candidates = recovered.iter().map(|(_, ids)| ids.len()).sum::<usize>();
            transcript.push(format!("Methodus: Recovered {} candidate(s) from a completed native Learn return. Open /review to inspect them.", candidates));
        }
        Self {
            engine, view: View::Chat, input: String::new(),
            transcript,
            show_welcome: !has_resumable,
            nodes: Vec::new(), selected: 0, status: "Learn ready".into(),
            runtime, runtime_selected, permission_mode, quit: false,
            input_cursor: 0, slash_sel: 0, transcript_offset: 0, transcript_version: 1,
            knowledge_content: String::new(), knowledge_scroll: 0, detail_return: View::Chat,
            detail_node_id: String::new(), knowledge_filter: String::new(), method_filter: String::new(),
            experience_filter: String::new(), review_filter: String::new(), merge_filter: String::new(), filtering: false,
            merge_candidate_id: None, mention_cache: Vec::new(), mention_dynamic: Vec::new(), mention_sel: 0,
            graph_nodes: Vec::new(), graph_edges: Vec::new(), graph_selected: 0, graph_return: View::Knowledge,
            pending_quit_at: None, learning_session_id: resumable.as_ref().map(|run| run.run_id.clone()), learning_executor_sid: resumable.as_ref().and_then(|run| run.executor_sid.clone()),
            learning_goal: resumable.map(|run| run.goal),
            team: None,
            confirmation: None,
        }
    }
    fn refresh(&mut self) {
        match self.engine.sync_graph().and_then(|_| self.engine.list_graph_nodes(None)) { Ok(nodes) => self.nodes = nodes, Err(error) => self.status = format!("Graph sync failed: {error}") }
        self.selected = self.selected.min(self.items_len().saturating_sub(1));
        if self.view == View::Team { self.team = self.engine.team_status().ok(); }
    }
    fn items_len(&self) -> usize { match self.view { View::Knowledge => filtered_nodes(self.engine.home(), &self.nodes, "knowledge", None, &self.knowledge_filter).len(), View::Method => filtered_nodes(self.engine.home(), &self.nodes, "method", None, &self.method_filter).len(), View::Experience => filtered_nodes(self.engine.home(), &self.nodes, "experience", None, &self.experience_filter).len(), View::Review => filtered_nodes(self.engine.home(), &self.nodes, "", Some("candidate"), &self.review_filter).len(), View::MergeTarget => filtered_nodes(self.engine.home(), &self.nodes, "knowledge", Some("committed"), &self.merge_filter).len(), _ => 0 } }
    fn panel(&mut self, view: View) { self.view = view; self.selected = 0; self.confirmation = None; self.filtering = false; self.refresh(); }
    fn say(&mut self, text: impl Into<String>) { self.show_welcome = false; self.transcript.push(text.into()); if self.transcript.len() > 200 { self.transcript.remove(0); } self.transcript_offset = 0; self.transcript_version = self.transcript_version.wrapping_add(1); }
    fn clear_input(&mut self) { self.input.clear(); self.input_cursor = 0; self.slash_sel = 0; self.mention_sel = 0; self.mention_dynamic.clear(); }
    fn clamp_cursor(&mut self) { self.input_cursor = floor_char_boundary(&self.input, self.input_cursor); }
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
    fn matching_mentions(&self) -> Vec<&MentionCandidate> { let query = at_query(&self.input).unwrap_or_default().to_lowercase(); let mut all = self.mention_cache.iter().collect::<Vec<_>>(); all.extend(self.mention_dynamic.iter()); let mut result = all.into_iter().filter(|candidate| candidate.label.to_lowercase().contains(&query)).collect::<Vec<_>>(); result.sort_by_key(|candidate| candidate.label.len()); result }
    fn move_mention(&mut self, delta: isize) { let n = self.matching_mentions().len(); if n == 0 { self.mention_sel = 0; } else if delta < 0 { self.mention_sel = self.mention_sel.saturating_sub(delta.unsigned_abs()); } else { self.mention_sel = (self.mention_sel + delta as usize).min(n - 1); } }
    fn accept_mention(&mut self) { self.ensure_mention_cache(); let Some(candidate) = self.matching_mentions().get(self.mention_sel).cloned() else { return; }; let Some(start) = mention_start(&self.input) else { return; }; let replacement = format!("@{}", candidate.label); self.input.replace_range(start..self.input_cursor, &replacement); self.input_cursor = start + replacement.len(); self.mention_sel = 0; }
    fn handle_ctrl_c(&mut self) { if !self.input.is_empty() { self.clear_input(); self.pending_quit_at = None; self.status = "Input cleared".into(); return; } let now = Instant::now(); if ctrl_c_should_quit(self.pending_quit_at, now) { self.quit = true; } else { self.pending_quit_at = Some(now); self.view = View::Chat; self.status = "Press Ctrl+C again to quit".into(); } }
    fn scroll_transcript(&mut self, delta: isize) { if delta > 0 { self.transcript_offset = self.transcript_offset.saturating_add(delta as usize); } else { self.transcript_offset = self.transcript_offset.saturating_sub(delta.unsigned_abs()); } }
    fn retain_confirmation_for(&mut self, code: KeyCode) {
        let keep = matches!((self.confirmation, code), (Some(ConfirmAction::Commit), KeyCode::Char('c')) | (Some(ConfirmAction::Reject), KeyCode::Char('r')) | (Some(ConfirmAction::MarkTeam), KeyCode::Char('t')) | (Some(ConfirmAction::Delete), KeyCode::Char('d')) | (Some(ConfirmAction::Revalidate), KeyCode::Char('v')) | (Some(ConfirmAction::Merge), KeyCode::Enter));
        if !keep { self.confirmation = None; }
    }
}

pub fn run_loop(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, engine: Engine) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = App::new(engine); app.refresh();
    loop { terminal.draw(|frame| draw(frame, &app))?; if app.quit { break; } if event::poll(Duration::from_millis(250))? { match event::read()? { Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => handle_key(terminal, &mut app, key)?, Event::Paste(text) if app.view == View::Chat => app.insert_paste(&text), _ => {} } } }
    Ok(())
}

fn handle_key(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App, key: KeyEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) { if key.kind == KeyEventKind::Press { app.handle_ctrl_c(); } return Ok(()); }
    if app.view == View::Chat && (matches!(key.code, KeyCode::BackTab) || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))) { cycle_permission(app); return Ok(()); }
    app.pending_quit_at = None;
    app.retain_confirmation_for(key.code);
    match app.view {
        View::Chat => match key.code {
            KeyCode::Esc if mention_open(&app.input) => app.mention_sel = 0, KeyCode::Esc if slash_menu_open(&app.input) => app.clear_input(), KeyCode::Tab if mention_open(&app.input) => app.accept_mention(), KeyCode::Tab if slash_menu_open(&app.input) => app.complete_slash(),
            KeyCode::Up if mention_open(&app.input) => app.move_mention(-1), KeyCode::Down if mention_open(&app.input) => app.move_mention(1), KeyCode::Enter if mention_open(&app.input) => app.accept_mention(), KeyCode::Up if slash_menu_open(&app.input) => app.move_slash(-1), KeyCode::Down if slash_menu_open(&app.input) => app.move_slash(1),
            KeyCode::PageUp => app.scroll_transcript(8), KeyCode::PageDown => app.scroll_transcript(-8), KeyCode::Up => app.scroll_transcript(1), KeyCode::Down => app.scroll_transcript(-1), KeyCode::Enter | KeyCode::Char('\n') if wants_newline(key) => app.insert_str("\n"), KeyCode::Enter if !app.input.trim().is_empty() => submit_chat(terminal, app)?,
            KeyCode::Backspace => app.backspace(), KeyCode::Delete => app.delete(), KeyCode::Left => app.move_cursor(-1), KeyCode::Right => app.move_cursor(1), KeyCode::Home => app.input_cursor = 0, KeyCode::End => app.input_cursor = app.input.len(), KeyCode::Char('a') if ctrl => app.input_cursor = 0, KeyCode::Char('e') if ctrl => app.input_cursor = app.input.len(), KeyCode::Char(ch) if !ctrl => app.insert_str(&ch.to_string()), _ => {}
        },
        View::Knowledge | View::Method | View::Experience | View::Review => handle_list_key(app, key, ctrl),
        View::Runtime => match key.code { KeyCode::Esc => app.view = View::Chat, KeyCode::Up => app.runtime_selected = app.runtime_selected.saturating_sub(1), KeyCode::Down => app.runtime_selected = (app.runtime_selected + 1).min(RUNTIMES.len().saturating_sub(1)), KeyCode::Enter => apply_selected_runtime(app), _ => {} },
        View::MergeTarget => match key.code { KeyCode::Esc => { app.merge_filter.clear(); app.merge_candidate_id = None; app.selected = 0; app.view = View::ReviewDetail; app.status = "Merge cancelled".into(); }, KeyCode::Enter => merge_into_selected_target(app), KeyCode::Up => app.selected = app.selected.saturating_sub(1), KeyCode::Down => app.selected = (app.selected + 1).min(app.items_len().saturating_sub(1)), KeyCode::Backspace => { app.merge_filter.pop(); app.selected = 0; }, KeyCode::Char(ch) if !ctrl => { app.merge_filter.push(ch); app.selected = 0; }, _ => {} },
        View::KnowledgeGraph => match key.code { KeyCode::Esc | KeyCode::Char('q') => app.view = app.graph_return, KeyCode::Up => app.graph_selected = app.graph_selected.saturating_sub(1), KeyCode::Down => app.graph_selected = (app.graph_selected + 1).min(app.graph_nodes.len().saturating_sub(1)), KeyCode::Enter => open_graph_node_detail(app), _ => {} },
        View::KnowledgeDetail | View::ExperienceDetail | View::ReviewDetail => match key.code { KeyCode::Esc | KeyCode::Char('q') => { app.view = app.detail_return; app.knowledge_scroll = 0; }, KeyCode::Char('g') if app.view != View::ReviewDetail => open_graph(app, app.detail_return), KeyCode::Char('e') => edit_current_node(terminal, app)?, KeyCode::Up => app.knowledge_scroll = app.knowledge_scroll.saturating_sub(1), KeyCode::Down => app.knowledge_scroll = app.knowledge_scroll.saturating_add(1), KeyCode::PageUp => app.knowledge_scroll = app.knowledge_scroll.saturating_sub(10), KeyCode::PageDown => app.knowledge_scroll = app.knowledge_scroll.saturating_add(10), KeyCode::Home => app.knowledge_scroll = 0, KeyCode::Char('c') if app.view == View::ReviewDetail => commit_candidate(app), KeyCode::Char('r') if app.view == View::ReviewDetail => reject_candidate(app), KeyCode::Char('t') if app.view == View::ReviewDetail => mark_candidate_team(app), KeyCode::Char('m') if app.view == View::ReviewDetail => open_merge_target(app), KeyCode::Char('d') if app.view != View::ReviewDetail => delete_node(app), KeyCode::Char('v') => revalidate_node(app), _ => {} },
        View::Help | View::Health => if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) { app.view = View::Chat },
        View::Team => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => app.view = View::Chat,
            KeyCode::Char('v') => { app.team = app.engine.team_status().ok(); app.status = "Team graph validation refreshed".into(); },
            KeyCode::Char('d') => { app.team = app.engine.team_status().ok(); let changes = app.team.as_ref().map(|team| team.diff.lines().count()).unwrap_or(0); app.status = format!("Team diff: {changes} lines · press p to write a publish plan"); },
            KeyCode::Char('t') => { let mut config = UserConfig::load(app.engine.home()); match config.cycle_team(app.engine.home()) { Ok(team) => { app.team = app.engine.team_status().ok(); app.status = format!("Current Team: {team}"); }, Err(error) => app.status = format!("Team switch failed: {error}") } },
            KeyCode::Char('p') => match app.engine.create_team_publish_plan() { Ok(path) => app.status = format!("Publish plan written: {}", path.display()), Err(error) => app.status = format!("Failed to write publish plan: {error}") },
            _ => {}
        },
    }
    Ok(())
}

fn handle_list_key(app: &mut App, key: KeyEvent, ctrl: bool) {
    let view = app.view;
    let filter = match view { View::Knowledge => &mut app.knowledge_filter, View::Method => &mut app.method_filter, View::Experience => &mut app.experience_filter, View::Review => &mut app.review_filter, _ => return };
    if app.filtering {
        match key.code {
            KeyCode::Esc => { app.filtering = false; app.status = "Filter mode closed".into(); }
            KeyCode::Enter => { app.filtering = false; app.status = "Filter applied".into(); }
            KeyCode::Backspace => { filter.pop(); app.selected = 0; }
            KeyCode::Char(ch) if !ctrl => { filter.push(ch); app.selected = 0; }
            _ => {}
        }
        return;
    }
    match key.code {
        KeyCode::Esc => app.view = View::Chat,
        KeyCode::Char('f') if !ctrl => { app.filtering = true; app.status = "Filter mode · type to filter · Enter apply · Esc close".into(); },
        KeyCode::Enter => if view == View::Knowledge || view == View::Method { open_knowledge_detail(app) } else { open_node_detail(app, if view == View::Review { View::ReviewDetail } else { View::ExperienceDetail }) },
        KeyCode::Char('c') if view == View::Review => commit_candidate(app),
        KeyCode::Char('t') if view == View::Review => mark_candidate_team(app),
        KeyCode::Char('m') if view == View::Review => open_merge_target(app),
        KeyCode::Char('r') if view == View::Review => reject_candidate(app),
        KeyCode::Char('g') if view == View::Knowledge || view == View::Method => open_graph(app, view), KeyCode::Up => app.selected = app.selected.saturating_sub(1), KeyCode::Down => app.selected = (app.selected + 1).min(app.items_len().saturating_sub(1)), KeyCode::Backspace => { filter.pop(); app.selected = 0; }, KeyCode::Char(ch) if !ctrl => { filter.push(ch); app.selected = 0; }, _ => {}
    }
}

fn submit_chat(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if app.input.trim_start().starts_with('/') { return command(terminal, app); }
    let input = std::mem::take(&mut app.input); app.input_cursor = 0; let text = input.trim().to_string(); if text.is_empty() { return Ok(()); }
    let runtime = app.runtime.clone();
    let permission_mode = app.permission_mode.clone();
    let handoff = match app.learning_session_id.as_deref() {
        Some(run_id) => app.engine.continue_native_learning(&runtime, &permission_mode, run_id, app.learning_executor_sid.as_deref(), &text),
        None => app.engine.prepare_native_learning(Some(&runtime), &permission_mode, &text),
    };
    let handoff = match handoff {
        Ok(handoff) => handoff,
        Err(error) => { app.say(format!("Methodus: Could not prepare Learn handoff: {error}")); app.status = "Learn handoff unavailable".into(); return Ok(()); }
    };
    if app.learning_goal.is_none() { app.learning_goal = Some(handoff.goal.clone()); }
    app.learning_session_id = Some(handoff.run_id.clone());
    app.learning_executor_sid = handoff.executor_sid.clone();
    app.say(format!("You: {text}"));
    app.say(format!("Methodus: Prepared {} Learn session. Handing the terminal to its native TUI…", runtime_label(&handoff.runtime)));
    app.status = format!("Handoff → {}", runtime_label(&handoff.runtime));
    terminal.draw(|frame| draw(frame, app))?;
    match run_native_learn_handoff(terminal, &handoff) {
        Ok(exit_status) => match app.engine.complete_native_learning(&handoff, &exit_status) {
            Ok(result) => {
                app.learning_session_id = Some(handoff.run_id.clone());
                app.learning_executor_sid = handoff.executor_sid.clone();
                if !result.candidate_ids.is_empty() {
                    app.refresh();
                    app.say(format!("Methodus: {} returned; imported {} candidate(s). Open /review to inspect them.", runtime_label(&handoff.runtime), result.candidate_ids.len()));
                    app.status = format!("Learn · {} candidate(s) ready for review", result.candidate_ids.len());
                } else if result.output_recorded {
                    app.say(format!("Methodus: {} returned; the synthesis was recorded without a candidate set.", runtime_label(&handoff.runtime)));
                    app.status = "Learn · ready for another native session".into();
                } else {
                    app.say(format!("Methodus: {} returned without a final synthesis. The Learn run remains resumable.", runtime_label(&handoff.runtime)));
                    app.status = "Learn · native session returned".into();
                }
            }
            Err(error) => { app.say(format!("Methodus: Native session returned, but its Learn record could not be finalized: {error}")); app.status = "Learn return needs attention".into(); }
        },
        Err(error) => {
            let _ = app.engine.mark_learning_status(&handoff.run_id, "failed");
            app.say(format!("Methodus: Native Learn handoff failed: {error}"));
            app.status = "Native Learn handoff failed".into();
        }
    }
    Ok(())
}

fn run_native_learn_handoff(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, handoff: &methodus_core::NativeLearnHandoff) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    disable_raw_mode()?;
    let _ = stdout().execute(PopKeyboardEnhancementFlags);
    let _ = stdout().execute(DisableBracketedPaste);
    stdout().execute(LeaveAlternateScreen)?;
    let launch = Command::new(&handoff.program).args(&handoff.args).current_dir(&handoff.cwd).status();
    let restore = (|| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        enable_raw_mode()?;
        stdout().execute(EnterAlternateScreen)?;
        stdout().execute(EnableBracketedPaste)?;
        let _ = stdout().execute(PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS));
        terminal.clear()?;
        Ok(())
    })();
    restore?;
    let status = launch?;
    Ok(status.code().map(|code| format!("exit {code}")).unwrap_or_else(|| "terminated by signal".into()))
}

fn command(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App) -> Result<(), Box<dyn std::error::Error + Send + Sync>> { let matches = matching_slash(&app.input); let Some(command) = matches.get(app.slash_sel).copied() else { app.status = "Unknown command; type / to browse available commands".into(); return Ok(()); }; let rest = slash_rest(&app.input); app.clear_input(); match command.name { "knowledge" => app.panel(View::Knowledge), "method" => app.panel(View::Method), "experience" => app.panel(View::Experience), "review" => app.panel(View::Review), "team" => app.panel(View::Team), "health" => app.panel(View::Health), "runtime" => { if rest.is_empty() { open_runtime_picker(app); } else { select_runtime(app, &rest); } }, "open" => open_path(terminal, app, &rest)?, "help" => app.panel(View::Help), "new" => { close_current_learning(app); if !rest.is_empty() { app.input = rest; app.input_cursor = app.input.len(); return submit_chat(terminal, app); } }, "quit" => app.quit = true, _ => app.status = format!("Unknown command /{}", command.name) } Ok(()) }

fn open_path(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App, requested: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = if requested.trim().is_empty() {
        app.nodes.iter().find(|node| node.id == app.detail_node_id).and_then(|node| { let relative = std::path::Path::new(&node.path); (!relative.is_absolute() && !relative.components().any(|component| component == std::path::Component::ParentDir)).then(|| app.engine.home().join(relative)) }).unwrap_or_else(|| app.engine.home().to_path_buf())
    } else {
        let requested = requested.trim().trim_matches('"').trim_matches('\'');
        if requested == "~" || requested.starts_with("~/") {
            std::env::var_os("HOME").map(std::path::PathBuf::from).map(|home| home.join(requested.trim_start_matches("~/"))).unwrap_or_else(|| app.engine.home().to_path_buf())
        } else { let path = std::path::PathBuf::from(requested); if path.is_absolute() { path } else { app.engine.launch_cwd().join(path) } }
    };
    if !path.exists() { app.status = format!("Path does not exist: {}", path.display()); return Ok(()); }
    let (program, args): (&str, Vec<std::path::PathBuf>) = if cfg!(target_os = "macos") { ("open", vec![path.clone()]) } else if cfg!(target_os = "windows") { ("cmd", vec![path.clone()]) } else { ("xdg-open", vec![path.clone()]) };
    disable_raw_mode()?; stdout().execute(LeaveAlternateScreen)?;
    let result = if cfg!(target_os = "windows") { Command::new(program).args(["/C", "start", ""]).arg(&path).status() } else { Command::new(program).args(args).status() };
    stdout().execute(EnterAlternateScreen)?; enable_raw_mode()?; terminal.clear()?;
    app.status = match result { Ok(status) if status.success() => format!("Opened: {}", path.display()), Ok(status) => format!("Open failed, exit code: {}", status.code().map(|code| code.to_string()).unwrap_or_else(|| "signal".into())), Err(error) => format!("Could not open path: {error}") };
    Ok(())
}
fn edit_current_node(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(node) = app.nodes.iter().find(|node| node.id == app.detail_node_id).cloned() else {
        app.status = "Current node not found".into();
        return Ok(());
    };
    let relative = std::path::Path::new(&node.path);
    if relative.is_absolute() || relative.components().any(|component| component == std::path::Component::ParentDir) {
        app.status = "Unsafe node path; editor launch refused".into();
        return Ok(());
    }
    let path = app.engine.home().join(relative);
    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "vi".into());
    let mut command = editor.split_whitespace();
    let Some(program) = command.next() else { app.status = "EDITOR is empty".into(); return Ok(()); };
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    let result = Command::new(program).args(command).arg(&path).status();
    stdout().execute(EnterAlternateScreen)?;
    enable_raw_mode()?;
    terminal.clear()?;
    match result {
        Ok(status) if status.success() => { app.refresh(); app.status = format!("Reloaded {}", node.title); }
        Ok(status) => app.status = format!("Editor exit code: {}", status.code().map(|code| code.to_string()).unwrap_or_else(|| "signal".into())),
        Err(error) => app.status = format!("Could not start editor: {error}"),
    }
    Ok(())
}
fn delete_node(app: &mut App) {
    let Some(node) = app.nodes.iter().find(|node| node.id == app.detail_node_id).cloned() else { app.status = "Current node not found".into(); return; };
    if !confirm_action(app, ConfirmAction::Delete, &format!("Confirm deletion of '{}' from the graph", node.title)) { return; }
    match app.engine.delete_graph_node(&node.id, "deleted from Methodus TUI") {
        Ok(()) => { app.status = format!("Deleted {}", node.title); app.view = app.detail_return; app.refresh(); }
        Err(error) => app.status = format!("Delete failed: {error}"),
    }
}
fn revalidate_node(app: &mut App) {
    let Some(node) = app.nodes.iter().find(|node| node.id == app.detail_node_id).cloned() else { app.status = "Current node not found".into(); return; };
    if !confirm_action(app, ConfirmAction::Revalidate, &format!("Confirm revalidate '{}' and restore it to committed", node.title)) { return; }
    match app.engine.revalidate_graph_node(&node.id, "revalidated from Methodus TUI") {
        Ok(()) => { app.status = format!("Revalidated {}", node.title); app.refresh(); }
        Err(error) => app.status = format!("Revalidation failed: {error}"),
    }
}
fn confirm_action(app: &mut App, action: ConfirmAction, message: &str) -> bool {
    if app.confirmation == Some(action) {
        app.confirmation = None;
        true
    } else {
        app.confirmation = Some(action);
        app.status = format!("{message}; press the same key again to confirm, Esc to cancel");
        false
    }
}
fn commit_candidate(app: &mut App) {
    let Some(node) = filtered_nodes(app.engine.home(), &app.nodes, "", Some("candidate"), &app.review_filter).get(app.selected).cloned() else { return; };
    if !confirm_action(app, ConfirmAction::Commit, &format!("Confirm promoting '{}' to Personal canonical", node.title)) { return; }
    match app.engine.promote_graph_candidate(&node.id) { Ok(()) => { app.status = format!("Promoted {}", node.title); app.refresh(); }, Err(error) => app.status = format!("Promotion failed: {error}") }
}
fn reject_candidate(app: &mut App) {
    let Some(node) = filtered_nodes(app.engine.home(), &app.nodes, "", Some("candidate"), &app.review_filter).get(app.selected).cloned() else { return; };
    if !confirm_action(app, ConfirmAction::Reject, &format!("Confirm rejecting candidate '{}'", node.title)) { return; }
    match app.engine.reject_graph_candidate(&node.id) { Ok(()) => { app.status = format!("Rejected {}", node.title); app.view = View::Review; app.refresh(); }, Err(error) => app.status = format!("Rejection failed: {error}") }
}
fn mark_candidate_team(app: &mut App) {
    let Some(node) = filtered_nodes(app.engine.home(), &app.nodes, "", Some("candidate"), &app.review_filter).get(app.selected).cloned() else { return; };
    if !confirm_action(app, ConfirmAction::MarkTeam, &format!("Confirm marking '{}' as Team-visible", node.title)) { return; }
    match app.engine.promote_candidate_to_team(&node.id) { Ok(()) => { app.status = format!("{} is now marked Team-visible", node.title); app.refresh(); }, Err(error) => app.status = format!("Failed to set Team visibility: {error}") }
}
fn open_merge_target(app: &mut App) {
    let candidate = app.nodes.iter().find(|node| node.id == app.detail_node_id).cloned()
        .or_else(|| filtered_nodes(app.engine.home(), &app.nodes, "", Some("candidate"), &app.review_filter).get(app.selected).cloned());
    let Some(candidate) = candidate else { app.status = "No candidate available for merging".into(); return; };
    if candidate.node_type != "knowledge" || candidate.status.as_deref() != Some("candidate") { app.status = "Only knowledge candidates can be merged".into(); return; }
    app.merge_candidate_id = Some(candidate.id.clone()); app.merge_filter.clear(); app.selected = 0; app.filtering = false; app.view = View::MergeTarget; app.status = format!("Select the committed knowledge target for '{}'; press Enter twice to confirm", candidate.title);
}
fn merge_into_selected_target(app: &mut App) { let Some(candidate_id) = app.merge_candidate_id.clone() else { app.status = "No candidate is waiting to be merged".into(); app.view = View::Review; return; }; let targets = filtered_nodes(app.engine.home(), &app.nodes, "knowledge", Some("committed"), &app.merge_filter); let Some(target) = targets.get(app.selected) else { app.status = "Select a committed knowledge target".into(); return; }; if !confirm_action(app, ConfirmAction::Merge, &format!("Confirm merging the candidate into '{}'", target.title)) { return; } match app.engine.merge_graph_candidate(&candidate_id, &target.id) { Ok(()) => { app.status = format!("Merged into {}", target.title); app.merge_candidate_id = None; app.merge_filter.clear(); app.selected = 0; app.view = View::Review; app.refresh(); }, Err(error) => app.status = format!("Merge failed: {error}") } }

fn draw(frame: &mut Frame, app: &App) { let theme = Theme::current(); let area = frame.area(); frame.render_widget(Block::default().style(Style::default().bg(theme.surface)), area); if area.width < 80 || area.height < 24 { frame.render_widget(Paragraph::new(format!("{MARK}  {WORDMARK}\n\nterminal too small — need 80 × 24")).style(theme.dim()).alignment(Alignment::Center), area); return; } let rows = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(1), Constraint::Min(5), Constraint::Length(1)]).split(area); draw_header(frame, rows[0], app, &theme); match app.view { View::Chat => draw_chat(frame, rows[1], app, &theme), View::KnowledgeDetail | View::ExperienceDetail | View::ReviewDetail => draw_detail(frame, rows[1], app, &theme), View::KnowledgeGraph => draw_graph(frame, rows[1], app, &theme), _ => frame.render_widget(Block::default().style(Style::default().bg(theme.overlay)), rows[1]) } if matches!(app.view, View::Help | View::Knowledge | View::Method | View::Experience | View::Review | View::Team | View::Health | View::Runtime | View::MergeTarget) { draw_overlay(frame, rows[1], app, &theme); } draw_footer(frame, rows[2], app, &theme); }
fn draw_header(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) { let active = match app.view { View::Chat => "learn", View::Knowledge | View::KnowledgeDetail | View::KnowledgeGraph => "knowledge", View::Method => "method", View::Experience | View::ExperienceDetail => "experience", View::Review | View::ReviewDetail | View::MergeTarget => "review", View::Team => "team", View::Health => "health", View::Runtime => "runtime", View::Help => "help" }; let candidates = app.nodes.iter().filter(|node| node.status.as_deref() == Some("candidate")).count(); let line = Line::from(vec![Span::styled(format!(" {MARK} "), theme.accent()), Span::styled(WORDMARK, theme.accent()), Span::styled(format!("  {} · {active}", runtime_label(&app.runtime)), theme.dim()), Span::styled(format!("  graph:{}", app.nodes.len()), theme.dim()), Span::styled(if candidates == 0 { String::new() } else { format!("  ▣{candidates}") }, Style::default().fg(theme.warning))]); frame.render_widget(Paragraph::new(line).style(theme.text()), area); }
fn draw_footer(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) { let style = if app.status.contains("failed") || app.status.contains("Failed") || app.status.contains("Could not") { Style::default().fg(theme.error).add_modifier(Modifier::BOLD) } else if app.status.starts_with("Deleted") || app.status.starts_with("Revalidated") || app.status.starts_with("Promoted") || app.status.starts_with("Rejected") || app.status.starts_with("Merged") || app.status.starts_with("Opened") || app.status.starts_with("Reloaded") || app.status.starts_with("Publish plan written") { Style::default().fg(theme.success) } else { Style::default().fg(theme.info) }; frame.render_widget(Paragraph::new(app.status.as_str()).style(style), area); }
fn draw_chat(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) { let slash_open = slash_menu_open(&app.input); let mention_open = !slash_open && mention_open(&app.input); let slash_height = if slash_open { (matching_slash(&app.input).len().max(1) as u16 + 2).min(8) } else { 0 }; let mention_height = if mention_open { (app.matching_mentions().len().max(1) as u16 + 2).min(8) } else { 0 }; let composer_height = composer_height(&app.input, area.width); let mut constraints = vec![Constraint::Min(3)]; if slash_open { constraints.push(Constraint::Length(slash_height)); } if mention_open { constraints.push(Constraint::Length(mention_height)); } constraints.push(Constraint::Length(composer_height)); let rows = Layout::default().direction(Direction::Vertical).constraints(constraints).split(area); if app.show_welcome { draw_welcome(frame, rows[0], theme); } else { draw_transcript(frame, rows[0], app, theme); } let mut index = 1; if slash_open { draw_slash_menu(frame, rows[index], app, theme); index += 1; } if mention_open { draw_mention_menu(frame, rows[index], app, theme); index += 1; } draw_composer(frame, rows[index], app, theme); }

fn draw_welcome(frame: &mut Frame, area: Rect, theme: &Theme) {
    const LOGO: [&str; 5] = [
        "█   █ █████ █████ █   █  ███  ████  █   █  ████",
        "██ ██ █       █   █   █ █   █ █   █ █   █ █",
        "█ █ █ ████    █   █████ █   █ █   █ █   █  ███",
        "█   █ █       █   █   █ █   █ █   █ █   █     █",
        "█   █ █████   █   █   █  ███  ████   ███  ████",
    ];
    let mut lines = LOGO.iter().map(|line| Line::from(Span::styled(*line, theme.accent()))).collect::<Vec<_>>();
    lines.extend([
        Line::default(),
        Line::from(Span::styled("Maintainer learning studio", theme.overlay_text().add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("Capture, verify, and connect knowledge.", theme.dim())),
        Line::default(),
        Line::from(Span::styled("Type a learning goal to begin  ·  /help for commands", theme.info)),
    ]);
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center).style(theme.text()), area);
}
fn draw_mention_menu(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) { let matches = app.matching_mentions(); let items = if matches.is_empty() { vec![ListItem::new("  no matching path").style(theme.dim())] } else { matches.iter().map(|candidate| ListItem::new(format!("  {}{}", if candidate.is_dir { "▸ " } else { "· " }, candidate.label)).style(theme.text())).collect() }; let mut state = ListState::default(); if !matches.is_empty() { state.select(Some(app.mention_sel)); } frame.render_stateful_widget(List::new(items).block(Block::default().borders(Borders::TOP).border_style(theme.border(true)).title(" @  ↑↓ select · Tab complete · Enter attach ")).highlight_style(theme.selected()).highlight_symbol("› "), area, &mut state); }
fn draw_transcript(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) { let width = area.width.max(8) as usize; let cached = TRANSCRIPT_CACHE.with(|cache| { let mut cache = cache.borrow_mut(); if cache.version != app.transcript_version || cache.width != width { cache.rows = layout_transcript(&app.transcript, width); cache.version = app.transcript_version; cache.width = width; } cache.rows.clone() }); let height = area.height.max(1) as usize; let max_offset = cached.len().saturating_sub(height); let offset = app.transcript_offset.min(max_offset); let end = cached.len().saturating_sub(offset); let start = end.saturating_sub(height); let lines = if cached.is_empty() { vec![Line::default(), Line::from(Span::styled(format!("{MARK}  {WORDMARK}"), theme.accent()))] } else { cached[start..end].iter().map(|row| transcript_line(row, theme)).collect() }; frame.render_widget(Paragraph::new(lines).style(theme.text()), area); }
fn draw_composer(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) { let inner_width = area.width.saturating_sub(4).max(8) as usize; let (shown, cursor_col, cursor_row) = composer_view(&app.input, app.input_cursor, inner_width, COMPOSER_MAX_ROWS); let title = format!(" learn · permission: {} · ⇧Tab cycle · Enter send · ⇧Enter newline ", permission_label(&app.permission_mode)); frame.render_widget(Paragraph::new(shown.into_iter().map(Line::from).collect::<Vec<_>>()).style(theme.text()).block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(theme.border(true)).title(title)), area); let x = area.x.saturating_add(1).saturating_add(cursor_col); let y = area.y.saturating_add(1).saturating_add(cursor_row); if x < area.right().saturating_sub(1) && y < area.bottom().saturating_sub(1) { frame.set_cursor_position(Position { x, y }); } }
fn draw_slash_menu(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) { let matches = matching_slash(&app.input); let items = if matches.is_empty() { vec![ListItem::new("  no matching command").style(theme.dim())] } else { matches.iter().map(|command| ListItem::new(format!("  /{}  {}", command.name, command.summary)).style(theme.text())).collect() }; let mut state = ListState::default(); if !matches.is_empty() { state.select(Some(app.slash_sel)); } frame.render_stateful_widget(List::new(items).block(Block::default().borders(Borders::TOP).border_style(theme.border(true)).title(" /  ↑↓ select · Tab complete · Enter run ")).highlight_style(theme.selected()).highlight_symbol("› "), area, &mut state); }

fn draw_overlay(frame: &mut Frame, base: Rect, app: &App, theme: &Theme) { let popup = centered(base, if app.view == View::Runtime { 68 } else { 88 }, if app.view == View::Runtime { 46 } else { 82 }); frame.render_widget(Clear, popup); let title = match app.view { View::Help => " help · commands & controls ".into(), View::Knowledge => format!(" knowledge · filter: {} ", display_filter(&app.knowledge_filter)), View::Method => format!(" method · filter: {} ", display_filter(&app.method_filter)), View::Experience => format!(" experience · filter: {} ", display_filter(&app.experience_filter)), View::Review => format!(" review · filter: {} ", display_filter(&app.review_filter)), View::MergeTarget => format!(" merge target · filter: {} ", display_filter(&app.merge_filter)), View::Team => " team · local Markdown roots ".into(), View::Health => " health · runtime and index checks ".into(), View::Runtime => " runtime · ↑↓ select · Enter apply · Esc cancel ".into(), _ => return }; let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(theme.border(true)).style(Style::default().bg(theme.overlay).fg(theme.overlay_fg)).title(Span::styled(title, theme.accent())); let inner = block.inner(popup); frame.render_widget(block, popup); match app.view { View::Help => draw_help(frame, inner, theme), View::Team => draw_team(frame, inner, app, theme), View::Health => draw_health(frame, inner, app, theme), View::Runtime => draw_runtime_picker(frame, inner, app, theme), _ => draw_nodes(frame, inner, app, theme) } }
fn draw_nodes(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let (content, actions_area) = if app.view == View::Review {
        let rows = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(5), Constraint::Length(3)]).split(area);
        (rows[0], Some(rows[1]))
    } else { (area, None) };
    let nodes = nodes_for_view(app);
    let cols = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(44), Constraint::Percentage(56)]).split(content);
    let items = nodes.iter().map(|n| ListItem::new(Line::from(vec![Span::styled(format!("{}  ", n.status.as_deref().unwrap_or("—")), theme.dim()), Span::styled(&n.title, theme.text())]))).collect::<Vec<_>>();
    let mut state = ListState::default();
    if !nodes.is_empty() { state.select(Some(app.selected)); }
    frame.render_stateful_widget(List::new(items).highlight_style(theme.selected()).highlight_symbol("› "), cols[0], &mut state);
    let filter = active_filter(app);
    let detail = nodes.get(app.selected).map(|node| format!("{}{}\n\n{}\n\nstatus      {}\nvisibility  {}\ntags        {}\nscope       {}\nconfidence  {}\npath        {}\n\n{}", if !filter.is_empty() { format!("filter      {}\n\n", filter) } else { String::new() }, node.title, node.summary.as_deref().unwrap_or("No summary available."), node.status.as_deref().unwrap_or("—"), node.visibility, if node.tags.is_empty() { "—".into() } else { node.tags.join(", ") }, node.scope.as_deref().unwrap_or("—"), node.confidence.map(|v| format!("{v:.2}")).unwrap_or_else(|| "—".into()), node.path, if app.view == View::Review { "Use the action bar below to decide this candidate." } else if app.view == View::MergeTarget { "Enter merges the candidate into this knowledge. Esc cancels." } else { "Enter reads the complete Markdown note · g opens graph" })).unwrap_or_else(|| if !filter.is_empty() { format!("No matches for ‘{}’.\n\nPress f to edit the filter, or Esc to return.", filter) } else { "Nothing here yet.".into() });
    frame.render_widget(Paragraph::new(detail).style(theme.overlay_text()).wrap(Wrap { trim: true }).block(Block::default().borders(Borders::LEFT).border_style(theme.border(false))), cols[1]);
    if app.view == View::Review {
        let confirmation = match app.confirmation {
            Some(ConfirmAction::Commit) => "Confirm approval: press c again",
            Some(ConfirmAction::MarkTeam) => "Confirm Team visibility: press t again",
            Some(ConfirmAction::Reject) => "Confirm rejection: press r again",
            _ => "Select a candidate, then choose an action",
        };
        let actions = format!(" {confirmation}\n [Enter] inspect   [c] approve   [t] Team   [m] merge   [r] reject   [f] filter · {}", if app.filtering { "FILTER MODE · type, Enter apply, Esc close" } else { "Esc back" });
        if let Some(actions_area) = actions_area {
            frame.render_widget(Paragraph::new(actions).style(theme.overlay_text()).block(Block::default().borders(Borders::TOP).border_style(theme.border(true))), actions_area);
        }
    }
}
fn draw_help(frame: &mut Frame, area: Rect, theme: &Theme) { let lines = vec![Line::from(Span::styled("Commands", theme.accent())), Line::from("Plain input   Start or continue the current Learn"), Line::from("/new          Close the current context and start a new Learn"), Line::from("/knowledge    Browse the knowledge graph and related nodes"), Line::from("/method       Browse reusable working methods"), Line::from("/experience   Browse validated task experience"), Line::from("/review       Review candidates and approve promotion"), Line::from("/team         Inspect the Personal / Team roots"), Line::from("/health       Check sources, indexes, and connector status"), Line::from("/runtime      Open the runtime picker; /runtime codex also works"), Line::from("/open         Open the current node or a path"), Line::from("/quit         Exit Methodus; q does not quit"), Line::default(), Line::from(Span::styled("Navigation", theme.accent())), Line::from("/             Open the command palette; ↑↓ select; Tab complete; Enter run"), Line::from("Lists         f filter; Enter read; g opens the selected graph neighborhood"), Line::from("Review        Enter inspect; c approve; t Team; m merge; r reject"), Line::from("Detail        e edit; d delete (actions confirm twice)"), Line::from("Composer      Enter send; Shift+Enter newline; Shift+Tab cycle permission"), Line::from("Exit          Ctrl+C clears input; press it again within three seconds to quit")]; frame.render_widget(Paragraph::new(lines).style(theme.overlay_text()).wrap(Wrap { trim: false }), area); }
fn draw_runtime_picker(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) { let mut lines = vec![Line::from(Span::styled("Choose the runtime for the next Learn", theme.overlay_text())), Line::default()]; for (index, runtime) in RUNTIMES.iter().enumerate() { let current = if runtime.id == app.runtime { "●" } else { "○" }; let marker = if index == app.runtime_selected { "›" } else { " " }; let style = if index == app.runtime_selected { theme.selected() } else { theme.overlay_text() }; lines.push(Line::from(Span::styled(format!("{marker} {current} {:<14}  {}", runtime.label, runtime.detail), style))); } if app.learning_session_id.is_some() { lines.push(Line::default()); lines.push(Line::from(Span::styled(format!("Current Learn is bound to {}; enter /new before switching", runtime_label(&app.runtime)), Style::default().fg(theme.warning).bg(theme.overlay)))); } frame.render_widget(Paragraph::new(lines).style(theme.overlay_text()).wrap(Wrap { trim: false }), area); }
fn draw_team(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let personal = app.engine.home().join("personal");
    let teams = app.engine.home().join("teams");
    let personal_count = app.nodes.iter().filter(|node| node.visibility == "personal").count();
    let team_count = app.nodes.iter().filter(|node| node.visibility == "team").count();
    let status = app.team.as_ref();
    let branch = status.and_then(|team| team.branch.as_deref()).unwrap_or("—");
    let git_state = match status {
        Some(team) if !team.is_git => "not a Git repository",
        Some(team) if team.dirty => "dirty",
        Some(_) => "clean",
        None => "not checked",
    };
    let changes = status.map(|team| team.changes.len()).unwrap_or(0);
    let issues = status.map(|team| team.validation_issues.len()).unwrap_or(0);
    let diff_lines = status.map(|team| team.diff.lines().count()).unwrap_or(0);
    let diff_preview = status
        .map(|team| team.diff.lines().take(12).collect::<Vec<_>>().join("\n"))
        .filter(|diff| !diff.is_empty())
        .unwrap_or_else(|| "(no Markdown diff)".into());
    let team_id = status.map(|team| team.team_id.clone()).unwrap_or_else(|| app.engine.team_id());
    let team_root = status.map(|team| team.root.display().to_string()).unwrap_or_else(|| teams.join(&team_id).display().to_string());
    let text = format!(
        "Personal\n  {}\n  indexed nodes: {}\n\nTeam · {}\n  {}\n  indexed nodes: {}\n  git: {} · branch: {}\n  changed files: {} · diff lines: {}\n  validation issues: {}\n\nDiff preview\n{}\n\nTeam is a normal Markdown/Git surface. Maintainers review candidates here; agents consume only committed or stale nodes.\n\n[t] switch Team   [v] validate   [d] inspect diff   [p] write publish plan\nEsc back",
        personal.display(), personal_count, team_id, team_root, team_count, git_state, branch, changes, diff_lines, issues, diff_preview
    );
    frame.render_widget(Paragraph::new(text).style(theme.overlay_text()).wrap(Wrap { trim: true }), area);
}
fn draw_health(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let mut lines = methodus_core::health_checks(app.engine.home()).into_iter().map(|check| Line::from(vec![Span::styled(if check.ok { "✓ " } else { "· " }, if check.ok { Style::default().fg(theme.success) } else { Style::default().fg(theme.warning) }), Span::styled(format!("{}  {}", check.label, check.detail), theme.overlay_text())])).collect::<Vec<_>>();
    let validation = methodus_core::validate_graph(app.engine.home()).unwrap_or_default();
    let errors = validation.iter().filter(|issue| issue.severity == methodus_core::IssueSeverity::Error).count();
    let warnings = validation.iter().filter(|issue| issue.severity == methodus_core::IssueSeverity::Warning).count();
    let revision = methodus_core::index_revision(app.engine.store()).unwrap_or_else(|_| "unavailable".into());
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(format!("indexed graph nodes: {} · revision: {}", app.nodes.len(), revision), theme.overlay_text())));
    lines.push(Line::from(Span::styled(format!("graph validation: {} errors · {} warnings", errors, warnings), if errors > 0 { Style::default().fg(theme.error) } else if warnings > 0 { Style::default().fg(theme.warning) } else { theme.overlay_text() })));
    for runtime in ["claude-code", "codex", "cursor"] {
        let target = match runtime { "claude-code" => std::env::var_os("HOME").map(std::path::PathBuf::from).map(|home| home.join(".claude/skills/methodus/SKILL.md")), "codex" => std::env::var_os("HOME").map(std::path::PathBuf::from).map(|home| home.join(".codex/skills/methodus/SKILL.md")), _ => std::env::var_os("HOME").map(std::path::PathBuf::from).map(|home| home.join(".cursor/skills/methodus/SKILL.md")) };
        let state = target.map(|path| if !path.exists() { "missing" } else if fs::read_to_string(path).ok().is_some_and(|body| body.contains("# Methodus connector") && body.contains("x-methodus-managed: true") && body.contains("version: 1")) { "current" } else { "drifted" }).unwrap_or("unknown");
        lines.push(Line::from(Span::styled(format!("connector/{runtime}: {state}"), theme.overlay_text())));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

fn draw_detail(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let Some(node) = app.nodes.iter().find(|node| node.id == app.detail_node_id) else { return; };
    let rows = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(5), Constraint::Length(3)]).split(area);
    let kind = match app.view { View::ExperienceDetail => "experience", View::ReviewDetail => "review", View::KnowledgeDetail if node.node_type == "method" => "method", _ => "knowledge" };
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(theme.border(true)).title(Span::styled(format!(" {kind} · {} ", node.title), theme.accent()));
    let inner = block.inner(rows[0]);
    frame.render_widget(block, rows[0]);
    frame.render_widget(Paragraph::new(app.knowledge_content.as_str()).style(theme.text()).wrap(Wrap { trim: false }).scroll((app.knowledge_scroll, 0)), inner);
    let confirmation = match app.confirmation {
        Some(ConfirmAction::Commit) => "Confirm approval: press c again",
        Some(ConfirmAction::MarkTeam) => "Confirm Team visibility: press t again",
        Some(ConfirmAction::Reject) => "Confirm rejection: press r again",
        Some(ConfirmAction::Delete) => "Confirm deletion: press d again",
        Some(ConfirmAction::Revalidate) => "Confirm revalidation: press v again",
        Some(ConfirmAction::Merge) => "Confirm merge: press Enter again",
        None => match app.view {
            View::ReviewDetail => "Candidate actions",
            _ => "Node actions",
        },
    };
    let actions = if app.view == View::ReviewDetail {
        "[c] approve   [t] Team   [m] merge   [r] reject   [e] edit   [Esc] back"
    } else if node.status.as_deref() == Some("rejected") {
        "Rejected candidate · [d] delete   [e] edit   [Esc] back"
    } else if node.status.as_deref() == Some("stale") {
        "[g] graph   [e] edit   [v] revalidate   [d] delete   [Esc] back"
    } else {
        "[g] graph   [e] edit   [d] delete   [Esc] back"
    };
    frame.render_widget(Paragraph::new(format!(" {confirmation}\n {actions}")).style(theme.overlay_text()).block(Block::default().borders(Borders::TOP).border_style(theme.border(true))), rows[1]);
}
fn open_knowledge_detail(app: &mut App) { let nodes = nodes_for_view(app); let Some(node) = nodes.get(app.selected).cloned() else { app.status = "No content is available to open".into(); return; }; read_detail(app, node, View::KnowledgeDetail); }
fn open_node_detail(app: &mut App, detail_view: View) { let nodes = nodes_for_view(app); let Some(node) = nodes.get(app.selected).cloned() else { app.status = "No content is available to open".into(); return; }; read_detail(app, node, detail_view); }
fn read_detail(app: &mut App, node: GraphNode, detail_view: View) { let relative = std::path::Path::new(&node.path); if relative.is_absolute() || relative.components().any(|component| component == std::path::Component::ParentDir) { app.status = "Unsafe node path".into(); return; } match fs::read_to_string(app.engine.home().join(relative)) { Ok(content) => { app.knowledge_content = content; app.knowledge_scroll = 0; app.detail_return = app.view; app.detail_node_id = node.id; app.view = detail_view; app.status = format!("Reading: {}", node.title); }, Err(error) => app.status = format!("Could not read content: {error}") } }
fn open_graph(app: &mut App, return_view: View) {
    let nodes = match return_view {
        View::Knowledge => browsable_nodes(app.engine.home(), &app.nodes, "knowledge", &app.knowledge_filter),
        View::Method => browsable_nodes(app.engine.home(), &app.nodes, "method", &app.method_filter),
        View::Experience => browsable_nodes(app.engine.home(), &app.nodes, "experience", &app.experience_filter),
        _ => Vec::new(),
    };
    let Some(focus) = nodes.get(app.selected).cloned().filter(|node| graph_visible(node)).or_else(|| app.nodes.iter().find(|node| node.id == app.detail_node_id && graph_visible(node)).cloned()) else { app.status = "Rejected or inactive nodes have no active graph neighborhood".into(); return; };
    let edges = match app.engine.graph_edges_for(&focus.id) { Ok(edges) => edges, Err(error) => { app.status = format!("Could not read graph edges: {error}"); return; } };
    let visible = app.nodes.iter().filter(|node| graph_visible(node)).cloned().collect::<Vec<_>>();
    let edges = edges.into_iter().filter(|edge| {
        let other = if edge.from_id == focus.id { &edge.to_id } else { &edge.from_id };
        visible.iter().any(|node| &node.id == other)
    }).collect::<Vec<_>>();
    let mut ids = vec![focus.id.clone()];
    for edge in &edges { let other = if edge.from_id == focus.id { &edge.to_id } else { &edge.from_id }; if !ids.contains(other) { ids.push(other.clone()); } }
    app.graph_nodes = ids.iter().filter_map(|id| visible.iter().find(|node| &node.id == id).cloned()).collect();
    app.graph_edges = edges; app.graph_selected = 0; app.graph_return = return_view; app.view = View::KnowledgeGraph;
    app.status = format!("Graph expanded: {} active node(s) · ↑↓ select · Enter open · Esc back", app.graph_nodes.len());
}
fn open_graph_node_detail(app: &mut App) { let Some(node) = app.graph_nodes.get(app.graph_selected).cloned() else { return; }; let detail = if node.node_type == "experience" { View::ExperienceDetail } else { View::KnowledgeDetail }; read_detail(app, node, detail); app.detail_return = View::KnowledgeGraph; }
fn draw_graph(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) { let cols = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(64), Constraint::Percentage(36)]).split(area); let focus_id = app.graph_nodes.first().map(|node| node.id.as_str()).unwrap_or_default(); let mut lines = vec![Line::from(Span::styled(format!(" ◉ {}", app.graph_nodes.first().map(|node| node.title.as_str()).unwrap_or("node")), theme.accent())), Line::default()]; for (index, node) in app.graph_nodes.iter().enumerate() { let marker = if index == app.graph_selected { "›" } else { " " }; let kind = match node.node_type.as_str() { "method" => "M", "experience" => "E", _ => "K" }; let relation = if node.id == focus_id { "focus" } else { app.graph_edges.iter().find_map(|edge| if (edge.from_id == focus_id && edge.to_id == node.id) || (edge.to_id == focus_id && edge.from_id == node.id) { Some(edge.relation.as_str()) } else { None }).unwrap_or("related") }; lines.push(Line::from(format!("{marker} {} [{kind}] {relation:<14} {}", if node.id == focus_id { "◉" } else { "├─" }, node.title))); } frame.render_widget(Paragraph::new(lines).style(theme.text()).wrap(Wrap { trim: true }).block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(theme.border(true)).title(" graph · one-hop neighborhood ")), cols[0]); let detail = app.graph_nodes.get(app.graph_selected).map(|node| format!("{}\n\n{}\n\nid        {}\ntype      {}\nstatus    {}\npath      {}\n\nEnter opens the full node.", node.title, node.summary.as_deref().unwrap_or("No summary available."), node.id, node.node_type, node.status.as_deref().unwrap_or("—"), node.path)).unwrap_or_else(|| "No related nodes.".into()); frame.render_widget(Paragraph::new(detail).style(theme.overlay_text()).wrap(Wrap { trim: true }).block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(theme.border(false)).title(" selected node ")), cols[1]); }

fn graph_visible(node: &GraphNode) -> bool { matches!(node.status.as_deref(), Some("committed" | "stale")) }
fn list_visible(node: &GraphNode) -> bool { graph_visible(node) || node.status.as_deref() == Some("rejected") }
fn browsable_nodes(home: &std::path::Path, nodes: &[GraphNode], node_type: &str, filter: &str) -> Vec<GraphNode> { nodes.iter().filter(|node| node.node_type == node_type && list_visible(node) && node_matches_filter(home, node, filter)).cloned().collect() }
fn nodes_for_view(app: &App) -> Vec<GraphNode> { match app.view { View::Knowledge => browsable_nodes(app.engine.home(), &app.nodes, "knowledge", &app.knowledge_filter), View::Method => browsable_nodes(app.engine.home(), &app.nodes, "method", &app.method_filter), View::Experience => browsable_nodes(app.engine.home(), &app.nodes, "experience", &app.experience_filter), View::Review => filtered_nodes(app.engine.home(), &app.nodes, "", Some("candidate"), &app.review_filter), View::MergeTarget => filtered_nodes(app.engine.home(), &app.nodes, "knowledge", Some("committed"), &app.merge_filter), _ => Vec::new() } }
fn active_filter(app: &App) -> &str { match app.view { View::Knowledge => &app.knowledge_filter, View::Method => &app.method_filter, View::Experience => &app.experience_filter, View::Review => &app.review_filter, View::MergeTarget => &app.merge_filter, _ => "" } }
fn filtered_nodes(home: &std::path::Path, nodes: &[GraphNode], node_type: &str, status: Option<&str>, filter: &str) -> Vec<GraphNode> { nodes.iter().filter(|node| (node_type.is_empty() || node.node_type == node_type) && status.is_none_or(|expected| node.status.as_deref() == Some(expected)) && node_matches_filter(home, node, filter)).cloned().collect() }
fn node_metadata(home: &std::path::Path, node: &GraphNode, key: &str) -> String {
    let relative = std::path::Path::new(&node.path);
    if relative.is_absolute() || relative.components().any(|component| component == std::path::Component::ParentDir) { return String::new(); }
    if key == "kind" { return methodus_core::read_graph_document(home, &home.join(relative)).ok().and_then(|document| document.kind).unwrap_or_default(); }
    fs::read_to_string(home.join(relative)).ok().and_then(|raw| raw.lines().find_map(|line| line.trim_start().strip_prefix(&format!("{key}:" )).map(|value| value.trim().trim_matches('"').trim_matches('\'').to_string()))).unwrap_or_default()
}
fn node_matches_filter(home: &std::path::Path, node: &GraphNode, filter: &str) -> bool { filter.split_whitespace().all(|term| { let term = term.to_lowercase(); if let Some(value) = term.strip_prefix("tag:") { return node.tags.iter().any(|tag| tag.to_lowercase().contains(value)); } if let Some(value) = term.strip_prefix("scope:") { return node.scope.as_deref().unwrap_or_default().to_lowercase().contains(value); } if let Some(value) = term.strip_prefix("visibility:") { return node.visibility.to_lowercase() == value; } if let Some(value) = term.strip_prefix("status:") { return node.status.as_deref().unwrap_or_default().to_lowercase() == value; } if let Some(value) = term.strip_prefix("type:") { return node.node_type.to_lowercase() == value; } if let Some(value) = term.strip_prefix("kind:") { return node_metadata(home, node, "kind").to_lowercase().contains(value); } if let Some(value) = term.strip_prefix("outcome:") { return node_metadata(home, node, "outcome").to_lowercase().contains(value); } if let Some(value) = term.strip_prefix("date:") { return node_metadata(home, node, "occurred_at").to_lowercase().contains(value) || node.updated_at.to_rfc3339().to_lowercase().contains(value); } [node.title.as_str(), node.id.as_str(), node.path.as_str(), node.summary.as_deref().unwrap_or(""), node.scope.as_deref().unwrap_or(""), node.visibility.as_str(), node.node_type.as_str()].iter().any(|value| value.to_lowercase().contains(&term)) || node.tags.iter().any(|tag| tag.to_lowercase().contains(&term)) }) }
fn display_filter(filter: &str) -> &str { if filter.is_empty() { "type to filter" } else { filter } }
fn runtime_index(id: &str) -> usize { RUNTIMES.iter().position(|runtime| runtime.id == id).unwrap_or(0) }
fn runtime_label(id: &str) -> &str { RUNTIMES.iter().find(|runtime| runtime.id == id).map(|runtime| runtime.label).unwrap_or(id) }
fn permission_label(mode: &str) -> &'static str { match mode { "cautious" => "Cautious execution", "acceptEdits" => "Auto-edit", _ => "Read-only plan" } }
fn open_runtime_picker(app: &mut App) { app.runtime_selected = runtime_index(&app.runtime); app.view = View::Runtime; app.status = "Choose Learn runtime · ↑↓ select · Enter apply · Esc cancel".into(); }
fn apply_selected_runtime(app: &mut App) { let runtime = RUNTIMES.get(app.runtime_selected).copied().unwrap_or(RUNTIMES[0]); select_runtime(app, runtime.id); }
fn select_runtime(app: &mut App, requested: &str) {
    let Some(runtime) = RUNTIMES.iter().find(|runtime| runtime.id == requested || runtime.label.eq_ignore_ascii_case(requested)) else { app.status = format!("Unknown runtime: {requested} · choose claude-code, codex, or cursor"); return; };
    if app.learning_session_id.is_some() && app.runtime != runtime.id { app.view = View::Chat; app.status = format!("Current Learn is bound to {}; enter /new before switching to {}", runtime_label(&app.runtime), runtime.label); return; }
    let mut config = UserConfig::load(app.engine.home()); config.default_runtime = Some(runtime.id.into()); match config.save(app.engine.home()) { Ok(()) => { app.runtime = runtime.id.into(); app.runtime_selected = runtime_index(runtime.id); app.view = View::Chat; app.status = format!("Learn runtime: {}", runtime.label); }, Err(error) => app.status = format!("Failed to save runtime: {error}") }
}
fn cycle_permission(app: &mut App) {
    let mut config = UserConfig::load(app.engine.home()); config.permission_mode = Some(app.permission_mode.clone()); config.cycle_permission(); let next = config.permission_mode().to_string(); match config.save(app.engine.home()) { Ok(()) => { app.permission_mode = next; app.status = format!("Permission: {} · applies to the next runtime call", permission_label(&app.permission_mode)); }, Err(error) => app.status = format!("Failed to save permission: {error}") }
}
fn close_current_learning(app: &mut App) {
    let had_run = app.learning_session_id.is_some();
    if let Some(run_id) = app.learning_session_id.as_deref() { let _ = app.engine.record_learning_event(run_id, "methodus", "Learn closed by maintainer with /new"); let _ = app.engine.mark_learning_status(run_id, "closed"); }
    app.learning_session_id = None; app.learning_executor_sid = None; app.learning_goal = None; app.view = View::Chat; app.status = "New Learn ready · enter a learning goal and use @ to attach sources".into(); if had_run { app.say("Methodus: Closed the previous learning context; the next input will create a new Learn.".to_string()); }
}

fn centered(area: Rect, width_pct: u16, height_pct: u16) -> Rect { let width = area.width.saturating_mul(width_pct).saturating_div(100).max(50).min(area.width); let height = area.height.saturating_mul(height_pct).saturating_div(100).max(10).min(area.height); Rect { x: area.x + area.width.saturating_sub(width) / 2, y: area.y + area.height.saturating_sub(height) / 2, width, height } }
fn slash_menu_open(input: &str) -> bool { input.trim_start().starts_with('/') }
fn mention_open(input: &str) -> bool { at_query(input).is_some() }
fn mention_start(input: &str) -> Option<usize> { let query = at_query(input)?; Some(input.len().saturating_sub(query.len() + 1)) }
fn absolute_path_candidates(query: &str) -> Vec<MentionCandidate> { let (display_prefix, path_query) = if query == "~" || query.starts_with("~/") { let home = std::env::var_os("HOME").map(std::path::PathBuf::from); ("~".to_string(), home.map(|home| home.join(query.trim_start_matches("~/").trim_start_matches('~')))) } else if query.starts_with('/') { (String::new(), Some(std::path::PathBuf::from(query))) } else { return Vec::new() }; let path_query = path_query.unwrap_or_default(); let (parent, partial) = if path_query.is_dir() { (path_query.clone(), String::new()) } else { (path_query.parent().unwrap_or(std::path::Path::new("/")).to_path_buf(), path_query.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default()) }; let Ok(entries) = fs::read_dir(&parent) else { return Vec::new() }; entries.flatten().filter_map(|entry| { let name = entry.file_name().to_string_lossy().into_owned(); if name.starts_with('.') || (!partial.is_empty() && !name.to_lowercase().contains(&partial)) { return None }; let path = entry.path(); let is_dir = path.is_dir(); let label = if display_prefix == "~" { format!("~/{}{}", path.strip_prefix(std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default()).ok()?.display(), if is_dir { "/" } else { "" }) } else { format!("{}{}", path.display(), if is_dir { "/" } else { "" }) }; Some(MentionCandidate { rel: label.clone(), label, is_dir, abs: path }) }).take(80).collect() }
fn slash_token(input: &str) -> Option<String> { let rest = input.trim_start().strip_prefix('/')?; Some(rest.split_whitespace().next().unwrap_or("").to_ascii_lowercase()) }
fn matching_slash(input: &str) -> Vec<&'static SlashCmd> { let Some(token) = slash_token(input) else { return Vec::new() }; SLASH_COMMANDS.iter().filter(|command| command.name.starts_with(&token) || command.aliases.iter().any(|alias| alias.starts_with(&token))).collect() }
fn slash_rest(input: &str) -> String { input.trim_start().strip_prefix('/').and_then(|rest| rest.split_once(char::is_whitespace)).map(|(_, rest)| rest.trim().to_string()).unwrap_or_default() }
fn prev_char_boundary(text: &str, index: usize) -> usize { text[..index.min(text.len())].char_indices().next_back().map(|(index, _)| index).unwrap_or(0) }
fn next_char_boundary(text: &str, index: usize) -> usize { let index = index.min(text.len()); text[index..].char_indices().nth(1).map(|(offset, _)| index + offset).unwrap_or(text.len()) }
fn floor_char_boundary(text: &str, mut index: usize) -> usize { index = index.min(text.len()); while index > 0 && !text.is_char_boundary(index) { index -= 1; } index }
fn display_cols(text: &str) -> u16 { UnicodeWidthStr::width(text) as u16 }
fn wants_newline(key: KeyEvent) -> bool { key.modifiers.contains(KeyModifiers::SHIFT) && matches!(key.code, KeyCode::Enter | KeyCode::Char('\n')) }
fn wrap_text(text: &str, width: usize) -> Vec<String> { if width == 0 { return vec![text.to_string()] }; let mut output = Vec::new(); for raw in text.split('\n') { if raw.is_empty() { output.push(String::new()); continue; } let mut line = String::new(); let mut columns = 0usize; for character in raw.chars() { let character_width = UnicodeWidthChar::width(character).unwrap_or(0); if columns > 0 && columns + character_width > width { output.push(std::mem::take(&mut line)); columns = 0; } line.push(character); columns += character_width; } output.push(line); } if output.is_empty() { output.push(String::new()); } output }
fn composer_height(input: &str, width: u16) -> u16 { let inner = width.saturating_sub(4).max(8) as usize; (wrap_text(input, inner.saturating_sub(2).max(1)).len().clamp(1, COMPOSER_MAX_ROWS) as u16 + 2).clamp(3, 10) }
fn composer_view(input: &str, cursor: usize, width: usize, max_rows: usize) -> (Vec<String>, u16, u16) { let cursor = floor_char_boundary(input, cursor); let inner = width.saturating_sub(2).max(1); let mut visual = Vec::new(); let mut cursor_row = 0u16; let mut cursor_col = 2u16; let mut found = false; let mut global = 0usize; let logical: Vec<&str> = if input.is_empty() { vec![""] } else { input.split('\n').collect() }; for (line_index, raw) in logical.iter().enumerate() { let line_start = global; let wrapped = wrap_text(raw, inner); let mut local = 0usize; for (wrap_index, piece) in wrapped.iter().enumerate() { visual.push(format!("{}{}", if line_index == 0 && wrap_index == 0 { "› " } else { "  " }, piece)); let start = line_start + local; let end = start + piece.len(); if !found && cursor >= start && cursor <= end { found = true; cursor_row = visual.len().saturating_sub(1) as u16; let offset = floor_char_boundary(piece, cursor.saturating_sub(start)); cursor_col = 2 + display_cols(&piece[..offset]); } local += piece.len(); } global = line_start + raw.len(); if line_index + 1 < logical.len() { global += 1; } } if visual.is_empty() { visual.push("› ".into()); } if visual.len() > max_rows { let row = cursor_row as usize; let start = row.saturating_add(1).saturating_sub(max_rows); return (visual[start..start + max_rows].to_vec(), cursor_col, (row - start) as u16); } (visual, cursor_col, cursor_row) }
fn layout_transcript(entries: &[String], width: usize) -> Vec<String> { let mut rows = Vec::new(); for entry in entries { let (label, body) = if let Some(body) = entry.strip_prefix("You: ") { ("you", body) } else if let Some(body) = entry.strip_prefix("Methodus:") { ("methodus", body) } else { ("methodus", entry.as_str()) }; let prefix = format!("{label:<9}"); let continuation = " ".repeat(9); for (index, line) in wrap_text(body.trim(), width.saturating_sub(9).max(8)).into_iter().enumerate() { rows.push(format!("{}{}", if index == 0 { &prefix } else { &continuation }, line)); } rows.push(String::new()); } rows }
fn transcript_line(row: &str, theme: &Theme) -> Line<'static> { if row.is_empty() { return Line::default() }; if let Some(body) = row.strip_prefix("you      ") { return Line::from(vec![Span::styled("you      ", theme.accent()), Span::styled(body.to_string(), theme.text())]); } if let Some(body) = row.strip_prefix("methodus ") { return Line::from(vec![Span::styled("methodus ", theme.dim().add_modifier(Modifier::BOLD)), Span::styled(body.to_string(), theme.dim())]); } Line::from(Span::styled(row.to_string(), theme.dim())) }
fn ctrl_c_should_quit(pending: Option<Instant>, now: Instant) -> bool { pending.is_some_and(|then| now.duration_since(then) <= CTRL_C_QUIT_WINDOW) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn slash_palette_exposes_management_commands() { let names = matching_slash("/").into_iter().map(|command| command.name).collect::<Vec<_>>(); for required in ["knowledge", "method", "experience", "team", "health", "runtime", "review", "new", "open", "quit"] { assert!(names.contains(&required)); } assert!(!names.contains(&"graph")); assert!(!names.contains(&"learn")); assert_eq!(matching_slash("/exit")[0].name, "quit"); }
    #[test] fn runtime_and_permission_labels_are_concrete() { assert_eq!(runtime_label("claude-code"), "Claude Code"); assert_eq!(runtime_index("codex"), 1); assert_eq!(permission_label("plan"), "Read-only plan"); assert_eq!(permission_label("cautious"), "Cautious execution"); assert_eq!(permission_label("acceptEdits"), "Auto-edit"); }
    #[test] fn only_shift_enter_creates_multiline_input() { assert!(wants_newline(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT))); assert!(!wants_newline(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL))); assert!(!wants_newline(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))); }
    #[test] fn composer_cursor_uses_unicode_display_columns() { let (rows, column, row) = composer_view("hello", 2, 20, 8); assert_eq!(rows, vec!["› hello"]); assert_eq!(column, 4); assert_eq!(row, 0); assert_eq!(next_char_boundary("hello", 0), 1); }
    #[test] fn transcript_wraps_by_terminal_width() { assert_eq!(wrap_text("one two three four", 8), vec!["one two ", "three fo", "ur"]); }
    #[test] fn ctrl_c_requires_second_press_inside_window() { let now = Instant::now(); assert!(!ctrl_c_should_quit(None, now)); assert!(ctrl_c_should_quit(Some(now), now)); assert!(!ctrl_c_should_quit(Some(now - Duration::from_secs(4)), now)); }
}
