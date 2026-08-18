//! Multi-Face composition: primary + context Faces, cross-Face tension detection.

use std::collections::{HashMap, HashSet};

use methodus_domain::{KnowledgeStatus, Question, QuestionStatus};
use methodus_store::Store;
use uuid::Uuid;

use chrono::Utc;

use crate::error::CoreError;
use crate::learning::{question_value, slugify};
use crate::resolution::tokenize;

/// Parse `/face network + storage kernel` → primary + context face ids.
pub fn parse_face_pin(input: &str) -> (Option<String>, Vec<String>) {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return (None, Vec::new());
    }
    let segments: Vec<&str> = trimmed.split('+').map(str::trim).filter(|s| !s.is_empty()).collect();
    let primary = segments
        .first()
        .and_then(|s| s.split_whitespace().next())
        .map(str::to_string);
    let mut context = Vec::new();
    for seg in segments.iter().skip(1) {
        for word in seg.split_whitespace() {
            if !word.is_empty() {
                context.push(word.to_string());
            }
        }
    }
    if let Some(ref p) = primary {
        context.retain(|id| id != p);
    }
    context.sort();
    context.dedup();
    (primary, context)
}

/// When multiple Faces contribute knowledge, surface disagreements as mentor questions.
pub fn detect_cross_face_debates(
    store: &Store,
    home: &std::path::Path,
    face_ids: &[String],
    request: &str,
) -> Result<Vec<Question>, CoreError> {
    if face_ids.len() < 2 {
        return Ok(Vec::new());
    }
    let tokens = tokenize(request);
    let items = store.list_knowledge(Some(KnowledgeStatus::Committed))?;
    let mut by_stem: HashMap<String, HashMap<String, String>> = HashMap::new();
    for item in items {
        let Some(fid) = item.face_id.as_deref() else {
            continue;
        };
        if !face_ids.iter().any(|f| f == fid) {
            continue;
        }
        let body = std::fs::read_to_string(home.join(&item.path)).unwrap_or_default();
        if body.trim().is_empty() {
            continue;
        }
        let score = overlap_score(&tokens, &body);
        if score == 0 && !tokens.is_empty() {
            continue;
        }
        let stem = item
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&item.path)
            .trim_end_matches(".md")
            .to_string();
        by_stem
            .entry(stem)
            .or_default()
            .insert(fid.to_string(), excerpt_claim(&body));
    }
    let mut created = Vec::new();
    for (topic, claims) in by_stem {
        if claims.len() < 2 {
            continue;
        }
        let distinct: HashSet<_> = claims.values().map(|s| normalize_claim(s)).collect();
        if distinct.len() < 2 {
            continue;
        }
        let faces: Vec<_> = claims.keys().cloned().collect();
        let question = format!(
            "Cross-Face tension on `{topic}`: {} — which applies here?",
            faces.join(" vs ")
        );
        let reason = format!("cross_face_debate:{topic}");
        if store
            .list_questions(None)?
            .iter()
            .any(|q| q.reason.as_deref() == Some(reason.as_str()))
        {
            continue;
        }
        let now = Utc::now();
        let id = format!("q_{}", short_id());
        let q = Question {
            id: id.clone(),
            question,
            reason: Some(reason),
            task_id: None,
            face_id: face_ids.first().cloned(),
            importance: 0.7,
            frequency: 1.0,
            impact: 0.6,
            uncertainty: 0.8,
            value: question_value(0.7, 1.0, 0.6, 0.8),
            status: QuestionStatus::Pending,
            not_before: None,
            answer: None,
            created_at: now,
            updated_at: now,
        };
        store.insert_question(&q)?;
        let _ = store.insert_event(
            &format!("ev_{}", Uuid::new_v4().to_string().replace('-', "")),
            "question.created",
            &now.to_rfc3339(),
            None,
            None,
            &serde_json::json!({"question_id": id, "kind": "cross_face_debate"}).to_string(),
            None,
        );
        created.push(q);
    }
    Ok(created)
}

fn overlap_score(tokens: &HashSet<String>, body: &str) -> usize {
    let body_tokens = tokenize(body);
    tokens.iter().filter(|t| body_tokens.contains(*t)).count()
}

fn excerpt_claim(body: &str) -> String {
    body.lines()
        .find(|l| !l.trim().is_empty() && !l.starts_with('#') && !l.starts_with("---"))
        .unwrap_or(body)
        .trim()
        .chars()
        .take(160)
        .collect()
}

fn normalize_claim(s: &str) -> String {
    slugify(s)
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

    #[test]
    fn parse_face_pin_splits_primary_and_context() {
        let (p, ctx) = parse_face_pin("network + storage kernel");
        assert_eq!(p.as_deref(), Some("network"));
        assert_eq!(ctx, vec!["kernel".to_string(), "storage".to_string()]);
    }

    #[test]
    fn parse_face_pin_single_primary() {
        let (p, ctx) = parse_face_pin("general");
        assert_eq!(p.as_deref(), Some("general"));
        assert!(ctx.is_empty());
    }
}
