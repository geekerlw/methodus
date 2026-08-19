use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use methodus_domain::{Approval, Experience, Session, SessionStatus, Task, TaskStatus};

use crate::migration::run_migrations;
use crate::StoreError;

/// The main database handle for Methodus state storage.
///
/// The inner `Connection` is wrapped in a `Mutex` so that `Arc<Store>` is `Send + Sync`,
/// which is required for sharing across async tasks.
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub(crate) fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StoreError> {
        self.conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))
    }
    /// Open (or create) a SQLite database at the given path, enable WAL mode,
    /// and run all pending migrations.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "wal")?;
        run_migrations(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Open an in-memory SQLite database. Useful for testing.
    pub fn open_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        run_migrations(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Acquires the lock and provides access to the underlying connection via a closure.
    /// Useful for tests or advanced queries.
    pub fn with_conn<F, R>(&self, f: F) -> Result<R, StoreError>
    where
        F: FnOnce(&Connection) -> Result<R, StoreError>,
    {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        f(&conn)
    }

    // ─── Task CRUD ───────────────────────────────────────────────────────

    pub fn insert_task(&self, task: &Task) -> Result<(), StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        conn.execute(
            "INSERT INTO tasks (id, title, request, project_id, status, runtime, workspace_id, resolution, version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                task.id,
                task.title,
                task.request,
                task.project_id,
                task.status.to_string(),
                task.runtime,
                task.workspace_id,
                task.resolution,
                task.version,
                task.created_at.to_rfc3339(),
                task.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get_task(&self, id: &str) -> Result<Option<Task>, StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let mut stmt = conn.prepare(
            "SELECT id, title, request, project_id, status, runtime, workspace_id, resolution, version, created_at, updated_at
             FROM tasks WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(TaskRow {
                id: row.get(0)?,
                title: row.get(1)?,
                request: row.get(2)?,
                project_id: row.get(3)?,
                status: row.get(4)?,
                runtime: row.get(5)?,
                workspace_id: row.get(6)?,
                resolution: row.get(7)?,
                version: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })?;

        match rows.next() {
            Some(row) => {
                let r = row?;
                Ok(Some(task_from_row(r)?))
            }
            None => Ok(None),
        }
    }

    pub fn list_tasks(&self) -> Result<Vec<Task>, StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let mut stmt = conn.prepare(
            "SELECT id, title, request, project_id, status, runtime, workspace_id, resolution, version, created_at, updated_at
             FROM tasks ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(TaskRow {
                id: row.get(0)?,
                title: row.get(1)?,
                request: row.get(2)?,
                project_id: row.get(3)?,
                status: row.get(4)?,
                runtime: row.get(5)?,
                workspace_id: row.get(6)?,
                resolution: row.get(7)?,
                version: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })?;

        let mut tasks = Vec::new();
        for row in rows {
            let r = row?;
            tasks.push(task_from_row(r)?);
        }
        Ok(tasks)
    }

    pub fn update_task_status(&self, id: &str, status: TaskStatus) -> Result<(), StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE tasks SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.to_string(), now, id],
        )?;
        Ok(())
    }

    pub fn update_task_workspace(&self, id: &str, workspace_id: &str) -> Result<(), StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE tasks SET workspace_id = ?1, updated_at = ?2 WHERE id = ?3",
            params![workspace_id, now, id],
        )?;
        Ok(())
    }

    // ─── Session CRUD ────────────────────────────────────────────────────

    pub fn insert_session(&self, session: &Session) -> Result<(), StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        conn.execute(
            "INSERT INTO sessions (id, task_id, runtime, executor_sid, transport, pid, cwd, status, last_turn, started_at, ended_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                session.id,
                session.task_id,
                session.runtime,
                session.executor_sid,
                session.transport,
                session.pid.map(|p| p as i64),
                session.cwd,
                session.status.to_string(),
                session.last_turn,
                session.started_at.to_rfc3339(),
                session.ended_at.map(|d| d.to_rfc3339()),
                session.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>, StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let mut stmt = conn.prepare(
            "SELECT id, task_id, runtime, executor_sid, transport, pid, cwd, status, last_turn, started_at, ended_at, updated_at
             FROM sessions WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                task_id: row.get(1)?,
                runtime: row.get(2)?,
                executor_sid: row.get(3)?,
                transport: row.get(4)?,
                pid: row.get(5)?,
                cwd: row.get(6)?,
                status: row.get(7)?,
                last_turn: row.get(8)?,
                started_at: row.get(9)?,
                ended_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })?;

        match rows.next() {
            Some(row) => {
                let r = row?;
                Ok(Some(session_from_row(r)?))
            }
            None => Ok(None),
        }
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>, StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let mut stmt = conn.prepare(
            "SELECT id, task_id, runtime, executor_sid, transport, pid, cwd, status, last_turn, started_at, ended_at, updated_at
             FROM sessions ORDER BY started_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                task_id: row.get(1)?,
                runtime: row.get(2)?,
                executor_sid: row.get(3)?,
                transport: row.get(4)?,
                pid: row.get(5)?,
                cwd: row.get(6)?,
                status: row.get(7)?,
                last_turn: row.get(8)?,
                started_at: row.get(9)?,
                ended_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })?;

        let mut sessions = Vec::new();
        for row in rows {
            let r = row?;
            sessions.push(session_from_row(r)?);
        }
        Ok(sessions)
    }

    pub fn list_sessions_for_task(&self, task_id: &str) -> Result<Vec<Session>, StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let mut stmt = conn.prepare(
            "SELECT id, task_id, runtime, executor_sid, transport, pid, cwd, status, last_turn, started_at, ended_at, updated_at
             FROM sessions WHERE task_id = ?1 ORDER BY started_at DESC",
        )?;
        let rows = stmt.query_map(params![task_id], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                task_id: row.get(1)?,
                runtime: row.get(2)?,
                executor_sid: row.get(3)?,
                transport: row.get(4)?,
                pid: row.get(5)?,
                cwd: row.get(6)?,
                status: row.get(7)?,
                last_turn: row.get(8)?,
                started_at: row.get(9)?,
                ended_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })?;

        let mut sessions = Vec::new();
        for row in rows {
            let r = row?;
            sessions.push(session_from_row(r)?);
        }
        Ok(sessions)
    }

    pub fn list_non_terminal_sessions(&self) -> Result<Vec<Session>, StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let mut stmt = conn.prepare(
            "SELECT id, task_id, runtime, executor_sid, transport, pid, cwd, status, last_turn, started_at, ended_at, updated_at
             FROM sessions WHERE status IN ('spawning', 'running', 'waiting_user', 'paused')
             ORDER BY started_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                task_id: row.get(1)?,
                runtime: row.get(2)?,
                executor_sid: row.get(3)?,
                transport: row.get(4)?,
                pid: row.get(5)?,
                cwd: row.get(6)?,
                status: row.get(7)?,
                last_turn: row.get(8)?,
                started_at: row.get(9)?,
                ended_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })?;

        let mut sessions = Vec::new();
        for row in rows {
            let r = row?;
            sessions.push(session_from_row(r)?);
        }
        Ok(sessions)
    }

    pub fn update_session_status(&self, id: &str, status: SessionStatus) -> Result<(), StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let now = Utc::now().to_rfc3339();
        if status.is_terminal() {
            conn.execute(
                "UPDATE sessions SET status = ?1, ended_at = COALESCE(ended_at, ?2), updated_at = ?2 WHERE id = ?3",
                params![status.to_string(), now, id],
            )?;
        } else {
            conn.execute(
                "UPDATE sessions SET status = ?1, updated_at = ?2 WHERE id = ?3",
                params![status.to_string(), now, id],
            )?;
        }
        Ok(())
    }

    pub fn set_session_pid(&self, id: &str, pid: Option<u32>) -> Result<(), StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE sessions SET pid = ?1, updated_at = ?2 WHERE id = ?3",
            params![pid.map(|p| p as i64), now, id],
        )?;
        Ok(())
    }

    pub fn set_executor_sid(&self, id: &str, executor_sid: &str) -> Result<(), StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE sessions SET executor_sid = ?1, updated_at = ?2 WHERE id = ?3",
            params![executor_sid, now, id],
        )?;
        Ok(())
    }

    // ─── Event CRUD ──────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn insert_event(
        &self,
        id: &str,
        event_type: &str,
        occurred_at: &str,
        task_id: Option<&str>,
        session_id: Option<&str>,
        payload: &str,
        seq: Option<i64>,
    ) -> Result<(), StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        conn.execute(
            "INSERT OR IGNORE INTO events (id, type, occurred_at, task_id, session_id, payload, seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                event_type,
                occurred_at,
                task_id,
                session_id,
                payload,
                seq
            ],
        )?;
        Ok(())
    }

    pub fn list_events(
        &self,
        task_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EventRecord>, StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let limit_i = limit as i64;
        let mut rows_out = Vec::new();
        if let Some(tid) = task_id {
            let mut stmt = conn.prepare(
                "SELECT id, type, occurred_at, task_id, session_id, payload, seq
                 FROM events WHERE task_id = ?1
                 ORDER BY occurred_at DESC, id DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![tid, limit_i], event_record_from_row)?;
            for row in rows {
                rows_out.push(row?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, type, occurred_at, task_id, session_id, payload, seq
                 FROM events ORDER BY occurred_at DESC, id DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit_i], event_record_from_row)?;
            for row in rows {
                rows_out.push(row?);
            }
        }
        rows_out.reverse();
        Ok(rows_out)
    }

    pub fn get_session_allowed_tools(&self, session_id: &str) -> Result<Vec<String>, StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let raw: Option<Option<String>> = conn
            .query_row(
                "SELECT allowed_tools FROM sessions WHERE id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(parse_allowed_tools(raw.flatten()))
    }

    pub fn set_session_allowed_tools(
        &self,
        session_id: &str,
        tools: &[String],
    ) -> Result<(), StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let now = Utc::now().to_rfc3339();
        let json = serde_json::to_string(tools).unwrap_or_else(|_| "[]".to_string());
        conn.execute(
            "UPDATE sessions SET allowed_tools = ?1, updated_at = ?2 WHERE id = ?3",
            params![json, now, session_id],
        )?;
        Ok(())
    }

    pub fn insert_approval(&self, approval: &Approval) -> Result<(), StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        conn.execute(
            "INSERT INTO approvals (id, session_id, task_id, subject, tool_name, tool_use_id, tool_input, decision, actor, requested_at, resolved_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                approval.id,
                approval.session_id,
                approval.task_id,
                approval.subject,
                approval.tool_name,
                approval.tool_use_id,
                approval.tool_input,
                approval.decision,
                approval.actor,
                approval.requested_at.to_rfc3339(),
                approval.resolved_at.map(|d| d.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    pub fn get_approval(&self, id: &str) -> Result<Option<Approval>, StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let mut stmt = conn.prepare(
            "SELECT id, session_id, task_id, subject, tool_name, tool_use_id, tool_input, decision, actor, requested_at, resolved_at
             FROM approvals WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], approval_from_query_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn list_pending_approvals(
        &self,
        task_id: Option<&str>,
    ) -> Result<Vec<Approval>, StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let mut out = Vec::new();
        if let Some(tid) = task_id {
            let mut stmt = conn.prepare(
                "SELECT id, session_id, task_id, subject, tool_name, tool_use_id, tool_input, decision, actor, requested_at, resolved_at
                 FROM approvals WHERE decision IS NULL AND task_id = ?1 ORDER BY requested_at ASC",
            )?;
            let rows = stmt.query_map(params![tid], approval_from_query_row)?;
            for row in rows {
                out.push(row?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, session_id, task_id, subject, tool_name, tool_use_id, tool_input, decision, actor, requested_at, resolved_at
                 FROM approvals WHERE decision IS NULL ORDER BY requested_at ASC",
            )?;
            let rows = stmt.query_map([], approval_from_query_row)?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    pub fn resolve_approval(
        &self,
        id: &str,
        decision: &str,
        actor: &str,
    ) -> Result<(), StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE approvals SET decision = ?1, actor = ?2, resolved_at = ?3 WHERE id = ?4",
            params![decision, actor, now, id],
        )?;
        Ok(())
    }

    // ─── Experience CRUD ─────────────────────────────────────────────────

    pub fn insert_experience(&self, exp: &Experience) -> Result<(), StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        conn.execute(
            "INSERT INTO experiences (id, task_id, face_id, path, content_hash, outcome, summary, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                exp.id,
                exp.task_id,
                exp.face_id,
                exp.path,
                exp.content_hash,
                exp.outcome,
                exp.summary,
                exp.created_at.to_rfc3339(),
                exp.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_experiences(&self) -> Result<Vec<Experience>, StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let mut stmt = conn.prepare(
            "SELECT id, task_id, face_id, path, content_hash, outcome, summary, created_at, updated_at
             FROM experiences ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ExperienceRow {
                id: row.get(0)?,
                task_id: row.get(1)?,
                face_id: row.get(2)?,
                path: row.get(3)?,
                content_hash: row.get(4)?,
                outcome: row.get(5)?,
                summary: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;

        let mut experiences = Vec::new();
        for row in rows {
            let r = row?;
            experiences.push(experience_from_row(r)?);
        }
        Ok(experiences)
    }

    pub fn get_experience(&self, id: &str) -> Result<Option<Experience>, StoreError> {
        Ok(self.list_experiences()?.into_iter().find(|e| e.id == id))
    }

    // ─── Workspace CRUD ──────────────────────────────────────────────────

    pub fn insert_workspace(
        &self,
        id: &str,
        task_id: &str,
        root_path: &str,
        status: &str,
        created_at: &str,
    ) -> Result<(), StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        conn.execute(
            "INSERT OR IGNORE INTO workspaces (id, task_id, root_path, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![id, task_id, root_path, status, created_at],
        )?;
        Ok(())
    }

    pub fn workspace_path_for_task(&self, task_id: &str) -> Result<Option<String>, StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let mut stmt = conn.prepare("SELECT root_path FROM workspaces WHERE task_id = ?1")?;
        let mut rows = stmt.query(params![task_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// Remove a task and its sessions / events / approvals / workspace rows.
    /// Returns workspace directories the caller should delete from disk.
    pub fn delete_task(&self, id: &str) -> Result<Vec<String>, StoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StoreError::Migration(format!("mutex poisoned: {e}")))?;
        let mut paths = Vec::new();
        {
            let mut stmt = conn.prepare("SELECT root_path FROM workspaces WHERE task_id = ?1")?;
            let rows = stmt.query_map(params![id], |row| row.get::<_, String>(0))?;
            for row in rows {
                paths.push(row?);
            }
        }
        conn.execute("DELETE FROM usage_rolls WHERE task_id = ?1", params![id])?;
        conn.execute("DELETE FROM approvals WHERE task_id = ?1", params![id])?;
        conn.execute("DELETE FROM events WHERE task_id = ?1", params![id])?;
        conn.execute("DELETE FROM experiences WHERE task_id = ?1", params![id])?;
        conn.execute("DELETE FROM workspaces WHERE task_id = ?1", params![id])?;
        conn.execute("DELETE FROM sessions WHERE task_id = ?1", params![id])?;
        conn.execute(
            "UPDATE questions SET task_id = NULL WHERE task_id = ?1",
            params![id],
        )?;
        conn.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        Ok(paths)
    }
}

/// Append-only event log row (lifecycle index; payload is JSON).
#[derive(Debug, Clone)]
pub struct EventRecord {
    pub id: String,
    pub event_type: String,
    pub occurred_at: String,
    pub task_id: Option<String>,
    pub session_id: Option<String>,
    pub payload: String,
    pub seq: Option<i64>,
}

fn event_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRecord> {
    Ok(EventRecord {
        id: row.get(0)?,
        event_type: row.get(1)?,
        occurred_at: row.get(2)?,
        task_id: row.get(3)?,
        session_id: row.get(4)?,
        payload: row.get(5)?,
        seq: row.get(6)?,
    })
}

fn parse_allowed_tools(raw: Option<String>) -> Vec<String> {
    let Some(s) = raw else {
        return Vec::new();
    };
    serde_json::from_str(&s).unwrap_or_default()
}

fn approval_from_query_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Approval> {
    let requested_raw: String = row.get(9)?;
    let resolved_raw: Option<String> = row.get(10)?;
    let requested_at = DateTime::parse_from_rfc3339(&requested_raw)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(e))
        })?;
    let resolved_at = match resolved_raw {
        Some(s) if !s.is_empty() => Some(
            DateTime::parse_from_rfc3339(&s)
                .map(|d| d.with_timezone(&Utc))
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        10,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
        ),
        _ => None,
    };
    Ok(Approval {
        id: row.get(0)?,
        session_id: row.get(1)?,
        task_id: row.get(2)?,
        subject: row.get(3)?,
        tool_name: row.get(4)?,
        tool_use_id: row.get(5)?,
        tool_input: row.get(6)?,
        decision: row.get(7)?,
        actor: row.get(8)?,
        requested_at,
        resolved_at,
    })
}

