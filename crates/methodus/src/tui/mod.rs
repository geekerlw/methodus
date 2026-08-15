//! In-process ratatui UI. Observes the Engine; does not own session lifecycle.

mod app;
mod ui;

use std::io::stdout;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use futures::StreamExt;
use methodus_core::{Engine, InstanceLock, RecoveredSession};
use ratatui::prelude::{CrosstermBackend, Terminal};

use app::{App, Command, Mode};

pub async fn run(
    engine: Engine,
    _lock: InstanceLock,
    recovered: Vec<RecoveredSession>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = run_loop(&mut terminal, engine, recovered).await;

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    engine: Engine,
    recovered: Vec<RecoveredSession>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = App::new(engine, recovered);
    app.refresh();
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        terminal.draw(|frame| ui::draw(frame, &app))?;
        if app.should_quit {
            break;
        }

        let cmd = if app.event_rx.is_some() {
            tokio::select! {
                _ = tick.tick() => {
                    app.refresh();
                    Command::None
                }
                ev = events.next() => key_command(&mut app, ev),
                rt = recv_runtime(&mut app) => {
                    if let Some(event) = rt {
                        app.push_runtime(event);
                    }
                    Command::None
                }
            }
        } else {
            tokio::select! {
                _ = tick.tick() => {
                    app.refresh();
                    Command::None
                }
                ev = events.next() => key_command(&mut app, ev),
            }
        };

        match cmd {
            Command::None => {}
            Command::Quit => app.should_quit = true,
            Command::Create { goal, face } => {
                match app
                    .engine
                    .create_task(&goal, &goal, face.as_deref(), app.runtime.as_deref())
                {
                    Ok(task) => {
                        app.status = format_created(&task);
                        app.input.clear();
                        app.mode = Mode::Normal;
                        app.select_task(&task.id);
                        app.refresh();
                    }
                    Err(e) => app.status = format!("create failed: {e}"),
                }
            }
            Command::Run { task_id, resume } => match app.engine.run_task(&task_id, resume).await {
                Ok(rx) => {
                    app.attach_session(task_id, rx);
                    app.refresh();
                }
                Err(e) => app.status = format!("run failed: {e}"),
            },
            Command::Approve { id, decision } => {
                match app.engine.approve(&id, decision, "tui").await {
                    Ok(rx) => {
                        app.status = format!("approval {id} → {decision}");
                        app.attach_receiver(rx);
                        app.refresh();
                    }
                    Err(e) => app.status = format!("approve failed: {e}"),
                }
            }
        }
    }
    Ok(())
}

fn key_command(app: &mut App, ev: Option<Result<Event, std::io::Error>>) -> Command {
    match ev {
        Some(Ok(Event::Key(key)))
            if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat =>
        {
            app.handle_key(key)
        }
        Some(Err(e)) => {
            app.status = format!("input error: {e}");
            Command::None
        }
        _ => Command::None,
    }
}

async fn recv_runtime(app: &mut App) -> Option<methodus_domain::RuntimeEvent> {
    match app.event_rx.as_mut() {
        Some(rx) => match rx.recv().await {
            Some(ev) => Some(ev),
            None => {
                app.event_rx = None;
                app.status = "session ended".to_string();
                app.refresh();
                None
            }
        },
        None => std::future::pending().await,
    }
}

fn format_created(task: &methodus_domain::Task) -> String {
    if let Some(res) =
        methodus_core::Resolution::parse_json(task.resolution.as_deref().unwrap_or(""))
    {
        let method = res.method.as_ref().map(|m| m.id.as_str()).unwrap_or("-");
        let low = if res.low_confidence {
            "  LOW CONFIDENCE — pin a Face on the Faces page"
        } else {
            ""
        };
        format!(
            "created {}  face={} method={} conf={:.2}{low}",
            task.id, res.face_id, method, res.confidence
        )
    } else {
        format!("created {}", task.id)
    }
}
