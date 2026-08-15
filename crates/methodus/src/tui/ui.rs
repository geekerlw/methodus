use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs, Wrap};
use ratatui::Frame;

use methodus_core::PermissionMode;

use super::app::{
    ellipsize, help_line, matching_slash, slash_menu_open, status_counts, App, ChatKind, ChatLine,
    Focus, Mode, Page, PromptKind, SetupSection, StatusLevel,
};

/// Facet mark: a diamond with a center — Methodus holds the method, not the work.
const MARK: &str = "◈";
const WORDMARK: &str = "Methodus";

struct Theme {
    fg: Color,
    muted: Color,
    emphasis: Color,
    bg: Color,
    surface: Color,
    overlay: Color,
    on_overlay: Color,
    border: Color,
    accent: Color,
    info: Color,
    warning: Color,
    success: Color,
    error: Color,
    permission: Color,
    plan: Color,
    auto_accept: Color,
}

impl Theme {
    fn current() -> Self {
        if std::env::var_os("NO_COLOR").is_some() {
            return Self {
                fg: Color::Reset,
                muted: Color::Reset,
                emphasis: Color::Reset,
                bg: Color::Reset,
                surface: Color::Reset,
                overlay: Color::Reset,
                on_overlay: Color::Reset,
                border: Color::Reset,
                accent: Color::Reset,
                info: Color::Reset,
                warning: Color::Reset,
                success: Color::Reset,
                error: Color::Reset,
                permission: Color::Reset,
                plan: Color::Reset,
                auto_accept: Color::Reset,
            };
        }
        // Swiss/minimal tokens on Claude Code's canvas:
        // body inherits the terminal; painted color is scarce (labels, focus, status).
        // Rejected the skill's default "dev tool" slate-navy + neon green — that is
        // the ink-blue skin. Accent stays terracotta; error is a separate red.
        Self {
            fg: Color::Reset,
            muted: Color::Rgb(118, 118, 112),
            emphasis: Color::Reset,
            bg: Color::Reset,
            surface: Color::Reset,
            overlay: Color::Rgb(28, 28, 26),
            on_overlay: Color::Rgb(245, 241, 234),
            border: Color::Rgb(72, 72, 68),
            accent: Color::Rgb(218, 119, 86),
            info: Color::Rgb(122, 158, 170),
            warning: Color::Rgb(212, 160, 84),
            success: Color::Rgb(167, 176, 110),
            error: Color::Rgb(224, 108, 117),
            permission: Color::Rgb(201, 162, 39),
            plan: Color::Rgb(122, 158, 170),
            auto_accept: Color::Rgb(167, 176, 110),
        }
    }

    fn text(&self) -> Style {
        Style::default().fg(self.fg).bg(self.surface)
    }

    fn dim(&self) -> Style {
        Style::default().fg(self.muted).bg(self.surface)
    }

    fn overlay_text(&self) -> Style {
        Style::default().fg(self.on_overlay).bg(self.overlay)
    }

    fn accent_border(&self) -> Style {
        Style::default().fg(self.accent)
    }

    fn muted_border(&self) -> Style {
        Style::default().fg(self.border)
    }

