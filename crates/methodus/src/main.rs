//! Methodus binary: open the TUI. First launch seeds `~/.methodus`.

use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;

use clap::{Parser, Subcommand, ValueEnum};

use methodus_core::{ensure_home, methodus_home, AgentQuery, Engine, InstanceLock};
use methodus_runtime::{ClaudeCodeAdapter, CodexAdapter, CursorAdapter, RuntimeAdapter};
use methodus_store::Store;

mod notify;
mod tui;

#[derive(Parser)]
#[command(
    name = "methodus",
    version,
    about = "Maintainer knowledge studio and Agent engineering-memory CLI",
    long_about = "Opens the Methodus maintainer TUI. The `agent` subcommands are a\n\
                  read-only machine-facing interface used by the official runtime connector Skill."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Read-only query protocol used by the connector Skill.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    /// Install the Methodus connector Skill for a supported runtime.
    Setup {
        #[arg(long, default_value = "all")]
        runtime: String,
        #[arg(long)]
        force: bool,
        /// Remove only a Methodus-owned connector.
        #[arg(long)]
        uninstall: bool,
    },
    /// Check the local graph, Team roots, runtimes, and connector installation.
    Doctor,
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
    /// Return a bounded context bundle for a goal.
    Prepare {
        #[arg(long)]
        goal: String,
        #[arg(long, default_value_t = 1200)]
        budget: i64,
        #[arg(long, value_delimiter = ',')]
        scope: Vec<String>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
        format: OutputFormat,
    },
    /// Search consumer-visible graph metadata.
    Search {
        #[arg(long)]
        query: String,
        #[arg(long = "type", value_delimiter = ',')]
        node_type: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        kind: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        scope: Vec<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
        format: OutputFormat,
    },
    /// Read one consumer-visible node or explicit history node.
    Get {
        id: String,
        #[arg(long)]
        facet: Option<String>,
        #[arg(long)]
        history: bool,
        #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
        format: OutputFormat,
    },
    /// Return a bounded one-hop relation neighborhood.
    Related {
        id: String,
        #[arg(long)]
        relation: Option<String>,
        #[arg(long, default_value_t = 1)]
        depth: u8,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
        format: OutputFormat,
    },
    /// Report read-only index and validation status.
    Status {
        #[arg(long, value_enum, default_value_t = OutputFormat::Markdown)]
        format: OutputFormat,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat { Markdown, Json }

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Agent { command }) => run_agent(command),
        Some(Command::Setup { runtime, force, uninstall }) => run_setup(&runtime, force, uninstall),
        Some(Command::Doctor) => run_doctor(),
        None => run_tui().await,
    };
    if let Err(e) = result {
        eprintln!("\x1b[31mError:\x1b[0m {e}");
        std::process::exit(error_exit_code(e.as_ref()));
    }
}

