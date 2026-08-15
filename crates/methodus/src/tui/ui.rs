use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs, Wrap};
use ratatui::Frame;

use super::app::{status_counts, App, Mode, Page};

const CYAN: Color = Color::Rgb(125, 207, 217);
const AMBER: Color = Color::Rgb(232, 184, 109);
const MUTED: Color = Color::Rgb(122, 132, 148);
const TEXT: Color = Color::Rgb(224, 228, 234);
const BG_PANEL: Color = Color::Rgb(18, 22, 28);

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(area);

    draw_tabs(frame, chunks[0], app);
    match app.page {
        Page::Dashboard => draw_dashboard(frame, chunks[1], app),
        Page::Tasks => draw_tasks(frame, chunks[1], app),
        Page::Session => draw_session(frame, chunks[1], app),
        Page::Faces => draw_faces(frame, chunks[1], app),
    }
    draw_status(frame, chunks[2], app);
    draw_help(frame, chunks[3], app);
    if app.mode == Mode::Creating {
        draw_create_modal(frame, area, app);
    }
}

fn draw_tabs(frame: &mut Frame, area: Rect, app: &App) {
    let titles: Vec<Line> = Page::all()
        .iter()
        .map(|p| Line::from(format!(" {} ", p.title())))
        .collect();
    let selected = Page::all().iter().position(|p| *p == app.page).unwrap_or(0);
    let runtime = app.runtime.as_deref().unwrap_or("claude-code");
    let face = app.default_face.as_deref().unwrap_or("auto");
    let tabs = Tabs::new(titles)
        .select(selected)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" methodus  ·  runtime {runtime}  ·  face {face} "))
                .border_style(Style::default().fg(CYAN))
                .style(Style::default().bg(BG_PANEL).fg(TEXT)),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(CYAN)
                .add_modifier(Modifier::BOLD),
        )
        .style(Style::default().fg(MUTED));
    frame.render_widget(tabs, area);
}

fn draw_dashboard(frame: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);
    let (queued, running, waiting, done) = status_counts(&app.tasks);
    let summary = format!(
        "queued {queued}   running {running}   waiting_user {waiting}   done {done}\n\n\
         pending approvals: {}\n\
         recovered sessions: {}\n\n\
         This TUI is the Methodus process. Keep it open (tmux) so runs continue.\n\
         n  new task     r  run selected     R  resume\n\
         y  approve once   s session   d deny   x abort",
        app.approvals.len(),
        app.recovered.len(),
    );
    frame.render_widget(
        Paragraph::new(summary)
            .style(Style::default().fg(TEXT).bg(BG_PANEL))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" overview ")
                    .border_style(Style::default().fg(MUTED)),
            )
            .wrap(Wrap { trim: false }),
        cols[0],
    );
    frame.render_widget(approval_list(app, " pending approvals "), cols[1]);
}

fn draw_tasks(frame: &mut Frame, area: Rect, app: &App) {
    if app.tasks.is_empty() {
        frame.render_widget(
            Paragraph::new("No tasks. Press n to create one.")
                .style(Style::default().fg(MUTED).bg(BG_PANEL))
                .block(panel(" tasks ")),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .tasks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mark = if i == app.task_sel { "▸" } else { " " };
            let style = if i == app.task_sel {
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD)
            } else {
                status_style(&t.status)
            };
            ListItem::new(format!("{mark} {:<12} {:<14} {}", t.status, t.id, t.title)).style(style)
        })
        .collect();
    frame.render_widget(
        List::new(items)
            .block(panel(" tasks  ·  Enter view  r run  n new "))
            .style(Style::default().bg(BG_PANEL).fg(TEXT)),
        area,
    );
}

fn draw_session(frame: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(8)])
        .split(area);
    let title = match &app.session_task_id {
        Some(id) => format!(" session  {id} "),
        None => " session  ·  open a task with Enter ".to_string(),
    };
    let start = app
        .transcript
        .len()
        .saturating_sub(area.height.saturating_sub(10) as usize);
    let body = app.transcript[start..].join("\n");
    frame.render_widget(
        Paragraph::new(body)
            .style(Style::default().fg(TEXT).bg(BG_PANEL))
            .block(panel(&title))
            .wrap(Wrap { trim: false }),
        cols[0],
    );
    frame.render_widget(approval_list(app, " approve "), cols[1]);
}

fn draw_faces(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .faces
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mark = if i == app.face_sel { "▸" } else { " " };
            let pin = if app.default_face.as_deref() == Some(f.id.as_str()) {
                "  [default]"
            } else {
                ""
            };
            let style = if i == app.face_sel {
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT)
            };
            ListItem::new(format!(
                "{mark} {} — {}{pin}\n    {}",
                f.id, f.name, f.description
            ))
            .style(style)
        })
        .collect();
    frame.render_widget(
        List::new(items)
            .block(panel(" faces  ·  Enter pin as default for new tasks "))
            .style(Style::default().bg(BG_PANEL).fg(TEXT)),
        area,
    );
}

fn approval_list<'a>(app: &'a App, title: &'a str) -> List<'a> {
    if app.approvals.is_empty() {
        return List::new([ListItem::new("none").style(Style::default().fg(MUTED))])
            .block(panel(title))
            .style(Style::default().bg(BG_PANEL).fg(TEXT));
    }
    let items: Vec<ListItem> = app
        .approvals
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let mark = if i == app.approval_sel { "▸" } else { " " };
            let style = if i == app.approval_sel {
                Style::default().fg(AMBER).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(AMBER)
            };
            ListItem::new(format!("{mark} {}  {}  {}", a.id, a.tool_name, a.subject)).style(style)
        })
        .collect();
    List::new(items)
        .block(panel(title))
        .style(Style::default().bg(BG_PANEL).fg(TEXT))
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(
        Paragraph::new(app.status.as_str())
            .style(Style::default().fg(AMBER).bg(BG_PANEL))
            .block(
                Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::TOP)
                    .border_style(Style::default().fg(MUTED)),
            ),
        area,
    );
}

fn draw_help(frame: &mut Frame, area: Rect, app: &App) {
    let text = match app.mode {
        Mode::Creating => " enter create   esc cancel   backspace delete ",
        Mode::Normal => {
            " 1-4 pages  tab next  n new  r run  R resume  t runtime  y/s/d/x approve  q quit "
        }
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(MUTED).bg(BG_PANEL)),
        area,
    );
}

fn draw_create_modal(frame: &mut Frame, area: Rect, app: &App) {
    let popup = centered(area, 70, 7);
    frame.render_widget(Clear, popup);
    let face = app.default_face.as_deref().unwrap_or("auto");
    let body = format!(
        "goal> {}\nface  {face}   runtime  {}",
        app.input,
        app.runtime.as_deref().unwrap_or("claude-code")
    );
    frame.render_widget(
        Paragraph::new(body)
            .style(Style::default().fg(TEXT).bg(BG_PANEL))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" new task ")
                    .border_style(Style::default().fg(CYAN)),
            ),
        popup,
    );
}

fn panel(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(MUTED))
        .style(Style::default().bg(BG_PANEL).fg(TEXT))
}

fn status_style(status: &methodus_domain::TaskStatus) -> Style {
    use methodus_domain::TaskStatus::*;
    match status {
        Running | Reviewing => Style::default().fg(Color::Rgb(129, 201, 149)),
        WaitingUser => Style::default().fg(AMBER),
        Failed => Style::default().fg(Color::Rgb(224, 108, 117)),
        Completed => Style::default().fg(MUTED),
        _ => Style::default().fg(TEXT),
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
