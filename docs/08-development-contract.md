# 08 — Development contract

This document is the short checklist for implementing or reviewing changes. The
longer product rationale lives in `00-product.md`–`07-learning-vs-refine.md`.

## 1. Ownership boundary

| Concern | Methodus owns | Runtime/user owns |
|---|---|---|
| Maintainer Learn conversation | prepare protocol/run record, native terminal handoff, candidate import | multi-turn dialogue, tools, terminal rendering, approvals |
| Canonical knowledge | Markdown layout, graph validation, lifecycle, review actions | direct Git edits are allowed but must pass validation |
| Agent consumption | read-only `methodus agent` protocol and connector instructions | when to call it, how to use context, final reasoning |
| Permissions | select/display a bounded Learn mode and map it to native flags | native allow/deny enforcement and ordinary coding permissions |
| Ordinary coding session | nothing | Claude Code, Codex, Cursor, or another native runtime |
| Team publication | status, validation, diff, publish plan | Git commit, push, merge, remote review |

A change that makes Methodus proxy a normal Agent conversation, create a task
workspace, copy graph content into a project, or write graph content from a consumer
Agent violates the contract.

## 2. Canonical file rules

- Markdown/YAML is the source of truth. SQLite rows are disposable projections.
- Personal content lives under `personal/{knowledge,methods,experiences}`.
- Review-only drafts live under `personal/candidates`; they are never returned by the
  Agent CLI.
- Team content lives under `teams/<id>/{knowledge,methods,experiences}`. The seeded
  Team is `teams/default`; the selected Team ID is stored in `config.yaml`.
- Learn operational state lives under `runs/`; it is not graph content.
- A node ID is stable across file moves. File paths are not IDs.
- Source evidence is a locator plus revision/fingerprint, not an uncontrolled copy of
  production logs or an entire repository.

Every canonical node should have frontmatter for `id`, `title`, `node_type`, `kind`,
`status`, `visibility`, and `summary`. Body facets are Markdown headings: `Learn`,
`Decide`, `Execute`, and `Evidence`. 5W2H belongs inside `Learn` when useful; it does
not dictate atom boundaries.

## 3. Lifecycle gates

```text
candidate → committed → stale → committed
candidate --reject--> deleted
```

- Only Review can move a candidate to canonical `committed` content.
- A stale transition is a warning derived from source freshness; it never rewrites
  prose automatically.
- Candidate nodes are excluded from normal Agent queries; Review rejection deletes
  the candidate. Legacy rejected/deprecated files are history-only cleanup items.
- Merge always names a concrete target; never infer a target from ranking alone.

## 4. Learn output contract

The deliberate-learning runtime must challenge scope, inspect evidence, seek
counterexamples, ask consequential questions, and separate fact/inference/
contradiction/unknown. When the maintainer explicitly finalizes the learning, it writes
the supplied run-specific return artifact with a fenced JSON object:

```json
{"candidates":[{"type":"knowledge|method|experience","kind":"...","title":"...","summary":"...","learn":"...","decide":"...","execute":"...","evidence":"...","outcome":"...","occurred_at":"...","tags":["..."]}],"relations":[{"from":"candidate-0","relation":"validated_by","to":"candidate-1"}],"unresolved_questions":[],"contradictions":[],"runtime_skills":[{"name":"...","runtime":"claude-code","outcome":"useful","reason":"..."}]}
```

Methodus assigns operational IDs and writes one Markdown draft per candidate. Runtime
output is never treated as committed just because JSON parses. Candidate relation
references may use `candidate-<index>`, a canonical ID, or an exact candidate title;
unresolved endpoints remain visible for Review instead of being silently guessed. If
evidence is not sufficient, the runtime should ask another focused question or return
no candidates.

Runtime Skill observations are optional evidence in a candidate; they do not cause
Skill installation or evolution.

## 5. Agent CLI contract

The connector may invoke only:

```text
methodus agent prepare --goal <text> [--budget <n>] [--scope personal,team]
methodus agent search --query <text> [--type ...] [--kind ...] [--scope ...]
methodus agent get <id> [--facet ...] [--history]
methodus agent related <id> [--relation ...] [--depth 1]
methodus agent status
```

The process opens SQLite read-only, does not migrate or sync, never calls an LLM, and
prints only the requested Markdown/JSON payload to stdout. Results are bounded by
item count and estimated tokens. On failure, the connector continues the user task
without inventing Methodus context.

## 6. TUI invariants

- The default view is Learn chat; slash commands open management panels.
- `Shift+Enter` inserts a newline; plain Enter submits.
- CJK/IME and bracketed paste must preserve text and cursor boundaries.
- `@` completion resolves launch-cwd paths plus absolute and `~` paths; accepting a
  directory never inserts a breaking space.
- Arrow keys navigate lists. Press `f` to enter visible filter mode; `j/k` are not
  hidden list navigation. Review action keys remain available outside filter mode.
- `q` is ordinary text. Empty-input double `Ctrl+C` exits within the documented
  window. `/quit` exits explicitly.
- Filters are visible in panel titles and clear with `Esc`.
- Detail views show complete Markdown; `g` opens a focused one-hop neighborhood of
  active (`committed` or `stale`) nodes only. Rejected nodes remain list-visible for
  cleanup, but rejected/candidate/deprecated nodes and their edges are excluded from
  active graph navigation.
- Canonical-node deletion removes the Markdown source, records the action, and
  re-syncs the projection. Review rejection follows the same removal rule.
- Permission text belongs beside the Learn composer; runtime name belongs in the top
  bar; do not duplicate either in a footer help strip.

## 7. Change checklist

Before merging a feature:

1. update the relevant product/data/protocol/TUI document;
2. prove the change respects the ownership and lifecycle gates above;
3. add a focused unit or CLI fixture for parsing, filtering, lifecycle, or output;
4. run `cargo fmt --check`, `cargo check --workspace`, `cargo test --workspace`, and
   `git diff --check`;
5. manually smoke-test `methodus --help`, `methodus agent --help`, and a temporary
   `METHODUS_HOME` query path without allowing the Agent CLI to write.

For connector changes, inspect ownership/version behavior against an unrelated Skill
file and verify that `doctor` reports `missing`, `current`, or `drifted` without
mutating the runtime. For graph changes, verify that deleting or moving a Markdown
source removes the old SQLite projection on the next maintainer refresh.
