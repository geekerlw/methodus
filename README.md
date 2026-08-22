<div align="center">

# Methodus

### Turn engineering investigation into durable memory for coding agents.

Methodus is a local-first knowledge studio where maintainers investigate, verify, and
publish engineering knowledge that Claude Code, Codex, Cursor, and other native agents
can retrieve on demand.

[![Rust 1.75+](https://img.shields.io/badge/rust-1.75%2B-orange?logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Status: early development](https://img.shields.io/badge/status-early%20development-yellow.svg)](docs/04-roadmap.md)

[Product contract](docs/00-product.md) · [Architecture](docs/02-architecture.md) · [TUI guide](docs/05-tui.md) · [Continuous learning](docs/10-continuous-learning.md)

</div>

> Coding agents are excellent at the task in front of them. Methodus helps an
> engineering team remember what it already learned—and use that memory safely.

![Methodus architecture](docs/architecture.svg)

## Why Methodus?

Important engineering knowledge is usually scattered across source code, logs, Git
history, incident notes, and conversations. It is difficult to verify, easy to forget,
and rarely available when a different developer asks the next question.

Methodus gives maintainers a deliberate workflow for turning that evidence into a
reviewed graph of reusable Knowledge, Methods, and Experiences. Native agents then
consume a small, bounded, read-only context bundle without leaving their normal
terminal workflow.

## The model

```text
Maintainer
  → Use question from the reviewed graph
  → explicit Learn conversation, or a Goal on a cadence
  → evidence, counterexamples, and open questions
  → structured CandidateSet
  → Review: edit, approve, reject, merge
  → Personal / Team Markdown graph

Developer in a native agent runtime
  → connector Skill
  → methodus agent manifest (read-only)
  → native agent selects and reads relevant nodes
  → native runtime continues the task
```

The TUI is the maintainer surface; the Agent CLI is the consumer read surface. Markdown
and Git remain inspectable sources of truth; SQLite is a rebuildable index.

## Quick start

### Requirements

- Rust 1.75 or newer
- At least one supported runtime on `PATH`: Claude Code, Codex, or Cursor Agent

### Install the connector and launch Methodus

```bash
# Install the official read-only connector for all supported runtimes.
cargo run -p methodus -- setup --runtime all

# Open the maintainer TUI.
cargo run -p methodus

# Check the local graph, runtimes, and connector installation.
cargo run -p methodus -- doctor
```

The first launch creates `~/.methodus/` and seeds a small Personal graph. The
connector contains instructions only—it does not contain your knowledge graph.

Keep the process open in `tmux`. Methodus schedules its own work while it runs, so a
closed terminal is a paused system.

## Use and Learn in the TUI

The home screen is Methodus Use. Type an ordinary question and Methodus prepares a
graph environment plus an inventory contract, creates a Methodus-managed runtime
workspace, then hands the terminal to the selected native Runtime. The Runtime reads
relevant Knowledge, Methods, and Experiences itself and owns the answer conversation.
The graph, project directory, and explicit `@` sources are protected by the Use
contract; temporary notes and plans belong in the managed workspace. Use does not
create learning candidates. Follow-up questions can resume the same Use conversation;
`@` can attach a local source as additional evidence.

If the Runtime cannot find sufficient committed evidence, the Use contract requires it
to avoid guessing and write one `learning_recommended` return object with a concrete
Learn task. Methodus places that recommendation in `/attention`; pressing `Enter`
accepts it as a new Learn Goal, while typed text replaces the task before creation.

Learning is explicit. `/learn <text>` starts a focused native learning conversation;
`/learn` enters Learn mode so the next message continues or starts that run. When the
runtime finalizes, it writes a Review-only return artifact; exit the Runtime and
Methodus restores the TUI to import the candidate set.

| Action | Key or command |
|---|---|
| Ask Methodus | Type a question, then `Enter` |
| Start or continue Learn | `/learn [text]` |
| Attach a source | Type `@`, choose a path, then `Tab` or `Enter` |
| Add a line break | `Shift+Enter` |
| Cycle Use/Learn Runtime permission | `Shift+Tab` (Use or Learn mode) |
| Switch Runtime | `/runtime` or `/runtime codex` |
| Clear the current session | `/new` (`/new <goal>` remains a Learn shortcut) |
| Browse knowledge and review | `/knowledge`, `/method`, `/experience`, `/review` |
| Create or manage scheduled learning | `/goal [text]` (`/goals` is an alias) |
| Handle questions and learning recommendations | `/attention` |
| Inspect graph relations | Select an active node, then press `g` |
| Leave Methodus | `/quit`; `Ctrl+C` twice is the escape hatch |

The Use Runtime receives a Methodus-managed environment contract, the selected Team and
Personal/Team directory structure, an inventory of consumer-visible Markdown nodes,
and the opened graph directories. It is instructed to inspect the files itself and
separate graph-backed facts, inferences, stale evidence, and unknowns. Its permission
mode follows the same mapping as Learn, but its writable surface is the managed Use
workspace. Learn uses the corresponding `workspaces/learn/<run-id>/` directory and
keeps its return artifact under `runs/<run-id>/`. The Learn Runtime is instructed to clarify scope, challenge assumptions,
inspect evidence, seek counterexamples, and return a structured CandidateSet only when
the evidence is sufficient. Both Use and Learn hand the terminal to the native runtime;
only Learn imports a return artifact into Review.

`/goal <text>` creates a persistent Goal using the same natural-language input and `@`
source attachments as Learn. Cadence, budget, and other policy fields are filled with
their defaults. `/goal` without text opens the management view; `/goals` remains a
compatible alias, and `e` opens advanced YAML editing.

`/new` clears the current Use or Learn context and returns to Use. `quit` only exits the
TUI: an active Learn run is restored on the next launch, while a run waiting for Review
remains a review record instead of reopening a Runtime conversation.

## Keep learning on a cadence

A Goal is a standing question rather than a task. You create one the same way you start
a Learn run — type `/goal` followed by what you want followed:

```text
/goal Understand Rust async cancellation and scheduling behavior in our services @src/runtime
```

It starts out investigating weekly, reviewing what it published weekly, consolidating
monthly, and checking its sources daily, under a $20 monthly ceiling. The first
learning turn starts immediately; later turns follow the cadence. Press `e` to open
the Goal as YAML in `$EDITOR` and change any of that:

```yaml
title: Understand Rust async cancellation and scheduling behavior in our services
prompt: Understand Rust async cancellation and scheduling behavior in our services.
sources: [docs/async, src/runtime]
cadence: weekly              # investigate
review_cadence: weekly       # re-check what is already published
summary_cadence: monthly     # consolidate
source_check_cadence: daily  # detect drift
quiet_hours: {start: "22:00", end: "07:00"}
budget_usd: 20
```

Due turns run headless while you keep using the terminal, and only interrupt you when
they need a judgment, fail, or produce candidates. `r` starts a learning turn
immediately, `s` a consolidation one. A turn that gets stuck on a decision
stops and asks; `/attention` shows the queue, and answering resumes the very session
that asked, natively, with your answer as the next message.

See [Continuous learning](docs/10-continuous-learning.md) for the scheduling policy,
budget accounting, and session-ownership rules.

## Give native agents reviewed memory

Install the connector once:

```bash
methodus setup --runtime all
methodus doctor
```

For substantial diagnosis, design, research, document, or presentation work, the
connector can call the read-only protocol:

```bash
methodus agent manifest --format json

methodus agent get knowledge/previous-shutdown-reason --facet all

methodus agent related knowledge/previous-shutdown-reason
methodus agent status
```

The manifest gives the native agent the graph roots and complete consumer-visible
inventory. The native agent chooses relevant nodes, then reads their full bodies with
`get` or follows authored relations with `related`. The protocol is read-only by
construction: it cannot create graph nodes, modify candidates, start a Runtime, or
read an ordinary coding transcript. If Methodus is unavailable, the connector tells
the native agent to continue normally.

## Trust model

Only reviewed content is available to consumer agents:

```text
candidate → committed → stale → committed
candidate --reject--> deleted
```

- `candidate` content is excluded from Agent retrieval; Review rejection deletes it.
- `stale` content is returned only when strongly relevant and carries a warning.
- `deprecated` content is retained for explicit history queries.
- Source changes never rewrite a conclusion automatically; maintainers decide what to
  update.

## Storage

```text
~/.methodus/
├── config.yaml             # Runtime, permission, and Team selection
├── state.db                # Rebuildable graph/search projection
├── methodus.lock           # Single-writer TUI lock
├── personal/
│   ├── knowledge/          # Canonical Personal Knowledge
│   ├── methods/            # Canonical Personal Methods
│   ├── experiences/        # Canonical Personal Experiences
│   └── candidates/         # Review-only drafts
├── teams/<team-id>/        # Local Git-backed Team roots
├── runs/                   # Learn transcripts, sources, and review audit
├── workspaces/             # Methodus-managed Learn and Use runtime workspaces
│   ├── learn/<run-id>/
│   └── use/<session-id>/
└── connectors/             # Connector ownership/version metadata
```

Markdown/YAML is canonical for graph content. Personal and Team are separate roots;
Team changes remain normal Git work. Methodus can validate, show status and diff, and
write a local publish plan, but never silently commits, pushes, merges, or discards
changes.

## What Methodus is—and is not

### It is

- A terminal Learn, Schedule, and Review workbench
- A Markdown-first Knowledge / Method / Experience graph
- A local evidence and freshness tracker
- A read-only Agent graph-environment CLI
- A small connector Skill for Claude Code, Codex, and Cursor

### It is not

- A replacement coding agent or general chat client
- An MCP server or an independent background daemon
- A task workspace or repository-copy manager
- A proxy for native Claude/Codex/Cursor interaction
- A generic Skill marketplace or automatic Skill generator
- An autonomous graph writer or Git publisher

## Documentation

| Document | What it covers |
|---|---|
| [Product contract](docs/00-product.md) | Positioning, workflows, graph semantics, and boundaries |
| [Runtime adapters](docs/01-runtime-adapters.md) | Learn Runtime integration and connector behavior |
| [Architecture](docs/02-architecture.md) | Processes, storage, indexing, and security |
| [Data model](docs/03-data-model.md) | Markdown nodes, evidence, lifecycle, and freshness |
| [Roadmap](docs/04-roadmap.md) | Current implementation status and remaining evaluation |
| [TUI guide](docs/05-tui.md) | Panels, commands, keyboard behavior, and recovery |
| [Agent CLI](docs/06-agent-cli.md) | Stable read-only protocol and exit codes |
| [Learning protocol](docs/07-learning-vs-refine.md) | Deliberate learning and CandidateSet generation |
| [Development contract](docs/08-development-contract.md) | Invariants and change checklist |
| [Architecture decisions](docs/09-decisions.md) | Locked product and technical decisions |
| [Continuous learning](docs/10-continuous-learning.md) | Goals, cadences, budgets, unattended turns, and the attention queue |

The architecture diagram is available as [SVG](docs/architecture.svg) and
[PNG](docs/architecture.png).

## Development

```bash
cargo fmt --check
cargo test --workspace
cargo check --workspace
git diff --check
```

The active implementation keeps writes explicit: scheduling can launch bounded native
Learn sessions, but only Review can publish canonical graph content. New work should
preserve that boundary.

## License

[MIT](LICENSE) © 2026 Steven Lee
