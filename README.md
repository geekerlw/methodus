<h1 align="center">Methodus</h1>

<p align="center">
  <strong>Persistent personal expert system</strong> for AI coding agents<br>
  Brain in Rust. Hands in Claude Code, Codex, or Cursor.
</p>

<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-1.75+-dea584?logo=rust&logoColor=white">
  <img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue">
  <img alt="TUI" src="https://img.shields.io/badge/UI-ratatui-black">
  <img alt="Executors" src="https://img.shields.io/badge/hands-Claude%20%7C%20Codex%20%7C%20Cursor-555">
</p>

<p align="center">
  <a href="#quick-start">Quick start</a> ·
  <a href="#how-a-day-goes">Daily use</a> ·
  <a href="#learning-loop">Learning</a> ·
  <a href="#docs">Docs</a>
</p>

<p align="center">
  <img src="docs/architecture.png" alt="Methodus architecture: execution loop on top, human-gated learning loop below" width="100%">
</p>

<p align="center"><sub>One process. Three loops. Nothing becomes live knowledge until you commit it in <code>/inbox</code>.</sub></p>

---

Coding agents are good at a task and then forget it. You paste the same constraints next week. Methodus is the process you leave running in `tmux` so that does not keep happening.

It is not a fourth coding agent. Claude Code, Codex, and Cursor still write the code. Methodus picks the expert snapshot (Face), the procedure (Method), and the skills; isolates the workspace; watches the event stream; and turns what actually happened into notes, skill patches, and knowledge — **only after you say yes**.

Policy, promotion, and conflict checks live in Rust. Prompts carry resolved context, not the state machine.

## Why it looks like this

Most “memory” for agents is either a growing system prompt or a vector store nobody reviews. Methodus takes the other bet:

| Choice | What that means here |
|--------|----------------------|
| Single process | Engine + ratatui TUI in one binary. Keep it open in `tmux`. No Methodus daemon, no socket. |
| Executor-agnostic | Core talks to a `RuntimeAdapter`. Swap Claude / Codex / Cursor without rewriting policy. |
| Evidence-first | Observation → Experience → candidate → committed Knowledge. One model dump is never fact. |
| Human gate | `/inbox` is the only path onto live skills and Face knowledge. Distill may *propose*. |
| Budgeted background | Learning jobs are queued, capped, cancelable. No `while true: call_llm()`. |
| Files you can read | Faces, notes, skills, and knowledge are YAML/Markdown under `~/.methodus`. SQLite holds lifecycle, events, and the queue. |

Prime Agent’s Continual Harness is a close cousin (in-session `/refine` on one REPL). Methodus is the supervisor version: same “keep what worked” instinct, different trust model — see [`docs/07-learning-vs-refine.md`](./docs/07-learning-vs-refine.md).

## Concepts

| Term | What it is |
|------|------------|
| **Face** | Domain-expert snapshot (Network, Kernel, …) — identity, notes, methods, skill deps. Not a persona the model invents, not a second process. |
| **Method** | A procedure for a class of problem: preconditions, steps, evidence. |
| **Skill** | An Agent Skill (`SKILL.md`) Methodus owns. Drafts sit in candidates until `/inbox` commit. |
| **Note** | Cheap Face memory (`faces/<id>/notes/`). Injected on later tasks; **3 injection hits** enqueue a skill. |
| **Knowledge** | Committed, scoped markdown. Injected as a *slice*, never the whole store. |
| **Pack** | A folder of faces/skills you load as a team baseline. Methodus records the path; git is your problem. |
| **Task** | A goal with a lifecycle and an event log — not just the prompt string. |

Resolution stack: personal home → focus pack → other active packs. Personal notes overlay pack files of the same name.

## Features

- **TUI is the product.** `methodus` opens a session (transcript + composer). Overlays: `/inbox`, `/face`, `/setup`, `/session`. No web UI, no desktop app.
- **Isolated workspaces.** Each task gets `~/.methodus/workspaces/<id>/`. Your repos stay where they are (`--add-dir`); Methodus does not copy the tree.
- **Injected context you can see.** `.methodus/selected-context.md` and `.methodus/injected.md` list what this turn actually received.
- **Trajectory distill.** After an execution task: at most one note **or** one skill patch **or** one skill draft. Bare `Use Tool` is not skill-worthy.
- **`/learn`.** Point at paths or URLs. Rust picks ingest / survey / module-expert. Results still land in `/inbox`.
- **Hits and misses.** Injected notes increment hits. Same gap after inject → confidence × 0.7 and a mentor question (`learning.injection_missed`).
- **Recovery.** Executor session ids + `--resume`. Kill Methodus, start it again, reconcile. Stay in `tmux` so you rarely have to.
- **Policy, not prompt.** `acceptEdits` / `plan` / `cautious`. Destructive ops still pause. Methodus never writes `~/.claude` / `~/.codex` / `~/.cursor`.

## Quick start

**Need:** Rust 1.75+, and at least one executor on `PATH`. Default runtime is Claude Code (`claude`).

```bash
git clone https://github.com/geekerlw/methodus.git
cd methodus
cargo run -p methodus
```

First launch creates `~/.methodus` (config, `state.db`, seed faces/skills). Run it in tmux if you want the process to outlive the terminal:

```bash
tmux new -s methodus
cargo run -p methodus
# detach: Ctrl-b d    attach: tmux attach -t methodus
```

Then in the TUI:

1. `/setup` — confirm the executor CLI is on PATH, register a project directory, pick permission mode.
2. Type a task. Watch the session pane. Approve when Policy asks.
3. When it finishes, open `/inbox`. Commit the note or patch you actually want to keep.
4. Next similar task should show that note under **Injected this turn**.

