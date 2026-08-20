//! Read-only Agent-facing retrieval. This module is deliberately independent from
//! task/workspace/session orchestration: runtimes call it through `methodus agent`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use methodus_domain::GraphNode;
use methodus_store::Store;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::CoreError;
use crate::graph::{estimated_tokens, facet, read_graph_document, GraphDocument, SourceEvidence};

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
pub struct AgentResponse {
    pub protocol_version: u32,
    pub command: String,
    pub goal: String,
    pub index_revision: String,
    pub estimated_tokens: i64,
    pub budget_tokens: Option<i64>,
    pub items: Vec<AgentItem>,
    pub lazy_ids: Vec<String>,
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

    pub fn prepare(&self, goal: &str, budget_tokens: i64, scopes: &[String]) -> Result<AgentResponse, CoreError> {
        let terms = terms(goal);
        let revision = self.index_revision()?;
        let mut candidates = self.visible_nodes(scopes)?.into_iter().filter_map(|node| {
            let score = score_node(&node, &terms);
            (score > 0).then_some((score, node))
        }).collect::<Vec<_>>();
        candidates.sort_by(|(left_score, left), (right_score, right)| {
            type_rank(&left.node_type).cmp(&type_rank(&right.node_type))
                .then_with(|| right_score.cmp(left_score))
                .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
                .then_with(|| left.id.cmp(&right.id))
        });

        let mut items = Vec::new();
        let mut warnings = Vec::new();
        let mut used = 0_i64;
        let mut truncated = false;
        for (score, node) in candidates.iter().take(24) {
            let facet_name = preferred_facet(node);
            let mut content = match self.node_content(node, facet_name) {
                Ok(content) => content,
                Err(error) => {
                    warnings.push(format!("skipped unreadable node {}: {}", node.id, error));
                    continue;
                }
            };
            let mut estimate = estimated_tokens(&content);
            if items.is_empty() && estimate > budget_tokens.max(1) {
                let max_chars = budget_tokens.max(1).saturating_mul(4) as usize;
                content = content.chars().take(max_chars).collect();
                estimate = estimated_tokens(&content);
                truncated = true;
            }
            if !items.is_empty() && used + estimate > budget_tokens.max(1) { continue; }
            match self.item(node, facet_name, content, format!("matched task terms with score {score}")) {
                Ok(item) => {
                    used += estimate;
                    items.push(item);
                }
                Err(error) => warnings.push(format!("skipped unreadable node {}: {}", node.id, error)),
            }
            if items.len() >= 8 || used >= budget_tokens.max(1) { break; }
        }

        let selected = items.iter().map(|item| item.id.as_str()).collect::<HashSet<_>>();
        let mut lazy_ids = Vec::new();
        for item in &items {
            for edge in self.store.graph_edges_for(&item.id)? {
                let related = if edge.from_id == item.id { edge.to_id } else { edge.from_id };
                if !selected.contains(related.as_str()) && !lazy_ids.contains(&related) {
                    if let Some(node) = self.store.graph_node(&related)? {
                        let status = node.status.as_deref().unwrap_or("committed");
                        if matches!(status, "committed" | "stale") { lazy_ids.push(related); }
                    }
                }
            }
        }
        lazy_ids.truncate(12);
        if items.is_empty() { warnings.push("no committed or strongly relevant stale nodes matched the goal".into()); }
        if truncated { warnings.push("the first selected facet was truncated to fit the requested token budget".into()); }
        if self.store.list_graph_nodes(None)?.iter().any(|node| node.status.as_deref() == Some("stale")) {
            warnings.push("the graph contains stale nodes; inspect warnings before applying historical rules".into());
        }
        Ok(AgentResponse {
            protocol_version: AGENT_PROTOCOL_VERSION,
            command: "prepare".into(),
            goal: goal.into(),
            index_revision: revision,
            estimated_tokens: used,
            budget_tokens: Some(budget_tokens.max(1)),
            items, lazy_ids, warnings,
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
fn terms(input: &str) -> Vec<String> { input.split_whitespace().filter_map(|term| { let normalized = term.trim_matches(|ch: char| !ch.is_alphanumeric() && !('\u{4e00}'..='\u{9fff}').contains(&ch)).to_lowercase(); (!normalized.is_empty()).then_some(normalized) }).collect() }
fn score_node(node: &GraphNode, terms: &[String]) -> i64 {
    let title = node.title.to_lowercase();
    let summary = node.summary.clone().unwrap_or_default().to_lowercase();
    let metadata = format!("{} {} {} {}", node.id, node.scope.clone().unwrap_or_default(), node.tags.join(" "), node.visibility).to_lowercase();
    let score = terms.iter().map(|term| {
        // Keep hyphenated titles such as `pre-shutdown` from becoming a false
        // exact hit for `shutdown`, while still matching normal prose words.
        let title_hit = title.split_whitespace().any(|word| word.trim_matches(|ch: char| !ch.is_alphanumeric()).eq(term));
        let summary_hit = summary.split_whitespace().any(|word| word.trim_matches(|ch: char| !ch.is_alphanumeric()).eq(term));
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
    fn prepare_returns_bounded_execute_context_and_lazy_relations() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("graph/knowledge")).unwrap();
        fs::write(dir.path().join("graph/knowledge/shutdown.md"), "---\nid: knowledge/shutdown\ntitle: Previous shutdown reason\nnode_type: knowledge\nkind: diagnostic-signal\nstatus: committed\nvisibility: personal\nsummary: Read the previous shutdown reason before investigating crashes.\nlinks:\n  next_step: [knowledge/crash]\n---\n\n## Execute\nRead the previous shutdown reason and branch by controlled exit, watchdog, crash, or power loss.\n").unwrap();
        fs::write(dir.path().join("graph/knowledge/crash.md"), "---\nid: knowledge/crash\ntitle: Pre-shutdown crash detection\nnode_type: knowledge\nkind: diagnostic-signal\nstatus: committed\nvisibility: personal\nsummary: Inspect the final log window for crash evidence.\n---\n\n## Execute\nInspect the final log window.\n").unwrap();
        let store = Store::open_memory().unwrap();
        crate::graph::sync_graph(&store, dir.path()).unwrap();
        let response = AgentQuery::new(&store, dir.path()).prepare("abnormal shutdown reason", 200, &[]).unwrap();
        assert_eq!(response.items[0].id, "knowledge/shutdown");
        assert!(response.lazy_ids.contains(&"knowledge/crash".to_string()));
        assert!(response.estimated_tokens <= 200);
    }

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
    fn prepare_excludes_indexed_documents_that_fail_consumer_validation() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("graph/knowledge")).unwrap();
        fs::write(dir.path().join("graph/knowledge/incomplete.md"), "---\nid: knowledge/incomplete\ntitle: Incomplete\nnode_type: knowledge\nstatus: committed\nsummary: Missing kind and visibility\n---\n\n## Execute\nDo not expose this.\n").unwrap();
        let store = Store::open_memory().unwrap();
        crate::graph::sync_graph(&store, dir.path()).unwrap();
        let response = AgentQuery::new(&store, dir.path()).prepare("incomplete", 200, &[]).unwrap();
        assert!(response.items.is_empty());
        assert!(response.warnings.iter().any(|warning| warning.contains("no committed")));
    }
}
