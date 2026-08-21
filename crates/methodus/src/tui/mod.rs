//! Methodus maintainer studio TUI.
//!
//! The TUI prepares deliberate Learn runs and curates their results. It temporarily
//! hands its terminal to a native runtime for a focused Learn conversation, but never
//! proxies or manages ordinary coding sessions.

mod background;
mod control;

use std::io::stdout;

use crossterm::{
    event::{
        DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use methodus_core::{Engine, InstanceLock};
use ratatui::prelude::{CrosstermBackend, Terminal};

pub async fn run(engine: Engine, _lock: InstanceLock) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    stdout().execute(EnableBracketedPaste)?;
    let _ = stdout().execute(PushKeyboardEnhancementFlags(
        KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS,
    ));
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    terminal.clear()?;
    // The draw loop is synchronous; unattended learning turns are spawned onto
    // this handle so they keep running while the maintainer uses the terminal.
    let result = control::run_loop(&mut terminal, engine, tokio::runtime::Handle::current());
    disable_raw_mode()?;
    let _ = stdout().execute(PopKeyboardEnhancementFlags);
    let _ = stdout().execute(DisableBracketedPaste);
    stdout().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}
