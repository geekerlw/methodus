# 00 — Product Reference

The **product contract**: what Methodus is, its domain model, the three loops, the
event catalog, the permission model, the CLI/TUI surface, what is explicitly out of
scope, and the acceptance scenarios. This is the *what & why*; the *how* lives in
`01`–`04`.

> This document replaces the original `PROJECT_SPEC.md`. Product intent is preserved
> here; stack- and mechanism-level statements that later design decisions corrected
> (TypeScript, a `methodusd` daemon, a PTY session layer) have been dropped in favor
> of the decisions recorded in `02-architecture.md` and `01-runtime-adapters.md`.

## 1. Positioning

Methodus is **not** another coding agent, and not a fixed multi-agent team UI. It is a
**Persistent Personal Expert System**: for a given task it selects the right expert
perspective, method, skills, and executor; prepares the task environment; supervises
the result; and distills vetted experience into reusable knowledge and better methods.

> Methodus does not have one fixed expertise. It learns better methods for the problems
> you care about.

The executor (Claude Code / Codex / Cursor) is the **hands**; Methodus is the
**brain** — it selects, prepares, observes, evaluates, and remembers. Methodus must not
reimplement an executor by copying its logic.

Implementation priorities:

- Core state and business rules live in Methodus code, not hidden in prompts.
- The executor is replaceable; Methodus is bound to no single model or CLI.
- All long-lived state is auditable, recoverable, and human-inspectable.
- Task workspaces are strictly separated from long-term expert/project memory.
- Background work is event-driven, pausable, and budgeted — never an unconditional
  drain on model calls.

## 2. Principles

1. **Persistent** — the process runs long-term, but background work is scheduler-,
   budget-, and policy-controlled.
2. **Personal** — knowledge, experience, and project context belong to the user and
   are stored locally by default.
3. **Adaptive** — no permanently bound role; each task loads one or more Faces on
   demand.
4. **Evidence-first** — observation, experience, hypothesis, candidate knowledge, and
   committed knowledge are layered; a single model output is never treated as fact.
5. **Composable** — Skills, Methods, Faces, and Runtimes are discoverable, composable,
   replaceable resources.
6. **Human-in-the-loop** — dangerous actions, knowledge promotion, proactive questions,
   and external network access are gated by policy.
7. **Workspace-first** — every execution has an explicit workspace; global and
   task-specific capabilities stack without disturbing the user's environment.
8. **Prompt is an interface, not a database** — prompts carry resolved context,
   method steps, and output constraints only; never state machines, permission
   decisions, or knowledge promotion.

## 3. Domain abstractions

### 3.1 Engine (System / Core)
The business core in the persistent process: state, planning, orchestration, policy,
events, and queues. It may call an LLM but is not identified with any one LLM and does
not require an agent CLI to start.

### 3.2 Task
One goal the user wants accomplished: natural-language goal, project, constraints,
approval requirements, deliverables. A Task has a lifecycle and an event record — it is
never just a prompt string.

Minimum shape:

```yaml
id: task_20260814_001
title: Analyze HTTPS video latency
request: <raw user request>
project_id: project-a
status: queued   # queued|planning|running|waiting_user|reviewing|completed|failed|cancelled
selected_faces: []
selected_methods: []
selected_skills: []
runtime: claude-code
workspace_id: ws_task_20260814_001
created_at: 2026-08-14T00:00:00Z
```

### 3.3 Face
A **domain-expert state snapshot** — not a separate agent process, not a fixed persona.
It defines a domain's identity, focus, knowledge entry points, experiences,
hypotheses, question pool, methods, and skill dependencies. Examples: Network,
Performance, Kernel, Storage, Security. A complex task may compose several Faces; the
MVP supports a single primary Face with manual multi-Face context.

Persistent state includes: `identity`, `knowledge`, `experiences`, `hypotheses`,
`questions`, `methods`, `skills`. A Face is not bound to a specific runtime — the same
Face can run under different executors across tasks.

### 3.4 Method
A **step-by-step methodology** for a class of problem — answers "how should this be
done". A verifiable procedure, not a generic prompt: preconditions, steps, evidence
requirements, failure branches, completion criteria, recommended skills, output format.
It may reference Skills but should not inline them.

```yaml
id: tcp-latency-investigation
version: 1.0.0
intent_tags: [network, tcp, latency]
preconditions: [target environment reachable]
steps:
  - check connectivity and topology
  - collect socket, retransmit, and capture evidence
  - rule out CPU, TLS, application-layer causes
evidence_required: [commands, observations, timestamps]
success_criteria: [evidenced root cause or explicit unknowns]
recommended_skills: [tcp-debug, packet-analysis]
```

### 3.5 Skill
A reusable capability package the user already has or Methodus discovered, usually
following the `SKILL.md` convention with optional references/scripts. Methodus'
responsibility: discover, index, filter, version-resolve, and expose Skills in the
workspace on demand; the executor loads and uses them by its own native rules.

