# 03 — Data Model

SQLite schema (DDL), on-disk file layout, and the source-of-truth split. This
realizes the entities and event model in
[`00-product.md`](./00-product.md) §3 (domain abstractions) and §6 (event model).

## 1. Source-of-truth rule

Two stores, one clear split:

- **SQLite (`state.db`)** is authoritative for **lifecycle, indexing, events, and
  queues**: task/session status, event log, job queue, approvals, resolved indexes,
  hashes, and cross-references. Fast to query, transactional, recoverable.
- **Files (YAML/Markdown)** are authoritative for **human-readable domain content**:
  a Face's identity/knowledge, a Method's procedure, Skill packages, Knowledge
  entries. Humans read and edit these; Methodus indexes them into SQLite.

Rule of thumb: **entity *body* → files; entity *lifecycle/index/relations* → SQLite.**
When re-indexing, SQLite rows for file-backed entities are derived and carry the
source `path` + content `hash`; the file wins on conflict, and a hash mismatch
triggers a re-index (never a silent overwrite of the file).

## 2. On-disk layout

```text
~/.methodus/
├── config.yaml                 # user config (executors, budgets, policy defaults)
├── state.db                    # SQLite: lifecycle, events, queue, indexes
├── methodus.lock               # single-instance advisory lock (runtime; gitignored)
├── faces/                      # <face>/face.yaml + knowledge/*.md, experiences/*.md ...
│   └── network/
│       ├── face.yaml
│       ├── knowledge/*.md
│       ├── experiences/*.md
│       └── hypotheses/*.md
├── methods/                    # <method>.yaml (procedure definitions)
├── skills/                     # global Methodus-owned skills (SKILL.md packages)
├── projects/                   # <project>/project.yaml + project-local knowledge
├── workspaces/                 # one isolated dir per task (see §5)
│   └── <task-id>/
└── queue/                      # (optional) durable job artifacts too big for SQLite
```

Domain-content roots (`faces/`, `methods/`, `skills/`, `projects/`) are the file
source-of-truth. `state.db` indexes them.

## 3. SQLite schema (DDL)

Conventions: every table has `id TEXT PRIMARY KEY`, `created_at`, `updated_at`
(ISO-8601 UTC text). Mutable entities add `status` and `version`. Events are
append-only. File-backed entities carry `path` + `content_hash`.

