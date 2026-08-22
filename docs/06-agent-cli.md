# 06 — Agent CLI and connector protocol

`methodus agent` is a machine-facing, read-only protocol used by the official connector
Skill. It is not the maintainer workflow and it does not answer the user's question on
behalf of the native runtime.

## 1. Contract

- no prompts, TUI, colors, spinners, or interactive fallback;
- stdout contains only the requested payload;
- stderr contains diagnostics safe for the runtime/user;
- stable exit codes and schema version;
- no graph, repository, configuration, feedback, or index mutation;
- no network or LLM calls;
- the manifest is a complete inventory of consumer-visible nodes for the requested
  scope; it does not perform question-based context selection;
- node bodies are read explicitly with `get`, so the native runtime decides what is
  relevant and how much evidence it needs.

The maintainer TUI (or an explicit `methodus doctor`) refreshes the SQLite projection.
An Agent invocation only reads the last validated projection, so a connector cannot
silently rewrite the graph while a developer is working.

## 2. Commands

### `manifest`

```text
methodus agent manifest [--scope personal,team] [--format markdown|json]
```

The `environment` alias is accepted for callers that prefer that name. The response
contains the graph snapshot revision, Methodus home, selected Team, Personal/Team
directory structure, graph roots, and every consumer-visible committed or stale
Knowledge, Method, and Experience node. Each item contains its stable ID, summary,
tags, facets, evidence references, relative path, and absolute path. Candidate,
rejected, and deprecated nodes are not listed.

This is the only first step for the connector. It is an environment/inventory contract,
not a preselected answer bundle.

### `search`

```text
methodus agent search --query <text> [--type knowledge,method,experience]
                      [--kind <kind>] [--scope personal,team]
                      [--limit <n>] [--format markdown|json]
```

Returns bounded metadata and lexical match rationale. It is an explicit fallback
locator, not the connector's default context-selection mechanism and not full-body
evidence.

### `get`

```text
methodus agent get <node-id> [--facet learn|decide|execute|evidence|all]
                 [--history] [--format markdown|json]
```

Returns one canonical node body. Candidate and rejected nodes are never accessible
through this interface. Deprecated nodes require an explicit history flag; stale nodes
always include a warning.

### `related`

```text
methodus agent related <node-id> [--relation <type>] [--depth 1]
                     [--limit <n>] [--format markdown|json]
```

Depth is one in v1. This prevents accidental graph explosions and unpredictable output.

### `status`

```text
methodus agent status [--format markdown|json]
```

Reports protocol version, a stable index revision for the current projection, the
selected Team ID, Personal/Team root availability, and stale/error counts. The
revision is derived from indexed content rather than query time, so a runtime can tell
whether two responses came from the same graph snapshot. It does not expose credentials
or maintainer transcripts.

## 3. Manifest response

Example Markdown:

```markdown
# Methodus graph environment

- protocol: 1
- command: manifest
- index_revision: sha256:<content-derived revision>
- home: /Users/example/.methodus
- selected_team: default
- visible_nodes: 2

## Directory structure

- personal/knowledge
  /Users/example/.methodus/personal/knowledge
- personal/methods
  /Users/example/.methodus/personal/methods
- teams/default/knowledge
  /Users/example/.methodus/teams/default/knowledge
- teams/default/methods
  /Users/example/.methodus/teams/default/methods
- teams/default/experiences (missing)
  /Users/example/.methodus/teams/default/experiences

## Graph roots

- /Users/example/.methodus/personal/knowledge
- /Users/example/.methodus/personal/methods

## Consumer-visible inventory

### method · Shutdown triage

- id: method/shutdown-triage
- status: committed
- visibility: personal
- kind: diagnosis
- summary: A repeatable shutdown investigation.
- relative_path: personal/methods/shutdown-triage.md
- absolute_path: /Users/example/.methodus/personal/methods/shutdown-triage.md
- facets: Decide, Execute
```

The JSON form has the same fields:

```json
{
  "protocol_version": 1,
  "command": "manifest",
  "index_revision": "sha256:<content-derived revision>",
  "home": "/Users/example/.methodus",
  "selected_team": "default",
  "directory_structure": [
    {
      "path": "personal/knowledge",
      "absolute_path": "/Users/example/.methodus/personal/knowledge",
      "exists": true
    },
    {
      "path": "teams/default/knowledge",
      "absolute_path": "/Users/example/.methodus/teams/default/knowledge",
      "exists": true
    }
  ],
  "graph_roots": ["/Users/example/.methodus/personal/methods"],
  "items": [
    {
      "id": "method/shutdown-triage",
      "node_type": "method",
      "title": "Shutdown triage",
      "status": "committed",
      "visibility": "personal",
      "summary": "A repeatable shutdown investigation.",
      "path": "personal/methods/shutdown-triage.md",
      "absolute_path": "/Users/example/.methodus/personal/methods/shutdown-triage.md",
      "facets": ["Decide", "Execute"],
      "tags": [],
      "sources": []
    }
  ],
  "warnings": []
}
```

Fields may be added compatibly within a protocol version; removing or changing field
meaning requires a version increment. Connector Skills declare the protocol range they
support.

## 4. Runtime reading protocol

The connector follows this sequence:

```text
manifest
  → native runtime semantically compares the inventory, selected Team, and directory structure with the user's question
  → get selected nodes with the needed facets
  → related for authored graph neighbors when useful
  → revalidate stale claims against the current repository
  → answer with facts, inferences, recommendations, and unknowns separated
```

Methodus does not rank or preselect the answer context. The runtime must cite node IDs
and source references when a claim materially relies on Methodus. If no relevant
committed evidence is found, it must say so rather than inventing a Methodus-backed
answer.

## 5. Exit codes

| Code | Meaning |
|---:|---|
| 0 | success, including a valid empty manifest |
| 2 | invalid arguments |
| 3 | Methodus home/index unavailable |
| 4 | requested node unavailable or not consumer-visible |
| 5 | schema/protocol incompatibility |
| 6 | index requires maintainer repair |

The connector should continue the user's task without Methodus for codes 3–6 and may
surface a concise note. It must never invent a successful Methodus response.

## 6. Connector triggering

The Skill should call `manifest` for substantial:

- diagnosis and incident investigation;
- architecture/design decisions;
- code-behavior or change-history questions;
- competitive research;
- formal document and presentation work.

It should normally skip trivial Q&A, mechanical edits, formatting, and tasks with no
plausible benefit from Personal/Team methods or engineering memory. A user can always
explicitly ask the agent to use Methodus.

Runtime permissions are outside the protocol. Methodus neither edits nor assumes the
user's allow/deny policy.

Connector lifecycle is maintainer-side rather than Agent-side:

```text
methodus setup --runtime claude-code
methodus setup --runtime claude-code --force
methodus setup --runtime claude-code --uninstall
methodus doctor
```

`--force` can replace only a connector carrying the Methodus ownership marker. An
unrelated Skill is never overwritten or removed; `doctor` reports it as `drifted`.
