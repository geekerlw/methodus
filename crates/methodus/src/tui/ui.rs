use std::cell::RefCell;

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use methodus_core::PermissionMode;

use super::app::{
    ellipsize, help_line, matching_slash, slash_menu_open, status_counts, App, ChatKind, ChatLine,
    Mode, Overlay, PromptKind, ReviewItem, SetupSection, StatusLevel, APPROVAL_CHOICES,
    INBOX_EXPERIENCE_CHOICES, INBOX_EVOLUTION_CHOICES, INBOX_QUESTION_CHOICES,
};
use super::md::{render_md, MdLine, MdStyle};
use super::util::truncate_display;

/// Facet mark: a diamond with a center — Methodus holds the method, not the work.
const MARK: &str = "◈";
const WORDMARK: &str = "Methodus";

// ─── Layout cache ────────────────────────────────────────────────────────────
// layout_chat is expensive (markdown parsing + word wrap for every line).
// Cache the result and only recompute when transcript content or width changes.
thread_local! {
    static LAYOUT_CACHE: RefCell<LayoutCache> = RefCell::new(LayoutCache::default());
    static INBOX_MD_CACHE: RefCell<InboxMdCache> = RefCell::new(InboxMdCache::default());
}

#[derive(Default)]
struct LayoutCache {
    version: u64,
    width: usize,
    rows: Vec<ChatRow>,
}

