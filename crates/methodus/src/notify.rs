//! OS notifications from the TUI process (not a desktop app).

use std::process::{Command, Stdio};

/// Fire-and-forget system notification. Safe to call from the UI thread.
pub fn send(title: &str, body: &str) {
    let title = sanitize(title);
    let body = sanitize(body);
    if title.is_empty() || body.is_empty() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("methodus-notify".into())
        .spawn(move || fire(&title, &body));
}

fn fire(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        let script =
            format!(r#"display notification "{body}" with title "{title}" sound name "Glass""#);
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
        let _ = Command::new("notify-send")
            .args([title, body])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (title, body);
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
