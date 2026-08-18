# CLAUDE.md

Guidance for Claude Code (and any AI executor) when working in this repository.

## What this repo is

Methodus is a **Persistent Personal Expert System**: a single long-running local Rust
process that orchestrates an external AI coding agent (Claude Code, Codex, or Cursor)
to complete tasks, then distills vetted experience into reusable knowledge. You keep
the one `methodus` process open in `tmux`. Methodus is the *brain*;
the executor is the *hands*.

**Current state:** Rust workspace with in-process Engine and ratatui TUI (`methodus`) as
the delivery shell. Keep the process open in `tmux` for daily use.

## Source of truth (read before writing code)

All documentation lives in [`docs/design/`](./docs/design/):

1. `00-product.md` — the **product contract** (*what & why*): domain abstractions
   (Task, Face, Method, Skill, Knowledge, Experience, Question, Hypothesis,
   Evolution), the three loops (Execution / Learning / Curiosity), the event catalog,
   permission model, CLI/TUI surface, out-of-scope list, and acceptance scenarios.
2. `01-runtime-adapters.md` — **verified** executor capabilities (spike results),
   the `RuntimeAdapter` trait, and per-executor integration. Read this before
   touching any session/adapter code; the CLI flags and JSON event shapes here
   were empirically confirmed, not guessed.
3. `02-architecture.md` — Rust stack, crate/module layout, the single-process
   runtime model (no daemon/client split), async model, crash recovery.
4. `03-data-model.md` — SQLite schema DDL, file layout, source-of-truth rules.
5. `04-roadmap.md` — implementation order and acceptance criteria per milestone.
6. `05-tui.md` — agent TUI contract (ratatui chrome, overlays, Pi-aligned components).
7. [`docs/legacy/`](./docs/legacy/) — the archived v1 prompt agent. Reference only;
   do not extend it.

`00-product.md` defines product intent; `01`–`05` define implementation. When they
appear to disagree, `01`–`05` win for implementation detail and `00` wins for product
intent. If neither is clear, ask.

## Non-negotiable design principles (from `00-product.md`)

- **Prompt is an interface, not a database.** State machines, permission decisions,
  path isolation, version resolution, retries, knowledge promotion, and conflict
  detection live in Rust — never in a prompt. Prompts carry only resolved task
  context, method steps, and output constraints.
- **Executor-agnostic.** Core logic depends only on the `RuntimeAdapter` trait.
  Never hard-code Claude Code (or any one executor) behavior into the core.
- **Evidence-first.** A single model output is never committed as knowledge. Respect
  the promotion path: Observation → Experience → Hypothesis/Candidate → committed
  Knowledge, with conflict checks.
- **Auditable & recoverable.** All long-lived state is in SQLite + files; events are
  append-only; the process must recover tasks/sessions after a restart (via executor
  session ids + `--resume`).
- **Budgeted background work.** No unbounded LLM loops. Learning/Curiosity work is
  queued, rate-limited, and cancelable.
- **Workspace isolation.** Each task gets its own workspace; global skills/MCP stay
  visible but task-specific context is only additive. Writes are bounded by
  project/workspace roots and gated by policy.

## Engineering conventions (once code exists)

- Language: **Rust**, async on `tokio`. See `docs/design/02-architecture.md` for the
  crate/module boundaries and dependency choices.
- Define types, state-machine enums, SQLite schema, and the event model **before**
  wiring any LLM/executor integration.
- All side effects go through injectable traits (e.g. `RuntimeAdapter`, repositories,
  clock) so they can be tested and swapped.
- Every feature needs: an explicit state model, success/failure/cancel/recovery
  paths, events + logs, a policy/error boundary, and at least one unit test plus an
  integration test where applicable.
- Verify structured executor output by parsing and validating it; never rely on a
  model happening to follow a format.

## Runtime facts you can rely on (verified)

These were confirmed empirically (see `docs/design/01-runtime-adapters.md`):

- Claude Code: `claude --print --output-format stream-json --verbose`
  (needs `--verbose` with stream-json); `--session-id <uuid>` + `--resume <id>`
  round-trip context; `--permission-mode manual` returns structured
  `permission_denials`; `--allowed-tools` grants precisely; `--bg` + `claude agents`
  provide a built-in background daemon.
- Codex: `codex exec --json` emits clean JSONL; `codex exec resume <id>` restores
  context; `codex app-server` exposes a full JSON-RPC protocol (thread/turn
  lifecycle + `item/*/requestApproval`) over stdio/unix/ws for real-time approval.
- Cursor: `cursor agent --print --output-format stream-json` + `--resume <id>` work;
  permission control is coarse (`--force` / `--plan` / `--auto-review`), tool calls
  are visible but no background daemon.

## Safety

- Never modify the user's global executor config (`~/.claude`, `~/.codex`, `~/.cursor`)
  from the core; Methodus only forwards to each executor's own permission machinery.
- Treat `.env`, credential files, and `*.db` runtime state as sensitive; they are
  git-ignored and must not be committed.
- Do not run destructive git operations without explicit user confirmation.
