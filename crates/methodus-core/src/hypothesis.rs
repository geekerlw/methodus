//! Hypothesis lifecycle: under-evidenced judgments before Knowledge promotion.

use std::fs;
use std::path::Path;

use chrono::Utc;
use methodus_domain::{Hypothesis, HypothesisStatus};
use methodus_store::Store;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::CoreError;
use crate::learning::{slugify, write_candidate};
use crate::workspace::is_safe_segment;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HypothesisReviewAction {
    Validate,
    Promote,
    Reject,
}

pub fn upsert_hypothesis(
    store: &Store,
    home: &Path,
    face: &str,
    topic: &str,
    claim: &str,
    source_detail: &str,
    confidence: f64,
) -> Result<(), CoreError> {
    if !is_safe_segment(face) {
        return Err(CoreError::Other(format!("unsafe face id: {face}")));
    }
    let slug = slugify(topic);
    if slug.is_empty() {
        return Ok(());
    }
    let rel = format!("faces/{face}/hypotheses/{slug}.md");
    let body = format!(
        "# Hypothesis: {topic}\n\n\
         **Claim:** {claim}\n\n\
         **Source:** {source_detail}\n\n\
         **Status:** candidate\n"
    );
    let hash = sha256_hex(body.as_bytes());
    if let Some(mut existing) = store.find_hypothesis_by_path(&rel)? {
        if existing.content_hash == hash {
            return Ok(());
        }
        if matches!(
            existing.status,
            HypothesisStatus::Rejected | HypothesisStatus::Promoted
        ) {
            return Ok(());
        }
        fs::write(home.join(&rel), &body)?;
        existing.content_hash = hash;
        existing.confidence = Some(confidence);
        existing.updated_at = Utc::now();
        store.update_hypothesis(&existing)?;
        return Ok(());
    }
    let now = Utc::now();
    let id = format!("hyp_{}", short_id());
    fs::create_dir_all(home.join(format!("faces/{face}/hypotheses")))?;
    fs::write(home.join(&rel), &body)?;
    let item = Hypothesis {
        id: id.clone(),
        face_id: Some(face.to_string()),
        path: rel,
        content_hash: hash,
        confidence: Some(confidence),
        status: HypothesisStatus::Candidate,
        created_at: now,
        updated_at: now,
    };
    store.insert_hypothesis(&item)?;
    let _ = store.insert_event(
        &format!("ev_{}", Uuid::new_v4().to_string().replace('-', "")),
        "hypothesis.created",
        &now.to_rfc3339(),
        None,
        None,
        &serde_json::json!({"hypothesis_id": id, "face_id": face}).to_string(),
        None,
    );
    Ok(())
}

pub fn review_hypothesis(
    store: &Store,
    home: &Path,
    id: &str,
    action: HypothesisReviewAction,
) -> Result<Hypothesis, CoreError> {
    let mut item = store
        .get_hypothesis(id)?
        .ok_or_else(|| CoreError::Other(format!("hypothesis not found: {id}")))?;
    let now = Utc::now();
    match action {
        HypothesisReviewAction::Validate => {
            item.status = item
                .status
                .checked_transition(HypothesisStatus::Validated)?;
            let _ = store.insert_event(
                &format!("ev_{}", Uuid::new_v4().to_string().replace('-', "")),
                "hypothesis.validated",
                &now.to_rfc3339(),
                None,
                None,
                &serde_json::json!({"hypothesis_id": id}).to_string(),
                None,
            );
        }
        HypothesisReviewAction::Reject => {
            item.status = item
                .status
                .checked_transition(HypothesisStatus::Rejected)?;
        }
        HypothesisReviewAction::Promote => {
            let face = item.face_id.as_deref().unwrap_or("general");
            let body = fs::read_to_string(home.join(&item.path)).unwrap_or_default();
            let title = body
                .lines()
                .find(|l| l.starts_with("# Hypothesis:"))
                .map(|l| l.trim_start_matches("# Hypothesis:").trim())
                .unwrap_or("hypothesis");
            write_candidate(
                store,
                home,
                face,
                title,
                &body,
                "hypothesis",
                None,
                Some(id),
                item.confidence.unwrap_or(0.55),
            )?;
            item.status = item
                .status
                .checked_transition(HypothesisStatus::Promoted)?;
            let _ = store.insert_event(
                &format!("ev_{}", Uuid::new_v4().to_string().replace('-', "")),
                "hypothesis.promoted",
                &now.to_rfc3339(),
                None,
                None,
                &serde_json::json!({"hypothesis_id": id}).to_string(),
                None,
            );
        }
    }
    item.updated_at = now;
    store.update_hypothesis(&item)?;
    Ok(item)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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
    use tempfile::tempdir;

    #[test]
    fn hypothesis_upsert_and_promote() {
        let dir = tempdir().unwrap();
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        upsert_hypothesis(
            &store,
            dir.path(),
            "nxm",
            "boot flow",
            "rev B changes init order",
            "module study uncertainty",
            0.4,
        )
        .unwrap();
        let items = store
            .list_hypotheses(Some(HypothesisStatus::Candidate))
            .unwrap();
        assert_eq!(items.len(), 1);
        let id = items[0].id.clone();
        review_hypothesis(&store, dir.path(), &id, HypothesisReviewAction::Promote).unwrap();
        let k = store
            .list_knowledge(Some(methodus_domain::KnowledgeStatus::Candidate))
            .unwrap();
        assert!(!k.is_empty());
    }
}
