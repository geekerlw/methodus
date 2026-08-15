//! Learning-loop repositories: jobs, knowledge items, questions.

use chrono::{DateTime, Utc};
use rusqlite::params;

use methodus_domain::{
    JobKind, JobStatus, KnowledgeItem, KnowledgeStatus, LearningJob, Question, QuestionStatus,
};

use crate::store::parse_datetime;
use crate::{Store, StoreError};

impl Store {
    // ─── Learning jobs ───────────────────────────────────────────────────

    /// Insert a job. Duplicate `dedupe_key` is ignored (idempotent enqueue).
    pub fn enqueue_job(&self, job: &LearningJob) -> Result<bool, StoreError> {
        let conn = self.lock_conn()?;
        let n = conn.execute(
            "INSERT OR IGNORE INTO learning_jobs
             (id, kind, priority, dedupe_key, input_refs, status, attempts,
              not_before, budget, requires_approval, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                job.id,
                job.kind.to_string(),
                job.priority,
                job.dedupe_key,
                job.input_refs,
                job.status.to_string(),
                job.attempts,
                job.not_before.map(|d| d.to_rfc3339()),
                job.budget,
                if job.requires_approval { 1 } else { 0 },
                job.created_at.to_rfc3339(),
                job.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(n > 0)
    }

    pub fn get_job(&self, id: &str) -> Result<Option<LearningJob>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, priority, dedupe_key, input_refs, status, attempts,
                    not_before, budget, requires_approval, created_at, updated_at
             FROM learning_jobs WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(job_from_query(row)?)),
            None => Ok(None),
        }
    }

    pub fn list_jobs(&self) -> Result<Vec<LearningJob>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, priority, dedupe_key, input_refs, status, attempts,
                    not_before, budget, requires_approval, created_at, updated_at
             FROM learning_jobs ORDER BY created_at DESC",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(job_from_query(row)?);
        }
        Ok(out)
    }

    /// Claim the next due queued job, marking it running and incrementing attempts.
    pub fn claim_next_job(&self, now: DateTime<Utc>) -> Result<Option<LearningJob>, StoreError> {
        let conn = self.lock_conn()?;
        let now_s = now.to_rfc3339();
        let id: Option<String> = conn
            .query_row(
                "SELECT id FROM learning_jobs
                 WHERE status = 'queued'
                   AND (not_before IS NULL OR not_before <= ?1)
                 ORDER BY priority DESC, created_at ASC
                 LIMIT 1",
                params![now_s],
                |row| row.get(0),
            )
            .optional_store()?;
        let Some(id) = id else {
            return Ok(None);
        };
        conn.execute(
            "UPDATE learning_jobs
             SET status = 'running', attempts = attempts + 1, updated_at = ?1
             WHERE id = ?2",
            params![now_s, id],
        )?;
        drop(conn);
        self.get_job(&id)
    }

    pub fn update_job_status(
        &self,
        id: &str,
        status: JobStatus,
        not_before: Option<DateTime<Utc>>,
    ) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE learning_jobs SET status = ?1, not_before = ?2, updated_at = ?3 WHERE id = ?4",
            params![
                status.to_string(),
                not_before.map(|d| d.to_rfc3339()),
                Utc::now().to_rfc3339(),
                id
            ],
        )?;
        Ok(())
    }

    /// Crash recovery: running jobs become queued again.
    pub fn requeue_running_jobs(&self) -> Result<usize, StoreError> {
        let conn = self.lock_conn()?;
        let n = conn.execute(
            "UPDATE learning_jobs SET status = 'queued', updated_at = ?1 WHERE status = 'running'",
            params![Utc::now().to_rfc3339()],
        )?;
        Ok(n)
    }

    // ─── Knowledge ───────────────────────────────────────────────────────

    pub fn insert_knowledge(&self, item: &KnowledgeItem) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO knowledge_items
             (id, face_id, project_id, path, content_hash, source, confidence, scope,
              status, conflict_of, version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                item.id,
                item.face_id,
                item.project_id,
                item.path,
                item.content_hash,
                item.source,
                item.confidence,
                item.scope,
                item.status.to_string(),
                item.conflict_of,
                item.version,
                item.created_at.to_rfc3339(),
                item.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get_knowledge(&self, id: &str) -> Result<Option<KnowledgeItem>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, face_id, project_id, path, content_hash, source, confidence, scope,
                    status, conflict_of, version, created_at, updated_at
             FROM knowledge_items WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(knowledge_from_query(row)?)),
            None => Ok(None),
        }
    }

    pub fn list_knowledge(
        &self,
        status: Option<KnowledgeStatus>,
    ) -> Result<Vec<KnowledgeItem>, StoreError> {
        let conn = self.lock_conn()?;
        let sql = match status {
            Some(_) => {
                "SELECT id, face_id, project_id, path, content_hash, source, confidence, scope,
                        status, conflict_of, version, created_at, updated_at
                 FROM knowledge_items WHERE status = ?1 ORDER BY created_at DESC"
            }
            None => {
                "SELECT id, face_id, project_id, path, content_hash, source, confidence, scope,
                        status, conflict_of, version, created_at, updated_at
                 FROM knowledge_items ORDER BY created_at DESC"
            }
        };
        let mut stmt = conn.prepare(sql)?;
        let mut rows = match &status {
            Some(s) => stmt.query(params![s.to_string()])?,
            None => stmt.query([])?,
        };
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(knowledge_from_query(row)?);
        }
        Ok(out)
    }

    pub fn list_knowledge_by_path(&self, path: &str) -> Result<Vec<KnowledgeItem>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, face_id, project_id, path, content_hash, source, confidence, scope,
                    status, conflict_of, version, created_at, updated_at
             FROM knowledge_items WHERE path = ?1 ORDER BY created_at ASC",
        )?;
        let mut rows = stmt.query(params![path])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(knowledge_from_query(row)?);
        }
        Ok(out)
    }

    pub fn update_knowledge(&self, item: &KnowledgeItem) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE knowledge_items
             SET status = ?1, conflict_of = ?2, content_hash = ?3, version = ?4,
                 updated_at = ?5, path = ?6
             WHERE id = ?7",
            params![
                item.status.to_string(),
                item.conflict_of,
                item.content_hash,
                item.version,
                item.updated_at.to_rfc3339(),
                item.path,
                item.id,
            ],
        )?;
        Ok(())
    }

    // ─── Questions ───────────────────────────────────────────────────────

    pub fn insert_question(&self, q: &Question) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO questions
             (id, question, reason, task_id, face_id, importance, frequency, impact,
              uncertainty, value, status, not_before, answer, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                q.id,
                q.question,
                q.reason,
                q.task_id,
                q.face_id,
                q.importance,
                q.frequency,
                q.impact,
                q.uncertainty,
                q.value,
                q.status.to_string(),
                q.not_before.map(|d| d.to_rfc3339()),
                q.answer,
                q.created_at.to_rfc3339(),
                q.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get_question(&self, id: &str) -> Result<Option<Question>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, question, reason, task_id, face_id, importance, frequency, impact,
                    uncertainty, value, status, not_before, answer, created_at, updated_at
             FROM questions WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(question_from_query(row)?)),
            None => Ok(None),
        }
    }

    pub fn list_questions(
        &self,
        status: Option<QuestionStatus>,
    ) -> Result<Vec<Question>, StoreError> {
        let conn = self.lock_conn()?;
        let sql = match status {
            Some(_) => {
                "SELECT id, question, reason, task_id, face_id, importance, frequency, impact,
                        uncertainty, value, status, not_before, answer, created_at, updated_at
                 FROM questions WHERE status = ?1 ORDER BY value DESC, created_at DESC"
            }
            None => {
                "SELECT id, question, reason, task_id, face_id, importance, frequency, impact,
                        uncertainty, value, status, not_before, answer, created_at, updated_at
                 FROM questions ORDER BY value DESC, created_at DESC"
            }
        };
        let mut stmt = conn.prepare(sql)?;
        let mut rows = match &status {
            Some(s) => stmt.query(params![s.to_string()])?,
            None => stmt.query([])?,
        };
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(question_from_query(row)?);
        }
        Ok(out)
    }

    pub fn find_question_by_text(
        &self,
        question: &str,
        face_id: Option<&str>,
    ) -> Result<Option<Question>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, question, reason, task_id, face_id, importance, frequency, impact,
                    uncertainty, value, status, not_before, answer, created_at, updated_at
             FROM questions WHERE question = ?1 AND IFNULL(face_id, '') = IFNULL(?2, '')
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![question, face_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(question_from_query(row)?)),
            None => Ok(None),
        }
    }

    pub fn update_question(&self, q: &Question) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE questions
             SET reason = ?1, frequency = ?2, value = ?3, status = ?4, not_before = ?5,
                 answer = ?6, task_id = ?7, updated_at = ?8
             WHERE id = ?9",
            params![
                q.reason,
                q.frequency,
                q.value,
                q.status.to_string(),
                q.not_before.map(|d| d.to_rfc3339()),
                q.answer,
                q.task_id,
                q.updated_at.to_rfc3339(),
                q.id,
            ],
        )?;
        Ok(())
    }
}

