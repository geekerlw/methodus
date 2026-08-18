//! Unified `/learn` — user supplies sources; Methodus picks the pipeline and archive target.

use std::path::{Path, PathBuf};

use crate::curiosity::parse_study_invocation;
use crate::error::CoreError;
use crate::ingest::{DOC_INGEST_METHOD_ID, REPO_SURVEY_METHOD_ID};
use crate::curiosity::MODULE_EXPERT_METHOD_ID;
use crate::project;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnMode {
    /// Focus project repo layout → `projects/{id}/knowledge/`
    RepoSurvey,
    /// External docs/standards → project knowledge corpus
    DocIngest,
    /// Paths/URLs → Face knowledge + skill draft + mentor questions
    ModuleExpert,
}

impl LearnMode {
    pub fn method_id(self) -> &'static str {
        match self {
            Self::RepoSurvey => REPO_SURVEY_METHOD_ID,
            Self::DocIngest => DOC_INGEST_METHOD_ID,
            Self::ModuleExpert => MODULE_EXPERT_METHOD_ID,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::RepoSurvey => "repo survey",
            Self::DocIngest => "document ingest",
            Self::ModuleExpert => "module expert",
        }
    }
}

/// Parse `/learn` input: optional topic hint + `@` paths and URLs.
pub fn parse_learn_invocation(input: &str) -> (String, Vec<String>) {
    parse_study_invocation(input)
}

/// Choose learning pipeline from sources and an optional topic hint.
pub fn plan_learn(home: &Path, sources: &[String], hint: &str) -> Result<(LearnMode, String), CoreError> {
    let hint = hint.trim();
    if sources.is_empty() {
        if project::focus_project(home).is_some() {
            return Ok((LearnMode::RepoSurvey, String::new()));
        }
        return Err(CoreError::Other(
            "/learn needs sources — e.g. /learn @~/docs/spec.pdf @~/src\n\
             or set a focus project in /setup and run /learn with no args to survey the repo"
                .into(),
        ));
    }

    if sources.iter().any(|s| s.starts_with("http://") || s.starts_with("https://")) {
        return Ok((LearnMode::ModuleExpert, scope_from_hint_or_sources(hint, sources)));
    }

    if sources.len() == 1 {
        if let Some(focus) = project::focus_project(home) {
            if path_matches_project_root(home, &sources[0], &focus.root)? {
                if looks_like_repo_root(&focus.root) {
                    return Ok((LearnMode::RepoSurvey, String::new()));
                }
            }
        }
        let expanded = expand_source(home, &sources[0]);
        if is_document_path(&expanded) {
            project::focus_project(home).ok_or_else(|| {
                CoreError::Other("set a focus project in /setup before ingesting documents".into())
            })?;
            return Ok((LearnMode::DocIngest, String::new()));
        }
        if expanded.is_dir() && looks_like_repo_root(&expanded) {
            if project::focus_project(home).is_some() {
                return Ok((LearnMode::RepoSurvey, String::new()));
            }
        }
    }

    if sources.iter().all(|s| {
        s.starts_with("http://") || s.starts_with("https://") || {
            let p = expand_source(home, s);
            is_document_path(&p)
        }
    }) {
        project::focus_project(home).ok_or_else(|| {
            CoreError::Other("set a focus project in /setup before ingesting documents".into())
        })?;
        return Ok((LearnMode::DocIngest, String::new()));
    }

    Ok((LearnMode::ModuleExpert, scope_from_hint_or_sources(hint, sources)))
}

fn scope_from_hint_or_sources(hint: &str, sources: &[String]) -> String {
    if !hint.is_empty() {
        return hint.to_string();
    }
    for src in sources {
        if src.starts_with("http://") || src.starts_with("https://") {
            if let Some(host) = src.split('/').nth(2) {
                return host.to_string();
            }
        }
        let p = Path::new(src);
        if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    "learn".to_string()
}

fn expand_source(home: &Path, raw: &str) -> PathBuf {
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

fn path_matches_project_root(home: &Path, raw: &str, project_root: &Path) -> Result<bool, CoreError> {
    let expanded = expand_source(home, raw);
    let canon = expanded.canonicalize().unwrap_or(expanded);
    let root = project_root.canonicalize().unwrap_or_else(|_| project_root.to_path_buf());
    Ok(canon == root)
}
fn is_document_path(path: &Path) -> bool {
    is_document_file(path)
}

fn is_document_file(path: &Path) -> bool {
    const DOC_EXT: &[&str] = &[
        "pdf", "md", "markdown", "txt", "doc", "docx", "xlsx", "xls", "ppt", "pptx", "html",
        "htm", "yaml", "yml", "rst", "csv", "rtf", "odt",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| DOC_EXT.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn looks_like_repo_root(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    for marker in [
        ".git",
        "Cargo.toml",
        "package.json",
        "go.mod",
        "CMakeLists.txt",
        "Makefile",
        "pom.xml",
        "build.gradle",
    ] {
        if path.join(marker).exists() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn touch(home: &Path, rel: &str) {
        let p = home.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, "x").unwrap();
    }

    #[test]
    fn empty_sources_with_focus_surveys() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        touch(home, "projects.yaml");
        std::fs::write(
            home.join("projects.yaml"),
            "focus: p1\nprojects:\n  - id: p1\n    path: /tmp/repo\n",
        )
        .unwrap();
        let (mode, _) = plan_learn(home, &[], "").unwrap();
        assert_eq!(mode, LearnMode::RepoSurvey);
    }

    #[test]
    fn pdf_sources_ingest() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        std::fs::write(
            home.join("projects.yaml"),
            "focus: p1\nprojects:\n  - id: p1\n    path: /tmp/repo\n",
        )
        .unwrap();
        let (mode, _) = plan_learn(home, &["/docs/spec.pdf".to_string()], "").unwrap();
        assert_eq!(mode, LearnMode::DocIngest);
    }

    #[test]
    fn url_sources_module_expert() {
        let tmp = TempDir::new().unwrap();
        let (mode, scope) =
            plan_learn(tmp.path(), &["https://example.com/docs".to_string()], "").unwrap();
        assert_eq!(mode, LearnMode::ModuleExpert);
        assert_eq!(scope, "example.com");
    }

    #[test]
    fn hint_preserved_for_module_expert() {
        let tmp = TempDir::new().unwrap();
        let (mode, scope) = plan_learn(
            tmp.path(),
            &["https://a.com/x".to_string()],
            "nxm platform",
        )
        .unwrap();
        assert_eq!(mode, LearnMode::ModuleExpert);
        assert_eq!(scope, "nxm platform");
    }
}
