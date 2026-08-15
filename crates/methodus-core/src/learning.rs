//! Rule-based learning jobs. No LLM: extract → detect gaps → propose candidate knowledge.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use methodus_domain::{
    Experience, JobKind, JobStatus, KnowledgeItem, KnowledgeStatus, LearningJob, Question,
    QuestionStatus, RuntimeEvent,
};
use methodus_store::Store;

use crate::error::CoreError;

pub const DEFAULT_BUDGET: &str = r#"{"max_ms":200,"tokens":0}"#;
pub const SKILL_DRAFT_SOURCE: &str = "skill_draft";
pub const MAX_KNOWLEDGE_INJECT: usize = 5;
const MAX_SNIPPET_CHARS: usize = 900;
const MAX_BOOTSTRAP_ITEMS: usize = 3;
const MAX_FALLBACK_RECENT: usize = 2;
const MAX_ATTEMPTS: i64 = 3;
const DEFAULT_IMPORTANCE: f64 = 0.6;
const DEFAULT_IMPACT: f64 = 0.5;
const DEFAULT_UNCERTAINTY: f64 = 0.8;
/// First-gap default value is 0.24; floor lets a new unknown surface when idle.
pub const IDLE_VALUE_FLOOR: f64 = 0.2;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobRefs {
    #[serde(default)]
    pub experience_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub face_id: Option<String>,
    #[serde(default)]
    pub question_id: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

pub fn question_value(importance: f64, frequency: f64, impact: f64, uncertainty: f64) -> f64 {
    importance * frequency.min(10.0) * impact * uncertainty
}

pub fn slugify(text: &str) -> String {
    let mut parts: Vec<String> = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .filter(|w| w.len() > 1)
        .take(6)
        .map(str::to_string)
        .collect();
    if parts.is_empty() {
        parts.push("note".to_string());
    }
    parts.join("-")
}

pub fn normalize_gap(line: &str) -> String {
    line.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(80)
        .collect::<String>()
        .to_lowercase()
}

pub fn is_gap_line(line: &str) -> bool {
    let l = line.to_lowercase();
    let trimmed = l.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return false;
    }
    trimmed.contains("unknown")
        || trimmed.contains("not sure")
        || trimmed.contains("unclear")
        || trimmed.contains("don't know")
        || trimmed.contains("do not know")
        || trimmed.contains("could not")
        || trimmed.contains("failed to")
        || trimmed.contains("todo")
        || trimmed.contains("fixme")
        || trimmed.contains("???")
}

pub fn extract_gaps(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        if is_gap_line(line) {
            let sig = normalize_gap(line.trim_start_matches(['-', '*', ' ']));
            if !sig.is_empty() && !out.contains(&sig) {
                out.push(sig);
            }
        }
    }
    out
}

pub fn result_section(body: &str) -> String {
    let mut out = Vec::new();
    let mut in_result = false;
    for line in body.lines() {
        let t = line.trim();
        if t.eq_ignore_ascii_case("## result") {
            in_result = true;
            continue;
        }
        if in_result && t.starts_with("## ") {
            break;
        }
        if in_result {
            out.push(line);
        }
    }
    out.join("\n")
}

pub fn extract_knowledge_hints(body: &str) -> Vec<String> {
    let mut hints = Vec::new();
    let mut in_section = false;
    for line in body.lines() {
        let t = line.trim();
        if t.eq_ignore_ascii_case("## knowledge") || t.eq_ignore_ascii_case("## candidate") {
            in_section = true;
            continue;
        }
        if t.starts_with("## ") {
            in_section = false;
            continue;
        }
        if in_section && !t.is_empty() && !t.starts_with('#') {
            hints.push(t.trim_start_matches(['-', '*', ' ']).to_string());
        }
    }
    hints
}

/// A vetted knowledge excerpt to inject into the next task (never the whole Face store).
#[derive(Debug, Clone)]
pub struct KnowledgeSnippet {
    pub id: String,
    pub src_path: PathBuf,
    pub dest_name: String,
    pub title: String,
    pub excerpt: String,
    pub origin: String,
}

