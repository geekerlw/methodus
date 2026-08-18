//! Curiosity loop — mentor questions from synthesized knowledge, not task keyword scraps.

use std::path::{Path, PathBuf};

use methodus_domain::{Experience, JobKind};
use methodus_store::Store;

use crate::error::CoreError;
use crate::learning::{enqueue_job, upsert_mentor_question, write_candidate_from_study, JobRefs};

pub const MODULE_EXPERT_METHOD_ID: &str = "module-expert-learning";

/// Whether this experience came from a module-expert study task.
pub fn is_module_expert_experience(body: &str, method_id: Option<&str>) -> bool {
    if method_id == Some(MODULE_EXPERT_METHOD_ID) {
        return true;
    }
    body.lines().any(|l| {
        let t = l.trim();
        t == "- mode: module_expert" || t == "mode: module_expert"
    })
}

/// Extract a markdown section by heading title (case-insensitive).
pub fn extract_section(body: &str, headings: &[&str]) -> String {
    let want: Vec<String> = headings.iter().map(|h| h.to_lowercase()).collect();
    let mut out = Vec::new();
    let mut in_section = false;
    let mut section_level = 0usize;

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            let title = trimmed.trim_start_matches('#').trim().to_lowercase();
            if want.iter().any(|w| title == *w || title.starts_with(&format!("{w} ("))) {
                in_section = true;
                section_level = level;
                continue;
            }
            if in_section && level <= section_level {
                break;
            }
        }
        if in_section {
            out.push(line.to_string());
        }
    }
    out.join("\n").trim().to_string()
}

/// Bullet items under a section (lines starting with `-` or `*`).
pub fn section_bullets(section: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in section.lines() {
        let t = line.trim();
        if t.starts_with("- ") || t.starts_with("* ") {
            let item = t.trim_start_matches(['-', '*', ' ']).trim();
            if !item.is_empty() {
                out.push(item.to_string());
            }
        }
    }
    out
}

/// `### Subtopic` blocks under Knowledge.
pub fn knowledge_subsections(section: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut current_title: Option<String> = None;
    let mut current_body: Vec<String> = Vec::new();

    for line in section.lines() {
        let t = line.trim();
        if t.starts_with("### ") {
            if let Some(title) = current_title.take() {
                let body = current_body.join("\n").trim().to_string();
                if !body.is_empty() {
                    out.push((title, body));
                }
            }
            current_body.clear();
            current_title = Some(t.trim_start_matches("### ").trim().to_string());
            continue;
        }
        if current_title.is_some() {
            current_body.push(line.to_string());
        }
    }
    if let Some(title) = current_title {
        let body = current_body.join("\n").trim().to_string();
        if !body.is_empty() {
            out.push((title, body));
        }
    }
    if out.is_empty() && !section.trim().is_empty() {
        out.push(("Summary".to_string(), section.trim().to_string()));
    }
    out
}

const UNCERTAINTY_MARKERS: &[&str] = &[
    "tbd",
    "unclear",
    "unknown",
    "unverified",
    "not sure",
    "needs confirmation",
    "待确认",
    "不清楚",
    "???",
    "? —",
];

/// Lines in knowledge text that signal mentor review is needed.
pub fn uncertainty_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let lower = line.to_lowercase();
        if UNCERTAINTY_MARKERS.iter().any(|m| lower.contains(m)) {
            let t = line.trim().trim_start_matches(['-', '*', ' ']).trim();
            if !t.is_empty() && !out.iter().any(|x| x == t) {
                out.push(t.to_string());
            }
        }
    }
    out
}

/// Parse mentor-facing questions from a study Result body.
pub fn parse_mentor_questions(result: &str) -> Vec<(String, String)> {
    let section = extract_section(
        result,
        &[
            "Open Questions (for Mentor)",
            "Open Questions",
            "Questions for Mentor",
        ],
    );
    section_bullets(&section)
        .into_iter()
        .map(|q| (q.clone(), "executor flagged during module study".to_string()))
        .collect()
}

