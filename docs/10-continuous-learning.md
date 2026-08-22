# 10 — Continuous learning

**Status:** implemented in `methodus-core` and the TUI. Sections marked *planned*
are deliberate gaps, not omissions.

Everything up to `09` describes Methodus as a surface a maintainer drives: you type a
learning goal, a runtime investigates, you review what comes back. This document adds
the other half — the work Methodus does when nobody is watching — and defines the
objects, the schedule, and the boundary that keeps unattended work trustworthy.

An earlier draft of this document specified a Tauri desktop application. That surface
was dropped; see [D11 in `09-decisions.md`](./09-decisions.md). The product thinking
here survived the deletion, because none of it depended on a window.

## 1. Product intent

Methodus is a **human-governed continuous-learning system and trusted memory layer for
agents**.

- A person defines a learning goal, its authorized sources, cadence, budget, and review
  policy.
- Methodus schedules research, source checks, revalidation, and consolidation.
- A native runtime performs each investigation headlessly, with no terminal attached.
- Runtime output always enters a review-only CandidateSet.
- A person approves, rejects, edits, or asks for another round.
- Only reviewed Personal or Team content reaches consumer agents.

The central invariant is:

> Automation may plan, schedule, investigate, revalidate, synthesize, and propose.
> Only human Review may establish or publish canonical knowledge.

Continuous learning does not make Methodus autonomous. It makes the maintainer's
attention the scarce resource the system is designed around: unattended turns exist so
that a person is asked fewer, better questions, later.

## 2. Surfaces

| Surface | Primary user | Responsibility |
|---|---|---|
| TUI | Knowledge maintainer | Goals, schedules, attention queue, Review, Library, Team, Health |
| Agent CLI | Consumer agent | Stable, bounded, read-only knowledge retrieval |

There is one maintainer surface. Scheduling, notification, and review all happen in the
terminal the maintainer already keeps open, which is also the only place a native
runtime can be handed a real TTY.

Policy lives in `methodus-core` (`crate::learning`), not in the TUI: which turn is due,
whether a budget is exhausted, whether a Goal is blocked on a person, and what a turn's
outcome means are all decided below the surface. A second surface, if one is ever
built, renders the same decisions rather than re-deriving them.

## 3. The Learning Goal

A Goal is the stable object above individual runs — a standing question rather than a
task.

```yaml
title: Understand Rust async runtime behavior
prompt: >
  Maintain current, evidence-backed knowledge of cancellation, scheduling,
  production failure modes, and applicability in our services.
sources:
  - docs/async
  - src/runtime
runtime: claude-code
permission_mode: plan
cadence: weekly              # investigate
review_cadence: weekly       # re-check what is already published
summary_cadence: monthly     # consolidate
source_check_cadence: daily  # detect drift
quiet_hours:
  start: "22:00"
  end: "07:00"
budget_usd: 20
review_policy: human_required
enabled: true
```

This is the literal editing surface: the Goal management panel (`/goal` with no text;
`/goals` is an alias) renders it into `$EDITOR` and parses what comes back. Identity,
timestamps, and computed due times are absent on purpose, so an edit cannot corrupt them.

It is not, however, how a Goal is *created*. Creation uses `/goal <text>` and takes one
stretch of natural language, exactly as a Learn run does; `@` attachments become the
Goal's authorized sources, the title is its first sentence, and every policy field above
takes its default. Requiring four cadence decisions before someone has stated an
objective inverts the order people actually think in, and makes the standing form of the
product harder to start than the one-off form.

Every cadence accepts `manual`, `daily`, `weekly`, `monthly`, or `every:<hours>`.
`manual` means the turn only happens when a person asks for it.

### 3.1 Four cadences, not one

The four kinds of work have genuinely different rhythms, and collapsing them loses the
distinction that makes scheduled learning useful:

| Work | Question it answers | Typical cadence |
|---|---|---|
| Learn | What don't we know yet? | weekly |
| Review | Is what we published still true? | weekly |
| Summary | What does all of this add up to? | monthly |
| Source check | Did the evidence move under us? | daily |

Each gets its own brief, so a review turn does not silently become another initial
investigation. A source check reads the index and never occupies a runtime, so it runs
even while the Goal is busy or waiting on a person.

### 3.2 Scheduling policy

One tick, evaluated whenever a person does something that could make work due and at
most a minute after otherwise:

1. Disabled Goals are skipped entirely.
2. A Goal inside its quiet hours is deferred **without** advancing its due timestamp,
   so deferred work runs when the window closes instead of slipping a whole cadence.
3. A Goal whose runtime session is already held — by a foreground handoff or an
   in-flight background turn — is skipped, and its turn stays due.
4. A Goal with an open question is given no new work. Answering unblocks it.
5. At most one runtime turn is selected per Goal per tick, in Learn → Review → Summary
   order. A Goal owns one executor session at a time.
6. A selected turn advances its schedule before it launches, so the same turn cannot be
   dispatched twice.
7. If the Goal's month-to-date spend has reached its budget, the turn is blocked and
   reported rather than run.

Budget accounting is per Goal per calendar month, updated atomically as each turn
reports its cost. Runtimes that do not report a monetary cost stay effectively uncapped
until their adapter can supply one; this is a known gap, not a design choice.

## 4. Runs and turns

