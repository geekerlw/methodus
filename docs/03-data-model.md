# 03 — Data Model

The data model makes Methodus a Markdown-first knowledge graph with an auditable task
context compiler. It implements the product contract in [`00-product.md`](./00-product.md).

## 1. Source of truth

Two stores, one clear split:

- **Markdown/YAML files** are authoritative for durable, human-readable graph
  content: Knowledge, Experience, Face lenses, Methods, Skills, and source records.
  A user must be able to browse, edit, back up, and git-sync these files without
  Methodus.
- **SQLite** is authoritative for lifecycle and derived state: task/capsule history,
  graph indexes, parsed edges, search indexes, context-selection decisions, review
  state, and job/event logs.

The file always wins over a derived SQLite row. A changed hash causes re-indexing;
Methodus never silently overwrites a graph file.

## 2. On-disk layout

```text
~/.methodus/
├── config.yaml
├── state.db
├── methodus.lock
├── graph/
│   ├── knowledge/<id>.md       # independent atomic knowledge nodes
│   ├── experiences/<id>.md
│   ├── artifacts/<id>.md       # source/evidence descriptors, not large copied files
│   ├── faces/<id>.yaml         # domain lenses / graph entry queries
│   ├── methods/<id>.yaml
│   └── candidates/             # reviewable graph-node drafts
├── skills/<name>/SKILL.md
├── packs.yaml                  # registered Markdown-first graph/skill packs
├── projects.yaml               # user repositories, never cloned by Methodus
├── workspaces/<task-id>/       # immutable task or learning capsules
└── queue/                      # optional large durable job payloads
```

A pack follows the same shape (a `pack.yaml`, optional `graph/` and `skills/`).
Personal graph content overlays active packs. Methodus records paths; synchronizing a
folder is an external organizational choice.

## 3. Graph files

Every graph node has a stable ID, title, kind, status, summary, source/evidence, and
typed links. Links may appear in frontmatter or body; frontmatter is the canonical
machine-readable form.

```yaml
---
id: knowledge/payment-idempotency
title: Payment callback idempotency
node_type: knowledge
kind: concept
status: committed
summary: Deduplicate callbacks using a stable provider event ID.
scope: At-least-once payment callbacks
confidence: 0.9
tags: [payment, reliability]
links:
  requires: [knowledge/database-unique-constraint]
  contrasts: [knowledge/request-id-deduplication]
  used_by: [skill/payment-change-checklist]
  applied_by: [experience/payment-webhook-2026-08]
sources: [artifact/stripe-webhook-docs]
---

## Learn (5W2H)
## Decide
## Execute
## Evidence
```

Knowledge body headings are facets. `Learn` is normally 5W2H; `Execute` is the compact
agent-facing content. If an item has no dedicated Execute heading, the compiler may
derive a candidate excerpt for review, but does not rewrite the source item silently.

A Face is a **lens**, not a container:

```yaml
id: face/payment-reliability
title: Payment reliability
intent_tags: [payment, webhook, consistency]
entry_queries:
  - tags_any: [payment, reliability]
preferred_methods: [method/failure-mode-review]
quality_checks: [skill/payment-change-checklist]
```

## 4. SQLite schema

SQLite mirrors files and records decisions that cannot be reconstructed from content
alone. The DDL below is logical; migrations may split it for compatibility.

