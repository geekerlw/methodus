//! Trajectory refinements: harness notes (B3) and incremental skill patches (B4).
//!
//! `propose_refinement` is rule-based (no LLM). A later budgeted polish may rewrite
//! the candidate JSON; apply happens only after `/inbox` commit.

use std::fs;
use std::path::{Path, PathBuf};

use methodus_domain::{JobKind, JobStatus, KnowledgeItem, KnowledgeStatus};
use methodus_store::Store;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::learning::{
    collect_pitfalls, collect_procedure_steps, collect_tools, enqueue_job, short_id, slugify,
    write_knowledge_file, JobRefs, SKILL_DRAFT_SOURCE,
};
use crate::workspace::is_safe_segment;

pub const HARNESS_NOTE_SOURCE: &str = "harness_note";
pub const SKILL_PATCH_SOURCE: &str = "skill_patch";
pub const NOTE_PROMOTE_HITS: i64 = 3;
pub const REFINE_LLM_EVENT: &str = "learning.refine_llm";
const MAX_NOTES_INJECT: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DistillKind {
    None,
    Patch,
    Skill,
    Note,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefineTargetKind {
    Note,
    Skill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefineOp {
    Create,
    Update,
}

fn default_planner() -> String {
    "rules".into()
}

/// Structured refinement the inbox can render and apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefinementProposal {
    pub target_kind: RefineTargetKind,
    pub target_id: String,
    pub op: RefineOp,
    #[serde(default)]
    pub add_procedure: Vec<String>,
    #[serde(default)]
    pub add_pitfalls: Vec<String>,
    #[serde(default)]
    pub note_body: Option<String>,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    #[serde(default)]
    pub rationale: String,
    #[serde(default)]
    pub hits: i64,
    /// `rules` (sync distill) or `llm` (budgeted polish).
    #[serde(default = "default_planner")]
    pub planner: String,
}

/// Executor JSON for polishing a rules draft. Unknown keys ignored.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LlmRefineOut {
    #[serde(default)]
    pub skip: bool,
    #[serde(default)]
    pub rationale: Option<String>,
    #[serde(default)]
    pub note_body: Option<String>,
    #[serde(default)]
    pub add_procedure: Option<Vec<String>>,
    #[serde(default)]
    pub add_pitfalls: Option<Vec<String>>,
}

pub fn is_refinement_source(source: &str) -> bool {
    source == HARNESS_NOTE_SOURCE || source == SKILL_PATCH_SOURCE
}

pub fn knowledge_inbox_tag(source: &str) -> &'static str {
    match source {
        SKILL_DRAFT_SOURCE => "S",
        SKILL_PATCH_SOURCE => "P",
        HARNESS_NOTE_SOURCE => "N",
        _ => "K",
    }
}

pub fn knowledge_inbox_label(source: &str) -> &'static str {
    match source {
        SKILL_DRAFT_SOURCE => "skill draft",
        SKILL_PATCH_SOURCE => "skill patch",
        HARNESS_NOTE_SOURCE => "harness note",
        _ => "knowledge",
    }
}

