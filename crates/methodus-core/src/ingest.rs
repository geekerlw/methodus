//! Project-level doc ingest and repo survey pipelines.

use std::fs;
use std::path::Path;

use chrono::Utc;
use methodus_domain::{Experience, JobKind, KnowledgeItem, KnowledgeStatus};
use methodus_store::Store;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::curiosity::{extract_section, knowledge_subsections};
use crate::error::CoreError;
use crate::learning::{enqueue_job, emit, full_result_section, result_section, slugify, JobRefs};

pub const DOC_INGEST_METHOD_ID: &str = "doc-ingest";
pub const REPO_SURVEY_METHOD_ID: &str = "repo-survey";
pub const DOC_INGEST_SOURCE: &str = "doc_ingest";
pub const REPO_SURVEY_SOURCE: &str = "repo_survey";

pub fn enqueue_ingest_jobs(store: &Store, exp: &Experience, refs: &JobRefs) -> Result<(), CoreError> {
    let _ = enqueue_job(
        store,
        JobKind::ProposeKnowledge,
        &format!("ingest:{}", exp.id),
        refs,
        7,
    )?;
    Ok(())
}

pub fn enqueue_survey_jobs(store: &Store, exp: &Experience, refs: &JobRefs) -> Result<(), CoreError> {
    let _ = enqueue_job(
        store,
        JobKind::ProposeKnowledge,
        &format!("survey:{}", exp.id),
        refs,
        7,
    )?;
    Ok(())
}

pub fn propose_project_from_experience(
    store: &Store,
    home: &Path,
    exp_id: &str,
    source: &str,
) -> Result<(), CoreError> {
    let exp = store
        .get_experience(exp_id)?
        .ok_or_else(|| CoreError::Other(format!("experience not found: {exp_id}")))?;
    let task = store
        .get_task(&exp.task_id)?
        .ok_or_else(|| CoreError::TaskNotFound(exp.task_id.clone()))?;
    let project_id = task
        .project_id
        .as_deref()
        .ok_or_else(|| CoreError::Other("ingest/survey task needs a focus project".into()))?;
    let body = fs::read_to_string(home.join(&exp.path)).unwrap_or_default();
    let result = if source == DOC_INGEST_SOURCE {
        full_result_section(&body)
    } else {
        result_section(&body)
    };
    if result.trim().is_empty() {
        return Ok(());
    }
    let knowledge_section = extract_section(&result, &["Knowledge", "Project Notes", "Survey"]);
    let subsections = knowledge_subsections(&knowledge_section);
    if subsections.is_empty() {
        write_project_candidate(
            store,
            home,
            project_id,
            task.title.trim(),
            &result,
            source,
            Some(&exp.task_id),
            0.55,
        )?;
    } else {
        for (title, content) in subsections {
            write_project_candidate(
                store,
                home,
                project_id,
                &title,
                &content,
                source,
                Some(&exp.task_id),
                0.6,
            )?;
        }
    }
    Ok(())
}

fn write_project_candidate(
    store: &Store,
    home: &Path,
    project_id: &str,
    title: &str,
    content: &str,
    source: &str,
    task_id: Option<&str>,
    confidence: f64,
) -> Result<(), CoreError> {
    let slug = slugify(title);
    let rel = format!("projects/{project_id}/knowledge/{slug}.md");
    let body = format!(
        "# {title}\n\n\
         - project: `{project_id}`\n\
         - source: {source}\n\n\
         {content}\n"
    );
    let hash = format!("{:x}", Sha256::digest(body.as_bytes()));
    let existing = store.list_knowledge_by_path(&rel)?;
    if existing.iter().any(|k| k.content_hash == hash) {
        return Ok(());
    }
    let abs = home.join(&rel);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&abs, &body)?;
    let now = Utc::now();
    let id = format!("know_{}", Uuid::new_v4().to_string().replace('-', "")[..12].to_string());
    let item = KnowledgeItem {
        id,
        face_id: None,
        project_id: Some(project_id.to_string()),
        path: rel,
        content_hash: hash,
        source: source.to_string(),
        confidence: Some(confidence),
        scope: Some("project".to_string()),
        status: KnowledgeStatus::Candidate,
        conflict_of: None,
        version: 1,
        created_at: now,
        updated_at: now,
    };
    store.insert_knowledge(&item)?;
    emit(
        store,
        "learning.candidate_created",
        task_id,
        serde_json::json!({"knowledge_id": item.id, "path": item.path, "project_id": project_id}),
    );
    Ok(())
}