    fn selected(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    fn label(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    fn mode_border(&self, mode: PermissionMode) -> Color {
        match mode {
            PermissionMode::Plan => self.plan,
            PermissionMode::AcceptEdits => self.auto_accept,
            PermissionMode::Cautious => self.permission,
        }
    }
}

pub fn draw(frame: &mut Frame, app: &App) {
    let theme = Theme::current();
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(theme.bg)), area);
    if area.width < 80 || area.height < 24 {
        frame.render_widget(
            Paragraph::new(format!(
                "{MARK}  {WORDMARK}\nterminal too small (need 80x24)"
            ))
            .style(theme.dim())
            .alignment(Alignment::Center),
            area,
        );
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(area);

    draw_header(frame, chunks[0], app, &theme);
    match app.page {
        Page::Work => draw_work(frame, chunks[1], app, &theme),
        Page::Faces => draw_faces(frame, chunks[1], app, &theme),
        Page::Review => draw_review(frame, chunks[1], app, &theme),
        Page::Setup => draw_setup(frame, chunks[1], app, &theme),
    }
    draw_footer(frame, chunks[2], app, &theme);

    if app.mode == Mode::Answering {
        draw_answer_modal(frame, area, app, &theme);
    }
    if app.mode == Mode::Prompt {
        draw_prompt_modal(frame, area, app, &theme);
    }
    if app.show_help {
        draw_help_overlay(frame, area, &theme);
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let titles: Vec<Line> = Page::all()
        .iter()
        .map(|p| {
            let icon = match p {
                Page::Work => "◆",
                Page::Faces => "◇",
                Page::Review => "▣",
                Page::Setup => "○",
            };
            Line::from(format!(" {icon} {} ", p.title()))
        })
        .collect();
    let selected = Page::all().iter().position(|p| *p == app.page).unwrap_or(0);
    let runtime = app.runtime.as_deref().unwrap_or("claude-code");
    let face = app.default_face.as_deref().unwrap_or("auto");
    let wait_mark = if app.approvals.is_empty() {
        String::new()
    } else {
        format!("  !{}", app.approvals.len())
    };
    let rec = if app.recovered.is_empty() {
        String::new()
    } else {
        format!("  rec{}", app.recovered.len())
    };
    let usage = if app.usage_today.turns > 0 {
        format!("  {}", app.usage_today.compact())
    } else if app.usage_all.turns > 0 {
        format!("  {}", app.usage_all.compact())
    } else {
        String::new()
    };
    let tabs = Tabs::new(titles)
        .select(selected)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Line::from(vec![
                    Span::styled(format!(" {MARK}  "), theme.label()),
                    Span::styled(
                        format!("{WORDMARK}  /  {runtime}  ·  face {face}"),
                        theme.dim(),
                    ),
                    Span::styled(wait_mark, Style::default().fg(theme.warning)),
                    Span::styled(format!("{rec}{usage} "), theme.dim()),
                ]))
                .border_style(theme.muted_border())
                .style(theme.text()),
        )
        .highlight_style(theme.label())
        .style(theme.dim());
    frame.render_widget(tabs, area);
}

fn draw_work(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(20)])
        .split(area);

    draw_task_list(frame, cols[0], app, theme);

    let composer_h = if app.input_error.is_some() { 4 } else { 3 };
    let slash_open = app.focus == Focus::Session && slash_menu_open(&app.input);
    let mention_open = app.focus == Focus::Session && app.mention_menu_open();
    let slash_h = if slash_open {
        let n = matching_slash(&app.input).len().max(1);
        (n as u16 + 2).min(8)
    } else {
        0
    };
    let mention_h = if mention_open {
        let n = app.matching_mentions().len().max(1);
        (n as u16 + 2).min(10)
    } else {
        0
    };
    let mut parts = vec![Constraint::Min(3)];
    if !app.approvals.is_empty() {
        parts.push(Constraint::Length(4));
    }
    if slash_h > 0 {
        parts.push(Constraint::Length(slash_h));
    }
    if mention_h > 0 {
        parts.push(Constraint::Length(mention_h));
    }
    parts.push(Constraint::Length(composer_h));
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(parts)
        .split(cols[1]);

    draw_transcript(frame, rows[0], app, theme, app.focus == Focus::Session);
    let mut i = 1;
    if !app.approvals.is_empty() {
        frame.render_widget(
            approval_list(app, theme, app.focus == Focus::Inbox),
            rows[i],
        );
        i += 1;
    }
    if slash_h > 0 {
        draw_slash_menu(frame, rows[i], app, theme);
        i += 1;
    }
    if mention_h > 0 {
        draw_at_menu(frame, rows[i], app, theme);
        i += 1;
    }
    draw_composer(frame, rows[i], app, theme);
}

fn draw_slash_menu(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let matches = matching_slash(&app.input);
    let items: Vec<ListItem> = if matches.is_empty() {
        vec![ListItem::new("  no matching command").style(theme.dim())]
    } else {
        matches
            .iter()
            .enumerate()
            .map(|(i, cmd)| {
                let mark = if i == app.slash_sel { ">" } else { " " };
                let alias = if cmd.aliases.is_empty() {
                    String::new()
                } else {
                    format!("  /{}", cmd.aliases.join(" /"))
                };
                let style = if i == app.slash_sel {
                    theme.selected()
                } else {
                    theme.text()
                };
                ListItem::new(format!("{mark} /{}{alias}  {}", cmd.name, cmd.summary)).style(style)
            })
            .collect()
    };
    frame.render_widget(
        List::new(items)
            .block(panel(
                " commands  ·  tab complete  ·  enter run ",
                true,
                theme,
            ))
            .style(theme.text()),
        area,
    );
}