```sql
CREATE TABLE graph_nodes (
    id TEXT PRIMARY KEY,
    node_type TEXT NOT NULL,       -- knowledge|experience|artifact|face|method|skill
    title TEXT NOT NULL,
    path TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    status TEXT,
    summary TEXT,
    scope TEXT,
    confidence REAL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_nodes_type_status ON graph_nodes(node_type, status);

CREATE TABLE graph_edges (
    id TEXT PRIMARY KEY,
    from_id TEXT NOT NULL REFERENCES graph_nodes(id),
    relation TEXT NOT NULL,        -- requires|extends|contrasts|uses|applied_by|...
    to_id TEXT NOT NULL REFERENCES graph_nodes(id),
    source TEXT NOT NULL,          -- authored|imported|candidate|derived
    confidence REAL,
    evidence_refs TEXT,            -- JSON node IDs
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(from_id, relation, to_id)
);
CREATE INDEX idx_edges_from ON graph_edges(from_id, relation);
CREATE INDEX idx_edges_to ON graph_edges(to_id, relation);

CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,            -- work|learn
    title TEXT NOT NULL,
    request TEXT NOT NULL,
    project_id TEXT,
    runtime TEXT,
    execution_mode TEXT NOT NULL,  -- native_handoff|managed
    status TEXT NOT NULL,          -- drafting|ready|launched|returned|reviewing|completed|...
    workspace_id TEXT,
    result_summary TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_tasks_status ON tasks(status);

CREATE TABLE workspaces (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    root_path TEXT NOT NULL,
    launch_cwd TEXT NOT NULL,      -- usually the user's repository, not root_path
    status TEXT NOT NULL,          -- compiled|launched|returned|archived
    manifest_hash TEXT NOT NULL,
    context_budget_tokens INTEGER,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE context_selections (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
    node_id TEXT NOT NULL REFERENCES graph_nodes(id),
    facet TEXT NOT NULL,           -- base|execute|skill|experience|lazy_reference
    rationale TEXT NOT NULL,
    priority REAL,
    estimated_tokens INTEGER,
    disposition TEXT,              -- injected|lazy|removed
    outcome TEXT,                  -- useful|unused|misleading|unknown
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_context_workspace ON context_selections(workspace_id);

CREATE TABLE launches (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    runtime TEXT NOT NULL,
    mode TEXT NOT NULL,            -- native_handoff|managed
    executor_session_id TEXT,
    command_summary TEXT,
    started_at TEXT NOT NULL,
    returned_at TEXT,
    exit_status TEXT
);

CREATE TABLE reviews (
    id TEXT PRIMARY KEY,
    task_id TEXT REFERENCES tasks(id),
    node_id TEXT REFERENCES graph_nodes(id),
    decision TEXT NOT NULL,        -- commit|revise|reject|defer
    rationale TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE events (
    id TEXT PRIMARY KEY,
    type TEXT NOT NULL,            -- task.*|workspace.*|launch.*|graph.*|review.*
    task_id TEXT,
    payload TEXT NOT NULL,
    occurred_at TEXT NOT NULL
);
```

Task graphs are not inferred from a vague embedding match alone. Retrieval may rank
candidates, but the persisted `context_selections` table records the final rationale,
budget, and outcome.

## 5. Task Workspace / context capsule

```text
~/.methodus/workspaces/<task-id>/
├── manifest.yaml               # immutable task, resolver, and launch snapshot
├── brief.md                    # small startup prompt handed to the runtime
├── context.md                  # selected execute facets and rationale
├── references.md               # lazy absolute/path references to full graph nodes
├── skills/                     # links or materialized task-specific skill packages
├── adapters/
│   ├── claude-code.md          # launch-specific native instruction rendering
│   └── codex.md
├── outcome.md                  # result + retrospective template, completed on return
└── artifacts/                  # task-local output/evidence pointers
```

`workspaces/<task-id>` is a package, **not automatically a source checkout**. The
default native launcher starts Claude Code/Codex in `launch_cwd` (the project root)
and gives it the concise `brief.md` plus paths to the capsule. This avoids mutating the
repository's permanent `CLAUDE.md`/`AGENTS.md` files for a one-off task.

The compiler must:

1. enforce a context budget before launch;
2. render only selected facets into `context.md`;
3. retain full graph notes as lazy references;
4. snapshot all selected versions/hashes in `manifest.yaml`; and
5. preserve the workspace for audit until explicit archival/cleanup.

## 6. State machines

- `TaskStatus`: `drafting → ready → launched → returned → reviewing → completed`, with
  `cancelled`/`failed` exits from launch or review.
- `WorkspaceStatus`: `compiled → launched → returned → archived`.
- `KnowledgeStatus`: `candidate → committed | conflicted | rejected`.
- `LaunchMode`: `native_handoff | managed`.
- `ContextOutcome`: `useful | unused | misleading | unknown`.

All state transitions are validated by domain code, not guessed from natural language.
Events are append-only and idempotent by event ID.

## 7. Open questions

1. Do graph edges authored in Markdown body require explicit frontmatter promotion, or
   is a body-link parser sufficient for v1?
2. Should SQLite FTS5 ship in the first graph-browser milestone, or can title/tag/link
   search establish the initial UX?
3. Which native handoff should be primary on each OS: terminal suspension, a new tmux
   pane, or a configured terminal command?
