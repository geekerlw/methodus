# Methodus documentation

This directory is the product and engineering source of truth for Methodus.

Methodus is a maintainer-operated knowledge studio and a read-only knowledge sidecar
for coding agents. Maintainers learn, curate, review, and publish engineering knowledge
in the terminal TUI. Claude Code, Codex, and other agents consume the published result
through one official connector Skill that calls the local Methodus CLI.

`00`–`09` describe the maintainer-driven half of the product: you ask, a runtime
investigates, you review. [`10-continuous-learning.md`](./10-continuous-learning.md)
adds the other half — the scheduled work Methodus does when nobody is watching.

| Doc | Purpose |
|---|---|
| [`00-product.md`](./00-product.md) | Product contract, users, boundaries, and end-to-end workflows |
| [`01-runtime-adapters.md`](./01-runtime-adapters.md) | Learning runtime adapters and the official connector Skill |
| [`02-architecture.md`](./02-architecture.md) | Components, process model, repositories, indexing, and publishing; includes the architecture diagram |
| [`03-data-model.md`](./03-data-model.md) | Markdown graph, source evidence, lifecycle, freshness, and SQLite indexes |
| [`04-roadmap.md`](./04-roadmap.md) | Implementation order and milestone acceptance criteria |
| [`05-tui.md`](./05-tui.md) | Maintainer-facing Learn, Goals, Attention, Review, Graph, Team, and Publish experience |
| [`06-agent-cli.md`](./06-agent-cli.md) | Stable read-only CLI protocol used by the connector Skill |
| [`07-learning-vs-refine.md`](./07-learning-vs-refine.md) | Deliberate learning, candidate generation, and non-automatic evolution |
| [`08-development-contract.md`](./08-development-contract.md) | Implementation invariants, file rules, lifecycle gates, and change checklist |
| [`09-decisions.md`](./09-decisions.md) | Locked product and architecture decisions for future contributors |
| [`10-continuous-learning.md`](./10-continuous-learning.md) | Learning Goals, cadences, budgets, unattended turns, and the attention queue |

## Standing decisions

1. **Write few, read many.** One or a few maintainers curate knowledge; ordinary
   developers consume it without learning the Methodus TUI.
2. **TUI for humans, CLI for agents.** The terminal is the only maintainer surface; a
   native runtime needs a real TTY, and policy lives in `methodus-core` so a second
   surface stays possible. Agent runtimes use a stable, non-interactive, read-only CLI.
3. **One official connector Skill.** Methodus does not manage a general Skill library.
   Its shipped connector only teaches a runtime when and how to call the CLI.
4. **No MCP and no ordinary coding-task workspace.** Agent knowledge is exposed
   through a manifest and read on demand; files are not copied into project/task
   directories and Methodus does not take over normal coding sessions. Its own Use
   and Learn handoffs use managed operational workspaces under Methodus home.
5. **Curated engineering memory, not generic RAG.** Code, Git, docs, PRs, and logs are
   evidence used during Learn. Only reviewed conclusions are published.
6. **Markdown + Git remain open.** Team repositories can be edited and reviewed with
   normal tools. SQLite is a rebuildable index and lifecycle store.
7. **No silent evolution.** Source changes mark knowledge stale. Models may propose
   candidates, but only maintainers commit, merge, deprecate, or publish them.
8. **Automation is review-gated.** Scheduling may start bounded native runtime work and
   notify maintainers, but it cannot make candidate output canonical.

The current architecture diagram is available as [`architecture.svg`](./architecture.svg)
and [`architecture.png`](./architecture.png). Keep future diagrams aligned with
`02-architecture.md` and this contract.
