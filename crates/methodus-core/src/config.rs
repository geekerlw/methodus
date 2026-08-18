//! User config from `<METHODUS_HOME>/config.yaml`. Edited from `/setup`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::policy::PermissionMode;

const RUNTIMES: &[&str] = &["claude-code", "cursor", "codex"];

/// Settings loaded from Methodus home. Missing file or keys use defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_face: Option<String>,
    /// Additional Faces whose committed knowledge is injected alongside the primary Face.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_faces: Option<Vec<String>>,
    /// Per-task executor cwd root. Unset → `<home>/workspaces`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
    /// OS notifications when Methodus needs you (approval, your turn).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<bool>,
}

impl UserConfig {
    pub fn load(home: &Path) -> Self {
        let path = home.join("config.yaml");
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_yaml::from_str(&raw).unwrap_or_default()
    }

    pub fn save(&self, home: &Path) -> Result<(), std::io::Error> {
        std::fs::create_dir_all(home)?;
        let body = serde_yaml::to_string(self)
            .unwrap_or_else(|_| "default_runtime: claude-code\n".to_string());
        std::fs::write(home.join("config.yaml"), body)
    }

    pub fn resolve_workspace_root(&self, home: &Path) -> PathBuf {
        match self.workspace_root.as_deref().map(str::trim) {
            Some(p) if !p.is_empty() => expand_path(home, p),
            _ => home.join("workspaces"),
        }
    }

    pub fn notifications_enabled(&self) -> bool {
        self.notifications != Some(false)
    }

    pub fn cycle_runtime(&mut self) {
        let cur = self.default_runtime.as_deref().unwrap_or("claude-code");
        let i = RUNTIMES.iter().position(|r| *r == cur).unwrap_or(0);
        self.default_runtime = Some(RUNTIMES[(i + 1) % RUNTIMES.len()].to_string());
    }

    pub fn cycle_permission(&mut self) {
        let next = PermissionMode::parse(self.permission_mode.as_deref()).next();
        self.permission_mode = Some(next.as_str().to_string());
    }
}

pub(crate) fn expand_path(home: &Path, raw: &str) -> PathBuf {
    let expanded = if let Some(rest) = raw.strip_prefix("~/") {
        match std::env::var_os("HOME") {
            Some(h) => PathBuf::from(h).join(rest),
            None => PathBuf::from(raw),
        }
    } else if raw == "~" {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(raw))
    } else {
        PathBuf::from(raw)
    };
    if expanded.is_absolute() {
        expanded
    } else {
        home.join(expanded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_is_home_workspaces() {
        let dir = tempdir().unwrap();
        let cfg = UserConfig::default();
        assert_eq!(
            cfg.resolve_workspace_root(dir.path()),
            dir.path().join("workspaces")
        );
    }

    #[test]
    fn absolute_override() {
        let dir = tempdir().unwrap();
        let cfg = UserConfig {
            workspace_root: Some("/tmp/methodus-runs".to_string()),
            ..Default::default()
        };
        assert_eq!(
            cfg.resolve_workspace_root(dir.path()),
            PathBuf::from("/tmp/methodus-runs")
        );
    }

    #[test]
    fn relative_override_joins_home() {
        let dir = tempdir().unwrap();
        let cfg = UserConfig {
            workspace_root: Some("runs".to_string()),
            ..Default::default()
        };
        assert_eq!(
            cfg.resolve_workspace_root(dir.path()),
            dir.path().join("runs")
        );
    }

    #[test]
    fn loads_yaml() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.yaml"),
            "workspace_root: /data/runs\n",
        )
        .unwrap();
        let cfg = UserConfig::load(dir.path());
        assert_eq!(cfg.workspace_root.as_deref(), Some("/data/runs"));
        assert_eq!(
            cfg.resolve_workspace_root(dir.path()),
            PathBuf::from("/data/runs")
        );
    }

    #[test]
    fn save_roundtrip() {
        let dir = tempdir().unwrap();
        let cfg = UserConfig {
            default_runtime: Some("codex".into()),
            permission_mode: Some("plan".into()),
            default_face: Some("network".into()),
            context_faces: Some(vec!["storage".into()]),
            workspace_root: Some("/data/runs".into()),
            notifications: Some(true),
        };
        cfg.save(dir.path()).unwrap();
        let mut loaded = UserConfig::load(dir.path());
        assert_eq!(loaded.default_runtime.as_deref(), Some("codex"));
        assert_eq!(loaded.default_face.as_deref(), Some("network"));
        loaded.cycle_runtime();
        assert_eq!(loaded.default_runtime.as_deref(), Some("claude-code"));
        assert!(loaded.notifications_enabled());
        loaded.cycle_permission();
        assert_eq!(loaded.permission_mode.as_deref(), Some("cautious"));
    }

    #[test]
    fn notifications_opt_out() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("config.yaml"), "notifications: false\n").unwrap();
        assert!(!UserConfig::load(dir.path()).notifications_enabled());
    }
}