pub fn select_committed_knowledge(
    store: &Store,
    home: &Path,
    face_id: &str,
    request: &str,
) -> Result<Vec<KnowledgeSnippet>, CoreError> {
    let items = store.list_knowledge(Some(KnowledgeStatus::Committed))?;
    let mut scored: Vec<(usize, KnowledgeItem, String)> = Vec::new();
    for item in items {
        if !knowledge_belongs_to_face(&item, face_id) {
            continue;
        }
        if item.source == SKILL_DRAFT_SOURCE {
            continue;
        }
        if matches!(item.scope.as_deref(), Some("skill") | Some("interaction")) {
            continue;
        }
        let abs = home.join(&item.path);
        let Ok(body) = fs::read_to_string(&abs) else {
            continue;
        };
        if body.trim().is_empty() {
            continue;
        }
        let score = knowledge_score(request, &item.path, &body);
        scored.push((score, item, body));
    }
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.updated_at.cmp(&a.1.updated_at))
    });
    let chosen = if scored.iter().any(|(s, _, _)| *s > 0) {
        scored
            .into_iter()
            .filter(|(s, _, _)| *s > 0)
            .take(MAX_KNOWLEDGE_INJECT)
            .collect::<Vec<_>>()
    } else if scored.len() <= MAX_BOOTSTRAP_ITEMS {
        scored
    } else {
        let mut recent = scored;
        recent.sort_by_key(|b| std::cmp::Reverse(b.1.updated_at));
        recent.truncate(MAX_FALLBACK_RECENT);
        recent
    };

    let mut out = Vec::new();
    let mut used_names = Vec::new();
    for (_, item, body) in chosen {
        let dest_name = unique_dest_name(&item.path, &item.id, &used_names);
        let stem = Path::new(&dest_name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !crate::workspace::is_safe_segment(&stem) {
            continue;
        }
        used_names.push(dest_name.clone());
        out.push(KnowledgeSnippet {
            id: item.id.clone(),
            src_path: home.join(&item.path),
            dest_name,
            title: knowledge_title(&item.path, &body),
            excerpt: excerpt_body(&body, MAX_SNIPPET_CHARS),
            origin: "personal".to_string(),
        });
    }
    append_pack_knowledge(home, face_id, request, &mut out, &mut used_names);
    Ok(out)
}

fn append_pack_knowledge(
    home: &Path,
    face_id: &str,
    request: &str,
    out: &mut Vec<KnowledgeSnippet>,
    used_names: &mut Vec<String>,
) {
    if out.len() >= MAX_KNOWLEDGE_INJECT {
        return;
    }
    let mut scored: Vec<(usize, crate::pack::PackKnowledgeFile, String)> = Vec::new();
    for file in crate::pack::list_knowledge_files(home) {
        if let Some(fid) = file.face_id.as_deref() {
            if fid != face_id {
                continue;
            }
        }
        let Ok(body) = fs::read_to_string(&file.abs_path) else {
            continue;
        };
        if body.trim().is_empty() {
            continue;
        }
        let path_s = file.abs_path.to_string_lossy();
        let score = knowledge_score(request, &path_s, &body);
        scored.push((score, file, body));
    }
    scored.sort_by_key(|b| std::cmp::Reverse(b.0));
    let ranked: Vec<_> = if scored.iter().any(|(s, _, _)| *s > 0) {
        scored.into_iter().filter(|(s, _, _)| *s > 0).collect()
    } else {
        scored
    };
    for (_, file, body) in ranked {
        if out.len() >= MAX_KNOWLEDGE_INJECT {
            break;
        }
        let dest_name = unique_dest_name(
            &file.abs_path.to_string_lossy(),
            &format!("pack-{}", file.pack_id),
            used_names,
        );
        let stem = Path::new(&dest_name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !crate::workspace::is_safe_segment(&stem) {
            continue;
        }
        used_names.push(dest_name.clone());
        let origin = format!("team:{}", file.pack_id);
        let title = knowledge_title(&file.abs_path.to_string_lossy(), &body);
        out.push(KnowledgeSnippet {
            id: format!("pack_{}_{stem}", file.pack_id),
            src_path: file.abs_path,
            dest_name,
            title,
            excerpt: excerpt_body(&body, MAX_SNIPPET_CHARS),
            origin,
        });
    }
}

pub fn render_knowledge_context(snippets: &[KnowledgeSnippet]) -> String {
    if snippets.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n## Face knowledge (committed)\n\n\
         Vetted notes for this Face. Prefer them over improvising. Full copies: `face-context/knowledge/`.\n",
    );
    for snip in snippets {
        out.push_str(&format!(
            "\n### {} ({})\n\n{}\n\n- file: `face-context/knowledge/{}`\n",
            snip.title, snip.origin, snip.excerpt, snip.dest_name
        ));
    }
    out
}

fn knowledge_belongs_to_face(item: &KnowledgeItem, face_id: &str) -> bool {
    match item.face_id.as_deref() {
        Some(id) => id == face_id,
        None => face_id == "general",
    }
}

fn knowledge_score(request: &str, path: &str, body: &str) -> usize {
    let req = crate::resolution::tokenize(request);
    if req.is_empty() {
        return 0;
    }
    let hay = crate::resolution::tokenize(&format!("{path} {body}"));
    req.iter().filter(|t| hay.contains(*t)).count()
}

fn knowledge_title(path: &str, body: &str) -> String {
    for line in body.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("# ") {
            let title = rest.trim();
            if !title.is_empty() {
                return title.chars().take(80).collect();
            }
        }
    }
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "note".to_string())
}

fn excerpt_body(body: &str, max_chars: usize) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let t: String = trimmed.chars().take(max_chars.saturating_sub(3)).collect();
    format!("{t}...")
}

fn unique_dest_name(path: &str, id: &str, used: &[String]) -> String {
    let raw_stem = Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| id.to_string());
    let stem = if crate::workspace::is_safe_segment(&raw_stem) {
        raw_stem
    } else {
        id.chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect::<String>()
    };
    let mut name = format!("{stem}.md");
    if used.iter().any(|u| u == &name) {
        name = format!("{stem}-{id}.md");
    }
    name
}

