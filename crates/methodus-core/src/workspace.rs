use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use sha2::{Digest, Sha256};

/// One graph item selected by the context compiler. `content` is a compact facet;
/// `reference_path` always points at the complete Markdown node for lazy reading.
#[derive(Debug, Clone)]
pub struct CapsuleSelection {
    pub node_id: String,
    pub title: String,
    pub facet: String,
    pub content: String,
    pub reference_path: PathBuf,
    pub rationale: String,
    pub priority: f64,
}

/// Input to the immutable Task Workspace / context capsule compiler.
#[derive(Debug, Clone)]
pub struct CapsuleSpec {
    pub task_id: String,
    pub title: String,
    pub request: String,
    pub runtime: String,
    pub launch_cwd: PathBuf,
    pub context_budget_tokens: i64,
    pub selections: Vec<CapsuleSelection>,
    pub skills: Vec<(String, PathBuf)>,
}

#[derive(Debug, Clone)]
pub struct CompiledCapsule {
    pub root: PathBuf,
    pub brief: String,
    pub manifest_hash: String,
    pub estimated_tokens: i64,
}

/// Create a task workspace under the given base directory.
/// Returns the workspace root path.
pub struct WorkspaceBuilder;

impl WorkspaceBuilder {
    /// Build a portable, auditable task package. Unlike `build`, this does not make
    /// itself the executor CWD: native runtimes continue in `launch_cwd` and receive
    /// the short `brief.md` path.
    pub fn build_capsule(base: &Path, spec: &CapsuleSpec) -> Result<CompiledCapsule, std::io::Error> {
        validate_task_id(&spec.task_id)?;
        fs::create_dir_all(base)?;
        let base_canon = fs::canonicalize(base)?;
        let root = base.join(&spec.task_id);
        fs::create_dir_all(root.join("skills"))?;
        fs::create_dir_all(root.join("adapters"))?;
        fs::create_dir_all(root.join("artifacts"))?;
        let root_canon = fs::canonicalize(&root)?;
        if !root_canon.starts_with(&base_canon) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "capsule escaped workspace root"));
        }

        let mut selected = Vec::new();
        let mut lazy = Vec::new();
        let mut used = 0i64;
        for item in &spec.selections {
            let estimate = estimate_tokens(&item.content);
            if used + estimate <= spec.context_budget_tokens {
                used += estimate;
                selected.push((item, estimate));
            } else {
                lazy.push((item, estimate));
            }
        }
        let context = render_context(&spec.title, &spec.request, &selected);
        let references = render_references(&selected, &lazy);
        let brief = format!(
            "# Task: {}\n\n{}\n\nRead the task capsule before acting:\n- context: `{}`\n- references: `{}`\n\nUse full references only when needed; do not assume unlisted knowledge.\n",
            spec.title,
            spec.request.trim(),
            root.join("context.md").display(),
            root.join("references.md").display(),
        );
        let manifest = format!(
            "task_id: {}\ntitle: {:?}\nruntime: {}\nlaunch_cwd: {:?}\ncontext_budget_tokens: {}\nestimated_context_tokens: {}\ncompiled_at: {}\n",
            spec.task_id, spec.title, spec.runtime, spec.launch_cwd, spec.context_budget_tokens,
            used, Utc::now().to_rfc3339(),
        );
        let outcome = "# Outcome\n\n## Result\n\n## Evidence\n\n## Retrospective\n- What helped?\n- What was missing or misleading?\n- Which knowledge or skill should be updated?\n";
        fs::write(root.join("manifest.yaml"), &manifest)?;
        fs::write(root.join("brief.md"), &brief)?;
        fs::write(root.join("context.md"), context)?;
        fs::write(root.join("references.md"), references)?;
        fs::write(root.join("outcome.md"), outcome)?;
        fs::write(root.join("adapters/claude-code.md"), "Launch Claude Code in the project cwd with brief.md as the initial task prompt.\n")?;
        fs::write(root.join("adapters/codex.md"), "Launch Codex in the project cwd with brief.md as the initial task prompt.\n")?;
        for (name, source) in &spec.skills {
            if is_safe_segment(name) {
                install_skill(name, source, &root.join("skills").join(name))?;
            }
        }
        let manifest_hash = format!("{:x}", Sha256::digest(manifest.as_bytes()));
        Ok(CompiledCapsule { root, brief, manifest_hash, estimated_tokens: used })
    }
    pub fn build(base: &Path, task_id: &str, context: &str) -> Result<PathBuf, std::io::Error> {
        validate_task_id(task_id)?;

        fs::create_dir_all(base)?;
        let base_canon = fs::canonicalize(base)?;

        let ws_root = base.join(task_id);
        fs::create_dir_all(ws_root.join(".methodus"))?;
        fs::create_dir_all(ws_root.join("artifacts"))?;
        fs::create_dir_all(ws_root.join("transcript"))?;
        fs::create_dir_all(ws_root.join("face-context"))?;

        let ws_canon = fs::canonicalize(&ws_root)?;
        if !ws_canon.starts_with(&base_canon) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "workspace {} escaped base {}",
                    ws_canon.display(),
                    base_canon.display()
                ),
            ));
        }

        fs::write(ws_root.join(".methodus/selected-context.md"), context)?;
        write_runtime_guides(&ws_root)?;
        Ok(ws_root)
    }

    /// Snapshot of which Face notes/knowledge this turn injected (`(none)` if empty).
    pub fn write_injected(ws_root: &Path, inventory: &str) -> Result<(), std::io::Error> {
        fs::write(ws_root.join(".methodus/injected.md"), inventory)
    }

    /// Write execution plan from resolved method steps and Faces.
    pub fn write_plan(ws_root: &Path, plan_md: &str) -> Result<(), std::io::Error> {
        fs::write(ws_root.join(".methodus/plan.md"), plan_md)
    }

    /// Copy a few vetted knowledge files into the workspace (never the whole Face store).
    pub fn materialize_knowledge(
        ws_root: &Path,
        files: &[(String, PathBuf)],
    ) -> Result<(), std::io::Error> {
        let dest_dir = ws_root.join("face-context").join("knowledge");
        fs::create_dir_all(&dest_dir)?;
        for (name, src) in files {
            if !src.is_file() {
                continue;
            }
            let rel = Path::new(name);
            let stem = rel
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if stem.is_empty() || !is_safe_segment(&stem) {
                continue;
            }
            if rel.extension().is_some_and(|e| e != "md") {
                continue;
            }
            let dest = if rel.parent().is_some_and(|p| !p.as_os_str().is_empty()) {
                dest_dir.join(rel)
            } else {
                dest_dir.join(format!("{stem}.md"))
            };
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(src, &dest)?;
        }
        Ok(())
    }

    /// Copy the resolved method YAML and selected skill packages into the workspace.
    /// Skills are placed where Claude Code looks (`./.claude/skills/`) and under `.methodus/`.
    pub fn materialize_resolution(
        ws_root: &Path,
        method_src: Option<&Path>,
        skills: &[(String, PathBuf)],
    ) -> Result<(), std::io::Error> {
        if let Some(src) = method_src {
            if src.is_file() {
                fs::copy(src, ws_root.join(".methodus/method.yaml"))?;
            }
        }
        for (name, src) in skills {
            if !is_safe_segment(name) {
                continue;
            }
            let claude_dest = ws_root.join(".claude/skills").join(name);
            let methodus_dest = ws_root.join(".methodus/skills").join(name);
            install_skill(name, src, &claude_dest)?;
            install_skill(name, src, &methodus_dest)?;
        }
        write_runtime_guides(ws_root)?;
        Ok(())
    }
}