```sql
-- ---------- Tasks ----------
CREATE TABLE tasks (
    id            TEXT PRIMARY KEY,
    title         TEXT NOT NULL,
    request       TEXT NOT NULL,            -- raw user request
    project_id    TEXT REFERENCES projects(id),
    status        TEXT NOT NULL,            -- queued|planning|running|waiting_user|
                                            -- reviewing|completed|failed|cancelled
    runtime       TEXT,                     -- claude-code|codex|cursor
    workspace_id  TEXT,
    resolution    TEXT,                     -- JSON: SelectedFaces/Methods/Skills + rationale
    version       INTEGER NOT NULL DEFAULT 1,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
CREATE INDEX idx_tasks_status ON tasks(status);

-- ---------- Sessions ----------
CREATE TABLE sessions (
    id             TEXT PRIMARY KEY,        -- Methodus session id
    task_id        TEXT NOT NULL REFERENCES tasks(id),
    runtime        TEXT NOT NULL,           -- executor kind
    executor_sid   TEXT,                    -- executor-issued id (uuid/thread_id) — recovery key
    transport      TEXT NOT NULL,           -- subprocess|app-server|background
    pid            INTEGER,
    cwd            TEXT NOT NULL,
    status         TEXT NOT NULL,           -- spawning|running|waiting_user|paused|
                                            -- exited|interrupted|failed
    last_turn      TEXT,                    -- summary of last injected turn
    started_at     TEXT NOT NULL,
    ended_at       TEXT,
    updated_at     TEXT NOT NULL
);
CREATE INDEX idx_sessions_task ON sessions(task_id);
CREATE INDEX idx_sessions_status ON sessions(status);

-- ---------- Events (append-only, unified log) ----------
CREATE TABLE events (
    id           TEXT PRIMARY KEY,
    type         TEXT NOT NULL,             -- e.g. session.output, task.completed
    occurred_at  TEXT NOT NULL,
    task_id      TEXT,
    session_id   TEXT,
    payload      TEXT NOT NULL,             -- JSON
    redaction    TEXT NOT NULL DEFAULT 'none',
    seq          INTEGER                     -- monotonic per (session) for ordering
);
CREATE INDEX idx_events_task ON events(task_id, occurred_at);
CREATE INDEX idx_events_session ON events(session_id, seq);
CREATE INDEX idx_events_type ON events(type);

-- ---------- Workspaces ----------
CREATE TABLE workspaces (
    id          TEXT PRIMARY KEY,
    task_id     TEXT NOT NULL REFERENCES tasks(id),
    root_path   TEXT NOT NULL,
    status      TEXT NOT NULL,              -- created|active|cleaned
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

-- ---------- Projects ----------
CREATE TABLE projects (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    root_path   TEXT NOT NULL,
    path        TEXT,                        -- project.yaml path
    content_hash TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

-- ---------- File-backed domain entities (indexed from disk) ----------
CREATE TABLE faces (
    id           TEXT PRIMARY KEY,          -- e.g. "network"
    name         TEXT NOT NULL,
    path         TEXT NOT NULL,             -- faces/<id>/face.yaml
    content_hash TEXT NOT NULL,
    intent_tags  TEXT,                      -- JSON array (for resolver matching)
    version      INTEGER NOT NULL DEFAULT 1,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

CREATE TABLE methods (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    path          TEXT NOT NULL,
    content_hash  TEXT NOT NULL,
    intent_tags   TEXT,                     -- JSON array
    version       TEXT,                     -- semver from the file
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

CREATE TABLE skills (
    id            TEXT PRIMARY KEY,         -- skill name
    source        TEXT NOT NULL,            -- user-explicit|project|global|builtin|generated
    path          TEXT NOT NULL,            -- SKILL.md path
    content_hash  TEXT NOT NULL,
    version       TEXT,
    compat        TEXT,                     -- JSON: runtime/skill compatibility
    conflict      TEXT,                     -- null | JSON conflict descriptor
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

-- ---------- Knowledge / Experience / Hypothesis / Question ----------
-- Bodies live in files under faces/<face>/... ; these rows index + track lifecycle.
CREATE TABLE knowledge_items (
    id           TEXT PRIMARY KEY,
    face_id      TEXT REFERENCES faces(id),
    project_id   TEXT REFERENCES projects(id),
    path         TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    source       TEXT NOT NULL,             -- experience|user_answer|doc|research
    confidence   REAL,
    scope        TEXT,                      -- applicability
    status       TEXT NOT NULL,             -- candidate|committed|conflicted|rejected
    conflict_of  TEXT REFERENCES knowledge_items(id),
    version      INTEGER NOT NULL DEFAULT 1,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
CREATE INDEX idx_knowledge_status ON knowledge_items(status);

CREATE TABLE experiences (
    id           TEXT PRIMARY KEY,
    task_id      TEXT REFERENCES tasks(id),
    face_id      TEXT REFERENCES faces(id),
    path         TEXT NOT NULL,             -- structured record on disk
    content_hash TEXT NOT NULL,
    outcome      TEXT,                      -- success|partial|failed
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

CREATE TABLE hypotheses (
    id           TEXT PRIMARY KEY,
    face_id      TEXT REFERENCES faces(id),
    path         TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    confidence   REAL,
    status       TEXT NOT NULL,             -- open|validated|rejected
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

CREATE TABLE questions (
    id           TEXT PRIMARY KEY,
    question     TEXT NOT NULL,
    reason       TEXT,
    task_id      TEXT REFERENCES tasks(id),
    face_id      TEXT REFERENCES faces(id),
    importance   REAL, frequency REAL, impact REAL, uncertainty REAL,
    value        REAL,                      -- computed priority
    status       TEXT NOT NULL,            -- pending|asked|answered|snoozed|dismissed
    not_before   TEXT,                     -- cooldown
    answer       TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
CREATE INDEX idx_questions_status ON questions(status);

-- ---------- Evolution ----------
CREATE TABLE evolution_candidates (
    id           TEXT PRIMARY KEY,
    target_kind  TEXT NOT NULL,             -- face|method|skill|knowledge
    target_id    TEXT NOT NULL,
    diff         TEXT NOT NULL,             -- proposed change
    rationale    TEXT,
    source       TEXT,
    status       TEXT NOT NULL,             -- candidate|approved|rejected|active
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

-- ---------- Learning queue ----------
CREATE TABLE learning_jobs (
    id                TEXT PRIMARY KEY,
    kind              TEXT NOT NULL,        -- extract_experience|detect_gaps|propose_knowledge|...
    priority          INTEGER NOT NULL DEFAULT 0,
    dedupe_key        TEXT,                 -- UNIQUE-ish to collapse duplicates
    input_refs        TEXT NOT NULL,        -- JSON refs to source entities
    status            TEXT NOT NULL,        -- queued|running|done|failed|cancelled
    attempts          INTEGER NOT NULL DEFAULT 0,
    not_before        TEXT,                 -- scheduled/backoff time
    budget            TEXT,                 -- JSON: token/cost cap
    requires_approval INTEGER NOT NULL DEFAULT 0,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);
CREATE INDEX idx_jobs_status_notbefore ON learning_jobs(status, not_before);
CREATE UNIQUE INDEX idx_jobs_dedupe ON learning_jobs(dedupe_key) WHERE dedupe_key IS NOT NULL;

-- ---------- Approvals ----------
CREATE TABLE approvals (
    id           TEXT PRIMARY KEY,
    session_id   TEXT REFERENCES sessions(id),
    subject      TEXT NOT NULL,             -- what is being approved
    scope        TEXT NOT NULL,             -- once|session|...
    decision     TEXT,                      -- approve|deny|abort (null while pending)
    actor        TEXT,                      -- user|policy
    requested_at TEXT NOT NULL,
    resolved_at  TEXT
);
CREATE INDEX idx_approvals_session ON approvals(session_id);
```

