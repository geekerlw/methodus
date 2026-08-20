# Methodus documentation

This directory is the product and engineering source of truth for Methodus.

Methodus is a maintainer-operated knowledge studio and a read-only knowledge sidecar
for coding agents. A small number of maintainers learn, curate, review, and publish
engineering knowledge in the TUI. Claude Code, Codex, and other agents consume the
published result through one official connector Skill that calls the local Methodus
CLI.

| Doc | Purpose |
|---|---|
| [`00-product.md`](./00-product.md) | Product contract, users, boundaries, and end-to-end workflows |
| [`01-runtime-adapters.md`](./01-runtime-adapters.md) | Learning runtime adapters and the official connector Skill |
| [`02-architecture.md`](./02-architecture.md) | Components, process model, repositories, indexing, and publishing; includes the architecture diagram |
| [`03-data-model.md`](./03-data-model.md) | Markdown graph, source evidence, lifecycle, freshness, and SQLite indexes |
| [`04-roadmap.md`](./04-roadmap.md) | Implementation order and milestone acceptance criteria |
| [`05-tui.md`](./05-tui.md) | Maintainer-facing Learn, Review, Graph, Team, and Publish experience |
| [`06-agent-cli.md`](./06-agent-cli.md) | Stable read-only CLI protocol used by the connector Skill |
| [`07-learning-vs-refine.md`](./07-learning-vs-refine.md) | Deliberate learning, candidate generation, and non-automatic evolution |
| [`08-development-contract.md`](./08-development-contract.md) | Implementation invariants, file rules, lifecycle gates, and change checklist |
| [`09-decisions.md`](./09-decisions.md) | Locked product and architecture decisions for future contributors |

## Standing decisions

1. **Write few, read many.** One or a few maintainers curate knowledge; ordinary
   developers consume it without learning the Methodus TUI.
2. **TUI for humans, CLI for agents.** Maintainers learn and govern in the TUI.
   Agent runtimes use a stable, non-interactive, read-only CLI.
3. **One official connector Skill.** Methodus does not manage a general Skill library.
   Its shipped connector only teaches a runtime when and how to call the CLI.
4. **No MCP and no ordinary task workspace.** Agent knowledge is retrieved on demand;
   files are not copied into per-task directories and Methodus does not take over
   normal coding sessions.
5. **Curated engineering memory, not generic RAG.** Code, Git, docs, PRs, and logs are
   evidence used during Learn. Only reviewed conclusions are published.
6. **Markdown + Git remain open.** Team repositories can be edited and reviewed with
   normal tools. SQLite is a rebuildable index and lifecycle store.
7. **No silent evolution.** Source changes mark knowledge stale. Models may propose
   candidates, but only maintainers commit, merge, deprecate, or publish them.

The current architecture diagram is available as [`architecture.svg`](./architecture.svg)
and [`architecture.png`](./architecture.png). Keep future diagrams aligned with
`02-architecture.md` and this contract.