/// Keep the machine-facing connector contract stable even when the underlying
/// core error wording changes. Clap owns its own argument-parse exit code (2);
/// this covers errors returned after parsing.
fn error_exit_code(error: &(dyn Error + 'static)) -> i32 {
    if let Some(core) = error.downcast_ref::<methodus_core::CoreError>() {
        return match core {
            methodus_core::CoreError::Store(_) => 3,
            methodus_core::CoreError::Runtime(_) | methodus_core::CoreError::UnknownRuntime(_) => 5,
            methodus_core::CoreError::Io(_) | methodus_core::CoreError::Locked(_) => 6,
            methodus_core::CoreError::Other(message) => classify_agent_error(message),
        };
    }
    classify_agent_error(&error.to_string())
}

fn classify_agent_error(message: &str) -> i32 {
    let lower = message.to_ascii_lowercase();
    if lower.contains("cannot be empty") || lower.contains("must be positive") || lower.contains("invalid") || lower.contains("limited to") {
        return 2;
    }
    if lower.contains("not found") || lower.contains("not consumer-visible") || lower.contains("deprecated") {
        return 4;
    }
    if lower.contains("protocol") || lower.contains("runtime") || lower.contains("connector") {
        return 5;
    }
    if lower.contains("not initialized") || lower.contains("state.db") || lower.contains("home is") {
        return 3;
    }
    6
}

fn run_agent(command: AgentCommand) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let home = methodus_home()?;
    let db = home.join("state.db");
    if !db.is_file() {
        return Err(format!("Methodus home is not initialized: {}; run `methodus` or `methodus setup` first", home.display()).into());
    }
    // Agent queries are deliberately read-only. The maintainer TUI and `doctor`
    // own index refreshes; a connector invocation must never rewrite the graph.
    let store = Store::open_read_only(&db)?;
    let query = AgentQuery::new(&store, &home);
    match command {
        AgentCommand::Prepare { goal, budget, scope, format } => {
            if goal.trim().is_empty() { return Err("--goal cannot be empty".into()); }
            if budget <= 0 { return Err("--budget must be positive".into()); }
            let response = query.prepare(&goal, budget, &scope)?;
            print_payload(&response, format, |value| render_prepare(value));
        }
        AgentCommand::Search { query: text, node_type, kind, scope, limit, format } => {
            let result = query.search(&text, &node_type, &kind, &scope, limit)?;
            print_payload(&result, format, |value| render_search(value));
        }
        AgentCommand::Get { id, facet, history, format } => {
            let result = query.get(&id, facet.as_deref(), history)?;
            print_payload(&result, format, |value| render_item(value));
        }
        AgentCommand::Related { id, relation, depth, limit, format } => {
            if depth > 1 { return Err("--depth is limited to 1 in protocol v1".into()); }
            let result = query.related(&id, relation.as_deref(), limit)?;
            print_payload(&result, format, |value| render_search(value));
        }
        AgentCommand::Status { format } => {
            let nodes = store.list_graph_nodes(None)?;
            let stale = nodes.iter().filter(|node| node.status.as_deref() == Some("stale")).count();
            let committed = nodes.iter().filter(|node| node.status.as_deref() == Some("committed")).count();
            let validation = methodus_core::validate_graph(&home)?;
            let errors = validation.iter().filter(|issue| issue.severity == methodus_core::IssueSeverity::Error).count();
            let warnings = validation.iter().filter(|issue| issue.severity == methodus_core::IssueSeverity::Warning).count();
            let status = serde_json::json!({
                "protocol_version": methodus_core::AGENT_PROTOCOL_VERSION,
                "index_revision": query.index_revision()?,
                "home": home.display().to_string(),
                "selected_team": methodus_core::UserConfig::load(&home).selected_team(),
                "nodes": nodes.len(), "committed": committed, "stale": stale,
                "validation_errors": errors, "validation_warnings": warnings,
                "personal": home.join("personal").is_dir(), "teams": home.join("teams").is_dir(),
            });
            print_payload(&status, format, |value| format!("Methodus agent protocol {}\n\nrevision   {}\nhome       {}\nteam       {}\nnodes      {}\ncommitted  {}\nstale      {}\nerrors     {}\nwarnings   {}\npersonal   {}\nteams      {}\n", scalar(&value["protocol_version"]), scalar(&value["index_revision"]), scalar(&value["home"]), scalar(&value["selected_team"]), scalar(&value["nodes"]), scalar(&value["committed"]), scalar(&value["stale"]), scalar(&value["validation_errors"]), scalar(&value["validation_warnings"]), scalar(&value["personal"]), scalar(&value["teams"])));
        }
    }
    Ok(())
}

fn print_payload<T, F>(value: &T, format: OutputFormat, markdown: F)
where T: serde::Serialize, F: FnOnce(serde_json::Value) -> String {
    let json = serde_json::to_value(value).unwrap_or_else(|_| serde_json::json!({"error":"serialization failed"}));
    match format { OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&json).unwrap_or_else(|_| "{}".into())), OutputFormat::Markdown => println!("{}", markdown(json)), }
}

