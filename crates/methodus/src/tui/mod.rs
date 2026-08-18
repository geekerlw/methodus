//! In-process ratatui UI. Observes the Engine; does not own session lifecycle.

mod app;
mod fuzzy;
mod md;
mod ui;
mod util;

use std::io::stdout;
use std::time::Duration;

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyboardEnhancementFlags, KeyEventKind, MouseEventKind, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use futures::StreamExt;
use methodus_core::{Engine, InstanceLock, RecoveredSession};
use ratatui::prelude::{CrosstermBackend, Terminal};

use app::{slash_menu_open, App, Command, StatusLevel};
use crate::notify::NotifyUrgency;

pub async fn run(
    engine: Engine,
    _lock: InstanceLock,
    recovered: Vec<RecoveredSession>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    stdout().execute(EnableMouseCapture)?;
    stdout().execute(EnableBracketedPaste)?;
    let _ = stdout().execute(PushKeyboardEnhancementFlags(
        KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS,
    ));
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = run_loop(&mut terminal, engine, recovered).await;

    disable_raw_mode()?;
    let _ = stdout().execute(PopKeyboardEnhancementFlags);
    stdout().execute(DisableBracketedPaste)?;
    stdout().execute(DisableMouseCapture)?;
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
            Command::Delete { task_id } => match app.engine.delete_task(&task_id) {
                Ok(()) => {
                    if app.session_task_id.as_deref() == Some(task_id.as_str()) {
                        app.session_task_id = None;
                        app.event_rx = None;
                        app.transcript.clear();
                    }
                    app.set_status(StatusLevel::Ok, format!("deleted {task_id}"));
                    app.refresh();
                }
                Err(e) => app.set_status(StatusLevel::Error, format!("delete failed: {e}")),
            },
            Command::ReviewKnowledge { id, action } => {
                match app.engine.review_knowledge(&id, action) {
                    Ok(item) => {
                        app.knowledge_pick_id = None;
                        if app.inbox_detail_open() {
                            app.close_inbox_detail();
                        }
                        app.set_status(StatusLevel::Ok, format!("{} → {}", item.id, item.status));
                        app.refresh();
                    }
                    Err(e) => app.set_status(StatusLevel::Error, format!("review failed: {e}")),
                }
            }
            Command::ReviewEvolution { id, approve } => {
                match app.engine.review_evolution(&id, approve) {
                    Ok(item) => {
                        app.evolution_pick_id = None;
                        if app.inbox_detail_open() {
                            app.close_inbox_detail();
                        }
                        app.set_status(
                            StatusLevel::Ok,
                            format!("{} → {} (face `{}`)", item.id, item.status, item.target_id),
                        );
                        app.refresh();
                    }
                    Err(e) => app.set_status(StatusLevel::Error, format!("evolution failed: {e}")),
                }
            }
            Command::ReviewHypothesis { id, action } => {
                match app.engine.review_hypothesis(&id, action) {
                    Ok(item) => {
                        app.hypothesis_pick_id = None;
                        if app.inbox_detail_open() {
                            app.close_inbox_detail();
                        }
                        app.set_status(StatusLevel::Ok, format!("{} → {}", item.id, item.status));
                        app.refresh();
                    }
                    Err(e) => app.set_status(StatusLevel::Error, format!("hypothesis failed: {e}")),
                }
            }
            Command::AnswerQuestion { id, text } => match app.engine.answer_question(&id, &text) {
                Ok(q) => {
                    app.set_status(StatusLevel::Ok, format!("{} answered", q.id));
                    app.mode = app::Mode::Normal;
                    app.input.clear();
                    app.input_error = None;
                    app.answering_id = None;
                    if app.inbox_detail_open() {
                        app.close_inbox_detail();
                    }
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
                    if app.inbox_detail_open() {
                        app.close_inbox_detail();
                    }
                    app.refresh();
                }
                Err(e) => app.set_status(StatusLevel::Error, format!("snooze failed: {e}")),
            },
            Command::DismissQuestion { id } => match app.engine.dismiss_question(&id) {
                Ok(q) => {
                    app.set_status(StatusLevel::Ok, format!("{} dismissed", q.id));
                    if app.inbox_detail_open() {
                        app.close_inbox_detail();
                    }
                    app.refresh();
                }
                Err(e) => app.set_status(StatusLevel::Error, format!("dismiss failed: {e}")),
            },
            Command::CompleteReview { task_id } => match app.engine.complete_review(&task_id) {
                Ok(task) => {
                    app.set_status(StatusLevel::Ok, format!("{} → {}", task.id, task.status));
                    if app.inbox_detail_open() {
                        app.close_inbox_detail();
                    }
                    app.refresh();
                }
                Err(e) => app.set_status(StatusLevel::Error, format!("review done failed: {e}")),
            },
            Command::Learn { hint, sources, face } => {
                if app.busy() {
                    app.set_status(StatusLevel::Warn, "wait until this turn ends");
                    continue;
                }
                match app.engine.create_learn_task(&hint, &sources, face.as_deref()) {
                    Ok((task, mode)) => {
                        let id = task.id.clone();
                        let label = mode.label();
                        app.session_task_id = None;
                        app.event_rx = None;
                        app.transcript.clear();
                        app.input.clear();
                        app.input_error = None;
                        match app.engine.run_task(&id, false).await {
                            Ok(rx) => {
                                app.attach_session(id, rx);
                                app.set_status(
                                    StatusLevel::Info,
                                    format!("learn ({label}) — results land in /inbox"),
                                );
                                app.refresh();
                            }
                            Err(e) => {
                                app.input_error = Some(e.to_string());
                                app.set_status(StatusLevel::Error, format!("learn failed: {e}"));
                            }
                        }
                    }
                    Err(e) => {
                        app.input_error = Some(e.to_string());
                        app.set_status(StatusLevel::Error, format!("learn failed: {e}"));
                    }
                }
            }
            Command::CleanupWorkspaces { max_age_days } => {
                match app.engine.cleanup_workspaces(max_age_days) {
                    Ok(n) => {
                        app.set_status(
                            StatusLevel::Ok,
                            format!("removed {n} workspace dir(s) older than {max_age_days}d"),
                        );
                        app.refresh();
                    }
                    Err(e) => app.set_status(StatusLevel::Error, format!("cleanup failed: {e}")),
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
            app.touch_user_input();
            app.handle_key(key)
        }
        Some(Ok(Event::Paste(text))) => {
            app.touch_user_input();
            app.insert_paste(&text);
            Command::None
        }
        Some(Ok(Event::Mouse(mouse))) => {
            app.touch_user_input();
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    if app.inbox_detail_open() {
                        app.scroll_review_detail(-3);
                    } else if slash_menu_open(&app.input) {
                        app.move_slash_sel(-1);
                    } else if app.mention_menu_open() {
                        app.move_mention_sel(-1);
                    } else {
                        app.scroll_session(1);
                    }
                }
                MouseEventKind::ScrollDown => {
                    if app.inbox_detail_open() {
                        app.scroll_review_detail(3);
                    } else if slash_menu_open(&app.input) {
                        app.move_slash_sel(1);
                    } else if app.mention_menu_open() {
                        app.move_mention_sel(1);
                    } else {
                        app.scroll_session(-1);
                    }
                }
                _ => {}
            }
            Command::None
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
                    app.set_status(
                        StatusLevel::Ok,
                        "your turn — type below · /inbox",
                    );
                    let task = app.session_task_id.as_deref().unwrap_or("session");
                    app.notify(
                        &format!("turn:{task}"),
                        NotifyUrgency::Low,
                        &format!("[{task}] your turn — type a follow-up"),
                    );
                    app.refresh();
                    None
                }
        },
        None => std::future::pending().await,
    }
}