trait OptionalQuery<T> {
    fn optional_store(self) -> Result<Option<T>, StoreError>;
}

impl<T> OptionalQuery<T> for Result<T, rusqlite::Error> {
    fn optional_store(self) -> Result<Option<T>, StoreError> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StoreError::from(e)),
        }
    }
}

fn parse_opt_datetime(raw: Option<String>) -> Result<Option<DateTime<Utc>>, StoreError> {
    match raw {
        Some(s) if !s.is_empty() => Ok(Some(parse_datetime(&s)?)),
        _ => Ok(None),
    }
}

fn job_from_query(row: &rusqlite::Row<'_>) -> Result<LearningJob, StoreError> {
    let kind: String = row.get(1)?;
    let status: String = row.get(5)?;
    let requires: i64 = row.get(9)?;
    Ok(LearningJob {
        id: row.get(0)?,
        kind: kind
            .parse::<JobKind>()
            .map_err(|e| StoreError::Migration(e.to_string()))?,
        priority: row.get(2)?,
        dedupe_key: row.get(3)?,
        input_refs: row.get(4)?,
        status: status
            .parse::<JobStatus>()
            .map_err(|e| StoreError::Migration(e.to_string()))?,
        attempts: row.get(6)?,
        not_before: parse_opt_datetime(row.get(7)?)?,
        budget: row.get(8)?,
        requires_approval: requires != 0,
        created_at: parse_datetime(&row.get::<_, String>(10)?)?,
        updated_at: parse_datetime(&row.get::<_, String>(11)?)?,
    })
}