/// Job handler: smallest trajectory-backed edit — at most one note, patch, or skill draft.
pub fn run_propose_refinement(
    store: &Store,
    home: &Path,
    refs: &JobRefs,
) -> Result<(), CoreError> {
    let task_id = refs
        .task_id
        .as_deref()
        .ok_or_else(|| CoreError::Other("propose_refinement missing task_id".into()))?;
    let task = store
        .get_task(task_id)?
        .ok_or_else(|| CoreError::TaskNotFound(task_id.to_string()))?;
    let tools = collect_tools(store, task_id);
    let pitfalls = collect_pitfalls(store, task_id);
    let procedure = numbered_steps(&collect_procedure_steps(store, task_id));
    if tools.is_empty() && pitfalls.is_empty() && procedure.is_empty() {
        maybe_enqueue_knowledge_fallback(store, refs)?;
        return Ok(());
    }
    let face = refs
        .face_id
        .clone()
        .or_else(|| face_from_task(&task))
        .unwrap_or_else(|| "general".to_string());
    let related = find_related_skill(home, &task.title);
    let (new_procedure, new_pitfalls) = if let Some((_, live)) = &related {
        let live_body = fs::read_to_string(live).unwrap_or_default();
        (
            new_lines(&procedure, &live_body),
            new_lines(&pitfalls, &live_body),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let already_open = task_has_open_distill(store, home, task_id)?;
    let kind = choose_refinement(
        &procedure,
        &pitfalls,
        tools.len(),
        related.is_some(),
        &new_procedure,
        &new_pitfalls,
        already_open,
    );
    match kind {
        DistillKind::None => {
            if !already_open {
                maybe_enqueue_knowledge_fallback(store, refs)?;
            }
            Ok(())
        }
        DistillKind::Patch => {
            let Some((name, _)) = related else {
                return Ok(());
            };
            let _ = write_skill_patch(
                store,
                home,
                refs,
                &task,
                &face,
                &name,
                new_procedure,
                new_pitfalls,
            )?;
            Ok(())
        }
        DistillKind::Skill => {
            enqueue_job(
                store,
                JobKind::ProposeSkill,
                &format!("skill:task:{}", task.id),
                refs,
                4,
            )?;
            Ok(())
        }
        DistillKind::Note => {
            let _ = upsert_harness_note(store, home, refs, &task, &face, &procedure, &pitfalls)?;
            Ok(())
        }
    }
}

fn maybe_enqueue_knowledge_fallback(store: &Store, refs: &JobRefs) -> Result<(), CoreError> {
    let Some(exp_id) = refs.experience_id.as_deref() else {
        return Ok(());
    };
    let mut propose_refs = refs.clone();
    propose_refs.source = Some("experience".to_string());
    enqueue_job(
        store,
        JobKind::ProposeKnowledge,
        &format!("propose:exp:{exp_id}"),
        &propose_refs,
        5,
    )?;
    Ok(())
}

/// One artifact per execution task: patch XOR skill draft XOR note.
pub(crate) fn choose_refinement(
    procedure: &[String],
    pitfalls: &[String],
    tool_count: usize,
    has_related_skill: bool,
    new_procedure: &[String],
    new_pitfalls: &[String],
    already_open: bool,
) -> DistillKind {
    if already_open {
        return DistillKind::None;
    }
    if has_related_skill {
        if new_procedure.is_empty() && new_pitfalls.is_empty() {
            return DistillKind::None;
        }
        return DistillKind::Patch;
    }
    let substantial = procedure.iter().filter(|s| is_substantial_step(s)).count();
    if (substantial >= 3 || (tool_count >= 3 && substantial >= 1)) && substantial > 0 {
        return DistillKind::Skill;
    }
    if !pitfalls.is_empty() || substantial > 0 {
        return DistillKind::Note;
    }
    DistillKind::None
}

/// Bare `Use \`Tool\`` is not a reusable step; path/command snippets are.
pub(crate) fn is_substantial_step(step: &str) -> bool {
    let t = step.trim();
    if t.is_empty() {
        return false;
    }
    if t.starts_with("Use `") && !t.contains(" — ") {
        return false;
    }
    t.contains('/') || t.contains(" — ") || t.chars().count() >= 20
}

fn task_has_open_distill(store: &Store, home: &Path, task_id: &str) -> Result<bool, CoreError> {
    for item in store.list_knowledge(Some(KnowledgeStatus::Candidate))? {
        if !is_distill_source(&item.source) {
            continue;
        }
        if candidate_task_id(home, &item).as_deref() == Some(task_id) {
            return Ok(true);
        }
    }
    for job in store.list_jobs()? {
        if job.kind != JobKind::ProposeSkill {
            continue;
        }
        if job.status != JobStatus::Queued && job.status != JobStatus::Running {
            continue;
        }
        let refs: JobRefs = serde_json::from_str(&job.input_refs).unwrap_or_default();
        if refs.task_id.as_deref() == Some(task_id) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_distill_source(source: &str) -> bool {
    source == HARNESS_NOTE_SOURCE
        || source == SKILL_PATCH_SOURCE
        || source == SKILL_DRAFT_SOURCE
}

fn candidate_task_id(home: &Path, item: &KnowledgeItem) -> Option<String> {
    let body = fs::read_to_string(home.join(&item.path)).ok()?;
    parse_proposal_task_id(&body).or_else(|| frontmatter_str(&body, "source_task"))
}

pub fn find_related_skill(home: &Path, title: &str) -> Option<(String, PathBuf)> {
    let base = slugify(title);
    if base.is_empty() || base == "note" {
        return None;
    }
    let dir = home.join("skills");
    let entries = fs::read_dir(&dir).ok()?;
    let mut best: Option<(usize, String, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !is_safe_segment(&name) {
            continue;
        }
        let live = path.join("SKILL.md");
        if !live.is_file() {
            continue;
        }
        let score = skill_name_score(&base, &name);
        if score == 0 {
            continue;
        }
        match &best {
            Some((best_score, _, _)) if *best_score >= score => {}
            _ => best = Some((score, name, live)),
        }
    }
    best.map(|(_, name, live)| (name, live))
}

/// Apply an inbox-committed skill patch onto the live SKILL.md.
pub fn apply_skill_patch(home: &Path, item: &KnowledgeItem) -> Result<String, CoreError> {
    if item.source != SKILL_PATCH_SOURCE {
        return Ok(item.path.clone());
    }
    let body = fs::read_to_string(home.join(&item.path)).unwrap_or_default();
    let proposal = parse_proposal(&body).ok_or_else(|| {
        CoreError::Other("skill patch is missing a structured proposal".into())
    })?;
    let name = proposal.target_id.clone();
    if !is_safe_segment(&name) {
        return Err(CoreError::Other(format!("unsafe skill name: {name}")));
    }
    let live_rel = format!("skills/{name}/SKILL.md");
    let live_abs = home.join(&live_rel);
    if !live_abs.is_file() {
        return Err(CoreError::Other(format!(
            "live skill `{name}` is gone — reject this patch"
        )));
    }
    let live = fs::read_to_string(&live_abs).unwrap_or_default();
    let merged = merge_skill_markdown(&live, &proposal.add_procedure, &proposal.add_pitfalls);
    fs::write(&live_abs, merged)?;
    Ok(live_rel)
}

/// Promote a harness note candidate into `faces/<id>/notes/`.
pub fn apply_harness_note(
    store: &Store,
    home: &Path,
    item: &KnowledgeItem,
) -> Result<(String, i64), CoreError> {
    if item.source != HARNESS_NOTE_SOURCE {
        return Ok((item.path.clone(), 1));
    }
    let face = item
        .face_id
        .as_deref()
        .filter(|s| is_safe_segment(s))
        .unwrap_or("general");
    let body = fs::read_to_string(home.join(&item.path)).unwrap_or_default();
    let proposal = parse_proposal(&body);
    let slug = proposal
        .as_ref()
        .map(|p| p.target_id.clone())
        .filter(|s| is_safe_segment(s))
        .unwrap_or_else(|| slug_from_path(&item.path));
    if !is_safe_segment(&slug) {
        return Err(CoreError::Other(format!("unsafe note slug: {slug}")));
    }
    let live_rel = format!("faces/{face}/notes/{slug}.md");
    let live_abs = home.join(&live_rel);
    if let Some(parent) = live_abs.parent() {
        fs::create_dir_all(parent)?;
    }
    let (live_body, hits) = if live_abs.is_file() {
        let existing = fs::read_to_string(&live_abs).unwrap_or_default();
        let hits = frontmatter_i64(&existing, "hits").unwrap_or(0);
        (merge_note_markdown(&existing, proposal.as_ref()), hits)
    } else {
        (committed_note_body(&slug, proposal.as_ref(), &body, 0), 0)
    };
    fs::write(&live_abs, live_body)?;
    let _ = store;
    Ok((live_rel, hits))
}

/// After a note is committed with enough hits, draft a skill from the same Face.
pub fn enqueue_note_skill_promote(
    store: &Store,
    item: &KnowledgeItem,
    hits: i64,
    task_id: Option<&str>,
) -> Result<(), CoreError> {
    if hits < NOTE_PROMOTE_HITS {
        return Ok(());
    }
    let Some(tid) = task_id.filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    let refs = JobRefs {
        experience_id: None,
        task_id: Some(tid.to_string()),
        face_id: item.face_id.clone(),
        question_id: None,
        source: Some(format!("note_promote:{}", item.id)),
    };
    enqueue_job(
        store,
        JobKind::ProposeSkill,
        &format!("skill:note:{}", item.id),
        &refs,
        4,
    )?;
    Ok(())
}

/// Injection count on a committed note. Promote to a skill after `NOTE_PROMOTE_HITS`.
pub fn bump_note_inject_hit(home: &Path, item: &KnowledgeItem) -> Result<i64, CoreError> {
    let abs = home.join(&item.path);
    let body = fs::read_to_string(&abs).unwrap_or_default();
    let hits = frontmatter_i64(&body, "hits").unwrap_or(0) + 1;
    let next = if body.starts_with("---\n") {
        replace_frontmatter_i64(&body, "hits", hits)
    } else {
        format!("---\nhits: {hits}\n---\n{body}")
    };
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&abs, next)?;
    Ok(hits)
}

pub fn select_committed_notes(
    store: &Store,
    home: &Path,
    face_ids: &[&str],
    request: &str,
) -> Result<Vec<crate::learning::KnowledgeSnippet>, CoreError> {
    if face_ids.is_empty() {
        return Ok(Vec::new());
    }
    let items = store.list_knowledge(Some(KnowledgeStatus::Committed))?;
    let mut scored: Vec<(usize, KnowledgeItem, String)> = Vec::new();
    for item in items {
        if item.source != HARNESS_NOTE_SOURCE {
            continue;
        }
        if !face_ids.iter().any(|f| item.face_id.as_deref() == Some(*f)) {
            continue;
        }
        let abs = home.join(&item.path);
        let Ok(body) = fs::read_to_string(&abs) else {
            continue;
        };
        if body.trim().is_empty() {
            continue;
        }
        let score = note_score(request, &item.path, &body);
        scored.push((score, item, body));
    }
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.updated_at.cmp(&a.1.updated_at))
    });
    let mut out = Vec::new();
    let mut used = Vec::new();
    for (_, item, body) in scored {
        if out.len() >= MAX_NOTES_INJECT {
            break;
        }
        let dest = format!("notes/{}", crate::learning::unique_dest_name(&item.path, &item.id, &used));
        used.push(dest.clone());
        out.push(crate::learning::KnowledgeSnippet {
            id: item.id.clone(),
            src_path: home.join(&item.path),
            dest_name: dest,
            title: note_title(&item.path, &body),
            excerpt: crate::learning::excerpt_body(&body, 400),
            origin: "note".to_string(),
        });
    }
    Ok(out)
}