fn draw_at_menu(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let matches = app.matching_mentions();
    let items: Vec<ListItem> = if matches.is_empty() && app.mention_cache_empty() {
        vec![ListItem::new(
            "  no readable dirs — launch from a folder, or register a project",
        )
        .style(theme.dim())]
    } else if matches.is_empty() {
        vec![ListItem::new("  no matching files").style(theme.dim())]
    } else {
        matches
            .iter()
            .enumerate()
            .map(|(i, cand)| {
                let mark = if i == app.mention_sel { ">" } else { " " };
                let kind = if cand.is_dir { "dir " } else { "file" };
                let style = if i == app.mention_sel {
                    theme.selected()
                } else if cand.is_dir {
                    Style::default().fg(theme.info)
                } else {
                    theme.text()
                };
                ListItem::new(format!("{mark} {kind}  {}", cand.label)).style(style)
            })
            .collect()
    };
    frame.render_widget(
        List::new(items)
            .block(panel(
                " @  ·  tab drill folder  ·  enter attach ",
                true,
                theme,
            ))
            .style(theme.text()),
        area,
    );
}

fn draw_task_list(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let focused = app.page == Page::Work && app.focus == Focus::Tasks;
    let (queued, running, waiting, done) = status_counts(&app.tasks);
    let title = format!(" tasks  q{queued} r{running} w{waiting} d{done} ");
    if app.tasks.is_empty() {
        frame.render_widget(
            Paragraph::new("no tasks\ntype on the right")
                .style(theme.dim())
                .block(panel(&title, focused, theme)),
            area,
        );
        return;
    }
    let inner_w = area.width.saturating_sub(4) as usize;
    let title_w = inner_w.saturating_sub(15);
    let items: Vec<ListItem> = app
        .tasks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mark = if i == app.task_sel { ">" } else { " " };
            let selected = i == app.task_sel;
            let status_style = status_style(&t.status, theme);
            let title_style = if selected {
                if focused {
                    theme.selected()
                } else {
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                }
            } else {
                theme.text()
            };
            let line = Line::from(vec![
                Span::styled(format!("{mark} "), title_style),
                Span::styled(format!("{:<12}", t.status), status_style),
                Span::styled(
                    format!(" {}", ellipsize(&t.title, title_w.max(4))),
                    title_style,
                ),
            ]);
            ListItem::new(line)
        })
        .collect();
    frame.render_widget(
        List::new(items)
            .block(panel(&title, focused, theme))
            .style(theme.text()),
        area,
    );
}