fn estimate_tokens(text: &str) -> i64 {
    ((text.chars().count() + 3) / 4) as i64
}

fn render_context(title: &str, request: &str, selected: &[(&CapsuleSelection, i64)]) -> String {
    let mut out = format!("# Selected context\n\nTask: {title}\n\n{request}\n");
    for (item, estimate) in selected {
        out.push_str(&format!("\n## {} · {}\n\n{}\n\n_Why selected: {} · ~{} tokens_\n", item.title, item.facet, item.content.trim(), item.rationale, estimate));
    }
    out
}

fn render_references(selected: &[(&CapsuleSelection, i64)], lazy: &[(&CapsuleSelection, i64)]) -> String {
    let mut out = String::from("# Full references\n\nThese files are deliberately not all startup context. Read only when needed.\n");
    for (item, estimate) in selected.iter().chain(lazy.iter()) {
        out.push_str(&format!("- `{}` — {} ({}, ~{} tokens; {})\n", item.reference_path.display(), item.node_id, item.facet, estimate, if lazy.iter().any(|(lazy_item, _)| lazy_item.node_id == item.node_id) { "lazy" } else { "selected" }));
    }
    out
}

const RUNTIME_GUIDE: &str = "\
# Methodus task workspace

This directory is an isolated sandbox for one task. The user's source trees are
not copied here.

