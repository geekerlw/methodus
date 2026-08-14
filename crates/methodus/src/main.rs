use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};

use methodus_core::engine::Engine;
use methodus_domain::RuntimeEvent;
use methodus_runtime::ClaudeCodeAdapter;
use methodus_store::Store;

#[derive(Parser)]
#[command(name = "methodus", about = "Persistent Personal Expert System")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize Methodus home directory and database
    Init,
    /// Task management
    Task {
        #[command(subcommand)]
        action: TaskAction,
    },
    /// Run a task (spawn executor, stream events)
    Run {
        /// Task ID to run
        task_id: String,
    },
    /// Session management
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },
}

#[derive(Subcommand)]
enum TaskAction {
    /// Create a new task
    Create {
        /// The goal / request for the task
        goal: String,
        /// Face (domain expert) to use
        #[arg(long)]
        face: Option<String>,
        /// Runtime executor to use (default: claude-code)
        #[arg(long)]
        runtime: Option<String>,
    },
    /// List all tasks
    List,
    /// Show details of a specific task
    Show {
        /// Task ID
        id: String,
    },
}

#[derive(Subcommand)]
enum SessionAction {
    /// List all sessions
    List,
    /// Show details of a specific session
    Show {
        /// Session ID
        id: String,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("\x1b[31mError:\x1b[0m {e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match cli.command {
        Commands::Init => run_init()?,
        Commands::Task { action } => {
            let engine = build_engine()?;
            match action {
                TaskAction::Create {
                    goal,
                    face,
                    runtime,
                } => {
                    let task =
                        engine.create_task(&goal, &goal, face.as_deref(), runtime.as_deref())?;
                    println!("{}", task.id);
                }
                TaskAction::List => {
                    let tasks = engine.store().list_tasks()?;
                    if tasks.is_empty() {
                        println!("No tasks.");
                        return Ok(());
                    }
                    println!("{:<20} {:<12} {:<40} CREATED", "ID", "STATUS", "TITLE");
                    println!("{}", "-".repeat(90));
                    for t in &tasks {
                        let title = if t.title.len() > 38 {
                            format!("{}…", &t.title[..37])
                        } else {
                            t.title.clone()
                        };
                        println!(
                            "{:<20} {:<12} {:<40} {}",
                            t.id,
                            t.status.to_string(),
                            title,
                            t.created_at.format("%Y-%m-%d %H:%M")
                        );
                    }
                }
                TaskAction::Show { id } => {
                    let task = engine
                        .store()
                        .get_task(&id)?
                        .ok_or_else(|| format!("task not found: {id}"))?;
                    println!("ID:         {}", task.id);
                    println!("Title:      {}", task.title);
                    println!("Status:     {}", task.status);
                    println!("Runtime:    {}", task.runtime.as_deref().unwrap_or("-"));
                    println!("Resolution: {}", task.resolution.as_deref().unwrap_or("-"));
                    println!("Created:    {}", task.created_at.to_rfc3339());
                    println!("Updated:    {}", task.updated_at.to_rfc3339());
                    println!("\n--- Request ---");
                    println!("{}", task.request);
                }
            }
        }
        Commands::Run { task_id } => {
            let engine = build_engine()?;
            let mut rx = engine.run_task(&task_id).await?;
            while let Some(event) = rx.recv().await {
                print_event(&event);
            }
            println!("\n\x1b[90m── session ended ──\x1b[0m");
        }
        Commands::Session { action } => {
            let engine = build_engine()?;
            match action {
                SessionAction::List => {
                    let sessions = engine.store().list_sessions()?;
                    if sessions.is_empty() {
                        println!("No sessions.");
                        return Ok(());
                    }
                    println!(
                        "{:<38} {:<20} {:<12} {:<12} STARTED",
                        "ID", "TASK", "RUNTIME", "STATUS"
                    );
                    println!("{}", "-".repeat(100));
                    for s in &sessions {
                        println!(
                            "{:<38} {:<20} {:<12} {:<12} {}",
                            s.id,
                            s.task_id,
                            s.runtime,
                            s.status.to_string(),
                            s.started_at.format("%Y-%m-%d %H:%M")
                        );
                    }
                }
                SessionAction::Show { id } => {
                    let session = engine
                        .store()
                        .get_session(&id)?
                        .ok_or_else(|| format!("session not found: {id}"))?;
                    println!("ID:          {}", session.id);
                    println!("Task ID:     {}", session.task_id);
                    println!("Runtime:     {}", session.runtime);
                    println!("Status:      {}", session.status);
                    println!("Transport:   {}", session.transport);
                    println!(
                        "PID:         {}",
                        session
                            .pid
                            .map(|p| p.to_string())
                            .unwrap_or_else(|| "-".to_string())
                    );
                    println!("CWD:         {}", session.cwd);
                    println!(
                        "Executor ID: {}",
                        session.executor_sid.as_deref().unwrap_or("-")
                    );
                    println!("Started:     {}", session.started_at.to_rfc3339());
                    println!(
                        "Ended:       {}",
                        session
                            .ended_at
                            .map(|d| d.to_rfc3339())
                            .unwrap_or_else(|| "-".to_string())
                    );
                }
            }
        }
    }
    Ok(())
}

/// Pretty-print a runtime event to the terminal with color coding.
fn print_event(event: &RuntimeEvent) {
    match event {
        RuntimeEvent::SessionStarted { session_id } => {
            println!("\x1b[36m▶ session started:\x1b[0m {session_id}");
        }
        RuntimeEvent::AssistantText { text } => {
            print!("{text}");
        }
        RuntimeEvent::Thinking { text } => {
            println!("\x1b[90m💭 {text}\x1b[0m");
        }
        RuntimeEvent::ToolCallStarted { name, .. } => {
            println!("\x1b[33m🔧 {name}\x1b[0m");
        }
        RuntimeEvent::ToolCallCompleted { id, .. } => {
            println!("\x1b[32m  ✓ tool done ({id})\x1b[0m");
        }
        RuntimeEvent::TurnCompleted { stop_reason } => {
            let reason = stop_reason.as_deref().unwrap_or("end_turn");
            println!("\x1b[90m── turn completed ({reason}) ──\x1b[0m");
        }
        RuntimeEvent::Result {
            is_error,
            text,
            cost_usd,
            ..
        } => {
            if *is_error {
                println!("\x1b[31m✗ ERROR:\x1b[0m {text}");
            } else {
                println!("\x1b[32m✓ RESULT:\x1b[0m {text}");
            }
            if let Some(cost) = cost_usd {
                println!("\x1b[90m  cost: ${cost:.4}\x1b[0m");
            }
        }
        RuntimeEvent::Error { message } => {
            println!("\x1b[31m✗ {message}\x1b[0m");
        }
    }
}

fn build_engine() -> Result<Engine, Box<dyn std::error::Error + Send + Sync>> {
    let home = methodus_home()?;
    let db_path = home.join("state.db");
    if !db_path.exists() {
        return Err("Methodus not initialized. Run `methodus init` first.".into());
    }
    let store = Arc::new(Store::open(&db_path)?);
    let adapter = Arc::new(ClaudeCodeAdapter::new());
    Ok(Engine::new(store, adapter, home))
}

fn run_init() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let home = methodus_home()?;
    let db_path = home.join("state.db");

    // If home and database already exist and are valid, report and exit.
    if db_path.exists() {
        // Try opening (runs migrations idempotently) to verify integrity.
        Store::open(&db_path)?;
        println!("already initialized ({})", home.display());
        return Ok(());
    }

    // Create the directory structure.
    let subdirs = ["faces", "methods", "skills", "projects", "workspaces"];
    for sub in &subdirs {
        fs::create_dir_all(home.join(sub))?;
    }

    // Create database and run migrations.
    Store::open(&db_path)?;

    println!("Initialized Methodus home at {}", home.display());
    Ok(())
}

fn methodus_home() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    if let Ok(val) = std::env::var("METHODUS_HOME") {
        return Ok(PathBuf::from(val));
    }

    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .or_else(|_| dirs_fallback())
        .map_err(|_| "could not determine home directory")?;

    Ok(home.join(".methodus"))
}

/// Fallback: try common env vars on different platforms.
fn dirs_fallback() -> Result<PathBuf, ()> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE")
            .map(PathBuf::from)
            .map_err(|_| ())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(())
    }
}
