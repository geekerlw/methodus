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
        Ok(ws_root)
    }

    /// Copy the resolved method YAML and selected SKILL.md files into the workspace.
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
            if !is_safe_segment(name) || !src.is_file() {
                continue;
            }
            let claude_dest = ws_root.join(".claude/skills").join(name);
            let methodus_dest = ws_root.join(".methodus/skills").join(name);
            fs::create_dir_all(&claude_dest)?;
            fs::create_dir_all(&methodus_dest)?;
            fs::copy(src, claude_dest.join("SKILL.md"))?;
            fs::copy(src, methodus_dest.join("SKILL.md"))?;
        }
        Ok(())
    }
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
    }
}