- Follow `.methodus/selected-context.md`.
- Directories listed under **Readable directories** are the real folders on disk.
  Read / Glob / LS them in place (they are also passed as extra dirs to the CLI).
- `.methodus/injected.md` lists which Face notes/knowledge were selected this turn.
- Vetted Face notes (if any) live in `face-context/knowledge/` — prefer them over guessing.
- Project skills live in `.claude/skills/<name>/SKILL.md` (copy also under `.methodus/skills/`).
- If you have a Skill tool, invoke each listed skill by name before improvising.
- Otherwise read those SKILL.md files and follow them.
";

fn write_runtime_guides(ws_root: &Path) -> Result<(), std::io::Error> {
    fs::write(ws_root.join("CLAUDE.md"), RUNTIME_GUIDE)?;
    fs::write(ws_root.join("AGENTS.md"), RUNTIME_GUIDE)?;
    Ok(())
}

fn install_skill(name: &str, src: &Path, dest: &Path) -> Result<(), std::io::Error> {
    fs::create_dir_all(dest)?;
    if src.is_dir() {
        return copy_skill_tree(src, dest);
    }
    if !src.is_file() {
        return Ok(());
    }
    let parent = src.parent();
    let parent_is_package = parent
        .and_then(|p| p.file_name())
        .is_some_and(|n| n == name);
    if parent_is_package {
        if let Some(p) = parent {
            return copy_skill_tree(p, dest);
        }
    }
    fs::copy(src, dest.join("SKILL.md"))?;
    if let Some(p) = parent {
        for extra in ["references", "scripts", "assets"] {
            let from = p.join(extra);
            if from.is_dir() {
                copy_skill_tree(&from, &dest.join(extra))?;
            }
        }
    }
    Ok(())
}

fn copy_skill_tree(src: &Path, dest: &Path) -> Result<(), std::io::Error> {
    fs::create_dir_all(dest)?;
    let Ok(entries) = fs::read_dir(src) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == ".git" || name_str == "node_modules" || name_str == "target" {
            continue;
        }
        let from = entry.path();
        let to = dest.join(&name);
        if from.is_dir() {
            copy_skill_tree(&from, &to)?;
        } else if from.is_file() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

pub fn validate_task_id(task_id: &str) -> Result<(), std::io::Error> {
    if !is_safe_segment(task_id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unsafe task id: {task_id}"),
        ));
    }
    Ok(())
}