/// Cached markdown layout for inbox detail (same idea as LAYOUT_CACHE).
/// Store wrapped MdLine rows; widgetize only the visible slice each frame.
#[derive(Default)]
struct InboxMdCache {
    /// The review_sel index when we last computed.
    review_sel: usize,
    /// Number of inbox items at cache time (detects list changes).
    inbox_count: usize,
    width: usize,
    lines: Vec<MdLine>,
}

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
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(area);

    draw_header(frame, chunks[0], app, &theme);
    draw_work(frame, chunks[1], app, &theme);
    if app.overlay.is_open() && !app.inbox_detail_open() {
        draw_overlay(frame, chunks[1], app, &theme);
    }
    draw_footer(frame, chunks[2], app, &theme);

    if app.mode == Mode::Prompt {
        draw_prompt_modal(frame, area, app, &theme);
    }
    if app.show_help {
        draw_help_overlay(frame, area, &theme);
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let runtime = app.runtime.as_deref().unwrap_or("claude-code");
    let face = app.default_face.as_deref().unwrap_or("auto");
    let wait_mark = if app.approvals.is_empty() {
        String::new()
    } else {
        format!("  !{}", app.approvals.len())
    };
    let review_n = app.questions.len() + app.knowledge.len() + app.hypotheses.len() + app.evolutions.len();
    let review_mark = if review_n == 0 {
        String::new()
    } else {
        format!("  ▣{review_n}")
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
    let overlay_mark = match app.overlay {
        Overlay::Setup => "  setup",
        Overlay::Inbox => "  inbox",
        Overlay::Faces => "  faces",
        Overlay::Sessions => "  sessions",
        Overlay::None => "",
    };
    let session = app
        .session_task_id
        .as_deref()
        .map(|id| format!("  ·  {id}"))
        .unwrap_or_default();
    let line = Line::from(vec![
        Span::styled(format!(" {MARK} "), theme.label()),
        Span::styled(WORDMARK, theme.label()),
        Span::styled(
            format!("  {runtime} · {face}{session}"),
            theme.dim(),
        ),
        Span::styled(wait_mark, Style::default().fg(theme.warning)),
        Span::styled(review_mark, Style::default().fg(theme.permission)),
        Span::styled(overlay_mark.to_string(), theme.label()),
        Span::styled(format!("{rec}{usage}"), theme.dim()),
    ]);
    // Agent-style: status strip, no chrome box.
    frame.render_widget(Paragraph::new(line).style(theme.text()), area);
}

fn draw_overlay(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    match app.overlay {
        Overlay::Sessions => draw_sessions(frame, area, app, theme),
        Overlay::Faces => draw_faces(frame, area, app, theme),
        Overlay::Inbox => draw_review(frame, area, app, theme),
        Overlay::Setup => {
            frame.render_widget(Clear, area);
            draw_setup(frame, area, app, theme);
        }
        Overlay::None => {}
    }
}

/// Floating card over the live transcript (Pi-style overlay: clear popup only).
fn floating_popup(area: Rect, percent_x: u16, height_pct: u16) -> Rect {
    let popup_h = (area.height.saturating_mul(height_pct) / 100).clamp(10, area.height);
    centered(area, percent_x, popup_h)
}

fn draw_work(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let session_area = area;

    let composer_h = composer_height(app, area.width);
    let picking = app.mode == Mode::Answering
        || app.pending_approval().is_some()
        || app.pending_knowledge().is_some()
        || app.inbox_question_menu()
        || app.inbox_experience_menu();
    let slash_open = !picking && slash_menu_open(&app.input);
    let mention_open = !picking && app.mention_menu_open();
    let slash_h = if slash_open {
        let n = matching_slash(&app.input).len().max(1);
        (n as u16 + 1).min(7)
    } else {
        0
    };
    let mention_h = if mention_open {
        let n = app.matching_mentions().len().max(1);
        (n as u16 + 1).min(9)
    } else {
        0
    };
    let mut parts = vec![Constraint::Min(3)];
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
        .split(session_area);

    if app.inbox_detail_open() {
        draw_inbox_detail(frame, rows[0], app, theme);
    } else {
        draw_transcript(frame, rows[0], app, theme, true);
    }
    let mut i = 1;
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

fn composer_height(app: &App, width: u16) -> u16 {
    // Rounded box: top + bottom borders. Inner width accounts for borders + padding.
    let inner = composer_inner_width(width);
    if app.inbox_detail_open() {
        if app.mode == Mode::Answering {
            let extra = if app
                .answering_question()
                .and_then(|q| q.reason.as_deref())
                .filter(|r| !r.is_empty())
                .is_some()
            {
                1
            } else {
                0
            };
            let err = if app.input_error.is_some() { 1 } else { 0 };
            let rows = composer_view(&app.input, app.input_cursor, inner, COMPOSER_MAX_ROWS)
                .0
                .len()
                .max(1);
            return (2 + 1 + extra + err + rows as u16).min(14);
        }
        if app.pending_knowledge().is_some() {
            let n = app.knowledge_pick_choices().len();
            return (2 + 2 + n as u16).min(10);
        }
        if app.pending_hypothesis().is_some() {
            let n = app.hypothesis_pick_choices().len();
            return (2 + 2 + n as u16).min(10);
        }
        if app.inbox_question_menu() {
            return (2 + 2 + INBOX_QUESTION_CHOICES.len() as u16).min(10);
        }
        if app.inbox_experience_menu() {
            return (2 + 2 + INBOX_EXPERIENCE_CHOICES.len() as u16).min(8);
        }
    }
    if app.mode == Mode::Answering {
        let extra = if app
            .answering_question()
            .and_then(|q| q.reason.as_deref())
            .filter(|r| !r.is_empty())
            .is_some()
        {
            1
        } else {
            0
        };
        let err = if app.input_error.is_some() { 1 } else { 0 };
        let rows = composer_view(&app.input, app.input_cursor, inner, COMPOSER_MAX_ROWS)
            .0
            .len()
            .max(1);
        return (2 + 1 + extra + err + rows as u16).min(14);
    }
    if app.pending_approval().is_some() {
        return 8;
    }
    if app.pending_knowledge().is_some() {
        return 11;
    }
    let rows = composer_view(&app.input, app.input_cursor, inner, COMPOSER_MAX_ROWS)
        .0
        .len()
        .max(1);
    (2 + rows as u16).clamp(3, 12)
}

/// Text columns inside the rounded input well (L/R border + 1-col pad).
fn composer_inner_width(area_width: u16) -> usize {
    area_width.saturating_sub(4).max(8) as usize
}

const COMPOSER_MAX_ROWS: usize = 8;

fn draw_slash_menu(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let matches = matching_slash(&app.input);
    let items: Vec<ListItem> = if matches.is_empty() {
        vec![ListItem::new("  no matching command").style(theme.dim())]
    } else {
        matches
            .iter()
            .map(|cmd| {
                let alias = if cmd.aliases.is_empty() {
                    String::new()
                } else {
                    format!("  /{}", cmd.aliases.join(" /"))
                };
                ListItem::new(format!("  /{}{alias}  {}", cmd.name, cmd.summary)).style(theme.text())
            })
            .collect()
    };
    let mut state = ListState::default();
    if !matches.is_empty() {
        state.select(Some(app.slash_sel));
    }
    frame.render_stateful_widget(
        List::new(items)
            .block(hairline(" /  ↑↓ j k · tab complete · enter run ", theme.accent_border()))
            .style(theme.text())
            .highlight_style(theme.selected())
            .highlight_symbol("> "),
        area,
        &mut state,
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
            .map(|cand| {
                let kind = if cand.is_dir { "dir " } else { "file" };
                let style = if cand.is_dir {
                    Style::default().fg(theme.info)
                } else {
                    theme.text()
                };
                ListItem::new(format!("  {kind}  {}", cand.label)).style(style)
            })
            .collect()
    };
    let mut state = ListState::default();
    if !matches.is_empty() {
        state.select(Some(app.mention_sel));
    }
    frame.render_stateful_widget(
        List::new(items)
            .block(hairline(" @  ↑↓ j k · tab drill · enter attach ", theme.accent_border()))
            .style(theme.text())
            .highlight_style(theme.selected())
            .highlight_symbol("> "),
        area,
        &mut state,
    );
}

fn draw_transcript(frame: &mut Frame, area: Rect, app: &App, theme: &Theme, _focused: bool) {
    let inner_w = area.width as usize;
    let inner_h = area.height as usize;
    let layout_w = inner_w.max(8);

    // Use cached layout if transcript hasn't changed and width is the same.
    let rows = LAYOUT_CACHE.with(|cache| {
        let mut c = cache.borrow_mut();
        if c.version == app.transcript_version && c.width == layout_w {
            c.rows.clone()
        } else {
            let computed = layout_chat(&app.transcript, layout_w);
            c.version = app.transcript_version;
            c.width = layout_w;
            c.rows = computed.clone();
            computed
        }
    });

    let visible: Vec<Line> = if rows.is_empty() {
        splash_lines(theme, inner_h >= 10)
    } else {
        let max_off = rows.len().saturating_sub(inner_h.max(1));
        let offset = app.transcript_offset.min(max_off);
        let end = rows.len().saturating_sub(offset);
        let start = end.saturating_sub(inner_h.max(1));
        rows[start..end]
            .iter()
            .map(|row| chat_line(row, theme))
            .collect()
    };
    // Full-bleed transcript — no panel. Agent CLIs keep chrome on the input only.
    frame.render_widget(
        Paragraph::new(visible)
            .alignment(if app.transcript.is_empty() {
                Alignment::Center
            } else {
                Alignment::Left
            })
            .style(theme.text()),
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
    spans: Vec<(String, MdStyle)>,
    is_label: bool,
}

fn chat_row(kind: ChatKind, pad: usize, text: String, is_label: bool) -> ChatRow {
    ChatRow {
        kind,
        pad,
        text,
        spans: Vec::new(),
        is_label,
    }
}

fn chat_row_md(kind: ChatKind, line: MdLine) -> ChatRow {
    let text = line.spans.iter().map(|s| s.text.as_str()).collect();
    ChatRow {
        kind,
        pad: line.pad,
        text,
        spans: line
            .spans
            .into_iter()
            .map(|s| (s.text, s.style))
            .collect(),
        is_label: false,
    }
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
            out.push(chat_row(kind, 0, String::new(), false));
        }
        match kind {
            ChatKind::You => {
                push_label(&mut out, kind, "you", width, true);
                for entry in group {
                    for wrapped in wrap_text(&entry.text, width) {
                        out.push(chat_row(kind, 0, wrapped, false));
                    }
                }
            }
            ChatKind::Assistant => {
                push_label(&mut out, kind, "assistant", width, false);
                let joined = group
                    .iter()
                    .map(|e| e.text.as_str())
                    .collect::<Vec<_>>()
                    .join("");
                for md in render_md(&joined, width) {
                    out.push(chat_row_md(kind, md));
                }
            }
            _ => {
                for entry in group {
                    for wrapped in wrap_text(&entry.text, width) {
                        out.push(chat_row(kind, 0, wrapped, false));
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
        width.saturating_sub(display_width(label))
    } else {
        0
    };
    out.push(chat_row(kind, pad, label.to_string(), true));
}

/// Wrap by terminal columns (CJK / fullwidth = 2), not by Unicode scalar count.
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
        let mut line = String::new();
        let mut cols = 0usize;
        for ch in raw.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            if cols > 0 && cols + w > width {
                out.push(std::mem::take(&mut line));
                cols = 0;
            }
            line.push(ch);
            cols = cols.saturating_add(w);
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn chat_line(row: &ChatRow, theme: &Theme) -> Line<'static> {
    let mut style = match row.kind {
        ChatKind::You | ChatKind::Assistant => theme.text(),
        ChatKind::Tool => Style::default().fg(theme.info),
        ChatKind::Meta => theme.dim(),
        ChatKind::Alert => Style::default().fg(theme.warning),
    };
    if row.is_label {
        style = match row.kind {
            ChatKind::Assistant => theme.label(),
            ChatKind::You => theme.dim().add_modifier(Modifier::BOLD),
            _ => style.add_modifier(Modifier::BOLD),
        };
    }
    let pad = " ".repeat(row.pad);
    if !row.spans.is_empty() && !row.is_label {
        let mut spans = vec![Span::raw(pad)];
        for (text, md) in &row.spans {
            spans.push(Span::styled(text.clone(), md_style(*md, theme)));
        }
        return Line::from(spans);
    }
    if row.kind == ChatKind::Tool && !row.is_label {
        if row.text.starts_with('+') {
            style = Style::default().fg(theme.success);
        } else if row.text.starts_with('-') {
            style = Style::default().fg(theme.error);
        }
    }
    Line::from(vec![Span::raw(pad), Span::styled(row.text.clone(), style)])
}

fn md_style(style: MdStyle, theme: &Theme) -> Style {
    match style {
        MdStyle::Body => theme.text(),
        MdStyle::Dim => theme.dim(),
        MdStyle::Bold => theme.text().add_modifier(Modifier::BOLD),
        MdStyle::Italic => theme.text().add_modifier(Modifier::ITALIC),
        MdStyle::Heading => theme.label(),
        MdStyle::Code => Style::default().fg(theme.info),
        MdStyle::Fence => theme.dim(),
        MdStyle::DiffAdd => Style::default().fg(theme.success),
        MdStyle::DiffDel => Style::default().fg(theme.error),
    }
}

fn draw_composer(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if app.inbox_question_menu() {
        draw_inbox_question_menu(frame, area, app, theme);
        return;
    }
    if app.inbox_experience_menu() {
        draw_inbox_experience_menu(frame, area, app, theme);
        return;
    }
    if app.inbox_evolution_menu() {
        draw_inbox_evolution_menu(frame, area, app, theme);
        return;
    }
    if app.pending_hypothesis().is_some() {
        draw_hypothesis_prompt(frame, area, app, theme);
        return;
    }
    if app.mode == Mode::Answering {
        draw_question_prompt(frame, area, app, theme);
        return;
    }
    if app.pending_approval().is_some() {
        draw_permission_prompt(frame, area, app, theme);
        return;
    }
    if app.pending_knowledge().is_some() {
        draw_knowledge_prompt(frame, area, app, theme);
        return;
    }
    if app.pending_evolution().is_some() {
        draw_evolution_prompt(frame, area, app, theme);
        return;
    }
    let err = app.input_error.as_deref();
    let mode = PermissionMode::parse(Some(app.permission_mode.as_str()));
    let inner_w = composer_inner_width(area.width);
    let empty = app.input.is_empty();
    let (shown, cursor_col, cursor_row) = if empty {
        (Vec::new(), 2u16, 0u16)
    } else {
        composer_view(&app.input, app.input_cursor, inner_w, COMPOSER_MAX_ROWS)
    };
    let lines: Vec<Line> = if empty {
        let hint = if app.busy() {
            "waiting for this turn…"
        } else {
            "type a message"
        };
        vec![Line::from(vec![
            Span::styled("> ", theme.dim()),
            Span::styled(hint, theme.dim()),
        ])]
    } else {
        shown.into_iter().map(Line::from).collect()
    };
    let border = if err.is_some() {
        Style::default().fg(theme.error)
    } else {
        Style::default().fg(theme.mode_border(mode))
    };
    let caption = if let Some(e) = err {
        Line::from(Span::styled(format!(" {e} "), Style::default().fg(theme.error)))
    } else if app.busy() {
        Line::from(Span::styled(" wait ", theme.dim()))
    } else {
        Line::from(Span::styled(format!(" {} ", mode.label()), theme.dim())).right_aligned()
    };
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.text())
            .block(input_well(theme, border, caption)),
        area,
    );
    // border + horizontal padding
    let x = area.x.saturating_add(2).saturating_add(cursor_col);
    let y = area.y.saturating_add(1).saturating_add(cursor_row);
    if x < area.x.saturating_add(area.width.saturating_sub(1))
        && y < area.y.saturating_add(area.height.saturating_sub(1))
    {
        frame.set_cursor_position(Position { x, y });
    }
}

/// Claude Code-style input well: rounded box, mode on the bottom rule.
fn input_well<'a>(theme: &'a Theme, border: Style, bottom: Line<'a>) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border)
        .padding(Padding::horizontal(1))
        .title_bottom(bottom)
        .style(theme.text())
}

fn draw_permission_prompt(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let Some(appr) = app.pending_approval() else {
        return;
    };
    let n = app.session_approvals().len();
    let idx = app.approval_sel.min(n.saturating_sub(1)) + 1;
    let title = if n > 1 {
        format!(" permission · {} · {idx}/{n} · ↑↓ enter ", appr.tool_name)
    } else {
        format!(" permission · {} · ↑↓ enter ", appr.tool_name)
    };
    let detail = ellipsize(
        &crate::tui::util::summarize_tool_input(&appr.tool_input),
        72,
    );
    let header = vec![Line::from(Span::styled(
        format!("Allow `{detail}`?"),
        Style::default().fg(theme.warning),
    ))];
    let choices: Vec<(&str, &str)> = APPROVAL_CHOICES
        .iter()
        .map(|c| (c.key, c.label))
        .collect();
    draw_select_list(
        frame,
        area,
        &title,
        &header,
        &choices,
        app.approval_choice,
        Style::default().fg(theme.permission),
        theme,
    );
}

fn draw_knowledge_prompt(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let Some(k) = app.pending_knowledge() else {
        return;
    };
    let kind = methodus_core::learning::knowledge_inbox_label(&k.source);
    let mut header = vec![
        Line::from(Span::styled(
            format!("Commit this {kind}?"),
            Style::default().fg(theme.warning),
        )),
        Line::from(Span::styled(ellipsize(&k.path, 88), theme.dim())),
    ];
    if !app.inbox_detail_open() {
        let preview = app.knowledge_preview();
        if !preview.is_empty() {
            for md in render_md(&preview, 88).into_iter().take(3) {
                header.push(md_line_widget(&md, theme));
            }
        }
    }
    let choices: Vec<(&str, &str)> = app
        .knowledge_pick_choices()
        .iter()
        .map(|c| (c.key, c.label))
        .collect();
    draw_select_list(
        frame,
        area,
        &format!(" {kind} · ↑↓ enter · y/d · esc "),
        &header,
        &choices,
        app.knowledge_choice,
        pick_border(app, theme, theme.permission),
        theme,
    );
}

fn draw_inbox_question_menu(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let header = vec![Line::from(Span::styled(
        "What would you like to do?",
        Style::default().fg(theme.warning),
    ))];
    draw_select_list(
        frame,
        area,
        " question · ↑↓ enter · esc list ",
        &header,
        INBOX_QUESTION_CHOICES,
        app.inbox_menu_choice,
        pick_border(app, theme, theme.warning),
        theme,
    );
}

fn draw_inbox_experience_menu(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let header = vec![Line::from(Span::styled(
        "Mark this experience review complete?",
        Style::default().fg(theme.muted),
    ))];
    draw_select_list(
        frame,
        area,
        " experience · ↑↓ enter · esc list ",
        &header,
        INBOX_EXPERIENCE_CHOICES,
        app.inbox_menu_choice,
        pick_border(app, theme, theme.muted),
        theme,
    );
}

fn draw_inbox_evolution_menu(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let face = app
        .pending_evolution()
        .map(|e| e.target_id.as_str())
        .unwrap_or("face");
    let header = vec![Line::from(Span::styled(
        format!("Apply proposed face.yaml updates to `{face}`?"),
        Style::default().fg(theme.warning),
    ))];
    draw_select_list(
        frame,
        area,
        " face evolution · ↑↓ enter · esc list ",
        &header,
        INBOX_EVOLUTION_CHOICES,
        app.inbox_menu_choice,
        pick_border(app, theme, theme.permission),
        theme,
    );
}

fn draw_evolution_prompt(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let Some(e) = app.pending_evolution() else {
        return;
    };
    let header = vec![
        Line::from(Span::styled(
            format!("Apply face.yaml updates to `{}`?", e.target_id),
            Style::default().fg(theme.warning),
        )),
        Line::from(Span::styled(
            ellipsize(e.rationale.as_deref().unwrap_or("-"), 88),
            theme.dim(),
        )),
    ];
    let choices: Vec<(&str, &str)> = app
        .evolution_pick_choices()
        .iter()
        .copied()
        .collect();
    draw_select_list(
        frame,
        area,
        " face evolution · ↑↓ enter · y/d · esc ",
        &header,
        &choices,
        app.evolution_choice,
        pick_border(app, theme, theme.permission),
        theme,
    );
}

fn draw_hypothesis_prompt(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let Some(h) = app.pending_hypothesis() else {
        return;
    };
    let header = vec![
        Line::from(Span::styled(
            "What should happen to this hypothesis?",
            Style::default().fg(theme.warning),
        )),
        Line::from(Span::styled(ellipsize(&h.path, 88), theme.dim())),
    ];
    let choices: Vec<(&str, &str)> = app
        .hypothesis_pick_choices()
        .iter()
        .copied()
        .collect();
    draw_select_list(
        frame,
        area,
        " hypothesis · ↑↓ enter · y/v/d · esc ",
        &header,
        &choices,
        app.hypothesis_choice,
        pick_border(app, theme, theme.permission),
        theme,
    );
}

/// Decision list is muted while the detail body owns ↑↓.
fn pick_border(app: &App, theme: &Theme, active: Color) -> Style {
    if app.inbox_detail_open() && app.inbox_detail_focus_body {
        theme.muted_border()
    } else {
        Style::default().fg(active)
    }
}

/// Pi-style SelectList in the composer: header lines + numbered choices.
fn draw_select_list(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    header: &[Line],
    choices: &[(&str, &str)],
    selected: usize,
    border: Style,
    theme: &Theme,
) {
    let mut lines: Vec<Line> = header.to_vec();
    for (i, (key, label)) in choices.iter().enumerate() {
        let is_sel = i == selected;
        let mark = if is_sel { ">" } else { " " };
        let style = if is_sel {
            theme.selected()
        } else {
            theme.text()
        };
        lines.push(Line::from(Span::styled(
            format!("{mark} {key}. {label}"),
            style,
        )));
    }
    frame.render_widget(
        Paragraph::new(lines).style(theme.text()).block(
            input_well(
                theme,
                border,
                Line::from(Span::styled(title.to_string(), theme.dim())),
            ),
        ),
        area,
    );
}

fn draw_question_prompt(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let q = app.answering_question();
    let prompt = q
        .map(|q| q.question.as_str())
        .unwrap_or("a question");
    let reason = q.and_then(|q| q.reason.as_deref()).unwrap_or("");
    let err = app.input_error.as_deref().unwrap_or("");
    let inner_w = composer_inner_width(area.width);
    let empty = app.input.is_empty();
    let (shown, cursor_col, cursor_row) = if empty {
        (Vec::new(), 2u16, 0u16)
    } else {
        composer_view(&app.input, app.input_cursor, inner_w, COMPOSER_MAX_ROWS)
    };
    let mut lines = Vec::new();
    if !app.inbox_detail_open() {
        lines.push(Line::from(Span::styled(
            ellipsize(prompt, inner_w.saturating_sub(1)),
            Style::default().fg(theme.warning),
        )));
        if !reason.is_empty() {
            lines.push(Line::from(Span::styled(
                ellipsize(reason, inner_w.saturating_sub(1)),
                theme.dim(),
            )));
        }
    } else {
        lines.push(Line::from(Span::styled("Your answer", Style::default().fg(theme.warning))));
    }
    if empty {
        lines.push(Line::from(vec![
            Span::styled("> ", theme.dim()),
            Span::styled("type an answer", theme.dim()),
        ]));
    } else {
        lines.extend(shown.into_iter().map(Line::from));
    }
    if !err.is_empty() {
        lines.push(Line::from(Span::styled(
            err,
            Style::default().fg(theme.error),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines).style(theme.text()).block(input_well(
            theme,
            Style::default().fg(theme.permission),
            Line::from(Span::styled(
                " enter submit · shift-enter newline · esc later ",
                theme.dim(),
            )),
        )),
        area,
    );
    let header = 1 + if reason.is_empty() { 0 } else { 1 };
    let x = area.x.saturating_add(2).saturating_add(cursor_col);
    let y = area.y.saturating_add(1 + header).saturating_add(cursor_row);
    if x < area.x.saturating_add(area.width.saturating_sub(1))
        && y < area.y.saturating_add(area.height.saturating_sub(1))
    {
        frame.set_cursor_position(Position { x, y });
    }
}

/// Fit `input` into `width` terminal columns; return shown text and cursor column
/// after the `"> "` prefix. CJK / fullwidth chars count as 2 columns.
#[cfg(test)]
fn visible_input(input: &str, width: usize) -> (String, u16) {
    let total = UnicodeWidthStr::width(input);
    if total <= width {
        return (input.to_string(), 2 + total as u16);
    }
    let mut cols = 0usize;
    let mut start = input.len();
    for (i, ch) in input.char_indices().rev() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cols + w > width {
            break;
        }
        cols += w;
        start = i;
    }
    (input[start..].to_string(), 2 + cols as u16)
}

/// Multiline composer: prefix first visual row with `> `, wrap by display columns,
/// keep the hardware cursor on the real glyph (CJK width 2).
fn composer_view(input: &str, cursor: usize, width: usize, max_rows: usize) -> (Vec<String>, u16, u16) {
    use super::util::floor_char_boundary;

    let mut cursor = cursor.min(input.len());
    while cursor > 0 && !input.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let inner = width.saturating_sub(2).max(1);
    let mut vis: Vec<String> = Vec::new();
    let mut cur_row = 0u16;
    let mut cur_col = 2u16;
    let mut found_cursor = false;

    let lines: Vec<&str> = if input.is_empty() {
        vec![""]
    } else {
        input.split('\n').collect()
    };
    let nlines = lines.len();
    let mut global = 0usize;

    for (li, raw) in lines.iter().enumerate() {
        let line_start = global;
        let wrapped = wrap_text(raw, inner);
        let mut local = 0usize;

        for (wi, piece) in wrapped.iter().enumerate() {
            let prefix = if li == 0 && wi == 0 { "> " } else { "  " };
            vis.push(format!("{prefix}{piece}"));

            let pstart = line_start + local;
            let pend = pstart + piece.len();
            if !found_cursor && cursor >= pstart && cursor <= pend {
                found_cursor = true;
                cur_row = (vis.len() - 1) as u16;
                let off = floor_char_boundary(piece, cursor.saturating_sub(pstart));
                cur_col = 2 + display_cols(&piece[..off]);
            }
            local += piece.len();
        }

        global = line_start + raw.len();
        if li + 1 < nlines {
            if !found_cursor && cursor == global {
                found_cursor = true;
                cur_row = vis.len().saturating_sub(1) as u16;
                cur_col = vis
                    .last()
                    .map(|s| display_cols(s))
                    .unwrap_or(2);
            }
            global += 1;
        }
    }

    if vis.is_empty() {
        vis.push("> ".to_string());
    }
    let max_rows = max_rows.max(1);
    if vis.len() > max_rows {
        let row = cur_row as usize;
        let start = row.saturating_add(1).saturating_sub(max_rows);
        let shown = vis[start..start + max_rows].to_vec();
        return (shown, cur_col, (row - start) as u16);
    }
    (vis, cur_col, cur_row)
}

fn overlay_filter_title(base: &str, filter: &str, matches: usize, total: usize) -> String {
    if filter.is_empty() {
        format!(" {base}  ·  type to filter  ·  esc ")
    } else {
        format!(" {base}  ·  {filter}  ·  {matches}/{total}  ·  esc ")
    }
}

fn md_line_widget(line: &MdLine, theme: &Theme) -> Line<'static> {
    if line.spans.is_empty() {
        return Line::from("");
    }
    let pad = " ".repeat(line.pad);
    let mut spans = vec![Span::raw(pad)];
    for s in &line.spans {
        spans.push(Span::styled(s.text.clone(), md_style(s.style, theme)));
    }
    Line::from(spans)
}

fn display_cols(s: &str) -> u16 {
    display_width(s) as u16
}

fn draw_sessions(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let popup = floating_popup(area, 78, 70);
    frame.render_widget(Clear, popup);

    let (queued, running, waiting, done) = status_counts(&app.tasks);
    let vis = app.visible_task_indices();
    let title = overlay_filter_title(
        &format!("conversations  q{queued} r{running} w{waiting} d{done}"),
        &app.overlay_filter,
        vis.len(),
        app.tasks.len(),
    );
    let block = panel(&title, true, theme);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if app.tasks.is_empty() {
        frame.render_widget(
            Paragraph::new("no conversations yet\n\nesc, then type a message to start")
                .style(theme.dim())
                .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(7)])
        .split(inner);

    let list_w = rows[0].width.saturating_sub(1) as usize;
    let title_w = list_w.saturating_sub(14);
    let vis = app.visible_task_indices();
    let items: Vec<ListItem> = if vis.is_empty() {
        vec![ListItem::new("  no matches").style(theme.dim())]
    } else {
        vis.into_iter()
            .map(|i| {
                let t = &app.tasks[i];
                let mark = if i == app.task_sel { ">" } else { " " };
                let selected = i == app.task_sel;
                let status_style = status_style(&t.status, theme);
                let title_style = if selected {
                    theme.selected()
                } else {
                    theme.text()
                };
                let st = compact_task_status(&t.status);
                let line = Line::from(vec![
                    Span::styled(format!("{mark} "), title_style),
                    Span::styled(format!("{st:<7}"), status_style),
                    Span::styled(
                        format!(" {}", ellipsize(&t.title, title_w.max(4))),
                        title_style,
                    ),
                ]);
                ListItem::new(line)
            })
            .collect()
    };
    frame.render_widget(List::new(items).style(theme.text()), rows[0]);

    let detail = app.selected_task_detail();
    frame.render_widget(
        Paragraph::new(detail)
            .style(theme.dim())
            .wrap(Wrap { trim: true })
            .block(hairline(" preview ", theme.muted_border())),
        rows[1],
    );
}

fn compact_task_status(status: &methodus_domain::TaskStatus) -> &'static str {
    use methodus_domain::TaskStatus::*;
    match status {
        Queued => "queued",
        Planning => "plan",
        Running => "run",
        WaitingUser => "wait",
        Reviewing => "review",
        Completed => "done",
        Failed => "fail",
        Cancelled => "cancel",
    }
}

