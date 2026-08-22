# 03 — Data model

## 1. Source of truth

Two stores have distinct authority:

- **Markdown/YAML + Git**: canonical Knowledge, Method, Experience, relations, and
  source references;
- **SQLite**: derived graph/search indexes plus local Learn, Review, validation, and
  publication state.

The file always wins over its indexed row. Direct edits are supported; a content-hash
change triggers re-indexing.

## 2. Local layout

```text
~/.methodus/
├── config.yaml
├── state.db
├── methodus.lock
├── personal/
│   ├── knowledge/<id>.md
│   ├── methods/<id>.md
│   ├── experiences/<id>.md
│   └── candidates/<candidate-id>.md
├── teams/
│   └── <team-id>/              # local Git checkout; external path mapping is planned
│       ├── methodus.yaml
│       ├── knowledge/
│       ├── methods/
│       └── experiences/
├── runs/
│   ├── reviews.jsonl           # append-only maintainer review audit
│   └── <run-id>/                # operational Learn state, never queried by Agent CLI
│       ├── state.yaml
│       ├── events.jsonl         # append-only user/assistant/runtime events
│       ├── assistant.md         # last synthesized runtime response
│       ├── sources.yaml         # attached source locators and fingerprints
│       └── publish-plan.md      # only for publish_<id> runs
├── workspaces/
│   ├── learn/<run-id>/           # native Learn runtime cwd
│   └── use/<session-id>/         # native Use runtime cwd
└── connectors/                 # connector ownership/version metadata
```

There is no ordinary coding-task workspace and no graph materialization into agent
projects. Methodus does create managed runtime workspaces for its own Learn and Use
handoffs; these are operational locations, not project copies.
The runtime keeps the live conversation in the TUI and writes the event stream,
synthesized response, source manifest, and candidate files when a CandidateSet is
detected. These are operational records and must never become Agent-visible graph
content until a maintainer promotes a candidate.

## 3. Common node frontmatter

```yaml
---
id: knowledge/abnormal-shutdown-triage
title: Abnormal shutdown triage
node_type: knowledge
kind: procedure
status: committed
visibility: team
scope: device-runtime/power
summary: Determine whether the previous shutdown was controlled, crash-driven, watchdog, or power loss.
tags: [shutdown, crash, watchdog, diagnostics]
confidence: 0.9
validated_at: 2026-08-20
evidence_waiver: false
sources:
  - id: source/shutdown-reason-code
    type: git-file
    repository: device-runtime
    path: src/power/shutdown_reason.c
    revision: a82c31f
    fingerprint: sha256:...
links:
  next_step:
    - knowledge/pre-shutdown-crash-detection
  validated_by:
    - experience/device-x-power-loss
---
```

Required fields are `id`, `title`, `node_type`, `kind`, `status`, `visibility`, and
`summary`. Team publication additionally requires at least one evidence source for
Knowledge and Method unless the maintainer records `evidence_waiver: true` in
frontmatter.

## 4. Knowledge bodies

```markdown
## Learn

Human explanation. 5W2H is recommended when it clarifies the concept.

## Decide

Applicability, alternatives, boundaries, and trade-offs.

## Execute

Ordered steps, conditions, branches, checks, pitfalls, and stop criteria.

## Evidence

What was inspected, what remains inferred, contradictions, and open questions.
```

Facet headings are optional when irrelevant, but `Execute` is required for procedural
Knowledge that should be usable by a consuming Runtime.

## 5. Method nodes

```yaml
---
id: method/competitive-research
title: Evidence-led competitive research
node_type: method
kind: research-workflow
status: committed
visibility: personal
summary: Compare competitors on a common evidence-backed frame and separate fact from inference.
tags: [research, competitors]
links:
  requires: []
  validated_by: []
---
```

Recommended body:

```markdown
## Intent
## Inputs and clarifying questions
## Phases
## Evidence standard
## Output contract
## Quality checks
## Failure modes
```

A Method is declarative and runtime-independent. It may describe required capabilities
such as web research or presentation generation, but never embeds or installs a
runtime Skill.

## 6. Experience nodes

```yaml
---
id: experience/device-x-power-loss
title: Device X intermittent power-loss investigation
node_type: experience
kind: incident
status: committed
visibility: team
summary: Shutdown-reason storage was absent because power was lost before persistence.
outcome: resolved
occurred_at: 2026-07-11
links:
  used_method: [method/abnormal-shutdown-triage]
  validates: [knowledge/previous-shutdown-reason]
---
```

Recommended body:

```markdown
## Situation
## Observations
## Investigation and decisions
## Outcome
## Reusable lesson
## Evidence
```

Agent retrieval uses the reusable lesson and summary by default, not the whole case.

## 7. Typed relations

Relations are directional in source files and indexed in both directions.

| Relation | Typical source → target |
|---|---|
| `requires` | Method/Knowledge → prerequisite Knowledge |
| `next_step` | Procedure step → next diagnostic Knowledge |
| `indicates` | Signal → possible cause |
| `emitted_by` | Signal → component |
| `implemented_by` | Decision → Change Narrative |
| `introduced_by` | behavior/signal → Change Narrative |
| `supersedes` | new Decision → old Decision |
| `conflicts_with` | claim ↔ incompatible claim |
| `validated_by` | Knowledge/Method → Experience |
| `contradicted_by` | Knowledge/Method → Experience |
| `derived_from` | candidate → source node/run |

