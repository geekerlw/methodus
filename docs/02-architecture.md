# 02 — Architecture

## 1. System shape

Methodus is a local-first Rust application with two entry surfaces in one binary:

- an in-process Ratatui maintainer studio;
- a short-lived, non-interactive `methodus agent` query interface.

No daemon or MCP server is required for the first product.

![Methodus architecture](./architecture.svg)

The diagram shows the two supported interaction surfaces: the maintainer TUI owns
focused Learn and review writes, while the official connector Skill reaches the
read-only `methodus agent` protocol from a native coding runtime.

```text
                         Maintainer
                             │
                       Methodus TUI
                             │
          ┌──────────────────┼──────────────────┐
          │                  │                  │
       Learn engine      Review/publish      Graph browser
          │                  │                  │
   Learning runtime      Markdown + Git      SQLite index

Claude / Codex
     │ official connector Skill
     ▼
methodus agent (read-only CLI)
     │
     └──────────── query Personal + synced Team index
```

## 2. Storage and ownership

- Markdown/YAML is authoritative for Knowledge, Method, Experience, relations, and
  source references.
- Git is the distribution and review mechanism for Team content.
- SQLite is authoritative only for the rebuildable graph/index projection. Learn and
  review audit state is file-backed under `runs/` so it remains inspectable and
  recoverable.
- The TUI is the normal write surface, but direct Markdown edits remain supported.
- The Agent CLI opens the store in a read-only/query mode and never mutates graph
  content.

## 3. Crate responsibilities

```text
crates/
├── methodus-domain/    shared graph/runtime/status value types
├── methodus-store/     SQLite migrations and rebuildable graph projection
├── methodus-runtime/   Claude/Codex/Cursor LearningRuntime adapters
├── methodus-core/      active application service, Markdown graph, retrieval,
│                      learning candidates, review actions, Team status
└── methodus/           binary, Ratatui maintainer TUI, setup/doctor, agent CLI
```

The active service intentionally stays small. Its source modules are
`engine.rs` (maintainer orchestration), `graph.rs` (Markdown parsing, links, source
fingerprints, validation), `agent.rs` (read-only retrieval), `home.rs` (home layout and
health), `config.rs` (runtime preference), `mentions.rs` (source path completion), and
`lock.rs` (single-writer TUI lock). Do not reintroduce the retired task/workspace,
Face, or arbitrary Skill modules under a new name. When a responsibility becomes too
large, extract it without changing the boundaries below.

Planned extraction seams, not required directories, are:

- deterministic retrieval and token budgeting from `agent.rs`;
- Learn state and CandidateSet parsing from `engine.rs`;
- source adapters and freshness checks from `graph.rs`;
- review/edit/deprecate/revalidate operations from `engine.rs`;
- Team repository configuration and publish validation from `engine.rs`.

Retired active-path modules include Face resolution/evolution, arbitrary Skill
management, task workspace compilation, and ordinary coding-session management.
Compatibility migrations may read legacy rows, but new code must not route product
behavior through them. Focused Learn retains a small native-handoff lifecycle only.

## 4. Process model

### 4.1 TUI process

The TUI is a single foreground process. It owns:

- one active maintainer interaction;
- zero or one active Learn turn;
- active Learn conversation state restored from `runs/<run-id>/state.yaml` and
  `events.jsonl` when a resumable executor ID exists;
- graph/index refresh and validation jobs;
- explicit Team status, validation, diff, and publish-plan actions; normal Git tooling
  remains responsible for sync/commit/push.

The current implementation performs small graph and Team checks synchronously and
keeps the render loop bounded by the local Markdown corpus. A process lock protects
TUI writes; read-only Agent CLI processes open the SQLite file with SQLite read-only
flags and do not run migrations or refresh the graph.

### 4.2 Agent CLI process

Each connector invocation starts a short-lived process:

1. parse and validate arguments;
2. open the current index read-only;
3. retrieve and budget results deterministically;
4. write Markdown or JSON to stdout;
5. exit with a documented code.