## 4. Enumerations (mirror Rust `enum`s in `methodus-domain`)

Status/kind columns are `TEXT` in SQLite but map to exhaustive Rust enums. The enum is
the real state machine; the DB stores its serialized name. Transitions are validated
in `methodus-domain`, never inferred from natural language.

- `TaskStatus`: `queued → planning → running → {waiting_user, reviewing} → {completed, failed, cancelled}`
- `SessionStatus`: `spawning → running → {waiting_user, paused} → {exited, interrupted, failed}`
- `KnowledgeStatus`: `candidate → {committed, conflicted, rejected}`
- `QuestionStatus`: `pending → {asked → answered, snoozed, dismissed}`
- `JobStatus`: `queued → running → {done, failed, cancelled}`

Event names follow `00-product.md` §6 (`task.*`, `session.*`, `workspace.*`, `knowledge.*`,
`question.*`, `approval.*`, …). Event handlers must be **idempotent** (keyed on event
`id`) so a replayed event never double-commits knowledge or repeats a side effect.

## 5. Workspace layout (per task)

```text
~/.methodus/workspaces/<task-id>/
├── .methodus/
│   ├── task.yaml            # snapshot of the resolved task
│   ├── plan.md              # execution plan
│   ├── selected-context.md  # the ONLY resolved context handed to the executor
│   └── session.json         # session handle(s) + executor session ids
├── face-context/            # minimal, task-relevant face material (read-only copies)
├── project-context/         # minimal project material
├── artifacts/               # outputs produced by the run
└── transcript/              # full executor event transcript (JSONL) + large tool outputs
```

Rules (from `00-product.md` §9): Face/Project memory is **not** written back through workspace
temp files; workspaces are retained by default for audit; global skills/MCP stay
visible while task-specific context is only additive; writes to the user's project
dir are bounded by the project root and gated by policy; never copy the entire
knowledge base into the workspace — inject only the minimal resolved context.

## 6. Migrations

- Versioned SQL files under `migrations/`, embedded into `methodus-store` and applied
  on process startup before the Engine serves any task.
- Forward-only for MVP; each migration is idempotent-safe to re-check.
- `methodus init` creates `~/.methodus/`, writes a default `config.yaml`, and runs
  migrations to create `state.db`.

## 7. Open questions

1. **FTS** — add SQLite FTS5 for knowledge/experience search now, or defer until there
   is content volume? Lean defer (Phase 2), but reserve a `knowledge_fts` virtual
   table name.
2. **Event payload size** — cap inline `payload` JSON size; spill large tool outputs
   to `transcript/` files and store only a pointer + summary. Define the threshold
   (proposed: 40 KB, matching Cursor's `fileOutputThresholdBytes`).
3. **experience.json import** — provide a one-shot importer for v1
   `workflow_patterns` (see `../legacy/README.md`) into `experiences`.