Unknown relation names are retained for forward compatibility but produce a validation
warning until registered.

## 8. Sources and freshness

Source descriptors identify evidence without copying an entire repository into the
graph.

Initial source types:

- `git-file`: repository ID, path, revision, content fingerprint;
- `git-commit`: repository ID and commit ID;
- `document`: local path or stable URL plus fingerprint/version;
- `log-pattern`: scrubbed template/fields and the code or specification that emits it;
- `manual`: maintainer assertion plus rationale and optional reviewer.

Freshness states are derived per source:

- `current`: fingerprint/revision still matches;
- `changed`: source exists but changed;
- `missing`: source cannot be resolved;
- `unchecked`: validation has not run or requires unavailable access.

A committed node becomes `stale` when a material source is changed or missing. This
transition does not alter its prose. Revalidation requires a maintainer Review action.

## 9. CandidateSet

A Learn run proposes a set rather than one mandatory document:

```json
{
  "graph_review": {
    "searched": true,
    "relevant_nodes": [{"id": "knowledge/existing", "reason": "same boundary"}],
    "no_match_reason": null
  },
  "candidates": [
    {
      "type": "method",
      "kind": "diagnostic-workflow",
      "title": "Abnormal shutdown triage",
      "summary": "Separate controlled exit, crash, watchdog, and power loss.",
      "disposition": "new",
      "target": null,
      "patch": null,
      "learn": "...",
      "decide": "...",
      "execute": "...",
      "evidence": "...",
      "tags": ["shutdown", "diagnostics"]
    },
    {
      "type": "knowledge",
      "kind": "diagnostic-signal",
      "title": "Previous shutdown reason",
      "summary": "Read the persisted reason before inspecting the final log window.",
      "disposition": "revise",
      "target": "knowledge/previous-shutdown-reason",
      "patch": "Update the Execute facet to read the persisted reason before the final log window.",
      "learn": "...",
      "execute": "...",
      "evidence": "...",
      "outcome": "resolved",
      "occurred_at": "2026-07-11",
      "tags": ["shutdown"]
    }
  ],
  "relations": [
    {"from": "candidate-0", "relation": "validated_by", "to": "candidate-1"}
  ],
  "unresolved_questions": [],
  "contradictions": [],
  "runtime_skills": [
    {"name": "repo-survey", "runtime": "claude-code", "outcome": "useful", "reason": "located the source history"}
  ]
}
```

The runtime returns this object in a fenced JSON block. Methodus assigns operational
candidate IDs and writes one Markdown file per draft under `personal/candidates/`.
The JSON contract may include `relations`, `unresolved_questions`, and
`contradictions`. Methodus resolves candidate references to operational IDs, writes
typed links into each draft, and validates the result during Review. Relations remain
Markdown source-of-truth after promotion.

The maintainer may split, merge, edit, exclude, or retarget drafts before sending them
to Review. Candidate IDs are operational; canonical IDs are assigned or confirmed at
commit.

## 10. Lifecycle and visibility

Node status:

```text
candidate → committed → stale → committed
                       ↘ deprecated
candidate → rejected
```

Visibility:

- `personal`: canonical file lives under the Personal root;
- `team`: canonical file lives in a configured Team repository.

Promotion is a reviewed file move/copy plus link validation and a Git publication
plan. `visibility: team` without residence in a Team repository is invalid.

## 11. SQLite projection

The exact migrations may evolve, but the logical projection includes. The repository
still carries legacy task/session/workspace columns for existing homes; they are
compatibility storage and are not part of the active Methodus workflow.

```sql
graph_nodes(id, node_type, title, summary, status, visibility,
            scope, tags_json, confidence, validated_at, path,
            repository_id, content_hash, indexed_at)

graph_edges(from_id, relation, to_id, source_path)
graph_sources(node_id, source_id, source_type, locator_json,
              recorded_fingerprint, current_fingerprint,
              freshness, checked_at)

learn_runs(id, goal, runtime, executor_sid, status, started_at, updated_at, completed_at)
learn_events(id, run_id, sequence, event_type, payload_json, created_at)
candidate_drafts(id, run_id, proposed_type, proposed_id, path, decision)
reviews(id, target_id, action, rationale, created_at)
repositories(id, kind, path, remote, revision, sync_state, checked_at)
connector_installs(runtime, path, version, checked_at)
```

`kind`, `sources`, and `evidence_waiver` remain Markdown frontmatter and are read
from the authored file for validation and filtering; they do not require a SQLite
schema column in the current projection.

Candidate/review/run tables are local operational state. Published Team repositories
contain only canonical Markdown/YAML, never the maintainer transcript database. The
current implementation keeps Learn state and review audit in `runs/` files; the
logical rows above describe the stable concepts, not a requirement to materialize
each one in SQLite.

## 12. Validation gates

Publication is blocked by:

- invalid or duplicate IDs;
- invalid frontmatter or unsupported lifecycle transitions;
- missing required relation targets;
- unresolved Git merge conflicts;
- candidate/rejected content in published canonical directories;
- Team Knowledge/Method without evidence or an explicit waiver.

Warnings include stale sources, unknown relation types, weak summaries, orphan nodes,
and likely semantic duplicates. Warnings require acknowledgement but do not all block
publication. Errors block a publish plan until the maintainer resolves them or records
the explicitly supported waiver.