fn draw_transcript(frame: &mut Frame, area: Rect, app: &App, theme: &Theme, focused: bool) {
    let title = match &app.session_task_id {
        Some(id) => format!(" session  {id} "),
        None => " session ".to_string(),
    };
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;
    let rows = layout_chat(&app.transcript, inner_w.max(8));
    let visible: Vec<Line> = if rows.is_empty() {
        splash_lines(theme, inner_h >= 10)
    } else {
        let end = rows.len().saturating_sub(app.transcript_offset);
        let start = end.saturating_sub(inner_h.max(1));
        rows[start..end]
            .iter()
            .map(|row| chat_line(row, theme))
            .collect()
    };
    frame.render_widget(
        Paragraph::new(visible)
            .alignment(if app.transcript.is_empty() {
                Alignment::Center
            } else {
                Alignment::Left
            })
            .block(panel(&title, focused && app.input.is_empty(), theme))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn splash_lines(theme: &Theme, tall: bool) -> Vec<Line<'static>> {
    let mark = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let name = Style::default()
        .fg(theme.emphasis)
        .add_modifier(Modifier::BOLD);
    let dim = theme.dim();
    if tall {
        vec![
            Line::from(""),
            Line::from(Span::styled("  ╱╲", mark)),
            Line::from(vec![
                Span::styled(" ╱", mark),
                Span::styled(MARK, mark),
                Span::styled("╲", mark),
            ]),
            Line::from(Span::styled("  ╲╱", mark)),
            Line::from(""),
            Line::from(Span::styled(WORDMARK, name)),
            Line::from(Span::styled("the system that remembers how you work", dim)),
            Line::from(""),
            Line::from(Span::styled("type a message below to start", dim)),
        ]
    } else {
        vec![
            Line::from(vec![
                Span::styled(MARK, mark),
                Span::raw("  "),
                Span::styled(WORDMARK, name),
            ]),
            Line::from(Span::styled("the system that remembers how you work", dim)),
        ]
    }
}

#[derive(Debug, Clone)]
struct ChatRow {
    kind: ChatKind,
    pad: usize,
    text: String,
    is_label: bool,
}

fn layout_chat(entries: &[ChatLine], width: usize) -> Vec<ChatRow> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let kind = entries[i].kind;
        let mut j = i + 1;
        while j < entries.len() && entries[j].kind == kind {
            j += 1;
        }
        let group = &entries[i..j];
        if !out.is_empty() {
            out.push(ChatRow {
                kind,
                pad: 0,
                text: String::new(),
                is_label: false,
            });
        }
        match kind {
            ChatKind::You => {
                let bubble = (width * 2 / 3).max(8).min(width.max(8));
                push_label(&mut out, kind, "you", width, true);
                for entry in group {
                    for wrapped in wrap_text(&entry.text, bubble) {
                        let pad = width.saturating_sub(wrapped.chars().count());
                        out.push(ChatRow {
                            kind,
                            pad,
                            text: wrapped,
                            is_label: false,
                        });
                    }
                }
            }
            ChatKind::Assistant => {
                push_label(&mut out, kind, "assistant", width, false);
                for entry in group {
                    for wrapped in wrap_text(&entry.text, width) {
                        out.push(ChatRow {
                            kind,
                            pad: 0,
                            text: wrapped,
                            is_label: false,
                        });
                    }
                }
            }
            ChatKind::Thinking => {
                push_label(&mut out, kind, "thinking", width, false);
                for entry in group {
                    for wrapped in wrap_text(&entry.text, width.saturating_sub(2).max(8)) {
                        out.push(ChatRow {
                            kind,
                            pad: 2,
                            text: wrapped,
                            is_label: false,
                        });
                    }
                }
            }
            _ => {
                for entry in group {
                    for wrapped in wrap_text(&entry.text, width) {
                        out.push(ChatRow {
                            kind,
                            pad: 0,
                            text: wrapped,
                            is_label: false,
                        });
                    }
                }
            }
        }
        i = j;
    }
    out
}

fn push_label(out: &mut Vec<ChatRow>, kind: ChatKind, label: &str, width: usize, right: bool) {
    let pad = if right {
        width.saturating_sub(label.chars().count())
    } else {
        0
    };
    out.push(ChatRow {
        kind,
        pad,
        text: label.to_string(),
        is_label: true,
    });
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    for raw in text.split('\n') {
        if raw.is_empty() {
            out.push(String::new());
            continue;
        }
        let chars: Vec<char> = raw.chars().collect();
        for chunk in chars.chunks(width) {
            out.push(chunk.iter().collect());
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn chat_line(row: &ChatRow, theme: &Theme) -> Line<'static> {
    let mut style = match row.kind {
        ChatKind::You | ChatKind::Assistant => theme.text(),
        ChatKind::Thinking => theme.dim().add_modifier(Modifier::ITALIC),
        ChatKind::Tool => Style::default().fg(theme.info),
        ChatKind::Meta => theme.dim(),
        ChatKind::Alert => Style::default().fg(theme.warning),
    };
    if row.is_label {
        style = match row.kind {
            ChatKind::Assistant => theme.label(),
            ChatKind::You => theme.dim().add_modifier(Modifier::BOLD),
            ChatKind::Thinking => theme.dim().add_modifier(Modifier::ITALIC | Modifier::BOLD),
            _ => style.add_modifier(Modifier::BOLD),
        };
    }
    let pad = " ".repeat(row.pad);
    Line::from(vec![Span::raw(pad), Span::styled(row.text.clone(), style)])
}

fn draw_composer(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let focused = app.page == Page::Work && app.focus == Focus::Session;
    let err = app.input_error.as_deref();
    let mode = PermissionMode::parse(Some(app.permission_mode.as_str()));
    let title = if let Some(e) = err {
        format!(" message  ·  {e} ")
    } else if app.busy() {
        " message  ·  wait ".to_string()
    } else {
        format!(" message  ·  {}  ·  enter send ", mode.as_str())
    };
    let inner_w = area.width.saturating_sub(4) as usize;
    let (shown, cursor_col) = visible_input(&app.input, inner_w.max(1));
    let hint = err
        .map(|_| String::new())
        .unwrap_or_else(|| match app.session_task() {
            None => "  first message starts a task · / commands · @ files".to_string(),
            Some(t) if app.busy() => format!("  running {} — wait", t.status),
            Some(t) => format!("  {}  {}", t.status, ellipsize(&t.title, 28)),
        });
    let body = if area.height >= 4 {
        format!("> {shown}\n{hint}")
    } else {
        format!("> {shown}")
    };
    let border = if err.is_some() {
        Style::default().fg(theme.error)
    } else if focused {
        Style::default().fg(theme.mode_border(mode))
    } else {
        theme.muted_border()
    };
    frame.render_widget(
        Paragraph::new(body).style(theme.text()).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border)
                .style(theme.text()),
        ),
        area,
    );
    if focused {
        let x = area.x.saturating_add(1).saturating_add(cursor_col);
        let y = area.y.saturating_add(1);
        if x < area.x.saturating_add(area.width.saturating_sub(1)) {
            frame.set_cursor_position(Position { x, y });
        }
    }
}