pub fn run_synthesize_knowledge(
    store: &Store,
    home: &Path,
    refs: &JobRefs,
) -> Result<(), CoreError> {
    let exp_id = refs
        .experience_id
        .as_deref()
        .ok_or_else(|| CoreError::Other("synthesize_knowledge missing experience_id".into()))?;
    let exp = store
        .get_experience(exp_id)?
        .ok_or_else(|| CoreError::Other(format!("experience not found: {exp_id}")))?;
    let body = std::fs::read_to_string(home.join(&exp.path)).unwrap_or_default();
    let result = crate::learning::full_result_section(&body);
    if result.trim().is_empty() {
        return Ok(());
    }
    let knowledge_section = extract_section(&result, &["Knowledge", "Synthesized Knowledge"]);
    let subsections = knowledge_subsections(&knowledge_section);
    let face = exp.face_id.as_deref().unwrap_or("general");
    for (title, content) in subsections {
        write_candidate_from_study(store, home, face, &title, &content, &exp, refs)?;
    }
    Ok(())
}

pub fn run_analyze_knowledge_gaps(
    store: &Store,
    home: &Path,
    refs: &JobRefs,
) -> Result<(), CoreError> {
    let exp_id = refs
        .experience_id
        .as_deref()
        .ok_or_else(|| CoreError::Other("analyze_knowledge_gaps missing experience_id".into()))?;
    let exp = store
        .get_experience(exp_id)?
        .ok_or_else(|| CoreError::Other(format!("experience not found: {exp_id}")))?;
    let body = std::fs::read_to_string(home.join(&exp.path)).unwrap_or_default();
    let result = crate::learning::full_result_section(&body);
    let face = exp.face_id.clone().or_else(|| refs.face_id.clone());

    for (question, reason) in parse_mentor_questions(&result) {
        upsert_mentor_question(store, &question, &reason, &exp, face.as_deref())?;
    }

    let knowledge_section = extract_section(&result, &["Knowledge", "Synthesized Knowledge"]);
    for (topic, content) in knowledge_subsections(&knowledge_section) {
        for line in uncertainty_lines(&content) {
            let q = if line.ends_with('?') {
                line.clone()
            } else {
                format!("Can you confirm ({topic}): {line}?")
            };
            upsert_mentor_question(
                store,
                &q,
                &format!("uncertainty in synthesized knowledge — {topic}"),
                &exp,
                face.as_deref(),
            )?;
            let _ = crate::hypothesis::upsert_hypothesis(
                store,
                home,
                face.as_deref().unwrap_or("general"),
                &topic,
                &line,
                "uncertainty flagged during module study",
                0.45,
            );
            let refs = crate::learning::JobRefs {
                experience_id: Some(exp.id.clone()),
                task_id: Some(exp.task_id.clone()),
                face_id: face.clone(),
                question_id: None,
                source: Some(format!("auto_research:{topic}")),
            };
            let _ = crate::learning::enqueue_job(
                store,
                methodus_domain::JobKind::AutoResearch,
                &format!("research:{}:{}", exp.id, crate::learning::slugify(&topic)),
                &refs,
                5,
            );
        }
    }
    Ok(())
}

pub fn run_auto_research(
    store: &Store,
    home: &Path,
    refs: &crate::learning::JobRefs,
) -> Result<(), CoreError> {
    let exp_id = refs
        .experience_id
        .as_deref()
        .ok_or_else(|| CoreError::Other("auto_research missing experience_id".into()))?;
    let exp = store
        .get_experience(exp_id)?
        .ok_or_else(|| CoreError::Other(format!("experience not found: {exp_id}")))?;
    let face = exp.face_id.as_deref().unwrap_or("general");
    let topic = refs
        .source
        .as_deref()
        .and_then(|s| s.strip_prefix("auto_research:"))
        .unwrap_or("open question");
    let body = format!(
        "# Research note: {topic}\n\n\
         **Status:** candidate — needs human review\n\n\
         **Source:** curiosity auto-research from experience `{exp_id}`\n\n\
         Suggested next steps:\n\
         - consult committed Face knowledge\n\
         - run `/study` on relevant docs if URLs/paths are known\n\
         - answer mentor questions in `/inbox`\n"
    );
    let _ = crate::learning::write_candidate(
        store,
        home,
        face,
        &format!("research: {topic}"),
        &body,
        "auto_research",
        None,
        Some(exp_id),
        0.5,
    );
    let _ = store.insert_event(
        &format!("ev_{}", uuid::Uuid::new_v4().to_string().replace('-', "")),
        "learning.candidate_created",
        &chrono::Utc::now().to_rfc3339(),
        Some(&exp.task_id),
        None,
        &serde_json::json!({"kind": "auto_research", "topic": topic}).to_string(),
        None,
    );
    Ok(())
}