pub fn render_notes_context(snippets: &[crate::learning::KnowledgeSnippet]) -> String {
    if snippets.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n## Harness notes (Face memory)\n\n\
         Short durable tactics for this Face. Prefer them over improvising. \
         Full copies: `face-context/knowledge/notes/`.\n",
    );
    for snip in snippets {
        out.push_str(&format!(
            "\n### {}\n\n{}\n\n- file: `face-context/knowledge/{}`\n",
            snip.title, snip.excerpt, snip.dest_name
        ));
    }
    out
}

pub fn render_refinement_detail(home: &Path, item: &KnowledgeItem) -> String {
    let candidate = fs::read_to_string(home.join(&item.path))
        .unwrap_or_else(|_| format!("(missing file)\n{}", item.path));
    if item.source == SKILL_PATCH_SOURCE {
        let mut out = String::from("## Skill patch\n\nApply appends Procedure / Pitfalls to the live skill. The base prompt is not rewritten.\n\n");
        if let Some(proposal) = parse_proposal(&candidate) {
            out.push_str(&format!(
                "**Target:** `{}` · **op:** {:?}\n\n**Why:** {}\n\n",
                proposal.target_id, proposal.op, proposal.rationale
            ));
            if !proposal.evidence_refs.is_empty() {
                out.push_str("**Evidence:**\n");
                for e in &proposal.evidence_refs {
                    out.push_str(&format!("- `{e}`\n"));
                }
                out.push('\n');
            }
            let live_rel = format!("skills/{}/SKILL.md", proposal.target_id);
            if let Ok(live) = fs::read_to_string(home.join(&live_rel)) {
                out.push_str("### Live skill (current)\n\n```markdown\n");
                out.push_str(&excerpt_md(&live, 40));
                out.push_str("\n```\n\n");
            }
            out.push_str("### Proposed additions\n\n");
            if !proposal.add_procedure.is_empty() {
                out.push_str("**Procedure**\n");
                for s in &proposal.add_procedure {
                    out.push_str(&format!("+ {s}\n"));
                }
                out.push('\n');
            }
            if !proposal.add_pitfalls.is_empty() {
                out.push_str("**Pitfalls**\n");
                for s in &proposal.add_pitfalls {
                    out.push_str(&format!("+ {s}\n"));
                }
                out.push('\n');
            }
        } else {
            out.push_str(&candidate);
        }
        return out;
    }
    if item.source == HARNESS_NOTE_SOURCE {
        let mut out = String::from(
            "## Harness note\n\nCheap Face memory — not a skill. Commit writes `faces/<id>/notes/`.\n\n",
        );
        if let Some(proposal) = parse_proposal(&candidate) {
            out.push_str(&format!(
                "**Note:** `{}` · **op:** {:?} · **hits:** {}\n\n**Why:** {}\n\n",
                proposal.target_id, proposal.op, proposal.hits, proposal.rationale
            ));
            if let Some(body) = proposal.note_body.as_deref() {
                out.push_str("### Body\n\n");
                out.push_str(body);
                out.push_str("\n\n");
            }
            if proposal.hits + 1 >= NOTE_PROMOTE_HITS {
                out.push_str(&format!(
                    "After commit, {NOTE_PROMOTE_HITS} injections on later tasks can promote this to a skill draft.\n"
                ));
            }
        } else {
            out.push_str(&candidate);
        }
        return out;
    }
    candidate
}

