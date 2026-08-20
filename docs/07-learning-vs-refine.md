# 07 — Deliberate learning and controlled refinement

Methodus learns through an explicit maintainer workflow. It does not observe every
coding-agent session and does not automatically evolve Skills, Methods, or canonical
Knowledge from trajectories.

## 1. Learn versus refinement

| Dimension | Learn | Refinement |
|---|---|---|
| Trigger | Maintainer starts a learning goal | Existing node is stale, contradicted, duplicated, or inadequate |
| Input | Selected sources, existing graph context, maintainer answers | Existing node, evidence delta, related Experience |
| Runtime role | Investigate, challenge, synthesize candidates | Propose the smallest evidence-backed change |
| Output | CandidateSet: zero or more Knowledge/Method/Experience + relations | Diff or replacement candidate against explicit target |
| Canonical write | Never automatic | Never automatic |
| Decision | Maintainer Review | Maintainer Review |

## 2. Deliberate-learning state machine

```text
Goal
  → Scope challenge
  → Existing-knowledge inspection
  → Evidence plan
  → Investigation
  → Counterexample/adversarial verification
  → Consequential maintainer questions
  → Synthesis
  → CandidateSet
  → Review
```

The runtime must keep four categories separate:

- **fact**: directly supported by a cited source;
- **inference**: reasoned conclusion with stated assumptions;
- **contradiction**: sources or nodes that cannot both hold under the same scope;
- **unknown**: unresolved and not safe to convert into an operational rule.

Unknowns are valid Learn outcomes. A run may produce no candidate when evidence is
insufficient.

## 3. Source discipline

Learn receives only sources the maintainer attaches or roots the maintainer authorizes.
In the current v1 adapter, the launch working directory is the available source root;
`@` completion can attach paths under it plus explicitly entered absolute/`~` paths.
Git history, docs, URLs, and scrubbed logs are protocol targets as source adapters are
added, not automatic ingestion.

For each consequential claim, synthesis records:

- source locator and revision/fingerprint;
- whether support is direct or inferred;
- contradictory evidence;
- scope and invalidation conditions;
- validation date.

Raw sensitive logs should remain temporary run inputs. Durable Team nodes store
scrubbed patterns, field meaning, and links to authoritative code/specifications.

## 4. Candidate atomization

One Learn run is not one atom. The runtime proposes the smallest reusable set without
fragmenting every paragraph into a node.

Prefer a separate node when a conclusion:

- has its own applicability boundary;
- can be reused independently;
- has a distinct evidence set or freshness lifecycle;
- participates in different graph relations;
- is a specific Experience rather than a reusable rule.

Prefer one node with multiple facets when Learn, Decide, Execute, and Evidence describe
the same reusable conclusion.

The maintainer can split, merge, exclude, or retarget drafts before Review.

The CandidateSet may additionally contain `relations`, `unresolved_questions`, and
`contradictions`. Methodus resolves relation endpoints against candidate indexes or
canonical IDs, writes the links into draft frontmatter, and leaves unresolved
references visible for Review instead of guessing.

## 5. Knowledge versus Method versus Experience

- Create **Knowledge** for a reusable conclusion, diagnostic signal, design decision,
  constraint, change narrative, or procedural step.
- Create **Method** for a repeatable way of conducting a class of work, including
  phases, evidence standard, output contract, and checks.
- Create **Experience** only for a concrete case with evidence and outcome that can
  validate or contradict reusable content.

A learning conversation by itself is not an Experience. Its transcript remains under
the Learn run for audit and resume.

## 6. Controlled refinement

Refinement proposes the smallest useful change:

- update one facet;
- add or remove a typed relation;
- mark an old node superseded/deprecated;
- merge a duplicate candidate into an explicitly selected target;
- revalidate a stale node against changed evidence;
- extract a reusable lesson from a reviewed Experience.

Refinement is an explicit maintainer action. A stale source can trigger a suggestion,
but it never causes an automatic rewrite or promotion.

It must not silently rewrite a node, overwrite a Team file, install a Skill, or infer
success merely because an agent produced an answer.

## 7. Review requirements

Review verifies:

- correct type and atom boundary;
- scope, kind, and summary;
- fact/inference/unknown separation;
- source validity and freshness;
- Execute safety and stopping criteria;
- duplicate and conflict candidates;
- relation direction and target existence;
- Personal versus Team visibility;
- explicit rationale for merge, deprecate, revalidate, or evidence waiver.

Only committed Personal/Team nodes are visible to the Agent CLI.

## 8. Runtime Skill boundary

Methodus does not generate or evolve runtime Skills. It ships one official connector
whose sole purpose is to call the read-only Agent CLI. Methods remain runtime-neutral;
concrete web, document, presentation, or coding capabilities belong to the selected
agent runtime.
