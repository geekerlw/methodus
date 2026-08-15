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

use app::{App, Command, StatusLevel};

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
    app.restore_recovered();
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
            Command::Send { task_id, text } => {
                let id = match task_id {
                    Some(id) => Some(id),
                    None => match app.engine.create_task(
                        &text,
                        &text,
                        app.default_face.as_deref(),
                        app.runtime.as_deref(),
                    ) {
                        Ok(task) => {
                            app.select_task(&task.id);
                            app.refresh();
                            Some(task.id)
                        }
                        Err(e) => {
                            app.input_error = Some(format!("could not create: {e}"));
                            app.set_status(StatusLevel::Error, format!("create failed: {e}"));
                            None
                        }
                    },
                };
                if let Some(id) = id {
                    match app.engine.send_turn(&id, &text).await {
                        Ok(rx) => {
                            app.attach_session(id, rx);
                            app.refresh();
                        }
                        Err(e) => {
                            app.input_error = Some(e.to_string());
                            app.set_status(StatusLevel::Error, format!("send failed: {e}"));
                        }
                    }
                }
            }
            Command::Run { task_id, resume } => match app.engine.run_task(&task_id, resume).await {
                Ok(rx) => {
                    app.attach_session(task_id, rx);
                    app.refresh();
                }
                Err(e) => {
                    app.input_error = Some(e.to_string());
                    app.set_status(StatusLevel::Error, format!("run failed: {e}"));
                }
            },
            Command::Approve { id, decision } => {
                match app.engine.approve(&id, decision, "tui").await {
                    Ok(rx) => {
                        app.set_status(StatusLevel::Ok, format!("approval {id} → {decision}"));
                        app.attach_receiver(rx);
                        app.refresh();
                    }
                    Err(e) => app.set_status(StatusLevel::Error, format!("approve failed: {e}")),
                }
            }
            Command::Cancel { task_id } => match app.engine.cancel_task(&task_id) {
                Ok(()) => {
                    app.set_status(StatusLevel::Ok, format!("cancelled {task_id}"));
                    app.refresh();
                }
                Err(e) => app.set_status(StatusLevel::Error, format!("cancel failed: {e}")),
            },
            Command::ReviewKnowledge { id, commit } => {
                match app.engine.review_knowledge(&id, commit) {
                    Ok(item) => {
                        app.set_status(StatusLevel::Ok, format!("{} → {}", item.id, item.status));
                        app.refresh();
                    }
                    Err(e) => app.set_status(StatusLevel::Error, format!("review failed: {e}")),
                }
            }
            Command::AnswerQuestion { id, text } => match app.engine.answer_question(&id, &text) {
                Ok(q) => {
                    app.set_status(StatusLevel::Ok, format!("{} answered", q.id));
                    app.mode = app::Mode::Normal;
                    app.input.clear();
                    app.input_error = None;
                    app.answering_id = None;
                    app.refresh();
                }
                Err(e) => {
                    app.input_error = Some(e.to_string());
                    app.set_status(StatusLevel::Error, format!("answer failed: {e}"));
                }
            },
            Command::SnoozeQuestion { id } => match app.engine.snooze_question(&id) {
                Ok(q) => {
                    app.set_status(StatusLevel::Ok, format!("{} snoozed", q.id));
                    app.refresh();
                }
                Err(e) => app.set_status(StatusLevel::Error, format!("snooze failed: {e}")),
            },
            Command::DismissQuestion { id } => match app.engine.dismiss_question(&id) {
                Ok(q) => {
                    app.set_status(StatusLevel::Ok, format!("{} dismissed", q.id));
                    app.refresh();
                }
                Err(e) => app.set_status(StatusLevel::Error, format!("dismiss failed: {e}")),
            },
            Command::LearnSkill { task_id, hint } => {
                match app.engine.learn_skill(&task_id, hint.as_deref()) {
                    Ok(item) => {
                        app.input.clear();
                        app.input_error = None;
                        app.page = app::Page::Review;
                        app.refresh();
                        app.set_status(
                            StatusLevel::Ok,
                            format!("{} drafted — y to commit on review", item.id),
                        );
                    }
                    Err(e) => {
                        app.input_error = Some(e.to_string());
                        app.set_status(StatusLevel::Error, format!("learn failed: {e}"));
                    }
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
            app.set_status(StatusLevel::Error, format!("input error: {e}"));
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
                app.set_status(StatusLevel::Ok, "your turn — type a follow-up");
                app.notify("turn", "your turn — type a follow-up");
                app.refresh();
                None
            }
        },
        None => std::future::pending().await,
    }
}
