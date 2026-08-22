# 05 — Maintainer TUI

The TUI is a knowledge studio for maintainers, and the only maintainer surface. Its
default Use composer answers questions from the reviewed graph; `/learn` prepares a
focused Learn run and hands the terminal to the selected native runtime. It also runs
the scheduled, unattended half of the product described in
[`10-continuous-learning.md`](./10-continuous-learning.md). It is not a general
coding-agent interface.

## 1. Default surface

```text
┌ ◈ Methodus · Use                     runtime · Personal + Team ┐
│                                                                    │
│ methodus  Ask about an existing Method, Knowledge, or Experience.  │
│                                                                    │
│ you       How should we diagnose abnormal device shutdown?          │
│                                                                    │
│ methodus  Existing graph context                                   │
│           • previous shutdown reason                               │
│           • watchdog reset                                         │
│                                                                    │
│           Recommendation: start with the watchdog reset signal…    │
│                                                                    │
├────────────────────────────────────────────────────────────────────┤
│ › ask Methodus · @source for additional evidence                     │
└────────────────────────────────────────────────────────────────────┘
```

The composer supports CJK/IME input, bracketed paste, `Shift+Enter` newline, file/source
mentions, discoverable slash commands, and double `Ctrl+C` exit. The top bar shows the
concrete Runtime name. Use protects the graph and attached evidence while its
managed workspace follows the same permission mode as Learn. The composer displays
that mode and `Shift+Tab` cycles guided workspace, controlled execution, and auto edit
for both Use and Learn.
Methodus maps the Learn selection to native runtime controls and never selects a
permission-bypass mode.

## 2. Primary panels

### Use

The home surface. Ordinary input is a question against the committed and stale graph.
Methodus creates `workspaces/use/<session-id>/`, writes a `METHODUS_USE.md` contract
and inventory there, opens only the consumer-visible Knowledge, Method, and Experience
directories, and hands the terminal to the selected native Runtime from that workspace.
Temporary notes and plans stay in that workspace; the graph, project directory, and
explicit `@` sources are not to be modified. A Use follow-up resumes the same native session when the
Runtime supports it, while a new `/new` clears it. The Use turn itself never creates a Learn run or a
CandidateSet.

If the Runtime cannot find sufficient committed evidence, the Use contract requires a
single `learning_recommended` return object instead of an invented answer. Methodus
opens it in `/attention`; `Enter` creates a Learn Goal from the recommended task, and
typed text replaces that task before the Goal is created. The Runtime never creates a
Goal directly.

`@` mentions add explicit local source paths to the question. They are passed as
protected evidence directories to the Runtime alongside the prepared graph
environment. The Use composer displays the same permission mode as Learn and
`Shift+Tab` cycles it for either mode.

### Learn

`/learn` switches the composer into deliberate Learn mode. `/learn <text>` starts the
same flow immediately. Learn shows:

- goal and current learning phase;
- retrieved existing nodes;
- attached sources and freshness;
- claims categorized as fact, inference, contradiction, or unknown;
- questions requiring maintainer judgment;
- usage and runtime state;
- proposed CandidateSet when synthesis is ready.

Learn creates `workspaces/learn/<run-id>/` as the native runtime's working directory
and returns control to the selected runtime's native TUI for the actual research
conversation. After that TUI exits, Methodus restores its screen and imports an
explicit synthesis artifact if the runtime wrote one. The current run is restored after
a TUI restart; Claude Code resumes its durable executor UUID, while other runtimes can
start a fresh native conversation carrying the same Learn context. “Session” means a
Learn run only; Methodus does not manage ordinary coding sessions.

### Goals

Goals are the standing questions Methodus keeps working on between Learn runs. The
panel lists each Goal with its state — `ready`, `running`, or `paused` — and how long
until its next learning turn. The detail pane shows the prompt, all four cadences with
their next due times, quiet hours, month-to-date spend against budget, review policy,
and authorized sources.

A Goal is created with `/goal <text>`, using the same natural-language input and `@`
source attachments as Learn. Policy takes its defaults — weekly learning, weekly review,
monthly summary, daily source checks, and a $20 monthly ceiling — so creation does not
require a policy form. `/goal` without text opens this management panel; `/goals` is a
compatible alias. Its `n` composer is the same creation flow when a person prefers to
start there. Creation queues the first learning turn immediately; later turns follow
the cadence.

Refinement is separate. `e` opens the Goal as YAML in `$EDITOR`, where cadences,
sources, quiet hours, budget, runtime, and review policy are all editable. The document
contains only maintainer-owned fields; identity, timestamps, and computed due times
never appear in it. A document that fails validation is kept as a draft so the next
edit reopens it instead of discarding what was typed.

Actions are `n` new, `Enter`/`r` learn now, `e` edit policy, `space` pause or resume,
`s` summarize now, and `d` delete. *Now* is implemented by moving the turn's due time
forward and waking the scheduler rather than launching a runtime directly, so
occupancy, attention, and budget checks still apply to it.

