//! Team baseline packs: Methodus-format folders. Sharing them (git, USB, …) is
//! an organizational choice — Methodus only loads directories.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::expand_path;
use crate::error::CoreError;
use crate::workspace::is_safe_segment;

const REGISTRY_FILE: &str = "packs.yaml";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PackRegistry {
    #[serde(default)]
    pub focus: Option<String>,
    #[serde(default)]
    pub packs: Vec<PackEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackEntry {
    pub id: String,
    pub path: String,
    #[serde(default = "default_active")]
    pub active: bool,
}

fn default_active() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct PackManifest {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct PackInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub root: PathBuf,
    pub active: bool,
    pub focus: bool,
}

/// Overlay order after the personal home: focus pack, then other active packs.
#[derive(Debug, Clone)]
pub struct PackLayer {
    pub id: String,
    pub source: String,
    pub root: PathBuf,
}

pub fn registry_path(home: &Path) -> PathBuf {
    home.join(REGISTRY_FILE)
}

pub fn load_registry(home: &Path) -> PackRegistry {
    let path = registry_path(home);
    let Ok(raw) = fs::read_to_string(&path) else {
        return discover_only(home);
    };
    let mut reg: PackRegistry = serde_yaml::from_str(&raw).unwrap_or_default();
    merge_discovered(home, &mut reg);
    reg
}

pub fn save_registry(home: &Path, reg: &PackRegistry) -> Result<(), CoreError> {
    fs::create_dir_all(home)?;
    let body = serde_yaml::to_string(reg).unwrap_or_else(|_| "packs: []\n".to_string());
    fs::write(registry_path(home), body)?;
    Ok(())
}

pub fn list_packs(home: &Path) -> Vec<PackInfo> {
    let reg = load_registry(home);
    let focus = reg.focus.as_deref();
    let mut out = Vec::new();
    for entry in &reg.packs {
        let root = expand_path(home, &entry.path);
        let (name, description) = read_manifest(&root)
            .map(|m| {
                (
                    if m.name.is_empty() {
                        m.id.clone()
                    } else {
                        m.name
                    },
                    m.description,
                )
            })
            .unwrap_or_else(|| (entry.id.clone(), String::new()));
        out.push(PackInfo {
            id: entry.id.clone(),
            name,
            description,
            root,
            active: entry.active,
            focus: focus == Some(entry.id.as_str()),
        });
    }
    out
}

/// Active pack roots, focus first.
pub fn overlay_roots(home: &Path) -> Vec<PackLayer> {
    let reg = load_registry(home);
    let mut focus = Vec::new();
    let mut rest = Vec::new();
    for entry in &reg.packs {
        if !entry.active {
            continue;
        }
        if !is_safe_segment(&entry.id) {
            continue;
        }
        let root = expand_path(home, &entry.path);
        if !root.is_dir() {
            continue;
        }
        let layer = PackLayer {
            id: entry.id.clone(),
            source: format!("team:{}", entry.id),
            root,
        };
        if reg.focus.as_deref() == Some(entry.id.as_str()) {
            focus.push(layer);
        } else {
            rest.push(layer);
        }
    }
    focus.append(&mut rest);
    focus
}

pub fn add_pack(home: &Path, pack_dir: &Path) -> Result<PackInfo, CoreError> {
    let root = canonicalize_dir(pack_dir)?;
    let manifest = read_manifest(&root).ok_or_else(|| {
        CoreError::InvalidPack(
            root.clone(),
            "missing pack.yaml with an id field".to_string(),
        )
    })?;
    if !is_safe_segment(&manifest.id) {
        return Err(CoreError::InvalidPack(
            root,
            format!("unsafe pack id `{}`", manifest.id),
        ));
    }
    let mut reg = load_registry(home);
    let stored_path = store_path(home, &root);
    if let Some(existing) = reg.packs.iter_mut().find(|p| p.id == manifest.id) {
        existing.path = stored_path;
        existing.active = true;
    } else {
        reg.packs.push(PackEntry {
            id: manifest.id.clone(),
            path: stored_path,
            active: true,
        });
    }
    if reg.focus.is_none() {
        reg.focus = Some(manifest.id.clone());
    }
    save_registry(home, &reg)?;
    Ok(PackInfo {
        id: manifest.id.clone(),
        name: if manifest.name.is_empty() {
            manifest.id.clone()
        } else {
            manifest.name
        },
        description: manifest.description,
        root,
        active: true,
        focus: reg.focus.as_deref() == Some(manifest.id.as_str()),
    })
}

pub fn set_focus(home: &Path, id: &str) -> Result<PackInfo, CoreError> {
    let mut reg = load_registry(home);
    let Some(entry) = reg.packs.iter_mut().find(|p| p.id == id) else {
        return Err(CoreError::PackNotFound(id.to_string()));
    };
    entry.active = true;
    reg.focus = Some(id.to_string());
    save_registry(home, &reg)?;
    list_packs(home)
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| CoreError::PackNotFound(id.to_string()))
}

pub fn set_active(home: &Path, id: &str, active: bool) -> Result<PackInfo, CoreError> {
    let mut reg = load_registry(home);
    let Some(entry) = reg.packs.iter_mut().find(|p| p.id == id) else {
        return Err(CoreError::PackNotFound(id.to_string()));
    };
    entry.active = active;
    if !active && reg.focus.as_deref() == Some(id) {
        reg.focus = reg
            .packs
            .iter()
            .find(|p| p.active && p.id != id)
            .map(|p| p.id.clone());
    }
    save_registry(home, &reg)?;
    list_packs(home)
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| CoreError::PackNotFound(id.to_string()))
}

