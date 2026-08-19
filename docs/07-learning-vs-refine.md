# 07 — Learning and Graph Evolution vs Prime Agent `/refine`

Reference: [Prime Agent](https://github.com/PrimeIntellect-ai/prime-agent) (Continual
Harness, Aug 2026).

Both projects seek durable improvement. Prime refines one agent harness from its
in-session trajectory. Methodus grows a Markdown-first knowledge graph around native
agent runtimes, with a deliberate handoff boundary and human review.

## Side-by-side

| Dimension | Prime Agent `/refine` | Methodus |
|---|---|---|
| Unit of improvement | Harness prompt, memory, sub-agents, skills | Knowledge node/facet, typed graph edge, Experience, Skill, Method, Face lens |
| Interaction boundary | Inside the harness REPL | Before/after native Claude/Codex/Cursor interaction |
| Primary source | Full in-session trajectory | Learning capsule outputs, outcome review, artifacts, and optional managed events |
| Learning entry | Refine after tactic repeats/fails | Explicit `Learn` session or returned work task |
| Durable representation | Harness-owned state | Independent Markdown files + SQLite graph index |
| Apply | Turn-boundary update / rollback ID | Candidate → evidence/conflict check → human review → commit |
| Context reuse | Harness memory in later turns | Minimal selected Execute facets + lazy graph references in a future capsule |

## What Methodus borrows

1. **Trajectory/outcome awareness.** Distillation must be tied to what happened and
   whether it worked, not merely to keywords in a note.
2. **Smallest useful change.** Prefer adding an edge, revising one Execute facet, or
   patching a Skill over creating a large duplicate document.
3. **A low-cost memory tier.** A short candidate rule can be useful before it has
   earned promotion into a full Skill.
4. **Auditable evolution.** Every proposed graph change records trigger, evidence,
   diff, and later outcome.

## What Methodus intentionally does differently

- It does not need to own the conversation transcript to learn. Native handoff is the
  default; the user can return an outcome, attach artifacts, and use a structured
  retrospective. Managed event streams are an optional richer source.
- It treats knowledge as a graph of standalone Markdown nodes, not as opaque prompt
  memory. A learner can read it without an agent; a future runtime can use it without
  importing the previous runtime's state.
- It separates **Learn** and **Execute** facets. A 5W2H explanation helps a person;
  a compact rule/pitfall slice helps an agent without consuming the whole note.
- It requires review for global graph/skill changes. No model output becomes canonical
  merely because it appeared in a successful session.

## Learning session contract

`/learn <topic>` creates a `Task(kind=learn)`. The compiler adds:

```text
learning skill + source artifacts + prerequisite nodes + neighbor nodes
→ native agent learning session
→ candidate atomic Markdown Knowledge node
→ review links, evidence, confidence, and facets
→ commit to graph
```

The candidate must contain, at minimum:

- stable ID/title/summary/scope/source metadata;
- a Learn facet in 5W2H form;
- an Execute facet or an explicit statement that it is not operational knowledge;
- links to prerequisites, alternatives, Skills, and Experiences when known;
- evidence and uncertainty/conflict notes.

## Context feedback loop

Graph retrieval is not automatically a good outcome. For each selected context item,
Methodus records `useful`, `unused`, `misleading`, or `unknown` on task return. The
resolver uses that feedback alongside scope, typed links, and evidence confidence.

```text
graph node selected → facet injected → task/learn return → outcome review
        ↑                                               │
        └────── selection score and/or graph proposal ─┘
```

This preserves Prime's “improve from experience” instinct while avoiding a growing,
unreviewed prompt and preserving the user's preferred agent TUI.

## Deferred work

- Automated extraction of a native agent transcript is optional and must never rely on
  ANSI scraping.
- Semantic/embedding graph links are candidate suggestions, not authoritative edges.
- Automatic Skill promotion requires repeated positive use and review; it is not a
  background self-modification loop.
