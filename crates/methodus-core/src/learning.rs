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
pub use crate::refine::{
    is_refinement_source, knowledge_inbox_label, knowledge_inbox_tag, HARNESS_NOTE_SOURCE,
    SKILL_PATCH_SOURCE,
};

pub const DEFAULT_BUDGET: &str = r#"{"max_ms":200,"tokens":0}"#;
pub const SKILL_DRAFT_SOURCE: &str = "skill_draft";
pub const MODULE_STUDY_SOURCE: &str = "module_study";
pub const MAX_KNOWLEDGE_INJECT: usize = 5;
const MAX_SNIPPET_CHARS: usize = 900;
const MAX_BOOTSTRAP_ITEMS: usize = 3;
const MAX_FALLBACK_RECENT: usize = 2;
const MAX_ATTEMPTS: i64 = 3;
const DEFAULT_IMPORTANCE: f64 = 0.6;
const DEFAULT_IMPACT: f64 = 0.5;
const DEFAULT_UNCERTAINTY: f64 = 0.8;
const MENTOR_IMPORTANCE: f64 = 0.85;
const MENTOR_IMPACT: f64 = 0.7;
const MENTOR_UNCERTAINTY: f64 = 0.95;
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

/// Everything after `## Result` — keeps nested `##` headings (module study output).
pub fn full_result_section(body: &str) -> String {
    let lower = body.to_lowercase();
    let needle = "## result";
    let Some(idx) = lower.find(needle) else {
        return String::new();
    };
    let rest = &body[idx + needle.len()..];
    rest.trim_start_matches(['\n', '\r']).trim().to_string()
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
    select_committed_knowledge_multi(store, home, &[face_id], request)
}

pub fn select_committed_knowledge_multi(
    store: &Store,
    home: &Path,
    face_ids: &[&str],
    request: &str,
) -> Result<Vec<KnowledgeSnippet>, CoreError> {
    if face_ids.is_empty() {
        return Ok(Vec::new());
    }
    let per_face = (MAX_KNOWLEDGE_INJECT / face_ids.len().max(1)).max(2);
    let mut out = Vec::new();
    let mut used_names = Vec::new();
    for face_id in face_ids {
        let mut batch =
            select_committed_knowledge_for_face(store, home, face_id, request, per_face)?;
        for snip in batch.drain(..) {
            let dest = if face_ids.len() > 1 {
                format!("{}/{}", face_id, snip.dest_name)
            } else {
                snip.dest_name.clone()
            };
            if used_names.iter().any(|n: &String| n == &dest) {
                continue;
            }
            used_names.push(dest.clone());
            out.push(KnowledgeSnippet {
                dest_name: dest,
                ..snip
            });
            if out.len() >= MAX_KNOWLEDGE_INJECT {
                break;
            }
        }
        if out.len() >= MAX_KNOWLEDGE_INJECT {
            break;
        }
    }
    Ok(out)
}

