# Methodus

> **Persistent Personal Expert System** — a long-running local daemon that picks the
> right expert perspective, method, and skills for your task, drives an AI coding
> agent to do the work in an isolated workspace, supervises the result, and distills
> vetted experience into reusable knowledge over time.

Methodus is **not** another coding agent. It is the brain that orchestrates one:
it selects, prepares, observes, evaluates, and remembers — while an external
executor (Claude Code, Codex, or Cursor) acts as the hands.

> Methodus does not have one fixed expertise. It learns better methods for the
> problems you care about.

## Status

**M0–M2 complete; M3 (TUI) not started.** `methodus init` / `task create` / `run` /
`recover` / `approve` / `events tail` drive Claude Code (manual permission loop) or
Codex (`--runtime codex`) in-process. Task create resolves a Face + Method + Skills
from `resources/` and `~/.methodus/` and surfaces low-confidence choices. Skills are
Methodus-owned only; executor user dirs (`~/.claude`, `~/.cursor`) are not scanned.

Not yet: TUI, learning loop.

- Product & design: [`docs/design/`](./docs/design/) — the product contract
  ([`00-product.md`](./docs/design/00-product.md): the *what & why*) plus the technical
  design (*how*: runtime adapters, architecture, data model, roadmap).
- v1 (archived prompt agent): [`docs/legacy/`](./docs/legacy/).

## Core ideas

- **Persistent** — a single long-lived local process you keep open (e.g. in `tmux`);
  background work is scheduled, budgeted, and policy-controlled (never an unbounded
  `while true` LLM loop).
- **Personal** — knowledge, experience, and project context are yours, stored locally.
- **Adaptive** — no permanently bound role; each task loads one or more *Faces*
  (domain expert snapshots) on demand.
- **Evidence-first** — observations, experiences, hypotheses, candidate knowledge,
  and committed knowledge are layered; a single model output is never treated as fact.
- **Executor-agnostic** — Methodus drives Claude Code / Codex / Cursor through a
  uniform `RuntimeAdapter`; it is not welded to any one of them.
- **Human-in-the-loop** — dangerous actions, knowledge promotion, and proactive
  questions are gated by policy.

## Architecture at a glance

```text
methodus (one long-lived Rust process, kept open in tmux)
  ├── Engine (methodus-core, a UI-free library)
  │     ├── Task / Face / Method / Skill resolution
  │     ├── Policy engine (permission decisions)
  │     ├── Knowledge & Experience store (SQLite + YAML/Markdown)
  │     ├── Learning / Curiosity scheduling (queued jobs, budgeted)
  │     └── Session Manager
  │           ├── Claude Code adapter   (print/stream-json, --resume; --bg for recovery)
  │           ├── Codex adapter         (exec --json; app-server JSON-RPC later)
  │           └── Cursor adapter        (agent --print stream-json + --resume)
  └── TUI (ratatui, same process; observes the Engine, issues commands in-process)
```

The one process owns all state and the executor sessions. Executor sessions also
persist on the executor side (session ids + `--resume`), so a Methodus restart
reconciles and resumes rather than losing work. Run it in `tmux` to keep it alive
across terminal sessions.

> No separate daemon/client split in v1. `methodus-core` is kept UI-free so that
> split can be added later as an optional layer *if* an unattended service, multiple
> simultaneous clients, or a desktop app is ever required. See
> [`docs/design/02-architecture.md`](./docs/design/02-architecture.md) §0.

## Technology

Rust. Async runtime on `tokio`, SQLite for lifecycle/queue/events, YAML/Markdown for
human-readable domain content (Faces, Methods, Knowledge), `ratatui` for the in-process
TUI. See [`docs/design/02-architecture.md`](./docs/design/02-architecture.md).

## Supported executors

All three have been verified to support non-interactive execution, structured event
streams, and session resume. See
[`docs/design/01-runtime-adapters.md`](./docs/design/01-runtime-adapters.md) for the
full capability matrix and integration details.

| Executor    | Non-interactive | Structured events | Session resume | Real-time approval | Persistent daemon |
|-------------|:---------------:|:-----------------:|:--------------:|:------------------:|:-----------------:|
| Claude Code | ✅ `--print`     | ✅ stream-json     | ✅ `--resume`   | ✅                  | ✅ `--bg`          |
| Codex       | ✅ `exec`        | ✅ `--json`        | ✅ `exec resume`| ✅ (app-server)     | ✅ (app-server)    |
| Cursor      | ✅ `--print`     | ✅ stream-json     | ✅ `--resume`   | ⚠️ coarse           | ❌                 |

## License

See [`LICENSE`](./LICENSE).