fn short_id() -> String {
    Uuid::new_v4().to_string().replace('-', "")[..12].to_string()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn emit(store: &Store, event_type: &str, task_id: Option<&str>, payload: serde_json::Value) {
    let id = format!("ev_{}", short_id());
    let _ = store.insert_event(
        &id,
        event_type,
        &Utc::now().to_rfc3339(),
        task_id,
        None,
        &payload.to_string(),
        None,
    );
}

pub fn enqueue_extract(store: &Store, exp: &Experience) -> Result<bool, CoreError> {
    let now = Utc::now();
    let refs = JobRefs {
        experience_id: Some(exp.id.clone()),
        task_id: Some(exp.task_id.clone()),
        face_id: exp.face_id.clone(),
        question_id: None,
        source: Some("experience".to_string()),
    };
    let job = LearningJob {
        id: format!("job_{}", short_id()),
        kind: JobKind::ExtractExperience,
        priority: 10,
        dedupe_key: Some(format!("extract:{}", exp.id)),
        input_refs: serde_json::to_string(&refs).unwrap_or_else(|_| "{}".to_string()),
        status: JobStatus::Queued,
        attempts: 0,
        not_before: None,
        budget: Some(DEFAULT_BUDGET.to_string()),
        requires_approval: false,
        created_at: now,
        updated_at: now,
    };
    let inserted = store.enqueue_job(&job)?;
    if inserted {
        emit(
            store,
            "learning.job_queued",
            Some(&exp.task_id),
            serde_json::json!({"job_id": job.id, "kind": job.kind.to_string()}),
        );
    }
    Ok(inserted)
}

pub fn enqueue_job(
    store: &Store,
    kind: JobKind,
    dedupe: &str,
    refs: &JobRefs,
    priority: i64,
) -> Result<bool, CoreError> {
    let now = Utc::now();
    let job = LearningJob {
        id: format!("job_{}", short_id()),
        kind,
        priority,
        dedupe_key: Some(dedupe.to_string()),
        input_refs: serde_json::to_string(refs).unwrap_or_else(|_| "{}".to_string()),
        status: JobStatus::Queued,
        attempts: 0,
        not_before: None,
        budget: Some(DEFAULT_BUDGET.to_string()),
        requires_approval: false,
        created_at: now,
        updated_at: now,
    };
    let inserted = store.enqueue_job(&job)?;
    if inserted {
        emit(
            store,
            "learning.job_queued",
            refs.task_id.as_deref(),
            serde_json::json!({"job_id": job.id, "kind": job.kind.to_string()}),
        );
    }
    Ok(inserted)
}

pub fn run_job(store: &Arc<Store>, home: &Path, job: &LearningJob) -> Result<(), CoreError> {
    emit(
        store,
        "learning.job_started",
        None,
        serde_json::json!({"job_id": job.id, "kind": job.kind.to_string()}),
    );
    let refs: JobRefs = serde_json::from_str(&job.input_refs).unwrap_or_default();
    match job.kind {
        JobKind::ExtractExperience => run_extract(store, home, &refs)?,
        JobKind::DetectGaps => run_detect(store, home, &refs)?,
        JobKind::ProposeKnowledge => run_propose(store, home, &refs)?,
        JobKind::ProposeSkill => run_propose_skill(store, home, &refs)?,
    }
    Ok(())
}

fn run_extract(store: &Store, home: &Path, refs: &JobRefs) -> Result<(), CoreError> {
    let exp_id = refs
        .experience_id
        .as_deref()
        .ok_or_else(|| CoreError::Other("extract_experience missing experience_id".into()))?;
    let exp = store
        .get_experience(exp_id)?
        .ok_or_else(|| CoreError::Other(format!("experience not found: {exp_id}")))?;
    let body = fs::read_to_string(home.join(&exp.path)).unwrap_or_default();
    let _gaps = extract_gaps(&body);
    enqueue_job(
        store,
        JobKind::DetectGaps,
        &format!("detect:{}", exp.id),
        refs,
        8,
    )?;
    Ok(())
}

fn run_detect(store: &Store, home: &Path, refs: &JobRefs) -> Result<(), CoreError> {
    let exp_id = refs
        .experience_id
        .as_deref()
        .ok_or_else(|| CoreError::Other("detect_gaps missing experience_id".into()))?;
    let exp = store
        .get_experience(exp_id)?
        .ok_or_else(|| CoreError::Other(format!("experience not found: {exp_id}")))?;
    let body = fs::read_to_string(home.join(&exp.path)).unwrap_or_default();
    let gaps = extract_gaps(&body);
    let hints = extract_knowledge_hints(&body);
    let face = exp.face_id.clone().or_else(|| refs.face_id.clone());

    for gap in &gaps {
        upsert_question(store, gap, &exp, face.as_deref())?;
    }

    if !gaps.is_empty() || !hints.is_empty() {
        let mut propose_refs = refs.clone();
        propose_refs.source = Some("experience".to_string());
        enqueue_job(
            store,
            JobKind::ProposeKnowledge,
            &format!("propose:exp:{}", exp.id),
            &propose_refs,
            5,
        )?;
    }
    if skill_worthy(
        &result_section(&body),
        count_tool_events(store, &exp.task_id),
    ) {
        enqueue_job(
            store,
            JobKind::ProposeSkill,
            &format!("skill:exp:{}", exp.id),
            refs,
            4,
        )?;
    }
    Ok(())
}

fn upsert_question(
    store: &Store,
    gap: &str,
    exp: &Experience,
    face: Option<&str>,
) -> Result<(), CoreError> {
    let text = format!("What should we know about: {gap}?");
    let now = Utc::now();
    if let Some(mut existing) = store.find_question_by_text(&text, face)? {
        if existing.status == QuestionStatus::Answered
            || existing.status == QuestionStatus::Dismissed
        {
            return Ok(());
        }
        existing.frequency += 1.0;
        existing.value = question_value(
            existing.importance,
            existing.frequency,
            existing.impact,
            existing.uncertainty,
        );
        existing.updated_at = now;
        if existing.task_id.is_none() {
            existing.task_id = Some(exp.task_id.clone());
        }
        store.update_question(&existing)?;
        return Ok(());
    }

    let freq = 1.0;
    let value = question_value(
        DEFAULT_IMPORTANCE,
        freq,
        DEFAULT_IMPACT,
        DEFAULT_UNCERTAINTY,
    );
    let q = Question {
        id: format!("q_{}", short_id()),
        question: text,
        reason: Some(format!("gap in experience {}", exp.id)),
        task_id: Some(exp.task_id.clone()),
        face_id: face.map(str::to_string),
        importance: DEFAULT_IMPORTANCE,
        frequency: freq,
        impact: DEFAULT_IMPACT,
        uncertainty: DEFAULT_UNCERTAINTY,
        value,
        status: QuestionStatus::Pending,
        not_before: None,
        answer: None,
        created_at: now,
        updated_at: now,
    };
    store.insert_question(&q)?;
    emit(
        store,
        "question.created",
        Some(&exp.task_id),
        serde_json::json!({"question_id": q.id, "value": q.value, "frequency": q.frequency}),
    );
    Ok(())
}

fn run_propose(store: &Store, home: &Path, refs: &JobRefs) -> Result<(), CoreError> {
    if let Some(qid) = refs.question_id.as_deref() {
        return propose_from_question(store, home, qid);
    }
    let exp_id = refs
        .experience_id
        .as_deref()
        .ok_or_else(|| CoreError::Other("propose_knowledge missing source".into()))?;
    propose_from_experience(store, home, exp_id)
}

fn propose_from_experience(store: &Store, home: &Path, exp_id: &str) -> Result<(), CoreError> {
    let exp = store
        .get_experience(exp_id)?
        .ok_or_else(|| CoreError::Other(format!("experience not found: {exp_id}")))?;
    let body = fs::read_to_string(home.join(&exp.path)).unwrap_or_default();
    let mut chunks = extract_knowledge_hints(&body);
    if chunks.is_empty() {
        chunks = extract_gaps(&body);
    }
    if chunks.is_empty() {
        return Ok(());
    }
    let title = chunks[0].clone();
    let result = result_section(&body);
    let content = if result.trim().is_empty() {
        chunks.join("\n\n")
    } else {
        result
    };
    write_candidate(
        store,
        home,
        exp.face_id.as_deref().unwrap_or("general"),
        &title,
        &content,
        "experience",
        Some(&exp.task_id),
        Some(exp_id),
        0.4,
    )
}

fn propose_from_question(store: &Store, home: &Path, question_id: &str) -> Result<(), CoreError> {
    let q = store
        .get_question(question_id)?
        .ok_or_else(|| CoreError::QuestionNotFound(question_id.to_string()))?;
    let answer = q
        .answer
        .as_deref()
        .ok_or_else(|| CoreError::Other(format!("question {question_id} has no answer")))?;
    let content = format!("## Question\n\n{}\n\n## Answer\n\n{answer}\n", q.question);
    write_candidate(
        store,
        home,
        q.face_id.as_deref().unwrap_or("general"),
        &q.question,
        &content,
        "user_answer",
        q.task_id.as_deref(),
        Some(question_id),
        0.7,
    )
}

#[allow(clippy::too_many_arguments)]
fn write_candidate(
    store: &Store,
    home: &Path,
    face: &str,
    title: &str,
    content: &str,
    source: &str,
    task_id: Option<&str>,
    source_id: Option<&str>,
    confidence: f64,
) -> Result<(), CoreError> {
    let slug = slugify(title);
    let rel = format!("faces/{face}/knowledge/{slug}.md");
    let now = Utc::now();
    let id = format!("know_{}", short_id());
    let body = format!(
        "# Knowledge `{id}`\n\n\
         - face: `{face}`\n\
         - source: {source}\n\
         - source_id: {}\n\
         - status: candidate\n\
         - created: {}\n\n\
         ## {title}\n\n\
         {content}\n",
        source_id.unwrap_or("-"),
        now.to_rfc3339(),
    );
    let hash = sha256_hex(body.as_bytes());

    let existing = store.list_knowledge_by_path(&rel)?;
    if existing.iter().any(|k| k.content_hash == hash) {
        return Ok(());
    }
    let committed = existing
        .iter()
        .find(|k| k.status == KnowledgeStatus::Committed);

    let (path, status, conflict_of) = if let Some(committed) = committed {
        if committed.content_hash == hash {
            return Ok(());
        }
        emit(
            store,
            "knowledge.conflict_detected",
            task_id,
            serde_json::json!({
                "path": rel,
                "committed_id": committed.id,
                "candidate_id": id,
            }),
        );
        (
            format!("faces/{face}/knowledge/{slug}--{id}.md"),
            KnowledgeStatus::Conflicted,
            Some(committed.id.clone()),
        )
    } else {
        (rel, KnowledgeStatus::Candidate, None)
    };

    let abs = home.join(&path);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent)?;
    }
    // Never overwrite a committed file on the canonical path.
    if abs.exists() {
        if let Some(found) = store
            .list_knowledge_by_path(&path)?
            .iter()
            .find(|k| k.status == KnowledgeStatus::Committed)
        {
            if found.content_hash != hash {
                let alt = format!("faces/{face}/knowledge/{slug}--{id}.md");
                return write_at(
                    store,
                    home,
                    &alt,
                    &id,
                    face,
                    source,
                    &hash,
                    KnowledgeStatus::Conflicted,
                    Some(found.id.clone()),
                    confidence,
                    &body,
                    task_id,
                );
            }
            return Ok(());
        }
    }
    write_at(
        store,
        home,
        &path,
        &id,
        face,
        source,
        &hash,
        status,
        conflict_of,
        confidence,
        &body,
        task_id,
    )
}

