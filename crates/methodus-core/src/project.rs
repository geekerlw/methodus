//! Project directories: the user's repos. Sharing/cloning them is outside Methodus.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::expand_path;
use crate::error::CoreError;
use crate::workspace::is_safe_segment;

const REGISTRY_FILE: &str = "projects.yaml";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectRegistry {
    #[serde(default)]
    pub focus: Option<String>,
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    pub focus: bool,
}

pub fn load_registry(home: &Path) -> ProjectRegistry {
    let Ok(raw) = fs::read_to_string(home.join(REGISTRY_FILE)) else {
        return ProjectRegistry::default();
    };
    serde_yaml::from_str(&raw).unwrap_or_default()
}

pub fn save_registry(home: &Path, reg: &ProjectRegistry) -> Result<(), CoreError> {
    fs::create_dir_all(home)?;
    let body = serde_yaml::to_string(reg).unwrap_or_else(|_| "projects: []\n".to_string());
    fs::write(home.join(REGISTRY_FILE), body)?;
    Ok(())
}

pub fn list_projects(home: &Path) -> Vec<ProjectInfo> {
    let reg = load_registry(home);
    let focus = reg.focus.as_deref();
    reg.projects
        .iter()
        .map(|e| ProjectInfo {
            id: e.id.clone(),
            name: if e.name.is_empty() {
                e.id.clone()
            } else {
                e.name.clone()
            },
            root: expand_path(home, &e.path),
            focus: focus == Some(e.id.as_str()),
        })
        .collect()
}

pub fn focus_project(home: &Path) -> Option<ProjectInfo> {
    list_projects(home).into_iter().find(|p| p.focus)
}

pub fn add_project(home: &Path, dir: &Path) -> Result<ProjectInfo, CoreError> {
    if !dir.is_dir() {
        return Err(CoreError::InvalidProject(
            dir.to_path_buf(),
            "not a directory".to_string(),
        ));
    }
    let root = fs::canonicalize(dir)?;
    let id = id_from_dir(&root)?;
    let name = root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| id.clone());
    let stored = store_path(home, &root);
    let mut reg = load_registry(home);
    if let Some(existing) = reg.projects.iter_mut().find(|p| p.id == id) {
        existing.path = stored;
        existing.name = name.clone();
    } else {
        reg.projects.push(ProjectEntry {
            id: id.clone(),
            name: name.clone(),
            path: stored,
        });
    }
    if reg.focus.is_none() {
        reg.focus = Some(id.clone());
    }
    save_registry(home, &reg)?;
    Ok(ProjectInfo {
        id: id.clone(),
        name,
        root,
        focus: reg.focus.as_deref() == Some(id.as_str()),
    })
}

pub fn set_focus(home: &Path, id: &str) -> Result<ProjectInfo, CoreError> {
    let mut reg = load_registry(home);
    if !reg.projects.iter().any(|p| p.id == id) {
        return Err(CoreError::ProjectNotFound(id.to_string()));
    }
    reg.focus = Some(id.to_string());
    save_registry(home, &reg)?;
    list_projects(home)
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| CoreError::ProjectNotFound(id.to_string()))
}

pub fn remove_project(home: &Path, id: &str) -> Result<(), CoreError> {
    let mut reg = load_registry(home);
    let before = reg.projects.len();
    reg.projects.retain(|p| p.id != id);
    if reg.projects.len() == before {
        return Err(CoreError::ProjectNotFound(id.to_string()));
    }
    if reg.focus.as_deref() == Some(id) {
        reg.focus = reg.projects.first().map(|p| p.id.clone());
    }
    save_registry(home, &reg)?;
    Ok(())
}

fn id_from_dir(root: &Path) -> Result<String, CoreError> {
    let raw = root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let id: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if !is_safe_segment(&id) {
        return Err(CoreError::InvalidProject(
            root.to_path_buf(),
            format!("cannot derive a safe id from `{raw}`"),
        ));
    }
    Ok(id)
}

fn store_path(home: &Path, abs: &Path) -> String {
    if let Ok(home_abs) = fs::canonicalize(home) {
        if let Ok(rel) = abs.strip_prefix(&home_abs) {
            return rel.to_string_lossy().into_owned();
        }
    }
    abs.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn add_focus_and_remove() {
        let home = tempdir().unwrap();
        let repo = tempdir().unwrap();
        let info = add_project(home.path(), repo.path()).unwrap();
        assert!(info.focus);
        assert_eq!(list_projects(home.path()).len(), 1);
        assert_eq!(focus_project(home.path()).unwrap().id, info.id);
        remove_project(home.path(), &info.id).unwrap();
        assert!(list_projects(home.path()).is_empty());
    }
}