fn visible_input(input: &str, width: usize) -> (String, u16) {
    let chars: Vec<char> = input.chars().collect();
    if chars.len() <= width {
        return (input.to_string(), 2 + chars.len() as u16);
    }
    let shown: String = chars[chars.len() - width..].iter().collect();
    (shown, 2 + width as u16)
}

fn draw_faces(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if app.faces.is_empty() {
        frame.render_widget(
            Paragraph::new("no faces yet")
                .style(theme.dim())
                .block(panel(" faces ", true, theme)),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .faces
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mark = if i == app.face_sel { ">" } else { " " };
            let pin = if app.default_face.as_deref() == Some(f.id.as_str()) {
                "  [default]"
            } else {
                ""
            };
            let style = if i == app.face_sel {
                Style::default()
                    .fg(theme.emphasis)
                    .add_modifier(Modifier::BOLD)
            } else {
                theme.text()
            };
            ListItem::new(format!(
                "{mark} {} — {}{pin}\n    [{}] {}",
                f.id,
                f.name,
                f.source,
                ellipsize(&f.description, 60)
            ))
            .style(style)
        })
        .collect();
    frame.render_widget(
        List::new(items)
            .block(panel(" faces  ·  enter pin as default ", true, theme))
            .style(theme.text()),
        area,
    );
}

fn draw_review(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if app.questions.is_empty() && app.knowledge.is_empty() {
        frame.render_widget(
            Paragraph::new("nothing to review")
                .style(theme.dim())
                .block(panel(" review ", true, theme)),
            area,
        );
        return;
    }
    let mut items: Vec<ListItem> = Vec::new();
    let mut idx = 0usize;
    for q in &app.questions {
        let mark = if idx == app.review_sel { ">" } else { " " };
        let style = sel_style(idx == app.review_sel, theme.warning, theme);
        items.push(
            ListItem::new(format!(
                "{mark} Q  {:<10} freq={:<3.0} val={:.2}  {}",
                q.status, q.frequency, q.value, q.question
            ))
            .style(style),
        );
        idx += 1;
    }
    for k in &app.knowledge {
        let mark = if idx == app.review_sel { ">" } else { " " };
        let kind = if k.source == methodus_core::learning::SKILL_DRAFT_SOURCE {
            "S"
        } else {
            "K"
        };
        let style = sel_style(idx == app.review_sel, theme.fg, theme);
        items.push(
            ListItem::new(format!(
                "{mark} {kind}  {:<10} {:<12}  {}",
                k.status, k.source, k.path
            ))
            .style(style),
        );
        idx += 1;
    }
    frame.render_widget(
        List::new(items)
            .block(panel(" review ", true, theme))
            .style(theme.text()),
        area,
    );
}

fn sel_style(selected: bool, idle: Color, theme: &Theme) -> Style {
    if selected {
        theme.selected()
    } else {
        Style::default().fg(idle)
    }
}