fn draw_faces(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let popup = floating_popup(area, 72, 60);
    frame.render_widget(Clear, popup);
    let vis = app.visible_face_indices();
    let title = overlay_filter_title("faces", &app.overlay_filter, vis.len(), app.faces.len());
    let block = panel(&title, true, theme);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if app.faces.is_empty() {
        frame.render_widget(
            Paragraph::new("no faces yet")
                .style(theme.dim())
                .alignment(Alignment::Center),
            inner,
        );
        return;
    }
    let vis = app.visible_face_indices();
    let items: Vec<ListItem> = if vis.is_empty() {
        vec![ListItem::new("  no matches").style(theme.dim())]
    } else {
        vis.into_iter()
            .map(|i| {
                let f = &app.faces[i];
                let mark = if i == app.face_sel { ">" } else { " " };
                let pin = if app.default_face.as_deref() == Some(f.id.as_str()) {
                    "  [default]"
                } else {
                    ""
                };
                let style = if i == app.face_sel {
                    theme.selected()
                } else {
                    theme.text()
                };
                ListItem::new(format!(
                    "{mark} {} — {}{pin}\n    [{}] {}",
                    f.id,
                    f.name,
                    f.source,
                    ellipsize(&f.description, 56)
                ))
                .style(style)
            })
            .collect()
    };
    frame.render_widget(List::new(items).style(theme.text()), inner);
}

