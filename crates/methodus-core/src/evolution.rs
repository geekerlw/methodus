//! Controlled Face/Method/Skill upgrades (`00-product.md` §3.10).
//!
//! MVP: after enough committed module-study knowledge on a Face, propose
//! `face.yaml` diffs (intent_tags, methods, skills) for human review in `/inbox`.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use methodus_domain::{EvolutionCandidate, EvolutionStatus, KnowledgeStatus};
use methodus_store::Store;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::curiosity::MODULE_EXPERT_METHOD_ID;
use crate::error::CoreError;
use crate::learning::{MODULE_STUDY_SOURCE, SKILL_DRAFT_SOURCE};
use crate::resolution::list_faces;
use crate::workspace::is_safe_segment;

/// Committed module-study knowledge rows before proposing a Face upgrade.
pub const MIN_MODULE_STUDY_COMMITS: i64 = 2;

const MODULE_EXPERT_SKILL_ID: &str = "module-expert-learning";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FaceEvolutionDiff {
    pub add_intent_tags: Vec<String>,
    pub add_methods: Vec<String>,
    pub add_skills: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FaceYaml {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    intent_tags: Vec<String>,
    #[serde(default)]
    methods: Vec<String>,
    #[serde(default)]
    skills: Vec<String>,
}

/// After module-study knowledge is committed, maybe enqueue a Face evolution candidate.
pub fn maybe_propose_face_evolution(
    store: &Store,
    home: &Path,
    face_id: &str,
) -> Result<Option<EvolutionCandidate>, CoreError> {
    if face_id.is_empty() || !is_safe_segment(face_id) {
        return Ok(None);
    }
    let commits = store.count_committed_knowledge(face_id, MODULE_STUDY_SOURCE)?;
    if commits < MIN_MODULE_STUDY_COMMITS {
        return Ok(None);
    }
    if store.has_pending_evolution("face", face_id)? {
        return Ok(None);
    }
    let milestone = commits / MIN_MODULE_STUDY_COMMITS;
    if store.evolution_at_milestone("face", face_id, milestone)? {
        return Ok(None);
    }

    let current = load_face_yaml(home, face_id)?;
    let diff = build_face_diff(store, home, face_id, &current)?;
    if diff.add_intent_tags.is_empty() && diff.add_methods.is_empty() && diff.add_skills.is_empty() {
        return Ok(None);
    }

    let now = Utc::now();
    let id = format!("evo_{}", short_id());
    let diff_json = serde_json::to_string(&diff).map_err(|e| CoreError::Other(e.to_string()))?;
    let rationale = Some(format!(
        "{commits} committed module-study knowledge entries on Face `{face_id}` — \
         propose wiring study methods/skills and domain tags into face.yaml."
    ));
    let item = EvolutionCandidate {
        id: id.clone(),
        target_kind: "face".into(),
        target_id: face_id.to_string(),
        diff: diff_json,
        rationale,
        source: Some(format!("{MODULE_STUDY_SOURCE}:milestone:{milestone}")),
        status: EvolutionStatus::Candidate,
        created_at: now,
        updated_at: now,
    };
    store.insert_evolution(&item)?;
    let _ = store.insert_event(
        &format!("ev_{}", Uuid::new_v4().to_string().replace('-', "")),
        "evolution.proposed",
        &now.to_rfc3339(),
        None,
        None,
        &serde_json::json!({
            "evolution_id": id,
            "target_kind": "face",
            "target_id": face_id,
        })
        .to_string(),
        None,
    );
    Ok(Some(item))
}

pub fn review_evolution(
    store: &Store,
    home: &Path,
    id: &str,
    approve: bool,
) -> Result<EvolutionCandidate, CoreError> {
    let mut item = store
        .get_evolution(id)?
        .ok_or_else(|| CoreError::Other(format!("evolution candidate not found: {id}")))?;
    if item.status != EvolutionStatus::Candidate {
        return Err(CoreError::Other(format!(
            "evolution {} is already {}",
            item.id, item.status
        )));
    }
    let now = Utc::now();
    if approve {
        apply_face_evolution(home, &item)?;
        item.status = item
            .status
            .checked_transition(EvolutionStatus::Active)?;
        let _ = store.insert_event(
            &format!("ev_{}", Uuid::new_v4().to_string().replace('-', "")),
            "evolution.approved",
            &now.to_rfc3339(),
            None,
            None,
            &serde_json::json!({"evolution_id": id, "target_id": item.target_id}).to_string(),
            None,
        );
    } else {
        item.status = item
            .status
            .checked_transition(EvolutionStatus::Rejected)?;
        let _ = store.insert_event(
            &format!("ev_{}", Uuid::new_v4().to_string().replace('-', "")),
            "evolution.rejected",
            &now.to_rfc3339(),
            None,
            None,
            &serde_json::json!({"evolution_id": id}).to_string(),
            None,
        );
    }
    item.updated_at = now;
    store.update_evolution(&item)?;
    Ok(item)
}

pub fn format_evolution_detail(item: &EvolutionCandidate) -> String {
    if item.target_kind == "method" {
        if let Ok(diff) = serde_json::from_str::<MethodEvolutionDiff>(&item.diff) {
            let mut out = format!(
                "## Proposed method `{}`\n\n",
                item.target_id
            );
            if let Some(r) = item.rationale.as_deref() {
                out.push_str(&format!("**Why:** {r}\n\n"));
            }
            out.push_str("```yaml\n");
            out.push_str(&diff.yaml_body);
            out.push_str("\n```\n\nApprove to write `~/.methodus/methods/`.\n");
            return out;
        }
    }
    if item.target_kind == "skill" {
        if let Ok(diff) = serde_json::from_str::<FaceEvolutionDiff>(&item.diff) {
            let mut out = format!("## Proposed skill wiring\n\n**Skill:** `{}`\n\n", item.target_id);
            if let Some(r) = item.rationale.as_deref() {
                out.push_str(&format!("**Why:** {r}\n\n"));
            }
            for s in &diff.add_skills {
                out.push_str(&format!("- add `{s}` to face.yaml\n"));
            }
            out.push_str("\nApprove to merge into personal face.yaml.\n");
            return out;
        }
    }
    let Ok(diff) = serde_json::from_str::<FaceEvolutionDiff>(&item.diff) else {
        return format!(
            "## Face evolution\n\nTarget: `{}`\n\n(raw diff)\n\n{}",
            item.target_id, item.diff
        );
    };
    let mut out = format!(
        "## Proposed face.yaml updates\n\n\
         **Face:** `{}`\n\n",
        item.target_id
    );
    if let Some(r) = item.rationale.as_deref() {
        out.push_str(&format!("**Why:** {r}\n\n"));
    }
    if !diff.add_intent_tags.is_empty() {
        out.push_str("### Add intent_tags\n\n");
        for tag in &diff.add_intent_tags {
            out.push_str(&format!("- `{tag}`\n"));
        }
        out.push('\n');
    }
    if !diff.add_methods.is_empty() {
        out.push_str("### Add methods\n\n");
        for m in &diff.add_methods {
            out.push_str(&format!("- `{m}`\n"));
        }
        out.push('\n');
    }
    if !diff.add_skills.is_empty() {
        out.push_str("### Add skills\n\n");
        for s in &diff.add_skills {
            out.push_str(&format!("- `{s}`\n"));
        }
        out.push('\n');
    }
    out.push_str("---\n\nApprove to merge into `~/.methodus/faces/` (personal overlay only).\n");
    out
}

fn build_face_diff(
    store: &Store,
    home: &Path,
    face_id: &str,
    current: &FaceYaml,
) -> Result<FaceEvolutionDiff, CoreError> {
    let mut add_intent_tags = tags_from_study(store, home, face_id);
    if face_id != "general" {
        push_unique(&mut add_intent_tags, face_id);
    }
    add_intent_tags.retain(|t| !current.intent_tags.iter().any(|e| e == t));

    let mut add_methods = Vec::new();
    if !current
        .methods
        .iter()
        .any(|m| m == MODULE_EXPERT_METHOD_ID)
    {
        add_methods.push(MODULE_EXPERT_METHOD_ID.to_string());
    }

    let mut add_skills = Vec::new();
    if !current
        .skills
        .iter()
        .any(|s| s == MODULE_EXPERT_SKILL_ID)
    {
        add_skills.push(MODULE_EXPERT_SKILL_ID.to_string());
    }
    for skill in committed_skill_names(store, face_id) {
        push_unique(&mut add_skills, &skill);
    }
    add_skills.retain(|s| !current.skills.iter().any(|e| e == s));

    Ok(FaceEvolutionDiff {
        add_intent_tags,
        add_methods,
        add_skills,
    })
}

fn tags_from_study(store: &Store, home: &Path, face_id: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(items) = store.list_committed_knowledge_for_face(face_id, MODULE_STUDY_SOURCE) {
        for item in items {
            if let Some(name) = item.path.rsplit('/').next() {
                let stem = name.trim_end_matches(".md");
                for part in stem.split('-') {
                    let tag = part.trim().to_lowercase();
                    if tag.len() >= 3 && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                        push_unique(&mut out, &tag);
                    }
                }
            }
        }
    }
    if let Ok(exps) = store.list_experiences() {
        for exp in exps {
            if exp.face_id.as_deref() != Some(face_id) {
                continue;
            }
            let Ok(body) = fs::read_to_string(home.join(&exp.path)) else {
                continue;
            };
            if !body.contains("mode: module_expert") {
                continue;
            }
            if let Some(topic) = parse_study_topic(&body) {
                for token in tokenize_topic(&topic) {
                    if token.len() >= 3 {
                        push_unique(&mut out, &token);
                    }
                }
            }
        }
    }
    out.truncate(8);
    out
}

