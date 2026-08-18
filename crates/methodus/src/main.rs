//! Methodus binary: open the TUI. First launch seeds `~/.methodus`.

use std::collections::HashMap;
use std::sync::Arc;

use clap::Parser;

use methodus_core::{ensure_home, methodus_home, Engine, InstanceLock};
use methodus_runtime::{ClaudeCodeAdapter, CodexAdapter, CursorAdapter, RuntimeAdapter};
use methodus_store::Store;

mod notify;
mod tui;

#[derive(Parser)]
#[command(
    name = "methodus",
    version,
    about = "Persistent Personal Expert System",
    long_about = "Opens the Methodus TUI. Keep this process running (e.g. in tmux).\n\
                  First launch creates ~/.methodus. Projects, packs, and settings live in /setup."
)]
struct Cli {}

#[tokio::main]
async fn main() {
    let _ = Cli::parse();
    if let Err(e) = run_tui().await {
        eprintln!("\x1b[31mError:\x1b[0m {e}");
        std::process::exit(1);
    }
}

async fn run_tui() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let home = methodus_home()?;
    ensure_home(&home)?;
    let lock = InstanceLock::try_acquire(&home)?;
    let engine = build_engine(&home)?;
    let recovered = engine.recover().await?;
    tui::run(engine, lock, recovered).await
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
