//! Methodus control-plane TUI.
//!
//! It compiles a capsule, gives the terminal to the native Agent, then returns
//! only for outcome capture and graph review.

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
use methodus_core::{Engine, InstanceLock, RecoveredSession};
use ratatui::prelude::{CrosstermBackend, Terminal};

pub async fn run(engine: Engine, _lock: InstanceLock, _recovered: Vec<RecoveredSession>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    stdout().execute(EnableBracketedPaste)?;
    let _ = stdout().execute(PushKeyboardEnhancementFlags(
        KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS,
    ));
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    terminal.clear()?;
    let result = control::run_loop(&mut terminal, engine);
    disable_raw_mode()?;
    let _ = stdout().execute(PopKeyboardEnhancementFlags);
    let _ = stdout().execute(DisableBracketedPaste);
    stdout().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}