fn inbox_knowledge_title(source: &str) -> &'static str {
    match methodus_core::learning::knowledge_inbox_label(source) {
        "skill draft" => " skill draft ",
        "skill patch" => " skill patch ",
        "harness note" => " harness note ",
        other => {
            let _ = other;
            " knowledge "
        }
    }
}

fn draw_review(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let popup = floating_popup(area, 82, 72);
    frame.render_widget(Clear, popup);
    let vis = app.visible_review_indices();
    let total = app.review_total();
    let title = overlay_filter_title("inbox", &app.overlay_filter, vis.len(), total);
    let block = panel(&title, true, theme);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let empty = app.questions.is_empty()
        && app.knowledge.is_empty()
        && app.hypotheses.is_empty()
        && app.evolutions.is_empty()
        && app.experiences.is_empty();
    if empty {
        frame.render_widget(
            Paragraph::new(
                "inbox is empty\n\n\
                 after a turn, experience may land here.\n\
                 /learn reads your sources and archives into project or Face knowledge.\n\
                 Outcomes land in /inbox for review — no separate jobs dashboard.\n\
                 skill drafts, patches, and Face notes appear here after non-trivial tasks.\n\
                 hypotheses appear when curiosity finds under-evidenced claims.\n\
                 face evolution proposals appear after study knowledge is committed.\n\
                 idle questions use the composer: type an answer, Enter.",
            )
            .style(theme.dim())
            .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(18)])
        .split(inner);

    let vis = app.visible_review_indices();
    let mut items: Vec<ListItem> = Vec::new();
    if vis.is_empty() {
        items.push(ListItem::new("  no matches").style(theme.dim()));
    }
    for idx in vis {
        let mark = if idx == app.review_sel { ">" } else { " " };
        if idx < app.questions.len() {
            let q = &app.questions[idx];
            let style = sel_style(idx == app.review_sel, theme.warning, theme);
            items.push(
                ListItem::new(format!(
                    "{mark} Q  {:<8}  {}",
                    q.status,
                    ellipsize(&q.question, 18)
                ))
                .style(style),
            );
            continue;
        }
        let j = idx - app.questions.len();
        if j < app.knowledge.len() {
            let k = &app.knowledge[j];
            let kind = methodus_core::learning::knowledge_inbox_tag(&k.source);
            let style = sel_style(idx == app.review_sel, theme.fg, theme);
            items.push(
                ListItem::new(format!(
                    "{mark} {kind}  {:<8}  {}",
                    k.status,
                    ellipsize(&k.path, 18)
                ))
                .style(style),
            );
            continue;
        }
        let k = j - app.knowledge.len();
        if k < app.hypotheses.len() {
            let h = &app.hypotheses[k];
            let style = sel_style(idx == app.review_sel, theme.warning, theme);
            items.push(
                ListItem::new(format!(
                    "{mark} H  {:<8}  {}",
                    h.status,
                    ellipsize(&h.path, 18)
                ))
                .style(style),
            );
            continue;
        }
        let e = k - app.hypotheses.len();
        if e < app.evolutions.len() {
            let ev = &app.evolutions[e];
            let style = sel_style(idx == app.review_sel, theme.info, theme);
            items.push(
                ListItem::new(format!(
                    "{mark} F  {:<8}  {}",
                    ev.status,
                    ellipsize(&format!("face:{}", ev.target_id), 18)
                ))
                .style(style),
            );
            continue;
        }
        if let Some(exp) = app.experiences.get(e - app.evolutions.len()) {
            let style = sel_style(idx == app.review_sel, theme.muted, theme);
            let outcome = exp.outcome.as_deref().unwrap_or("-");
            items.push(
                ListItem::new(format!(
                    "{mark} E  {:<8}  {}",
                    outcome,
                    ellipsize(exp.summary.as_deref().unwrap_or(&exp.id), 18)
                ))
                .style(style),
            );
        }
    }
    frame.render_widget(List::new(items).style(theme.text()), cols[0]);

    let body = app.review_summary();
    let title = match app.selected_review() {
        Some(ReviewItem::Question(_)) => " summary ",
        Some(ReviewItem::Knowledge(k)) => inbox_knowledge_title(&k.source),
        Some(ReviewItem::Hypothesis(_)) => " hypothesis ",
        Some(ReviewItem::Evolution(_)) => " face evolution ",
        Some(ReviewItem::Experience(_)) => " experience ",
        None => " summary ",
    };
    let preview_w = cols[1].width.saturating_sub(2) as usize;
    let md_lines: Vec<Line> = render_md(&body, preview_w.max(16))
        .iter()
        .map(|l| md_line_widget(l, theme))
        .collect();
    frame.render_widget(
        Paragraph::new(md_lines)
            .style(theme.text())
            .block(hairline(title, theme.muted_border())),
        cols[1],
    );
}

