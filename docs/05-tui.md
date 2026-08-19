# 05 — TUI: Graph and Task Control Plane

Methodus has a Rust/ratatui TUI, but it is **not an agent chat client**. Claude Code,
Codex, and Cursor retain their native transcript, composer, tool cards, and permission
interaction. Methodus owns the before-and-after surface: graph, task compilation,
handoff, review, and history.

## 1. Design decision

| Approach | Decision | Why |
|---|---|---|
| Rebuild an agent transcript/composer | No | duplicates and weakens the native agent workflow |
| Parse or embed a running agent TUI | No | brittle, invasive, and contrary to the product boundary |
| Graph/wiki browser plus task control | Yes | exposes Methodus's unique asset: knowledge, selection, and learning |
| Web/desktop shell | Not in v1 | ratatui is sufficient for a local, keyboard-first control plane |

When the user launches an interactive task, Methodus saves state, yields the current
terminal or opens a configured pane, and starts the selected agent. On agent exit or
explicit return, Methodus restores its TUI to the task's outcome screen. It never
expects to display the agent conversation on return.

## 2. Primary surfaces

```text
┌ Methodus ─ Graph • Skill • Experience • Session • Review ─────────┐
│ Graph: payment idempotency                                         │
│ summary · execute facet · links · evidence                         │
│                                                                     │
│ requires → database unique constraint                              │
│ used by  → payment change checklist                                │
│ applied  → webhook incident fix                                    │
├───────────────────────────────────────────────────────────────────┤
│ / new task · learn · search · review · open history                │
└───────────────────────────────────────────────────────────────────┘
```

### Conversation-first launcher

The default home uses the familiar transcript-plus-composer shape, but it is a task
launcher rather than a proxy for the native Agent conversation. Sending ordinary
text creates a task, runs the disposable context planner, compiles a capsule, and
automatically yields the terminal to the selected native runtime.

Graph, Skill, Experience, Session, and Review open as focused overlays above this stable
home so the draft and launcher history retain their spatial position.

Knowledge detail renders facets independently:

- **Learn:** 5W2H, explanation, examples, and practice prompts.
- **Execute:** concise operational rules and pitfalls.
- **Decide:** applicability, alternatives, and trade-offs.
- **Evidence:** source artifacts, confidence, and contradictions.

### New Task / Learn

The user enters a work objective or a learning objective. The resolver proposes a
Face lens (optional), graph nodes, Skills, a context budget, project, and runtime.
Before launch the user sees a capsule preview:

```text
Task: add idempotency to payment callback
Runtime: Claude Code · Native handoff · project: payments-service

Injected (1,120 estimated tokens / 1,600 budget)
  ✓ Knowledge / payment idempotency / Execute      420  exact task scope
  ✓ Experience / webhook incident 2026-08          280  prior verified pitfall
  ✓ Skill / payment change checklist                420  required validation

Lazy references (not startup context)
  · Provider webhook docs
  · Unique-constraint implementation note

[Edit selection] [Launch Claude Code] [Cancel]
```

`Learn` creates the same kind of capsule, but includes the learning Skill, sources,
prerequisites, depth, and candidate knowledge output contract.

### Handoff

Handoff is a transition screen, not an in-app session pane:

1. Write and hash the capsule.
2. Show the command/runtime/cwd and what will be available by reference.
3. Yield terminal or create the configured pane.
4. On return, show task state plus `outcome.md` rather than a synthetic transcript.

For a detached terminal/pane, the History surface offers **Mark returned**. This opens
the same outcome flow; Methodus must not infer task success simply because a process is
no longer visible.

### Review

Review is the only route to durable graph changes. It presents task outcomes,
Experiences, candidate Knowledge, link proposals, Skill drafts, and conflicts. The
user can commit, revise, reject, defer, or connect a node to an existing one.

### History

History lists task and learning capsules with launch mode, project, selected context,
return/outcome state, and links to derived Experience/Knowledge. It enables a
follow-up task with prior selections shown as candidates—not automatically reinjected.

## 3. Interaction contract

Use a slash command palette and focused overlays. Typing `/` opens discoverable
command suggestions; arrows select, `Tab` completes, and `Enter` executes.

| Command / action | Result |
|---|---|
| ordinary composer text | Create a work task, plan its context, compile its capsule, and hand off automatically. |
| `/learn <topic>` | Draft a learning task; attach sources via `@`. |
| `/knowledge` (alias `/graph`) | Browse graph nodes and relations. |
| `/skill` | Browse reusable Skill packages. |
| `/review` | Open candidate/outcome review. |
| `/session` | Browse tasks, capsules, and launches. |
| `/runtime` | Cycle Claude Code, Codex, and Cursor. |
| `Shift+Tab` | Cycle permission mode and persist it as the default. |
| `/open` | Open the selected capsule directory in the OS file manager. |
| `/quit` (alias `/exit`) | Exit Methodus. |

Key behavior:

- `Enter` opens the focused node or confirms the currently explicit action.
- `Tab` moves among search, graph list, node detail, links, and action strip.
- `Space` selects/unselects a proposed context item; the token budget updates live.
- `l` opens the focused relation; `b` opens backlinks; `e` switches to Execute facet;
  `5` switches to Learn/5W2H when present.
- `Esc` closes an overlay or cancels a draft; it never cancels a native runtime without
  a dedicated confirmation.
- `q` is ordinary composer input. Exit uses `/quit`, `/exit`, or `Ctrl+C` twice
  within three seconds; the first `Ctrl+C` clears non-empty input.
- Bracketed paste preserves multiline content. `Shift+Enter` adds a newline. Cursor
  movement and rendering use UTF-8 boundaries plus terminal display width, so CJK
  characters remain aligned.
- `PageUp`/`PageDown` scroll the cached launcher transcript while new messages keep
  the view pinned to the bottom.

## 4. Runtime status and notifications

The status strip exposes only lifecycle facts Methodus knows: capsule compiled,
handoff launched, marked returned, pending review, or managed execution running. It
does not claim to know an interactive Agent's turn-by-turn state.

OS notifications are appropriate for pending review, a returned managed run, or a
high-value knowledge question. Native agent completion notifications remain the
agent's own responsibility unless the launch process itself is the handoff target.

## 5. Implementation components

| Component | Responsibility |
|---|---|
| `GraphSearch` | title/tag/FTS search and filters |
| `NodeDetail` | facets, metadata, sources, typed links, use history |
| `ContextBuilder` | proposed selections, rationale, budgets, lazy refs |
| `HandoffScreen` | terminal/pane launch confirmation and return transition |
| `OutcomeReview` | result, retrospective, Experience and candidate graph changes |
| `HistoryList` | capsules, launches, outcomes, follow-up draft |

The TUI crate does not own agent session lifecycle, edit the user's global
`~/.claude`/`~/.codex` configuration, or render an external agent transcript.

## 6. Acceptance

- A user can find a knowledge item by link/tag/search, inspect its 5W2H and Execute
  facets, and traverse to related Skill and Experience nodes.
- A task launch cannot occur without a visible capsule preview, context budget, and
  selection rationale.
- During normal work, all human-agent conversation happens in the native Agent TUI.
- A returned learning task can be reviewed into one independent Knowledge file with
  typed links, not merely stored as a transcript.