fn render_prepare(value: serde_json::Value) -> String {
    let mut out = format!("# Methodus context\n\n- protocol: {}\n- goal: {}\n- index_revision: {}\n- estimated_tokens: {} / {}\n", scalar(&value["protocol_version"]), scalar(&value["goal"]), scalar(&value["index_revision"]), scalar(&value["estimated_tokens"]), scalar(&value["budget_tokens"]));
    if let Some(items) = value["items"].as_array() { for item in items { out.push_str(&format!("\n## {} · {}\n\n{}\n\n_Why selected: {}_\n", scalar(&item["title"]), scalar(&item["facet"]), scalar(&item["content"]), scalar(&item["rationale"]))); if let Some(warnings) = item["warnings"].as_array() { for warning in warnings { out.push_str(&format!("\n⚠ {}\n", scalar(warning))); } } } }
    if let Some(ids) = value["lazy_ids"].as_array() { if !ids.is_empty() { out.push_str("\n## Lazy references\n\n"); for id in ids { out.push_str(&format!("- {}\n", scalar(id))); } } }
    if let Some(warnings) = value["warnings"].as_array() { if !warnings.is_empty() { out.push_str("\n## Warnings\n\n"); for warning in warnings { out.push_str(&format!("- {}\n", scalar(warning))); } } }
    out
}

fn render_search(value: serde_json::Value) -> String {
    let rows = value.as_array().cloned().unwrap_or_default();
    if rows.is_empty() { return "No matching Methodus nodes.".into(); }
    rows.into_iter().map(|item| format!("- {} · {} · {}\n  {}\n  {}", scalar(&item["id"]), scalar(&item["node_type"]), scalar(&item["status"]), scalar(&item["summary"]), scalar(&item["rationale"]))).collect::<Vec<_>>().join("\n")
}

fn render_item(value: serde_json::Value) -> String {
    let sources = value["sources"].as_array().map(|rows| rows.iter().map(|row| format!("- {}", scalar(&row["path"]))).collect::<Vec<_>>().join("\n")).unwrap_or_else(|| "none".into());
    format!("# {}\n\n- id: {}\n- type: {}\n- facet: {}\n- status: {}\n- visibility: {}\n- source: {}\n- hash: {}\n\n## Sources\n\n{}\n\n{}", scalar(&value["title"]), scalar(&value["id"]), scalar(&value["node_type"]), scalar(&value["facet"]), scalar(&value["status"]), scalar(&value["visibility"]), scalar(&value["path"]), scalar(&value["content_hash"]), sources, scalar(&value["content"]))
}

fn scalar(value: &serde_json::Value) -> String {
    value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string())
}

const CONNECTOR_SKILL: &str = include_str!("../../../resources/skills/methodus-connector/SKILL.md");

fn run_setup(runtime: &str, force: bool, uninstall: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let home = methodus_home()?;
    if !uninstall { ensure_home(&home)?; }
    let runtimes = if runtime == "all" { vec!["claude-code", "codex", "cursor"] } else { vec![runtime] };
    for runtime in runtimes {
        let target = connector_target(runtime)?;
        if uninstall {
            if !target.exists() { println!("missing connector for {runtime}: {}", target.display()); }
            else if connector_owned(&target) { std::fs::remove_file(&target)?; remove_connector_metadata(&home, runtime)?; println!("uninstalled Methodus connector for {runtime}: {}", target.display()); }
            else { println!("skipped unrelated Skill for {runtime}: {}", target.display()); }
            continue;
        }
        if target.exists() && !connector_owned(&target) { return Err(format!("refusing to overwrite unrelated Skill: {}", target.display()).into()); }
        if target.exists() && !force {
            let state = if std::fs::read_to_string(&target).ok().is_some_and(|body| body == CONNECTOR_SKILL) { "current" } else { "drifted" };
            write_connector_metadata(&home, runtime, &target, state)?;
            println!("{} is {state} (use --force to replace)", target.display());
            continue;
        }
        if let Some(parent) = target.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::write(&target, CONNECTOR_SKILL)?;
        write_connector_metadata(&home, runtime, &target, "current")?;
        println!("installed Methodus connector for {runtime}: {}", target.display());
    }
    Ok(())
}

fn connector_target(runtime: &str) -> Result<std::path::PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let user_home = std::env::var_os("HOME").map(std::path::PathBuf::from).ok_or("HOME is not set")?;
    let path = match runtime { "claude-code" => user_home.join(".claude/skills/methodus/SKILL.md"), "codex" => user_home.join(".codex/skills/methodus/SKILL.md"), "cursor" => user_home.join(".cursor/skills/methodus/SKILL.md"), other => return Err(format!("unsupported runtime: {other}").into()) };
    Ok(path)
}

