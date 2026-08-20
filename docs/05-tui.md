# 05 — Maintainer TUI

The TUI is a knowledge studio for maintainers. It is allowed to host a focused Learn
conversation because questioning, source evidence, candidate decomposition, and Review
are Methodus-owned interactions. It is not a general coding-agent interface.

## 1. Default surface

```text
┌ ◈ Methodus · Learn                    runtime · Personal + Team ┐
│                                                                    │
│ methodus  What do you want to understand or make repeatable?       │
│                                                                    │
│ you       Learn how we diagnose abnormal device shutdown.          │
│                                                                    │
│ methodus  Existing graph context                                   │
│           • previous shutdown reason                               │
│           • watchdog reset                                         │
│                                                                    │
│           Before researching: which device/runtime scope applies?  │
│                                                                    │
├────────────────────────────────────────────────────────────────────┤
│ › attach @source or answer                                           │
└────────────────────────────────────────────────────────────────────┘
```

The composer supports CJK/IME input, bracketed paste, `Shift+Enter` newline, file/source
mentions, discoverable slash commands, and double `Ctrl+C` exit. The top bar shows the
concrete learning runtime name. The permission mode is visible beside the composer;
`Shift+Tab` cycles read-only plan, controlled execution, and auto edit. Methodus maps the
selection to native runtime controls and never selects a permission-bypass mode.

## 2. Primary panels

### Learn

The home surface. It shows:

- goal and current learning phase;
- retrieved existing nodes;
- attached sources and freshness;
- claims categorized as fact, inference, contradiction, or unknown;
- questions requiring maintainer judgment;
- usage and runtime state;
- proposed CandidateSet when synthesis is ready.

Learn supports continuing the active runtime conversation in the same TUI process;
the current run is restored from its durable state after a TUI restart when the
runtime exposes a resumable executor ID. “Session” means a Learn run only; Methodus
does not manage ordinary coding sessions.

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
| ordinary input | Start or continue the focused Learn run |
| `/new [goal]` | Close the current context and optionally start a new Learn goal |
| `/knowledge` | Browse Knowledge |
| `/method` | Browse Methods |
| `/experience` | Browse Experiences |
| `/review` | Open candidate Review |
| `/graph` | Open the Knowledge graph at the first indexed node |
| `/team` | Inspect Team status; `v` validates, `d` refreshes diff, `p` writes a publish plan |
| `/health` | Inspect source, graph, repository, and connector health |
| `/runtime [id]` | Open the runtime picker or directly select `claude-code`, `codex`, or `cursor` |
| `/help` | Open command and interaction help |
| `/open [path]` | Open the current node, Methodus home, or an explicit local path |
| `/quit` | Exit Methodus |

There is no ordinary task command, workspace command, runtime handoff, Skill browser,
or MCP setup surface.

## 4. Candidate-set flow

When the learning runtime proposes output, Methodus persists the assistant response
and writes one review-only Markdown draft per candidate. The current v1 Review panel
lets the maintainer filter candidates, open the complete draft, edit it externally,
and apply explicit lifecycle actions. A richer structured selection view is the next
draft-editing extension:

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
automatically durable experience. Until the richer selection view lands, the same
include/exclude decision is made by editing or rejecting the corresponding draft in
Review.

## 5. Status and error behavior

- Runtime failure keeps the conversation and durable run visible. If an executor ID is
  available, the run can be resumed; otherwise the maintainer can retry or switch
  runtime without silently publishing the partial result.
- Source failure remains visible as missing/unchecked evidence.
- Graph or Git validation errors open the relevant item; they are not hidden in a
  transient footer.
- Publication blockers require correction. Warnings require acknowledgement.
- Agent CLI consumer activity is not presented as a live session in the TUI.

## 6. Interaction invariants

- `q` is text, never a global quit shortcut.
- Empty-input double `Ctrl+C` within the configured window exits.
- `Esc` closes or backs out; it never destroys a Learn run.
- Arrow keys navigate lists; printable characters filter and never act as hidden
  `j/k` navigation.
- Filters are visibly rendered wherever typing changes a list.
- `Shift+Tab` cycles the visible permission mode; the choice applies to the next
  runtime turn and persists in `config.yaml`.
- Enter opens complete content; full Markdown remains accessible.
- Destructive or publication actions require an explicit target and confirmation;
  implementation must use a visible second-step confirmation, not an implicit key
  chord.
- Narrow terminals degrade to a clear minimum-size message rather than corrupt layout.

## 7. Acceptance

A maintainer can, without memorizing operational CLI commands:

1. start and resume an interactive Learn run;
2. attach evidence and answer consequential questions;
3. receive and edit a multi-node candidate set;
4. review sources, relations, duplicates, and stale state;
5. commit Personal content and explicitly mark it for Team promotion;
6. validate and inspect a publication diff or write a publish plan;
7. browse the resulting graph and understand what consumer agents can retrieve.