### Attention

The queue of maintainer decisions and runtime learning recommendations. Each entry
names its Goal or Use session, how long it has been waiting, and whether the runtime
wants a judgment, a permission, or a new Learn task.

Answering resumes the session that asked. `Enter` opens a reply composer; `Shift+Enter`
adds a newline and plain `Enter` submits. Submitting hands the terminal to the runtime
holding that executor session, delivering the answer as the next message. The question
is only resolved once that handoff has run, so a failed launch leaves it in the queue.
Use-to-Learn recommendations are the exception: they do not resume the Use runtime;
plain `Enter` accepts the proposed task and creates a new Learn Goal, while typed text
edits it first. The Goal receives the same default cadence and budget as `/goal <text>`.
`d` dismisses a question without answering, for the ones a maintainer decides are moot;
the Goal unblocks and its next scheduled turn starts fresh.

### Knowledge

- filter by free text, `tag:`, `scope:`, `status:`, `visibility:`, and `kind:`;
- open the complete Markdown node;
- inspect the Learn, Decide, Execute, and Evidence headings in the complete note;
- inspect source health and backlinks;
- open one-hop typed graph relations.

### Method

- browse workflow intent, phases, evidence standard, output contract, and checks;
- see Knowledge prerequisites and validating Experience when authored relations exist;
- compare a Personal override with its Team base through the Markdown/Git view;
- use a Learn run to propose a revision to a stale or ineffective Method.

### Experience

- browse specific incidents/attempts and reusable lessons;
- filter by outcome, Method, component, date, and visibility when those frontmatter
  fields are present;
- follow validates/contradicts/caused-by relations;
- promote a reusable lesson by creating a new reviewed Knowledge candidate.

### Review

Review is the only normal route from model output to canonical graph content. It is
also the maintainer surface for editing candidate Markdown, checking source health,
and recording the rationale for lifecycle changes.

Actions:

- inspect candidate Markdown and evidence;
- compare with likely duplicates and explicit merge targets;
- split or merge drafts;
- edit kind, summary, facets, sources, and relations while preserving a stable ID;
- commit Personal, reject, deprecate, revalidate, or propose Team promotion;
- record rationale for consequential decisions.

Merge never picks a target implicitly.

When a Knowledge candidate should revise an existing node, `m` first opens the
committed Knowledge target picker. `Enter` on a target opens a facet-level patch
review instead of writing immediately. The patch review shows the selected facet
side by side as `target` and `candidate`:

- `↑`/`↓` changes the facet (`Learn`, `Decide`, `Execute`, or `Evidence`);
- `Space` or `a` accepts the candidate facet, while `k` keeps the target facet;
- the initial state keeps every target facet, so nothing is applied by accident;
- `Enter` previews the selected patch, and a second `Enter` confirms it;
- only accepted candidate facets replace target sections. Target frontmatter and
  unselected sections remain intact, and the candidate is removed only after the
  write succeeds.

The review audit records the target and the exact accepted facets. The runtime's
prose `patch` proposal remains evidence for this decision; it does not bypass the
maintainer's facet selection.

### Graph

The graph is a navigation aid, not decorative force-directed animation. The first
implementation shows a focused one-hop relation view with typed edges, keyboard
selection, and node details. Large graph rendering must be bounded and lazy.

### Team

- inspect the selected Team Markdown root (the first root is `teams/default`);
- show Git branch, dirty state, changed files, validation issues, and a bounded diff;
- write a local `runs/publish_<id>/publish-plan.md` for review;
- review Personal → Team file changes before using normal Git tooling.

The current Team panel does not commit, push, merge, or discard Git changes. Switching
among local Team IDs is supported; external repository-path configuration and conflict
resolution remain planned extensions.

### Health

- runtime binaries, required files, stable index revision, graph validation errors and
  warnings, stale graph nodes, Team validation, and connector missing/current/drifted
  state in one diagnostic view.

## 3. Slash commands

Commands switch surfaces or start explicit maintainer actions:

| Command | Result |
|---|---|
| ordinary input | Ask Methodus from the reviewed graph and continue Use |
| `/learn [text]` | Start or continue a deliberate Learn run |
| `/new [goal]` | Clear the current context and return to Use; with a goal, start Learn |
| `/goal [text]` | Create a Goal, or manage Goals when no text is supplied (`/goals` is an alias) |
| `/attention` | Handle runtime questions and learning recommendations |
| `/knowledge` | Browse Knowledge |
| `/method` | Browse Methods |
| `/experience` | Browse Experiences |
| `/review` | Open candidate Review |
| `/team` | Inspect Team status; `v` validates, `d` refreshes diff, `p` writes a publish plan |
| `/health` | Inspect source, graph, repository, and connector health |
| `/runtime [id]` | Open the runtime picker or directly select `claude-code`, `codex`, or `cursor` |
| `/help` | Open command and interaction help |
| `/open [path]` | Open the current node, Methodus home, or an explicit local path |
| `/quit` | Exit Methodus |

