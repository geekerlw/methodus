//! OS notifications raised by the TUI process.
//!
//! Unattended turns finish while the maintainer is looking at something else, so
//! the terminal alone cannot carry news that needs acting on.

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

/// Fire-and-forget system notification. Safe to call from the UI thread.
pub fn send(title: &str, body: &str, urgency: NotifyUrgency) {
    let title = sanitize(title);
    let body = sanitize(body);
    if title.is_empty() || body.is_empty() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("methodus-notify".into())
        .spawn(move || fire(&title, &body, urgency));
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
}