pub fn remove_pack(home: &Path, id: &str) -> Result<(), CoreError> {
    let mut reg = load_registry(home);
    let before = reg.packs.len();
    reg.packs.retain(|p| p.id != id);
    if reg.packs.len() == before {
        return Err(CoreError::PackNotFound(id.to_string()));
    }
    if reg.focus.as_deref() == Some(id) {
        reg.focus = reg.packs.iter().find(|p| p.active).map(|p| p.id.clone());
    }
    save_registry(home, &reg)?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct PackKnowledgeFile {
    pub pack_id: String,
    pub face_id: Option<String>,
    pub abs_path: PathBuf,
}

pub fn list_knowledge_files(home: &Path) -> Vec<PackKnowledgeFile> {
    let mut out = Vec::new();
    for layer in overlay_roots(home) {
        collect_knowledge_dir(&mut out, &layer.id, None, &layer.root.join("knowledge"));
        let faces = layer.root.join("faces");
        let Ok(entries) = fs::read_dir(&faces) else {
            continue;
        };
        for entry in entries.flatten() {
            let face_id = entry.file_name().to_string_lossy().into_owned();
            if !is_safe_segment(&face_id) {
                continue;
            }
            collect_knowledge_dir(
                &mut out,
                &layer.id,
                Some(face_id),
                &entry.path().join("knowledge"),
            );
        }
    }
    out
}

fn collect_knowledge_dir(
    out: &mut Vec<PackKnowledgeFile>,
    pack_id: &str,
    face_id: Option<String>,
    dir: &Path,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        out.push(PackKnowledgeFile {
            pack_id: pack_id.to_string(),
            face_id: face_id.clone(),
            abs_path: path,
        });
    }
}

fn discover_only(home: &Path) -> PackRegistry {
    let mut reg = PackRegistry::default();
    merge_discovered(home, &mut reg);
    reg
}

fn merge_discovered(home: &Path, reg: &mut PackRegistry) {
    let dir = home.join("packs");
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let root = entry.path();
        if !root.is_dir() {
            continue;
        }
        let Some(manifest) = read_manifest(&root) else {
            continue;
        };
        if !is_safe_segment(&manifest.id) {
            continue;
        }
        if reg.packs.iter().any(|p| p.id == manifest.id) {
            continue;
        }
        let rel = format!("packs/{}", entry.file_name().to_string_lossy());
        reg.packs.push(PackEntry {
            id: manifest.id,
            path: rel,
            active: true,
        });
    }
}

fn read_manifest(root: &Path) -> Option<PackManifest> {
    let raw = fs::read_to_string(root.join("pack.yaml")).ok()?;
    serde_yaml::from_str(&raw).ok()
}

fn canonicalize_dir(path: &Path) -> Result<PathBuf, CoreError> {
    if !path.is_dir() {
        return Err(CoreError::InvalidPack(
            path.to_path_buf(),
            "not a directory".to_string(),
        ));
    }
    Ok(fs::canonicalize(path)?)
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

    fn write_pack(dir: &Path, id: &str, face: &str, skill: &str) {
        fs::create_dir_all(dir.join("faces").join(face)).unwrap();
        fs::write(
            dir.join("pack.yaml"),
            format!("id: {id}\nname: {id} pack\n"),
        )
        .unwrap();
        fs::write(
            dir.join("faces").join(face).join("face.yaml"),
            format!("id: {face}\nname: {face}\nintent_tags: [{face}]\n"),
        )
        .unwrap();
        fs::create_dir_all(dir.join("skills").join(skill)).unwrap();
        fs::write(
            dir.join("skills").join(skill).join("SKILL.md"),
            format!("---\nname: {skill}\ndescription: from {id}\n---\n"),
        )
        .unwrap();
    }

    #[test]
    fn add_and_focus_two_packs() {
        let home = tempdir().unwrap();
        let a = tempdir().unwrap();
        let b = tempdir().unwrap();
        write_pack(a.path(), "team-a", "alpha", "alpha-skill");
        write_pack(b.path(), "team-b", "beta", "beta-skill");
        add_pack(home.path(), a.path()).unwrap();
        add_pack(home.path(), b.path()).unwrap();
        let listed = list_packs(home.path());
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|p| p.id == "team-a" && p.focus));
        set_focus(home.path(), "team-b").unwrap();
        let layers = overlay_roots(home.path());
        assert_eq!(layers[0].id, "team-b");
        assert!(layers.iter().any(|l| l.id == "team-a"));

        set_active(home.path(), "team-b", false).unwrap();
        let layers = overlay_roots(home.path());
        assert!(layers.iter().all(|l| l.id != "team-b"));
        assert_eq!(layers[0].id, "team-a");
        let listed = list_packs(home.path());
        assert!(listed.iter().any(|p| p.id == "team-a" && p.focus));
    }

    #[test]
    fn discovers_folder_under_home_packs() {
        let home = tempdir().unwrap();
        let root = home.path().join("packs/dropped");
        write_pack(&root, "dropped-x", "gamma", "gamma-skill");
        let listed = list_packs(home.path());
        assert!(listed.iter().any(|p| p.id == "dropped-x"));
        let layers = overlay_roots(home.path());
        assert!(layers.iter().any(|l| l.id == "dropped-x"));
    }
}
