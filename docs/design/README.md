# Methodus Technical Design

This directory is the **complete documentation set** for Methodus v2 — both the product
contract and its technical design. It is self-contained; there is no separate spec file
(the original `PROJECT_SPEC.md` has been folded into `00-product.md`).

Read in order:

| # | Document | What it covers |
|---|----------|----------------|
| 00 | [`00-product.md`](./00-product.md) | **The product contract** (*what & why*): positioning, principles, domain abstractions (Task, Face, Method, Skill, Knowledge, Experience, Question, Hypothesis, Evolution), the three loops, the event catalog, the permission model, the CLI/TUI surface, out-of-scope list, and acceptance scenarios. |
| 01 | [`01-runtime-adapters.md`](./01-runtime-adapters.md) | Verified capabilities of Claude Code / Codex / Cursor, the capability matrix, the `RuntimeAdapter` trait, and per-executor integration (CLI flags, JSON event shapes, session resume, permission/approval, Codex `app-server` JSON-RPC). Everything here was empirically confirmed. |
| 02 | [`02-architecture.md`](./02-architecture.md) | Rust technology stack, crate/module layout, the **single-process** runtime model (no daemon/client split), async/concurrency model, crash recovery. |
| 03 | [`03-data-model.md`](./03-data-model.md) | SQLite schema (DDL), on-disk file layout, and the source-of-truth split between the database and human-readable YAML/Markdown. |
| 04 | [`04-roadmap.md`](./04-roadmap.md) | Phased implementation plan (walking skeleton first; single always-on process), with per-phase acceptance criteria. |

## Provenance & key decisions

`00-product.md` preserves the product intent from the original spec. Three
stack/mechanism decisions made during the design spike **correct** the original spec
and are already reflected across `00`–`04`:

1. **Language is Rust**, not TypeScript (rationale in `02-architecture.md`).
2. **No PTY layer.** All three executors expose structured (JSON/JSONL)
   non-interactive modes with session resume, so Methodus drives them via SDK-style
   process/protocol integration rather than screen-scraping a pseudo-terminal
   (rationale in `01-runtime-adapters.md`).
3. **Single always-on process, not a daemon/client split.** Methodus is one
   long-lived process kept open (in `tmux`); persistence + restart recovery come from
   executor-native session resume. The split is a deferred, optional refactor kept
   cheap by keeping `methodus-core` UI-free (rationale in `02-architecture.md` §0).

## Design status

Complete enough to begin M0/M1 coding. Open decisions and risks are tracked inline in
each document under an **"Open questions"** heading.
