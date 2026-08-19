//! Markdown-first knowledge graph indexing and compact facet extraction.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use methodus_domain::{GraphEdge, GraphNode};
use methodus_store::Store;
use serde_yaml::{Mapping, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::CoreError;

#[derive(Debug, Clone)]
pub struct GraphDocument {
    pub node: GraphNode,
    pub body: String,
    pub links: BTreeMap<String, Vec<String>>,
}

/// Index all independent graph Markdown nodes below `~/.methodus/graph/`.
/// Missing or malformed documents are skipped so a single unfinished note never makes
/// the user's graph unreadable; callers can surface diagnostics later.
pub fn sync_graph(store: &Store, home: &Path) -> Result<usize, CoreError> {
    let root = home.join("graph");
    if !root.exists() {
        return Ok(0);
    }
    let mut indexed = 0;
    for path in markdown_files(&root)? {
        let Ok(document) = read_graph_document(home, &path) else {
            continue;
        };
        store.upsert_graph_node(&document.node)?;
        let now = Utc::now();
        let from_id = document.node.id.clone();
        let edges = document
            .links
            .iter()
            .flat_map(|(relation, targets)| {
                let from_id = from_id.clone();
                targets.iter().map(move |target| GraphEdge {
                    id: format!("edge_{}", Uuid::new_v4()),
                    from_id: from_id.clone(),
                    relation: relation.clone(),
                    to_id: target.clone(),
                    source: "authored".to_string(),
                    confidence: None,
                    evidence_refs: Vec::new(),
                    created_at: now,
                    updated_at: now,
                })
            })
            .collect::<Vec<_>>();
        store.replace_graph_edges(&document.node.id, &edges)?;
        indexed += 1;
    }
    Ok(indexed)
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
            confidence: float_value(&meta, "confidence"),
            created_at: now,
            updated_at: now,
        },
        body: body.to_string(),
        links,
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
    fn extracts_a_single_facet() {
        let body = "## Learn\nExplain it.\n\n## Execute\nUse a key.\n\n## Evidence\nTest.";
        assert_eq!(facet(body, "Execute").as_deref(), Some("Use a key."));
    }
}