pub fn enqueue_module_study_jobs(
    store: &Store,
    exp: &Experience,
    refs: &JobRefs,
) -> Result<(), CoreError> {
    enqueue_job(
        store,
        JobKind::SynthesizeKnowledge,
        &format!("synthesize:{}", exp.id),
        refs,
        9,
    )?;
    enqueue_job(
        store,
        JobKind::AnalyzeKnowledgeGaps,
        &format!("curiosity:{}", exp.id),
        refs,
        8,
    )?;
    enqueue_job(
        store,
        JobKind::ProposeSkill,
        &format!("skill:study:{}", exp.id),
        refs,
        7,
    )?;
    Ok(())
}

/// Split `/study` input into topic label and explicit sources (paths / URLs).
pub fn parse_study_invocation(input: &str) -> (String, Vec<String>) {
    let mut sources = Vec::new();
    let mut scope_parts = Vec::new();
    for word in input.split_whitespace() {
        let w = word.trim();
        if w.is_empty() {
            continue;
        }
        if w.starts_with("http://") || w.starts_with("https://") {
            sources.push(w.to_string());
        } else if let Some(rest) = w.strip_prefix('@') {
            if !rest.is_empty() {
                sources.push(rest.to_string());
            }
        } else if w.starts_with('/')
            || w.starts_with('~')
            || w.starts_with("./")
            || w.starts_with("../")
        {
            sources.push(w.to_string());
        } else {
            scope_parts.push(w);
        }
    }
    let scope = scope_parts.join(" ");
    (scope, sources)
}

/// Resolve study paths for Read/Glob (URLs stay in prompt only).
pub fn study_named_roots(
    home: &Path,
    sources: &[String],
) -> Result<Vec<(String, PathBuf)>, CoreError> {
    let mut out = Vec::new();
    for src in sources {
        if src.starts_with("http://") || src.starts_with("https://") {
            continue;
        }
        let expanded = expand_study_path(home, src);
        if !expanded.exists() {
            return Err(CoreError::Other(format!("study source not found: {src}")));
        }
        let canon = expanded.canonicalize().unwrap_or(expanded);
        let id = canon
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("src{}", out.len()));
        if !out.iter().any(|(_, p)| p == &canon) {
            out.push((id, canon));
        }
    }
    Ok(out)
}

fn expand_study_path(home: &Path, raw: &str) -> PathBuf {
    if raw.starts_with('~') {
        if let Ok(user_home) = std::env::var("HOME") {
            return PathBuf::from(raw.replacen('~', &user_home, 1));
        }
    }
    let p = PathBuf::from(raw);
    if p.is_absolute() {
        p
    } else {
        home.join(p)
    }
}

pub fn render_study_sources(sources: &[String]) -> String {
    if sources.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n## Study sources (read in place — not the task workspace)\n\n\
         Long-lived outputs go to Methodus home (knowledge / skills / experiences). \
         Future execution tasks load committed material into their workspace.\n\n",
    );
    for s in sources {
        out.push_str(&format!("- {s}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"## Sources
- `src/foo.c` — entry point

## Knowledge

### Boot flow

The device boots via `main()` then calls init. TBD: clock source on rev B.

### API

`foo_init()` must be called first.

## Open Questions (for Mentor)

- Is rev B clock source external crystal or internal RC?
- Docs mention 5V but schematic shows 3.3V — which applies?

## Notes
- did not read tests/
"#;

    #[test]
    fn extracts_mentor_questions() {
        let qs = parse_mentor_questions(SAMPLE);
        assert_eq!(qs.len(), 2);
        assert!(qs[0].0.contains("rev B"));
    }

    #[test]
    fn knowledge_subsections_and_uncertainty() {
        let k = extract_section(SAMPLE, &["Knowledge"]);
        let subs = knowledge_subsections(&k);
        assert_eq!(subs.len(), 2);
        let flags = uncertainty_lines(&subs[0].1);
        assert!(flags.iter().any(|l| l.contains("TBD")));
    }

    #[test]
    fn detects_module_expert_marker() {
        let body = "# Experience\n\n- mode: module_expert\n";
        assert!(is_module_expert_experience(body, None));
        assert!(!is_module_expert_experience("plain", None));
    }

    #[test]
    fn parse_study_invocation_splits_scope_and_sources() {
        let (scope, src) = parse_study_invocation(
            "nxm upgrade @~/docs/nxm https://example.com/wiki /src/nxm/main.c",
        );
        assert_eq!(scope, "nxm upgrade");
        assert_eq!(src.len(), 3);
        assert!(src.contains(&"~/docs/nxm".to_string()));
        assert!(src.iter().any(|s| s.starts_with("https://")));
    }
}
