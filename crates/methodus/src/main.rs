use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};

use methodus_core::{Engine, InstanceLock};
use methodus_domain::{ApprovalDecision, RuntimeEvent};
use methodus_runtime::{ClaudeCodeAdapter, CodexAdapter, RuntimeAdapter};
use methodus_store::Store;

mod tui;

const GENERAL_FACE_YAML: &str = include_str!("../../../resources/faces/general/face.yaml");
const GENERAL_METHOD_YAML: &str = include_str!("../../../resources/methods/general-software.yaml");
const WORKSPACE_SKILL_MD: &str =
    include_str!("../../../resources/skills/workspace-hygiene/SKILL.md");
const DEFAULT_CONFIG: &str = include_str!("../../../resources/config.yaml");

#[derive(Parser)]
#[command(name = "methodus", about = "Persistent Personal Expert System")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
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
        /// Resume an interrupted executor session (`--resume <executor_sid>`)
        #[arg(long)]
        resume: bool,
    },
    /// Reconcile sessions left running after a crash
    Recover,
    /// Resolve a pending permission approval
    Approve {
        /// Approval ID (`appr_…`)
        id: String,
        /// once | session | deny | abort
        #[arg(long)]
        decision: String,
    },
    /// Append-only event log (read-only; safe from a second terminal)
    Events {
        #[command(subcommand)]
        action: EventsAction,
    },
    /// Session management
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Experience records (file + index)
    Experience {
        #[command(subcommand)]
        action: ExperienceAction,
    },
    /// Interactive TUI (default if no subcommand is given)
    Tui,
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

#[derive(Subcommand)]
enum ExperienceAction {
    /// List experience records
    List,
}

