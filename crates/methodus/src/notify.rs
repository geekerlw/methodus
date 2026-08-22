//! Notifications raised by the TUI process.
//!
//! Unattended turns finish while the maintainer is looking at something else, so
//! the terminal alone cannot carry news that needs acting on. When the TUI is
//! running in Ghostty, the terminal itself owns the notification. That keeps the
//! notification attached to the current terminal surface instead of attributing
//! it to the `osascript` process.

use std::io::{self, IsTerminal, Write};
use std::process::{Command, Stdio};

/// Urgency tier — controls sound (macOS) and notify-send level (Linux).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyUrgency {
    /// Approval blocked, executor error — may play a sound when OS notify fires.
    Critical,
    /// Inbox item, idle question — silent OS banner.
    Normal,
    /// Background fact worth knowing but never worth acting on immediately.
    Low,
}

/// Raise a notification. Safe to call from the UI thread.
///
/// Ghostty understands OSC 9 notifications sent through the current PTY. They
/// are preferable to invoking `osascript` because Ghostty can associate the
/// notification with the terminal surface that produced it. Other terminals
/// keep the platform-specific fallback below.
pub fn send(title: &str, body: &str, urgency: NotifyUrgency) {
    let title = sanitize(title);
    let body = sanitize(body);
    if title.is_empty() || body.is_empty() {
        return;
    }

    if supports_ghostty_notifications() && send_ghostty(&title, &body) {
        return;
    }

    let _ = std::thread::Builder::new()
        .name("methodus-notify".into())
        .spawn(move || fire(&title, &body, urgency));
}

fn supports_ghostty_notifications() -> bool {
    if !io::stdout().is_terminal() {
        return false;
    }

    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let term = std::env::var("TERM").unwrap_or_default();
    let has_ghostty_marker = [
        "GHOSTTY_RESOURCES_DIR",
        "GHOSTTY_BIN_DIR",
        "GHOSTTY_SHELL_FEATURES",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some());
    is_ghostty_environment(&term_program, &term, has_ghostty_marker)
}

fn is_ghostty_environment(term_program: &str, term: &str, has_ghostty_marker: bool) -> bool {
    term_program.trim().eq_ignore_ascii_case("ghostty")
        || term.trim().eq_ignore_ascii_case("xterm-ghostty")
        || has_ghostty_marker
}

fn send_ghostty(title: &str, body: &str) -> bool {
    // OSC 9 has one text field. Prefixing the body with the Methodus title
    // preserves the semantic distinction that the OS notification path used to
    // provide, while Ghostty remains the visible notification owner.
    let sequence = ghostty_sequence(title, body);
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(sequence.as_bytes())
        .and_then(|_| stdout.flush())
        .is_ok()
}

fn ghostty_sequence(title: &str, body: &str) -> String {
    format!("\x1b]9;{title}: {body}\x1b\\")
}

fn fire(title: &str, body: &str, urgency: NotifyUrgency) {
    #[cfg(target_os = "macos")]
    {
        let sound = match urgency {
            NotifyUrgency::Critical => r#" sound name "Glass""#,
            NotifyUrgency::Normal | NotifyUrgency::Low => "",
        };
        let script = format!(r#"display notification "{body}" with title "{title}"{sound}"#);
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(target_os = "linux")]
    {
        let level = match urgency {
            NotifyUrgency::Critical => "critical",
            NotifyUrgency::Normal => "normal",
            NotifyUrgency::Low => "low",
        };
        let _ = Command::new("notify-send")
            .args(["-u", level, title, body])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (title, body, urgency);
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '"' | '\\' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .take(160)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_quotes_and_controls() {
        assert_eq!(sanitize("needs \"approval\"\nnow"), "needs  approval  now");
        assert!(sanitize("").is_empty());
    }

    #[test]
    fn ghostty_notification_uses_osc9_and_string_terminator() {
        assert_eq!(
            ghostty_sequence("Methodus", "open /attention"),
            "\x1b]9;Methodus: open /attention\x1b\\"
        );
    }

    #[test]
    fn ghostty_is_detected_from_standard_terminal_markers() {
        assert!(is_ghostty_environment("ghostty", "xterm-256color", false));
        assert!(is_ghostty_environment("", "xterm-ghostty", false));
        assert!(is_ghostty_environment("", "screen", true));
        assert!(!is_ghostty_environment("Apple_Terminal", "xterm-256color", false));
    }
}