#[allow(clippy::too_many_arguments)]
fn write_at(
    store: &Store,
    home: &Path,
    path: &str,
    id: &str,
    face: &str,
    source: &str,
    hash: &str,
    status: KnowledgeStatus,
    conflict_of: Option<String>,
    confidence: f64,
    body: &str,
    task_id: Option<&str>,
) -> Result<(), CoreError> {
    let abs = home.join(path);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent)?;
    }
    if abs.exists() {
        let existing_on_disk = store.list_knowledge_by_path(path)?;
        if existing_on_disk
            .iter()
            .any(|k| k.status == KnowledgeStatus::Committed)
        {
            return Err(CoreError::Other(format!(
                "refusing to overwrite committed knowledge at {path}"
            )));
        }
    }
    fs::write(&abs, body)?;
    let now = Utc::now();
    let item = KnowledgeItem {
        id: id.to_string(),
        face_id: Some(face.to_string()),
        project_id: None,
        path: path.to_string(),
        content_hash: hash.to_string(),
        source: source.to_string(),
        confidence: Some(confidence),
        scope: None,
        status,
        conflict_of,
        version: 1,
        created_at: now,
        updated_at: now,
    };
    store.insert_knowledge(&item)?;
    emit(
        store,
        "learning.candidate_created",
        task_id,
        serde_json::json!({"knowledge_id": id, "path": path, "status": item.status.to_string()}),
    );
    Ok(())
}