fn tokenize_topic(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(str::to_string)
        .collect()
}

fn parse_study_topic(body: &str) -> Option<String> {
    for line in body.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Module expert study:") {
            let topic = rest.trim();
            if !topic.is_empty() {
                return Some(topic.to_string());
            }
        }
    }
    None
}

fn committed_skill_names(store: &Store, face_id: &str) -> Vec<String> {
    let Ok(items) = store.list_knowledge(Some(KnowledgeStatus::Committed)) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter(|k| k.source == SKILL_DRAFT_SOURCE && k.face_id.as_deref() == Some(face_id))
        .filter_map(|k| skill_name_from_path(&k.path))
        .collect()
}

pub(crate) fn skill_name_from_path(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() >= 3 && parts[0] == "skills" && parts.last() == Some(&"SKILL.md") {
        return Some(parts[1].to_string());
    }
    None
}

fn apply_face_evolution(home: &Path, item: &EvolutionCandidate) -> Result<(), CoreError> {
    match item.target_kind.as_str() {
        "face" => apply_face_yaml_evolution(home, item),
        "method" => apply_method_evolution(home, item),
        "skill" => apply_skill_evolution(home, item),
        other => Err(CoreError::Other(format!(
            "unsupported evolution target_kind: {other}"
        ))),
    }
}

