//! Permission policy: which denied tools auto-grant vs pause for the user.

use methodus_domain::PermissionDenial;

/// Methodus-side permission mode. Each adapter maps this to its own CLI flags.
/// Daily default is `AcceptEdits` (goal mode). Never a full bypass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    /// Goal mode: routine tools auto-run; destructive ops still pause.
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
            Self::AcceptEdits => "auto (destructive asks)",
            Self::Plan => "plan (read-only)",
            Self::Cautious => "ask (writes + shell)",
        }
    }

    pub fn claude_flag(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::AcceptEdits => "auto",
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

/// Classify a permission denial from the executor (includes tool input for risk checks).
pub fn classify_denial(d: &PermissionDenial, mode: PermissionMode) -> PolicyVerdict {
    match mode {
        PermissionMode::AcceptEdits => {
            if is_high_risk_denial(d) {
                PolicyVerdict::NeedsApproval
            } else {
                PolicyVerdict::AutoAllow
            }
        }
        PermissionMode::Plan | PermissionMode::Cautious => {
            if is_read_tool(&d.tool_name) {
                PolicyVerdict::AutoAllow
            } else {
                PolicyVerdict::NeedsApproval
            }
        }
    }
}

fn is_high_risk_denial(d: &PermissionDenial) -> bool {
    if matches!(
        d.tool_name.as_str(),
        "Delete" | "Remove" | "Trash" | "delete_file" | "RemoveFile"
    ) {
        return true;
    }
    let scan = denial_scan_text(&d.tool_name, &d.tool_input);
    is_high_risk_text(&scan)
}

fn denial_scan_text(tool_name: &str, input: &serde_json::Value) -> String {
    let mut parts = vec![tool_name.to_string()];
    collect_scan_strings(input, &mut parts);
    parts.join(" ")
}

fn collect_scan_strings(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(items) => {
            for item in items {
                collect_scan_strings(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                out.push(k.clone());
                collect_scan_strings(v, out);
            }
        }
        _ => {}
    }
}

/// High-risk: delete/rename files, remove environments, install/uninstall system software.
pub fn is_high_risk_text(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let normalized = lower
        .replace('\n', " ")
        .replace('\t', " ")
        .replace('&', " ; ")
        .replace('|', " ; ");
    for segment in normalized.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        if is_high_risk_shell_segment(segment) {
            return true;
        }
    }
    is_high_risk_shell_segment(normalized.trim())
}

fn is_high_risk_shell_segment(segment: &str) -> bool {
    let s = segment.trim();
    if s.is_empty() {
        return false;
    }
    if has_token(s, "rm")
        || has_token(s, "rmdir")
        || has_token(s, "unlink")
        || has_token(s, "shred")
        || has_token(s, "truncate")
    {
        return true;
    }
    if has_token(s, "mv") || s.contains("git mv") {
        return true;
    }
    if s.contains("git clean") || s.contains("git reset --hard") || s.contains("git reset -hard")
    {
        return true;
    }
    if s.contains("docker rm")
        || s.contains("docker rmi")
        || s.contains("docker system prune")
        || s.contains("docker compose down")
        || s.contains("docker-compose down")
    {
        return true;
    }
    if s.contains("kubectl delete") || s.contains("terraform destroy") {
        return true;
    }
    if s.contains("conda env remove")
        || s.contains("conda remove --all")
        || s.contains("mamba env remove")
    {
        return true;
    }
    if s.contains("pip uninstall") || s.contains("pip3 uninstall") {
        return true;
    }
    if s.contains("npm uninstall")
        || s.contains("npm remove")
        || s.contains("pnpm remove")
        || s.contains("pnpm uninstall")
        || s.contains("yarn remove")
    {
        return true;
    }
    if s.contains("npm install -g")
        || s.contains("npm i -g")
        || s.contains("pnpm add -g")
        || s.contains("yarn global add")
    {
        return true;
    }
    if s.contains("cargo uninstall") {
        return true;
    }
    if is_pkg_install_or_remove(s, "apt-get")
        || is_pkg_install_or_remove(s, "apt")
        || is_pkg_install_or_remove(s, "yum")
        || is_pkg_install_or_remove(s, "dnf")
        || is_pkg_install_or_remove(s, "apk")
        || is_pkg_install_or_remove(s, "pacman")
        || is_pkg_install_or_remove(s, "brew")
        || is_pkg_install_or_remove(s, "snap")
        || is_pkg_install_or_remove(s, "choco")
        || is_pkg_install_or_remove(s, "winget")
        || is_pkg_install_or_remove(s, "gem")
    {
        return true;
    }
    false
}