pub(crate) fn write_skill_patch(
    store: &Store,
    home: &Path,
    refs: &JobRefs,
    task: &methodus_domain::Task,
    face: &str,
    skill_name: &str,
    add_procedure: Vec<String>,
    add_pitfalls: Vec<String>,
) -> Result<Option<KnowledgeItem>, CoreError> {
    if add_procedure.is_empty() && add_pitfalls.is_empty() {
        return Ok(None);
    }
    let proposal = RefinementProposal {
        target_kind: RefineTargetKind::Skill,
        target_id: skill_name.to_string(),
        op: RefineOp::Update,
        add_procedure: add_procedure.clone(),
        add_pitfalls: add_pitfalls.clone(),
        note_body: None,
        evidence_refs: vec![task.id.clone()],
        rationale: format!(
            "Trajectory of `{}` adds steps or pitfalls not yet in `{skill_name}`.",
            task.title
        ),
        hits: 1,
        planner: default_planner(),
    };
    let id = format!("know_{}", short_id());
    let rel = format!("skills/.candidates/{skill_name}--patch-{id}/SKILL.patch.md");
    let body = format_patch_markdown(&proposal, &task.id);
    write_knowledge_file(
        store,
        home,
        &rel,
        &id,
        face,
        SKILL_PATCH_SOURCE,
        KnowledgeStatus::Candidate,
        None,
        0.6,
        &body,
        Some(&task.id),
        Some("skill"),
        refs.task_id.as_deref(),
    )
}