fn knowledge_from_query(row: &rusqlite::Row<'_>) -> Result<KnowledgeItem, StoreError> {
    let status: String = row.get(8)?;
    Ok(KnowledgeItem {
        id: row.get(0)?,
        face_id: row.get(1)?,
        project_id: row.get(2)?,
        path: row.get(3)?,
        content_hash: row.get(4)?,
        source: row.get(5)?,
        confidence: row.get(6)?,
        scope: row.get(7)?,
        status: status
            .parse::<KnowledgeStatus>()
            .map_err(|e| StoreError::Migration(e.to_string()))?,
        conflict_of: row.get(9)?,
        version: row.get(10)?,
        created_at: parse_datetime(&row.get::<_, String>(11)?)?,
        updated_at: parse_datetime(&row.get::<_, String>(12)?)?,
    })
}

fn question_from_query(row: &rusqlite::Row<'_>) -> Result<Question, StoreError> {
    let status: String = row.get(10)?;
    Ok(Question {
        id: row.get(0)?,
        question: row.get(1)?,
        reason: row.get(2)?,
        task_id: row.get(3)?,
        face_id: row.get(4)?,
        importance: row.get::<_, Option<f64>>(5)?.unwrap_or(0.6),
        frequency: row.get::<_, Option<f64>>(6)?.unwrap_or(1.0),
        impact: row.get::<_, Option<f64>>(7)?.unwrap_or(0.5),
        uncertainty: row.get::<_, Option<f64>>(8)?.unwrap_or(0.8),
        value: row.get::<_, Option<f64>>(9)?.unwrap_or(0.0),
        status: status
            .parse::<QuestionStatus>()
            .map_err(|e| StoreError::Migration(e.to_string()))?,
        not_before: parse_opt_datetime(row.get(11)?)?,
        answer: row.get(12)?,
        created_at: parse_datetime(&row.get::<_, String>(13)?)?,
        updated_at: parse_datetime(&row.get::<_, String>(14)?)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn job(kind: JobKind, dedupe: &str) -> LearningJob {
        let now = Utc::now();
        LearningJob {
            id: format!("job_{dedupe}"),
            kind,
            priority: 10,
            dedupe_key: Some(dedupe.to_string()),
            input_refs: r#"{"experience_id":"exp_1"}"#.to_string(),
            status: JobStatus::Queued,
            attempts: 0,
            not_before: None,
            budget: Some(r#"{"max_ms":200}"#.to_string()),
            requires_approval: false,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn v5_tables_exist() {
        let store = Store::open_memory().unwrap();
        for table in &[
            "knowledge_items",
            "questions",
            "hypotheses",
            "learning_jobs",
        ] {
            let count: i64 = store
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                        params![table],
                        |row| row.get(0),
                    )
                    .map_err(StoreError::from)
                })
                .unwrap();
            assert_eq!(count, 1, "{table}");
        }
    }

    #[test]
    fn enqueue_is_idempotent_on_dedupe() {
        let store = Store::open_memory().unwrap();
        let a = job(JobKind::ExtractExperience, "extract:exp_1");
        let mut b = job(JobKind::ExtractExperience, "extract:exp_1");
        b.id = "job_other".to_string();
        assert!(store.enqueue_job(&a).unwrap());
        assert!(!store.enqueue_job(&b).unwrap());
        assert_eq!(store.list_jobs().unwrap().len(), 1);
    }

    #[test]
    fn claim_and_requeue() {
        let store = Store::open_memory().unwrap();
        store
            .enqueue_job(&job(JobKind::DetectGaps, "detect:exp_1"))
            .unwrap();
        let claimed = store.claim_next_job(Utc::now()).unwrap().unwrap();
        assert_eq!(claimed.status, JobStatus::Running);
        assert_eq!(claimed.attempts, 1);
        assert!(store.claim_next_job(Utc::now()).unwrap().is_none());
        assert_eq!(store.requeue_running_jobs().unwrap(), 1);
        let again = store.claim_next_job(Utc::now()).unwrap().unwrap();
        assert_eq!(again.id, claimed.id);
        assert_eq!(again.attempts, 2);
    }

    #[test]
    fn knowledge_and_question_crud() {
        let store = Store::open_memory().unwrap();
        let now = Utc::now();
        let item = KnowledgeItem {
            id: "know_1".to_string(),
            face_id: Some("general".to_string()),
            project_id: None,
            path: "faces/general/knowledge/latch.md".to_string(),
            content_hash: "abc".to_string(),
            source: "experience".to_string(),
            confidence: Some(0.4),
            scope: None,
            status: KnowledgeStatus::Candidate,
            conflict_of: None,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        store.insert_knowledge(&item).unwrap();
        assert_eq!(
            store
                .list_knowledge(Some(KnowledgeStatus::Candidate))
                .unwrap()
                .len(),
            1
        );

        let q = Question {
            id: "q_1".to_string(),
            question: "What about the latch?".to_string(),
            reason: Some("repeated unknown".to_string()),
            task_id: Some("t1".to_string()),
            face_id: Some("general".to_string()),
            importance: 0.6,
            frequency: 2.0,
            impact: 0.5,
            uncertainty: 0.8,
            value: 0.48,
            status: QuestionStatus::Pending,
            not_before: None,
            answer: None,
            created_at: now,
            updated_at: now,
        };
        store.insert_question(&q).unwrap();
        let found = store
            .find_question_by_text("What about the latch?", Some("general"))
            .unwrap()
            .unwrap();
        assert_eq!(found.id, "q_1");
    }
}
