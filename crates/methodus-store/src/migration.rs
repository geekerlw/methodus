use crate::StoreError;
use rusqlite::Connection;

/// Run all pending migrations against the given connection.
///
/// Creates a `_migrations` meta-table to track which versions have been applied,
/// then applies each pending migration in order.
pub fn run_migrations(conn: &Connection) -> Result<(), StoreError> {
    // Create the migrations meta-table if it doesn't exist.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            version    INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );",
    )?;

    // V1: tasks and sessions tables
    if !is_applied(conn, 1)? {
        apply_v1(conn)?;
        mark_applied(conn, 1)?;
    }

    // V2: events, workspaces, and experiences tables
    if !is_applied(conn, 2)? {
        apply_v2(conn)?;
        mark_applied(conn, 2)?;
    }

    // V3: experience file index (path + content_hash) per 03-data-model.md
    if !is_applied(conn, 3)? {
        apply_v3(conn)?;
        mark_applied(conn, 3)?;
    }

    // V4: approvals + session allow-list for the M2 permission loop
    if !is_applied(conn, 4)? {
        apply_v4(conn)?;
        mark_applied(conn, 4)?;
    }

    // V5: learning queue, knowledge, questions, hypotheses (M4)
    if !is_applied(conn, 5)? {
        apply_v5(conn)?;
        mark_applied(conn, 5)?;
    }

    // V6: executor token / cost rolls
    if !is_applied(conn, 6)? {
        apply_v6(conn)?;
        mark_applied(conn, 6)?;
    }

    Ok(())
}

fn is_applied(conn: &Connection, version: i64) -> Result<bool, StoreError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM _migrations WHERE version = ?1",
        [version],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn mark_applied(conn: &Connection, version: i64) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO _migrations (version, applied_at) VALUES (?1, datetime('now'))",
        [version],
    )?;
    Ok(())
}

fn apply_v1(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "
        -- Tasks
        CREATE TABLE tasks (
            id            TEXT PRIMARY KEY,
            title         TEXT NOT NULL,
            request       TEXT NOT NULL,
            project_id    TEXT,
            status        TEXT NOT NULL,
            runtime       TEXT,
            workspace_id  TEXT,
            resolution    TEXT,
            version       INTEGER NOT NULL DEFAULT 1,
            created_at    TEXT NOT NULL,
            updated_at    TEXT NOT NULL
        );
        CREATE INDEX idx_tasks_status ON tasks(status);

        -- Sessions
        CREATE TABLE sessions (
            id             TEXT PRIMARY KEY,
            task_id        TEXT NOT NULL REFERENCES tasks(id),
            runtime        TEXT NOT NULL,
            executor_sid   TEXT,
            transport      TEXT NOT NULL,
            pid            INTEGER,
            cwd            TEXT NOT NULL,
            status         TEXT NOT NULL,
            last_turn      TEXT,
            started_at     TEXT NOT NULL,
            ended_at       TEXT,
            updated_at     TEXT NOT NULL
        );
        CREATE INDEX idx_sessions_task ON sessions(task_id);
        CREATE INDEX idx_sessions_status ON sessions(status);
        ",
    )?;
    Ok(())
}

fn apply_v2(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "
        -- Events
        CREATE TABLE events (
            id          TEXT PRIMARY KEY,
            type        TEXT NOT NULL,
            occurred_at TEXT NOT NULL,
            task_id     TEXT,
            session_id  TEXT,
            payload     TEXT NOT NULL,
            seq         INTEGER
        );
        CREATE INDEX idx_events_session ON events(session_id, seq);
        CREATE INDEX idx_events_task ON events(task_id, occurred_at);

        -- Workspaces
        CREATE TABLE workspaces (
            id          TEXT PRIMARY KEY,
            task_id     TEXT NOT NULL,
            root_path   TEXT NOT NULL,
            status      TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL
        );

        -- Experiences
        CREATE TABLE experiences (
            id           TEXT PRIMARY KEY,
            task_id      TEXT,
            face_id      TEXT,
            outcome      TEXT,
            summary      TEXT,
            created_at   TEXT NOT NULL
        );
        ",
    )?;
    Ok(())
}

