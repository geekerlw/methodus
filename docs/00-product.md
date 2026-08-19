# 00 — Product Reference

This is the product contract: what Methodus owns, where it deliberately stops, and
how knowledge becomes useful work. Implementation details live in `01`–`05`.

## 1. Positioning

Methodus is a **local, graph-native knowledge and task-context system for agent
work**. It is not another coding agent, and it does not replace Claude Code, Codex,
or Cursor's native TUI.

For a task, Methodus selects a minimal set of Skills, Knowledge, and Experience from
the user's knowledge graph, compiles them into a temporary **Task Workspace** (also
called a context capsule), and hands the user into their chosen agent's native
environment. When work finishes, Methodus collects the outcome and turns verified
lessons into graph updates.

```text
knowledge graph → resolve and compile a task capsule → native Agent TUI
      ↑                                                     ↓
      └────── review outcome, experience, and new knowledge ┘
```

The agent is the **execution environment**. Methodus is the **context compiler,
knowledge graph, and learning loop**. It owns the before and after of an agent run;
the agent owns the human-in-the-loop conversation during the run.

### Product promise

> Every agent task starts from the relevant things the user or team has already
> learned, without turning the entire knowledge base into prompt tokens.

### Product boundaries

- **Methodus owns:** Markdown-first knowledge, graph indexing, task resolution,
  workspace/capsule generation, agent launch handoff, review, and experience
  distillation.
- **Agent runtimes own:** their terminal UI, conversation, tools, native permissions,
  session persistence, and execution behavior.
- **Methodus may optionally manage a runtime headlessly** for automation or a
  structured workflow, but that is a secondary mode—not the default product surface.

## 2. Principles

1. **Graph-native, Markdown-first** — independent Markdown files are human-owned
   source data; explicit links and SQLite indexes make them a navigable knowledge
   graph. The graph must still work without a proprietary database or UI.
2. **Atomic knowledge, contextual use** — knowledge is stored as small reusable
   units, but injected as task-specific facets rather than whole documents.
3. **Native interaction stays native** — Methodus never proxies or screen-scrapes an
   agent's TUI in the normal path.
4. **Minimal, explainable context** — every injected item has a rationale, a bounded
   token budget, and a lazy reference for deeper reading.
5. **Evidence-first** — an agent output is not automatically fact. Observation,
   Experience, Hypothesis, candidate Knowledge, and committed Knowledge stay
   distinct.
6. **Human-owned learning** — learning can be initiated deliberately, not only
   inferred from completed coding work. Global knowledge changes are reviewable.
7. **Executor-agnostic** — the graph and capsules are portable; adapters only render
   them for Claude Code, Codex, Cursor, or a future runtime.
8. **Local and auditable** — durable content is readable on disk; lifecycle,
   resolution, injection, and review decisions are queryable and recoverable.

## 3. Domain model

### 3.1 Knowledge Graph

The Knowledge Graph is the primary long-lived product asset. It follows the useful
parts of an Obsidian/LLM-wiki model: every node is a readable file with stable ID and
links, while Methodus indexes those links, metadata, and use history for resolution.

```text
Knowledge ──requires/contrasts/extends──▶ Knowledge
    │ uses                                  ▲ validates/refutes
    ▼                                       │
Skill ──recommended for──▶ Task ◀──produced─┘ Experience
    │                                      │
    └──────────────uses evidence───────────┘ Artifact
```

Node types:

| Node | Meaning |
|---|---|
| **Knowledge** | An atomic concept, fact, decision rule, or reusable method. |
| **Skill** | An executable procedure/package, normally a `SKILL.md`. |
| **Experience** | What happened in one task or learning session, with evidence and outcome. |
| **Task** | A concrete work objective and its capsule/result. |
| **Artifact** | A source document, code diff, test run, PR, URL, or other evidence. |
| **Face** | An optional domain lens over a graph region—not an owner of knowledge and not a model persona. |

Edges have a type, direction, source, confidence where relevant, and provenance. A
link can be authored in Markdown frontmatter/body; Methodus indexes it and can propose
missing links, but does not silently invent authoritative relationships.

### 3.2 Face

A **Face** is a reusable expert lens: its focus tags, preferred Methods, quality bar,
and starting graph queries. Examples are `network`, `security`, or `product-design`.
It helps resolve a task, but it does not contain a private copy of the world's
knowledge. A Knowledge node may be visible through several Faces.

