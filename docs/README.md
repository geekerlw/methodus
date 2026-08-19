# Methodus documentation

This directory is the **source of truth** for Methodus: product contract, architecture,
data model, TUI, and the learning loop. There is no separate spec file
(`PROJECT_SPEC.md` is folded into `00-product.md`).

| # | Document | What it covers |
|---|----------|----------------|
| — | [`architecture.svg`](./architecture.svg) / [`.png`](./architecture.png) | Legacy system diagram; update after the graph/capsule implementation spike |
| 00 | [`00-product.md`](./00-product.md) | **Product contract**: Markdown-first knowledge graph, context capsules, native agent handoff, deliberate learning |
| 01 | [`01-runtime-adapters.md`](./01-runtime-adapters.md) | Verified runtime capabilities; default native handoff vs optional managed adapters |
| 02 | [`02-architecture.md`](./02-architecture.md) | Rust stack, graph/workspace compiler modules, single-process control plane |
| 03 | [`03-data-model.md`](./03-data-model.md) | Graph files, typed-edge index, task capsules, context-selection and review state |
| 04 | [`04-roadmap.md`](./04-roadmap.md) | Graph-first delivery milestones and acceptance criteria |
| 05 | [`05-tui.md`](./05-tui.md) | Ratatui graph/task/review control plane; explicitly not an agent chat |
| 07 | [`07-learning-vs-refine.md`](./07-learning-vs-refine.md) | Graph evolution and deliberate learning compared with Prime Agent `/refine` |

`00-product.md` wins for product intent; `01`–`05` win for implementation detail.

## Decisions that stick

1. **Language is Rust**, not TypeScript (`02-architecture.md`).
2. **Native handoff first.** Claude Code/Codex/Cursor own their interactive TUI;
   structured JSON/JSONL is an optional managed-execution path (`01-runtime-adapters.md`).
3. **One always-on control-plane process**, not a daemon/client split. Keep `methodus` open in `tmux`
   (`02-architecture.md` §0).
4. **User-triggered execution.** Background work is post-task learning and idle Curiosity.
   No cron auto-run of executors (`04-roadmap.md`).
5. **Markdown graph is the knowledge source.** SQLite indexes it; prompts receive only
   selected facets and lazy references. Nothing silently commits to live knowledge or skills.

Open questions sit at the bottom of each document.
