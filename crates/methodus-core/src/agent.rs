//! Read-only Agent-facing retrieval. This module is deliberately independent from
//! task/workspace/session orchestration: runtimes call it through `methodus agent`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use methodus_domain::GraphNode;
use methodus_store::Store;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::config::UserConfig;
use crate::error::CoreError;
use crate::graph::{facet, read_graph_document, GraphDocument, SourceEvidence};

pub const AGENT_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct AgentItem {
    pub id: String,
    pub node_type: String,
    pub kind: Option<String>,
    pub title: String,
    pub facet: String,
    pub status: String,
    pub visibility: String,
    pub summary: String,
    pub content: String,
    pub rationale: String,
    pub path: String,
    pub content_hash: String,
    pub sources: Vec<SourceEvidence>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSearchResult {
    pub id: String,
    pub node_type: String,
    pub title: String,
    pub kind: Option<String>,
    pub status: String,
    pub visibility: String,
    pub summary: String,
    pub path: String,
    pub content_hash: String,
    pub score: i64,
    pub rationale: String,
    pub warnings: Vec<String>,
}

/// An inventory of the consumer-visible graph. It does not select content for a
/// question; it tells a runtime where the authoritative Markdown files are and
/// what each file contains.
#[derive(Debug, Clone, Serialize)]
pub struct AgentManifestItem {
    pub id: String,
    pub node_type: String,
    pub kind: Option<String>,
    pub title: String,
    pub status: String,
    pub visibility: String,
    pub summary: String,
    pub path: String,
    pub absolute_path: String,
    pub scope: Option<String>,
    pub tags: Vec<String>,
    pub facets: Vec<String>,
    pub sources: Vec<SourceEvidence>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentDirectory {
    pub path: String,
    pub absolute_path: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentManifest {
    pub protocol_version: u32,
    pub command: String,
    pub index_revision: String,
    pub home: String,
    pub selected_team: String,
    pub directory_structure: Vec<AgentDirectory>,
    pub graph_roots: Vec<String>,
    pub items: Vec<AgentManifestItem>,
    pub warnings: Vec<String>,
}

pub struct AgentQuery<'a> {
    store: &'a Store,
    home: &'a Path,
}

impl<'a> AgentQuery<'a> {
    pub fn new(store: &'a Store, home: &'a Path) -> Self { Self { store, home } }

    /// A content-derived revision for the read-only projection. It deliberately
    /// excludes query time so a connector can compare two responses reliably.
    pub fn index_revision(&self) -> Result<String, CoreError> {
        index_revision(self.store)
    }

    /// Return the consumer-visible graph inventory without trying to answer a
    /// question. This is used by the interactive Use runtime as an environment
    /// manifest; the runtime is responsible for reading and reasoning over the
    /// listed Markdown files.
    pub fn manifest(&self, scopes: &[String]) -> Result<AgentManifest, CoreError> {
        let revision = self.index_revision()?;
        let mut items = self
            .visible_nodes(scopes)?
            .into_iter()
            .filter_map(|node| {
                let path = safe_node_path(self.home, &node.path).ok()?;
                let document = read_graph_document(self.home, &path).ok()?;
                let facets = document
                    .body
                    .lines()
                    .filter_map(|line| line.strip_prefix("## "))
                    .map(str::trim)
                    .filter(|heading| !heading.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                Some(AgentManifestItem {
                    id: node.id,
                    node_type: node.node_type,
                    kind: document.kind,
                    title: node.title,
                    status: node.status.unwrap_or_else(|| "committed".into()),
                    visibility: node.visibility,
                    summary: node.summary.unwrap_or_default(),
                    path: node.path,
                    absolute_path: path.display().to_string(),
                    scope: node.scope,
                    tags: node.tags,
                    facets,
                    sources: document.sources,
                })
            })
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            type_rank(&left.node_type)
                .cmp(&type_rank(&right.node_type))
                .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
                .then_with(|| left.id.cmp(&right.id))
        });
        let selected_team = UserConfig::load(self.home).selected_team().to_string();
        let directory_structure = directory_structure(self.home, &selected_team);
        let mut graph_roots = directory_structure
            .iter()
            .filter(|directory| {
                directory.exists
                    && Path::new(&directory.path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| matches!(name, "knowledge" | "methods" | "experiences"))
            })
            .map(|directory| PathBuf::from(&directory.absolute_path))
            .collect::<Vec<_>>();
        graph_roots.sort();
        graph_roots.dedup();
        let mut warnings = Vec::new();
        if items.is_empty() {
            warnings.push("no consumer-visible Methodus nodes are indexed".into());
        }
        if items.iter().any(|item| item.status == "stale") {
            warnings.push("the graph contains stale nodes; revalidate their source evidence before treating them as current".into());
        }
        Ok(AgentManifest {
            protocol_version: AGENT_PROTOCOL_VERSION,
            command: "manifest".into(),
            index_revision: revision,
            home: self.home.display().to_string(),
            selected_team,
            directory_structure,
            graph_roots: graph_roots
                .into_iter()
                .map(|path| path.display().to_string())
                .collect(),
            items,
            warnings,
        })
    }

    pub fn search(&self, query: &str, node_types: &[String], kinds: &[String], scopes: &[String], limit: usize) -> Result<Vec<AgentSearchResult>, CoreError> {
        let terms = terms(query);
        let allowed_types = node_types.iter().map(|value| value.to_lowercase()).collect::<HashSet<_>>();
        let allowed_kinds = kinds.iter().map(|value| value.to_lowercase()).collect::<HashSet<_>>();
        let mut result = self.visible_nodes(scopes)?.into_iter()
            .filter(|node| allowed_types.is_empty() || allowed_types.contains(&node.node_type.to_lowercase()))
            .filter_map(|node| {
                let kind = self.kind_for(&node);
                if !allowed_kinds.is_empty() && !kind.as_deref().is_some_and(|value| allowed_kinds.contains(&value.to_lowercase())) { return None; }
                let score = score_node(&node, &terms);
                (score > 0).then(|| {
                    let mut warnings = Vec::new();
                    if node.status.as_deref() == Some("stale") {
                        warnings.push("source evidence is stale; revalidate before treating this as a current rule".into());
                    }
                    AgentSearchResult {
                    id: node.id, node_type: node.node_type, title: node.title, kind,
                    status: node.status.unwrap_or_else(|| "committed".into()), visibility: node.visibility,
                    summary: node.summary.unwrap_or_default(), path: node.path, content_hash: node.content_hash,
                    score, rationale: "matched title, summary, scope, id, or tags".into(), warnings,
                }})
            }).collect::<Vec<_>>();
        result.sort_by(|left, right| right.score.cmp(&left.score).then_with(|| left.title.cmp(&right.title)).then_with(|| left.id.cmp(&right.id)));
        result.truncate(limit.clamp(1, 50));
        Ok(result)
    }

    pub fn get(&self, id: &str, requested_facet: Option<&str>, include_history: bool) -> Result<AgentItem, CoreError> {
        let node = self.store.graph_node(id)?.ok_or_else(|| CoreError::Other(format!("agent node not found: {id}")))?;
        let status = node.status.as_deref().unwrap_or("committed");
        if matches!(status, "candidate" | "rejected") { return Err(CoreError::Other(format!("node {id} is not consumer-visible ({status})"))); }
        if status == "deprecated" && !include_history { return Err(CoreError::Other(format!("node {id} is deprecated; pass history mode to inspect it"))); }
        let document = read_graph_document(self.home, &safe_node_path(self.home, &node.path)?)?;
        if !consumer_document_valid(&node, &document) {
            return Err(CoreError::Other(format!("node {id} fails graph consumer validation")));
        }
        let facet_name = requested_facet.unwrap_or_else(|| preferred_facet(&node));
        let content = self.node_content(&node, facet_name)?;
        self.item(&node, facet_name, content, "explicit node request".into())
    }

    pub fn related(&self, id: &str, relation: Option<&str>, limit: usize) -> Result<Vec<AgentSearchResult>, CoreError> {
        let root = self.store.graph_node(id)?.ok_or_else(|| CoreError::Other(format!("agent node not found: {id}")))?;
        let root_status = root.status.as_deref().unwrap_or("committed");
        if matches!(root_status, "candidate" | "rejected") {
            return Err(CoreError::Other(format!("node {id} is not consumer-visible ({root_status})")));
        }
        if root_status == "deprecated" {
            return Err(CoreError::Other(format!("node {id} is deprecated; pass history mode to inspect it")));
        }
        let edges = self.store.graph_edges_for(id)?;
        let ids = edges.into_iter().filter(|edge| relation.is_none_or(|wanted| edge.relation == wanted)).map(|edge| if edge.from_id == id { edge.to_id } else { edge.from_id }).collect::<Vec<_>>();
        let mut output = Vec::new();
        let max = limit.clamp(1, 50);
        for related_id in ids {
            if output.len() >= max { break; }
            let Some(node) = self.store.graph_node(&related_id)? else { continue; };
            let status = node.status.clone().unwrap_or_else(|| "committed".into());
            if matches!(status.as_str(), "candidate" | "rejected" | "deprecated") { continue; }
            let kind = self.kind_for(&node);
            let warnings = if status == "stale" {
                vec!["source evidence is stale; revalidate before treating this as a current rule".into()]
            } else {
                Vec::new()
            };
            output.push(AgentSearchResult {
                id: node.id, node_type: node.node_type, title: node.title, kind, status,
                visibility: node.visibility, summary: node.summary.unwrap_or_default(), path: node.path,
                content_hash: node.content_hash, score: 0, rationale: "typed graph relation".into(), warnings,
            });
        }
        Ok(output)
    }

    fn visible_nodes(&self, scopes: &[String]) -> Result<Vec<GraphNode>, CoreError> {
        let scope_filter = scopes.iter().map(|scope| scope.to_lowercase()).collect::<HashSet<_>>();
        Ok(self.store.list_graph_nodes(None)?.into_iter().filter_map(|node| {
            let status = node.status.as_deref().unwrap_or("committed");
            let visible_status = matches!(status, "committed" | "stale");
            let visible_scope = scope_filter.is_empty() || scope_filter.contains(&node.visibility.to_lowercase());
            if !visible_status || !visible_scope { return None; }
            let path = safe_node_path(self.home, &node.path).ok()?;
            let document = read_graph_document(self.home, &path).ok()?;
            consumer_document_valid(&node, &document).then_some(node)
        }).collect())
    }

    fn node_content(&self, node: &GraphNode, facet_name: &str) -> Result<String, CoreError> {
        let document = read_graph_document(self.home, &safe_node_path(self.home, &node.path)?)?;
        if facet_name.eq_ignore_ascii_case("all") { return Ok(document.body.trim().to_string()); }
        Ok(facet(&document.body, facet_name).unwrap_or_else(|| document.body.trim().to_string()))
    }

    fn item(&self, node: &GraphNode, facet_name: &str, content: String, rationale: String) -> Result<AgentItem, CoreError> {
        let mut warnings = Vec::new();
        if node.status.as_deref() == Some("stale") { warnings.push("source evidence is stale; revalidate before treating this as a current rule".into()); }
        let sources = read_graph_document(self.home, &safe_node_path(self.home, &node.path)?)?.sources;
        Ok(AgentItem {
            id: node.id.clone(), node_type: node.node_type.clone(), kind: self.kind_for(node), title: node.title.clone(), facet: facet_name.to_string(),
            status: node.status.clone().unwrap_or_else(|| "committed".into()), visibility: node.visibility.clone(), summary: node.summary.clone().unwrap_or_default(),
            content, rationale, path: node.path.clone(), content_hash: node.content_hash.clone(), sources, warnings,
        })
    }

    fn kind_for(&self, node: &GraphNode) -> Option<String> {
        safe_node_path(self.home, &node.path).ok().and_then(|path| read_graph_document(self.home, &path).ok()).and_then(|document| document.kind)
    }
}

fn directory_structure(home: &Path, selected_team: &str) -> Vec<AgentDirectory> {
    let mut paths = vec![
        "graph".to_string(),
        "graph/knowledge".to_string(),
        "graph/methods".to_string(),
        "graph/experiences".to_string(),
        "personal".to_string(),
        "personal/knowledge".to_string(),
        "personal/methods".to_string(),
        "personal/experiences".to_string(),
        "teams".to_string(),
        "teams/default".to_string(),
        "teams/default/knowledge".to_string(),
        "teams/default/methods".to_string(),
        "teams/default/experiences".to_string(),
    ];
    for suffix in ["", "/knowledge", "/methods", "/experiences"] {
        paths.push(format!("teams/{selected_team}{suffix}"));
    }
    let teams = home.join("teams");
    if let Ok(entries) = fs::read_dir(&teams) {
        for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
            let Some(team_id) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            for suffix in ["", "/knowledge", "/methods", "/experiences"] {
                paths.push(format!("teams/{team_id}{suffix}"));
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths.into_iter()
        .map(|path| {
            let absolute_path = home.join(&path);
            AgentDirectory {
                path,
                absolute_path: absolute_path.display().to_string(),
                exists: absolute_path.is_dir(),
            }
        })
        .collect()
}

fn consumer_document_valid(node: &GraphNode, document: &GraphDocument) -> bool {
    let required = ["id", "title", "node_type", "kind", "status", "visibility", "summary"];
    required.iter().all(|key| document.frontmatter_keys.contains(*key))
        && document.kind.as_deref().is_some_and(|kind| !kind.trim().is_empty())
        && document.node.summary.as_deref().is_some_and(|summary| !summary.trim().is_empty())
        && matches!(node.node_type.as_str(), "knowledge" | "method" | "experience")
        && matches!(node.visibility.as_str(), "personal" | "team")
        && document.node.id == node.id
        && (!node.path.starts_with("teams/") || !matches!(node.node_type.as_str(), "knowledge" | "method") || !document.sources.is_empty() || document.evidence_waiver)
        && matches!(document.node.status.as_deref(), Some("candidate" | "committed" | "stale" | "deprecated" | "rejected"))
}

fn safe_node_path(home: &Path, relative: &str) -> Result<PathBuf, CoreError> {
    let path = Path::new(relative);
    if path.is_absolute() || path.components().any(|component| component == std::path::Component::ParentDir) {
        return Err(CoreError::Other(format!("unsafe graph path: {relative}")));
    }
    Ok(home.join(path))
}

/// Compute a stable revision from all indexed graph rows and authored edges.
pub fn index_revision(store: &Store) -> Result<String, CoreError> {
    let mut nodes = store.list_graph_nodes(None)?;
    nodes.sort_by(|left, right| left.id.cmp(&right.id));
    let mut material = String::new();
    for node in nodes {
        material.push_str(&format!(
            "node\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\n",
            node.id, node.node_type, node.title, node.path, node.content_hash,
            node.status.unwrap_or_default(), node.visibility, node.scope.unwrap_or_default(),
            node.tags.join(","),
        ));
    }
    let mut edges = Vec::new();
    for node in store.list_graph_nodes(None)? {
        for edge in store.graph_edges_for(&node.id)? {
            edges.push((edge.from_id, edge.relation, edge.to_id));
        }
    }
    edges.sort();
    for (from, relation, to) in edges {
        material.push_str(&format!("edge\0{from}\0{relation}\0{to}\n"));
    }
    Ok(format!("sha256:{:x}", Sha256::digest(material.as_bytes())))
}

fn preferred_facet(node: &GraphNode) -> &'static str { match node.node_type.as_str() { "method" | "knowledge" => "Execute", "experience" => "Reusable lesson", _ => "all" } }
fn type_rank(node_type: &str) -> u8 { match node_type { "method" => 0, "knowledge" => 1, "experience" => 2, _ => 3 } }

/// Tokenize mixed natural-language questions without requiring spaces between
/// Chinese words and Latin identifiers. CJK runs keep the full phrase and add
/// bigrams so a question like `nxm进程崩溃后怎么处理` can match a title such as
/// `nxm 进程崩溃的信号处理与恢复链路`.
fn terms(input: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut ascii = String::new();
    let mut cjk = String::new();
    let flush_ascii = |terms: &mut Vec<String>, ascii: &mut String| {
        if !ascii.is_empty() {
            terms.push(ascii.to_lowercase());
            ascii.clear();
        }
    };
    let flush_cjk = |terms: &mut Vec<String>, cjk: &mut String| {
        if cjk.is_empty() {
            return;
        }
        let chars = cjk.chars().collect::<Vec<_>>();
        terms.push(cjk.clone());
        if chars.len() > 1 {
            for pair in chars.windows(2) {
                terms.push(pair.iter().collect());
            }
        }
        cjk.clear();
    };
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            flush_cjk(&mut terms, &mut cjk);
            ascii.push(ch);
        } else if is_cjk(ch) {
            flush_ascii(&mut terms, &mut ascii);
            cjk.push(ch);
        } else {
            flush_ascii(&mut terms, &mut ascii);
            flush_cjk(&mut terms, &mut cjk);
        }
    }
    flush_ascii(&mut terms, &mut ascii);
    flush_cjk(&mut terms, &mut cjk);
    terms.sort();
    terms.dedup();
    terms
}

fn is_cjk(ch: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&ch)
        || ('\u{3400}'..='\u{4dbf}').contains(&ch)
        || ('\u{f900}'..='\u{faff}').contains(&ch)
}

