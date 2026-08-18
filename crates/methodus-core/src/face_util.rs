//! Create or resolve Face directories for study / ingest flows.

use std::fs;
use std::path::Path;

use crate::curiosity::MODULE_EXPERT_METHOD_ID;
use crate::error::CoreError;
use crate::learning::slugify;
use crate::resolution::list_faces;
use crate::workspace::is_safe_segment;

const MODULE_EXPERT_SKILL: &str = "module-expert-learning";

/// Resolve the Face id for a study run; auto-create a personal Face shell when needed.
pub fn ensure_study_face(home: &Path, scope: &str, pinned: Option<&str>) -> Result<String, CoreError> {
    if let Some(id) = pinned.map(str::trim).filter(|s| !s.is_empty()) {
        if !is_safe_segment(id) {
            return Err(CoreError::InvalidTaskId(id.to_string()));
        }
        ensure_personal_face(home, id, scope, true)?;
        return Ok(id.to_string());
    }
    let id = face_id_from_scope(scope);
    if id.is_empty() || !is_safe_segment(&id) {
        return Ok("general".to_string());
    }
    if face_exists(home, &id) {
        return Ok(id);
    }
    ensure_personal_face(home, &id, scope, true)?;
    Ok(id)
}

fn face_id_from_scope(scope: &str) -> String {
    let scope = scope.trim();
    if scope.is_empty() {
        return String::new();
    }
    let first = scope
        .split_whitespace()
        .next()
        .unwrap_or(scope);
    slugify(first)
}

fn face_exists(home: &Path, id: &str) -> bool {
    list_faces(home).iter().any(|f| f.id == id)
}

fn ensure_personal_face(
    home: &Path,
    id: &str,
    scope: &str,
    module_expert: bool,
) -> Result<(), CoreError> {
    let dir = home.join("faces").join(id);
    let yaml = dir.join("face.yaml");
    if yaml.is_file() {
        return Ok(());
    }
    fs::create_dir_all(dir.join("knowledge"))?;
    fs::create_dir_all(dir.join("experiences"))?;
    fs::create_dir_all(dir.join("hypotheses"))?;
    let name = scope.trim();
    let name = if name.is_empty() { id } else { name };
    let mut tags: Vec<String> = scope
        .split_whitespace()
        .map(slugify)
        .filter(|t| t.len() >= 2 && t != "general")
        .collect();
    if !tags.iter().any(|t| t == id) {
        tags.insert(0, id.to_string());
    }
    tags.truncate(8);
    let tags_yaml = tags
        .iter()
        .map(|t| format!("  - {t}"))
        .collect::<Vec<_>>()
        .join("\n");
    let (methods, skills) = if module_expert {
        (
            format!("  - {MODULE_EXPERT_METHOD_ID}\n  - general-software"),
            format!("  - {MODULE_EXPERT_SKILL}\n  - workspace-hygiene"),
        )
    } else {
        (
            "  - general-software".to_string(),
            "  - workspace-hygiene".to_string(),
        )
    };
    let body = format!(
        "id: {id}\n\
         name: {name}\n\
         description: Domain expert for {name} (auto-created by Methodus).\n\
         intent_tags:\n{tags_yaml}\n\
         methods:\n{methods}\n\
         skills:\n{skills}\n"
    );
    fs::write(yaml, body)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn creates_face_from_study_scope() {
        let dir = tempdir().unwrap();
        let id = ensure_study_face(dir.path(), "nxm upgrade module", None).unwrap();
        assert_eq!(id, "nxm");
        assert!(dir.path().join("faces/nxm/face.yaml").exists());
    }
}
