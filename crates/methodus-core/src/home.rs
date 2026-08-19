//! Methodus home: first-launch seed and health checks. No CLI required.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use methodus_store::Store;

use crate::error::CoreError;

const GENERAL_FACE_YAML: &str = include_str!("../../../resources/faces/general/face.yaml");
const GENERAL_METHOD_YAML: &str = include_str!("../../../resources/methods/general-software.yaml");
const MODULE_EXPERT_METHOD_YAML: &str =
    include_str!("../../../resources/methods/module-expert-learning.yaml");
const DOC_INGEST_METHOD_YAML: &str = include_str!("../../../resources/methods/doc-ingest.yaml");
const REPO_SURVEY_METHOD_YAML: &str = include_str!("../../../resources/methods/repo-survey.yaml");
const WORKSPACE_SKILL_MD: &str =
    include_str!("../../../resources/skills/workspace-hygiene/SKILL.md");
const MODULE_EXPERT_SKILL_MD: &str =
    include_str!("../../../resources/skills/module-expert-learning/SKILL.md");
const DEFAULT_CONFIG: &str = include_str!("../../../resources/config.yaml");
const CAPSULE_KNOWLEDGE: &str = "---\nid: knowledge/context-capsule\ntitle: Task context capsule\nnode_type: knowledge\nstatus: committed\nsummary: Give an agent a small, auditable task brief plus lazy references instead of the whole knowledge base.\nscope: Any Methodus native handoff\nconfidence: 1.0\ntags: [methodus, context, token-efficiency]\nlinks:\n  used_by: [skill/learning]\n---\n\n## Learn (5W2H)\nA context capsule packages the task goal, selected knowledge facets, skills, and references for one agent run.\n\n## Decide\nUse a capsule whenever the task would otherwise need repeated background explanation or project-specific constraints.\n\n## Execute\nRead `context.md` first. Use `references.md` only for deeper evidence. Keep the initial task context bounded.\n\n## Evidence\nMethodus task workspace manifest and outcome review.\n";
const LEARNING_SKILL: &str = "---\nid: skill/learning\ntitle: Active learning session\nnode_type: skill\nstatus: committed\nsummary: Explain a concept, test understanding, then draft one atomic 5W2H knowledge note with evidence and links.\nscope: Methodus Learn tasks\nconfidence: 1.0\nlinks:\n  uses: [knowledge/context-capsule]\n---\n\n## Execute\nClarify the learner's goal and prior knowledge. Teach with examples, ask retrieval questions, distinguish facts from inferences, and write one candidate knowledge note with Learn, Decide, Execute, and Evidence facets.\n";

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
    let _ = crate::catalog::sync_catalog(&store, home)?;
    let _ = crate::graph::sync_graph(&store, home)?;
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
        "graph/knowledge",
        "graph/experiences",
        "graph/artifacts",
        "graph/faces",
        "graph/candidates",
        "graph/skills/learning",
        "faces/general",
        "faces/general/experiences",
        "faces/general/knowledge",
        "faces/general/hypotheses",
        "faces/general/notes",
        "skills/workspace-hygiene",
        "skills/module-expert-learning",
    ] {
        fs::create_dir_all(home.join(sub))?;
    }
    write_if_missing(home.join("faces/general/face.yaml"), GENERAL_FACE_YAML)?;
    write_if_missing(
        home.join("methods/general-software.yaml"),
        GENERAL_METHOD_YAML,
    )?;
    write_if_missing(
        home.join("methods/module-expert-learning.yaml"),
        MODULE_EXPERT_METHOD_YAML,
    )?;
    write_if_missing(home.join("methods/doc-ingest.yaml"), DOC_INGEST_METHOD_YAML)?;
    write_if_missing(home.join("methods/repo-survey.yaml"), REPO_SURVEY_METHOD_YAML)?;
    write_if_missing(
        home.join("skills/workspace-hygiene/SKILL.md"),
        WORKSPACE_SKILL_MD,
    )?;
    write_if_missing(
        home.join("skills/module-expert-learning/SKILL.md"),
        MODULE_EXPERT_SKILL_MD,
    )?;
    write_if_missing(home.join("config.yaml"), DEFAULT_CONFIG)?;
    write_if_missing(home.join("graph/knowledge/context-capsule.md"), CAPSULE_KNOWLEDGE)?;
    write_if_missing(home.join("graph/skills/learning/SKILL.md"), LEARNING_SKILL)?;
    write_if_missing(
        home.join("packs.yaml"),
        "# Team baseline packs (Methodus-format folders).\n\
         # Register a folder from /setup.\n\
         # How you copy folders between people is up to you.\n\
         focus:\n\
         packs: []\n",
    )?;
    write_if_missing(
        home.join("projects.yaml"),
        "# Project directories (your repos). Register from /setup.\n\
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