fn approval_list<'a>(app: &'a App, theme: &'a Theme, focused: bool) -> List<'a> {
    let title = if focused {
        " approvals  ·  focused "
    } else {
        " approvals  ·  y/s/d/x when message is empty "
    };
    if app.approvals.is_empty() {
        return List::new([ListItem::new("none").style(theme.dim())])
            .block(panel(title, focused, theme))
            .style(theme.text());
    }
    let items: Vec<ListItem> = app
        .approvals
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let mark = if i == app.approval_sel { ">" } else { " " };
            let style = if i == app.approval_sel {
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.warning)
            };
            ListItem::new(format!(
                "{mark} {}  {}  {}",
                a.tool_name,
                ellipsize(&a.subject, 40),
                a.id
            ))
            .style(style)
        })
        .collect();
    List::new(items)
        .block(panel(title, focused, theme))
        .style(theme.text())
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);
    let status_style = match app.status_level {
        StatusLevel::Error => Style::default()
            .fg(theme.error)
            .add_modifier(Modifier::BOLD),
        StatusLevel::Warn => Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD),
        StatusLevel::Ok => theme.dim(),
        StatusLevel::Info => theme.dim(),
    };
    frame.render_widget(
        Paragraph::new(app.status.as_str()).style(status_style),
        rows[0],
    );
    frame.render_widget(Paragraph::new(help_line(app)).style(theme.dim()), rows[1]);
}

fn draw_setup(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Min(5),
            Constraint::Min(5),
            Constraint::Length(6),
        ])
        .split(area);

    draw_setup_settings(frame, rows[0], app, theme);
    draw_setup_projects(frame, rows[1], app, theme);
    draw_setup_packs(frame, rows[2], app, theme);
    draw_setup_health(frame, rows[3], app, theme);
}

fn draw_setup_settings(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let focused = app.setup_section == SetupSection::Settings;
    let ws = if app.workspace_root.trim().is_empty() {
        "(default workspaces/)".to_string()
    } else {
        app.workspace_root.clone()
    };
    let rows = [
        format!(
            "  runtime          {}",
            app.runtime.as_deref().unwrap_or("claude-code")
        ),
        format!(
            "  permission       {} — {}",
            app.permission_mode,
            PermissionMode::parse(Some(app.permission_mode.as_str())).label()
        ),
        format!(
            "  notifications    {}",
            if app.notifications { "on" } else { "off" }
        ),
        format!("  workspace root   {}", ellipsize(&ws, 48)),
    ];
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let mark = if focused && i == app.setup_sel {
                ">"
            } else {
                " "
            };
            let style = if focused && i == app.setup_sel {
                Style::default()
                    .fg(theme.emphasis)
                    .add_modifier(Modifier::BOLD)
            } else {
                theme.text()
            };
            ListItem::new(format!("{mark}{line}")).style(style)
        })
        .collect();
    frame.render_widget(
        List::new(items)
            .block(panel(
                " settings  ·  enter cycle  a edit workspace ",
                focused,
                theme,
            ))
            .style(theme.text()),
        area,
    );
}

fn draw_setup_projects(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let focused = app.setup_section == SetupSection::Projects;
    let items: Vec<ListItem> = if app.projects.is_empty() {
        vec![ListItem::new("  (none — a to add a repo directory)").style(theme.dim())]
    } else {
        app.projects
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mark = if focused && i == app.setup_sel {
                    ">"
                } else {
                    " "
                };
                let star = if p.focus { "*" } else { " " };
                let style = if focused && i == app.setup_sel {
                    Style::default()
                        .fg(theme.emphasis)
                        .add_modifier(Modifier::BOLD)
                } else {
                    theme.text()
                };
                ListItem::new(format!(
                    "{mark}{star} {:<16} {}",
                    p.id,
                    ellipsize(&p.root.display().to_string(), 42)
                ))
                .style(style)
            })
            .collect()
    };
    frame.render_widget(
        List::new(items)
            .block(panel(
                " projects  ·  enter focus  a add  d drop ",
                focused,
                theme,
            ))
            .style(theme.text()),
        area,
    );
}

