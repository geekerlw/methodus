//! `@path` mentions: pick files/folders as task context, like Claude Code.

use std::fs;
use std::path::{Path, PathBuf};

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    "__pycache__",
    ".git",
    ".svn",
    ".hg",
    ".next",
    ".cache",
    ".venv",
    "venv",
    "coverage",
];

const MAX_CANDIDATES: usize = 1500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionCandidate {
    /// Path relative to its root; directories end with `/`.
    pub rel: String,
    /// What the picker shows and what `@` inserts (may be `root/rel`).
    pub label: String,
    pub is_dir: bool,
    pub abs: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    pub raw: String,
    pub abs: PathBuf,
    pub is_dir: bool,
}

/// Active `@query` at the end of the input (no whitespace after `@`).
pub fn at_query(input: &str) -> Option<&str> {
    let mut last = None;
    for (i, ch) in input.char_indices() {
        if ch != '@' {
            continue;
        }
        let prev_ok = i == 0 || input[..i].chars().last().is_some_and(|p| p.is_whitespace());
        if prev_ok {
            last = Some(i);
        }
    }
    let start = last?;
    let rest = input.get(start + 1..)?;
    if rest.chars().any(char::is_whitespace) {
        return None;
    }
    Some(rest)
}

pub fn list_candidates(root: &Path, limit: usize) -> Vec<MentionCandidate> {
    let cap = limit.min(MAX_CANDIDATES).max(1);
    let mut out = Vec::new();
    if !root.is_dir() {
        return out;
    }
    out.push(MentionCandidate {
        rel: "./".to_string(),
        label: "./".to_string(),
        is_dir: true,
        abs: root.to_path_buf(),
    });
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= cap {
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut ents: Vec<_> = entries.flatten().collect();
        ents.sort_by_key(|e| e.file_name());
        for entry in ents.into_iter().rev() {
            if out.len() >= cap {
                break;
            }
            let path = entry.path();
            let name = entry.file_name();
            let name_s = name.to_string_lossy();
            if name_s.starts_with('.') || SKIP_DIRS.iter().any(|s| *s == name_s) {
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let rel_s = rel.to_string_lossy().replace('\\', "/");
            if rel_s.is_empty() {
                continue;
            }
            let is_dir = path.is_dir();
            if is_dir {
                stack.push(path.clone());
                out.push(MentionCandidate {
                    rel: format!("{rel_s}/"),
                    label: format!("{rel_s}/"),
                    is_dir: true,
                    abs: path,
                });
            } else {
                out.push(MentionCandidate {
                    rel: rel_s.clone(),
                    label: rel_s,
                    is_dir: false,
                    abs: path,
                });
            }
        }
    }
    out
}

pub fn filter_candidates<'a>(
    all: &'a [MentionCandidate],
    query: &str,
) -> Vec<&'a MentionCandidate> {
    let q = query.trim().trim_start_matches("./").to_ascii_lowercase();
    let mut scored: Vec<(i32, &'a MentionCandidate)> = all
        .iter()
        .filter_map(|c| score_candidate(&q, c).map(|s| (s, c)))
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.rel.len().cmp(&b.1.rel.len()))
    });
    scored.into_iter().take(40).map(|(_, c)| c).collect()
}

fn score_candidate(query: &str, cand: &MentionCandidate) -> Option<i32> {
    if query.is_empty() {
        return Some(1);
    }
    let rel = cand.rel.to_ascii_lowercase();
    let label = cand.label.to_ascii_lowercase();
    let file = rel.trim_end_matches('/');
    if query_hits(query, file) || query_hits(query, label.trim_end_matches('/')) {
        return Some(2000);
    }
    if file.starts_with(query) || rel.starts_with(query) || label.starts_with(query) {
        return Some(1000 - rel.len() as i32);
    }
    if let Some(name) = file.rsplit('/').next() {
        if name.starts_with(query) {
            return Some(800 - rel.len() as i32);
        }
        if name.contains(query) {
            return Some(400 - rel.len() as i32);
        }
    }
    if file.contains(query) || label.contains(query) {
        return Some(200 - rel.len() as i32);
    }
    None
}

fn query_hits(query: &str, value: &str) -> bool {
    value == query || value == format!("{query}/")
}

/// Launch cwd + registered projects. Source stays on disk; nothing is copied.
pub fn context_roots(home: &Path, launch_cwd: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    fn push(out: &mut Vec<(String, PathBuf)>, seen: &mut Vec<PathBuf>, id: String, path: PathBuf) {
        let Ok(canon) = path.canonicalize() else {
            return;
        };
        if seen.iter().any(|s| s == &canon) {
            return;
        }
        seen.push(canon);
        out.push((id, path));
    }

    let home_canon = home.canonicalize().ok();
    let ws_canon = home.join("workspaces").canonicalize().ok();
    let user_home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .and_then(|p| p.canonicalize().ok());

    for proj in crate::project::list_projects(home) {
        push(&mut out, &mut seen, proj.id, proj.root);
    }
    if launch_cwd.is_dir() {
        if let Ok(canon) = launch_cwd.canonicalize() {
            let skip = canon == Path::new("/")
                || home_canon.as_ref().is_some_and(|h| &canon == h)
                || ws_canon
                    .as_ref()
                    .is_some_and(|ws| &canon == ws || canon.starts_with(ws))
                || user_home.as_ref().is_some_and(|h| &canon == h);
            if !skip {
                let name = launch_cwd
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "cwd".to_string());
                push(&mut out, &mut seen, name, launch_cwd.to_path_buf());
            }
        }
    }
    out
}