pub fn is_learn_request(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    t == "/learn"
        || t.starts_with("/learn ")
        || t.contains("沉淀成技能")
        || t.contains("沉淀为技能")
        || t.contains("save this as a skill")
        || t.contains("save as a skill")
}

pub fn learn_hint(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let rest = if let Some(r) = trimmed.strip_prefix("/learn") {
        r.trim()
    } else if let Some(r) = trimmed.strip_prefix("沉淀成技能") {
        r.trim()
    } else if let Some(r) = trimmed.strip_prefix("沉淀为技能") {
        r.trim()
    } else {
        ""
    };
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    }
}

/// Draft a candidate skill from a live task (explicit `/learn`).
pub fn propose_skill_from_task(
    store: &Store,
    home: &Path,
    task_id: &str,
    hint: Option<&str>,
) -> Result<Option<KnowledgeItem>, CoreError> {
    let task = store
        .get_task(task_id)?
        .ok_or_else(|| CoreError::TaskNotFound(task_id.to_string()))?;
    let refs = JobRefs {
        experience_id: None,
        task_id: Some(task.id.clone()),
        face_id: face_from_task(&task),
        question_id: None,
        source: Some("user_learn".to_string()),
    };
    write_skill_candidate(store, home, &refs, hint, true)
}

pub fn install_skill_draft(home: &Path, item: &KnowledgeItem) -> Result<String, CoreError> {
    if item.source != SKILL_DRAFT_SOURCE {
        return Ok(item.path.clone());
    }
    let src = home.join(&item.path);
    let body = fs::read_to_string(&src).unwrap_or_default();
    let name = skill_name_from_draft(&item.path, &body);
    if !crate::workspace::is_safe_segment(&name) {
        return Err(CoreError::Other(format!("unsafe skill name: {name}")));
    }
    let live_rel = format!("skills/{name}/SKILL.md");
    let live_abs = home.join(&live_rel);
    if live_abs.exists() {
        return Err(CoreError::Other(format!(
            "skill `{name}` already exists at {live_rel}; left as conflict"
        )));
    }
    let promoted = body.replace("status: candidate", "status: committed");
    if let Some(parent) = live_abs.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&live_abs, promoted)?;
    Ok(live_rel)
}