fn draw_inbox_detail(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let title = match app.selected_review() {
        Some(ReviewItem::Question(_)) => " question ",
        Some(ReviewItem::Knowledge(k)) => inbox_knowledge_title(&k.source),
        Some(ReviewItem::Hypothesis(_)) => " hypothesis ",
        Some(ReviewItem::Evolution(_)) => " face evolution ",
        Some(ReviewItem::Experience(_)) => " experience ",
        None => " inbox ",
    };
    let panel_title = if matches!(app.selected_review(), Some(ReviewItem::Evolution(_))) {
        format!("inbox · face `{}`", 
            app.selected_review()
                .and_then(|r| match r { ReviewItem::Evolution(e) => Some(e.target_id.as_str()), _ => None })
                .unwrap_or(""))
    } else {
        format!("inbox · {title}")
    };
    let block = panel(&panel_title, app.inbox_detail_focus_body, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let preview_w = inner.width.saturating_sub(2) as usize;
    let md_w = preview_w.max(16);
    // hairline is TOP-only (1 row), not a full box.
    let inner_h = inner.height.saturating_sub(1) as usize;

    // Cache wrapped markdown (parse + wrap). Widgetize only the viewport,
    // matching transcript: LAYOUT_CACHE holds rows, draw maps start..end.
    let inbox_count = app.knowledge.len() + app.questions.len()
        + app.hypotheses.len() + app.evolutions.len() + app.experiences.len();
    let (visible, scroll, max_scroll) = INBOX_MD_CACHE.with(|cache| {
        let mut c = cache.borrow_mut();
        if c.review_sel != app.review_sel || c.inbox_count != inbox_count || c.width != md_w {
            let body = app.review_detail();
            c.lines = if body.is_empty() {
                Vec::new()
            } else {
                render_md(&body, md_w)
            };
            c.review_sel = app.review_sel;
            c.inbox_count = inbox_count;
            c.width = md_w;
        }
        let (scroll, end, max_scroll) =
            inbox_visible_range(c.lines.len(), inner_h, app.review_detail_scroll);
        let visible = c.lines[scroll..end]
            .iter()
            .map(|l| md_line_widget(l, theme))
            .collect::<Vec<Line>>();
        (visible, scroll, max_scroll)
    });

    if visible.is_empty() && scroll == 0 {
        frame.render_widget(
            Paragraph::new("no item selected").style(theme.dim()),
            inner,
        );
        return;
    }

    let scroll_hint = if max_scroll > 0 {
        let more = if scroll < max_scroll { "  ▼ more" } else { "" };
        format!(" scroll {scroll}/{max_scroll}{more} ")
    } else {
        String::new()
    };
    frame.render_widget(
        Paragraph::new(visible)
            .style(theme.text())
            .block(hairline(&scroll_hint, theme.muted_border())),
        inner,
    );
    if max_scroll > 0 {
        let mut bar = ScrollbarState::new(max_scroll).position(scroll);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .thumb_symbol("█"),
            inner,
            &mut bar,
        );
    }
}

