# 01 — Runtime integration

Methodus integrates with agent runtimes in three deliberately separate ways:

1. a **native Use handoff** used from the maintainer TUI for graph-backed questions;
2. a **native Learn handoff** used from the maintainer TUI for deliberate research;
3. an **official connector Skill** used by ordinary agents to call the read-only CLI.

It does not launch or supervise ordinary coding sessions. Use and Learn both leave the
Methodus alternate screen and restore it after the native runtime exits; only Learn has
a durable run and return-artifact import.

## 1. Native Learn handoff

The TUI prepares a focused Learn run, then deliberately leaves its alternate screen
and yields the same terminal to Claude Code, Codex, or Cursor. The selected runtime
owns the entire multi-turn conversation, tool display, approval prompts, and terminal
rendering. When it exits, Methodus restores its TUI and imports only the explicit
return artifact for Review.

Methodus never screen-scrapes or proxies the native TUI. It does not create a task
workspace or supervise ordinary coding sessions.

`SpawnInput` and `RuntimeAdapter` remain a compatibility seam for non-interactive
integration tests and future programmatic use; they are not the execution path for
the interactive Learn experience.

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

`SpawnInput` carries the Learn prompt, Methodus-managed runtime workspace, the Methodus run ID
(`session_id`), and an optional executor-side ID (`executor_session_id`) for a fresh
runtime session, followed by the maintainer-selected permission mode, allowed tools,
sandbox, and extra source directories. The two IDs are intentionally separate:
`learn_*` is only a Methodus record key and must never be passed to Claude's
`--session-id` or `--resume`; Claude receives a UUID and resumed turns use the
executor ID stored by the run. If an old Claude run contains a non-UUID executor ID,
the core engine starts a fresh runtime turn while preserving the same Learn run.
Methodus exposes one portable Learn permission selector and maps it conservatively to
native controls. In Plan mode, source changes remain prohibited by the Learn protocol,
while the runtime can request the maintainer's explicit approval for the single return
artifact under `runs/<learn-id>/`. Cautious mode keeps confirmations; Auto edit enables
the runtime's normal bounded auto-edit behavior. No launch path may emit a
bypass-permissions flag. The maintainer owns the choice, and the runtime still owns
individual approval prompts and enforcement.

Claude receives a UUID distinct from the Methodus `learn_*` record key; a later Learn
handoff can use that UUID to resume its native conversation. If an old stored Claude
ID is invalid, Methodus begins a fresh native conversation while retaining the same
Learn run and evidence record. Other runtimes may begin a fresh native conversation
with the same run context when no durable runtime ID is available. The workspace is
created under Methodus home at `workspaces/learn/<run-id>/`; the launch repository is
used only to resolve explicit `@` or registered source paths and is not the runtime's
working directory.

## 2. Learning protocol

Every native Learn handoff receives the same runtime-independent protocol:

1. read `METHODUS_LEARN.md` and its graph inventory;
2. restate the goal and scope;
3. inspect relevant committed or stale graph nodes;
4. identify assumptions and missing evidence;
5. ask the maintainer only consequential questions;
6. investigate attached sources and seek counterexamples;
7. separate fact, inference, contradiction, and unknown;
8. classify each candidate as new or an explicit revision/merge/revalidation/
   supersession proposal, never a canonical write.

The protocol is versioned with Methodus. At finalization, the runtime writes the
complete synthesis and fenced CandidateSet JSON to the exact per-turn artifact path
provided in the handoff brief. It is not installed as a general runtime Skill and is
not copied into ordinary task workspaces.

## 3. Connector Skill

Methodus ships one small connector for each runtime's Skill format. The connector is
the only Skill that Methodus installs or updates.

The connector contains no Team or Personal knowledge. It teaches the agent to:

- call `methodus agent manifest` before substantial diagnosis, design, research,
  document, or presentation work;
- treat the manifest as an environment/inventory contract, not a preselected answer;
- select relevant nodes using the user's question, then call `get --facet all` and
  `related` to inspect their bodies and authored neighbors;
- use `search` only as an explicit lexical fallback, never as the complete context;
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
- If CLI output is invalid or a requested node cannot be read, the connector treats
  Methodus as unavailable and must not invent Methodus claims.
- Connector compatibility is checked with golden invocation tests, not assumed from
  prose alone.