fn skill_name_from_draft(path: &str, body: &str) -> String {
    if let Some(name) = frontmatter_field(body, "name") {
        if crate::workspace::is_safe_segment(&name) {
            return name;
        }
    }
    let dir = Path::new(path)
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "skill".to_string());
    match dir.rsplit_once("--") {
        Some((base, rest)) if rest.starts_with("know_") => base.to_string(),
        _ => dir,
    }
}

fn frontmatter_field(body: &str, key: &str) -> Option<String> {
    let rest = body.strip_prefix("---\n")?;
    let fm = rest.split_once("\n---")?.0;
    for line in fm.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        if k.trim() != key {
            continue;
        }
        let raw = v.trim().trim_matches('"').trim().to_string();
        if raw.is_empty() {
            return None;
        }
        return Some(raw);
    }
    None
}

fn skill_slug(title: &str, task_id: &str) -> String {
    let base = slugify(title);
    let suffix: String = task_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .rev()
        .take(8)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if base == "note" {
        format!("skill-{suffix}")
    } else {
        format!("{base}-{suffix}")
    }
}

fn face_from_task(task: &methodus_domain::Task) -> Option<String> {
    crate::resolution::Resolution::parse_json(task.resolution.as_deref().unwrap_or(""))
        .map(|r| r.face_id)
}

fn run_propose_skill(store: &Store, home: &Path, refs: &JobRefs) -> Result<(), CoreError> {
    let _ = write_skill_candidate(store, home, refs, None, false)?;
    Ok(())
}