pub fn list_from_roots(roots: &[(String, PathBuf)], limit: usize) -> Vec<MentionCandidate> {
    if roots.is_empty() {
        return Vec::new();
    }
    let prefix = roots.len() > 1;
    let per = (limit / roots.len()).max(80);
    let mut out = Vec::new();
    for (id, root) in roots {
        if out.len() >= limit {
            break;
        }
        let mut chunk = list_candidates(root, per.min(limit - out.len()));
        if prefix {
            for c in &mut chunk {
                let rest = c.rel.trim_start_matches("./");
                c.label = if rest.is_empty() {
                    format!("{id}/")
                } else {
                    format!("{id}/{rest}")
                };
            }
        }
        out.extend(chunk);
    }
    out
}

pub fn readable_dirs(roots: &[(String, PathBuf)]) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for (_, root) in roots {
        let Ok(canon) = root.canonicalize() else {
            continue;
        };
        if !dirs.contains(&canon) {
            dirs.push(canon);
        }
    }
    dirs
}

pub fn render_readable_dirs(roots: &[(String, PathBuf)]) -> String {
    if roots.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n## Readable directories\n\n\
         These are the user's real folders on disk. Do not copy them into this workspace; \
         Read / Glob / LS them in place.\n",
    );
    for (id, root) in roots {
        out.push_str(&format!("- `{id}` → `{}`\n", root.display()));
    }
    out
}

fn named_roots(roots: &[PathBuf]) -> Vec<(String, PathBuf)> {
    roots
        .iter()
        .map(|p| {
            let id = p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "root".to_string());
            (id, p.clone())
        })
        .collect()
}

/// Resolve `@path` tokens in `text` against named roots (`id` / relative path).
pub fn resolve_named(text: &str, roots: &[(String, PathBuf)]) -> Vec<Mention> {
    let mut out = Vec::new();
    for raw in mention_tokens(text) {
        if let Some(found) = resolve_one(&raw, roots) {
            if out.iter().any(|m: &Mention| m.abs == found.abs) {
                continue;
            }
            out.push(found);
        }
    }
    out
}

/// Resolve `@path` tokens in `text` against directory roots.
pub fn resolve(text: &str, roots: &[PathBuf]) -> Vec<Mention> {
    let named = named_roots(roots);
    resolve_named(text, &named)
}

fn mention_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, ch) in text.char_indices() {
        if ch != '@' {
            continue;
        }
        let prev_ok = i == 0 || text[..i].chars().last().is_some_and(|p| p.is_whitespace());
        if !prev_ok {
            continue;
        }
        let rest = &text[i + 1..];
        let token: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
        if !token.is_empty() {
            out.push(token);
        }
    }
    out
}

fn join_under(root: &Path, rel: &str, raw: &str) -> Option<Mention> {
    if rel.is_empty() || rel == "." {
        return Some(Mention {
            raw: raw.to_string(),
            abs: root.to_path_buf(),
            is_dir: true,
        });
    }
    let candidate = root.join(rel);
    let abs = candidate.canonicalize().ok()?;
    let root_abs = root.canonicalize().ok()?;
    if !abs.starts_with(&root_abs) {
        return None;
    }
    Some(Mention {
        raw: raw.to_string(),
        abs,
        is_dir: candidate.is_dir() || raw.ends_with('/'),
    })
}

fn resolve_one(raw: &str, roots: &[(String, PathBuf)]) -> Option<Mention> {
    let trimmed = raw.trim().trim_end_matches('/');
    let rel = trimmed.trim_start_matches("./");
    if rel.contains('\0') || Path::new(rel).is_absolute() {
        return None;
    }
    if rel.is_empty() || rel == "." {
        let (_, root) = roots.first()?;
        return join_under(root, "", raw);
    }
    for (id, root) in roots {
        if rel == id {
            return join_under(root, "", raw);
        }
        let prefix = format!("{id}/");
        if let Some(rest) = rel.strip_prefix(&prefix) {
            return join_under(root, rest, raw);
        }
    }
    for (_, root) in roots {
        if let Some(found) = join_under(root, rel, raw) {
            return Some(found);
        }
    }
    None
}

pub fn extra_dirs(mentions: &[Mention], roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for m in mentions {
        for root in roots {
            let Ok(root_abs) = root.canonicalize() else {
                continue;
            };
            if m.abs.starts_with(&root_abs) && !dirs.iter().any(|d| d == &root_abs) {
                dirs.push(root_abs);
            }
        }
    }
    dirs
}