fn term_matches(text: &str, term: &str) -> bool {
    if term.chars().any(is_cjk) {
        text.contains(term)
    } else {
        text.split_whitespace()
            .any(|word| word.trim_matches(|ch: char| !ch.is_alphanumeric()).eq(term))
    }
}
fn score_node(node: &GraphNode, terms: &[String]) -> i64 {
    let title = node.title.to_lowercase();
    let summary = node.summary.clone().unwrap_or_default().to_lowercase();
    let metadata = format!("{} {} {} {}", node.id, node.scope.clone().unwrap_or_default(), node.tags.join(" "), node.visibility).to_lowercase();
    let score = terms.iter().map(|term| {
        // Keep hyphenated titles such as `pre-shutdown` from becoming a false
        // exact hit for `shutdown`, while still matching normal prose words.
        let title_hit = term_matches(&title, term);
        let summary_hit = term_matches(&summary, term);
        if title_hit { 8 } else if summary_hit || metadata.contains(term) { 3 } else { 0 }
    }).sum::<i64>();
    if score == 0 { return 0; }
    score + i64::from(node.status.as_deref() == Some("committed"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn index_revision_is_stable_for_an_unchanged_projection() {
        let store = Store::open_memory().unwrap();
        let now = chrono::Utc::now();
        store.upsert_graph_node(&GraphNode {
            id: "knowledge/stable".into(), node_type: "knowledge".into(), title: "Stable".into(),
            path: "graph/knowledge/stable.md".into(), content_hash: "hash".into(), status: Some("committed".into()),
            summary: Some("A stable node".into()), scope: None, confidence: None, created_at: now, updated_at: now,
            visibility: "personal".into(), tags: vec!["test".into()],
        }).unwrap();
        let first = index_revision(&store).unwrap();
        let second = index_revision(&store).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("sha256:"));
    }

    #[test]
    fn manifest_returns_the_full_visible_inventory_without_question_selection() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("personal/knowledge")).unwrap();
        fs::create_dir_all(dir.path().join("personal/methods")).unwrap();
        fs::create_dir_all(dir.path().join("teams/default/knowledge")).unwrap();
        fs::create_dir_all(dir.path().join("teams/default/methods")).unwrap();
        fs::create_dir_all(dir.path().join("teams/default/experiences")).unwrap();
        fs::write(
            dir.path().join("personal/knowledge/shutdown.md"),
            "---\nid: knowledge/shutdown\ntitle: Shutdown signals\nnode_type: knowledge\nkind: diagnostic-signal\nstatus: committed\nvisibility: personal\nsummary: Signals that distinguish shutdown causes.\ntags: [power]\n---\n\n## Execute\nRead the final shutdown window.\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("personal/methods/triage.md"),
            "---\nid: method/triage\ntitle: Shutdown triage\nnode_type: method\nkind: diagnosis\nstatus: committed\nvisibility: personal\nsummary: A repeatable shutdown investigation.\n---\n\n## Decide\nBranch by the observed signal.\n",
        )
        .unwrap();
        let store = Store::open_memory().unwrap();
        crate::graph::sync_graph(&store, dir.path()).unwrap();

        let manifest = AgentQuery::new(&store, dir.path())
            .manifest(&[])
            .unwrap();

        assert_eq!(manifest.command, "manifest");
        assert_eq!(manifest.selected_team, "default");
        assert_eq!(manifest.items.len(), 2);
        assert!(manifest.items.iter().any(|item| item.id == "knowledge/shutdown"));
        assert!(manifest.items.iter().any(|item| item.id == "method/triage"));
        assert!(manifest
            .graph_roots
            .contains(&dir.path().join("personal/knowledge").display().to_string()));
        assert!(manifest.directory_structure.iter().any(|directory| {
            directory.path == "teams/default/knowledge" && directory.exists
        }));
        let shutdown = manifest
            .items
            .iter()
            .find(|item| item.id == "knowledge/shutdown")
            .unwrap();
        assert_eq!(shutdown.path, "personal/knowledge/shutdown.md");
        assert_eq!(shutdown.absolute_path, dir.path().join(&shutdown.path).display().to_string());
        assert_eq!(shutdown.facets, vec!["Execute"]);
    }
}
