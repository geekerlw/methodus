# Learning vs Prime Agent `/refine` — comparison and breakthroughs

Reference: [Prime Agent](https://github.com/PrimeIntellect-ai/prime-agent) (Continual Harness, Aug 2026).

Methodus and Prime Agent both aim at **durable improvement**, but from opposite
trust models. Prime optimizes for long-running autonomy inside one harness; Methodus
optimizes for **executor-agnostic supervision**, evidence gates, and human review.

## Side-by-side

| Dimension | Prime Agent (`/refine`) | Methodus (Learning loop) |
|-----------|-------------------------|---------------------------|
| **Trigger** | Agent or user calls `/refine` / `refine.run()` when a tactic repeats or fails | Task completes → queued jobs; user runs `/learn` for corpus building |
| **Signal source** | Full trajectory (REPL + JSONL session) | Experience file + normalized `RuntimeEvent` stream |
| **What mutates** | Harness state H = (prompt, sub-agents, skills, memory) — CRUD in-session | Candidate knowledge / skill / hypothesis / evolution — files + SQLite |
| **Edit granularity** | Smallest CRUD delta on existing harness entry | Often whole new candidate file (skill draft, knowledge md) |
| **LLM in loop** | Yes — planning phase proposes the edit (background) | Jobs stay rule/parse; optional budgeted polish (`tick_refine_llm`, cap/day) rewrites note/patch JSON only — apply still `/inbox` |
| **Promotion** | Apply at next turn boundary; rollback by refinement ID | `/inbox` commit; never silent install to live skills |
| **Skills** | Markdown + **Python-backed** executables; harness skill = lightweight description | Agent Skills `SKILL.md`; draft → human approve → `skills/<name>/` |
| **Self-schedule** | Autonomous mode, heartbeats, schedules | **Out of scope** — post-task + idle curiosity only |

## What Prime does better (gaps today)

1. **Trajectory-first refinement** — edits are tied to “what was tried → what happened”, not
   keyword gaps in markdown.
2. **Incremental harness updates** — update one memory or skill note instead of minting
   another candidate file.
3. **Two-tier skills** — harness skill description (cheap, refine-able) vs packaged
   executable skill (skill-creator).
4. **Refinement audit trail** — each change records trigger + outcome + reversible ID.
5. **In-run adaptation** — harness readable mid-task via REPL; Methodus injects knowledge
   only at task start.

## What Methodus keeps (non-negotiable)

- **Prompt is not the database** — policy, promotion, conflict detection stay in Rust.
- **Evidence-first** — one executor output ≠ committed knowledge.
- **Human gate on global writes** — skills and Face/Method evolution through `/inbox`.
- **Executor-agnostic** — no IPython kernel requirement; Claude/Codex/Cursor adapters.

## Breakthrough directions (ordered)

### B1 — Trajectory-native distillation (M4+, started)

Use the **event stream** as primary input for execution-task skill drafts:
ordered tool steps with path/command snippets, pitfalls from errors and permission denials.
For `/learn` module-expert runs, parse the executor’s structured `## Skill` section instead
of templating from tool names.

*Prime borrow:* smallest relevant edit from trajectory, not from static heuristics alone.

### B2 — Refine job (bounded LLM, human-gated)

Add optional job kind `propose_refinement`:

```text
inputs: task_id, experience_id, optional existing skill/knowledge id
output: structured RefinementProposal { target, op: create|update, diff, evidence_refs[] }
```

Planning runs in the learning queue (token budget, dedupe). **Apply** only after inbox
commit — mirrors Prime’s plan/apply split while keeping Methodus trust model.

*Prime borrow:* LLM proposes; harness apply is fast and auditable.

### B3 — Harness notes vs executable skills

Introduce **`faces/<id>/notes/`** (or `harness/`): short prompt/memory entries
refinable without full SKILL.md ceremony. Promote to skill when the same note fires
≥ N times across tasks (frequency from Question/knowledge dedupe keys).

*Prime borrow:* H = (ρ, G, K, M) with different promotion cost per layer.

### B4 — Incremental skill merge

When `propose_skill` matches an existing committed skill (slug or embedding later),
emit **patch candidate** (append Procedure step, append Pitfall) instead of a parallel
`.candidates/` tree. Inbox shows diff view like Evolution.

*Prime borrow:* “smallest CRUD edit” instead of always `create_skill`.

### B5 — Learn ↔ execution feedback loop

After user commits learn-derived knowledge, tag scope for injection; on next execution
task in that Face/project, **count injections as hits**. When the same gap/question
**recurs** after those items were injected, lower confidence and open a mentor
question. Execution tasks that already distilled a note/patch/skill do not also mint
a parallel knowledge candidate (`/learn` ingest/survey still do).

*Prime borrow:* close the loop Prime gets from in-session memory, using events.

## `/learn` entry (product)

Single user-facing command. User supplies sources only; Rust selects
repo-survey / doc-ingest / module-expert. No `/study`, `/ingest`, `/survey` in the
slash palette — reduces “which pipeline?” cognitive load and keeps archiving unified
under `/inbox`.

## Recommended next implementation slice

1. ✅ Trajectory procedure steps + `## Skill` parsing (B1 partial)
2. ✅ `RefinementProposal` type + inbox renderer (B2 scaffold)
3. ✅ Harness notes directory + inject at task resolve (B3)
4. ✅ Skill patch candidates when name collides (B4)
5. ✅ Distill tightness — one note **or** patch **or** skill draft per execution task; bare `Use \`Tool\`` is not skill-worthy
6. ✅ Injection inventory in `.methodus/selected-context.md` + `.methodus/injected.md`
7. ✅ Budgeted LLM polish of note/patch candidates (`refine_llm`, daily cap); apply remains inbox-gated
8. ✅ Injection hits (`learning.injected`); note → skill after 3 uses, not 3 commits
9. ✅ Injection miss: same gap after inject → downrank + mentor question (`learning.injection_missed`)
10. ✅ Inbox exclusivity: execution distill XOR experience knowledge; `/learn` still writes knowledge

`propose_refinement` is a queued job (`JobKind::ProposeRefinement`). Distill is **rule-based**.
A later `Engine::tick_refine_llm` (TUI background, skipped while a user session runs) may
rewrite the candidate; `skip: true` rejects noise. Inbox tags: **N** (note), **P** (patch).
Apply is commit-gated. A committed note enqueues `propose_skill` after **3 injections**.

Defer Python-backed skills unless Methodus gains a sandboxed executor runtime — out of
scope for executor-agnostic design.
