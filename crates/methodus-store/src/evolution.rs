use methodus_domain::{EvolutionCandidate, EvolutionStatus, KnowledgeStatus};
use rusqlite::params;

use crate::learning::knowledge_from_query;
use crate::store::parse_datetime;
use crate::Store;
use crate::StoreError;

impl Store {
    pub fn insert_evolution(&self, item: &EvolutionCandidate) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO evolution_candidates
             (id, target_kind, target_id, diff, rationale, source, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                item.id,
                item.target_kind,
                item.target_id,
                item.diff,
                item.rationale,
                item.source,
                item.status.to_string(),
                item.created_at.to_rfc3339(),
                item.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get_evolution(&self, id: &str) -> Result<Option<EvolutionCandidate>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, target_kind, target_id, diff, rationale, source, status,
                    created_at, updated_at
             FROM evolution_candidates WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(evolution_from_query(row)?)),
            None => Ok(None),
        }
    }

    pub fn list_evolution(
        &self,
        status: Option<EvolutionStatus>,
    ) -> Result<Vec<EvolutionCandidate>, StoreError> {
        let conn = self.lock_conn()?;
        let sql = match status {
            Some(_) => {
                "SELECT id, target_kind, target_id, diff, rationale, source, status,
                        created_at, updated_at
                 FROM evolution_candidates WHERE status = ?1 ORDER BY created_at DESC"
            }
            None => {
                "SELECT id, target_kind, target_id, diff, rationale, source, status,
                        created_at, updated_at
                 FROM evolution_candidates ORDER BY created_at DESC"
            }
        };
        let mut stmt = conn.prepare(sql)?;
        let mut rows = match &status {
            Some(s) => stmt.query(params![s.to_string()])?,
            None => stmt.query([])?,
        };
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(evolution_from_query(row)?);
        }
        Ok(out)
    }

    pub fn update_evolution(&self, item: &EvolutionCandidate) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE evolution_candidates
             SET status = ?1, updated_at = ?2
             WHERE id = ?3",
            params![
                item.status.to_string(),
                item.updated_at.to_rfc3339(),
                item.id,
            ],
        )?;
        Ok(())
    }

    pub fn has_pending_evolution(&self, target_kind: &str, target_id: &str) -> Result<bool, StoreError> {
        let conn = self.lock_conn()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM evolution_candidates
             WHERE target_kind = ?1 AND target_id = ?2 AND status = 'candidate'",
            params![target_kind, target_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn count_committed_knowledge(
        &self,
        face_id: &str,
        source: &str,
    ) -> Result<i64, StoreError> {
        let conn = self.lock_conn()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM knowledge_items
             WHERE face_id = ?1 AND source = ?2 AND status = ?3",
            params![face_id, source, KnowledgeStatus::Committed.to_string()],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn list_committed_knowledge_for_face(
        &self,
        face_id: &str,
        source: &str,
    ) -> Result<Vec<methodus_domain::KnowledgeItem>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, face_id, project_id, path, content_hash, source, confidence, scope,
                    status, conflict_of, version, created_at, updated_at
             FROM knowledge_items
             WHERE face_id = ?1 AND source = ?2 AND status = ?3
             ORDER BY created_at ASC",
        )?;
        let mut rows = stmt.query(params![
            face_id,
            source,
            KnowledgeStatus::Committed.to_string()
        ])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(knowledge_from_query(row)?);
        }
        Ok(out)
    }
}

fn evolution_from_query(row: &rusqlite::Row<'_>) -> Result<EvolutionCandidate, StoreError> {
    let status: String = row.get(6)?;
    Ok(EvolutionCandidate {
        id: row.get(0)?,
        target_kind: row.get(1)?,
        target_id: row.get(2)?,
        diff: row.get(3)?,
        rationale: row.get(4)?,
        source: row.get(5)?,
        status: status
            .parse::<EvolutionStatus>()
            .map_err(|e| StoreError::Migration(e.to_string()))?,
        created_at: parse_datetime(&row.get::<_, String>(7)?)?,
        updated_at: parse_datetime(&row.get::<_, String>(8)?)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::tempdir;

    #[test]
    fn evolution_crud_and_pending_check() {
        let dir = tempdir().unwrap();
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        let now = Utc::now();
        let item = EvolutionCandidate {
            id: "evo_test1".into(),
            target_kind: "face".into(),
            target_id: "nxm".into(),
            diff: r#"{"add_intent_tags":["nxm"]}"#.into(),
            rationale: Some("study milestone".into()),
            source: Some("module_study".into()),
            status: EvolutionStatus::Candidate,
            created_at: now,
            updated_at: now,
        };
        store.insert_evolution(&item).unwrap();
        assert!(store.has_pending_evolution("face", "nxm").unwrap());
        let listed = store
            .list_evolution(Some(EvolutionStatus::Candidate))
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(store.get_evolution("evo_test1").unwrap().unwrap().target_id, "nxm");
    }
}
