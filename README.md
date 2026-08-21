<div align="center">

# Methodus

### Turn engineering investigation into durable memory for coding agents.

Methodus is a local-first knowledge studio where maintainers investigate, verify, and
publish engineering knowledge that Claude Code, Codex, Cursor, and other native agents
can retrieve on demand.

[![Rust 1.75+](https://img.shields.io/badge/rust-1.75%2B-orange?logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Status: early development](https://img.shields.io/badge/status-early%20development-yellow.svg)](docs/04-roadmap.md)

[Product contract](docs/00-product.md) · [Architecture](docs/02-architecture.md) · [TUI guide](docs/05-tui.md)

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
  → focused Learn conversation
  → evidence, counterexamples, and open questions
  → structured CandidateSet
  → Review: edit, approve, reject, merge
  → Personal / Team Markdown graph

Developer in a native agent runtime
  → connector Skill
  → methodus agent (read-only)
  → relevant Method / Knowledge / Experience
  → native runtime continues the task
```

The TUI is the maintainer write surface. The Agent CLI is the consumer read surface.
Markdown and Git remain inspectable sources of truth; SQLite is a rebuildable index.

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

## Learn in the TUI

The home screen starts a focused learning conversation. Type an ordinary message to
state what you want to understand; Methodus records the goal and hands the terminal to
your selected native Runtime. Continue the investigation there just as you normally
would. When you ask it to finalize, it writes a Review-only return artifact; exit the
Runtime and Methodus restores the TUI to import the candidate set.

| Action | Key or command |
|---|---|
| Start or continue Learn | Type a message, then `Enter` |
| Attach a source | Type `@`, choose a path, then `Tab` or `Enter` |
| Add a line break | `Shift+Enter` |
| Cycle Runtime permission | `Shift+Tab` |
| Switch Runtime | `/runtime` or `/runtime codex` |
| Start a fresh learning goal | `/new` |
| Browse knowledge and review | `/knowledge`, `/method`, `/experience`, `/review` |
| Inspect graph relations | Select an active node, then press `g` |
| Leave Methodus | `/quit`; `Ctrl+C` twice is the escape hatch |

The learning Runtime is instructed to clarify scope, challenge assumptions, inspect
evidence, seek counterexamples, separate fact from inference, and return a structured
CandidateSet only when the evidence is sufficient. Methodus does not proxy this
conversation, so runtime tool views, approvals, and multi-turn interaction stay native.

`/new` closes the current Learn context. `quit` only exits the TUI: an active Learn run
is restored on the next launch, while a run waiting for Review remains a review record
instead of reopening a Runtime conversation.

## Give native agents reviewed memory

Install the connector once:

```bash
methodus setup --runtime all
methodus doctor
```

For substantial diagnosis, design, research, document, or presentation work, the
connector can call the read-only protocol:

```bash
methodus agent prepare \
  --goal "Diagnose abnormal device shutdown" \
  --budget 1200

methodus agent search \
  --query "previous shutdown reason" \
  --type knowledge,experience

methodus agent get knowledge/previous-shutdown-reason --facet execute
methodus agent related knowledge/previous-shutdown-reason
methodus agent status
```

The protocol returns bounded Markdown by default or JSON with `--format json`. It is
read-only by construction: it cannot create graph nodes, modify candidates, start a
Runtime, or read an ordinary coding transcript. If Methodus is unavailable, the
connector tells the native agent to continue normally.

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
└── connectors/             # Connector ownership/version metadata
```

Markdown/YAML is canonical for graph content. Personal and Team are separate roots;
Team changes remain normal Git work. Methodus can validate, show status and diff, and
write a local publish plan, but never silently commits, pushes, merges, or discards
changes.

## What Methodus is—and is not

### It is

- A maintainer-facing Learn and Review TUI
- A Markdown-first Knowledge / Method / Experience graph
- A local evidence and freshness tracker
- A bounded, deterministic Agent retrieval CLI
- A small connector Skill for Claude Code, Codex, and Cursor

### It is not

- A replacement coding agent or general chat client
- An MCP server or background daemon
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

The architecture diagram is available as [SVG](docs/architecture.svg) and
[PNG](docs/architecture.png).

## Development

```bash
cargo fmt --check
cargo test --workspace
cargo check --workspace
git diff --check
```

The active implementation intentionally stays narrow: one foreground TUI, one
focused Learn conversation, one official connector, and no hidden writes. New work
should preserve those boundaries.

## License

[MIT](LICENSE) © 2026 Steven Lee
