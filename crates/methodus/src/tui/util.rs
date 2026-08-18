//! Shared formatting helpers for TUI layers.

use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Short summary of a tool invocation for permission prompts and OS notifications.
pub fn summarize_tool_input(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "(no details)".to_string();
    }
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return summarize_tool_json(&v);
    }
    trimmed.to_string()
}

pub fn summarize_tool_json(raw: &Value) -> String {
    match raw {
        Value::String(s) => s.clone(),
        Value::Object(map) => {
            if let Some(p) = map.get("file_path").or_else(|| map.get("path")) {
                if let Some(s) = p.as_str() {
                    return s.to_string();
                }
            }
            if let Some(cmd) = map.get("command").and_then(|c| c.as_str()) {
                return cmd.to_string();
            }
            raw.to_string()
        }
        Value::Null => "(no details)".to_string(),
        other => other.to_string(),
    }
}

/// Fit `s` to at most `max_cols` terminal columns (CJK = 2). Appends `…` when trimmed.
pub fn truncate_display(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(s) <= max_cols {
        return s.to_string();
    }
    if max_cols == 1 {
        return "…".to_string();
    }
    let budget = max_cols - 1;
    let mut cols = 0usize;
    let mut out = String::new();
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cols + w > budget {
            break;
        }
        out.push(ch);
        cols += w;
    }
    out.push('…');
    out
}

/// Byte index into `s` that is a char boundary and ≤ `byte_idx`.
pub fn floor_char_boundary(s: &str, byte_idx: usize) -> usize {
    let mut i = byte_idx.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truncate_display_cjk() {
        let s = "一二三四五六七八";
        assert_eq!(truncate_display(s, 8), "一二三…");
    }

    #[test]
    fn floor_char_boundary_mid_cjk() {
        let s = "你好";
        // byte 1 splits the first character
        assert!(!s.is_char_boundary(1));
        assert_eq!(floor_char_boundary(s, 1), 0);
    }

    #[test]
    fn bash_command_summary() {
        assert_eq!(
            summarize_tool_json(&json!({"command": "git status"})),
            "git status"
        );
    }

    #[test]
    fn file_path_summary() {
        assert_eq!(
            summarize_tool_json(&json!({"file_path": "/tmp/x"})),
            "/tmp/x"
        );
    }

    #[test]
    fn string_json_input() {
        assert_eq!(
            summarize_tool_input(r#"{"command":"cargo test"}"#),
            "cargo test"
        );
    }
}
