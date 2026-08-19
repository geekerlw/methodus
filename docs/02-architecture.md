# 02 — Architecture

Rust technology stack, crate/module layout, the **single-process** runtime model, the
async model, and crash recovery. This document defines *how the code is organized*;
the domain semantics come from [`00-product.md`](./00-product.md). The system diagram
is [`architecture.svg`](./architecture.svg).

## 0. Process model decision: single always-on process (no daemon/client split)

Methodus runs as **one long-lived process** that holds graph state, task compilation,
the scheduler, the store, and its graph/review UI together. You keep it open (e.g. in
a `tmux` window) — that is the "persistent" part. Normal Agent interaction does not
run inside this process: Methodus launches the chosen native Agent TUI and receives the
user back when that process exits or the user explicitly returns.

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
| Subprocess | `tokio::process::Command` | native terminal handoff and optional managed executor CLIs |
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
│   ├── methodus-runtime/      # native-launch + optional managed adapters
│   │   └── (depends on domain; owns terminal subprocess + Codex app-server client)
│   ├── methodus-core/         # graph resolver/compiler, policy, scheduler,
│   │   │                        learning/review loops, launch manager, event bus — no UI, no main()
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
├── graph/          # file indexing, typed-edge queries, graph search
├── resolution/     # Task Resolver (rule-based v1; LLM ranker interface reserved)
├── policy/         # permission decisions, approval routing, budgets
├── launch/         # native handoff lifecycle + optional managed runtime sessions
├── workspace/      # Workspace Compiler: capsule, budget, adapter rendering, path safety
├── pack.rs         # team pack folders: packs.yaml registry, focus, overlay roots
├── project.rs      # project directories: projects.yaml registry + focus
├── home.rs         # first-launch seed + health checks
├── events/         # event bus (in-proc broadcast) + append-only persistence hook
├── scheduler/      # job queue driver (event/threshold/idle triggers)
├── learning.rs     # deliberate learn sessions + experience/distillation jobs
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
│    │     ├── Graph + Workspace Compiler (selects facets under a budget)                │
│    │     ├── Launch Manager    (native handoff; managed stream only when requested)    │
│    │     ├── Scheduler         (learning/curiosity jobs; budgeted; cancelable)         │
│    │     ├── Event bus         (tokio broadcast; fans events to store + UI)            │
│    │     └── Store             (SQLite pool + file content store)                      │
│    └── UI (ratatui, same process): graph / task compile / review / history             │
│          subscribes to the event bus; issues commands to the Engine via direct calls   │
└────────────────────────────────────────────────────────────────────────────────────┘
        │ native handoff (default) / structured management (optional)
        ▼
   Executor native TUIs (claude / codex / cursor) — each owns its conversation/session
```

- The Engine and the TUI live in **one process**; the TUI talks to the Engine through
  **direct in-process calls** and subscribes to the in-memory event bus. No socket, no
  serialization boundary.
- **Native handoff:** the executor owns the interactive session and its persistence.
  Methodus records only the launch metadata and task capsule; it never attempts to
  reconstruct an interactive transcript.
- **Managed execution:** only this optional mode consumes structured events and stores
  an executor-issued session ID for resume/recovery.
- **Terminal handoff:** the launcher either suspends/restores the Methodus terminal or
  creates a configured tmux/terminal target. The graph UI returns after the native
  process exits, without interpreting its screen output.

### No second-terminal CLI

Because there is no daemon to talk to, a second `methodus` in another terminal cannot
drive the running instance (the lock refuses it). Observe from the TUI; keep the
process in `tmux`. Scripting subcommands are out of v1.

## 5. Async, concurrency & single-instance

- One **`tokio` multi-thread runtime**.
- A native launch is a supervised child-process/terminal-handoff lifecycle; it stores
  only launch/return facts and never consumes an interactive transcript. A managed
  executor session (when explicitly selected) owns its `EventStream`, normalizes
  `RuntimeEvent`s, and persists them via the store.
- The **event bus** is a `tokio::sync::broadcast`; the store subscriber is durable, the
  UI subscriber is best-effort (bounded; a slow UI cannot stall a session — drop-oldest
  with a "lagged" marker).
- **Cancellation** uses `tokio_util::sync::CancellationToken` per session and per job,
  so `cancel`/`kill` and shutdown are clean.
- **Backpressure:** applies only to managed executor stdout, which is read and parsed
  line-by-line. Large managed tool outputs go to artifact files with a summary in
  SQLite; native TUI output is not captured.
- **Single-instance guard:** since state lives in `state.db`, hold an advisory
  lock/lockfile (`~/.methodus/methodus.lock`) so two full instances don't drive
  sessions or the scheduler concurrently.

## 6. Crash / restart recovery

The process must recover after being killed or restarted. Mechanism:

1. All task/workspace/review/job lifecycle lives in SQLite (see `03-data-model.md`); events are
   append-only. Nothing critical lives only in memory.
2. On startup, native-handoff launches left in `launched` state are marked as needing a
   user return/outcome check; Methodus does not claim to reattach to their TUI. For the
   optional managed mode, scan for sessions in a non-terminal state and reconcile against
   the executor's own persistence:
   - **Claude Code:** query `claude agents --json`; if the background agent still
     exists, reattach; otherwise mark `interrupted` and offer resume via
     `--resume <session_id>`.
   - **Codex:** the stored `thread_id` allows `codex exec resume` / app-server
     `thread/resume`.
   - **Cursor:** stored `session_id` allows `--resume`, but an in-flight turn is lost;
     mark `interrupted`.
3. In-flight managed **turns** are not assumed to survive; their session/context may
   survive via the executor's own persistence. The task capsule always survives and can
   be used to launch a fresh native follow-up.
4. Learning/curiosity **jobs** are durable queue rows with `attempts`/`not_before`; the
   scheduler re-picks them on startup.

**Corollary for managed adapters:** capture and persist the executor-issued session ID
from the first `SessionStarted` event. Native handoff instead persists a launch record,
the capsule manifest hash, and the result-review state.

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