There is no session, scheduler, background model call, network call, or graph write in
this path.

## 5. Retrieval pipeline

`prepare(goal, budget)` is deterministic in v1:

```text
normalize goal
  → lexical/tag/scope recall over committed + eligible stale nodes
  → select Methods matching intent
  → expand a bounded set of typed relations
  → rank Knowledge and reusable Experience lessons
  → prefer committed over stale
  → select facets under token budget
  → emit rationale, lifecycle, source status, and lazy node IDs
```

The native agent remains the reasoning model. Methodus does not make a second LLM call
for ordinary consumption. Semantic embeddings may be added later only if evaluated
against a real engineering-query corpus.

## 6. Learn pipeline

```text
Learn goal
  → existing-graph retrieval
  → source registration
  → runtime dialogue and evidence collection
  → explicit unknown/conflict tracking
  → proposed CandidateSet
  → maintainer split/merge/edit
  → Review decisions
  → Personal commit
  → optional Team publish
```

Learn runs are local operational state, not durable graph nodes. They store the goal,
runtime/executor identity, event stream, last assistant synthesis, unresolved
questions, contradictions, and candidate drafts under `runs/`. No normal coding
workspace is created and graph content is not copied into a run. A run can be resumed
or reviewed after the TUI process exits without making the run itself Agent-visible.

## 7. Personal and Team overlay

The query view combines:

1. Personal committed content;
2. one or more locally synced Team repositories. `teams/default` is the seed; the
   selected Team ID is stored in `config.yaml` and can be changed from the TUI.
   External repository-path mapping is a later Team-management extension.

Stable IDs are globally unique within the combined view. A Personal node may override
a Team node only through an explicit local overlay marker; accidental duplicate IDs
are validation errors.

Team Git operations are explicit TUI actions. v1 provides status, validation, a
bounded diff, and a local `publish-plan.md`; normal Git tooling performs commit,
push, merge, and remote synchronization. Methodus does not automatically push, merge,
or rewrite remote history.

## 8. Implementation status

The repository currently provides the following vertical slice:

| Area | v1 behavior |
|---|---|
| Graph | Markdown sync from `graph/`, `personal/`, and `teams/*`; typed links; duplicate/broken-link validation; source fingerprint stale marking |
| Learn | Runtime-backed Learn conversation with explicit permission mode; deliberate-learning protocol; durable event stream; structured CandidateSet JSON; transcript and candidate files under `runs/` and `personal/candidates/` |
| Review | TUI inspect, commit Personal, reject, mark Team visibility, and explicit Knowledge merge target |
| Team | selected Team status, Git diff, validation summary, and non-mutating publish plan |
| Agent boundary | `methodus agent prepare/search/get/related/status`; read-only SQLite; candidate/rejected exclusion; bounded results |
| Connector | `methodus setup` installs the single official read-only connector Skill; it contains no graph data |

Candidate editing, stale revalidation, deprecation, Team selection, and connector
ownership checks are maintainer actions. Draft splitting and richer relation editing
remain incremental extensions. All such changes must extend the same state
transitions and never bypass Review. Git commit/push automation remains outside
Methodus.

## 9. Security boundary

- The Agent CLI is read-only by construction.
- Runtime permission decisions belong to the user/runtime; Methodus only maps the
  visible maintainer selection to bounded native modes and never bypasses enforcement.
- Real production logs are temporary Learn inputs unless explicitly converted into a
  scrubbed source artifact; Team nodes should store log patterns and meaning, not
  sensitive raw logs.
- Paths must remain inside configured source roots unless a maintainer explicitly
  attaches an external source.
- Git credentials and remote authentication remain external to Methodus.

## 10. Observability and recovery

- Every Learn/Review/Publish transition emits a local audit event.
- A failed Learn turn preserves the run, transcript, sources, and last valid candidate
  set.
- Indexes are rebuilt from Markdown after schema changes or corruption.
- Publication is blocked on invalid frontmatter, duplicate IDs, broken required links,
  or unresolved merge conflicts.
- Stale detection reports risk and never edits canonical conclusions.