fn apply_v3(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "
        ALTER TABLE experiences ADD COLUMN path TEXT NOT NULL DEFAULT '';
        ALTER TABLE experiences ADD COLUMN content_hash TEXT NOT NULL DEFAULT '';
        ALTER TABLE experiences ADD COLUMN updated_at TEXT;
        ",
    )?;
    Ok(())
}

fn apply_v4(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "
        ALTER TABLE sessions ADD COLUMN allowed_tools TEXT;

        CREATE TABLE approvals (
            id           TEXT PRIMARY KEY,
            session_id   TEXT,
            task_id      TEXT,
            subject      TEXT NOT NULL,
            tool_name    TEXT NOT NULL,
            tool_use_id  TEXT,
            tool_input   TEXT NOT NULL,
            decision     TEXT,
            actor        TEXT,
            requested_at TEXT NOT NULL,
            resolved_at  TEXT
        );
        CREATE INDEX idx_approvals_session ON approvals(session_id);
        CREATE INDEX idx_approvals_task ON approvals(task_id);
        ",
    )?;
    Ok(())
}

fn apply_v5(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "
        CREATE TABLE knowledge_items (
            id           TEXT PRIMARY KEY,
            face_id      TEXT,
            project_id   TEXT,
            path         TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            source       TEXT NOT NULL,
            confidence   REAL,
            scope        TEXT,
            status       TEXT NOT NULL,
            conflict_of  TEXT,
            version      INTEGER NOT NULL DEFAULT 1,
            created_at   TEXT NOT NULL,
            updated_at   TEXT NOT NULL
        );
        CREATE INDEX idx_knowledge_status ON knowledge_items(status);
        CREATE INDEX idx_knowledge_path ON knowledge_items(path);

        CREATE TABLE questions (
            id           TEXT PRIMARY KEY,
            question     TEXT NOT NULL,
            reason       TEXT,
            task_id      TEXT,
            face_id      TEXT,
            importance   REAL,
            frequency    REAL,
            impact       REAL,
            uncertainty  REAL,
            value        REAL,
            status       TEXT NOT NULL,
            not_before   TEXT,
            answer       TEXT,
            created_at   TEXT NOT NULL,
            updated_at   TEXT NOT NULL
        );
        CREATE INDEX idx_questions_status ON questions(status);

        CREATE TABLE hypotheses (
            id           TEXT PRIMARY KEY,
            face_id      TEXT,
            path         TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            confidence   REAL,
            status       TEXT NOT NULL,
            created_at   TEXT NOT NULL,
            updated_at   TEXT NOT NULL
        );

        CREATE TABLE learning_jobs (
            id                TEXT PRIMARY KEY,
            kind              TEXT NOT NULL,
            priority          INTEGER NOT NULL DEFAULT 0,
            dedupe_key        TEXT,
            input_refs        TEXT NOT NULL,
            status            TEXT NOT NULL,
            attempts          INTEGER NOT NULL DEFAULT 0,
            not_before        TEXT,
            budget            TEXT,
            requires_approval INTEGER NOT NULL DEFAULT 0,
            created_at        TEXT NOT NULL,
            updated_at        TEXT NOT NULL
        );
        CREATE INDEX idx_jobs_status_notbefore ON learning_jobs(status, not_before);
        CREATE UNIQUE INDEX idx_jobs_dedupe ON learning_jobs(dedupe_key)
            WHERE dedupe_key IS NOT NULL;
        ",
    )?;
    Ok(())
}

fn apply_v6(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "
        CREATE TABLE usage_rolls (
            id                  TEXT PRIMARY KEY,
            task_id             TEXT,
            session_id          TEXT,
            runtime             TEXT,
            input_tokens        INTEGER NOT NULL DEFAULT 0,
            output_tokens       INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens   INTEGER NOT NULL DEFAULT 0,
            cache_write_tokens  INTEGER NOT NULL DEFAULT 0,
            cost_usd            REAL,
            occurred_at         TEXT NOT NULL
        );
        CREATE INDEX idx_usage_occurred ON usage_rolls(occurred_at);
        CREATE INDEX idx_usage_task ON usage_rolls(task_id);
        ",
    )?;
    Ok(())
}