// ─── Internal row types and conversion helpers ───────────────────────────────

struct TaskRow {
    id: String,
    title: String,
    request: String,
    project_id: Option<String>,
    status: String,
    runtime: Option<String>,
    workspace_id: Option<String>,
    resolution: Option<String>,
    version: i64,
    created_at: String,
    updated_at: String,
}

pub(crate) fn parse_datetime(s: &str) -> Result<DateTime<Utc>, StoreError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| StoreError::Migration(format!("failed to parse datetime '{}': {}", s, e)))
}

fn parse_task_status(s: &str) -> Result<TaskStatus, StoreError> {
    s.parse::<TaskStatus>()
        .map_err(|e| StoreError::Migration(e.to_string()))
}

fn parse_session_status(s: &str) -> Result<SessionStatus, StoreError> {
    s.parse::<SessionStatus>()
        .map_err(|e| StoreError::Migration(e.to_string()))
}

fn task_from_row(r: TaskRow) -> Result<Task, StoreError> {
    Ok(Task {
        id: r.id,
        title: r.title,
        request: r.request,
        project_id: r.project_id,
        status: parse_task_status(&r.status)?,
        runtime: r.runtime,
        workspace_id: r.workspace_id,
        resolution: r.resolution,
        version: r.version,
        created_at: parse_datetime(&r.created_at)?,
        updated_at: parse_datetime(&r.updated_at)?,
    })
}