fn is_pkg_install_or_remove(segment: &str, pkg_mgr: &str) -> bool {
    if !has_token(segment, pkg_mgr) {
        return false;
    }
    segment.contains(" install")
        || segment.contains(" remove")
        || segment.contains(" uninstall")
        || segment.contains(" purge")
        || segment.contains(" erase")
        || segment.contains(" autoremove")
}

fn has_token(segment: &str, token: &str) -> bool {
    segment
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
        .any(|w| w == token)
}

fn is_read_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "Read" | "Glob" | "Grep" | "LS" | "TodoRead" | "TodoWrite" | "WebSearch" | "Skill"
    )
}

/// Tools granted on every spawn so the executor can load skills and inspect the workspace
/// without waiting for a denial round-trip.
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

/// Pre-authorize tools at spawn time so goal-mode turns need fewer round-trips.
pub fn spawn_allowed_tools(mode: PermissionMode) -> Vec<String> {
    let mut allowed = baseline_allowed_tools();
    if mode == PermissionMode::AcceptEdits {
        grant_tools(
            &mut allowed,
            [
                "Write".to_string(),
                "Edit".to_string(),
                "MultiEdit".to_string(),
                "NotebookEdit".to_string(),
                "Bash".to_string(),
                "WebFetch".to_string(),
            ],
        );
    }
    allowed
}

pub fn split_denials(
    denials: &[PermissionDenial],
    mode: PermissionMode,
) -> (Vec<PermissionDenial>, Vec<PermissionDenial>) {
    let mut auto = Vec::new();
    let mut user = Vec::new();
    for d in denials {
        match classify_denial(d, mode) {
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

    fn denial(tool: &str, input: serde_json::Value) -> PermissionDenial {
        PermissionDenial {
            tool_name: tool.to_string(),
            tool_use_id: None,
            tool_input: input,
        }
    }

    #[test]
    fn goal_mode_auto_allows_routine_bash() {
        let mode = PermissionMode::AcceptEdits;
        assert_eq!(
            classify_denial(
                &denial("Bash", serde_json::json!({"command": "git status"})),
                mode
            ),
            PolicyVerdict::AutoAllow
        );
        assert_eq!(
            classify_denial(
                &denial("Bash", serde_json::json!({"command": "cargo test -p methodus"})),
                mode
            ),
            PolicyVerdict::AutoAllow
        );
        assert_eq!(
            classify_denial(
                &denial("Bash", serde_json::json!({"command": "npm install"})),
                mode
            ),
            PolicyVerdict::AutoAllow
        );
        assert_eq!(
            classify_denial(
                &denial("Write", serde_json::json!({"file_path": "src/main.rs"})),
                mode
            ),
            PolicyVerdict::AutoAllow
        );
        assert_eq!(
            classify_denial(
                &denial("WebFetch", serde_json::json!({"url": "https://example.com"})),
                mode
            ),
            PolicyVerdict::AutoAllow
        );
    }

    #[test]
    fn goal_mode_blocks_destructive_ops() {
        let mode = PermissionMode::AcceptEdits;
        for cmd in [
            "rm -rf node_modules",
            "mv old.rs new.rs",
            "git mv a b",
            "brew install ripgrep",
            "brew uninstall ripgrep",
            "apt-get remove curl",
            "npm uninstall lodash",
            "npm install -g typescript",
            "pip uninstall requests",
            "docker rm mycontainer",
            "conda env remove -n dev",
            "kubectl delete pod x",
        ] {
            assert_eq!(
                classify_denial(&denial("Bash", serde_json::json!({"command": cmd})), mode),
                PolicyVerdict::NeedsApproval,
                "expected block: {cmd}"
            );
        }
    }

    #[test]
    fn cautious_still_asks_for_bash() {
        let mode = PermissionMode::Cautious;
        assert_eq!(
            classify_denial(
                &denial("Bash", serde_json::json!({"command": "git status"})),
                mode
            ),
            PolicyVerdict::NeedsApproval
        );
        assert_eq!(
            classify_denial(
                &denial("Read", serde_json::json!({"path": "/tmp/a"})),
                mode
            ),
            PolicyVerdict::AutoAllow
        );
    }

    #[test]
    fn spawn_allowed_includes_bash_in_accept_edits() {
        let tools = spawn_allowed_tools(PermissionMode::AcceptEdits);
        assert!(tools.iter().any(|t| t == "Write"));
        assert!(tools.iter().any(|t| t == "Bash"));
        assert!(tools.iter().any(|t| t == "WebFetch"));
        assert!(!spawn_allowed_tools(PermissionMode::Plan)
            .iter()
            .any(|t| t == "Bash"));
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
        assert_eq!(PermissionMode::AcceptEdits.claude_flag(), "auto");
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

    #[test]
    fn risk_scan_chained_commands() {
        assert!(is_high_risk_text("git status && rm -rf /tmp/x"));
        assert!(!is_high_risk_text("git status && cargo test"));
    }
}
