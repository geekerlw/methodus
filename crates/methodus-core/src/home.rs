//! Methodus home: first-launch seed and health checks. No CLI required.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use methodus_store::Store;

use crate::error::CoreError;

const GENERAL_FACE_YAML: &str = include_str!("../../../resources/faces/general/face.yaml");
const GENERAL_METHOD_YAML: &str = include_str!("../../../resources/methods/general-software.yaml");
const WORKSPACE_SKILL_MD: &str =
    include_str!("../../../resources/skills/workspace-hygiene/SKILL.md");
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
    Store::open(&home.join("state.db"))?;
    Ok(())
}

fn seed_home(home: &Path) -> Result<(), CoreError> {
    for sub in [
        "faces",
        "methods",
        "skills",
        "projects",
        "packs",
        "workspaces",
        "faces/general",
        "faces/general/experiences",
        "faces/general/knowledge",
        "faces/general/hypotheses",
        "skills/workspace-hygiene",
    ] {
        fs::create_dir_all(home.join(sub))?;
    }
    write_if_missing(home.join("faces/general/face.yaml"), GENERAL_FACE_YAML)?;
    write_if_missing(
        home.join("methods/general-software.yaml"),
        GENERAL_METHOD_YAML,
    )?;
    write_if_missing(
        home.join("skills/workspace-hygiene/SKILL.md"),
        WORKSPACE_SKILL_MD,
    )?;
    write_if_missing(home.join("config.yaml"), DEFAULT_CONFIG)?;
    write_if_missing(
        home.join("packs.yaml"),
        "# Team baseline packs (Methodus-format folders).\n\
         # Register a folder from the TUI setup page.\n\
         # How you copy folders between people is up to you.\n\
         focus:\n\
         packs: []\n",
    )?;
    write_if_missing(
        home.join("projects.yaml"),
        "# Project directories (your repos). Register from the TUI setup page.\n\
         focus:\n\
         projects: []\n",
    )?;
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
        "face general",
        &home.join("faces/general/face.yaml"),
        true,
    );
    push_file(
        &mut out,
        "method general-software",
        &home.join("methods/general-software.yaml"),
        true,
    );
    push_file(
        &mut out,
        "skill workspace-hygiene",
        &home.join("skills/workspace-hygiene/SKILL.md"),
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
        assert!(home.join("faces/general/face.yaml").exists());
        assert!(home.join("config.yaml").exists());
        assert!(home.join("packs.yaml").exists());
        assert!(home.join("projects.yaml").exists());
        let checks = health_checks(&home);
        assert!(checks.iter().filter(|c| c.required).all(|c| c.ok));
    }
}