fn apply_face_yaml_evolution(home: &Path, item: &EvolutionCandidate) -> Result<(), CoreError> {
    let diff: FaceEvolutionDiff =
        serde_json::from_str(&item.diff).map_err(|e| CoreError::Other(e.to_string()))?;
    let path = ensure_personal_face_yaml(home, &item.target_id)?;
    let raw = fs::read_to_string(&path).unwrap_or_default();
    let mut face: FaceYaml =
        serde_yaml::from_str(&raw).map_err(|e| CoreError::Other(format!("face.yaml parse: {e}")))?;
    merge_vec(&mut face.intent_tags, &diff.add_intent_tags);
    merge_vec(&mut face.methods, &diff.add_methods);
    merge_vec(&mut face.skills, &diff.add_skills);
    let body = serde_yaml::to_string(&face).map_err(|e| CoreError::Other(e.to_string()))?;
    fs::write(&path, body)?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MethodEvolutionDiff {
    pub yaml_body: String,
}

fn apply_method_evolution(home: &Path, item: &EvolutionCandidate) -> Result<(), CoreError> {
    let diff: MethodEvolutionDiff =
        serde_json::from_str(&item.diff).map_err(|e| CoreError::Other(e.to_string()))?;
    let path = home.join("methods").join(format!("{}.yaml", item.target_id));
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, diff.yaml_body)?;
    Ok(())
}

