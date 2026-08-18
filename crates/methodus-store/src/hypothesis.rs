use methodus_domain::{Hypothesis, HypothesisStatus};
use rusqlite::params;

use crate::store::parse_datetime;
use crate::Store;
use crate::StoreError;

impl Store {
    pub fn insert_hypothesis(&self, item: &Hypothesis) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO hypotheses
             (id, face_id, path, content_hash, confidence, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                item.id,
                item.face_id,
                item.path,
                item.content_hash,
                item.confidence,
                item.status.to_string(),
                item.created_at.to_rfc3339(),
                item.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get_hypothesis(&self, id: &str) -> Result<Option<Hypothesis>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, face_id, path, content_hash, confidence, status, created_at, updated_at
             FROM hypotheses WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(hypothesis_from_query(row)?)),
            None => Ok(None),
        }
    }

    pub fn list_hypotheses(
        &self,
        status: Option<HypothesisStatus>,
    ) -> Result<Vec<Hypothesis>, StoreError> {
        let conn = self.lock_conn()?;
        let sql = match status {
            Some(_) => {
                "SELECT id, face_id, path, content_hash, confidence, status, created_at, updated_at
                 FROM hypotheses WHERE status = ?1 ORDER BY created_at DESC"
            }
            None => {
                "SELECT id, face_id, path, content_hash, confidence, status, created_at, updated_at
                 FROM hypotheses ORDER BY created_at DESC"
            }
        };
        let mut stmt = conn.prepare(sql)?;
        let mut rows = match &status {
            Some(s) => stmt.query(params![s.to_string()])?,
            None => stmt.query([])?,
        };
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(hypothesis_from_query(row)?);
        }
        Ok(out)
    }

    pub fn update_hypothesis(&self, item: &Hypothesis) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE hypotheses SET status = ?1, content_hash = ?2, confidence = ?3, updated_at = ?4
             WHERE id = ?5",
            params![
                item.status.to_string(),
                item.content_hash,
                item.confidence,
                item.updated_at.to_rfc3339(),
                item.id,
            ],
        )?;
        Ok(())
    }

    pub fn find_hypothesis_by_path(&self, path: &str) -> Result<Option<Hypothesis>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, face_id, path, content_hash, confidence, status, created_at, updated_at
             FROM hypotheses WHERE path = ?1 ORDER BY created_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![path])?;
        match rows.next()? {
            Some(row) => Ok(Some(hypothesis_from_query(row)?)),
            None => Ok(None),
        }
    }

    pub fn evolution_at_milestone(
        &self,
        target_kind: &str,
        target_id: &str,
        milestone: i64,
    ) -> Result<bool, StoreError> {
        let conn = self.lock_conn()?;
        let needle = format!("milestone:{milestone}");
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM evolution_candidates
             WHERE target_kind = ?1 AND target_id = ?2 AND source LIKE ?3
               AND status IN ('candidate', 'active', 'approved')",
            params![target_kind, target_id, format!("%{needle}%")],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }
}

fn hypothesis_from_query(row: &rusqlite::Row<'_>) -> Result<Hypothesis, StoreError> {
    let status: String = row.get(5)?;
    Ok(Hypothesis {
        id: row.get(0)?,
        face_id: row.get(1)?,
        path: row.get(2)?,
        content_hash: row.get(3)?,
        confidence: row.get(4)?,
        status: status
            .parse::<HypothesisStatus>()
            .map_err(|e| StoreError::Migration(e.to_string()))?,
        created_at: parse_datetime(&row.get::<_, String>(6)?)?,
        updated_at: parse_datetime(&row.get::<_, String>(7)?)?,
    })
}