fn run_doctor() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let home = methodus_home()?;
    ensure_home(&home)?;
    for check in methodus_core::health_checks(&home) { println!("{}  {}  {}", if check.ok { "ok" } else { "--" }, check.label, check.detail); }
    let store = Store::open_read_only(&home.join("state.db"))?;
    let revision = methodus_core::index_revision(&store)?;
    let validation = methodus_core::validate_graph(&home)?;
    let errors = validation.iter().filter(|issue| issue.severity == methodus_core::IssueSeverity::Error).count();
    let warnings = validation.iter().filter(|issue| issue.severity == methodus_core::IssueSeverity::Warning).count();
    println!("ok  graph/index  revision={} nodes={} errors={} warnings={}", revision, store.list_graph_nodes(None)?.len(), errors, warnings);
    for issue in validation { println!("{}  graph  {}: {}", issue.severity.as_str(), issue.path, issue.message); }
    for runtime in ["claude-code", "codex", "cursor"] {
        let target = connector_target(runtime)?;
        let state = if !target.exists() { "missing" } else if connector_owned(&target) && std::fs::read_to_string(&target).ok().is_some_and(|body| body == CONNECTOR_SKILL) { "current" } else { "drifted" };
        if state != "missing" { let _ = write_connector_metadata(&home, runtime, &target, state); }
        println!("{}  connector/{runtime}  {}", state, target.display());
    }
    Ok(())
}

fn connector_owned(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path).ok().is_some_and(|body| body.contains("name: methodus") && body.contains("x-methodus-managed: true") && body.contains("# Methodus connector"))
}

fn connector_metadata_path(home: &std::path::Path, runtime: &str) -> std::path::PathBuf { home.join("connectors").join(format!("{runtime}.yaml")) }

fn write_connector_metadata(home: &std::path::Path, runtime: &str, target: &std::path::Path, state: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = connector_metadata_path(home, runtime);
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    std::fs::write(path, format!("runtime: {runtime}\npath: {:?}\nversion: 1\nstate: {state}\nchecked_at: {:?}\n", target.display().to_string(), chrono::Utc::now().to_rfc3339()))?;
    Ok(())
}

fn remove_connector_metadata(home: &std::path::Path, runtime: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = connector_metadata_path(home, runtime);
    if path.exists() { std::fs::remove_file(path)?; }
    Ok(())
}

async fn run_tui() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let home = methodus_home()?;
    ensure_home(&home)?;
    let lock = InstanceLock::try_acquire(&home)?;
    let engine = build_engine(&home)?;
    tui::run(engine, lock).await
}

fn build_engine(
    home: &std::path::Path,
) -> Result<Engine, Box<dyn std::error::Error + Send + Sync>> {
    let store = Arc::new(Store::open(&home.join("state.db"))?);
    let mut adapters: HashMap<String, Arc<dyn RuntimeAdapter>> = HashMap::new();
    adapters.insert("cursor".to_string(), Arc::new(CursorAdapter::new()));
    adapters.insert(
        "claude-code".to_string(),
        Arc::new(ClaudeCodeAdapter::new()),
    );
    adapters.insert("codex".to_string(), Arc::new(CodexAdapter::new()));
    Ok(Engine::with_runtimes(store, home.to_path_buf(), adapters))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn connector_ownership_rejects_unrelated_skill() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("SKILL.md");
        fs::write(&path, "---\nname: other\n---\n# Other Skill\n").unwrap();
        assert!(!connector_owned(&path));
        fs::write(&path, CONNECTOR_SKILL).unwrap();
        assert!(connector_owned(&path));
    }

    #[test]
    fn agent_error_codes_are_stable() {
        assert_eq!(error_exit_code(&methodus_core::CoreError::Other("--goal cannot be empty".into())), 2);
        assert_eq!(error_exit_code(&methodus_core::CoreError::Other("--budget must be positive".into())), 2);
        assert_eq!(error_exit_code(&methodus_core::CoreError::Other("agent node not found: knowledge/missing".into())), 4);
        assert_eq!(error_exit_code(&methodus_core::CoreError::UnknownRuntime("other".into())), 5);
    }
}
