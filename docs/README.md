# Methodus documentation

This directory is the **source of truth** for Methodus: product contract, architecture,
data model, TUI, and the learning loop. There is no separate spec file
(`PROJECT_SPEC.md` is folded into `00-product.md`).

| # | Document | What it covers |
|---|----------|----------------|
| — | [`architecture.svg`](./architecture.svg) / [`.png`](./architecture.png) | System diagram: Execution loop + human-gated Learning loop |
| 00 | [`00-product.md`](./00-product.md) | **Product contract** (*what & why*): Faces, Methods, Skills, Knowledge, the three loops, events, permissions, TUI surface, out of scope |
| 01 | [`01-runtime-adapters.md`](./01-runtime-adapters.md) | Verified Claude Code / Codex / Cursor capabilities and the `RuntimeAdapter` trait |
| 02 | [`02-architecture.md`](./02-architecture.md) | Rust stack, crate layout, **single-process** runtime (no daemon/client split), crash recovery |
| 03 | [`03-data-model.md`](./03-data-model.md) | SQLite schema, on-disk files, source-of-truth split |
| 04 | [`04-roadmap.md`](./04-roadmap.md) | Milestones and acceptance criteria |
| 05 | [`05-tui.md`](./05-tui.md) | In-process ratatui TUI contract |
| 07 | [`07-learning-vs-refine.md`](./07-learning-vs-refine.md) | Learning loop vs Prime Agent `/refine` — comparison and what we shipped |

`00-product.md` wins for product intent; `01`–`05` win for implementation detail.

## Decisions that stick

1. **Language is Rust**, not TypeScript (`02-architecture.md`).
2. **No PTY layer.** Executors are driven through structured JSON/JSONL (`01-runtime-adapters.md`).
3. **One always-on process**, not a daemon/client split. Keep `methodus` open in `tmux`
   (`02-architecture.md` §0).
4. **User-triggered execution.** Background work is post-task learning and idle Curiosity.
   No cron auto-run of executors (`04-roadmap.md`).
5. **Prompt is not the database.** Policy, promotion, and conflict checks live in Rust.
   Candidates land in `/inbox`; nothing silent-commits to live skills.

Open questions sit at the bottom of each document.