fn upsert_harness_note(
    store: &Store,
    home: &Path,
    refs: &JobRefs,
    task: &methodus_domain::Task,
    face: &str,
    procedure: &[String],
    pitfalls: &[String],
) -> Result<Option<KnowledgeItem>, CoreError> {
    let tactic = pitfalls
        .first()
        .cloned()
        .or_else(|| procedure.first().cloned())
        .unwrap_or_else(|| format!("lesson from {}", task.title));
    let slug = slugify(&format!("{} {tactic}", task.title));
    if !is_safe_segment(&slug) {
        return Ok(None);
    }
    let live_rel = format!("faces/{face}/notes/{slug}.md");
    let live_abs = home.join(&live_rel);
    let existing_live = live_abs.is_file();
    let mut hits = 1i64;
    if existing_live {
        let prev = fs::read_to_string(&live_abs).unwrap_or_default();
        hits = frontmatter_i64(&prev, "hits").unwrap_or(1) + 1;
    }
    let cand_path = format!("faces/{face}/notes/.candidates/{slug}.md");
    if let Ok(rows) = store.list_knowledge_by_path(&cand_path) {
        if let Some(prev) = rows.iter().find(|k| k.status == KnowledgeStatus::Candidate) {
            hits = hits.max(frontmatter_i64(
                &fs::read_to_string(home.join(&prev.path)).unwrap_or_default(),
                "hits",
            ).unwrap_or(1) + 1);
        }
    }
    let op = if existing_live {
        RefineOp::Update
    } else {
        RefineOp::Create
    };
    let note_body = format!("- {tactic}");
    let proposal = RefinementProposal {
        target_kind: RefineTargetKind::Note,
        target_id: slug.clone(),
        op,
        add_procedure: procedure.to_vec(),
        add_pitfalls: pitfalls.to_vec(),
        note_body: Some(note_body.clone()),
        evidence_refs: vec![task.id.clone()],
        rationale: format!("Reusable tactic from `{}`.", task.title),
        hits,
        planner: default_planner(),
    };
    let conflict_of = if existing_live {
        store
            .list_knowledge_by_path(&live_rel)?
            .into_iter()
            .find(|k| k.status == KnowledgeStatus::Committed)
            .map(|k| k.id)
    } else {
        None
    };
    let status = if conflict_of.is_some() {
        KnowledgeStatus::Conflicted
    } else {
        KnowledgeStatus::Candidate
    };
    let id = format!("know_{}", short_id());
    let body = format_note_markdown(&proposal, &task.id);
    write_knowledge_file(
        store,
        home,
        &cand_path,
        &id,
        face,
        HARNESS_NOTE_SOURCE,
        status,
        conflict_of,
        0.55,
        &body,
        Some(&task.id),
        Some("note"),
        refs.task_id.as_deref(),
    )
}

