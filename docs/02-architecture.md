# 02 — Architecture

Rust technology stack, crate/module layout, the **single-process** runtime model, the
async model, and crash recovery. This document defines *how the code is organized*;
the domain semantics come from [`00-product.md`](./00-product.md). The system diagram
is [`architecture.svg`](./architecture.svg).

## 0. Process model decision: single always-on process (no daemon/client split)

Methodus runs as **one long-lived process** that holds core state, executor sessions,
the scheduler, the store, and the UI together. You keep it open (e.g. in a `tmux`
window) — that *is* the "persistent" part.

**We explicitly do NOT build a detached background daemon + thin client + socket
IPC.** The daemon/client split's only real benefit is decoupling the always-on "brain"
from the window you view it through. For a personal, single-user, single-machine tool
that you keep open, that decoupling is not worth its cost (a socket, a JSON-RPC line
protocol, request/response serialization, and a second recovery problem).

**This is not a one-way door.** `methodus-core` stays a pure library, so the
daemon/client split can be added *later* as an optional layer that wraps the same core
— without rewriting it. It becomes worth doing only if one of these becomes a hard
requirement:

- Methodus must keep working with **no window open at all** (true unattended service).
- **Multiple simultaneous clients** need to talk to the same live brain.

Until then: single process, TUI in `tmux`. See §4 and `04-roadmap.md`.

## 1. Why Rust

The original spec suggested TypeScript. We chose **Rust** because Methodus is long-lived
personal infrastructure, not a throwaway prototype:

- **Single static binary** for `methodus` — trivial distribution, no runtime
  dependency to install.
- **Long-running stability** — no GC pauses, predictable memory; it is meant to stay
  open for days.
- **Domain modeling** — the domain model is full of state machines and tagged unions
  (Task status, event types, permission decisions). Rust `enum` + exhaustive
  `match` express these precisely and catch missing transitions at compile time.
- **Low external-IO complexity** — Methodus mostly spawns CLIs, reads/writes files,
  and talks SQLite. It does not need the npm ecosystem.

Accepted cost: ~2–3× slower initial development than TypeScript. Worth it for a tool
meant to be maintained and trusted over years.

## 2. Technology stack

| Concern | Choice | Notes |
|---------|--------|-------|
| Async runtime | `tokio` (multi-thread) | concurrent sessions, timers, scheduler |
| CLI parsing | `clap` (derive) | `methodus` binary (`--help` / `--version`); no operational subcommands |
| SQLite | `sqlx` (async, compile-time-checked) or `rusqlite` (sync + pool) | see §7; leaning `sqlx` for async + migrations |
| Migrations | `sqlx::migrate!` (or `refinery` if `rusqlite`) | versioned, embedded |
| Serialization | `serde` + `serde_json` + `serde_yaml` | events, executor I/O, YAML domain files |
| Subprocess | `tokio::process::Command` | drive executor CLIs |
| Codex app-server | JSON-RPC client over `unix://` | later milestone; this is a *client* to Codex's daemon, not our own daemon |
| TUI | `ratatui` + `crossterm` | the always-on UI, in-process |
| Logging/events | `tracing` + `tracing-subscriber` | structured; bridges to event log |
| IDs | `uuid` (v4/v7) | entity + session ids |
| Time | `chrono` (UTC) | timestamps, `not_before` scheduling |
| Errors | `thiserror` (libs) + `anyhow` (binary) | typed errors in core |
| Testing | built-in + `tokio::test` + `assert_cmd` + `tempfile` | unit + integration/e2e |

No socket/IPC framework is needed for v1 (the UI calls core in-process). Dependencies
are pinned to exact versions in `Cargo.toml`. No large server frameworks.

## 3. Workspace & crate layout

A Cargo **workspace** with focused crates. Crate boundaries enforce the product's
separation rules at the dependency-graph level, and — critically — keep
`methodus-core` a **pure library** so the process model can evolve without a rewrite.

```text
methodus/
├── Cargo.toml                 # [workspace]
├── crates/
│   ├── methodus-domain/       # pure types: entities, enums, state machines, events
│   │   └── (no I/O, no tokio; just data + transition logic + validation)
│   ├── methodus-store/        # SQLite repositories + migrations + file-content store
│   │   └── (depends on domain)
│   ├── methodus-runtime/      # RuntimeAdapter trait + ClaudeCode/Codex/Cursor impls
│   │   └── (depends on domain; owns subprocess + Codex app-server client)
│   ├── methodus-core/         # orchestration LIBRARY: resolver, policy, scheduler,
│   │   │                        loops, session manager, event bus — no UI, no main()
│   │   └── (depends on domain, store, and the runtime trait)
│   └── methodus/              # THE binary: opens the TUI, runs core in-proc
│       └── (depends on core, store, runtime)
├── migrations/                # SQL migration files (embedded by methodus-store)
├── resources/                 # seed Faces / Methods / Skills shipped with the build
│   ├── faces/
│   ├── methods/
│   └── skills/
├── docs/
└── ...
```

Notes vs the earlier daemon design:

- **No `methodusd` binary and no `methodus-ipc` crate.** There is one binary,
  `methodus`, that embeds the core library and the TUI.
- `methodus-core` has **no `main()`** and no UI — it is a library the binary drives.
  This is the single most important structural rule: it is what makes a future
  daemon/client split a wrapping exercise rather than a rewrite. Do not let UI or CLI
  concerns leak into `methodus-core`.

### Module map inside `methodus-core`