/// Task ids (and other path segments) must be a single non-empty component.
pub fn is_safe_segment(id: &str) -> bool {
    if id.is_empty() || id.len() > 128 || id == "." || id == ".." {
        return false;
    }
    id.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn rejects_path_traversal() {
        assert!(validate_task_id("../etc").is_err());
        assert!(validate_task_id("foo/bar").is_err());
        assert!(validate_task_id("..").is_err());
        assert!(validate_task_id("").is_err());
        assert!(validate_task_id("task_abc123def456").is_ok());
    }

    #[test]
    fn build_writes_context_under_base() {
        let dir = tempdir().unwrap();
        let ws = WorkspaceBuilder::build(dir.path(), "task_abc123def456", "# hello").unwrap();
        assert!(ws.starts_with(dir.path()));
        let ctx = fs::read_to_string(ws.join(".methodus/selected-context.md")).unwrap();
        assert_eq!(ctx, "# hello");
        assert!(ws.join("transcript").is_dir());
    }

    #[test]
    fn build_rejects_unsafe_id() {
        let dir = tempdir().unwrap();
        let err = WorkspaceBuilder::build(dir.path(), "../escape", "x").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn materialize_copies_method_and_skill() {
        let dir = tempdir().unwrap();
        let ws = WorkspaceBuilder::build(dir.path(), "task_abc123def456", "# ctx").unwrap();
        let method = dir.path().join("m.yaml");
        fs::write(&method, "id: general-software\n").unwrap();
        let skill = dir.path().join("SKILL.md");
        fs::write(&skill, "---\nname: workspace-hygiene\n---\n").unwrap();
        WorkspaceBuilder::materialize_resolution(
            &ws,
            Some(&method),
            &[("workspace-hygiene".to_string(), skill)],
        )
        .unwrap();
        assert!(ws.join(".methodus/method.yaml").is_file());
        assert!(ws
            .join(".claude/skills/workspace-hygiene/SKILL.md")
            .is_file());
        assert!(ws
            .join(".methodus/skills/workspace-hygiene/SKILL.md")
            .is_file());
        assert!(ws.join("CLAUDE.md").is_file());
        assert!(ws.join("AGENTS.md").is_file());
    }

    #[test]
    fn materialize_copies_skill_scripts() {
        let dir = tempdir().unwrap();
        let ws = WorkspaceBuilder::build(dir.path(), "task_abc123def456", "# ctx").unwrap();
        let pkg = dir.path().join("skills/tcp-debug");
        fs::create_dir_all(pkg.join("scripts")).unwrap();
        fs::write(
            pkg.join("SKILL.md"),
            "---\nname: tcp-debug\ndescription: debug tcp\n---\n",
        )
        .unwrap();
        fs::write(pkg.join("scripts/capture.sh"), "#!/bin/sh\necho ok\n").unwrap();
        WorkspaceBuilder::materialize_resolution(
            &ws,
            None,
            &[("tcp-debug".to_string(), pkg.join("SKILL.md"))],
        )
        .unwrap();
        assert!(ws
            .join(".claude/skills/tcp-debug/scripts/capture.sh")
            .is_file());
    }

    #[test]
    fn capsule_keeps_over_budget_knowledge_lazy() {
        let dir = tempdir().unwrap();
        let full = dir.path().join("full.md");
        fs::write(&full, "# full").unwrap();
        let capsule = WorkspaceBuilder::build_capsule(dir.path(), &CapsuleSpec {
            task_id: "task_capsule".into(), title: "Example".into(), request: "Do work".into(),
            runtime: "claude-code".into(), launch_cwd: dir.path().to_path_buf(), context_budget_tokens: 3,
            selections: vec![CapsuleSelection { node_id: "knowledge/one".into(), title: "One".into(), facet: "Execute".into(), content: "one two three four five six seven eight".into(), reference_path: full, rationale: "test".into(), priority: 1.0 }],
            skills: Vec::new(),
        }).unwrap();
        assert!(capsule.root.join("brief.md").is_file());
        assert!(fs::read_to_string(capsule.root.join("references.md")).unwrap().contains("lazy"));
    }
}
