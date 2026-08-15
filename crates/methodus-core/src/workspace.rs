use std::fs;
use std::path::{Path, PathBuf};

/// Create a task workspace under the given base directory.
/// Returns the workspace root path.
pub struct WorkspaceBuilder;

impl WorkspaceBuilder {
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
            let stem = Path::new(name)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !is_safe_segment(&stem) || Path::new(name).extension().is_some_and(|e| e != "md") {
                continue;
            }
            fs::copy(src, dest_dir.join(format!("{stem}.md")))?;
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

const RUNTIME_GUIDE: &str = "\
# Methodus task workspace

This directory is an isolated sandbox for one task. The user's source trees are
not copied here.

- Follow `.methodus/selected-context.md`.
- Directories listed under **Readable directories** are the real folders on disk.
  Read / Glob / LS them in place (they are also passed as extra dirs to the CLI).
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
}