fn draw_setup_packs(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let focused = app.setup_section == SetupSection::Packs;
    let items: Vec<ListItem> = if app.packs.is_empty() {
        vec![ListItem::new("  (none — a to register a pack.yaml folder)").style(theme.dim())]
    } else {
        app.packs
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mark = if focused && i == app.setup_sel {
                    ">"
                } else {
                    " "
                };
                let star = if p.focus { "*" } else { " " };
                let state = if p.active { "on " } else { "off" };
                let style = if focused && i == app.setup_sel {
                    Style::default()
                        .fg(theme.emphasis)
                        .add_modifier(Modifier::BOLD)
                } else {
                    theme.text()
                };
                ListItem::new(format!(
                    "{mark}{star} {:<16} {state}  {}",
                    p.id,
                    ellipsize(&p.root.display().to_string(), 36)
                ))
                .style(style)
            })
            .collect()
    };
    frame.render_widget(
        List::new(items)
            .block(panel(
                " packs  ·  enter focus  space on/off  a add  d drop ",
                focused,
                theme,
            ))
            .style(theme.text()),
        area,
    );
}

fn draw_setup_health(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let mut lines: Vec<Line> = if app.health.is_empty() {
        vec![Line::from("  (checking)")]
    } else {
        app.health
            .iter()
            .map(|c| {
                let (mark, mark_style) = if c.ok {
                    ("ok", Style::default().fg(theme.success))
                } else {
                    (
                        "!!",
                        Style::default()
                            .fg(theme.error)
                            .add_modifier(Modifier::BOLD),
                    )
                };
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(format!("{mark}  "), mark_style),
                    Span::styled(format!("{:<24} {}", c.label, c.detail), theme.dim()),
                ])
            })
            .collect()
    };
    lines.push(Line::from(vec![Span::styled(
        format!("  --  {:<24} {}", "usage today", app.usage_today.compact()),
        theme.dim(),
    )]));
    lines.push(Line::from(vec![Span::styled(
        format!("  --  {:<24} {}", "usage all", app.usage_all.compact()),
        theme.dim(),
    )]));
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.text())
            .block(panel(" health ", false, theme)),
        area,
    );
}

fn draw_prompt_modal(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let popup = centered(area, 72, 8);
    frame.render_widget(Clear, popup);
    let title = match app.prompt_kind {
        Some(PromptKind::AddProject) => " add project directory ",
        Some(PromptKind::AddPack) => " add pack folder ",
        Some(PromptKind::WorkspaceRoot) => " workspace root ",
        None => " path ",
    };
    let err = app.input_error.as_deref().unwrap_or("");
    let body = format!("path>\n{}\n{err}", app.input);
    let border = if app.input_error.is_some() {
        Style::default().fg(theme.error)
    } else {
        theme.accent_border()
    };
    frame.render_widget(
        Paragraph::new(body).style(theme.overlay_text()).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border)
                .style(theme.overlay_text()),
        ),
        popup,
    );
    let cursor_x = popup.x + 1 + app.input.len() as u16;
    let cursor_y = popup.y + 2;
    if cursor_x < popup.x.saturating_add(popup.width.saturating_sub(1)) {
        frame.set_cursor_position(Position {
            x: cursor_x,
            y: cursor_y,
        });
    }
}

fn draw_answer_modal(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let popup = centered(area, 70, 8);
    frame.render_widget(Clear, popup);
    let qid = app.answering_id.as_deref().unwrap_or("-");
    let err = app.input_error.as_deref().unwrap_or("");
    let body = format!("answer {qid}>\n{}\n{err}", app.input);
    let border = if app.input_error.is_some() {
        Style::default().fg(theme.error)
    } else {
        theme.accent_border()
    };
    frame.render_widget(
        Paragraph::new(body).style(theme.overlay_text()).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" answer question ")
                .border_style(border)
                .style(theme.overlay_text()),
        ),
        popup,
    );
    let cursor_x = popup.x + 1 + app.input.len() as u16;
    let cursor_y = popup.y + 2;
    if cursor_x < popup.x.saturating_add(popup.width.saturating_sub(1)) {
        frame.set_cursor_position(Position {
            x: cursor_x,
            y: cursor_y,
        });
    }
}

