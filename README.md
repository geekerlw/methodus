# Methodus

Methodus is a local, Markdown-first **knowledge graph and context compiler** for
Claude Code, Codex, and Cursor. It is deliberately not another coding-agent chat
surface: Methodus prepares the right context before a task, hands the terminal to
the runtime's native TUI, then returns to record results and improve the graph.

```text
Graph (Knowledge · Skill · Experience · Artifact)
  → select compact facets under a token budget
  → compile a Task Workspace capsule
  → native Claude Code / Codex / Cursor TUI
  → return outcome → Experience + reviewable Knowledge candidate
```

## What is implemented

- Markdown graph nodes under `~/.methodus/graph/`, with typed frontmatter links and
  a SQLite index for search, selection, context provenance, and launch history.
- Token-bounded task capsules containing an execution brief, selected context,
  lazy references, runtime adapter notes, and copied `SKILL.md` packages.
- A native TUI handoff: Methodus temporarily exits its alternate screen and gives
  the terminal directly to the selected runtime. It neither proxies nor parses the
  agent conversation.
- A conversation-first TUI: send a goal like an Agent message, then Methodus plans
  context and automatically switches the terminal to the selected native runtime.
- Slash-command management panels: `/knowledge`, `/skill`, `/experience`, `/session`,
  and `/review` keep durable state available without displacing the task composer.
- Learning tasks that return as an Experience plus a reviewable 5W2H Knowledge
  candidate; review promotes the candidate by changing its Markdown status.

## Start

```bash
cargo run -p methodus
```

The first run creates `~/.methodus`. In the task conversation:

- Write a goal and press `Enter`; Methodus creates a short-lived, read-only
  planner runtime to choose graph context, compiles a capsule, then automatically
  hands the terminal to Claude Code, Codex, or Cursor.
- Use `/knowledge`, `/skill`, `/experience`, `/session`, and `/review` to manage state.
- Use `/runtime` to switch the native runtime; press `Shift+Tab` to cycle permission mode.
- Type `/` for the command palette; use `↑`/`↓` and `Tab` to select and complete.
- `/open` opens the current capsule, while `/quit` or `/exit` closes Methodus.
  A plain `q` remains ordinary input; pressing `Ctrl+C` twice within three seconds
  is the keyboard escape hatch.
- Use `/learn`, then send a topic beginning with `学习：` to create a learning task.
- After the native runtime exits, write an outcome and press `Enter` to feed it
  back into the experience/knowledge loop.

The composer supports bracketed paste, multiline input (`Shift+Enter`), UTF-8 cursor
editing, and correct double-width cursor placement for Chinese/CJK text. Task-launch
messages are cached and can be scrolled with `PageUp`/`PageDown`.

The task's repository remains the native runtime working directory. The generated
capsule lives in Methodus storage and contains the concise startup brief plus all
auditable context decisions.

## Design

The current product contract and technical shape are documented in
[`docs/00-product.md`](docs/00-product.md), [`docs/03-data-model.md`](docs/03-data-model.md),
and [`docs/05-tui.md`](docs/05-tui.md).
