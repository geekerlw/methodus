# 09 — Product decisions

This file records decisions that should not be reopened casually during
implementation. Product rationale and detailed contracts live in `00`–`08`; this
file is the short architectural guardrail for contributors.

## D1 — Methodus is a maintainer knowledge studio

Methodus owns learning, curation, graph navigation, review, and Personal → Team
publication. It is not another coding agent and is not the place where an ordinary
developer's task is executed.

The main TUI view is intentionally an agent-like Learn conversation. Slash commands
switch to Knowledge, Method, Experience, Review, Graph, Team, Health, Help, and runtime
settings. A maintainer can do the full knowledge workflow without memorizing a long
CLI command list.

## D2 — Native runtimes remain native

Claude Code, Codex, Cursor, or another runtime owns ordinary task interaction, tools,
permissions, terminal rendering, and its own session lifecycle. Methodus does not
create a per-task workspace, copy knowledge into a project, proxy a runtime TUI, or
manage ordinary coding sessions.

Methodus may start a focused Learn run because that run is a maintainer operation. It
hands the terminal to the native runtime rather than proxying it, and restores the TUI
only when the runtime exits. The maintainer may explicitly select a bounded native
permission mode. The Learn runtime receives an explicit protocol and source roots,
and its executor ID is retained only to resume that Learn run.

## D3 — The connector is a Skill, not MCP

The first integration is one official connector Skill per supported runtime format.
The Skill calls a local, read-only `methodus agent` CLI. There is no MCP server in the
default product.

The connector contains instructions, not a copy of the graph. It can prepare/search/
get/relate/status, but cannot create, edit, approve, promote, deprecate, or publish
content. If Methodus is unavailable, the runtime continues the user task without
inventing context.

Methodus does not manage arbitrary runtime Skills. A Skill observed during a task may
be recorded as evidence in an Experience, so a maintainer can later make a cautious
recommendation; it is never auto-installed or auto-evolved.

## D4 — Markdown and Git are canonical

Knowledge, Method, Experience, relations, and source references are Markdown/YAML
files. Personal content is local; Team content is a normal Git-backed directory.
SQLite is a disposable projection and local operational index. Deleting or moving a
Markdown source must be reflected by the next maintainer sync.

Team promotion is an explicit Review action. Methodus validates, shows a publish plan,
and records the decision; normal Git tooling performs commit, push, merge, and remote
review. No hidden cloud account is required.

## D5 — Knowledge is faceted, not artificially atomized

The graph has three active node types:

- Knowledge — reusable conclusions, signals, decisions, constraints, or procedures;
- Method — a runtime-independent way to perform a class of work;
- Experience — a concrete case with evidence and outcome.

`Learn`, `Decide`, `Execute`, and `Evidence` are facets. 5W2H is a useful structure
inside `Learn`, especially for concepts and system flows, but it is not a universal
atom boundary. Split a node only when it has an independent scope, evidence lifecycle,
reuse value, or graph role.

## D6 — Learning is deliberate and review-gated

The Learn runtime must understand the goal, challenge scope and assumptions, inspect
evidence, seek counterexamples, ask consequential questions, separate fact/inference/
contradiction/unknown, and only then propose a CandidateSet.

CandidateSet output is never canonical. It may contain multiple typed candidates,
relations, unresolved questions, and contradictions. Methodus writes a durable run,
then the maintainer inspects, edits, splits, merges, rejects, commits, deprecates,
revalidates, or promotes through Review.

Canonical lifecycle:

```text
candidate → committed → stale → committed
                       ↘ deprecated
candidate → rejected
```

Source changes mark risk; they never silently rewrite knowledge.

## D7 — Personal and Team are the only visibility layers

Maintainers may experiment in Personal and explicitly promote selected, validated
content into a selected Team root. Consumer agents query both unless a scope filter is
provided. Candidate and rejected content is never consumer-visible; stale content is
only a warned historical hypothesis; deprecated content requires explicit history
mode.

The product does not need Face/domain experts to represent cross-domain work. A task
can retrieve several Methods, Knowledge nodes, and Experiences through graph relations
and bounded lexical selection.

## D8 — Safety and ownership are explicit

Runtime permission policy belongs to the runtime and user. Methodus displays and
persists the selected Learn mode, maps it to native controls, and must not silently
broaden permissions or invoke a bypass mode.

Only maintainers write canonical graph content. Consumer Agents are read-only. Setup
and connector updates are ownership-aware and must never overwrite an unrelated user
Skill. All destructive Review actions require a concrete target, rationale, and visible
confirmation.

## D9 — Performance is bounded before it is clever

Agent retrieval is deterministic for a fixed index revision, bounded by item count and
estimated tokens, and limited to one-hop relation expansion in protocol v1. Graph
visualization is a focused neighborhood, not a full force-directed rendering. Embeddings
or background services are considered only after real engineering-query evaluation
shows that lexical/tag/scope retrieval is insufficient.

## D10 — What success looks like

One maintainer can turn code, Git history, docs, logs, incidents, and design evidence
into reviewed, reusable engineering memory. A developer's native agent can retrieve
that memory at the moment of diagnosis, design, research, document, or presentation
work—without the developer learning Methodus, without a copied workspace, and without
Methodus taking over the actual task.