fn select_committed_knowledge_for_face(
    store: &Store,
    home: &Path,
    face_id: &str,
    request: &str,
    limit: usize,
) -> Result<Vec<KnowledgeSnippet>, CoreError> {
    let items = store.list_knowledge(Some(KnowledgeStatus::Committed))?;
    let mut scored: Vec<(usize, KnowledgeItem, String)> = Vec::new();
    for item in items {
        if !knowledge_belongs_to_face(&item, face_id) {
            continue;
        }
        if item.source == SKILL_DRAFT_SOURCE
            || item.source == crate::refine::SKILL_PATCH_SOURCE
            || item.source == crate::refine::HARNESS_NOTE_SOURCE
        {
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
            .take(limit)
            .collect::<Vec<_>>()
    } else if scored.len() <= MAX_BOOTSTRAP_ITEMS.min(limit) {
        scored
    } else {
        let mut recent = scored;
        recent.sort_by_key(|b| std::cmp::Reverse(b.1.updated_at));
        recent.truncate(MAX_FALLBACK_RECENT.min(limit));
        recent
    };

    let mut out = Vec::new();
    let mut used_names = Vec::new();
    for (_, item, body) in chosen {
        if out.len() >= limit {
            break;
        }
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
    out.truncate(limit);
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

/// Short inventory of what this turn actually loaded — first thing the executor should see.
pub fn render_injected_inventory(
    notes: &[KnowledgeSnippet],
    knowledge: &[KnowledgeSnippet],
) -> String {
    let mut out = String::from(
        "\n## Injected this turn\n\n\
         Methodus selected these committed Face items for this task. Prefer them over improvising.\n\n",
    );
    if notes.is_empty() && knowledge.is_empty() {
        out.push_str("- (none)\n");
        return out;
    }
    for snip in notes {
        out.push_str(&format!(
            "- **note** `{}` → `face-context/knowledge/{}`\n",
            snip.title, snip.dest_name
        ));
    }
    for snip in knowledge {
        out.push_str(&format!(
            "- **knowledge** `{}` ({}) → `face-context/knowledge/{}`\n",
            snip.title, snip.origin, snip.dest_name
        ));
    }
    out.push('\n');
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

pub(crate) fn excerpt_body(body: &str, max_chars: usize) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let t: String = trimmed.chars().take(max_chars.saturating_sub(3)).collect();
    format!("{t}...")
}

pub(crate) fn unique_dest_name(path: &str, id: &str, used: &[String]) -> String {
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

pub(crate) fn short_id() -> String {
    Uuid::new_v4().to_string().replace('-', "")[..12].to_string()
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

pub(crate) fn emit(store: &Store, event_type: &str, task_id: Option<&str>, payload: serde_json::Value) {
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
        JobKind::SynthesizeKnowledge => crate::curiosity::run_synthesize_knowledge(store, home, &refs)?,
        JobKind::AnalyzeKnowledgeGaps => {
            crate::curiosity::run_analyze_knowledge_gaps(store, home, &refs)?
        }
        JobKind::AutoResearch => crate::curiosity::run_auto_research(store, home, &refs)?,
        JobKind::SynthesizeMethod => crate::evolution::run_synthesize_method(store, home, &refs)?,
        JobKind::ProposeRefinement => crate::refine::run_propose_refinement(store, home, &refs)?,
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
    let method_id = task_method_id(store, &exp.task_id);
    if crate::curiosity::is_module_expert_experience(&body, method_id.as_deref()) {
        crate::curiosity::enqueue_module_study_jobs(store, &exp, refs)?;
    } else if method_id.as_deref() == Some(crate::ingest::DOC_INGEST_METHOD_ID) {
        crate::ingest::enqueue_ingest_jobs(store, &exp, refs)?;
    } else if method_id.as_deref() == Some(crate::ingest::REPO_SURVEY_METHOD_ID) {
        crate::ingest::enqueue_survey_jobs(store, &exp, refs)?;
    } else {
        let _gaps = extract_gaps(&body);
        enqueue_job(
            store,
            JobKind::DetectGaps,
            &format!("detect:{}", exp.id),
            refs,
            8,
        )?;
    }
    Ok(())
}

fn task_method_id(store: &Store, task_id: &str) -> Option<String> {
    let task = store.get_task(task_id).ok()??;
    let resolution = crate::resolution::Resolution::parse_json(task.resolution.as_deref()?)?;
    resolution.method.map(|m| m.id)
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
    let mut gaps = extract_gaps(&body);
    if gaps.is_empty() && exp.outcome.as_deref() == Some("failed") {
        let result = result_section(&body);
        let fallback = result
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("the last run failed");
        let sig = normalize_gap(fallback);
        if !sig.is_empty() {
            gaps.push(sig);
        }
    }
    let face = exp.face_id.clone().or_else(|| refs.face_id.clone());

    for gap in &gaps {
        upsert_question(store, gap, &exp, face.as_deref())?;
    }
    record_injection_misses(store, home, &exp, &gaps)?;

    enqueue_job(
        store,
        JobKind::ProposeRefinement,
        &format!("refine:exp:{}", exp.id),
        refs,
        6,
    )?;
    Ok(())
}

pub const INJECTED_EVENT: &str = "learning.injected";
pub const INJECTION_MISSED_EVENT: &str = "learning.injection_missed";

/// Count Face notes/knowledge selected for this task. Deduped per task; notes gain hits.
pub fn record_injections(
    store: &Store,
    home: &Path,
    task_id: &str,
    notes: &[KnowledgeSnippet],
    knowledge: &[KnowledgeSnippet],
) -> Result<usize, CoreError> {
    let already = injected_ids_for_task(store, task_id)?;
    let mut n = 0usize;
    for (snip, kind) in notes
        .iter()
        .map(|s| (s, "note"))
        .chain(knowledge.iter().map(|s| (s, "knowledge")))
    {
        if already.iter().any(|id| id == &snip.id) {
            continue;
        }
        if snip.id.starts_with("pack_") || snip.origin.starts_with("team:") {
            emit(
                store,
                INJECTED_EVENT,
                Some(task_id),
                serde_json::json!({
                    "knowledge_id": snip.id,
                    "kind": kind,
                    "origin": snip.origin,
                    "hits": 0,
                }),
            );
            n += 1;
            continue;
        }
        let Some(item) = store.get_knowledge(&snip.id)? else {
            continue;
        };
        let hits = if item.source == crate::refine::HARNESS_NOTE_SOURCE {
            crate::refine::bump_note_inject_hit(home, &item)?
        } else {
            count_injected_events(store, &item.id)? + 1
        };
        emit(
            store,
            INJECTED_EVENT,
            Some(task_id),
            serde_json::json!({
                "knowledge_id": item.id,
                "kind": kind,
                "hits": hits,
            }),
        );
        if item.source == crate::refine::HARNESS_NOTE_SOURCE {
            crate::refine::enqueue_note_skill_promote(store, &item, hits, Some(task_id))?;
        }
        n += 1;
    }
    Ok(n)
}

fn injected_ids_for_task(store: &Store, task_id: &str) -> Result<Vec<String>, CoreError> {
    let mut ids = Vec::new();
    for ev in store.list_events(Some(task_id), 400)? {
        if ev.event_type != INJECTED_EVENT {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&ev.payload) {
            if let Some(id) = v.get("knowledge_id").and_then(|x| x.as_str()) {
                ids.push(id.to_string());
            }
        }
    }
    Ok(ids)
}

fn count_injected_events(store: &Store, knowledge_id: &str) -> Result<i64, CoreError> {
    let mut n = 0i64;
    for ev in store.list_events(None, 2000)? {
        if ev.event_type != INJECTED_EVENT {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&ev.payload) {
            if v.get("knowledge_id").and_then(|x| x.as_str()) == Some(knowledge_id) {
                n += 1;
            }
        }
    }
    Ok(n)
}

/// After this task used injected Face items, the same gap still appeared → downrank + ask.
fn record_injection_misses(
    store: &Store,
    home: &Path,
    exp: &Experience,
    gaps: &[String],
) -> Result<(), CoreError> {
    if gaps.is_empty() {
        return Ok(());
    }
    let gap_text = gaps.join("\n");
    let mut asked = 0usize;
    for ev in store.list_events(Some(&exp.task_id), 400)? {
        if ev.event_type != INJECTED_EVENT {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&ev.payload) else {
            continue;
        };
        let Some(id) = v.get("knowledge_id").and_then(|x| x.as_str()) else {
            continue;
        };
        if id.starts_with("pack_") {
            continue;
        }
        let Some(mut item) = store.get_knowledge(id)? else {
            continue;
        };
        if item.status != KnowledgeStatus::Committed {
            continue;
        }
        let body = fs::read_to_string(home.join(&item.path)).unwrap_or_default();
        if knowledge_score(&gap_text, &item.path, &body) == 0 {
            continue;
        }
        let old = item.confidence.unwrap_or(0.55);
        let next = (old * 0.7).max(0.2);
        item.confidence = Some(next);
        item.updated_at = Utc::now();
        store.update_knowledge(&item)?;
        emit(
            store,
            INJECTION_MISSED_EVENT,
            Some(&exp.task_id),
            serde_json::json!({
                "knowledge_id": item.id,
                "confidence": next,
                "gap": gaps.first().cloned().unwrap_or_default(),
            }),
        );
        if asked < 2 {
            let title = knowledge_title(&item.path, &body);
            let gap0 = gaps.first().map(String::as_str).unwrap_or("this gap");
            let q = format!(
                "Injected `{title}` did not cover: {gap0}. Revise that note, or dismiss it?"
            );
            upsert_mentor_question(
                store,
                &q,
                "injection miss",
                exp,
                item.face_id.as_deref(),
            )?;
            asked += 1;
        }
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

/// Mentor-facing question from module study or knowledge uncertainty.
pub fn upsert_mentor_question(
    store: &Store,
    text: &str,
    reason_detail: &str,
    exp: &Experience,
    face: Option<&str>,
) -> Result<(), CoreError> {
    let question = text.trim();
    if question.is_empty() {
        return Ok(());
    }
    let now = Utc::now();
    if let Some(mut existing) = store.find_question_by_text(question, face)? {
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
        if existing.reason.as_deref().is_none_or(|r| r.is_empty()) {
            existing.reason = Some(format!("mentor: {reason_detail}"));
        }
        store.update_question(&existing)?;
        return Ok(());
    }

    let freq = 1.0;
    let value = question_value(MENTOR_IMPORTANCE, freq, MENTOR_IMPACT, MENTOR_UNCERTAINTY);
    let q = Question {
        id: format!("q_{}", short_id()),
        question: question.to_string(),
        reason: Some(format!("mentor: {reason_detail}")),
        task_id: Some(exp.task_id.clone()),
        face_id: face.map(str::to_string),
        importance: MENTOR_IMPORTANCE,
        frequency: freq,
        impact: MENTOR_IMPACT,
        uncertainty: MENTOR_UNCERTAINTY,
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
        serde_json::json!({
            "question_id": q.id,
            "value": q.value,
            "audience": "mentor",
            "source": "module_study",
        }),
    );
    Ok(())
}

pub fn write_candidate_from_study(
    store: &Store,
    home: &Path,
    face: &str,
    title: &str,
    content: &str,
    exp: &Experience,
    refs: &JobRefs,
) -> Result<(), CoreError> {
    let sources = crate::curiosity::extract_section(content, &["Sources"]);
    let mut body = content.to_string();
    if !sources.is_empty() {
        body = format!("{content}\n\n## Sources (inline)\n\n{sources}");
    }
    write_candidate(
        store,
        home,
        face,
        title,
        &body,
        MODULE_STUDY_SOURCE,
        Some(&exp.task_id),
        Some(&exp.id),
        0.65,
    )?;
    let _ = refs;
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
    if let Some(method) = task_method_id(store, &exp.task_id) {
        if method == crate::ingest::DOC_INGEST_METHOD_ID {
            return crate::ingest::propose_project_from_experience(
                store,
                home,
                exp_id,
                crate::ingest::DOC_INGEST_SOURCE,
            );
        }
        if method == crate::ingest::REPO_SURVEY_METHOD_ID {
            return crate::ingest::propose_project_from_experience(
                store,
                home,
                exp_id,
                crate::ingest::REPO_SURVEY_SOURCE,
            );
        }
    }
    let body = fs::read_to_string(home.join(&exp.path)).unwrap_or_default();
    let result = result_section(&body);
    let mut chunks = extract_knowledge_hints(&body);
    if chunks.is_empty() {
        chunks = extract_gaps(&body);
    }
    if chunks.is_empty() {
        let trimmed = result.trim();
        if trimmed.chars().count() < 20 {
            return Ok(());
        }
        let title_line = trimmed
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("task result");
        chunks.push(ellipsize_desc(title_line));
    }
    let title = chunks[0].clone();
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
pub(crate) fn write_candidate(
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

/// Write or refresh a knowledge candidate at an explicit path (notes / skill patches).
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_knowledge_file(
    store: &Store,
    home: &Path,
    path: &str,
    id: &str,
    face: &str,
    source: &str,
    status: KnowledgeStatus,
    conflict_of: Option<String>,
    confidence: f64,
    body: &str,
    _task_id: Option<&str>,
    scope: Option<&str>,
    emit_task_id: Option<&str>,
) -> Result<Option<KnowledgeItem>, CoreError> {
    let hash = sha256_hex(body.as_bytes());
    let existing = store.list_knowledge_by_path(path)?;
    if let Some(prev) = existing.iter().find(|k| k.content_hash == hash) {
        return Ok(Some(prev.clone()));
    }
    let now = Utc::now();
    if let Some(mut prev) = existing
        .into_iter()
        .find(|k| k.status == KnowledgeStatus::Candidate || k.status == KnowledgeStatus::Conflicted)
    {
        let abs = home.join(&prev.path);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&abs, body)?;
        prev.content_hash = hash;
        prev.status = status;
        prev.conflict_of = conflict_of;
        prev.updated_at = now;
        store.update_knowledge(&prev)?;
        return Ok(Some(prev));
    }
    let abs = home.join(path);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&abs, body)?;
    let item = KnowledgeItem {
        id: id.to_string(),
        face_id: Some(face.to_string()),
        project_id: None,
        path: path.to_string(),
        content_hash: hash,
        source: source.to_string(),
        confidence: Some(confidence),
        scope: scope.map(str::to_string),
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
        emit_task_id,
        serde_json::json!({"knowledge_id": id, "path": path, "kind": source}),
    );
    Ok(Some(item))
}

/// Force a skill draft from a task (tests). Never writes a live skill.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn propose_skill_from_task(
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
        source: Some("task_distill".to_string()),
    };
    write_skill_candidate(store, home, &refs, hint, true)
}

pub fn install_skill_draft(
    home: &Path,
    item: &KnowledgeItem,
    replace: bool,
) -> Result<String, CoreError> {
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
    if live_abs.exists() && !replace {
        return Err(CoreError::Other(format!(
            "skill `{name}` already exists at {live_rel} — use replace in /inbox"
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
            .map(|e| {
                let body = fs::read_to_string(home.join(&e.path)).unwrap_or_default();
                let method_id = task_method_id(store, &e.task_id);
                if crate::curiosity::is_module_expert_experience(&body, method_id.as_deref()) {
                    full_result_section(&body)
                } else {
                    result_section(&body)
                }
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    let study_skill = refs.experience_id.as_deref().is_some_and(|eid| {
        store
            .get_experience(eid)
            .ok()
            .flatten()
            .is_some_and(|e| {
                let body = fs::read_to_string(home.join(&e.path)).unwrap_or_default();
                let method_id = task_method_id(store, &e.task_id);
                crate::curiosity::is_module_expert_experience(&body, method_id.as_deref())
            })
    });
    let skill_from_study = if study_skill {
        refs.experience_id.as_deref().and_then(|eid| {
            store.get_experience(eid).ok().flatten().map(|e| {
                let body = fs::read_to_string(home.join(&e.path)).unwrap_or_default();
                crate::curiosity::extract_skill_section(&body)
            })
        })
    } else {
        None
    };
    let skill_from_study = skill_from_study.filter(|s| !s.trim().is_empty());
    let explicit = explicit || study_skill;
    if !explicit && !skill_worthy(&result, tools.len()) {
        return Ok(None);
    }
    if tools.is_empty() && result.trim().is_empty() && skill_from_study.is_none() && !explicit {
        return Ok(None);
    }

    let title = hint
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            skill_from_study.as_ref().and_then(|s| {
                s.lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty() && !l.starts_with('#'))
            })
        })
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
    let procedure = if let Some(skill) = skill_from_study.as_deref() {
        skill.trim().to_string()
    } else {
        let from_events = collect_procedure_steps(store, task_id);
        if !from_events.is_empty() {
            from_events
        } else if result.trim().is_empty() {
            format!("1. Revisit the original request:\n   {}", task.request)
        } else {
            result.trim().to_string()
        }
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
    if let Some((name, live)) = crate::refine::find_related_skill(home, title) {
        let live_body = fs::read_to_string(&live).unwrap_or_default();
        let add_procedure: Vec<String> = procedure
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| {
                l.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ')
                    .trim()
                    .to_string()
            })
            .filter(|c| {
                let key = c.chars().take(40).collect::<String>().to_lowercase();
                !live_body.to_lowercase().contains(&key) && c.chars().count() >= 8
            })
            .take(6)
            .collect();
        let add_pitfalls: Vec<String> = pitfalls
            .iter()
            .filter(|c| {
                let key = c.chars().take(40).collect::<String>().to_lowercase();
                !live_body.to_lowercase().contains(&key) && c.chars().count() >= 8
            })
            .cloned()
            .take(6)
            .collect();
        return crate::refine::write_skill_patch(
            store,
            home,
            refs,
            &task,
            &face,
            &name,
            add_procedure,
            add_pitfalls,
        );
    }
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
         Re-run the original request and confirm the same outcome.\n\n\
         ## Evidence\n\n\
         - task: `{task_id}`\n\
         - distilled: {now}\n"
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
    let (path, status, conflict_of) = if live.exists() {
        if let Some((name, live_path)) = crate::refine::find_related_skill(home, title) {
            let live_body = fs::read_to_string(&live_path).unwrap_or_default();
            let add_procedure = procedure
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !live_body.contains(*l))
                .map(str::to_string)
                .take(6)
                .collect();
            return crate::refine::write_skill_patch(
                store,
                home,
                refs,
                &task,
                &face,
                &name,
                add_procedure,
                pitfalls.clone(),
            );
        }
        (
            format!("skills/.candidates/{slug}--{id}/SKILL.md"),
            KnowledgeStatus::Conflicted,
            store
                .list_knowledge_by_path(&format!("skills/{slug}/SKILL.md"))?
                .into_iter()
                .find(|k| k.status == KnowledgeStatus::Committed)
                .map(|k| k.id),
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

pub(crate) fn collect_tools(store: &Store, task_id: &str) -> Vec<String> {
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

/// Ordered tool steps from the session event stream (trajectory-first distillation).
pub(crate) fn collect_procedure_steps(store: &Store, task_id: &str) -> String {
    let Ok(events) = store.list_events(Some(task_id), 400) else {
        return String::new();
    };
    let mut steps = Vec::new();
    for ev in events {
        let Ok(parsed) = serde_json::from_str::<RuntimeEvent>(&ev.payload) else {
            continue;
        };
        if let RuntimeEvent::ToolCallStarted { name, input, .. } = parsed {
            let detail = summarize_tool_input(&name, &input);
            let line = if detail.is_empty() {
                format!("Use `{name}`")
            } else {
                format!("`{name}` — {detail}")
            };
            if steps.last().is_some_and(|prev: &String| prev == &line) {
                continue;
            }
            steps.push(line);
        }
    }
    if steps.is_empty() {
        return String::new();
    }
    steps
        .into_iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {s}", i + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn summarize_tool_input(name: &str, input: &serde_json::Value) -> String {
    let pick = |keys: &[&str]| -> Option<String> {
        for key in keys {
            if let Some(v) = input.get(*key).and_then(|v| v.as_str()) {
                let t = v.trim();
                if !t.is_empty() {
                    return Some(ellipsize_desc(t));
                }
            }
        }
        None
    };
    match name {
        "Read" | "Write" | "Edit" | "StrReplace" => pick(&["path", "file_path", "target_file"]),
        "Shell" | "Bash" => pick(&["command"]),
        "Grep" | "Glob" => pick(&["pattern", "glob_pattern"]),
        _ => pick(&["path", "command", "pattern", "query", "url"]),
    }
    .unwrap_or_default()
}

pub(crate) fn collect_pitfalls(store: &Store, task_id: &str) -> Vec<String> {
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
    fn plain_result_becomes_candidate_and_failed_run_asks() {
        use crate::scheduler;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("state.db")).unwrap());
        let now = Utc::now();
        let rel = "faces/general/experiences/exp_plain.md";
        fs::create_dir_all(dir.path().join("faces/general/experiences")).unwrap();
        fs::write(
            dir.path().join(rel),
            "# Experience `exp_plain`\n\n\
             - outcome: success\n\n\
             ## Request\n\nfix the latch\n\n\
             ## Result\n\n\
             The latch on the carrier board uses gpio 4 with a 3.3V pull-up.\n",
        )
        .unwrap();
        store
            .insert_experience(&Experience {
                id: "exp_plain".into(),
                task_id: "task_plain".into(),
                face_id: Some("general".into()),
                path: rel.into(),
                content_hash: "h".into(),
                outcome: Some("success".into()),
                summary: Some("The latch on the carrier board uses gpio 4.".into()),
                created_at: now,
                updated_at: now,
            })
            .unwrap();
        store
            .insert_task(&methodus_domain::Task {
                id: "task_plain".into(),
                title: "fix the latch".into(),
                request: "fix the latch".into(),
                project_id: None,
                status: methodus_domain::TaskStatus::Reviewing,
                runtime: None,
                workspace_id: None,
                resolution: None,
                version: 1,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
        enqueue_extract(&store, &store.get_experience("exp_plain").unwrap().unwrap()).unwrap();
        scheduler::tick(&store, dir.path()).unwrap();
        let cands = store
            .list_knowledge(Some(KnowledgeStatus::Candidate))
            .unwrap();
        assert!(
            cands.iter().any(|k| k.source == "experience"),
            "plain result should become candidate knowledge"
        );
        assert!(store.list_questions(None).unwrap().is_empty());

        let fail_rel = "faces/general/experiences/exp_fail.md";
        fs::write(
            dir.path().join(fail_rel),
            "# Experience `exp_fail`\n\n\
             - outcome: failed\n\n\
             ## Result\n\n\
             executor timed out talking to the probe\n",
        )
        .unwrap();
        store
            .insert_experience(&Experience {
                id: "exp_fail".into(),
                task_id: "task_fail".into(),
                face_id: Some("general".into()),
                path: fail_rel.into(),
                content_hash: "h2".into(),
                outcome: Some("failed".into()),
                summary: Some("executor timed out talking to the probe".into()),
                created_at: now,
                updated_at: now,
            })
            .unwrap();
        store
            .insert_task(&methodus_domain::Task {
                id: "task_fail".into(),
                title: "probe".into(),
                request: "probe".into(),
                project_id: None,
                status: methodus_domain::TaskStatus::Failed,
                runtime: None,
                workspace_id: None,
                resolution: None,
                version: 1,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
        enqueue_extract(&store, &store.get_experience("exp_fail").unwrap().unwrap()).unwrap();
        scheduler::tick(&store, dir.path()).unwrap();
        let qs = store.list_questions(None).unwrap();
        assert!(
            qs.iter().any(|q| q.question.contains("timed out")),
            "failed run should open a question: {qs:?}"
        );
    }

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
    fn skill_worthy_heuristic() {
        assert!(skill_worthy("", 3));
        assert!(!skill_worthy("done", 0));
        assert!(skill_worthy("- a\n- b\n- c\n", 0));
    }

    #[test]
    fn skill_draft_writes_candidate_and_install_promotes() {
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

        let live = install_skill_draft(dir.path(), &item, false).unwrap();
        assert_eq!(
            live,
            format!("skills/{}/SKILL.md", skill_slug("cpu-sample", &task.id))
        );
        let live_body = fs::read_to_string(dir.path().join(&live)).unwrap();
        assert!(live_body.contains("status: committed"));
        assert!(install_skill_draft(dir.path(), &item, false).is_err());
        assert!(install_skill_draft(dir.path(), &item, true).is_ok());
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
        let inv = render_injected_inventory(&[], &picked);
        assert!(inv.contains("## Injected this turn"));
        assert!(inv.contains("**knowledge**"));
        assert!(render_injected_inventory(&[], &[]).contains("(none)"));
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

    #[test]
    fn module_study_synthesizes_knowledge_and_mentor_questions() {
        use crate::scheduler;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("state.db")).unwrap());
        let now = Utc::now();
        let rel = "faces/general/experiences/exp_study.md";
        fs::create_dir_all(dir.path().join("faces/general/experiences")).unwrap();
        fs::create_dir_all(dir.path().join("faces/general/knowledge")).unwrap();
        let result = "## Sources\n\n- `src/main.c`\n\n\
             ## Knowledge\n\n### Boot\n\nCalls init(). TBD: clock source.\n\n\
             ## Open Questions (for Mentor)\n\n\
             - Which clock source on rev B?\n";
        fs::write(
            dir.path().join(rel),
            format!(
                "# Experience `exp_study`\n\n\
                 - task: `task_study`\n\
                 - face: `general`\n\
                 - outcome: success\n\
                 - mode: module_expert\n\
                 - created: {}\n\n\
                 ## Request\n\nstudy boot\n\n\
                 ## Result\n\n{result}",
                now.to_rfc3339()
            ),
        )
        .unwrap();
        store
            .insert_experience(&Experience {
                id: "exp_study".into(),
                task_id: "task_study".into(),
                face_id: Some("general".into()),
                path: rel.into(),
                content_hash: "h".into(),
                outcome: Some("success".into()),
                summary: Some("study boot".into()),
                created_at: now,
                updated_at: now,
            })
            .unwrap();
        enqueue_extract(&store, &store.get_experience("exp_study").unwrap().unwrap()).unwrap();
        scheduler::tick(&store, dir.path()).unwrap();

        let cands = store
            .list_knowledge(Some(KnowledgeStatus::Candidate))
            .unwrap();
        assert!(
            cands.iter().any(|k| k.source == MODULE_STUDY_SOURCE),
            "expected module_study candidate, got {cands:?}"
        );
        let qs = store.list_questions(None).unwrap();
        assert!(
            qs.iter()
                .any(|q| q.question.contains("clock source") && q.reason.as_deref().is_some_and(|r| r.starts_with("mentor:"))),
            "expected mentor question, got {qs:?}"
        );
    }
}