pub fn render_attached(mentions: &[Mention]) -> String {
    if mentions.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n## Attached paths\n\nThe user attached these with @. Read them before acting.\n",
    );
    for m in mentions {
        let kind = if m.is_dir { "directory" } else { "file" };
        out.push_str(&format!("- ({kind}) `{}`\n", m.abs.display()));
    }
    out
}

/// Expand the user prompt with resolved absolute paths and collect `--add-dir` roots.
pub fn prepare_prompt(text: &str, roots: &[PathBuf]) -> (String, Vec<PathBuf>) {
    let mentions = resolve(text, roots);
    if mentions.is_empty() {
        return (text.to_string(), Vec::new());
    }
    let mut prompt = text.to_string();
    prompt.push_str(&render_attached(&mentions));
    (prompt, extra_dirs(&mentions, roots))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn tree() -> (tempfile::TempDir, PathBuf) {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("src/util")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("src/util/io.rs"), "").unwrap();
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("node_modules/pkg/index.js"), "").unwrap();
        (dir, root)
    }

    #[test]
    fn at_query_tracks_unfinished_mention() {
        assert_eq!(at_query("look at @src"), Some("src"));
        assert_eq!(at_query("@"), Some(""));
        assert_eq!(at_query("see @src/main.rs please"), None);
        assert_eq!(at_query("mail user@host.com"), None);
        assert_eq!(at_query("hello"), None);
    }

    #[test]
    fn list_skips_node_modules_and_includes_dirs() {
        let (_tmp, root) = tree();
        let all = list_candidates(&root, 200);
        assert!(all.iter().any(|c| c.rel == "./"));
        assert!(all.iter().any(|c| c.rel == "src/" && c.is_dir));
        assert!(all.iter().any(|c| c.rel == "src/main.rs" && !c.is_dir));
        assert!(!all.iter().any(|c| c.rel.contains("node_modules")));
    }

    #[test]
    fn filter_matches_basename_and_path() {
        let (_tmp, root) = tree();
        let all = list_candidates(&root, 200);
        let hits = filter_candidates(&all, "main");
        assert!(hits.iter().any(|c| c.rel == "src/main.rs"));
        let src = filter_candidates(&all, "src/");
        assert!(src.iter().any(|c| c.rel == "src/"));
    }

    #[test]
    fn resolve_and_extra_dirs_stay_inside_root() {
        let (_tmp, root) = tree();
        let mentions = resolve("fix @src/main.rs and @src/util/", &[root.clone()]);
        assert_eq!(mentions.len(), 2);
        assert!(!mentions[0].is_dir);
        assert!(mentions[1].is_dir);
        let escaped = resolve("read @../secret", &[root.clone()]);
        assert!(escaped.is_empty());
        let dirs = extra_dirs(&mentions, &[root.clone()]);
        assert!(dirs.iter().any(|d| d == &root.canonicalize().unwrap()));
    }

    #[test]
    fn prepare_prompt_appends_absolute_paths() {
        let (_tmp, root) = tree();
        let (prompt, dirs) = prepare_prompt("please read @src/main.rs", &[root.clone()]);
        assert!(prompt.contains("Attached paths"));
        assert!(prompt.contains("main.rs"));
        assert!(!dirs.is_empty());
    }

    #[test]
    fn list_from_roots_prefixes_when_multiple() {
        let a = tempdir().unwrap();
        let b = tempdir().unwrap();
        fs::write(a.path().join("alpha.rs"), "").unwrap();
        fs::write(b.path().join("beta.rs"), "").unwrap();
        let roots = vec![
            ("proj-a".to_string(), a.path().to_path_buf()),
            ("proj-b".to_string(), b.path().to_path_buf()),
        ];
        let all = list_from_roots(&roots, 200);
        assert!(all.iter().any(|c| c.label == "proj-a/alpha.rs"));
        assert!(all.iter().any(|c| c.label == "proj-b/beta.rs"));
        let hits = filter_candidates(&all, "beta");
        assert!(hits.iter().any(|c| c.label == "proj-b/beta.rs"));
    }

    #[test]
    fn resolve_accepts_root_id_prefix() {
        let a = tempdir().unwrap();
        let b = tempdir().unwrap();
        fs::write(a.path().join("alpha.rs"), "").unwrap();
        fs::write(b.path().join("beta.rs"), "").unwrap();
        let roots = vec![
            ("proj-a".to_string(), a.path().to_path_buf()),
            ("proj-b".to_string(), b.path().to_path_buf()),
        ];
        let mentions = resolve_named("read @proj-b/beta.rs", &roots);
        assert_eq!(mentions.len(), 1);
        assert!(mentions[0].abs.ends_with("beta.rs"));
        let escaped = resolve_named("read @proj-a/../secret", &roots);
        assert!(escaped.is_empty());
    }

    #[test]
    fn context_roots_dedupes_cwd_that_is_a_project() {
        let home = tempdir().unwrap();
        let proj = tempdir().unwrap();
        let info = crate::project::add_project(home.path(), proj.path()).unwrap();
        let roots = context_roots(home.path(), proj.path());
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].0, info.id);
        let dirs = readable_dirs(&roots);
        assert_eq!(dirs.len(), 1);
        assert!(render_readable_dirs(&roots).contains("Readable directories"));
    }
}