fn apply_skill_evolution(home: &Path, item: &EvolutionCandidate) -> Result<(), CoreError> {
    let diff: FaceEvolutionDiff =
        serde_json::from_str(&item.diff).map_err(|e| CoreError::Other(e.to_string()))?;
    let face_id = item
        .source
        .as_deref()
        .and_then(|s| s.strip_prefix("face:"))
        .unwrap_or("general");
    let path = ensure_personal_face_yaml(home, face_id)?;
    let raw = fs::read_to_string(&path).unwrap_or_default();
    let mut face: FaceYaml =
        serde_yaml::from_str(&raw).map_err(|e| CoreError::Other(format!("face.yaml parse: {e}")))?;
    merge_vec(&mut face.skills, &diff.add_skills);
    let body = serde_yaml::to_string(&face).map_err(|e| CoreError::Other(e.to_string()))?;
    fs::write(path, body)?;
    Ok(())
}

/// After enough committed experience-sourced knowledge, propose a synthesized Method.
pub fn maybe_propose_method_evolution(
    store: &Store,
    _home: &Path,
    face_id: &str,
) -> Result<Option<EvolutionCandidate>, CoreError> {
    if !is_safe_segment(face_id) {
        return Ok(None);
    }
    let method_id = format!("{face_id}-learned");
    if store.has_pending_evolution("method", &method_id)? {
        return Ok(None);
    }
    let commits = store.count_committed_knowledge(face_id, "experience")?;
    if commits < 3 {
        return Ok(None);
    }
    let items = store.list_committed_knowledge_for_face(face_id, "experience")?;
    let steps: Vec<String> = items
        .iter()
        .take(6)
        .filter_map(|k| k.path.rsplit('/').next())
        .map(|name| format!("Apply lesson from `{name}`"))
        .collect();
    if steps.is_empty() {
        return Ok(None);
    }
    let yaml_body = format!(
        "id: {method_id}\nname: {face_id} learned method\nversion: 0.1.0\nintent_tags:\n  - {face_id}\nsteps:\n{steps}\n",
        steps = steps
            .iter()
            .map(|s| format!("  - {s}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let now = Utc::now();
    let id = format!("evo_{}", short_id());
    let diff_json =
        serde_json::to_string(&MethodEvolutionDiff { yaml_body }).map_err(|e| CoreError::Other(e.to_string()))?;
    let item = EvolutionCandidate {
        id: id.clone(),
        target_kind: "method".into(),
        target_id: method_id,
        diff: diff_json,
        rationale: Some(format!(
            "{commits} committed experience-derived knowledge entries on Face `{face_id}` — propose a reusable Method."
        )),
        source: Some(format!("experience:milestone:{commits}")),
        status: EvolutionStatus::Candidate,
        created_at: now,
        updated_at: now,
    };
    store.insert_evolution(&item)?;
    let _ = store.insert_event(
        &format!("ev_{}", Uuid::new_v4().to_string().replace('-', "")),
        "evolution.proposed",
        &now.to_rfc3339(),
        None,
        None,
        &serde_json::json!({"evolution_id": id, "target_kind": "method", "target_id": item.target_id}).to_string(),
        None,
    );
    Ok(Some(item))
}

/// Job handler: attempt method synthesis for a Face after learning completes.
pub fn run_synthesize_method(
    store: &Store,
    home: &Path,
    refs: &crate::learning::JobRefs,
) -> Result<(), CoreError> {
    let face = refs
        .face_id
        .as_deref()
        .ok_or_else(|| CoreError::Other("synthesize_method missing face_id".into()))?;
    let _ = maybe_propose_method_evolution(store, home, face)?;
    Ok(())
}

/// After a skill draft is committed, propose wiring it into face.yaml.
pub fn maybe_propose_skill_evolution(
    store: &Store,
    home: &Path,
    face_id: &str,
    skill_name: &str,
) -> Result<Option<EvolutionCandidate>, CoreError> {
    if !is_safe_segment(face_id) || !is_safe_segment(skill_name) {
        return Ok(None);
    }
    if store.has_pending_evolution("skill", skill_name)? {
        return Ok(None);
    }
    let current = load_face_yaml(home, face_id)?;
    if current.skills.iter().any(|s| s == skill_name) {
        return Ok(None);
    }
    let diff = FaceEvolutionDiff {
        add_intent_tags: Vec::new(),
        add_methods: Vec::new(),
        add_skills: vec![skill_name.to_string()],
    };
    let now = Utc::now();
    let id = format!("evo_{}", short_id());
    let item = EvolutionCandidate {
        id: id.clone(),
        target_kind: "skill".into(),
        target_id: skill_name.to_string(),
        diff: serde_json::to_string(&diff).map_err(|e| CoreError::Other(e.to_string()))?,
        rationale: Some(format!(
            "Skill `{skill_name}` was committed on Face `{face_id}` — propose adding to face.yaml."
        )),
        source: Some(format!("face:{face_id}")),
        status: EvolutionStatus::Candidate,
        created_at: now,
        updated_at: now,
    };
    store.insert_evolution(&item)?;
    Ok(Some(item))
}

fn load_face_yaml(home: &Path, face_id: &str) -> Result<FaceYaml, CoreError> {
    for face in list_faces(home) {
        if face.id == face_id {
            let path = resolve_face_yaml_path(home, face_id, &face.source)?;
            let raw = fs::read_to_string(&path).unwrap_or_default();
            return serde_yaml::from_str(&raw)
                .map_err(|e| CoreError::Other(format!("face.yaml parse: {e}")));
        }
    }
    if face_id == "general" {
        let path = home.join("faces/general/face.yaml");
        let raw = fs::read_to_string(&path).unwrap_or_default();
        return serde_yaml::from_str(&raw)
            .map_err(|e| CoreError::Other(format!("face.yaml parse: {e}")));
    }
    Err(CoreError::Other(format!("face not found: {face_id}")))
}

fn resolve_face_yaml_path(home: &Path, face_id: &str, source: &str) -> Result<PathBuf, CoreError> {
    let personal = home.join("faces").join(face_id).join("face.yaml");
    if personal.is_file() {
        return Ok(personal);
    }
    if source == "personal" {
        return Ok(personal);
    }
    for pack in crate::pack::overlay_roots(home) {
        let p = pack.root.join("faces").join(face_id).join("face.yaml");
        if p.is_file() {
            return Ok(p);
        }
    }
    Ok(personal)
}

fn ensure_personal_face_yaml(home: &Path, face_id: &str) -> Result<PathBuf, CoreError> {
    let personal_dir = home.join("faces").join(face_id);
    let personal_yaml = personal_dir.join("face.yaml");
    if personal_yaml.is_file() {
        return Ok(personal_yaml);
    }
    for face in list_faces(home) {
        if face.id != face_id {
            continue;
        }
        let src = resolve_face_yaml_path(home, face_id, &face.source)?;
        if src.is_file() {
            fs::create_dir_all(&personal_dir)?;
            fs::copy(&src, &personal_yaml)?;
            return Ok(personal_yaml);
        }
    }
    Err(CoreError::Other(format!(
        "cannot write face.yaml — Face `{face_id}` not found under ~/.methodus/faces or packs"
    )))
}

fn merge_vec(existing: &mut Vec<String>, add: &[String]) {
    for item in add {
        push_unique(existing, item);
    }
}

fn push_unique(out: &mut Vec<String>, item: &str) {
    if item.is_empty() {
        return;
    }
    if out.iter().any(|e| e == item) {
        return;
    }
    out.push(item.to_string());
}

fn short_id() -> String {
    Uuid::new_v4()
        .to_string()
        .replace('-', "")
        .chars()
        .take(12)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use methodus_domain::KnowledgeItem;
    use methodus_store::Store;
    use tempfile::tempdir;

    #[test]
    fn proposes_face_evolution_after_study_commits() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let store = Store::open(&home.join("state.db")).unwrap();
        crate::home::ensure_home(home).unwrap();
        fs::create_dir_all(home.join("faces/general/knowledge")).unwrap();

        for (i, slug) in ["nxm-boot", "nxm-upgrade"].iter().enumerate() {
            let path = format!("faces/general/knowledge/{slug}.md");
            fs::write(home.join(&path), format!("# {slug}\n")).unwrap();
            let now = Utc::now();
            store
                .insert_knowledge(&KnowledgeItem {
                    id: format!("know_study{i}"),
                    face_id: Some("general".into()),
                    project_id: None,
                    path,
                    content_hash: format!("hash{i}"),
                    source: MODULE_STUDY_SOURCE.into(),
                    confidence: Some(0.65),
                    scope: None,
                    status: KnowledgeStatus::Committed,
                    conflict_of: None,
                    version: 1,
                    created_at: now,
                    updated_at: now,
                })
                .unwrap();
        }

        let item = maybe_propose_face_evolution(&store, home, "general")
            .unwrap()
            .expect("evolution candidate");
        assert_eq!(item.target_id, "general");
        let diff: FaceEvolutionDiff = serde_json::from_str(&item.diff).unwrap();
        assert!(diff.add_intent_tags.iter().any(|t| t.contains("nxm")));
        assert!(
            !diff.add_intent_tags.is_empty()
                || !diff.add_methods.is_empty()
                || !diff.add_skills.is_empty()
        );
    }

    #[test]
    fn approve_merges_into_personal_face_yaml() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let store = Store::open(&home.join("state.db")).unwrap();
        crate::home::ensure_home(home).unwrap();
        let diff = FaceEvolutionDiff {
            add_intent_tags: vec!["nxm".into()],
            add_methods: vec![MODULE_EXPERT_METHOD_ID.into()],
            add_skills: vec![MODULE_EXPERT_SKILL_ID.into()],
        };
        let now = Utc::now();
        let item = EvolutionCandidate {
            id: "evo_apply1".into(),
            target_kind: "face".into(),
            target_id: "general".into(),
            diff: serde_json::to_string(&diff).unwrap(),
            rationale: None,
            source: Some(MODULE_STUDY_SOURCE.into()),
            status: EvolutionStatus::Candidate,
            created_at: now,
            updated_at: now,
        };
        store.insert_evolution(&item).unwrap();
        review_evolution(&store, home, "evo_apply1", true).unwrap();
        let raw = fs::read_to_string(home.join("faces/general/face.yaml")).unwrap();
        assert!(raw.contains("nxm"));
        assert!(raw.contains(MODULE_EXPERT_METHOD_ID));
        assert!(raw.contains(MODULE_EXPERT_SKILL_ID));
    }
}
