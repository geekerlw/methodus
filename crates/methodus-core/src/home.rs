//! Methodus home: first-launch seed and health checks. No CLI required.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use methodus_store::Store;

use crate::error::CoreError;

const GENERAL_METHOD_GRAPH_MD: &str = "---\nid: method/general-software\ntitle: Evidence-led software work\nnode_type: method\nkind: engineering-workflow\nstatus: committed\nvisibility: personal\nsummary: Clarify the goal, inspect evidence, make the smallest safe change, and verify the result.\ntags: [engineering, verification]\n---\n\n## Intent\n\nKeep software work evidence-led and explicit about uncertainty.\n\n## Phases\n\n1. Clarify scope and acceptance criteria.\n2. Inspect the relevant code, tests, and history.\n3. Make the smallest safe change or decision.\n4. Verify behavior and record evidence.\n\n## Execute\n\nSeparate facts from assumptions, preserve source references, and stop when the acceptance criteria are met.\n\n## Quality checks\n\nRun focused tests first, then the broadest affordable verification.\n";
const DELIBERATE_LEARNING_PROTOCOL_MD: &str =
    include_str!("../../../resources/protocols/deliberate-learning.md");
const DEFAULT_CONFIG: &str = include_str!("../../../resources/config.yaml");
#[derive(Debug, Clone)]
pub struct HealthCheck {
    pub label: String,
    pub ok: bool,
    pub required: bool,
    pub detail: String,
}

pub fn methodus_home() -> Result<PathBuf, CoreError> {
    if let Ok(val) = std::env::var("METHODUS_HOME") {
        return Ok(PathBuf::from(val));
    }
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from))
        .map_err(|_| CoreError::Other("could not determine home directory".into()))?;
    Ok(home.join(".methodus"))
}

/// Create the home tree + seed files + `state.db` if missing. Idempotent.
pub fn ensure_home(home: &Path) -> Result<(), CoreError> {
    fs::create_dir_all(home)?;
    seed_home(home)?;
    let store = Store::open(&home.join("state.db"))?;
    let _ = crate::graph::sync_graph(&store, home)?;
    Ok(())
}

fn seed_home(home: &Path) -> Result<(), CoreError> {
    for sub in [
        "protocols", "personal/knowledge", "personal/methods",
        "personal/experiences", "personal/candidates", "teams/default/knowledge", "teams/default/methods", "teams/default/experiences", "runs", "workspaces/learn", "workspaces/use", "connectors",
    ] {
        fs::create_dir_all(home.join(sub))?;
    }
    write_if_missing(home.join("personal/methods/general-software.md"), GENERAL_METHOD_GRAPH_MD)?;
    write_if_missing(home.join("protocols/deliberate-learning.md"), DELIBERATE_LEARNING_PROTOCOL_MD)?;
    write_if_missing(home.join("config.yaml"), DEFAULT_CONFIG)?;
    Ok(())
}

fn write_if_missing(path: PathBuf, body: &str) -> Result<(), CoreError> {
    if !path.exists() {
        fs::write(path, body)?;
    }
    Ok(())
}

pub fn health_checks(home: &Path) -> Vec<HealthCheck> {
    let mut out = Vec::new();
    push_file(&mut out, "state.db", &home.join("state.db"), true);
    push_file(&mut out, "config.yaml", &home.join("config.yaml"), true);
    push_file(
        &mut out,
        "method general-software",
        &home.join("personal/methods/general-software.md"),
        true,
    );
    out.push(bin_check("cursor"));
    out.push(bin_check("claude"));
    out.push(bin_check("codex"));
    out
}

fn push_file(out: &mut Vec<HealthCheck>, label: &str, path: &Path, required: bool) {
    let ok = path.exists();
    out.push(HealthCheck {
        label: label.to_string(),
        ok,
        required,
        detail: if ok {
            "ok".to_string()
        } else {
            path.display().to_string()
        },
    });
}

fn bin_check(name: &str) -> HealthCheck {
    let found = Command::new("which")
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    HealthCheck {
        label: name.to_string(),
        ok: found,
        required: false,
        detail: if found {
            "on PATH".to_string()
        } else {
            "not on PATH (optional)".to_string()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn ensure_home_is_idempotent() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("mh");
        ensure_home(&home).unwrap();
        ensure_home(&home).unwrap();
        assert!(home.join("state.db").exists());
        assert!(home.join("protocols/deliberate-learning.md").exists());
        assert!(home.join("config.yaml").exists());
        assert!(home.join("personal/methods/general-software.md").exists());
        assert!(home.join("teams/default/knowledge").is_dir());
        let checks = health_checks(&home);
        assert!(checks.iter().filter(|c| c.required).all(|c| c.ok));
    }
}