#[derive(Subcommand)]
enum EventsAction {
    /// Show recent events (optionally follow)
    Tail {
        /// Filter by task id
        #[arg(long)]
        task: Option<String>,
        /// Keep polling for new events
        #[arg(long)]
        follow: bool,
        /// Number of historical events to print first
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: usize,
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
    let command = cli.command.unwrap_or(Commands::Tui);
    match command {
        Commands::Tui => run_tui().await?,
        Commands::Init => run_init()?,
        Commands::Task { action } => match action {
            TaskAction::Create {
                goal,
                face,
                runtime,
            } => {
                let home = methodus_home()?;
                let _lock = InstanceLock::try_acquire(&home)?;
                let engine = build_engine(&home)?;
                let task = engine.create_task(&goal, &goal, face.as_deref(), runtime.as_deref())?;
                if let Some(res) =
                    methodus_core::Resolution::parse_json(task.resolution.as_deref().unwrap_or(""))
                {
                    let skills = res
                        .skills
                        .iter()
                        .map(|s| s.name.as_str())
                        .collect::<Vec<_>>()
                        .join(",");
                    let method = res.method.as_ref().map(|m| m.id.as_str()).unwrap_or("-");
                    eprintln!(
                        "resolved face={} method={} skills=[{}] confidence={:.2}",
                        res.face_id, method, skills, res.confidence
                    );
                    if res.low_confidence {
                        eprintln!(
                            "low-confidence resolution: {}. Pin a Face with --face <id>.",
                            res.rationale
                        );
                    }
                }
                println!("{}", task.id);
            }
            TaskAction::List => {
                let engine = build_engine(&methodus_home()?)?;
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
                let engine = build_engine(&methodus_home()?)?;
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
        },
        Commands::Run { task_id, resume } => {
            let home = methodus_home()?;
            let _lock = InstanceLock::try_acquire(&home)?;
            let engine = build_engine(&home)?;
            let mut rx = engine.run_task(&task_id, resume).await?;
            while let Some(event) = rx.recv().await {
                print_event(&event);
            }
            println!("\n\x1b[90m── session ended ──\x1b[0m");
            if let Ok(Some(task)) = engine.store().get_task(&task_id) {
                if task.status == methodus_domain::TaskStatus::WaitingUser {
                    println!(
                        "\x1b[33mTask is waiting for approval.\x1b[0m Run `methodus approve <id> --decision once|session|deny|abort`."
                    );
                }
            }
        }
        Commands::Approve { id, decision } => {
            let home = methodus_home()?;
            let _lock = InstanceLock::try_acquire(&home)?;
            let engine = build_engine(&home)?;
            let parsed: ApprovalDecision = decision.parse()?;
            let mut rx = engine.approve(&id, parsed, "user").await?;
            let mut any = false;
            while let Some(event) = rx.recv().await {
                any = true;
                print_event(&event);
            }
            if !any {
                let pending = engine.store().list_pending_approvals(None)?;
                if pending.is_empty() {
                    println!("approval {id} resolved ({parsed}).");
                } else {
                    println!(
                        "approval {id} resolved ({parsed}); {} still pending.",
                        pending.len()
                    );
                    for p in pending {
                        println!("  {}  {}  {}", p.id, p.tool_name, p.subject);
                    }
                }
            }
        }
        Commands::Events { action } => match action {
            EventsAction::Tail {
                task,
                follow,
                limit,
            } => {
                let engine = build_engine(&methodus_home()?)?;
                let mut seen = HashSet::new();
                loop {
                    let events = engine.store().list_events(task.as_deref(), limit)?;
                    for ev in events {
                        if seen.insert(ev.id.clone()) {
                            println!(
                                "{}  {:<22}  {}",
                                ev.occurred_at,
                                ev.event_type,
                                ev.task_id.as_deref().unwrap_or("-")
                            );
                        }
                    }
                    if !follow {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
            }
        },
        Commands::Recover => {
            let home = methodus_home()?;
            let _lock = InstanceLock::try_acquire(&home)?;
            let engine = build_engine(&home)?;
            let recovered = engine.recover().await?;
            if recovered.is_empty() {
                println!("No in-flight sessions to reconcile.");
                return Ok(());
            }
            for rec in &recovered {
                if rec.still_live {
                    println!(
                        "live     task={} session={} executor={}",
                        rec.task_id,
                        rec.session_id,
                        rec.executor_sid.as_deref().unwrap_or("-")
                    );
                } else {
                    println!(
                        "interrupted  task={} session={} executor={}  →  methodus run {} --resume",
                        rec.task_id,
                        rec.session_id,
                        rec.executor_sid.as_deref().unwrap_or("-"),
                        rec.task_id
                    );
                }
            }
        }
        Commands::Session { action } => {
            let engine = build_engine(&methodus_home()?)?;
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
        Commands::Experience { action } => {
            let engine = build_engine(&methodus_home()?)?;
            match action {
                ExperienceAction::List => {
                    let experiences = engine.store().list_experiences()?;
                    if experiences.is_empty() {
                        println!("No experiences.");
                        return Ok(());
                    }
                    println!("{:<20} {:<20} {:<10} PATH", "ID", "TASK", "OUTCOME");
                    println!("{}", "-".repeat(90));
                    for e in &experiences {
                        println!(
                            "{:<20} {:<20} {:<10} {}",
                            e.id,
                            e.task_id,
                            e.outcome.as_deref().unwrap_or("-"),
                            e.path
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

async fn run_tui() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let home = methodus_home()?;
    let lock = InstanceLock::try_acquire(&home)?;
    let engine = build_engine(&home)?;
    let recovered = engine.recover().await?;
    tui::run(engine, lock, recovered).await
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
        RuntimeEvent::ApprovalRequested {
            id,
            tool_name,
            input,
        } => {
            println!("\x1b[33m⚠ approval requested:\x1b[0m {id}");
            println!("  tool:  {tool_name}");
            println!("  input: {input}");
            println!("  → methodus approve {id} --decision once|session|deny|abort");
        }
        RuntimeEvent::Error { message } => {
            println!("\x1b[31m✗ {message}\x1b[0m");
        }
    }
}

fn build_engine(
    home: &std::path::Path,
) -> Result<Engine, Box<dyn std::error::Error + Send + Sync>> {
    let db_path = home.join("state.db");
    if !db_path.exists() {
        return Err("Methodus not initialized. Run `methodus init` first.".into());
    }
    let store = Arc::new(Store::open(&db_path)?);
    let mut adapters: HashMap<String, Arc<dyn RuntimeAdapter>> = HashMap::new();
    adapters.insert(
        "claude-code".to_string(),
        Arc::new(ClaudeCodeAdapter::new()),
    );
    adapters.insert("codex".to_string(), Arc::new(CodexAdapter::new()));
    Ok(Engine::with_runtimes(store, home.to_path_buf(), adapters))
}

fn run_init() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let home = methodus_home()?;
    let db_path = home.join("state.db");

    fs::create_dir_all(&home)?;
    seed_home(&home)?;

    if db_path.exists() {
        Store::open(&db_path)?;
        println!("already initialized ({})", home.display());
        return Ok(());
    }

    Store::open(&db_path)?;

    println!("Initialized Methodus home at {}", home.display());
    Ok(())
}

fn seed_home(home: &std::path::Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let subdirs = [
        "faces",
        "methods",
        "skills",
        "projects",
        "workspaces",
        "faces/general",
        "faces/general/experiences",
        "faces/general/knowledge",
        "faces/general/hypotheses",
        "skills/workspace-hygiene",
    ];
    for sub in &subdirs {
        fs::create_dir_all(home.join(sub))?;
    }

    let face_path = home.join("faces/general/face.yaml");
    if !face_path.exists() {
        fs::write(&face_path, GENERAL_FACE_YAML)?;
    }
    let method_path = home.join("methods/general-software.yaml");
    if !method_path.exists() {
        fs::write(&method_path, GENERAL_METHOD_YAML)?;
    }
    let skill_path = home.join("skills/workspace-hygiene/SKILL.md");
    if !skill_path.exists() {
        fs::write(&skill_path, WORKSPACE_SKILL_MD)?;
    }
    let config_path = home.join("config.yaml");
    if !config_path.exists() {
        fs::write(&config_path, DEFAULT_CONFIG)?;
    }
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