There is no ordinary task command, task workspace command, Skill browser, or MCP setup
surface. Both Use and focused Learn hand the terminal to a native Runtime; only Learn
returns a structured artifact for Methodus to import into Review.

## 4. Candidate-set flow

When the learning runtime finalizes, it writes its synthesis to the run-specific return
artifact. After the native TUI exits, Methodus imports that output and writes one
review-only Markdown draft per candidate. The Review panel lets the maintainer filter
candidates, open the complete draft, edit it externally, and apply explicit lifecycle
actions. When a candidate targets existing Knowledge, the facet-level patch review
described above provides the structured conflict decision:

```text
Candidate set · Learn run learn_...

[x] Method     Abnormal shutdown diagnosis
[x] Knowledge  Previous shutdown reason
[x] Knowledge  Pre-shutdown crash detection
[ ] Experience This research session

Relations: 4 proposed · Unknowns: 1 · Contradictions: 0

Enter inspect · Space include · s split · m merge drafts · r review
```

The unchecked research Experience illustrates the default: a Learn transcript is not
automatically durable experience. Candidate inclusion/exclusion still happens by
editing, approving, or rejecting the corresponding draft in Review.

## 5. Unattended work

The event loop consults the scheduler between frames and launches due turns on the
Tokio runtime. They run headless while the maintainer keeps using the terminal; results
arrive as events the loop drains each frame.

Scheduling is event-driven with a slow poll behind it. Anything a person did that could
make work due — creating, editing, or enabling a Goal, bringing a turn forward,
resolving a question — wakes the scheduler on the next frame, so *now* means now. The
one-minute fallback poll exists only for the two things nobody triggers: a cadence
coming due and the end of a quiet-hours window. A poll with nothing due is two indexed
SQLite reads, so its interval bounds latency rather than cost.

While the terminal is handed to a native runtime the loop is blocked, which is the
behavior we want: no unattended turn starts mid-conversation. The Goal a foreground
Learn belongs to is reported to the scheduler as occupied, so a background turn cannot
resume the same executor session underneath the maintainer.

The header carries two counters: `⚑` for open questions and `◇` for turns in flight.
Anything that changed the graph or needs a decision is also written to the scrollback
with the command that acts on it; a turn merely starting is status-bar news.

OS notifications follow the tiers in
[`10-continuous-learning.md`](./10-continuous-learning.md#6-attention-and-notification).
A turn that completed with nothing new never produces one.

## 6. Status and error behavior

- Runtime failure keeps the conversation and durable run visible. If an executor ID is
  available, the run can be resumed; otherwise the maintainer can retry or switch
  runtime without silently publishing the partial result.
- Source failure remains visible as missing/unchecked evidence.
- Graph or Git validation errors open the relevant item; they are not hidden in a
  transient footer.
- Publication blockers require correction. Warnings require acknowledgement.
- Agent CLI consumer activity is not presented as a live session in the TUI.

## 7. Interaction invariants

- `q` is text, never a global quit shortcut.
- Empty-input double `Ctrl+C` within the configured window exits.
- `Esc` closes or backs out; it never destroys a Learn run.
- Arrow keys navigate lists. Rejected candidates remain visible in Knowledge, Method,
  and Experience for cleanup; deprecated and review-only candidates remain out of
  those lists. Rejected nodes do not participate in active graph neighborhoods.
- Filters are visibly rendered in panel titles. Press `f` to enter filter mode, type
  the query, then press `Enter` to apply or `Esc` to close it; this keeps Review
  approval actions available as direct keys.
- `Shift+Tab` cycles the visible permission mode; the choice applies to the next Use or
  Learn runtime turn and persists in `config.yaml`.
- Enter opens complete content; full Markdown remains accessible.
- `g` opens the selected active node's one-hop graph neighborhood; there is no
  separate graph command.
- Detail views reserve a fixed bottom action strip. It always displays the available
  node actions and turns into an explicit “press the same key again” confirmation
  prompt for delete, revalidate, and Review decisions.
- Review rejection immediately deletes the candidate source and records the decision.
  In a canonical node detail, `d` permanently removes the managed node and its graph
  projection; there is no archive or recovery-copy state.
- Destructive or publication actions require an explicit target and confirmation;
  implementation must use a visible second-step confirmation, not an implicit key
  chord.
- Narrow terminals degrade to a clear minimum-size message rather than corrupt layout.

## 8. Acceptance

A maintainer can, without memorizing operational CLI commands:

1. start and resume an interactive Learn run;
2. attach evidence and answer consequential questions;
3. receive and edit a multi-node candidate set;
4. review sources, relations, duplicates, and stale state;
5. commit Personal content and explicitly mark it for Team promotion;
6. validate and inspect a publication diff or write a publish plan;
7. browse the resulting graph and understand what consumer agents can retrieve;
8. write a Goal, leave the TUI open, and be interrupted only when a scheduled turn
   needs a judgment, fails, or produces candidates.
