# 06 — Agent CLI and connector protocol

`methodus agent` is a machine-facing, read-only protocol used by the official connector
Skill. It is not the maintainer workflow.

## 1. Contract

- no prompts, TUI, colors, spinners, or interactive fallback;
- stdout contains only the requested payload;
- stderr contains diagnostics safe for the runtime/user;
- stable exit codes and schema version;
- no graph, repository, configuration, feedback, or index mutation;
- no network or LLM calls;
- deterministic results for a fixed index and arguments;
- bounded nodes and estimated tokens.

The maintainer TUI (or an explicit `methodus doctor`) refreshes the SQLite projection.
An agent invocation only reads the last validated projection, so a connector cannot
silently rewrite the graph while a developer is working.

## 2. Commands

### `prepare`

```text
methodus agent prepare --goal <text> [--budget <tokens>]
                       [--scope personal,team] [--format markdown|json]
```

Returns the best entry Method, selected Knowledge facets, reusable Experience lessons,
selection rationale, status/source warnings, and lazy node IDs.

### `search`

```text
methodus agent search --query <text> [--type knowledge,method,experience]
                      [--kind <kind>] [--scope personal,team]
                      [--limit <n>] [--format markdown|json]
```

Returns metadata and match rationale, not full bodies.

### `get`

```text
methodus agent get <node-id> [--facet learn|decide|execute|evidence|all]
                 [--format markdown|json]
```

Returns one canonical node. Candidate and rejected nodes are never accessible through
this interface. Deprecated nodes require an explicit history flag; stale nodes always
include a warning.

### `related`

```text
methodus agent related <node-id> [--relation <type>] [--depth 1]
                     [--limit <n>] [--format markdown|json]
```

Depth is one in v1. This prevents accidental graph explosions and unpredictable token
use.

### `status`

```text
methodus agent status [--format markdown|json]
```

Reports protocol version, a stable index revision for the current projection, the
selected Team ID, Personal/Team root availability, and stale/error counts. The
revision must be derived from indexed content (not query time), so a runtime can tell
whether two responses came from the same graph snapshot.
It does not expose credentials or maintainer transcripts.

## 3. Markdown response

Example `prepare` response:

```markdown
# Methodus context

- protocol: 1
- index_revision: sha256:<content-derived revision>
- goal: Diagnose abnormal device shutdown
- estimated_tokens: 910 / 1200

## Method

### method/abnormal-shutdown-triage · Execute
Why selected: exact diagnostic intent and device/power scope.

1. Read the previous shutdown reason.
2. Inspect the final pre-shutdown window for crash/watchdog evidence.
3. Branch by controlled shutdown, crash, watchdog, or abrupt power loss.

## Knowledge

### knowledge/previous-shutdown-reason · Execute
...

## Experience lessons

### experience/device-x-power-loss
Missing persisted shutdown reason can itself indicate power loss before persistence.

## Lazy references

- knowledge/pre-shutdown-crash-detection

## Warnings

- knowledge/previous-shutdown-reason is stale: referenced source changed after a82c31f.
```

## 4. JSON envelope

```json
{
  "protocol_version": 1,
  "command": "prepare",
  "goal": "Diagnose abnormal device shutdown",
  "index_revision": "...",
  "estimated_tokens": 910,
  "budget_tokens": 1200,
  "items": [
    {
      "id": "method/abnormal-shutdown-triage",
      "node_type": "method",
      "facet": "execute",
      "status": "committed",
      "visibility": "team",
      "rationale": "...",
      "content": "...",
      "path": "teams/default/knowledge/previous-shutdown-reason.md",
      "content_hash": "...",
      "warnings": []
    }
  ],
  "lazy_ids": [],
  "warnings": []
}
```

Fields may be added compatibly within a protocol version; removing or changing field
meaning requires a version increment. Connector Skills declare the protocol range they
support.

The connector must treat `protocol_version`, `index_revision`, lifecycle status, and
warnings as data, not prose decoration. It may quote a node only together with its ID
and source path/hash when those fields are available.

## 5. Selection policy

- default budget: 1,200 estimated tokens;
- hard item count and per-item content caps;
- Method before supporting Knowledge, then compact Experience lessons;
- `Execute` and `Decide` preferred for action tasks;
- Personal and Team are both eligible unless scope is restricted;
- committed outranks stale;
- stale is included only when strongly relevant or explicitly requested;
- deprecated is history-only;
- candidate/rejected is excluded;
- graph expansion follows a bounded allowlist of relation types.

## 6. Exit codes

| Code | Meaning |
|---:|---|
| 0 | success, including a valid empty result |
| 2 | invalid arguments |
| 3 | Methodus home/index unavailable |
| 4 | requested node unavailable or not consumer-visible |
| 5 | schema/protocol incompatibility |
| 6 | index requires maintainer repair |

The connector should continue the user's task without Methodus for codes 3–6 and may
surface a concise note. It must never invent a successful Methodus response.

## 7. Connector triggering

The Skill should call `prepare` for substantial:

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