> Methodus does not sandbox the executor. Workers run as you. Review permission prompts and `/inbox` the way you would review a PR.

## How a day goes

```text
you type a goal
  → Resolver loads Face / Method / Skills / notes
  → Policy maps to executor permission flags
  → Session spawn or --resume in an isolated cwd
  → events stream into the TUI
  → done  →  distill job  →  /inbox
  → you commit  →  files + SQLite
  → next task injects the slice  →  hits or miss
```

Curiosity questions wait until you are idle, then occupy the composer (ask / snooze / dismiss). There is no jobs dashboard. Outcomes surface through `/inbox` and that idle ask.

### Commands

| Command | |
|---------|--|
| `/learn` | `@paths` or URLs → knowledge pipeline |
| `/inbox` | Candidates, questions, patches — Enter to act |
| `/face` | Pin a Face (`/face <id>`) |
| `/setup` | Runtime, projects, packs, health |
| `/session` | Switch conversations (Tab also works) |
| `/clear` `/new` | New conversation (does not resume the executor) |
| `/retry` | Retry the open conversation |
| `/cancel` | Cancel a running task |
| `/delete` | Delete a finished task |
| `/help` | Palette |
| `/quit` | Or `/exit`, or Ctrl-C twice |

`1`–`4` pick overlay options. Esc closes. Empty-input `[` `]` cycles conversations.

## Learning loop

After an execution task Methodus may enqueue **one** artifact. `/learn` ingest/survey can still write knowledge candidates; an execution task that already distilled a note/patch/skill does not also mint a parallel knowledge file.

```text
Trajectory (events + experience)
        │
        ▼
   Distill  ──  note XOR patch XOR skill draft
        │
        ▼
    /inbox   ←  human gate (N = note, P = patch)
        │ commit
        ▼
 Durable state   faces/<id>/notes  ·  skills/  ·  knowledge/*.md  ·  SQLite
        │
        ▼
 Next-task inject   (count hits)
        │
        ├─ hits ≥ 3  →  enqueue propose_skill
        └─ same gap after inject  →  downrank + Curiosity question
```

Optional: `refine_llm: true` in `~/.methodus/config.yaml` (default on, **8/day**) polishes a note/patch in plan mode. Apply is still `/inbox`. Set `refine_llm: false` to keep the rules draft as-is.

## Executors

Verified: non-interactive run, structured event stream, session resume. Methodus normalizes each stream to `RuntimeEvent`.

| | Non-interactive | Events | Resume | Mid-turn approval | Background |
|--|:---:|:---:|:---:|:---:|:---:|
| **Claude Code** (default) | `--print` | stream-json | `--resume` | yes | `--bg` |
| **Codex** | `exec` | `--json` | `exec resume` | app-server | app-server |
| **Cursor** | `--print` | stream-json | `--resume` | coarse | no |

Spike notes and flag shapes: [`docs/01-runtime-adapters.md`](./docs/01-runtime-adapters.md).

## On disk

```text
~/.methodus/
├── config.yaml          # runtime, permission mode, refine_llm cap
├── state.db             # tasks, events, queue, indexes
├── faces/<id>/          # face.yaml, knowledge/, notes/, experiences/
├── methods/             # procedure YAML
├── skills/              # committed SKILL.md packages
├── packs.yaml           # registered team folders + focus
├── projects.yaml        # your repos + focus
└── workspaces/<task>/   # isolated cwd; selected-context.md, injected.md
```

Domain files are source of truth. The database indexes them. Hash mismatch → re-index, never a silent overwrite of the file.

## What it will not do

- Reimplement Claude/Codex/Cursor, or rewrite their permission/MCP/skill systems.
- Touch your global executor config (`~/.claude`, `~/.codex`, `~/.cursor`).
- Cron the executor. No heartbeat-autonomous “keep coding at 3am” loop.
- Silent-commit skills or Face YAML.
- Grow a company wiki or a module tree into global Faces. Repo maps belong to **projects**.
- Ship a desktop GUI. The TUI is the product.
- Git commit / push as a Methodus feature. Packs are folders.

## Status

`0.1.0`. Walking skeleton through TUI, adapters, `/learn`, and the gated refine loop (notes, patches, injection hits/misses) is in tree. Treat it as dogfood, not a polished release. Roadmap: [`docs/04-roadmap.md`](./docs/04-roadmap.md).

```text
methodus/                 # this binary — TUI + Engine in-process
crates/
  methodus-domain/        # types, enums, transitions — no I/O
  methodus-store/         # SQLite + files
  methodus-runtime/       # RuntimeAdapter + Claude/Codex/Cursor
  methodus-core/          # resolver, policy, scheduler, learning — no UI
  methodus/               # ratatui
resources/                # seed faces / methods / skills
docs/                     # product + architecture (start at docs/README.md)
```

## Docs

| | |
|---|---|
| [Product contract](./docs/00-product.md) | Domain model, three loops, permissions, out of scope |
| [Architecture](./docs/02-architecture.md) | Crates, single-process runtime, crash recovery |
| [Runtime adapters](./docs/01-runtime-adapters.md) | Verified CLI flags and event shapes |
| [Data model](./docs/03-data-model.md) | SQLite DDL and file layout |
| [TUI](./docs/05-tui.md) | Chrome, overlays, composer states |
| [Learning vs `/refine`](./docs/07-learning-vs-refine.md) | What we borrowed from Prime, what we refused |
| [Index](./docs/README.md) | Full set |

## License

[MIT](./LICENSE) © Steven Lee