fn write_skill_candidate(
    store: &Store,
    home: &Path,
    refs: &JobRefs,
    hint: Option<&str>,
    explicit: bool,
) -> Result<Option<KnowledgeItem>, CoreError> {
    let task_id = refs
        .task_id
        .as_deref()
        .ok_or_else(|| CoreError::Other("propose_skill missing task_id".into()))?;
    let task = store
        .get_task(task_id)?
        .ok_or_else(|| CoreError::TaskNotFound(task_id.to_string()))?;
    let tools = collect_tools(store, task_id);
    let pitfalls = collect_pitfalls(store, task_id);
    let result = if let Some(eid) = refs.experience_id.as_deref() {
        store
            .get_experience(eid)?
            .and_then(|e| fs::read_to_string(home.join(&e.path)).ok())
            .map(|b| result_section(&b))
            .unwrap_or_default()
    } else {
        String::new()
    };
    if !explicit && !skill_worthy(&result, tools.len()) {
        return Ok(None);
    }
    if tools.is_empty() && result.trim().is_empty() && !explicit {
        return Ok(None);
    }

    let title = hint
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(task.title.trim());
    let slug = skill_slug(title, task_id);
    let face = refs
        .face_id
        .clone()
        .or_else(|| face_from_task(&task))
        .unwrap_or_else(|| "general".to_string());
    let desc = format!(
        "Use when working on: {}. Distilled from task `{task_id}`.",
        ellipsize_desc(&task.request)
    );
    let desc_yaml = yaml_quote(&desc);
    let procedure = if tools.is_empty() {
        if result.trim().is_empty() {
            format!("1. Revisit the original request:\n   {}", task.request)
        } else {
            result.trim().to_string()
        }
    } else {
        tools
            .iter()
            .enumerate()
            .map(|(i, t)| format!("{}. Use `{t}`", i + 1))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let pit_block = if pitfalls.is_empty() {
        "- (none recorded yet)".to_string()
    } else {
        pitfalls
            .iter()
            .map(|p| format!("- {p}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let now = Utc::now();
    let id = format!("know_{}", short_id());
    let rel = format!("skills/.candidates/{slug}/SKILL.md");
    let body = format!(
        "---\n\
         name: {slug}\n\
         description: {desc_yaml}\n\
         status: candidate\n\
         source_task: {task_id}\n\
         ---\n\n\
         # {slug}\n\n\
         ## When to use\n\n\
         {desc}\n\n\
         ## Procedure\n\n\
         {procedure}\n\n\
         ## Pitfalls\n\n\
         {pit_block}\n\n\
         ## Verification\n\n\
         Re-run the original request and confirm the same outcome.\n"
    );
    let hash = sha256_hex(body.as_bytes());
    let existing = store.list_knowledge_by_path(&rel)?;
    if let Some(prev) = existing.iter().find(|k| k.content_hash == hash) {
        return Ok(Some(prev.clone()));
    }
    if let Some(mut prev) = existing
        .into_iter()
        .find(|k| k.status == KnowledgeStatus::Candidate)
    {
        let abs = home.join(&prev.path);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&abs, &body)?;
        prev.content_hash = hash;
        prev.updated_at = now;
        store.update_knowledge(&prev)?;
        return Ok(Some(prev));
    }
    let live = home.join(format!("skills/{slug}/SKILL.md"));
    let live_rows = store.list_knowledge_by_path(&format!("skills/{slug}/SKILL.md"))?;
    let (path, status, conflict_of) = if live.exists() {
        (
            format!("skills/.candidates/{slug}--{id}/SKILL.md"),
            KnowledgeStatus::Conflicted,
            live_rows
                .iter()
                .find(|k| k.status == KnowledgeStatus::Committed)
                .map(|k| k.id.clone()),
        )
    } else {
        (rel, KnowledgeStatus::Candidate, None)
    };
    let abs = home.join(&path);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&abs, &body)?;
    let item = KnowledgeItem {
        id: id.clone(),
        face_id: Some(face),
        project_id: None,
        path: path.clone(),
        content_hash: hash,
        source: SKILL_DRAFT_SOURCE.to_string(),
        confidence: Some(if explicit { 0.7 } else { 0.45 }),
        scope: Some("skill".to_string()),
        status,
        conflict_of,
        version: 1,
        created_at: now,
        updated_at: now,
    };
    store.insert_knowledge(&item)?;
    emit(
        store,
        "learning.candidate_created",
        Some(task_id),
        serde_json::json!({"knowledge_id": id, "path": path, "kind": "skill"}),
    );
    Ok(Some(item))
}

fn skill_worthy(result: &str, tool_count: usize) -> bool {
    if tool_count >= 3 {
        return true;
    }
    let steps = result
        .lines()
        .filter(|l| {
            let t = l.trim();
            t.starts_with("- ")
                || t.chars().next().is_some_and(|c| c.is_ascii_digit())
                || t.starts_with("```")
        })
        .count();
    steps >= 3
}

fn count_tool_events(store: &Store, task_id: &str) -> usize {
    collect_tools(store, task_id).len()
}

fn collect_tools(store: &Store, task_id: &str) -> Vec<String> {
    let Ok(events) = store.list_events(Some(task_id), 400) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ev in events {
        let Ok(parsed) = serde_json::from_str::<RuntimeEvent>(&ev.payload) else {
            continue;
        };
        if let RuntimeEvent::ToolCallStarted { name, .. } = parsed {
            if !out.iter().any(|n| n == &name) {
                out.push(name);
            }
        }
    }
    out
}

fn collect_pitfalls(store: &Store, task_id: &str) -> Vec<String> {
    let Ok(events) = store.list_events(Some(task_id), 400) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ev in events {
        let Ok(parsed) = serde_json::from_str::<RuntimeEvent>(&ev.payload) else {
            continue;
        };
        match parsed {
            RuntimeEvent::Error { message } => {
                let line = message.chars().take(120).collect::<String>();
                if !line.is_empty() {
                    out.push(line);
                }
            }
            RuntimeEvent::ApprovalRequested { tool_name, .. } => {
                out.push(format!("needed approval for `{tool_name}`"));
            }
            RuntimeEvent::Result {
                is_error: true,
                text,
                ..
            } => {
                let line: String = text
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(120)
                    .collect();
                if !line.is_empty() {
                    out.push(line);
                }
            }
            _ => {}
        }
        if out.len() >= 6 {
            break;
        }
    }
    out
}

fn ellipsize_desc(s: &str) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= 140 {
        one
    } else {
        let t: String = one.chars().take(137).collect();
        format!("{t}...")
    }
}

fn yaml_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn unsnooze_due(store: &Store) -> Result<usize, CoreError> {
    let now = Utc::now();
    let mut n = 0;
    for mut q in store.list_questions(Some(QuestionStatus::Snoozed))? {
        if q.not_before.map(|t| t <= now).unwrap_or(true) {
            q.status = q.status.checked_transition(QuestionStatus::Pending)?;
            q.not_before = None;
            q.updated_at = now;
            store.update_question(&q)?;
            n += 1;
        }
    }
    Ok(n)
}

/// Pending → Asked for the highest-value idle question. Stays Pending if the user
/// is already being asked, or nothing clears the value floor.
pub fn promote_idle_question(store: &Store) -> Result<Option<Question>, CoreError> {
    let asked = store.list_questions(Some(QuestionStatus::Asked))?;
    if !asked.is_empty() {
        return Ok(None);
    }
    let now = Utc::now();
    let pending = store.list_questions(Some(QuestionStatus::Pending))?;
    let Some(mut q) = pending
        .into_iter()
        .find(|q| q.value >= IDLE_VALUE_FLOOR && q.not_before.map(|t| t <= now).unwrap_or(true))
    else {
        return Ok(None);
    };
    q.status = q.status.checked_transition(QuestionStatus::Asked)?;
    q.updated_at = now;
    store.update_question(&q)?;
    emit(
        store,
        "question.asked",
        q.task_id.as_deref(),
        serde_json::json!({"question_id": q.id, "value": q.value}),
    );
    Ok(Some(q))
}

pub fn snooze_hours() -> Duration {
    Duration::hours(24)
}

pub fn max_attempts() -> i64 {
    MAX_ATTEMPTS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gap_lines_and_slug() {
        let body = "# Experience\n\n## Result\n\nunknown: latch protocol\nall good\n";
        let gaps = extract_gaps(body);
        assert_eq!(gaps.len(), 1);
        assert!(gaps[0].contains("latch protocol"));
        assert_eq!(
            slugify("What should we know about: latch protocol?"),
            "what-should-we-know-about-latch"
        );
    }

    #[test]
    fn value_grows_with_frequency() {
        let once = question_value(0.6, 1.0, 0.5, 0.8);
        let twice = question_value(0.6, 2.0, 0.5, 0.8);
        assert!(twice > once);
        assert!((twice - 0.48).abs() < 1e-9);
    }

    #[test]
    fn learn_request_and_worthy_heuristic() {
        assert!(is_learn_request("/learn"));
        assert!(is_learn_request("/learn cpu-sample"));
        assert!(is_learn_request("请沉淀成技能"));
        assert!(is_learn_request("save this as a skill"));
        assert!(!is_learn_request("learn about rust"));
        assert_eq!(
            learn_hint("/learn cpu-sample").as_deref(),
            Some("cpu-sample")
        );
        assert!(skill_worthy("", 3));
        assert!(!skill_worthy("done", 0));
        assert!(skill_worthy("- a\n- b\n- c\n", 0));
    }

    #[test]
    fn explicit_learn_writes_candidate_and_install_promotes() {
        use methodus_domain::{Task, TaskStatus};

        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        let now = Utc::now();
        let task = Task {
            id: "task_abc123def456".into(),
            title: "Sample CPU of a process".into(),
            request: "sample cpu of nginx".into(),
            project_id: None,
            status: TaskStatus::Reviewing,
            runtime: None,
            workspace_id: None,
            resolution: None,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        store.insert_task(&task).unwrap();

        let item = propose_skill_from_task(&store, dir.path(), &task.id, Some("cpu-sample"))
            .unwrap()
            .expect("draft");
        assert_eq!(item.source, SKILL_DRAFT_SOURCE);
        assert!(item.path.contains(".candidates"));
        assert_eq!(item.status, KnowledgeStatus::Candidate);
        let draft = fs::read_to_string(dir.path().join(&item.path)).unwrap();
        assert!(draft.contains("status: candidate"));
        assert!(draft.contains("cpu-sample"));

        let live = install_skill_draft(dir.path(), &item).unwrap();
        assert_eq!(
            live,
            format!("skills/{}/SKILL.md", skill_slug("cpu-sample", &task.id))
        );
        let live_body = fs::read_to_string(dir.path().join(&live)).unwrap();
        assert!(live_body.contains("status: committed"));
        assert!(install_skill_draft(dir.path(), &item).is_err());
    }

    #[test]
    fn committed_knowledge_prefers_request_overlap() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        let now = Utc::now();
        fs::create_dir_all(dir.path().join("faces/general/knowledge")).unwrap();
        let latch = "faces/general/knowledge/latch.md";
        fs::write(
            dir.path().join(latch),
            "# Latch protocol\n\nThe latch uses gpio 4.\n",
        )
        .unwrap();
        let other = "faces/general/knowledge/baking.md";
        fs::write(dir.path().join(other), "# Baking\n\nPreheat the oven.\n").unwrap();
        for (id, path) in [("know_latch", latch), ("know_bake", other)] {
            store
                .insert_knowledge(&KnowledgeItem {
                    id: id.into(),
                    face_id: Some("general".into()),
                    project_id: None,
                    path: path.to_string(),
                    content_hash: "h".into(),
                    source: "experience".into(),
                    confidence: Some(0.8),
                    scope: None,
                    status: KnowledgeStatus::Committed,
                    conflict_of: None,
                    version: 1,
                    created_at: now,
                    updated_at: now,
                })
                .unwrap();
        }
        let picked =
            select_committed_knowledge(&store, dir.path(), "general", "debug the latch gpio")
                .unwrap();
        assert_eq!(picked.len(), 1);
        assert!(picked[0].excerpt.contains("gpio 4"));
        assert!(render_knowledge_context(&picked).contains("face-context/knowledge/"));
    }

    fn sample_question(id: &str, value: f64, status: QuestionStatus) -> Question {
        let now = Utc::now();
        Question {
            id: id.to_string(),
            question: format!("What should we know about: {id}?"),
            reason: Some("test".into()),
            task_id: None,
            face_id: Some("general".into()),
            importance: 0.6,
            frequency: 1.0,
            impact: 0.5,
            uncertainty: 0.8,
            value,
            status,
            not_before: None,
            answer: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn idle_promotes_highest_pending_when_free() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        store
            .insert_question(&sample_question("q_low", 0.05, QuestionStatus::Pending))
            .unwrap();
        store
            .insert_question(&sample_question("q_high", 0.48, QuestionStatus::Pending))
            .unwrap();
        let asked = promote_idle_question(&store).unwrap().expect("asked");
        assert_eq!(asked.id, "q_high");
        assert_eq!(asked.status, QuestionStatus::Asked);
        let again = promote_idle_question(&store).unwrap();
        assert!(again.is_none(), "already asking");
        let low = store.get_question("q_low").unwrap().unwrap();
        assert_eq!(low.status, QuestionStatus::Pending);
    }

    #[test]
    fn idle_skips_below_floor() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        store
            .insert_question(&sample_question("q_tiny", 0.01, QuestionStatus::Pending))
            .unwrap();
        assert!(promote_idle_question(&store).unwrap().is_none());
    }
}