| Object | Purpose |
|---|---|
| `LearningGoal` | Long-lived human intent and policy |
| `GoalRun` | The link binding one Learn run to its Goal and work kind |
| `GoalUsage` | Month-to-date spend for one Goal |
| `HumanAttention` | A question, permission request, or runtime learning recommendation needing maintainer action |
| CandidateSet | Review-only output of a completed turn |

A turn ends in exactly one of three dispositions:

```text
                    ┌─ failed ─────────→ the schedule continues; nothing is recorded
scheduled → running ├─ awaiting_input ─→ a question enters the attention queue
                    └─ completed ──────→ candidates enter Review, or nothing did
```

`completed` means the turn ended. It does not mean anything was approved.

Every transition is durable before a notification is emitted, so a restart reconstructs
state from Methodus records rather than from whatever the surface remembered.

## 5. Execution and session ownership

Unattended turns run headless — Claude Code `--print --output-format stream-json`,
Codex `exec --json` — with a predeclared permission profile and the Goal's source roots.
There is no terminal, no rendering, and no way for a background turn to prompt anyone.

That constraint is what makes the attention queue necessary. A turn that reaches a
decision it cannot make alone stops and asks, in a structured envelope Methodus parses:

```json
{"outcome":"needs_input","question":"...","context":"why this blocks reliable learning"}
```

Answering is the same act as resuming. A maintainer types an answer in `/attention`;
Methodus resumes **that** executor session natively, handing the real terminal to the
runtime with the answer as the next message. The question is only resolved once the
handoff has actually run, so a failed launch leaves it in the queue.

One executor session has exactly one owner. The scheduler is told which Goals the
foreground holds and refuses to start a background turn for them, and a Goal with a
turn in flight is likewise off limits. Methodus never starts two `resume` processes
against one executor session.

## 6. Attention and notification

The attention queue is the maintainer's inbox, and it is deliberately small: it holds
only things that need a maintainer decision or follow-up. Candidates ready for review
are not attention; they are Review.

The native Use surface also uses this queue for an evidence gap. Its contract requires
the Runtime to return one concrete `learning_recommended` task instead of guessing.
Methodus records it as attention with the Use session as its run ID. Accepting the
item creates a fresh Learn Goal with the normal defaults; it never turns an unreviewed
Use answer directly into canonical knowledge.

OS notifications are reserved for what a person must act on:

| Event | Urgency |
|---|---|
| A turn needs an answer or a permission | critical, with sound |
| A turn or the scheduler failed | critical, with sound |
| Candidates are ready for Review | normal, silent |
| A Goal hit its budget | normal, silent |
| Sources went stale | low |
| A turn finished with nothing new | none — the status bar suffices |

When the TUI is running in Ghostty, Methodus emits the terminal's OSC 9 desktop
notification sequence. Ghostty remains the notification owner, so selecting the
notification returns to the terminal surface that raised it instead of opening a
Script Editor notification. Other terminals use the platform notification fallback.

Notifying about ordinary progress trains people to ignore notifications, which costs
more than the missed information is worth.

## 7. Security and trust boundaries

- Methodus never emits a permission-bypass flag, and never modifies a runtime's own
  global configuration.
- Source paths are checked against the Goal's authorized roots before launch.
- One executor session lock prevents concurrent background and foreground resumes.
- Credentials and environment secrets are excluded from events, notifications, and
  candidate sources.
- Candidate output stays invisible to consumer agents until Review commits it.
- A background turn cannot write canonical graph content under any policy.

## 8. Failure and recovery

Each of these is distinguished, and each shows its cause alongside a concrete recovery
action:

- runtime executable missing, or authentication unavailable;
- permission denied for a specific tool;
- source unavailable or changed;
- runtime process exited without a result;
- executor session cannot resume;
- return artifact missing or unparseable;
- budget reached;
- graph or index validation failure.

A failure preserves the run, its events, its source manifest, its executor session ID,
and any human review history. Runtime session recovery is an optimization; the Methodus
run is the durable record.

## 9. Planned

Named so that their absence is a decision rather than an oversight:

- **Learning plans.** A Goal currently holds one prompt. Decomposing it into topics with
  dependencies and coverage tracking is the natural next step, and the reason `GoalRun`
  records a work kind.
- **Candidate revisions and review rounds.** Requesting changes should create a new
  immutable revision against the same candidate, with a semantic diff, rather than a
  fresh unrelated candidate.
- **Source impact.** A source check currently reports that nodes went stale. It should
  say which Goals and which committed nodes are affected, and offer revalidation.
- **Headless resume.** Answers travel back through the native handoff because no
  headless resume path exists yet. A short answer should not require a terminal.
- **Cost for every runtime.** Budgets only bind runtimes that report a monetary cost.

## 10. Acceptance

A maintainer can:

1. write a Goal with sources, four cadences, runtime, permission mode, and budget;
2. leave the TUI open and have due turns run without touching anything;
3. be interrupted only when a turn needs a judgment, fails, or produces candidates;
4. answer a question and have that answer resume the very session that asked it;
5. see month-to-date spend per Goal and have work stop at the budget;
6. pause a Goal, or bring its next turn forward, without editing its cadence;
7. restart Methodus without losing Goals, schedules, questions, or candidates;
8. keep consumer-agent access read-only and limited to reviewed content.

The product succeeds when maintainers spend their time setting learning direction and
making consequential judgments, rather than remembering when knowledge needs revisiting.