Skill source priority: (1) user-specified, (2) current project, (3) user global skill
dir, (4) Methodus built-in, (5) approved auto-generated. Default to read-only symlink
or controlled materialization; any write into the global skill dir requires human
approval.

### 3.6 Knowledge
A reusable, relatively stable, sourced, confidence-scored entry. Records source
(experience, user answer, doc, research), evidence, update time, applicability, and
conflict state. Promotion path:

```text
Observation → Experience → Hypothesis / Candidate Knowledge
           → evidence check → conflict check → committed Knowledge
```

A single execution result never directly overwrites existing knowledge; conflicts enter
a review state.

### 3.7 Experience
A structured record of what happened in one task/experiment: commands, output
summaries, changes, conclusions, failure reasons, reusable lessons. It is **not**
automatically equivalent to general knowledge.

### 3.8 Question
A knowledge gap or fact to confirm. Has reason, linked task/experience, impact,
importance, urgency, recurrence, confidence, and status
(`pending|asked|answered|snoozed|dismissed`). Proactive asking must satisfy policy
thresholds, cooldowns, and user preferences — no chatty interruptions. Answers are
recorded as sourced Project or Face Knowledge.

### 3.9 Hypothesis
A judgment currently plausible but under-evidenced: evidence list, counter-evidence,
confidence, validation plan, lifecycle. Promoted to Knowledge only after validation;
when refuted, kept as `rejected` with history rather than deleted.

### 3.10 Evolution
The controlled improvement of Faces, Methods, Skills, and Knowledge — not free-form
model file edits. Auto-generated upgrades enter as `candidate`, pass rule checks, wait
for human approval where required, then become `active`. Every upgrade carries diff,
source, rationale, test/validation results, and rollback info.

## 4. The three loops

### 4.1 Execution Loop
```text
user Task
  → parse goal, project, constraints
  → resolve Face / Method / Skill / Knowledge
  → produce execution plan
  → request necessary approvals
  → create isolated workspace
  → start runtime session
  → stream events, handle permissions/questions
  → complete / fail / pause / hand off to human
  → form Result + Experience
  → trigger Learning Queue
```
Plans, sessions, workspaces, and final results must persist and survive a restart.

### 4.2 Learning Loop
```text
task completed / threshold / scheduled event
  → read Experience
  → extract observations and candidate insights
  → detect recurring patterns, knowledge conflicts, gaps
  → generate Candidate Knowledge / Hypothesis / Method / Skill
  → rule + evidence checks
  → auto-commit low-risk items or request human review
  → update Face / Project indexes
  → record Evolution Event
```
Background learning is always queued work — never `while true: call_llm()`.

### 4.3 Curiosity Loop
```text
Experience + Knowledge + Hypothesis
  → Knowledge Gap Manager
  → compute question value
  → dedupe, merge, rank
  → Ask / Research / Snooze / Dismiss
  → save answer + evidence
  → update knowledge or hypothesis
```
Question value combines at least `importance * frequency * impact * uncertainty`, with
an interruption budget, cooldowns, and project relevance.

## 5. Permission, approval, human-in-the-loop

Policy distinguishes at least: read-only file access; project-dir writes; shell
execution; network/research; config change or skill install; global knowledge/skill
writes; process deletion or forced termination.

Default: reads and low-risk analysis auto-run; destructive commands, external sends,
global writes, unknown commands, and high-risk permissions **pause the session** and
raise an approval request. The user may `approve once`, `approve session`, `deny`, or
`abort`. Methodus only decides policy and forwards — it never bypasses the executor's
own permission prompts. Every approval records subject, scope, decision, actor,
timestamp, session id.

(For how each executor surfaces approvals and how Methodus grants them, see
`01-runtime-adapters.md` §3–§7.)

## 6. Event model

Event handlers must be **idempotent** (keyed on event id): a replayed event never
double-commits knowledge or repeats a side effect. Event names include:

```text
task.created  task.planned  task.resolved  task.started  task.waiting_user
task.completed  task.failed  task.cancelled

workspace.created  workspace.cleaned

session.spawned  session.attached  session.detached  session.output  session.input
session.status_changed  session.permission_requested  session.question_requested
session.exited

experience.created  learning.job_queued  learning.job_started
learning.candidate_created  knowledge.committed  knowledge.conflict_detected
hypothesis.created  hypothesis.validated
question.created  question.asked  question.answered
evolution.proposed  evolution.approved  evolution.rejected
approval.requested  approval.resolved
```

## 7. Scheduler & Learning Queue

Trigger types: **event-driven** (task done, user answer, session failure),
**threshold** (N experiences accrued, a repeated unknown), **scheduled/idle** (fixed
time or user idle). A queue job carries at least: `kind`, `priority`, `dedupe_key`,
`input_refs`, `status`, `attempts`, `not_before`, `budget`, `requires_approval`.

