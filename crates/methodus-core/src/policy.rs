//! Permission policy: which denied tools auto-grant vs pause for the user.

use methodus_domain::PermissionDenial;

/// Methodus-side permission mode. Each adapter maps this to its own CLI flags.
/// Daily default is `AcceptEdits` (few prompts). Never a full bypass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    /// File edits auto-run; shell / network / global config still pause.
    AcceptEdits,
    /// Read-only / analyze. No writes.
    Plan,
    /// Ask on writes and other side effects (Claude `manual` + Methodus approval).
    Cautious,
}

impl PermissionMode {
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.unwrap_or("acceptEdits") {
            "plan" => Self::Plan,
            "cautious" | "manual" | "default" => Self::Cautious,
            _ => Self::AcceptEdits,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AcceptEdits => "acceptEdits",
            Self::Plan => "plan",
            Self::Cautious => "cautious",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::AcceptEdits => Self::Plan,
            Self::Plan => Self::Cautious,
            Self::Cautious => Self::AcceptEdits,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::AcceptEdits => "edits (shell still asks)",
            Self::Plan => "plan (read-only)",
            Self::Cautious => "ask (writes + shell)",
        }
    }

    pub fn claude_flag(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::AcceptEdits => "acceptEdits",
            Self::Cautious => "manual",
        }
    }

    /// Cursor has no mid-turn callback. Never `--force`.
    pub fn cursor_flag(self) -> &'static str {
        match self {
            Self::Plan => "--plan",
            Self::AcceptEdits | Self::Cautious => "--auto-review",
        }
    }

    pub fn codex_sandbox(self) -> &'static str {
        match self {
            Self::Plan => "read-only",
            Self::AcceptEdits | Self::Cautious => "workspace-write",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyVerdict {
    AutoAllow,
    NeedsApproval,
}

/// Reads, skill load, and other low-risk inspection tools auto-run; writes, shell, and
/// network require an explicit approval (00-product.md §5).
pub fn classify_tool(tool_name: &str) -> PolicyVerdict {
    match tool_name {
        "Read" | "Glob" | "Grep" | "LS" | "TodoRead" | "TodoWrite" | "WebSearch" | "Skill" => {
            PolicyVerdict::AutoAllow
        }
        _ => PolicyVerdict::NeedsApproval,
    }
}

/// Tools granted on every spawn so the executor can load selected skills and inspect
/// the workspace without waiting for a denial round-trip.
pub fn baseline_allowed_tools() -> Vec<String> {
    vec![
        "Skill".to_string(),
        "Read".to_string(),
        "Glob".to_string(),
        "Grep".to_string(),
        "LS".to_string(),
        "TodoRead".to_string(),
        "TodoWrite".to_string(),
    ]
}

pub fn split_denials(
    denials: &[PermissionDenial],
) -> (Vec<PermissionDenial>, Vec<PermissionDenial>) {
    let mut auto = Vec::new();
    let mut user = Vec::new();
    for d in denials {
        match classify_tool(&d.tool_name) {
            PolicyVerdict::AutoAllow => auto.push(d.clone()),
            PolicyVerdict::NeedsApproval => user.push(d.clone()),
        }
    }
    (auto, user)
}

pub fn grant_tools(allowed: &mut Vec<String>, tools: impl IntoIterator<Item = String>) {
    for name in tools {
        if !allowed.iter().any(|t| t == &name) {
            allowed.push(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_is_auto_bash_needs_user() {
        assert_eq!(classify_tool("Read"), PolicyVerdict::AutoAllow);
        assert_eq!(classify_tool("Bash"), PolicyVerdict::NeedsApproval);
        assert_eq!(classify_tool("Skill"), PolicyVerdict::AutoAllow);
        assert_eq!(classify_tool("Write"), PolicyVerdict::NeedsApproval);
    }

    #[test]
    fn grant_tools_dedupes() {
        let mut allowed = vec!["Read".to_string()];
        grant_tools(&mut allowed, ["Read".into(), "Write".into()]);
        assert_eq!(allowed, vec!["Read".to_string(), "Write".to_string()]);
    }

    #[test]
    fn permission_mode_maps_and_never_bypasses() {
        assert_eq!(PermissionMode::parse(None), PermissionMode::AcceptEdits);
        assert_eq!(
            PermissionMode::parse(Some("default")),
            PermissionMode::Cautious
        );
        assert_eq!(PermissionMode::AcceptEdits.claude_flag(), "acceptEdits");
        assert_eq!(PermissionMode::Cautious.claude_flag(), "manual");
        assert_eq!(PermissionMode::Plan.cursor_flag(), "--plan");
        assert_eq!(PermissionMode::AcceptEdits.cursor_flag(), "--auto-review");
        assert_eq!(PermissionMode::Cautious.cursor_flag(), "--auto-review");
        assert_eq!(PermissionMode::Plan.codex_sandbox(), "read-only");
        assert_eq!(PermissionMode::AcceptEdits.next(), PermissionMode::Plan);
        assert_eq!(PermissionMode::Plan.next(), PermissionMode::Cautious);
        assert_eq!(PermissionMode::Cautious.next(), PermissionMode::AcceptEdits);
        for mode in [
            PermissionMode::AcceptEdits,
            PermissionMode::Plan,
            PermissionMode::Cautious,
        ] {
            assert_ne!(mode.claude_flag(), "bypassPermissions");
            assert_ne!(mode.cursor_flag(), "--force");
            assert_ne!(mode.codex_sandbox(), "danger-full-access");
        }
    }
}