struct SessionRow {
    id: String,
    task_id: String,
    runtime: String,
    executor_sid: Option<String>,
    transport: String,
    pid: Option<i64>,
    cwd: String,
    status: String,
    last_turn: Option<String>,
    started_at: String,
    ended_at: Option<String>,
    updated_at: String,
}

fn session_from_row(r: SessionRow) -> Result<Session, StoreError> {
    let ended_at = match r.ended_at {
        Some(ref s) => Some(parse_datetime(s)?),
        None => None,
    };
    Ok(Session {
        id: r.id,
        task_id: r.task_id,
        runtime: r.runtime,
        executor_sid: r.executor_sid,
        transport: r.transport,
        pid: r.pid.map(|p| p as u32),
        cwd: r.cwd,
        status: parse_session_status(&r.status)?,
        last_turn: r.last_turn,
        started_at: parse_datetime(&r.started_at)?,
        ended_at,
        updated_at: parse_datetime(&r.updated_at)?,
    })
}

struct ExperienceRow {
    id: String,
    task_id: Option<String>,
    face_id: Option<String>,
    path: String,
    content_hash: String,
    outcome: Option<String>,
    summary: Option<String>,
    created_at: String,
    updated_at: Option<String>,
}

fn experience_from_row(r: ExperienceRow) -> Result<Experience, StoreError> {
    let created_at = parse_datetime(&r.created_at)?;
    let updated_at = match r.updated_at {
        Some(ref s) if !s.is_empty() => parse_datetime(s)?,
        _ => created_at,
    };
    Ok(Experience {
        id: r.id,
        task_id: r.task_id.unwrap_or_default(),
        face_id: r.face_id,
        path: r.path,
        content_hash: r.content_hash,
        outcome: r.outcome,
        summary: r.summary,
        created_at,
        updated_at,
    })
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use methodus_domain::{SessionStatus, TaskStatus};

    #[test]
    fn test_open_memory() {
        let store = Store::open_memory().expect("open_memory should succeed");

        // Verify tasks table exists
        let count: i64 = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tasks'",
                    [],
                    |row| row.get(0),
                )
                .map_err(StoreError::from)
            })
            .expect("query should succeed");
        assert_eq!(count, 1, "tasks table should exist");

        // Verify sessions table exists
        let count: i64 = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sessions'",
                    [],
                    |row| row.get(0),
                )
                .map_err(StoreError::from)
            })
            .expect("query should succeed");
        assert_eq!(count, 1, "sessions table should exist");
    }

    #[test]
    fn test_migrations_idempotent() {
        let store = Store::open_memory().expect("open_memory should succeed");

        // Running migrations again should not error
        store
            .with_conn(|conn| {
                run_migrations(conn).map_err(|e| StoreError::Migration(format!("{e}")))?;
                Ok(())
            })
            .expect("second run_migrations should succeed");
        store
            .with_conn(|conn| {
                run_migrations(conn).map_err(|e| StoreError::Migration(format!("{e}")))?;
                Ok(())
            })
            .expect("third run_migrations should succeed");
    }

    #[test]
    fn test_v2_tables_exist() {
        let store = Store::open_memory().expect("open_memory should succeed");

        for table in &["events", "workspaces", "experiences"] {
            let count: i64 = store
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                        params![table],
                        |row| row.get(0),
                    )
                    .map_err(StoreError::from)
                })
                .expect("query should succeed");
            assert_eq!(count, 1, "{} table should exist", table);
        }
    }

    #[test]
    fn test_task_crud() {
        let store = Store::open_memory().unwrap();
        let now = Utc::now();
        let task = Task {
            id: "t-001".to_string(),
            title: "Test task".to_string(),
            request: "Do something".to_string(),
            project_id: None,
            status: TaskStatus::Queued,
            runtime: Some("claude-code".to_string()),
            workspace_id: None,
            resolution: None,
            version: 1,
            created_at: now,
            updated_at: now,
        };

        store.insert_task(&task).unwrap();

        let fetched = store.get_task("t-001").unwrap().unwrap();
        assert_eq!(fetched.id, "t-001");
        assert_eq!(fetched.title, "Test task");
        assert_eq!(fetched.status, TaskStatus::Queued);

        store
            .update_task_status("t-001", TaskStatus::Planning)
            .unwrap();
        let fetched = store.get_task("t-001").unwrap().unwrap();
        assert_eq!(fetched.status, TaskStatus::Planning);

        let tasks = store.list_tasks().unwrap();
        assert_eq!(tasks.len(), 1);
    }

    #[test]
    fn test_get_task_not_found() {
        let store = Store::open_memory().unwrap();
        let result = store.get_task("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn delete_task_removes_sessions_and_events() {
        let store = Store::open_memory().unwrap();
        let now = Utc::now();
        store
            .insert_task(&Task {
                id: "t-001".to_string(),
                title: "gone".to_string(),
                request: "x".to_string(),
                project_id: None,
                status: TaskStatus::Failed,
                runtime: None,
                workspace_id: None,
                resolution: None,
                version: 1,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
        store
            .insert_event(
                "e1",
                "user.message",
                &now.to_rfc3339(),
                Some("t-001"),
                None,
                "{}",
                None,
            )
            .unwrap();
        store.delete_task("t-001").unwrap();
        assert!(store.get_task("t-001").unwrap().is_none());
        assert!(store.list_events(Some("t-001"), 10).unwrap().is_empty());
    }

    #[test]
    fn test_session_crud() {
        let store = Store::open_memory().unwrap();
        let now = Utc::now();

        // Insert a task first (foreign key)
        let task = Task {
            id: "t-001".to_string(),
            title: "Test".to_string(),
            request: "Test".to_string(),
            project_id: None,
            status: TaskStatus::Running,
            runtime: None,
            workspace_id: None,
            resolution: None,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        store.insert_task(&task).unwrap();

        let session = Session {
            id: "s-001".to_string(),
            task_id: "t-001".to_string(),
            runtime: "claude-code".to_string(),
            executor_sid: None,
            transport: "stdio".to_string(),
            pid: Some(12345),
            cwd: "/tmp".to_string(),
            status: SessionStatus::Spawning,
            last_turn: None,
            started_at: now,
            ended_at: None,
            updated_at: now,
        };

        store.insert_session(&session).unwrap();

        let fetched = store.get_session("s-001").unwrap().unwrap();
        assert_eq!(fetched.id, "s-001");
        assert_eq!(fetched.status, SessionStatus::Spawning);
        assert_eq!(fetched.pid, Some(12345));

        store
            .update_session_status("s-001", SessionStatus::Running)
            .unwrap();
        store.set_executor_sid("s-001", "exec-123").unwrap();
        let fetched = store.get_session("s-001").unwrap().unwrap();
        assert_eq!(fetched.status, SessionStatus::Running);
        assert_eq!(fetched.executor_sid, Some("exec-123".to_string()));

        let sessions = store.list_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn test_get_session_not_found() {
        let store = Store::open_memory().unwrap();
        let result = store.get_session("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_event_insert() {
        let store = Store::open_memory().unwrap();
        store
            .insert_event(
                "e-001",
                "assistant_text",
                "2024-01-01T00:00:00+00:00",
                Some("t-001"),
                Some("s-001"),
                r#"{"text":"hello"}"#,
                Some(1),
            )
            .unwrap();

        let count: i64 = store
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
                    .map_err(StoreError::from)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_experience_crud() {
        let store = Store::open_memory().unwrap();
        let now = Utc::now();

        let exp = Experience {
            id: "exp-001".to_string(),
            task_id: "t-001".to_string(),
            face_id: Some("rust-expert".to_string()),
            path: "faces/rust-expert/experiences/exp-001.md".to_string(),
            content_hash: "abc123".to_string(),
            outcome: Some("success".to_string()),
            summary: Some("Completed without issues".to_string()),
            created_at: now,
            updated_at: now,
        };

        store.insert_experience(&exp).unwrap();

        let experiences = store.list_experiences().unwrap();
        assert_eq!(experiences.len(), 1);
        assert_eq!(experiences[0].id, "exp-001");
        assert_eq!(experiences[0].outcome, Some("success".to_string()));
        assert_eq!(
            experiences[0].path,
            "faces/rust-expert/experiences/exp-001.md"
        );
    }

    #[test]
    fn test_v3_experience_columns() {
        let store = Store::open_memory().expect("open_memory should succeed");
        let count: i64 = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('experiences') WHERE name='path'",
                    [],
                    |row| row.get(0),
                )
                .map_err(StoreError::from)
            })
            .expect("query should succeed");
        assert_eq!(count, 1, "experiences.path should exist after v3");
    }

    #[test]
    fn test_workspace_insert() {
        let store = Store::open_memory().unwrap();
        store
            .insert_workspace(
                "w-001",
                "t-001",
                "/tmp/workspace",
                "active",
                "2024-01-01T00:00:00+00:00",
            )
            .unwrap();

        let count: i64 = store
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))
                    .map_err(StoreError::from)
            })
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            store.workspace_path_for_task("t-001").unwrap().as_deref(),
            Some("/tmp/workspace")
        );
        assert_eq!(store.workspace_path_for_task("missing").unwrap(), None);
    }

    #[test]
    fn test_list_non_terminal_sessions() {
        let store = Store::open_memory().unwrap();
        let now = Utc::now();
        let task = Task {
            id: "t-001".to_string(),
            title: "Test".to_string(),
            request: "Test".to_string(),
            project_id: None,
            status: TaskStatus::Running,
            runtime: None,
            workspace_id: None,
            resolution: None,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        store.insert_task(&task).unwrap();

        let live = Session {
            id: "s-live".to_string(),
            task_id: "t-001".to_string(),
            runtime: "claude-code".to_string(),
            executor_sid: Some("exec-1".to_string()),
            transport: "subprocess".to_string(),
            pid: Some(1),
            cwd: "/tmp".to_string(),
            status: SessionStatus::Running,
            last_turn: None,
            started_at: now,
            ended_at: None,
            updated_at: now,
        };
        let dead = Session {
            id: "s-dead".to_string(),
            task_id: "t-001".to_string(),
            runtime: "claude-code".to_string(),
            executor_sid: Some("exec-2".to_string()),
            transport: "subprocess".to_string(),
            pid: None,
            cwd: "/tmp".to_string(),
            status: SessionStatus::Exited,
            last_turn: None,
            started_at: now,
            ended_at: Some(now),
            updated_at: now,
        };
        store.insert_session(&live).unwrap();
        store.insert_session(&dead).unwrap();

        let active = store.list_non_terminal_sessions().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "s-live");

        let for_task = store.list_sessions_for_task("t-001").unwrap();
        assert_eq!(for_task.len(), 2);
    }

    #[test]
    fn test_events_idempotent_and_list() {
        let store = Store::open_memory().unwrap();
        store
            .insert_event(
                "e-1",
                "session.output",
                "2024-01-01T00:00:00+00:00",
                Some("t-1"),
                None,
                "{}",
                Some(1),
            )
            .unwrap();
        store
            .insert_event(
                "e-1",
                "session.output",
                "2024-01-01T00:00:00+00:00",
                Some("t-1"),
                None,
                "{}",
                Some(1),
            )
            .unwrap();
        let events = store.list_events(Some("t-1"), 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "session.output");
    }

    #[test]
    fn test_approval_pending_and_resolve() {
        let store = Store::open_memory().unwrap();
        let now = Utc::now();
        let appr = Approval {
            id: "appr-1".to_string(),
            session_id: "s-1".to_string(),
            task_id: "t-1".to_string(),
            subject: "Write /tmp/x".to_string(),
            tool_name: "Write".to_string(),
            tool_use_id: Some("tu1".to_string()),
            tool_input: r#"{"path":"/tmp/x"}"#.to_string(),
            decision: None,
            actor: None,
            requested_at: now,
            resolved_at: None,
        };
        store.insert_approval(&appr).unwrap();
        assert_eq!(store.list_pending_approvals(Some("t-1")).unwrap().len(), 1);
        store.resolve_approval("appr-1", "once", "user").unwrap();
        assert!(store
            .list_pending_approvals(Some("t-1"))
            .unwrap()
            .is_empty());
        let got = store.get_approval("appr-1").unwrap().unwrap();
        assert_eq!(got.decision.as_deref(), Some("once"));
    }

    #[test]
    fn test_session_allowed_tools() {
        let store = Store::open_memory().unwrap();
        let now = Utc::now();
        store
            .insert_task(&Task {
                id: "t-001".to_string(),
                title: "t".to_string(),
                request: "t".to_string(),
                project_id: None,
                status: TaskStatus::Running,
                runtime: None,
                workspace_id: None,
                resolution: None,
                version: 1,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
        store
            .insert_session(&Session {
                id: "s-001".to_string(),
                task_id: "t-001".to_string(),
                runtime: "claude-code".to_string(),
                executor_sid: None,
                transport: "subprocess".to_string(),
                pid: None,
                cwd: "/tmp".to_string(),
                status: SessionStatus::Running,
                last_turn: None,
                started_at: now,
                ended_at: None,
                updated_at: now,
            })
            .unwrap();
        assert!(store.get_session_allowed_tools("s-001").unwrap().is_empty());
        store
            .set_session_allowed_tools("s-001", &["Read".into(), "Write".into()])
            .unwrap();
        assert_eq!(
            store.get_session_allowed_tools("s-001").unwrap(),
            vec!["Read".to_string(), "Write".to_string()]
        );
    }
}
