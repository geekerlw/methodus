use std::fs;
use std::path::{Path, PathBuf};

pub struct WorkspaceBuilder;

impl WorkspaceBuilder {
    /// Create a task workspace under the given base directory.
    /// Returns the workspace root path.
    pub fn build(base: &Path, task_id: &str, context: &str) -> Result<PathBuf, std::io::Error> {
        let ws_root = base.join(task_id);
        fs::create_dir_all(ws_root.join(".methodus"))?;
        fs::create_dir_all(ws_root.join("artifacts"))?;
        fs::create_dir_all(ws_root.join("transcript"))?;
        // Write the resolved context for the executor
        fs::write(ws_root.join(".methodus/selected-context.md"), context)?;
        Ok(ws_root)
    }
}
