# 05 — TUI (agent surface)

How Methodus presents itself in the terminal. Product surface rules live in
[`00-product.md`](./00-product.md) §8; this document is the **implementation contract**
for the in-process `ratatui` UI.

## 1. Decision: stay on ratatui, copy patterns not stacks

Methodus is a **Rust single-process** binary (`02-architecture.md`). Daily UI and Engine
share that process.

| Approach | Verdict |
|----------|---------|
| Embed Pi `pi-tui` / Claude Code Ink | **No** — TypeScript runtimes; wrong stack |
| Use Pi/Claude as the shell, Methodus as brain only | **No** — inverts the product (Methodus is brain + daily UI) |
| **ratatui** (+ small mature crates as needed) | **Yes** — already the mature Rust TUI choice |
| Desktop shell wrapping `methodus-core` | **No** — TUI *is* the product shell; no second GUI |

**Reference, do not vendor:** [Pi TUI components](https://pi.dev/docs/latest/tui)
(overlays, `SelectList`, `Editor`/`Input`, theme tokens, markdown). Claude Code / other
agent REPLs for the same *interaction* shape: transcript + composer, picks in the
composer, floating overlays.

## 2. Layout paradigm

Daily driver = **agent chat**, not a multi-page dashboard.

```text
┌ status strip (1 line, no box) ─────────────────────────────┐
│ transcript (full-bleed, no panel)                          │
│                                                            │
╭────────────────────────────────────────────────────────────╮
│ > type a message                                           │
╰────────────────────────────────────────────── acceptEdits ─╯
  status + context keys
```

Rules:

- **One operable surface:** the composer. States: `type` / `pick` / `ask` / `palette`.
- **Chrome sparingly:** top status strip; composer is a **rounded input well** (Claude
  Code shape). Slash/mention palettes stay hairline lists above the well.
- **Overlays float** over the transcript (Clear only the popup rect). Do not wipe the
  whole chat to open Sessions / Faces / Inbox.
- **Setup** may use a fuller overlay (more fields); still one outer frame, not four
  nested panels.
- Esc: close overlay → cancel pick/ask → (done). Empty-input Tab / `/session` opens
  the conversation picker.

## 3. Component inventory (Pi-aligned)

Map Pi concepts → Methodus ratatui pieces. Implement in this order.

| # | Component | Pi analogue | Methodus role | Status |
|---|-----------|-------------|---------------|--------|
| C0 | **StatusStrip** | header / theme bar | runtime, face, `!N`, `▣N`, session id | done (minimal) |
| C1 | **Transcript** | message list | full-bleed chat; You / Assistant / tool / meta | done (markdown) |
| C2 | **Composer** | `Input` / `Editor` | rounded input well; placeholder; CJK cursor | done |
| C3 | **Hairline / chrome** | borders as separators | palettes use `Borders::TOP`; composer is a well | done |
| C4 | **FloatingOverlay** | `overlay: true` | Sessions / Faces / Inbox / Events / Jobs popups | done (Setup full-bleed) |
| C5 | **SelectList** | `SelectList` | permission + knowledge picks; ↑↓ Enter + keys | done (shared helper) |
| C6 | **SlashPalette** | command palette | `/` filter list above composer | done |
| C7 | **MentionList** | file picker | `@` path list | done |
| C8 | **QuestionAsk** | custom dialog in composer | idle / inbox question occupies composer | done |
| C9 | **Theme tokens** | theme.fg(...) | semantic slots only; respect `NO_COLOR` | partial |
| C10 | **MarkdownBubble** | `Markdown` | render assistant (and knowledge preview) lightly | done |
| C11 | **Editor** | multiline `Editor` + IME `Focusable` | Shift-Enter newline; CJK IME cursor | done |
| C12 | **Diff / tool card** | diffs, tool widgets | compact tool rows + edit snippets | done |
| C13 | **FuzzySelect** | fuzzy `SelectList` | filter sessions / faces / inbox | done |

Out of scope for the TUI crate: owning session lifecycle (Engine does); mutating
`~/.claude` / `~/.codex` / `~/.cursor`.

## 4. Implementation waves

### Wave A — chrome & overlays (now)

1. Document this file; point product §8 / roadmap M3 here. **done**
2. Sessions / Faces / Inbox as **floating** pickers over the live transcript. **done**
3. One **SelectList** draw helper shared by permission and knowledge picks. **done**
4. Keep Setup as a dedicated overlay; reduce inner boxes to hairlines. **done**

### Wave B — reading quality

1. Lightweight markdown for Assistant lines (headings, fences, bold) without a full
   browser-grade renderer. **done**
2. Knowledge / skill preview in picks uses the same renderer. **done**
3. Long transcripts: keep scroll; consider virtualization only if dogfood hurts. **done**
   (virtualization still deferred)

### Wave C — input quality

1. Multiline composer (paste + explicit newline key). **done** (`Shift-Enter`; `Ctrl-J` fallback)
2. IME-friendly hardware cursor placement when composing CJK. **done**
3. Optional: pull a small textarea crate if hand-rolled editing gets costly. **skipped**
   (hand-rolled editor is enough)

### Wave D — power user

1. Fuzzy filter on overlay lists. **done**
2. Richer tool cards / diff snippets when RuntimeEvent carries enough structure. **done**

## 5. OS notifications

Methodus fires **macOS Notification Center** / **`notify-send`** (Linux) when the tmux
pane looks idle (no key/mouse input for ~30s). While you are actively in the pane,
**status strip + composer** are the source of truth — no OS banner or sound.

| Tier | Events | OS sound |
|------|--------|----------|
| Critical | permission blocked, executor error | yes (macOS Glass) |
| Normal | inbox candidate, idle question | no |
| Low | turn complete (“your turn”) | no; only when pane idle |

- Setup → **notifications** toggles OS notify (`config.yaml`). Off = status bar only.
- `METHODUS_NOTIFY=always` forces OS banners even when the pane looks active.
- Dedup: same key within 30s does not re-notify (errors keyed by message prefix).
- Idle questions notify when away; **composer takeover** only when the pane is engaged.

## 6. Keybinding contract (summary)

| Context | Keys |
|---------|------|
| Type | Enter send; Shift-Enter newline; `/` palette; `@` files; empty Tab → sessions; empty `[` `]` cycle |
| Inbox list | ↑↓ select; Enter → full view; type filter; Esc session |
| Inbox detail | `[` `]` / PgUp PgDn / wheel scroll body; Esc → list; actions in composer |
| Pick (permission / knowledge) | ↑↓, Enter, digit / letter shortcuts, Esc later |
| Ask | type answer, Enter submit, Esc later, empty `d` dismiss |
| Overlay | type to filter; ↑↓ / j k move; Enter act; Esc clears filter then closes |
| Global | `?` help; `/quit` or ctrl-c twice; ctrl-n new conversation |
| Events / Jobs overlay | ↑↓ select; `[r]` refresh; Jobs: `[c]` cancel; Esc session |

Footer shows **only** what is actionable in the current composer/overlay state.

### Inbox master–detail

Inbox uses **progressive disclosure** so long skill/knowledge/experience bodies do not cram into the list popup:

1. **List + summary** — floating overlay: left column is the filtered queue (Q / K / S / E); right column shows a short `review_summary` (status, path, first lines). No scroll in this pane.
2. **Enter → detail** — overlay closes; the work area becomes a scrollable full body (`review_detail`). Knowledge conflicts show existing + candidate in one view.
3. **Composer actions** — bottom SelectList or answer field:
   - **Question:** Answer / Later / Dismiss / Back
   - **Knowledge / skill draft:** commit / reject (replace when conflicted)
   - **Experience:** Mark review done / Back
4. After an action succeeds, detail closes back to the list (or session if inbox emptied).

Skill drafts from completed tasks land in `/inbox` automatically; use the same detail
composer to commit or reject.

**Face evolution (Evolution loop):** after **2+ committed** module-study knowledge
entries on the same Face, Methodus proposes `face.yaml` updates (intent_tags, methods,
skills) as an inbox item tagged **F**. Approve to merge into `~/.methodus/faces/` only
(personal overlay; pack faces are copied locally first).

When **3+ committed experience** entries exist for a Face, Methodus may also propose
**method** or **skill** evolution candidates (YAML drafts under `methods/` or
`skills/.candidates/`). Same inbox flow: approve merges into home resources.

### Multi-Face composition

Tasks can combine a **primary Face** with **context Faces** so knowledge, methods, and
skills from multiple domains load together.

```text
/face network + storage
  → primary: network
  → context: [storage]  (persisted in config.yaml context_faces)

/face            → open Faces overlay; Enter pins primary only
```

Rules:

- Syntax: first token before `+` is primary; each `+` segment adds context face ids
  (whitespace-separated).
- At task run, Resolution merges committed knowledge from all selected Faces into the
  workspace (`{face}/{name}.md` layout).
- Each Face's `face.yaml` is copied into the workspace for executor context.
- When **2+ Faces** overlap on the same knowledge stem with conflicting claims, Methodus
  enqueues a mentor **Question** (cross-face debate) before run proceeds.
- Default/context faces persist in `~/.methodus/config.yaml` and apply to new tasks until
  changed.

### Module expert study (`/study`)

Learning reads **user-specified sources**, not the task workspace. Outputs land in Methodus home; execution tasks load them later.

```text
/study <topic> @~/path/to/code https://doc.example.com/page
  → executor reads those paths/URLs in place
  → workspace is scratch/transcript only
  → jobs: SynthesizeKnowledge → faces/…/knowledge (module_study)
         AnalyzeKnowledgeGaps → mentor Questions
         ProposeSkill → skills/.candidates/ draft
         Experience file under faces/…/experiences/
  → /inbox: commit knowledge + skill; idle: answer mentor questions
```

Ordinary tasks still inject committed knowledge into workspace at run time. Study tasks do not use project focus as the corpus.

**Hypotheses (Curiosity loop):** when gap analysis finds under-evidenced claims, inbox shows **H**
items. Promote → knowledge candidate; Validate → keep hypothesis; Reject → discard.

### Project ingest (`/ingest`) and repo survey (`/survey`)

Requires a **focus project** in `/setup`.

```text
/ingest @~/docs/standard.pdf @~/specs/
  → doc-ingest method → projects/{id}/knowledge/*.md (candidate)

/survey
  → repo-survey method → projects/{id}/knowledge/*.md from project root layout
```

### Workspace cleanup (`/cleanup`)

```text
/cleanup [days]   # default 30 — remove workspace dirs for terminal tasks older than N days
```

### System overlays (`/events`, `/jobs`)

Audit and background work without leaving the chat surface:

```text
/events   → floating list of recent audit events (type + timestamp)
/jobs     → learning queue; [c] cancels selected job, [r] refresh
```

These overlays share the same floating-picker pattern as Sessions / Faces / Inbox.

## 7. Acceptance (dogfood)

- Full task loop completable without leaving the agent chat surface.
- Opening Sessions / Faces / Inbox does not erase the conversation underneath.
- Permission and knowledge picks feel like the same control (SelectList).
- No dependency on Node/Ink/pi-tui for the Methodus binary.

## 8. Open questions

- Whether Setup should also become a floating card vs full-bleed overlay.
