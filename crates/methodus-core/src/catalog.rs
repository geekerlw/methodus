//! Sync file-backed faces, methods, and skills into SQLite catalog tables.

use std::fs;
use std::path::Path;

use chrono::Utc;
use methodus_store::Store;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::CoreError;
use crate::resolution::scan_skills;
use crate::workspace::is_safe_segment;

#[derive(Debug, Deserialize)]
struct FaceYaml {
    id: String,
    name: String,
    #[serde(default)]
    intent_tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MethodYaml {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    intent_tags: Vec<String>,
}

/// Re-index faces/methods/skills from disk into catalog tables.
pub fn sync_catalog(store: &Store, home: &Path) -> Result<CatalogSyncReport, CoreError> {
    let now = Utc::now().to_rfc3339();
    let mut faces = 0usize;
    let mut methods = 0usize;
    let mut skills = 0usize;

    let faces_dir = home.join("faces");
    if faces_dir.is_dir() {
        for entry in fs::read_dir(&faces_dir).into_iter().flatten().flatten() {
            let path = entry.path().join("face.yaml");
            if !path.is_file() {
                continue;
            }
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(face) = serde_yaml::from_str::<FaceYaml>(&raw) else {
                continue;
            };
            if !is_safe_segment(&face.id) {
                continue;
            }
            let rel = format!("faces/{}/face.yaml", face.id);
            store.upsert_face_catalog(
                &face.id,
                &face.name,
                &rel,
                &sha256_hex(raw.as_bytes()),
                &serde_json::to_string(&face.intent_tags).unwrap_or_else(|_| "[]".into()),
                &now,
            )?;
            faces += 1;
        }
    }

    let methods_dir = home.join("methods");
    if methods_dir.is_dir() {
        for entry in fs::read_dir(&methods_dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext != "yaml" && ext != "yml" {
                continue;
            }
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(method) = serde_yaml::from_str::<MethodYaml>(&raw) else {
                continue;
            };
            if !is_safe_segment(&method.id) {
                continue;
            }
            let rel = format!("methods/{}.yaml", method.id);
            let name = if method.name.is_empty() {
                method.id.as_str()
            } else {
                method.name.as_str()
            };
            store.upsert_method_catalog(
                &method.id,
                name,
                &rel,
                &sha256_hex(raw.as_bytes()),
                &serde_json::to_string(&method.intent_tags).unwrap_or_else(|_| "[]".into()),
                &method.version,
                &now,
            )?;
            methods += 1;
        }
    }

    for skill in scan_skills(home) {
        if !is_safe_segment(&skill.name) {
            continue;
        }
        let rel = skill.path.to_string_lossy().into_owned();
        let Ok(raw) = fs::read_to_string(&skill.path) else {
            continue;
        };
        store.upsert_skill_catalog(
            &skill.name,
            &skill.source,
            &rel,
            &sha256_hex(raw.as_bytes()),
            None,
            &now,
        )?;
        skills += 1;
    }

    Ok(CatalogSyncReport {
        faces,
        methods,
        skills,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogSyncReport {
    pub faces: usize,
    pub methods: usize,
    pub skills: usize,
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn sync_catalog_indexes_seed_files() {
        let dir = tempdir().unwrap();
        crate::home::ensure_home(dir.path()).unwrap();
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        let report = sync_catalog(&store, dir.path()).unwrap();
        assert!(report.faces >= 1);
        assert!(report.methods >= 1);
        assert!(report.skills >= 1);
    }
}