fn format_patch_markdown(p: &RefinementProposal, task_id: &str) -> String {
    let proc = if p.add_procedure.is_empty() {
        "_none_".to_string()
    } else {
        p.add_procedure
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let pits = if p.add_pitfalls.is_empty() {
        "_none_".to_string()
    } else {
        p.add_pitfalls
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let json = serde_json::to_string_pretty(p).unwrap_or_else(|_| "{}".into());
    format!(
        "---\n\
         kind: skill_patch\n\
         name: {}\n\
         op: update\n\
         planner: {}\n\
         status: candidate\n\
         source_task: {task_id}\n\
         ---\n\n\
         # Patch `{}`\n\n\
         {}\n\n\
         ## Add to Procedure\n\n\
         {proc}\n\n\
         ## Add to Pitfalls\n\n\
         {pits}\n\n\
         ## Evidence\n\n\
         - task: `{task_id}`\n\n\
         ## Proposal\n\n\
         ```json\n{json}\n```\n",
        p.target_id, p.planner, p.target_id, p.rationale
    )
}

fn format_note_markdown(p: &RefinementProposal, task_id: &str) -> String {
    let json = serde_json::to_string_pretty(p).unwrap_or_else(|_| "{}".into());
    let body = p.note_body.as_deref().unwrap_or("- (empty)");
    format!(
        "---\n\
         kind: note\n\
         name: {}\n\
         op: {}\n\
         hits: {}\n\
         planner: {}\n\
         status: candidate\n\
         source_task: {task_id}\n\
         ---\n\n\
         # {}\n\n\
         {body}\n\n\
         ## Why\n\n\
         {}\n\n\
         ## Evidence\n\n\
         - task: `{task_id}`\n\n\
         ## Proposal\n\n\
         ```json\n{json}\n```\n",
        p.target_id,
        match p.op {
            RefineOp::Create => "create",
            RefineOp::Update => "update",
        },
        p.hits,
        p.planner,
        p.target_id,
        p.rationale
    )
}

pub fn parse_proposal_task_id(body: &str) -> Option<String> {
    parse_proposal(body)?.evidence_refs.into_iter().next()
}

pub(crate) fn parse_proposal(body: &str) -> Option<RefinementProposal> {
    let rest = body.split("```json").nth(1)?;
    let json = rest.split("```").next()?.trim();
    serde_json::from_str(json).ok()
}

pub fn parse_llm_refine_output(text: &str) -> Option<LlmRefineOut> {
    let json = extract_json_object(text)?;
    serde_json::from_str(&json).ok()
}

fn extract_json_object(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if let Some(rest) = trimmed.split("```json").nth(1) {
        let inner = rest.split("```").next()?.trim();
        if inner.starts_with('{') {
            return Some(inner.to_string());
        }
    }
    if let Some(rest) = trimmed.split("```").nth(1) {
        let inner = rest.split("```").next()?.trim();
        if inner.starts_with('{') {
            return Some(inner.to_string());
        }
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(trimmed[start..=end].to_string())
}

pub fn polish_prompt(digest: &str, draft_json: &str) -> String {
    format!(
        "You polish a Methodus refinement draft. Do not apply it. Do not use tools.\n\n\
         Rewrite only if the draft is a durable tactic. If it is noise, generic, or already \
         obvious, set skip=true.\n\n\
         Return JSON only with keys: skip (bool), rationale (string), note_body (string or null), \
         add_procedure (string array), add_pitfalls (string array).\n\n\
         ## Trajectory digest\n\n{digest}\n\n\
         ## Draft\n\n```json\n{draft_json}\n```\n"
    )
}

pub fn trajectory_digest(store: &Store, task_id: &str, title: &str) -> String {
    let procedure = collect_procedure_steps(store, task_id);
    let pitfalls = collect_pitfalls(store, task_id);
    let mut out = format!("Task: {title}\n\n## Procedure\n\n");
    if procedure.trim().is_empty() {
        out.push_str("(none)\n");
    } else {
        out.push_str(&procedure);
        out.push('\n');
    }
    out.push_str("\n## Pitfalls\n\n");
    if pitfalls.is_empty() {
        out.push_str("(none)\n");
    } else {
        for p in pitfalls {
            out.push_str(&format!("- {p}\n"));
        }
    }
    if out.chars().count() > 2500 {
        out.chars().take(2500).collect()
    } else {
        out
    }
}

/// Oldest unpolished note/patch candidate that has not been attempted today.
pub fn next_unpolished_candidate(
    store: &Store,
    home: &Path,
    skip_ids: &[String],
) -> Result<Option<KnowledgeItem>, CoreError> {
    let mut items = store.list_knowledge(Some(KnowledgeStatus::Candidate))?;
    items.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    for item in items {
        if item.source != HARNESS_NOTE_SOURCE && item.source != SKILL_PATCH_SOURCE {
            continue;
        }
        if skip_ids.iter().any(|id| id == &item.id) {
            continue;
        }
        let body = fs::read_to_string(home.join(&item.path)).unwrap_or_default();
        if is_unpolished(&body) {
            return Ok(Some(item));
        }
    }
    Ok(None)
}

fn is_unpolished(body: &str) -> bool {
    if let Some(p) = parse_proposal(body) {
        return p.planner != "llm";
    }
    frontmatter_str(body, "planner")
        .map(|s| s != "llm")
        .unwrap_or(true)
}

pub fn apply_llm_polish(
    store: &Store,
    home: &Path,
    item: &KnowledgeItem,
    out: &LlmRefineOut,
) -> Result<KnowledgeItem, CoreError> {
    let body = fs::read_to_string(home.join(&item.path)).unwrap_or_default();
    let mut proposal = parse_proposal(&body).ok_or_else(|| {
        CoreError::Other("candidate is missing a structured proposal".into())
    })?;
    if let Some(r) = out.rationale.as_ref().filter(|s| !s.trim().is_empty()) {
        proposal.rationale = r.clone();
    }
    if let Some(n) = out.note_body.as_ref().filter(|s| !s.trim().is_empty()) {
        proposal.note_body = Some(n.clone());
    }
    if let Some(p) = out.add_procedure.as_ref().filter(|v| !v.is_empty()) {
        proposal.add_procedure = p.clone();
    }
    if let Some(p) = out.add_pitfalls.as_ref().filter(|v| !v.is_empty()) {
        proposal.add_pitfalls = p.clone();
    }
    proposal.planner = "llm".into();
    let task_id = proposal
        .evidence_refs
        .first()
        .cloned()
        .or_else(|| frontmatter_str(&body, "source_task"))
        .unwrap_or_default();
    let new_body = match proposal.target_kind {
        RefineTargetKind::Note => format_note_markdown(&proposal, &task_id),
        RefineTargetKind::Skill => format_patch_markdown(&proposal, &task_id),
    };
    rewrite_candidate_body(store, home, item, &new_body)
}

fn rewrite_candidate_body(
    store: &Store,
    home: &Path,
    item: &KnowledgeItem,
    body: &str,
) -> Result<KnowledgeItem, CoreError> {
    let abs = home.join(&item.path);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&abs, body)?;
    let mut next = item.clone();
    next.content_hash = crate::learning::sha256_hex(body.as_bytes());
    next.updated_at = chrono::Utc::now();
    store.update_knowledge(&next)?;
    Ok(next)
}

fn merge_skill_markdown(live: &str, procedure: &[String], pitfalls: &[String]) -> String {
    let mut out = live.trim_end().to_string();
    if !procedure.is_empty() {
        if !out.contains("## Procedure") {
            out.push_str("\n\n## Procedure\n");
        }
        out.push('\n');
        for s in procedure {
            out.push_str(&format!("- {s}\n"));
        }
    }
    if !pitfalls.is_empty() {
        if !out.contains("## Pitfalls") {
            out.push_str("\n\n## Pitfalls\n");
        }
        out.push('\n');
        for s in pitfalls {
            out.push_str(&format!("- {s}\n"));
        }
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn merge_note_markdown(existing: &str, proposal: Option<&RefinementProposal>) -> String {
    let Some(p) = proposal else {
        return existing.to_string();
    };
    let mut out = existing.trim_end().to_string();
    if let Some(add) = p.note_body.as_deref() {
        if !out.contains(add.trim_start_matches("- ").trim()) {
            out.push('\n');
            out.push_str(add);
            out.push('\n');
        }
    }
    out
}

fn committed_note_body(
    slug: &str,
    proposal: Option<&RefinementProposal>,
    candidate: &str,
    hits: i64,
) -> String {
    if let Some(p) = proposal {
        let body = p.note_body.as_deref().unwrap_or("");
        return format!(
            "---\n\
             kind: note\n\
             name: {slug}\n\
             hits: {hits}\n\
             status: committed\n\
             ---\n\n\
             # {slug}\n\n\
             {body}\n\n\
             ## Why\n\n\
             {}\n",
            p.rationale
        );
    }
    candidate.replace("status: candidate", "status: committed")
}

fn numbered_steps(block: &str) -> Vec<String> {
    block
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let t = l.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ');
            t.trim().to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

fn new_lines(candidates: &[String], haystack: &str) -> Vec<String> {
    candidates
        .iter()
        .filter(|c| {
            let key = c.chars().take(40).collect::<String>().to_lowercase();
            !haystack.to_lowercase().contains(&key) && c.chars().count() >= 8
        })
        .cloned()
        .take(6)
        .collect()
}

fn skill_name_score(base: &str, skill_name: &str) -> usize {
    let stripped = strip_task_suffix(skill_name);
    if stripped == *base {
        return 100;
    }
    if stripped.starts_with(base) || base.starts_with(&stripped) {
        return 80;
    }
    let a: Vec<&str> = base.split('-').filter(|w| w.len() > 2).collect();
    let b: Vec<&str> = stripped.split('-').filter(|w| w.len() > 2).collect();
    let n = a.iter().filter(|t| b.contains(t)).count();
    if n >= 2 || (n == 1 && a.len() == 1) {
        n * 10
    } else {
        0
    }
}

fn strip_task_suffix(name: &str) -> String {
    if let Some((base, suf)) = name.rsplit_once('-') {
        if (6..=10).contains(&suf.len()) && suf.chars().all(|c| c.is_ascii_alphanumeric()) {
            return base.to_string();
        }
    }
    name.to_string()
}

fn slug_from_path(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| is_safe_segment(s))
        .unwrap_or_else(|| "note".to_string())
}

fn face_from_task(task: &methodus_domain::Task) -> Option<String> {
    crate::resolution::Resolution::parse_json(task.resolution.as_deref().unwrap_or(""))
        .map(|r| r.face_id)
}

fn note_score(request: &str, path: &str, body: &str) -> usize {
    let req = crate::resolution::tokenize(request);
    if req.is_empty() {
        return 1;
    }
    let hay = crate::resolution::tokenize(&format!("{path} {body}"));
    req.iter().filter(|t| hay.contains(*t)).count().max(1)
}

fn note_title(path: &str, body: &str) -> String {
    for line in body.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("# ") {
            if !rest.is_empty() {
                return rest.chars().take(80).collect();
            }
        }
    }
    slug_from_path(path)
}

fn excerpt_md(body: &str, max_lines: usize) -> String {
    body.lines().take(max_lines).collect::<Vec<_>>().join("\n")
}

fn frontmatter_str(body: &str, key: &str) -> Option<String> {
    let rest = body.strip_prefix("---\n")?;
    let fm = rest.split_once("\n---")?.0;
    for line in fm.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        if k.trim() == key {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn frontmatter_i64(body: &str, key: &str) -> Option<i64> {
    let rest = body.strip_prefix("---\n")?;
    let fm = rest.split_once("\n---")?.0;
    for line in fm.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        if k.trim() == key {
            return v.trim().parse().ok();
        }
    }
    None
}

fn replace_frontmatter_i64(body: &str, key: &str, value: i64) -> String {
    let Some(rest) = body.strip_prefix("---\n") else {
        return format!("---\n{key}: {value}\n---\n{body}");
    };
    let Some((fm, after)) = rest.split_once("\n---") else {
        return body.to_string();
    };
    let mut found = false;
    let mut lines = Vec::new();
    for line in fm.lines() {
        if let Some((k, _)) = line.split_once(':') {
            if k.trim() == key {
                lines.push(format!("{key}: {value}"));
                found = true;
                continue;
            }
        }
        lines.push(line.to_string());
    }
    if !found {
        lines.push(format!("{key}: {value}"));
    }
    format!("---\n{}\n---{after}", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use methodus_domain::{Task, TaskStatus};

    fn task(id: &str, title: &str) -> Task {
        let now = Utc::now();
        Task {
            id: id.into(),
            title: title.into(),
            request: title.into(),
            project_id: None,
            status: TaskStatus::Reviewing,
            runtime: None,
            workspace_id: None,
            resolution: None,
            version: 1,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn related_skill_matches_task_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("skills/sample-cpu-abc123de");
        fs::create_dir_all(&live).unwrap();
        fs::write(live.join("SKILL.md"), "# sample-cpu\n").unwrap();
        let found = find_related_skill(dir.path(), "Sample CPU of a process").unwrap();
        assert_eq!(found.0, "sample-cpu-abc123de");
    }

    #[test]
    fn skill_patch_merges_procedure_and_pitfalls() {
        let live = "# skill\n\n## Procedure\n\n1. Use `Read`\n\n## Pitfalls\n\n- (none recorded yet)\n";
        let merged = merge_skill_markdown(
            live,
            &["`Grep` — pattern foo".into()],
            &["needed approval for `Bash`".into()],
        );
        assert!(merged.contains("`Grep`"));
        assert!(merged.contains("needed approval"));
    }

    #[test]
    fn apply_note_writes_face_notes_dir() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        let t = task("t1", "debug latch");
        store.insert_task(&t).unwrap();
        let refs = JobRefs {
            experience_id: None,
            task_id: Some(t.id.clone()),
            face_id: Some("general".into()),
            question_id: None,
            source: Some("test".into()),
        };
        let item = upsert_harness_note(
            &store,
            dir.path(),
            &refs,
            &t,
            "general",
            &["Read gpio map".into()],
            &["probe timed out".into()],
        )
        .unwrap()
        .expect("note candidate");
        assert_eq!(item.source, HARNESS_NOTE_SOURCE);
        let (live, hits) = apply_harness_note(&store, dir.path(), &item).unwrap();
        assert!(live.starts_with("faces/general/notes/"));
        assert_eq!(hits, 0);
        let live_body = fs::read_to_string(dir.path().join(&live)).unwrap();
        assert!(live_body.contains("hits: 0"));
        let mut live_item = item.clone();
        live_item.path = live.clone();
        assert_eq!(bump_note_inject_hit(dir.path(), &live_item).unwrap(), 1);
        assert!(
            fs::read_to_string(dir.path().join(&live))
                .unwrap()
                .contains("hits: 1")
        );
    }

    #[test]
    fn apply_skill_patch_appends_to_live() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        let live_dir = dir.path().join("skills/tcp-debug");
        fs::create_dir_all(&live_dir).unwrap();
        fs::write(
            live_dir.join("SKILL.md"),
            "# tcp-debug\n\n## Procedure\n\n1. Read logs\n",
        )
        .unwrap();
        let t = task("t2", "tcp debug sockets");
        store.insert_task(&t).unwrap();
        let refs = JobRefs {
            experience_id: None,
            task_id: Some(t.id.clone()),
            face_id: Some("general".into()),
            question_id: None,
            source: Some("test".into()),
        };
        let item = write_skill_patch(
            &store,
            dir.path(),
            &refs,
            &t,
            "general",
            "tcp-debug",
            vec!["ss -tnp before blaming the app".into()],
            vec!["needed approval for Bash".into()],
        )
        .unwrap()
        .expect("patch");
        assert_eq!(item.source, SKILL_PATCH_SOURCE);
        let live = apply_skill_patch(dir.path(), &item).unwrap();
        let body = fs::read_to_string(dir.path().join(&live)).unwrap();
        assert!(body.contains("ss -tnp"));
        assert!(body.contains("needed approval"));
    }

    #[test]
    fn parse_roundtrip_proposal() {
        let p = RefinementProposal {
            target_kind: RefineTargetKind::Skill,
            target_id: "tcp-debug".into(),
            op: RefineOp::Update,
            add_procedure: vec!["check sockets".into()],
            add_pitfalls: vec!["needed approval for Bash".into()],
            note_body: None,
            evidence_refs: vec!["task_1".into()],
            rationale: "repeat".into(),
            hits: 1,
            planner: default_planner(),
        };
        let md = format_patch_markdown(&p, "task_1");
        let back = parse_proposal(&md).unwrap();
        assert_eq!(back.target_id, "tcp-debug");
        assert_eq!(back.add_procedure.len(), 1);
        assert_eq!(back.planner, "rules");
    }

    #[test]
    fn choose_refinement_is_exclusive() {
        assert_eq!(
            choose_refinement(&[], &[], 3, false, &[], &[], false),
            DistillKind::None
        );
        assert_eq!(
            choose_refinement(
                &["Use `Read`".into(), "Use `Bash`".into(), "Use `Grep`".into()],
                &[],
                3,
                false,
                &[],
                &[],
                false
            ),
            DistillKind::None
        );
        assert_eq!(
            choose_refinement(
                &["`Bash` — ps aux | grep nginx".into()],
                &[],
                3,
                false,
                &[],
                &[],
                false
            ),
            DistillKind::Skill
        );
        assert_eq!(
            choose_refinement(
                &["`Bash` — ss -tnp".into()],
                &["needed approval for Bash".into()],
                2,
                true,
                &["`Bash` — ss -tnp".into()],
                &["needed approval for Bash".into()],
                false
            ),
            DistillKind::Patch
        );
        assert_eq!(
            choose_refinement(
                &[],
                &["probe timed out on gpio".into()],
                0,
                false,
                &[],
                &[],
                false
            ),
            DistillKind::Note
        );
        assert_eq!(
            choose_refinement(
                &["`Read` — /proc/1/stat".into()],
                &[],
                1,
                false,
                &[],
                &[],
                true
            ),
            DistillKind::None
        );
    }

    #[test]
    fn substantial_step_rejects_bare_use() {
        assert!(!is_substantial_step("Use `Read`"));
        assert!(is_substantial_step("`Read` — /tmp/latch.md"));
        assert!(is_substantial_step("ss -tnp before blaming the app"));
    }

    #[test]
    fn parse_llm_refine_output_accepts_fenced_json() {
        let text = "Here you go\n```json\n{\"skip\": true, \"rationale\": \"noise\"}\n```\n";
        let out = parse_llm_refine_output(text).unwrap();
        assert!(out.skip);
        assert_eq!(out.rationale.as_deref(), Some("noise"));
    }
}
