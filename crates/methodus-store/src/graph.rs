//! SQLite index operations for Markdown-first graph files and task capsules.

use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use methodus_domain::{ContextSelection, GraphEdge, GraphNode, TaskWorkspace};

use crate::{Store, StoreError};

impl Store {
    pub fn upsert_graph_node(&self, node: &GraphNode) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO graph_nodes
             (id,node_type,title,path,content_hash,status,summary,scope,confidence,created_at,updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
             ON CONFLICT(id) DO UPDATE SET
             node_type=excluded.node_type,title=excluded.title,path=excluded.path,
             content_hash=excluded.content_hash,status=excluded.status,summary=excluded.summary,
             scope=excluded.scope,confidence=excluded.confidence,updated_at=excluded.updated_at",
            params![
                node.id, node.node_type, node.title, node.path, node.content_hash,
                node.status, node.summary, node.scope, node.confidence,
                node.created_at.to_rfc3339(), node.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn replace_graph_edges(&self, from_id: &str, edges: &[GraphEdge]) -> Result<(), StoreError> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM graph_edges WHERE from_id = ?1", [from_id])?;
        for edge in edges {
            tx.execute(
                "INSERT INTO graph_edges
                 (id,from_id,relation,to_id,source,confidence,evidence_refs,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    edge.id, edge.from_id, edge.relation, edge.to_id, edge.source,
                    edge.confidence, serde_json::to_string(&edge.evidence_refs).unwrap_or_else(|_| "[]".into()),
                    edge.created_at.to_rfc3339(), edge.updated_at.to_rfc3339(),
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_graph_nodes(&self, query: Option<&str>) -> Result<Vec<GraphNode>, StoreError> {
        let conn = self.lock_conn()?;
        let pattern = query.map(|q| format!("%{}%", q.trim().to_lowercase()));
        let mut stmt = if pattern.is_some() {
            conn.prepare("SELECT id,node_type,title,path,content_hash,status,summary,scope,confidence,created_at,updated_at
                          FROM graph_nodes WHERE lower(title) LIKE ?1 OR lower(coalesce(summary,'')) LIKE ?1
                          ORDER BY updated_at DESC, title COLLATE NOCASE")?
        } else {
            conn.prepare("SELECT id,node_type,title,path,content_hash,status,summary,scope,confidence,created_at,updated_at
                          FROM graph_nodes ORDER BY updated_at DESC, title COLLATE NOCASE")?
        };
        let rows = if let Some(pattern) = pattern {
            stmt.query_map([pattern], graph_node_from_row)?
        } else {
            stmt.query_map([], graph_node_from_row)?
        };
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
    }

    pub fn graph_node(&self, id: &str) -> Result<Option<GraphNode>, StoreError> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT id,node_type,title,path,content_hash,status,summary,scope,confidence,created_at,updated_at
             FROM graph_nodes WHERE id = ?1",
            [id],
            graph_node_from_row,
        ).optional().map_err(StoreError::from)
    }

    pub fn graph_edges_for(&self, node_id: &str) -> Result<Vec<GraphEdge>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id,from_id,relation,to_id,source,confidence,evidence_refs,created_at,updated_at
             FROM graph_edges WHERE from_id = ?1 OR to_id = ?1 ORDER BY relation, from_id, to_id",
        )?;
        let rows = stmt.query_map([node_id], graph_edge_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
    }

    pub fn insert_task_workspace(&self, workspace: &TaskWorkspace) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO workspaces
             (id,task_id,root_path,status,created_at,updated_at,launch_cwd,manifest_hash,context_budget_tokens)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(id) DO UPDATE SET root_path=excluded.root_path,status=excluded.status,
             updated_at=excluded.updated_at,launch_cwd=excluded.launch_cwd,
             manifest_hash=excluded.manifest_hash,context_budget_tokens=excluded.context_budget_tokens",
            params![workspace.id, workspace.task_id, workspace.root_path, workspace.status,
                workspace.created_at.to_rfc3339(), workspace.updated_at.to_rfc3339(),
                workspace.launch_cwd, workspace.manifest_hash, workspace.context_budget_tokens],
        )?;
        Ok(())
    }

    pub fn replace_context_selections(&self, workspace_id: &str, items: &[ContextSelection]) -> Result<(), StoreError> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM context_selections WHERE workspace_id = ?1", [workspace_id])?;
        for item in items {
            tx.execute(
                "INSERT INTO context_selections
                 (id,workspace_id,node_id,facet,rationale,priority,estimated_tokens,disposition,outcome,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![item.id, item.workspace_id, item.node_id, item.facet, item.rationale,
                    item.priority, item.estimated_tokens, item.disposition, item.outcome,
                    item.created_at.to_rfc3339(), item.updated_at.to_rfc3339()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_context_outcome(&self, id: &str, outcome: &str) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE context_selections SET outcome = ?1, updated_at = ?2 WHERE id = ?3",
            params![outcome, Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    pub fn list_context_selections(&self, workspace_id: &str) -> Result<Vec<ContextSelection>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id,workspace_id,node_id,facet,rationale,priority,estimated_tokens,disposition,outcome,created_at,updated_at
             FROM context_selections WHERE workspace_id = ?1 ORDER BY priority DESC, estimated_tokens ASC",
        )?;
        let rows = stmt.query_map([workspace_id], context_selection_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
    }

    pub fn record_launch(&self, task_id: &str, runtime: &str, mode: &str, command_summary: &str) -> Result<String, StoreError> {
        let id = format!("launch_{}", Uuid::new_v4());
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO launches (id,task_id,runtime,mode,command_summary,started_at)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![id, task_id, runtime, mode, command_summary, Utc::now().to_rfc3339()],
        )?;
        Ok(id)
    }

    pub fn complete_launch(&self, id: &str, exit_status: &str) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE launches SET returned_at = ?1, exit_status = ?2 WHERE id = ?3",
            params![Utc::now().to_rfc3339(), exit_status, id],
        )?;
        Ok(())
    }
}

fn graph_node_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphNode> {
    Ok(GraphNode {
        id: row.get(0)?, node_type: row.get(1)?, title: row.get(2)?, path: row.get(3)?,
        content_hash: row.get(4)?, status: row.get(5)?, summary: row.get(6)?,
        scope: row.get(7)?, confidence: row.get(8)?,
        created_at: parse_time(row.get(9)?)?, updated_at: parse_time(row.get(10)?)?,
    })
}

fn graph_edge_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphEdge> {
    let refs: String = row.get(6)?;
    Ok(GraphEdge {
        id: row.get(0)?, from_id: row.get(1)?, relation: row.get(2)?, to_id: row.get(3)?,
        source: row.get(4)?, confidence: row.get(5)?,
        evidence_refs: serde_json::from_str(&refs).unwrap_or_default(),
        created_at: parse_time(row.get(7)?)?, updated_at: parse_time(row.get(8)?)?,
    })
}

fn context_selection_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContextSelection> {
    Ok(ContextSelection {
        id: row.get(0)?, workspace_id: row.get(1)?, node_id: row.get(2)?, facet: row.get(3)?,
        rationale: row.get(4)?, priority: row.get(5)?, estimated_tokens: row.get(6)?,
        disposition: row.get(7)?, outcome: row.get(8)?,
        created_at: parse_time(row.get(9)?)?, updated_at: parse_time(row.get(10)?)?,
    })
}

fn parse_time(raw: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&raw)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|err| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err)))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use methodus_domain::{ContextSelection, GraphEdge, GraphNode, TaskWorkspace};

    use crate::Store;

    #[test]
    fn graph_index_and_context_selection_round_trip() {
        let store = Store::open_memory().unwrap();
        let now = Utc::now();
        store.upsert_graph_node(&GraphNode {
            id: "knowledge/idempotency".into(), node_type: "knowledge".into(), title: "Idempotency".into(),
            path: "graph/knowledge/idempotency.md".into(), content_hash: "hash".into(), status: Some("committed".into()),
            summary: Some("Prevent duplicate effects".into()), scope: None, confidence: Some(0.9), created_at: now, updated_at: now,
        }).unwrap();
        store.replace_graph_edges("knowledge/idempotency", &[GraphEdge {
            id: "edge_1".into(), from_id: "knowledge/idempotency".into(), relation: "requires".into(), to_id: "knowledge/unique-key".into(),
            source: "authored".into(), confidence: None, evidence_refs: vec![], created_at: now, updated_at: now,
        }]).unwrap();
        store.insert_task_workspace(&TaskWorkspace {
            id: "ws_task".into(), task_id: "task".into(), root_path: "/tmp/ws".into(), launch_cwd: "/tmp/project".into(),
            status: "compiled".into(), manifest_hash: "manifest".into(), context_budget_tokens: 1000, created_at: now, updated_at: now,
        }).unwrap();
        store.replace_context_selections("ws_task", &[ContextSelection {
            id: "ctx_1".into(), workspace_id: "ws_task".into(), node_id: "knowledge/idempotency".into(), facet: "Execute".into(),
            rationale: "exact match".into(), priority: Some(1.0), estimated_tokens: 10, disposition: "injected".into(), outcome: None, created_at: now, updated_at: now,
        }]).unwrap();
        store.set_context_outcome("ctx_1", "useful").unwrap();
        assert_eq!(store.graph_edges_for("knowledge/idempotency").unwrap().len(), 1);
        assert_eq!(store.list_context_selections("ws_task").unwrap()[0].outcome.as_deref(), Some("useful"));
    }
}