fn draw_help_overlay(frame: &mut Frame, area: Rect, theme: &Theme) {
    let popup = centered(area, 72, 30);
    frame.render_widget(Clear, popup);
    let lines = vec![
        Line::from(vec![
            Span::styled(format!("{MARK}  {WORDMARK}"), theme.label()),
            Span::styled("  ·  keys", theme.dim()),
        ]),
        Line::from("  type         chat in the message box (session focused)"),
        Line::from("  enter        send — first line creates+runs a task"),
        Line::from("  up/down      scroll transcript"),
        Line::from("  esc          session -> tasks (does not quit)"),
        Line::from("  tab          tasks <-> session <-> approvals"),
        Line::from("  ctrl-n       new conversation"),
        Line::from("  n            new conversation (from tasks)"),
        Line::from("  r / R        run / resume selected task"),
        Line::from("  c then y     cancel selected task"),
        Line::from("  @            attach file or folder from the focus project"),
        Line::from("  /            commands: /clear /help /learn /quit"),
        Line::from(
            "  /clear       new conversation — next turn does not resume Claude/Cursor/Codex",
        ),
        Line::from("  /learn       draft a skill from this conversation (Review to commit)"),
        Line::from("  y s d x      approve once / session / deny / abort"),
        Line::from("  t            cycle runtime claude-code / cursor / codex"),
        Line::from("  1 2 3 4      work / faces / review / setup"),
        Line::from("  [ ]          previous / next page"),
        Line::from("  ?            help"),
        Line::from("  /quit        quit (or /exit, or ctrl-c twice)"),
        Line::from(""),
        Line::from("Setup: cycle runtime + permission (edits / plan / ask)."),
        Line::from("Idle: Methodus asks a high-value question when you are not running."),
        Line::from("Keep this process open (tmux). Workspaces live under Methodus home."),
    ];
    frame.render_widget(
        Paragraph::new(lines).style(theme.overlay_text()).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" help ")
                .border_style(theme.accent_border())
                .style(theme.overlay_text()),
        ),
        popup,
    );
}

fn panel<'a>(title: &'a str, focused: bool, theme: &'a Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(if focused {
            theme.accent_border()
        } else {
            theme.muted_border()
        })
        .style(theme.text())
}

fn status_style(status: &methodus_domain::TaskStatus, theme: &Theme) -> Style {
    use methodus_domain::TaskStatus::*;
    match status {
        Running => Style::default().fg(theme.success),
        WaitingUser | Reviewing => Style::default().fg(theme.warning),
        Failed => Style::default().fg(theme.error),
        Completed | Cancelled => theme.dim(),
        _ => theme.text(),
    }
}

fn centered(area: Rect, percent_x: u16, height: u16) -> Rect {
    let popup_h = height.min(area.height);
    let popup_w = (area.width.saturating_mul(percent_x) / 100)
        .max(20)
        .min(area.width);
    let x = area.x + area.width.saturating_sub(popup_w) / 2;
    let y = area.y + area.height.saturating_sub(popup_h) / 2;
    Rect {
        x,
        y,
        width: popup_w,
        height: popup_h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{ChatKind, ChatLine};

    #[test]
    fn you_sits_on_the_right_assistant_on_the_left() {
        let rows = layout_chat(
            &[
                ChatLine {
                    kind: ChatKind::You,
                    text: "hi".into(),
                },
                ChatLine {
                    kind: ChatKind::Assistant,
                    text: "hello".into(),
                },
            ],
            24,
        );
        let you = rows
            .iter()
            .find(|r| r.kind == ChatKind::You && !r.is_label && r.text == "hi")
            .unwrap();
        let asst = rows
            .iter()
            .find(|r| r.kind == ChatKind::Assistant && !r.is_label && r.text == "hello")
            .unwrap();
        assert!(
            you.pad > asst.pad,
            "you pad {} vs assistant {}",
            you.pad,
            asst.pad
        );
        assert_eq!(asst.pad, 0);
        assert!(rows.iter().any(|r| r.is_label && r.text == "you"));
        assert!(rows.iter().any(|r| r.is_label && r.text == "assistant"));
    }

    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn splash_shows_mark_and_wordmark() {
        let theme = Theme::current();
        let tall: Vec<String> = splash_lines(&theme, true).iter().map(line_text).collect();
        assert!(tall.iter().any(|t| t.contains(MARK)), "{tall:?}");
        assert!(
            tall.iter().any(|t| t.replace(' ', "").contains(WORDMARK)),
            "{tall:?}"
        );
        let compact: Vec<String> = splash_lines(&theme, false).iter().map(line_text).collect();
        assert!(compact.iter().any(|t| t.contains(WORDMARK)));
    }
}
