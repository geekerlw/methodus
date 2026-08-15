//! Permission policy: which denied tools auto-grant vs pause for the user.

use methodus_domain::PermissionDenial;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyVerdict {
    AutoAllow,
    NeedsApproval,
}

/// Reads and other low-risk inspection tools auto-run; writes, shell, and
/// network require an explicit approval (00-product.md §5).
pub fn classify_tool(tool_name: &str) -> PolicyVerdict {
    match tool_name {
        "Read" | "Glob" | "Grep" | "LS" | "TodoRead" | "TodoWrite" | "WebSearch" => {
            PolicyVerdict::AutoAllow
        }
        _ => PolicyVerdict::NeedsApproval,
    }
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
        assert_eq!(classify_tool("Write"), PolicyVerdict::NeedsApproval);
    }

    #[test]
    fn grant_tools_dedupes() {
        let mut allowed = vec!["Read".to_string()];
        grant_tools(&mut allowed, ["Read".into(), "Write".into()]);
        assert_eq!(allowed, vec!["Read".to_string(), "Write".to_string()]);
    }
}