This replaces the older “Face as a folder that owns knowledge” interpretation. Faces
remain useful for intent routing; the graph remains the source of knowledge truth.

### 3.3 Knowledge and its facets

A Knowledge item is an independent file with a stable ID, sources, scope, confidence,
links, and a short agent summary. Its human-readable learning facet commonly uses
**5W2H**; that is a template, not the entire data model.

```yaml
---
id: knowledge/payment-idempotency
title: Payment callback idempotency
kind: concept
status: committed                 # candidate|committed|conflicted|rejected
summary: Deduplicate callbacks by a stable event ID before business side effects.
scope: payment callbacks in services with at-least-once delivery
confidence: 0.9
tags: [payment, reliability]
links:
  requires: [knowledge/database-unique-constraint]
  applied_by: [experience/payment-webhook-2026-08]
  used_by: [skill/payment-change-checklist]
sources: [artifact/provider-webhook-docs]
---

## Learn (5W2H)
## Decide
## Execute
## Pitfalls and counterexamples
## Evidence
```

The same item can expose several **facets** without being duplicated:

| Facet | Consumer | Content |
|---|---|---|
| Learn | a person and learning runtime | 5W2H, examples, analogy, quiz/recall prompts |
| Execute | an agent task capsule | short rule, steps, constraints, pitfalls |
| Decide | a task planner | applicability, alternatives, trade-offs |
| Evidence | reviewers | sources, confidence, contradictions, validation history |
| Graph | Methodus | typed links, tags, scope, use and outcome history |

The resolver normally injects only the **Execute** facet plus a path to the full item.

### 3.4 Skill, Method, Experience, Artifact

- A **Skill** is a reusable executable capability (`SKILL.md`, optional scripts and
  references). It may depend on Knowledge nodes and have runtime compatibility.
- A **Method** is a high-level, verifiable procedure for a problem class. It selects
  Skills; it is not a large prompt.
- An **Experience** records a specific attempt: context, decisions, evidence,
  result, reusable lesson, and the Knowledge it validated, refined, or contradicted.
- An **Artifact** is immutable or versioned evidence. It is linked rather than copied
  into a prompt unless the task explicitly needs its content.

### 3.5 Task and Task Workspace

A **Task** is a goal with constraints, selected context, launch history, result, and
review state—not merely a prompt.

A **Task Workspace** is an immutable, auditable task package compiled from the graph.
It is not necessarily the agent process's current working directory. In the default
native-handoff mode, the agent launches in the user's repository while the capsule
lives beside it under Methodus storage and is referenced by a concise startup brief.

The compiler produces:

- goal, constraints, scope, and acceptance criteria;
- a rationale for every selected Skill/Knowledge/Experience item;
- compact execution facets under a token budget;
- lazy references to full Markdown and source artifacts;
- runtime-specific launch instructions; and
- a result/retrospective template for the return trip.

### 3.6 Learning Session

A **Learning Session** is a first-class task type. The user can say “learn this
concept” without attaching it to a coding task. Methodus compiles a learning capsule:
the learning Skill, relevant prerequisite/neighbor knowledge, sources, and a
learning goal. The selected runtime uses its native TUI to explain, question, test
understanding, and draft a knowledge item.

The durable output is a candidate atomic Markdown item (usually with a 5W2H learning
facet), links to related nodes, sources, and an Experience record of what was learned.
It becomes committed only through review.

## 4. The loops

### 4.1 Work loop: compile → hand off → return

```text
user task
  → resolve graph nodes / budget / rationale
  → compile Task Workspace
  → launch chosen runtime in native TUI
  → user and agent work without Methodus in the conversation path
  → runtime exits or user returns to Methodus
  → capture outcome, evidence, and short retrospective
  → create Experience; queue review/distillation
```

**Native handoff is the default.** Methodus either temporarily yields the terminal to
the executor or opens a configured terminal/tmux pane. It records launch and return;
it does not parse ANSI, relay messages, or attempt to infer every conversational turn.

An optional **managed execution** mode may use structured CLI/SDK output for a
non-interactive task, policy-controlled automation, or observability. It must use the
same capsule and produce the same return artifacts. It is never required for the
knowledge graph to function.

### 4.2 Deliberate learning loop