MVP learning jobs: `extract_experience`, `detect_gaps`, `propose_knowledge` (never
auto-overwrites existing entries). Deferred: auto-research, Method synthesis, Skill
generation, cross-Face debate. All jobs are pausable, retryable, cancelable, and
recoverable after a restart.

## 8. CLI / TUI surface

CLI command name is `methodus`. Illustrative commands:

```text
methodus init | doctor
methodus task create "<goal>" [--project PATH] [--face NAME] [--method NAME]
methodus task list | show | cancel | retry
methodus run <task-id>
methodus session list | show | input | cancel | kill
methodus face list | show | activate
methodus method list | show
methodus skill list | inspect | scan | resolve
methodus knowledge list | search | show | review
methodus experience list | show
methodus question list | answer | snooze | dismiss
methodus approve <approval-id>
methodus workspace show | open | prune
methodus events tail
methodus tui
```

MVP must implement at least: `init`, `doctor`, `task create/list/show`, `run`,
`session list/show/cancel`, `face list/show`, `experience list/show`, `events tail`,
`tui`. Mutating/session control is primarily driven from the TUI (single-process
model — see `02-architecture.md` §4); read-only queries are safe from a second
terminal.

The TUI is the first-class UI and must close the core loop in the terminal. Suggested
pages: Dashboard (queue / current task / pending approvals / pending questions),
Tasks, Session (live transcript, input, approval), Faces, Queue, Review (candidate
knowledge, conflicts, Evolution diffs). Keyboard actions must be visible, cancelable,
recoverable; the TUI is part of the same process as the Engine (kept open in `tmux`).

## 9. Workspace & isolation (product rules)

Each task gets an isolated workspace. Rules: Face and Project memory are not written
back through workspace temp files; workspaces are retained by default for audit
(cleanup is an explicit command); global skills/MCP/user tools stay visible while the
workspace only adds task-specific capabilities; writes to the user's project dir are
bounded by the project root and gated by policy; never copy the entire knowledge base
to the executor — inject only the minimal task-relevant context. (Concrete layout:
`03-data-model.md` §5.)

## 10. Explicitly out of scope (v1)

- No fixed multi-agent team topology.
- No single super-prompt holding all logic.
- No forcing ACP/A2A as the only backend protocol.
- No reimplementing the executor's permission/skill/MCP mechanisms.
- No auto-modifying the user's global skill/MCP/shell config.
- No auto-generating large volumes of expert knowledge without real Experience.
- No complex web UI, cloud accounts, remote collaboration, vector search, or
  cross-device sync.
- No unbudgeted background scheduler continuously calling the LLM.

## 11. Development constraints

1. Define types, state machines, and events before LLM integration.
2. All side effects go through injectable interfaces (testable, runtime-swappable).
3. File paths are validated against workspace/project roots to prevent escape.
4. Events, approvals, transcripts, and knowledge changes are all auditable.
5. Missing/incompatible executor, or a broken session, must yield a diagnosable error.
6. Every background job has timeout, retry cap, concurrency cap, and a cancel path.
7. Business state uses explicit enums and transitions — never natural-language
   inference.
8. Any auto-promote / auto-write / auto-execute behavior has test coverage and a policy
   switch.
9. Implement one real working runtime first, then expand.
10. Every phase ends with an end-to-end acceptance scenario, not just prompt output.

## 12. Acceptance scenarios

**A — Single-Face task.** Init Methodus; register a Network Face and a local
`SKILL.md`; create a project and Task; the resolver selects the Face + a Method + the
Skill; the builder creates an isolated workspace preserving global capabilities and
adding the project Skill; a configured runtime starts; the user leaves and returns
later; the session completes and persists transcript, result, and Experience.

**B — Approval.** When the executor requests a file write or high-risk command, the
session enters `waiting_user` and the TUI shows an explicit scope; approve → continue;
deny → safe return; both are logged.

**C — Learning & questions.** A repeated unknown across Experiences generates a
Question; the user answers in the TUI; the answer becomes sourced candidate Knowledge;
an existing conflicting entry is never silently overwritten.

**D — Crash recovery.** The process exits during a session and restarts; it recovers
task state, discovers the existing executor session, shows the transcript, and lets the
user reattach or cancel.

## 13. Final success criterion

Methodus is judged not by whether it writes code better than Claude Code, but by
whether it delivers this closed loop:

```text
one user
  → keeps using the same Methodus
  → gets the right Face / Method / Skill per task
  → executes safely in an isolated workspace
  → can leave anytime and recover the background session
  → task results become auditable Experience
  → important experience becomes knowledge and better methods
  → high-value questions are asked only when truly needed
  → over time it becomes a personal expert system that knows this user and their
    projects better
```

This loop is the core product value and the highest priority for all downstream
technical trade-offs.
