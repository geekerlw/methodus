# 04 — Implementation Roadmap

The implementation order follows the product bet: first prove that a graph-compiled
context capsule improves a user's existing Claude Code/Codex workflow. Do not first
build a replacement agent chat, a full observability system, or autonomous agents.

## Design decisions carried into every milestone

1. **Native handoff is the default.** Methodus creates the context and returns after
   work; Claude Code/Codex/Cursor keep their own interactive TUI.
2. **Task Workspace means capsule, not copied repository.** The user project remains
   the agent `cwd`; Methodus stores an immutable, auditable task package separately.
3. **Markdown graph first.** SQLite indexes and queries graph files; it does not
   replace them as the knowledge source of truth.
4. **Learn is first-class.** A deliberate learning session produces a candidate atomic
   knowledge item, not an unstructured chat transcript.
5. **Managed adapters are optional.** JSON events and interactive approval protocols
   are useful later, but are not the prerequisite for daily value.

## Spike (done — 2026-08-14)

Verified that Claude Code, Codex, and Cursor expose structured non-interactive modes
and resume mechanisms. Findings are retained in
[`01-runtime-adapters.md`](./01-runtime-adapters.md). This de-risks optional managed
execution; it does not change the default native-handoff boundary.

## M0 — Graph and capsule foundations

**Goal:** compile and inspect a task package without launching an agent.

- Define domain types and state machines for graph nodes, typed edges, task kind,
  workspace/capsule, context selection, launch, and review.
- Create the Markdown graph layout and SQLite indexing/migrations from
  [`03-data-model.md`](./03-data-model.md).
- Implement stable IDs, frontmatter validation, authored-link indexing, and a small
  seed graph with Knowledge, a Face lens, a Method, and a Skill.
- Implement the Workspace Compiler: task brief, selected Execute facets, lazy
  references, selection rationale, hashes, and a hard context budget.
- Provide a read-only TUI preview of a graph node and a proposed capsule.

**Acceptance:** a task against a sample project resolves to a capsule. The user can
inspect every injected item, its reason, estimated token size, and the full referenced
source before anything is launched.

## M1 — Native Claude Code handoff

**Goal:** prove the main before → native Agent TUI → after loop.

- Register a project directory and a local Claude Code installation.
- Launch Claude Code in its normal TUI at the project root, with a concise capsule
  brief and paths to the immutable Methodus workspace.
- Implement a reliable handoff target: terminal suspension/restore or a tmux pane.
  Do not parse ANSI, relay prompts, or re-render the Claude conversation.
- Record launch/return state; on return present an outcome and short retrospective
  form, then create an Experience linked to the task and selected context.
- Preserve all workspaces/capsules for audit; add explicit archive rather than default
  deletion.

**Acceptance:** a user starts a repository task in Methodus, works entirely in native
Claude Code, returns, and sees an Experience linked to exactly what context was
recommended. The source repository did not receive a one-off Methodus configuration
file.

## M2 — Review and deliberate learning

**Goal:** make knowledge acquisition a daily workflow, not only passive distillation.

- Add `Learn` task kind: topic, source attachments/URLs, desired depth, and selected
  learning Skill.
- Resolve prerequisite and neighbor graph nodes into a learning capsule.
- On return, parse/import the candidate output into an atomic knowledge file template
  with Learn (5W2H), Decide, Execute, Evidence, and typed-link sections.
- Add review actions: commit, revise, reject, defer, and connect to an existing node.
- Detect conflicts without overwriting committed Knowledge.

**Acceptance:** the user learns a concept through native Claude Code or Codex, reviews
a candidate 5W2H Knowledge node, adds a prerequisite link, commits it, and can browse
the link in Methodus.

## M3 — Graph and task-control TUI

**Goal:** make Methodus comfortable as a knowledge/task control plane, not a second
agent chat.

- Graph search and node detail: title/summary, facets, typed backlinks, sources, and
  recent task use.
- Task/learn composer and context-inspection flow; launch mode and runtime choice.
- Review inbox for candidate nodes, graph-edge proposals, and returned task outcomes.
- History view for capsules, selections, launch state, and follow-up creation.
- Setup for projects, packs, runtimes, budgets, and terminal handoff targets.

**Acceptance:** all before/after work—create task, inspect context, launch, return,
review, and browse the resulting graph—is possible without Methodus impersonating an
agent chat.

## M4 — Context quality feedback

**Goal:** improve recommendations based on evidence rather than growing prompts.

- Record context outcome as useful, unused, misleading, or unknown during return/review.
- Use explicit tags, typed links, scope, task results, and selection history for the
  first deterministic resolver; add semantic ranking only as a bounded helper.
- Detect recurring Experience patterns, contradictions, and missing links; propose
  candidate Knowledge/Skill/Method/Face-lens changes for review.
- Add per-task and per-day context/model budgets, injection-use reports, and lazy
  reference metrics.

**Acceptance:** after several related tasks, Methodus can explain why it selected a
knowledge facet, show whether it helped, and avoid repeatedly injecting items marked
unused or misleading.

## M5+ — Optional managed execution and scale

Only after native handoff is a daily habit, add optional capabilities where they solve
a demonstrated problem:

- Managed Claude structured execution for a non-interactive workflow.
- Codex `exec` and, later, app-server integration for structured approvals/interrupt.
- Automatic outcome extraction from stable artifacts (diff, tests, task template),
  always with review.
- FTS/semantic retrieval, graph visualization, pack sharing conventions, and richer
  source import.
- A daemon/client split only if work must continue without an open Methodus process or
  multiple clients must control one graph.

## Explicitly deferred

- A Methodus-hosted agent transcript/composer or terminal screen scraping.
- Autonomous cron/RSS/heartbeat calls to an executor.
- Automatic global skill/knowledge writes.
- A cloud collaboration product, desktop shell, or mandatory vector database.

## Definition of done

A feature is done only when it has: explicit file schema and domain transitions;
success/failure/cancel/recovery behavior; a bounded context/token policy; a review and
evidence boundary for durable graph writes; integration coverage against at least one
real runtime handoff where relevant; and no mutation of global Agent configuration.
