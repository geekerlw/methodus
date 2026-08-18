//! SQLite catalog index for faces, methods, and skills (03-data-model §3).

use rusqlite::params;

use crate::{Store, StoreError};

impl Store {
    pub fn upsert_face_catalog(
        &self,
        id: &str,
        name: &str,
        path: &str,
        content_hash: &str,
        intent_tags_json: &str,
        updated_at: &str,
    ) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO faces (id, name, path, content_hash, intent_tags, version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               path = excluded.path,
               content_hash = excluded.content_hash,
               intent_tags = excluded.intent_tags,
               version = faces.version + 1,
               updated_at = excluded.updated_at",
            params![id, name, path, content_hash, intent_tags_json, updated_at],
        )?;
        Ok(())
    }

    pub fn upsert_method_catalog(
        &self,
        id: &str,
        name: &str,
        path: &str,
        content_hash: &str,
        intent_tags_json: &str,
        version: &str,
        updated_at: &str,
    ) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO methods (id, name, path, content_hash, intent_tags, version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               path = excluded.path,
               content_hash = excluded.content_hash,
               intent_tags = excluded.intent_tags,
               version = excluded.version,
               updated_at = excluded.updated_at",
            params![
                id,
                name,
                path,
                content_hash,
                intent_tags_json,
                version,
                updated_at
            ],
        )?;
        Ok(())
    }

    pub fn upsert_skill_catalog(
        &self,
        id: &str,
        source: &str,
        path: &str,
        content_hash: &str,
        version: Option<&str>,
        updated_at: &str,
    ) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO skills (id, source, path, content_hash, version, compat, conflict, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6, ?6)
             ON CONFLICT(id) DO UPDATE SET
               source = excluded.source,
               path = excluded.path,
               content_hash = excluded.content_hash,
               version = excluded.version,
               updated_at = excluded.updated_at",
            params![id, source, path, content_hash, version, updated_at],
        )?;
        Ok(())
    }

    pub fn cancel_learning_job(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.lock_conn()?;
        let n = conn.execute(
            "UPDATE learning_jobs SET status = 'cancelled', updated_at = datetime('now')
             WHERE id = ?1 AND status IN ('queued', 'running')",
            params![id],
        )?;
        Ok(n > 0)
    }
}
