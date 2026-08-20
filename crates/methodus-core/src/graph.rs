//! Markdown-first knowledge graph indexing and compact facet extraction.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use methodus_domain::{GraphEdge, GraphNode};
use methodus_store::Store;
use serde::Serialize;
use serde_yaml::{Mapping, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::CoreError;

#[derive(Debug, Clone)]
pub struct GraphDocument {
    pub node: GraphNode,
    pub body: String,
    pub links: BTreeMap<String, Vec<String>>,
    pub frontmatter_keys: std::collections::BTreeSet<String>,
    /// Optional frontmatter kind. It is kept outside `GraphNode` so adding a
    /// new taxonomy never requires a SQLite schema migration.
    pub kind: Option<String>,
    pub evidence_waiver: bool,
    pub sources: Vec<SourceEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceEvidence {
    pub path: String,
    pub repository: Option<String>,
    pub fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GraphIssue {
    pub path: String,
    pub severity: IssueSeverity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum IssueSeverity {
    Error,
    Warning,
}

impl IssueSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

/// Index all independent graph Markdown nodes below the legacy graph root, Personal
/// root, and locally synced Team roots. Source files remain authoritative.
/// Missing or malformed documents are skipped so a single unfinished note never makes
/// the user's graph unreadable; callers can surface diagnostics later.
pub fn sync_graph(store: &Store, home: &Path) -> Result<usize, CoreError> {
    // SQLite is a disposable projection. Rebuild it so deleted or moved Markdown
    // nodes cannot remain visible to the read-only Agent CLI.
    store.clear_graph_index()?;
    let mut indexed = 0;
    let mut roots = vec![home.join("graph"), home.join("personal")];
    let teams = home.join("teams");
    if teams.is_dir() {
        for entry in fs::read_dir(teams)? {
            let path = entry?.path();
            if path.is_dir() { roots.push(path); }
        }
    }
    roots.sort();
    for root in roots {
        if !root.exists() { continue; }
        for path in markdown_files(&root)? {
            let Ok(mut document) = read_graph_document(home, &path) else { continue; };
            if sources_are_stale(home, &document.sources)
                && !matches!(document.node.status.as_deref(), Some("candidate" | "rejected" | "deprecated"))
            {
                document.node.status = Some("stale".into());
            }
            store.upsert_graph_node(&document.node)?;
            let now = Utc::now();
            let from_id = document.node.id.clone();
            let edges = document.links.iter().flat_map(|(relation, targets)| {
                let from_id = from_id.clone();
                targets.iter().map(move |target| GraphEdge {
                    id: format!("edge_{}", Uuid::new_v4()), from_id: from_id.clone(), relation: relation.clone(),
                    to_id: target.clone(), source: "authored".to_string(), confidence: None,
                    evidence_refs: Vec::new(), created_at: now, updated_at: now,
                })
            }).collect::<Vec<_>>();
            store.replace_graph_edges(&document.node.id, &edges)?;
            indexed += 1;
        }
    }
    Ok(indexed)
}

/// Validate Markdown graph files without changing the index. This is used by
/// Team review/publish planning and reports malformed files, duplicate IDs, and
/// broken authored links instead of silently omitting them.
pub fn validate_graph(home: &Path) -> Result<Vec<GraphIssue>, CoreError> {
    let roots = graph_roots(home)?;
    let mut issues = Vec::new();
    let mut documents = Vec::new();
    for root in roots {
        if !root.exists() { continue; }
        for path in markdown_files(&root)? {
            match read_graph_document(home, &path) {
                Ok(document) => {
                    let relative = path.strip_prefix(home).unwrap_or(&path).display().to_string();
                    issues.extend(validate_document(home, &path, &document));
                    documents.push((path, document));
                    if relative.is_empty() {
                        issues.push(GraphIssue { path: relative, severity: IssueSeverity::Error, message: "graph document has no relative path".into() });
                    }
                }
                Err(error) => issues.push(GraphIssue { path: path.strip_prefix(home).unwrap_or(&path).display().to_string(), severity: IssueSeverity::Error, message: error.to_string() }),
            }
        }
    }
    let mut ids = std::collections::HashMap::<String, String>::new();
    for (path, document) in &documents {
        let relative = path.strip_prefix(home).unwrap_or(path).to_string_lossy().replace('\\', "/");
        if let Some(previous) = ids.insert(document.node.id.clone(), relative.clone()) {
            issues.push(GraphIssue { path: relative, severity: IssueSeverity::Error, message: format!("duplicate id '{}' also declared by {previous}", document.node.id) });
        }
    }
    let known = ids.keys().cloned().collect::<std::collections::HashSet<_>>();
    for (path, document) in &documents {
        let relative = path.strip_prefix(home).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        for targets in document.links.values() {
            for target in targets {
                if !known.contains(target) { issues.push(GraphIssue { path: relative.clone(), severity: IssueSeverity::Error, message: format!("broken link target '{target}'") }); }
            }
        }
    }
    let mut connected = std::collections::HashSet::new();
    for (_, document) in &documents {
        if !document.links.is_empty() { connected.insert(document.node.id.clone()); }
        for targets in document.links.values() {
            connected.extend(targets.iter().cloned());
        }
    }
    for (path, document) in documents {
        if !connected.contains(&document.node.id) {
            issues.push(GraphIssue {
                path: path.strip_prefix(home).unwrap_or(&path).display().to_string(),
                severity: IssueSeverity::Warning,
                message: format!("orphan node '{}' has no authored relation", document.node.id),
            });
        }
    }
    Ok(issues)
}

fn validate_document(home: &Path, path: &Path, document: &GraphDocument) -> Vec<GraphIssue> {
    let relative = path.strip_prefix(home).unwrap_or(path).to_string_lossy().replace('\\', "/");
    let mut issues = Vec::new();
    let required = ["id", "title", "node_type", "kind", "status", "visibility", "summary"];
    for key in required {
        let value_empty = match key {
            "id" => document.node.id.trim().is_empty(),
            "title" => document.node.title.trim().is_empty(),
            "node_type" => document.node.node_type.trim().is_empty(),
            "kind" => document.kind.as_deref().unwrap_or("").trim().is_empty(),
            "status" => document.node.status.as_deref().unwrap_or("").trim().is_empty(),
            "visibility" => document.node.visibility.trim().is_empty(),
            "summary" => document.node.summary.as_deref().unwrap_or("").trim().is_empty(),
            _ => false,
        };
        if value_empty || !document.frontmatter_keys.contains(key) {
            issues.push(GraphIssue { path: relative.clone(), severity: IssueSeverity::Error, message: format!("missing required frontmatter field '{key}'") });
        }
    }
    if let Some(status) = document.node.status.as_deref() {
        if !matches!(status, "candidate" | "committed" | "stale" | "deprecated" | "rejected") {
            issues.push(GraphIssue { path: relative.clone(), severity: IssueSeverity::Error, message: format!("unsupported lifecycle status '{status}'") });
        }
    }
    if !matches!(document.node.node_type.as_str(), "knowledge" | "method" | "experience") {
        issues.push(GraphIssue { path: relative.clone(), severity: IssueSeverity::Warning, message: format!("unknown active node type '{}'", document.node.node_type) });
    }
    if !matches!(document.node.visibility.as_str(), "personal" | "team") {
        issues.push(GraphIssue { path: relative.clone(), severity: IssueSeverity::Error, message: format!("unsupported visibility '{}'", document.node.visibility) });
    }
    let in_team = relative.starts_with("teams/");
    let in_personal = relative.starts_with("personal/") || relative.starts_with("graph/");
    if in_team && document.node.visibility != "team" {
        issues.push(GraphIssue { path: relative.clone(), severity: IssueSeverity::Error, message: "Team files must use visibility: team".into() });
    }
    if in_personal && document.node.visibility == "team" && !relative.starts_with("graph/") {
        issues.push(GraphIssue { path: relative.clone(), severity: IssueSeverity::Error, message: "Personal files cannot declare visibility: team".into() });
    }
    if in_team && matches!(document.node.node_type.as_str(), "knowledge" | "method") && document.sources.is_empty() && !document.evidence_waiver {
        issues.push(GraphIssue { path: relative.clone(), severity: IssueSeverity::Error, message: "Team Knowledge/Method requires at least one evidence source or evidence_waiver: true".into() });
    }
    const RELATIONS: &[&str] = &[
        "requires", "next_step", "alternative_to", "conflicts_with", "indicates",
        "emitted_by", "caused_by", "affects", "implemented_by", "introduced_by",
        "supersedes", "validated_by", "contradicted_by", "derived_from", "used_method",
        "validates", "refines",
    ];
    for relation in document.links.keys() {
        if !RELATIONS.contains(&relation.as_str()) {
            issues.push(GraphIssue { path: relative.clone(), severity: IssueSeverity::Warning, message: format!("unknown relation type '{relation}'") });
        }
    }
    issues
}

fn graph_roots(home: &Path) -> Result<Vec<PathBuf>, CoreError> {
    let mut roots = vec![home.join("graph"), home.join("personal")];
    let teams = home.join("teams");
    if teams.is_dir() {
        for entry in fs::read_dir(teams)? {
            let path = entry?.path();
            if path.is_dir() { roots.push(path); }
        }
    }
    roots.sort();
    Ok(roots)
}

pub fn read_graph_document(home: &Path, path: &Path) -> Result<GraphDocument, CoreError> {
    let raw = fs::read_to_string(path)?;
    let (frontmatter, body) = split_frontmatter(&raw);
    let meta = parse_frontmatter(frontmatter)?;
    let relative = path
        .strip_prefix(home)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let inferred_type = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .unwrap_or("knowledge")
        .trim_end_matches('s');
    let id = string_value(&meta, "id").unwrap_or_else(|| {
        let stem = path.file_stem().and_then(|v| v.to_str()).unwrap_or("untitled");
        format!("{inferred_type}/{stem}")
    });
    let title = string_value(&meta, "title").unwrap_or_else(|| id.clone());
    let node_type = string_value(&meta, "node_type").unwrap_or_else(|| inferred_type.to_string());
    let now = Utc::now();
    let hash = format!("{:x}", Sha256::digest(raw.as_bytes()));
    let links = links_value(&meta);
    let kind = string_value(&meta, "kind");
    let evidence_waiver = bool_value(&meta, "evidence_waiver");
    let sources = sources_value(&meta);
    let frontmatter_keys = meta.keys().filter_map(Value::as_str).map(str::to_string).collect();
    Ok(GraphDocument {
        node: GraphNode {
            id,
            node_type,
            title,
            path: relative,
            content_hash: hash,
            status: string_value(&meta, "status"),
            summary: string_value(&meta, "summary"),
            scope: string_value(&meta, "scope"),
            visibility: string_value(&meta, "visibility").unwrap_or_else(|| "personal".into()),
            tags: string_list_value(&meta, "tags"),
            confidence: float_value(&meta, "confidence"),
            created_at: now,
            updated_at: now,
        },
        body: body.to_string(),
        links,
        frontmatter_keys,
        kind,
        evidence_waiver,
        sources,
    })
}

/// Extract a single Markdown facet body. The full document remains a lazy reference.
pub fn facet(body: &str, wanted: &str) -> Option<String> {
    let heading = format!("## {}", wanted.to_ascii_lowercase());
    let mut found = false;
    let mut lines = Vec::new();
    for line in body.lines() {
        if line.trim_start().to_ascii_lowercase().starts_with(&heading) {
            found = true;
            continue;
        }
        if found && line.trim_start().starts_with("## ") {
            break;
        }
        if found {
            lines.push(line);
        }
    }
    let text = lines.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

pub fn estimated_tokens(text: &str) -> i64 {
    // Stable, deliberately conservative estimate for pre-launch budget previews.
    ((text.chars().count() + 3) / 4) as i64
}

fn markdown_files(root: &Path) -> Result<Vec<PathBuf>, CoreError> {
    let mut out = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("md")) {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

fn split_frontmatter(raw: &str) -> (&str, &str) {
    let Some(rest) = raw.strip_prefix("---\n") else {
        return ("", raw);
    };
    if let Some(end) = rest.find("\n---\n") {
        (&rest[..end], &rest[end + 5..])
    } else {
        ("", raw)
    }
}

fn parse_frontmatter(raw: &str) -> Result<Mapping, CoreError> {
    if raw.trim().is_empty() {
        return Ok(Mapping::new());
    }
    serde_yaml::from_str::<Mapping>(raw)
        .map_err(|error| CoreError::Other(format!("invalid graph frontmatter: {error}")))
}

fn string_value(map: &Mapping, key: &str) -> Option<String> {
    map.get(Value::String(key.to_string()))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn float_value(map: &Mapping, key: &str) -> Option<f64> {
    map.get(Value::String(key.to_string())).and_then(Value::as_f64)
}

fn bool_value(map: &Mapping, key: &str) -> bool {
    map.get(Value::String(key.to_string())).and_then(Value::as_bool).unwrap_or(false)
}

fn string_list_value(map: &Mapping, key: &str) -> Vec<String> {
    match map.get(Value::String(key.to_string())) {
        Some(Value::String(value)) => vec![value.clone()],
        Some(Value::Sequence(values)) => values.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

fn links_value(map: &Mapping) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let Some(Value::Mapping(links)) = map.get(Value::String("links".to_string())) else {
        return out;
    };
    for (relation, targets) in links {
        let Some(relation) = relation.as_str() else { continue };
        let values = match targets {
            Value::String(value) => vec![value.clone()],
            Value::Sequence(values) => values.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            _ => Vec::new(),
        };
        if !values.is_empty() {
            out.insert(relation.to_string(), values);
        }
    }
    out
}

fn sources_value(map: &Mapping) -> Vec<SourceEvidence> {
    let Some(Value::Sequence(sources)) = map.get(Value::String("sources".into())) else { return Vec::new() };
    sources.iter().filter_map(|source| {
        let Value::Mapping(source) = source else { return None };
        let path = source.get(Value::String("path".into())).and_then(Value::as_str)?.trim();
        if path.is_empty() { return None; }
        Some(SourceEvidence {
            path: path.into(),
            repository: source.get(Value::String("repository".into())).and_then(Value::as_str).map(str::to_string),
            fingerprint: source.get(Value::String("fingerprint".into())).and_then(Value::as_str).map(str::to_string),
        })
    }).collect()
}

fn sources_are_stale(home: &Path, sources: &[SourceEvidence]) -> bool {
    sources.iter().any(|source| {
        let Some(expected) = source.fingerprint.as_deref().and_then(|value| value.strip_prefix("sha256:")) else { return false };
        let mut candidates = Vec::new();
        let path = Path::new(&source.path);
        if path.is_absolute() { candidates.push(path.to_path_buf()); }
        candidates.push(home.join(path));
        if let Some(repository) = &source.repository {
            candidates.push(home.join("projects").join(repository).join(path));
            candidates.push(home.join("teams").join(repository).join(path));
        }
        let Some(current) = candidates.into_iter().find_map(|path| fs::read(path).ok()) else { return true };
        format!("{:x}", Sha256::digest(current)) != expected
    })
}

/// Return whether any recorded source is changed or missing.
pub fn sources_are_stale_now(home: &Path, sources: &[SourceEvidence]) -> bool {
    sources_are_stale(home, sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use methodus_store::Store;
    use tempfile::tempdir;

    #[test]
    fn indexes_markdown_nodes_and_typed_links() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        fs::create_dir_all(home.join("graph/knowledge")).unwrap();
        fs::write(home.join("graph/knowledge/idempotency.md"), "---\nid: knowledge/idempotency\ntitle: Idempotency\nsummary: Avoid duplicate side effects\nlinks:\n  requires: [knowledge/unique-key]\n---\n## Execute\nUse a stable key.\n").unwrap();
        let store = Store::open_memory().unwrap();
        assert_eq!(sync_graph(&store, home).unwrap(), 1);
        assert_eq!(store.graph_node("knowledge/idempotency").unwrap().unwrap().title, "Idempotency");
        assert_eq!(store.graph_edges_for("knowledge/idempotency").unwrap().len(), 1);
    }

    #[test]
    fn source_fingerprint_marks_a_committed_node_stale() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("graph/knowledge")).unwrap();
        fs::write(dir.path().join("evidence.txt"), "version one").unwrap();
        let fingerprint = format!("{:x}", Sha256::digest(b"version one"));
        fs::write(dir.path().join("graph/knowledge/source.md"), format!("---\nid: knowledge/source\ntitle: Source backed note\nnode_type: knowledge\nstatus: committed\nsources:\n  - path: evidence.txt\n    fingerprint: sha256:{fingerprint}\n---\n\n## Execute\nUse the source.\n")).unwrap();
        let store = Store::open_memory().unwrap();
        sync_graph(&store, dir.path()).unwrap();
        assert_eq!(store.graph_node("knowledge/source").unwrap().unwrap().status.as_deref(), Some("committed"));
        fs::write(dir.path().join("evidence.txt"), "version two").unwrap();
        sync_graph(&store, dir.path()).unwrap();
        assert_eq!(store.graph_node("knowledge/source").unwrap().unwrap().status.as_deref(), Some("stale"));
    }

    #[test]
    fn sync_removes_deleted_markdown_nodes_from_projection() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("personal/knowledge");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("temporary.md");
        fs::write(&path, "---\nid: knowledge/temporary\ntitle: Temporary\nnode_type: knowledge\nstatus: committed\n---\n\n## Execute\nRead it.\n").unwrap();
        let store = Store::open_memory().unwrap();
        sync_graph(&store, dir.path()).unwrap();
        assert!(store.graph_node("knowledge/temporary").unwrap().is_some());
        fs::remove_file(path).unwrap();
        sync_graph(&store, dir.path()).unwrap();
        assert!(store.graph_node("knowledge/temporary").unwrap().is_none());
    }

    #[test]
    fn validation_reports_broken_links_and_duplicate_ids() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("personal/knowledge")).unwrap();
        let note = "---\nid: knowledge/same\ntitle: Same\nnode_type: knowledge\nstatus: committed\nlinks:\n  next_step: [knowledge/missing]\n---\n\n## Execute\nRead it.\n";
        fs::write(dir.path().join("personal/knowledge/a.md"), note).unwrap();
        fs::write(dir.path().join("personal/knowledge/b.md"), note.replace("title: Same", "title: Duplicate")).unwrap();
        let issues = validate_graph(dir.path()).unwrap();
        assert!(issues.iter().any(|issue| issue.message.contains("duplicate id")));
        assert!(issues.iter().any(|issue| issue.message.contains("broken link")));
    }

    #[test]
    fn validation_reports_required_fields_and_orphan_warning() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("personal/knowledge");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("incomplete.md"), "---\nid: knowledge/incomplete\ntitle: Incomplete\nnode_type: knowledge\nstatus: committed\nvisibility: personal\nsummary: Missing kind\n---\n\n## Learn\nDraft\n").unwrap();
        let issues = validate_graph(dir.path()).unwrap();
        assert!(issues.iter().any(|issue| issue.severity == IssueSeverity::Error && issue.message.contains("kind")));
        assert!(issues.iter().any(|issue| issue.severity == IssueSeverity::Warning && issue.message.contains("orphan")));
    }

    #[test]
    fn team_reusable_nodes_require_evidence_or_an_explicit_waiver() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("teams/default/knowledge");
        fs::create_dir_all(&root).unwrap();
        let body = "---\nid: knowledge/team-rule\ntitle: Team rule\nnode_type: knowledge\nkind: procedure\nstatus: committed\nvisibility: team\nsummary: A team rule\n---\n\n## Execute\nUse it.\n";
        fs::write(root.join("rule.md"), body).unwrap();
        let issues = validate_graph(dir.path()).unwrap();
        assert!(issues.iter().any(|issue| issue.severity == IssueSeverity::Error && issue.message.contains("evidence")));
        fs::write(root.join("rule.md"), body.replace("summary: A team rule", "summary: A team rule\nevidence_waiver: true")).unwrap();
        assert!(!validate_graph(dir.path()).unwrap().iter().any(|issue| issue.message.contains("evidence")));
    }

    #[test]
    fn extracts_a_single_facet() {
        let body = "## Learn\nExplain it.\n\n## Execute\nUse a key.\n\n## Evidence\nTest.";
        assert_eq!(facet(body, "Execute").as_deref(), Some("Use a key."));
    }
}
