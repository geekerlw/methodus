# 01 — Runtime integration

Methodus integrates with agent runtimes in two deliberately separate ways:

1. a **managed learning adapter** used only inside the maintainer TUI;
2. an **official connector Skill** used by ordinary agents to call the read-only CLI.

It does not launch or supervise ordinary coding sessions.

## 1. Learning adapter

The TUI owns a focused learning conversation. A learning adapter provides model turns
and selected research capabilities without attempting to reproduce the full coding
agent product.

The code-level seam is the existing `methodus_runtime::RuntimeAdapter` trait:

```rust
#[async_trait]
pub trait RuntimeAdapter: Send + Sync {
    async fn spawn(
        &self,
        input: SpawnInput,
    ) -> Result<(SessionHandle, Receiver<RuntimeEvent>), RuntimeError>;
    async fn resume(
        &self,
        executor_sid: &str,
        input: SpawnInput,
    ) -> Result<(SessionHandle, Receiver<RuntimeEvent>), RuntimeError>;
    async fn stop(&self, handle: &SessionHandle) -> Result<(), RuntimeError>;
}
```

`SpawnInput` carries the Learn prompt, launch directory, runtime session IDs,
the maintainer-selected permission mode, allowed tools, sandbox, and extra source
directories. `plan` maps to a read-only sandbox; `cautious` and `acceptEdits` map to
native confirmation/auto-edit behavior inside a workspace-write sandbox. No adapter
may emit a bypass-permissions flag.
The adapter is allowed to mediate this focused Learn run because it is a maintainer
workflow; it must never grow into a general task-session or workspace manager.
The adapter returns normalized `RuntimeEvent` values to the TUI. A future extraction
may introduce a narrower `LearningRuntime` wrapper, but it must preserve this boundary
and must not become an ordinary coding-session manager.

The current normalized event vocabulary includes:

```rust
pub enum LearnEvent {
    SessionStarted { session_id: String },
    AssistantText { text: String },
    Thinking { text: String },
    ToolCallStarted { id: String, name: String, input: Value },
    ToolCallCompleted { id: String, output: Value, exit_code: Option<i32> },
    TurnCompleted { stop_reason: Option<String> },
    Result { is_error: bool, text: String, session_id: Option<String>, .. },
    Error { message: String },
}
```

The concrete v1 type is `methodus_domain::RuntimeEvent`; the pseudocode above omits
serde and permission-denial fields for readability. CandidateSet detection happens in
the core/TUI after a completed assistant response rather than as a separate adapter
event.

Adapters may use a runtime's structured CLI/API surface. PTY screen scraping is out of
scope. Runtime-specific session IDs are stored only to resume Learn sessions.

Methodus exposes one portable Learn permission selector and maps it conservatively to
each runtime. The maintainer owns the choice, it is persisted with the Learn run, and
the selected runtime still owns individual approval prompts and enforcement.

## 2. Learning protocol

Every adapter receives the same runtime-independent protocol:

1. restate the goal and scope;
2. inspect relevant committed graph nodes;
3. identify assumptions and missing evidence;
4. ask the maintainer only consequential questions;
5. investigate attached sources and seek counterexamples;
6. separate fact, inference, contradiction, and unknown;
7. propose a typed candidate set, never canonical writes.

The protocol is versioned with Methodus. It is not installed as a general runtime
Skill and is not copied into ordinary task workspaces.

## 3. Connector Skill

Methodus ships one small connector for each runtime's Skill format. The connector is
the only Skill that Methodus installs or updates.

The connector contains no Team or Personal knowledge. It teaches the agent to:

- call `methodus agent prepare` before substantial diagnosis, design, research,
  document, or presentation work;
- skip Methodus for trivial questions and mechanical edits;
- call `search`, `get`, or `related` only when the prepared bundle is insufficient;
- treat `stale` content as a hypothesis that must be revalidated;
- cite node IDs and evidence when relying on Methodus;
- continue normally when Methodus is unavailable;
- never call maintainer write paths.

Methodus does not guarantee that every runtime triggers a Skill identically. Adapter
tests must verify spawn/resume argument mapping, event parsing, and graceful fallback
for every supported runtime.

## 4. Installation boundary

Human-facing entry points remain small:

```text
methodus          open the maintainer TUI
methodus setup    install/update the official connector (`--uninstall` removes only a Methodus-owned file)
methodus doctor   report CLI, connector, repository, and index health
```

`setup` installs the official connector into the selected runtime's user Skill
directory. Installation must be ownership-aware: Methodus may update or uninstall a
file only when it carries the Methodus marker/version, and it must never overwrite an
unrelated user Skill. `--force` means “replace this Methodus-owned connector after
showing the target”, not “take over an arbitrary file”.

`doctor` reports one of `missing`, `current`, or `drifted` for each supported runtime.
Uninstall is explicit and only removes a connector that is still Methodus-owned.
Neither setup nor doctor changes runtime permissions, project files, or graph data.

## 5. Capability matrix

Each runtime adapter reports capabilities rather than relying on a global lowest
common denominator.

| Capability | Learning adapter | Connector Skill |
|---|---:|---:|
| Stateful Learn conversation | required | not applicable |
| Structured event stream | preferred | not applicable |
| Source attachment | required through Methodus mediation | not applicable |
| Candidate-set output | required | forbidden |
| Invoke read-only CLI | not required | required |
| Ordinary coding-session control | forbidden | owned by runtime |
| Permission management | forbidden | owned by runtime/user |

## 6. Failure behavior

- If a learning runtime fails, keep the active conversation/error visible, persist the
  run event, and never publish partial output automatically. The maintainer can resume
  the same executor when its ID is available, retry with the selected runtime, or
  explicitly switch runtimes.
- If the connector cannot find `methodus`, it should tell the agent to continue
  without Methodus rather than block the task.
- If CLI output is invalid or over budget, the connector treats it as unavailable and
  must not invent Methodus claims.
- Connector compatibility is checked with golden invocation tests, not assumed from
  prose alone.