```text
learn intent + source(s)
  → resolve prerequisite/neighbor knowledge
  → compile Learning Workspace + learning Skill
  → native runtime learning session
  → candidate 5W2H knowledge item + links + evidence
  → review / commit / reject / revise
  → graph indexes update
```

### 4.3 Experience and graph-evolution loop

```text
task or learning return
  → Experience + linked evidence
  → identify candidate lessons, contradictions, and missing links
  → propose Knowledge / Skill / Method / Face-lens refinements
  → evidence and conflict checks
  → human review
  → graph update and future-resolution feedback
```

Injected context has an outcome signal: useful, unused, misleading, or unknown. This
lets Methodus improve selection and reduce future token waste, rather than rewarding
mere retrieval.

## 5. Context and token policy

Methodus optimizes **total task cost**, not only the first prompt:

```text
total cost = startup context + on-demand reads + retries/incorrect work + human correction
```

The compiler applies three layers:

1. **Base brief:** task objective, hard constraints, acceptance criteria, and project
   pointers. Always small.
2. **Selected facets:** only high-confidence, high-value Knowledge/Skill/Experience
   fragments for this task; normally a few items, not a corpus dump.
3. **Lazy references:** absolute/local paths to full notes and artifacts, read only
   when the agent needs them.

Every context item records selection reason, estimated size, priority, and outcome.
The user can inspect and remove an item before launch. A content budget is enforced by
the compiler; it never relies on the runtime to truncate arbitrarily.

## 6. Trust, permissions, and review

Methodus does not take over an executor's native permissions in native-handoff mode.
It supplies a policy recommendation and the scoped workspace; the executor continues
to show its normal permission prompts. Methodus governs its own writes:

- committing/replacing global Knowledge, Skills, Methods, and Face lenses;
- installing or updating Methodus-owned Skill packages;
- fetching web sources or importing external artifacts;
- deleting archived task workspaces.

Candidate knowledge never silently overwrites a committed item. Conflicts retain both
claims, their evidence, and the resolution decision.

## 7. Product surface

Methodus has a focused TUI for **graph browsing, task compilation, review, history,
and setup**. It is not a second agent chat.

- **Graph:** browse/search nodes, links, sources, facets, and recent use.
- **New task / learn:** state an objective, inspect proposed context, choose a runtime,
  then launch/handoff.
- **Review:** commit, revise, link, or reject candidate knowledge and experiences.
- **History:** reopen a capsule, see outcome and context rationale, then launch a
  follow-up task if needed.
- **Setup:** projects, packs, runtime launchers, budgets, and policies.

The detailed interaction contract is in [`05-tui.md`](./05-tui.md).

## 8. Explicitly out of scope (v1)

- Replacing, embedding, or screen-scraping Claude Code/Codex/Cursor's interactive UI.
- A single super-prompt or permanently injected knowledge corpus.
- A proprietary-only graph that cannot be read or edited as Markdown.
- Fully autonomous background execution or unbudgeted model calls.
- Silent promotion of global Knowledge, Skill, Method, or Face changes.
- Automatically cloning, committing, pushing, or synchronizing user repositories.
- Cloud accounts, team collaboration services, and a complex web/desktop UI.

## 9. Acceptance scenarios

**A — Native task handoff.** The user selects a repository task. Methodus proposes a
small capsule with two Knowledge execute facets and one Skill, explains why, and
launches Claude Code in its normal TUI. The agent edits the repository without a
Methodus conversation proxy. On return, the user records the result and Methodus
creates an Experience linked to the capsule.

**B — Learning a concept.** The user selects `Learn: payment callback idempotency`
with a source document. The runtime conducts a learning conversation and produces a
candidate 5W2H note. In review, the user connects it to a prerequisite and a
payment-change Skill before committing it.

**C — Token restraint.** A task has ten potentially relevant graph nodes. The compiler
injects the minimal brief and three execute facets, keeps the other seven as lazy
references, and records the selection rationale and budget. The user can inspect this
before launch.

**D — Portable runtime.** The same capsule launches in Codex instead of Claude Code;
the graph files, selected context, result format, and learning loop remain unchanged.

## 10. Success criterion

Methodus succeeds when a user can keep their preferred agent workflow while their
knowledge graph makes each new task start from verified, task-relevant learning—and
each work or learning session makes the graph more useful for the next one.
