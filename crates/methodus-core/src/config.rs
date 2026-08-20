//! Small user configuration for the maintainer TUI.

use std::path::Path;

use serde::{Deserialize, Serialize};

const RUNTIMES: &[&str] = &["claude-code", "cursor", "codex"];
const PERMISSION_MODES: &[&str] = &["plan", "cautious", "acceptEdits"];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
}

impl UserConfig {
    pub fn load(home: &Path) -> Self {
        serde_yaml::from_str(&std::fs::read_to_string(home.join("config.yaml")).unwrap_or_default()).unwrap_or_default()
    }

    pub fn save(&self, home: &Path) -> Result<(), std::io::Error> {
        std::fs::create_dir_all(home)?;
        let body = serde_yaml::to_string(self).unwrap_or_else(|_| "default_runtime: claude-code\n".into());
        std::fs::write(home.join("config.yaml"), body)
    }

    pub fn cycle_runtime(&mut self) {
        let current = self.default_runtime.as_deref().unwrap_or("claude-code");
        let index = RUNTIMES.iter().position(|runtime| *runtime == current).unwrap_or(0);
        self.default_runtime = Some(RUNTIMES[(index + 1) % RUNTIMES.len()].into());
    }

    pub fn permission_mode(&self) -> &str {
        self.permission_mode
            .as_deref()
            .filter(|mode| PERMISSION_MODES.contains(mode))
            .unwrap_or("plan")
    }

    pub fn cycle_permission(&mut self) {
        let current = self.permission_mode();
        let index = PERMISSION_MODES.iter().position(|mode| *mode == current).unwrap_or(0);
        self.permission_mode = Some(PERMISSION_MODES[(index + 1) % PERMISSION_MODES.len()].into());
    }

    pub fn selected_team(&self) -> &str { self.team_id.as_deref().filter(|id| valid_team_id(id)).unwrap_or("default") }

    pub fn cycle_team(&mut self, home: &Path) -> Result<String, std::io::Error> {
        let mut teams = std::fs::read_dir(home.join("teams"))?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .collect::<Vec<_>>();
        if !teams.iter().any(|team| team == "default") { teams.push("default".into()); }
        teams.sort();
        teams.dedup();
        let current = self.selected_team();
        let index = teams.iter().position(|team| team == current).unwrap_or(0);
        let selected = teams[(index + 1) % teams.len()].clone();
        self.team_id = Some(selected.clone());
        self.save(home)?;
        Ok(selected)
    }
}

fn valid_team_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn config_round_trips_runtime() {
        let dir = tempdir().unwrap();
        let config = UserConfig { default_runtime: Some("codex".into()), team_id: Some("alpha".into()), permission_mode: Some("cautious".into()) };
        config.save(dir.path()).unwrap();
        assert_eq!(UserConfig::load(dir.path()).default_runtime.as_deref(), Some("codex"));
        assert_eq!(UserConfig::load(dir.path()).selected_team(), "alpha");
        assert_eq!(UserConfig::load(dir.path()).permission_mode(), "cautious");
    }

    #[test]
    fn runtime_cycles_deterministically() {
        let mut config = UserConfig::default();
        config.cycle_runtime();
        assert_eq!(config.default_runtime.as_deref(), Some("cursor"));
    }

    #[test]
    fn invalid_team_id_falls_back_to_default() {
        let config = UserConfig { default_runtime: None, team_id: Some("../outside".into()), permission_mode: Some("unsafe".into()) };
        assert_eq!(config.selected_team(), "default");
        assert_eq!(config.permission_mode(), "plan");
    }

    #[test]
    fn permission_cycles_without_bypass_mode() {
        let mut config = UserConfig::default();
        config.cycle_permission();
        assert_eq!(config.permission_mode(), "cautious");
        config.cycle_permission();
        assert_eq!(config.permission_mode(), "acceptEdits");
        config.cycle_permission();
        assert_eq!(config.permission_mode(), "plan");
    }
}