```text
methodus-core/src/
├── resolution/     # Task Resolver (rule-based v1; LLM resolver interface reserved)
├── policy/         # permission decisions, approval routing, budgets
├── session/        # Session Manager: lifecycle over RuntimeAdapter, event fan-out
├── workspace/      # Workspace Builder + path-safety validation
├── pack.rs         # team pack folders: packs.yaml registry, focus, overlay roots
├── project.rs      # project directories: projects.yaml registry + focus
├── home.rs         # first-launch seed + health checks
├── events/         # event bus (in-proc broadcast) + append-only persistence hook
├── scheduler/      # job queue driver (event/threshold/idle triggers)
├── learning.rs     # extract_experience, detect_gaps, propose_knowledge, propose_skill jobs
├── refine.rs       # propose_refinement + budgeted LLM polish of note/patch drafts
├── curiosity/      # knowledge-gap → question valuation
└── engine.rs       # top-level orchestrator tying the Execution Loop together;
                    # the binary constructs one Engine and the TUI observes it
```

## 4. Runtime model: one process, in-process UI

```text
┌─────────────────────── methodus (one long-lived process; kept open in tmux) ──────────┐
│  tokio runtime                                                                         │
│    ├── Engine (methodus-core)                                                          │
│    │     ├── Session Manager   (owns M executor sessions; each = task + adapter stream)│
│    │     ├── Scheduler         (learning/curiosity jobs; budgeted; cancelable)         │
│    │     ├── Event bus         (tokio broadcast; fans events to store + UI)            │
│    │     └── Store             (SQLite pool + file content store)                      │
│    └── UI (ratatui, same process)                                                      │
│          subscribes to the event bus; issues commands to the Engine via direct calls   │
└────────────────────────────────────────────────────────────────────────────────────┘
        │ spawns / resumes
        ▼
   Executor CLIs (claude / codex / cursor) — each with its OWN session persistence
```

- The Engine and the TUI live in **one process**; the TUI talks to the Engine through
  **direct in-process calls** and subscribes to the in-memory event bus. No socket, no
  serialization boundary.
- **Executor sessions are kept alive by the executors themselves** (Claude `--bg` /
  Codex app-server / session ids + `--resume`). So even though Methodus is a single
  process, the *executor* work is not fatally coupled to it: if Methodus is restarted,
  it reconciles and resumes (see §6).
- **"Attach/detach" is reframed.** Within the single process the TUI is always
  attached to the live Engine. Cross-terminal persistence ("close the terminal, come
  back later") is achieved by running the process in `tmux`/`screen`, plus
  executor-session resume for recovery after a real restart.

### No second-terminal CLI

Because there is no daemon to talk to, a second `methodus` in another terminal cannot
drive the running instance (the lock refuses it). Observe from the TUI; keep the
process in `tmux`. Scripting subcommands are out of v1.

## 5. Async, concurrency & single-instance

- One **`tokio` multi-thread runtime**.
- Each executor session runs as a supervised task that owns its `EventStream` (from
  `RuntimeAdapter`), normalizes native events into `RuntimeEvent`, publishes them to
  the event bus, and persists them (append-only) via the store.
- The **event bus** is a `tokio::sync::broadcast`; the store subscriber is durable, the
  UI subscriber is best-effort (bounded; a slow UI cannot stall a session — drop-oldest
  with a "lagged" marker).
- **Cancellation** uses `tokio_util::sync::CancellationToken` per session and per job,
  so `cancel`/`kill` and shutdown are clean.
- **Backpressure:** executor stdout is read line-by-line; parsing is streaming. Large
  tool outputs go to the session transcript file, with only a summary in SQLite.
- **Single-instance guard:** since state lives in `state.db`, hold an advisory
  lock/lockfile (`~/.methodus/methodus.lock`) so two full instances don't drive
  sessions or the scheduler concurrently.

## 6. Crash / restart recovery

The process must recover after being killed or restarted. Mechanism:

1. All task/session/job lifecycle lives in SQLite (see `03-data-model.md`); events are
   append-only. Nothing critical lives only in memory.
2. On startup, scan for sessions in a non-terminal state and reconcile against the
   executor's own persistence:
   - **Claude Code:** query `claude agents --json`; if the background agent still
     exists, reattach; otherwise mark `interrupted` and offer resume via
     `--resume <session_id>`.
   - **Codex:** the stored `thread_id` allows `codex exec resume` / app-server
     `thread/resume`.
   - **Cursor:** stored `session_id` allows `--resume`, but an in-flight turn is lost;
     mark `interrupted`.
3. In-flight **turns** are not assumed to survive; the *session/context* survives via
   the executor's own persistence. Methodus records the "last known turn" so the user
   can resume or cancel.
4. Learning/curiosity **jobs** are durable queue rows with `attempts`/`not_before`; the
   scheduler re-picks them on startup.

**Corollary for `RuntimeAdapter`:** always capture and persist the executor-issued
session id from the first `SessionStarted` event — it is the recovery key. This is what
lets a single-process Methodus survive its own restarts without losing executor work.

## 7. Store engine decision (open)

- **`sqlx`** — async, compile-time-checked queries, first-class migrations; fits the
  tokio model. Slight build-time cost (needs a dev DB or offline metadata).
- **`rusqlite`** — simpler, synchronous; would need a blocking pool
  (`tokio::task::spawn_blocking`) to avoid stalling the runtime.

**Recommendation:** start with `sqlx` unless compile-time query checking proves painful
early, then fall back to `rusqlite` + a blocking pool. Decide in M0 and record here.

## 8. Open questions

1. **`sqlx` vs `rusqlite`** — finalize in M0 (§7).
2. **One-off mutating CLI** — closed: v1 is TUI-only; no operational subcommands.
3. **Event bus durability ordering** — guarantee persist-before-UI-push, or allow the
   UI to see events slightly ahead of the durable write? Prefer persist-first for
   auditability; measure latency impact.
4. **When (if ever) to add the daemon/client split** — revisit at M4 against the three
   triggers in §0. Keeping `methodus-core` UI-free is the standing precondition.