/// Viewport into a cached layout: `(scroll, end, max_scroll)`.
fn inbox_visible_range(len: usize, inner_h: usize, requested: usize) -> (usize, usize, usize) {
    let page = inner_h.max(1);
    let max_scroll = len.saturating_sub(page);
    let scroll = requested.min(max_scroll);
    let end = (scroll + page).min(len);
    (scroll, end, max_scroll)
}

fn sel_style(selected: bool, idle: Color, theme: &Theme) -> Style {
    if selected {
        theme.selected()
    } else {
        Style::default().fg(idle)
    }
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
    let w = area.width as usize;
    frame.render_widget(
        Paragraph::new(truncate_display(&app.status, w)).style(status_style),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(truncate_display(&help_line(app), w)).style(theme.dim()),
        rows[1],
    );
}

fn draw_setup(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let outer = panel(" setup  ·  tab section  ·  esc session ", true, theme);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(4),
            Constraint::Min(4),
            Constraint::Length(5),
        ])
        .split(inner);

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
            "  notifications    {} — {}",
            if app.notifications { "on" } else { "off" },
            if app.notifications {
                "OS when away; status bar when here"
            } else {
                "status bar only"
            }
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
            .block(hairline(
                if focused {
                    " › settings · enter cycle · a workspace "
                } else {
                    " settings "
                },
                if focused {
                    theme.accent_border()
                } else {
                    theme.muted_border()
                },
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
            .block(hairline(
                if focused {
                    " › projects · enter focus · a add · d drop "
                } else {
                    " projects "
                },
                if focused {
                    theme.accent_border()
                } else {
                    theme.muted_border()
                },
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
            .block(hairline(
                if focused {
                    " › packs · enter focus · space · a add · d drop "
                } else {
                    " packs "
                },
                if focused {
                    theme.accent_border()
                } else {
                    theme.muted_border()
                },
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
            .block(hairline(" health ", theme.muted_border())),
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
    let cursor = app.input_cursor.min(app.input.len());
    let prefix = if app.input.is_char_boundary(cursor) {
        &app.input[..cursor]
    } else {
        &app.input
    };
    let cursor_x = popup.x + 1 + display_cols(prefix);
    let cursor_y = popup.y + 2;
    if cursor_x < popup.x.saturating_add(popup.width.saturating_sub(1)) {
        frame.set_cursor_position(Position {
            x: cursor_x,
            y: cursor_y,
        });
    }
}

fn draw_help_overlay(frame: &mut Frame, area: Rect, theme: &Theme) {
    let popup = centered(area, 72, 32);
    frame.render_widget(Clear, popup);
    let lines = vec![
        Line::from(vec![
            Span::styled(format!("{MARK}  {WORDMARK}"), theme.label()),
            Span::styled("  ·  keys", theme.dim()),
        ]),
        Line::from("  type         chat in the message box"),
        Line::from("  enter        send"),
        Line::from("  shift-enter  newline in the input box (ctrl-j if the terminal swallows shift-enter)"),
        Line::from("  paste        bracketed paste into the composer"),
        Line::from("  left/right   move cursor (IME cursor follows CJK width)"),
        Line::from("  up/down      scroll the conversation"),
        Line::from("  tab          conversations — enter open, d delete finished, c cancel running"),
        Line::from("  [ ]          previous / next conversation (empty input)"),
        Line::from("  esc          close overlay, then cancel pick/ask"),
        Line::from("  ctrl-n       new conversation"),
        Line::from("  @            attach file or folder from a registered project"),
        Line::from("  /            commands"),
        Line::from("  /setup       runtime, projects, packs"),
        Line::from("  /inbox       questions + candidate knowledge/skills — Enter to act"),
        Line::from("  inbox detail ↑↓ scroll body, tab to decide, pgup/pgdn / wheel also scroll"),
        Line::from("  /face        pin a Face (or /face <id>)"),
        Line::from("  /session     pick another conversation"),
        Line::from("  /clear /new  new conversation — does not resume the executor"),
        Line::from("  /learn       @paths/URLs → knowledge (auto pipeline)"),
        Line::from("  /retry       retry the open conversation"),
        Line::from("  /cancel      cancel a running task"),
        Line::from("  /delete      delete a finished task"),
        Line::from("  ↑↓ enter     permission pick in the message box (1–4)"),
        Line::from("  skill/know   same pick UI — y commit, d reject, Esc later"),
        Line::from("  idle ask     composer becomes an answer box — type, Enter, Esc later"),
        Line::from("  ?            help"),
        Line::from("  /quit        quit (or /exit, or ctrl-c twice)"),
        Line::from(""),
        Line::from("The composer is the only operable surface. Overlays are not pages."),
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

/// Single top rule — used on the main agent surface instead of nested boxes.
fn hairline<'a>(title: &'a str, border: Style) -> Block<'a> {
    Block::default()
        .borders(Borders::TOP)
        .title(title)
        .border_style(border)
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
        assert_eq!(you.pad, 0, "you body is full-width so the whole message is visible");
        assert_eq!(asst.pad, 0);
        let you_label = rows
            .iter()
            .find(|r| r.is_label && r.text == "you")
            .unwrap();
        assert!(you_label.pad > 0, "you label stays on the right");
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

    #[test]
    fn visible_input_cursor_uses_display_width() {
        let (shown, col) = visible_input("hi", 20);
        assert_eq!(shown, "hi");
        assert_eq!(col, 4); // "> " + 2

        let (shown, col) = visible_input("你好", 20);
        assert_eq!(shown, "你好");
        assert_eq!(col, 6); // "> " + 2+2

        let (shown, col) = visible_input("ab你好cd", 20);
        assert_eq!(shown, "ab你好cd");
        assert_eq!(col, 2 + 2 + 4 + 2);

        // Truncate from the left by columns, not char count.
        let (shown, col) = visible_input("一二三四五", 6);
        assert_eq!(shown, "三四五");
        assert_eq!(col, 2 + 6);

        let (shown, col) = visible_input("一二三四五", 5);
        assert_eq!(shown, "四五");
        assert_eq!(col, 2 + 4);
    }

    #[test]
    fn display_cols_counts_cjk_as_two() {
        assert_eq!(display_cols("a"), 1);
        assert_eq!(display_cols("中"), 2);
        assert_eq!(display_cols("a中b"), 4);
    }

    #[test]
    fn wrap_text_splits_on_display_columns() {
        let lines = wrap_text("一二三四五六", 6);
        assert_eq!(lines, vec!["一二三", "四五六"]);
        assert!(lines.iter().all(|l| display_width(l) <= 6));

        let mixed = wrap_text("ab你好cd世界", 6);
        assert!(mixed.iter().all(|l| display_width(l) <= 6));
        assert_eq!(mixed.concat(), "ab你好cd世界");
    }

    #[test]
    fn assistant_markdown_keeps_heading_text() {
        let rows = layout_chat(
            &[ChatLine {
                kind: ChatKind::Assistant,
                text: "## Hello **world**\n\n- alpha".into(),
            }],
            40,
        );
        let body: Vec<_> = rows
            .iter()
            .filter(|r| !r.is_label && !r.text.is_empty())
            .map(|r| r.text.as_str())
            .collect();
        assert!(body.iter().any(|t| t.contains("Hello")), "{body:?}");
        assert!(body.iter().any(|t| t.contains("alpha")), "{body:?}");
        assert!(rows.iter().any(|r| r
            .spans
            .iter()
            .any(|(_, s)| *s == MdStyle::Heading || *s == MdStyle::Bold)));
    }

    #[test]
    fn composer_view_places_cjk_cursor() {
        let (lines, col, row) = composer_view("你好", 3, 20, 8);
        assert_eq!(row, 0);
        assert_eq!(lines[0], "> 你好");
        assert_eq!(col, 2 + 2); // after first CJK char (3 bytes = 你)
        let (_, col, row) = composer_view("a\nb", 3, 20, 8);
        assert_eq!(row, 1);
        assert_eq!(col, 3); // "  b" cursor at end
    }

    #[test]
    fn composer_view_long_cjk_cursor_no_panic() {
        let input: String = "你".repeat(250);
        let cursor = 501.min(input.len());
        let cursor = crate::tui::util::floor_char_boundary(&input, cursor);
        let (lines, _col, _row) = composer_view(&input, cursor, 20, 8);
        assert!(lines.len() >= 2);
    }

    #[test]
    fn you_bubble_pad_fits_cjk_in_width() {
        let rows = layout_chat(
            &[ChatLine {
                kind: ChatKind::You,
                text: "总结下github最近三天的trending, 还有hacker news".into(),
            }],
            40,
        );
        for row in &rows {
            if row.is_label || row.text.is_empty() {
                continue;
            }
            assert!(
                row.pad + display_width(&row.text) <= 40,
                "pad {} + {:?} = {}",
                row.pad,
                row.text,
                row.pad + display_width(&row.text)
            );
        }
        assert!(rows.iter().any(|r| r.kind == ChatKind::You && !r.is_label));
    }

    #[test]
    fn inbox_visible_range_virtualizes_to_page() {
        assert_eq!(inbox_visible_range(100, 10, 0), (0, 10, 90));
        assert_eq!(inbox_visible_range(100, 10, 5), (5, 15, 90));
        assert_eq!(inbox_visible_range(100, 10, 90), (90, 100, 90));
        assert_eq!(inbox_visible_range(100, 10, 999), (90, 100, 90));
        assert_eq!(inbox_visible_range(8, 10, 0), (0, 8, 0));
        assert_eq!(inbox_visible_range(0, 10, 0), (0, 0, 0));
    }
}
