# 01 — Runtime Adapters

> **Status: verified.** Every CLI flag, JSON event shape, and behavior in this
> document was confirmed by running the real tools on 2026-08-14
> (Claude Code 2.1.220, Codex 0.146.1, Cursor Agent 3.14.7) on macOS.
> Do not "correct" these from memory — re-run the probes if a tool version changes.

## 1. The central finding

The original spec assumed Methodus might have to allocate a **PTY** and screen-scrape an
interactive CLI (parsing ANSI to detect prompts). **This is not necessary.**

All three executors expose a **non-interactive, structured-output mode** with:

- a machine-readable event stream (JSON or JSONL),
- **session persistence + resume** (multi-turn continuation across process
  invocations), and
- some form of **permission control**.

Therefore Methodus can drive executors as **SDK-style subprocesses / protocol clients**
when a task explicitly needs managed execution. It must not drive an interactive CLI
as a terminal puppet: parsing ANSI or guessing whether Claude/Codex is waiting for a
human is still out of scope.

The default product path is now **native handoff**: Methodus compiles a Task Workspace,
launches the user's normal Claude Code/Codex/Cursor TUI with a concise brief, and
steps out of the conversational path. Structured adapters remain valuable for optional
headless/managed tasks, health checks, and future automation; they are not required to
make the graph and learning loops useful.

## 2. Two launch modes

| Mode | Methodus does | Agent runtime does | Use when |
|---|---|---|---|
| **Native handoff (default)** | compile capsule, start terminal/TUI, record launch and return, collect outcome | owns all interactive conversation, tools, approval UI, and session persistence | normal daily coding or learning work |
| **Managed execution (optional)** | spawn structured process, consume JSON events, apply Methodus policy, persist session | executes non-interactive or protocol-driven turns | automation, replayable batch work, or a deliberately managed flow |

Both modes consume the same `manifest.yaml`, `brief.md`, selected context, skills,
and outcome format. The difference is only the transport and degree of control.

Native handoff must not modify a repository's permanent `CLAUDE.md`, `AGENTS.md`, or
global runtime configuration. The launch brief points to the generated capsule, while
the runtime's working directory remains the user-selected project.

## 3. Capability matrix

| Capability | Claude Code | Codex CLI | Cursor Agent |
|------------|:-----------:|:---------:|:------------:|
| Non-interactive execution | ✅ `--print` | ✅ `exec` | ✅ `agent --print` |
| Structured event stream | ✅ `--output-format stream-json` (+`--verbose`) | ✅ `--json` (JSONL) | ✅ `--output-format stream-json` |
| Session persistence | ✅ `--session-id <uuid>` | ✅ auto (`thread_id`) | ✅ auto (`session_id`) |
| Session resume / multi-turn | ✅ `--resume <id>` | ✅ `exec resume <id>` | ✅ `--resume <id>` |
| Real-time approval (structured) | ✅ `permission_denials[]` + `--allowed-tools` | ✅ `app-server` JSON-RPC `requestApproval` | ⚠️ coarse only |
| Fine-grained tool permission | ✅ `--allowed-tools "Write" "Bash(git *)"` | ⚠️ sandbox 3-level | ❌ `--force` all-or-nothing |
| Tool calls visible in stream | ✅ `tool_use` blocks | ✅ `command_execution` items | ✅ `tool_call` events |
| Mid-turn interrupt | ⚠️ signal/kill | ✅ `turn/interrupt` (app-server) | ❌ |
| Persistent background daemon | ✅ `--bg` + `claude agents` | ✅ `codex app-server daemon` | ❌ |
| Cost / token tracking | ✅ `total_cost_usd` | ✅ `usage` tokens | ✅ `usage` tokens |
| Streaming multi-turn input | ✅ `--input-format stream-json` | ❌ (stdin one-shot) | ❌ |

**Ranking for Methodus:** Claude Code (richest control) ≥ Codex (via app-server,
near-parity + best interrupt/approval granularity) > Cursor (usable baseline, coarse
permissions, no daemon). **Default runtime is Claude Code**; Cursor and Codex
remain selectable. The matrix above is capability, not a ranking of daily use.

## 4. The adapter boundary

Core logic depends only on an adapter boundary. Executor-specific quirks live behind
it. A production API has two entry points: `handoff` for the default native terminal
launch, and `spawn`/`resume` for optional structured managed execution.

```rust
/// Identifies a concrete executor implementation.
pub enum RuntimeKind { ClaudeCode, Codex, Cursor }

pub enum LaunchMode { NativeHandoff, Managed }

pub struct HandoffInput {
    pub runtime: RuntimeKind,
    pub launch_cwd: PathBuf,       // user project root
    pub capsule_path: PathBuf,     // immutable Methodus task package
    pub brief: String,             // bounded startup context + capsule reference
    pub terminal: TerminalTarget,  // current terminal, tmux pane, or configured terminal
}

/// Minimum common contract — all three executors satisfy this.
#[async_trait]
pub trait RuntimeAdapter: Send + Sync {
    fn kind(&self) -> RuntimeKind;

    /// Probe availability + version (e.g. `claude --version`).
    async fn detect(&self) -> Result<RuntimeAvailability>;

    /// Static capability declaration (drives resolver + policy decisions).
    fn capabilities(&self) -> RuntimeCapabilities;

    /// Launch the runtime's ordinary interactive UI. No conversation/event parsing.
    async fn handoff(&self, input: HandoffInput) -> Result<NativeLaunch>;

    /// Start a new session for a task. Returns a handle + an event stream.
    async fn spawn(&self, input: SpawnInput) -> Result<(SessionHandle, EventStream)>;

    /// Continue an existing session with a new user turn.
    async fn resume(&self, session: &SessionHandle, prompt: &str)
        -> Result<EventStream>;

    /// Stop / cancel a session (graceful, then forced if `force`).
    async fn stop(&self, session: &SessionHandle, force: bool) -> Result<()>;
}

/// Optional real-time control. Implemented by Claude Code and Codex (app-server).
/// Cursor returns `Unsupported`.
#[async_trait]
pub trait InteractiveRuntime: RuntimeAdapter {
    /// Respond to a pending approval request surfaced in the event stream.
    async fn resolve_approval(&self, session: &SessionHandle, id: ApprovalId,
        decision: ApprovalDecision) -> Result<()>;

    /// Interrupt the current turn without killing the session.
    async fn interrupt(&self, session: &SessionHandle) -> Result<()>;

    /// Send an additional turn without re-spawning the process.
    async fn send_turn(&self, session: &SessionHandle, prompt: &str) -> Result<()>;
}
```

Supporting types (normalized across executors):

```rust
pub struct SpawnInput {
    pub prompt: String,
    pub cwd: PathBuf,                 // user project root; capsule is referenced separately
    pub capsule_path: PathBuf,         // immutable Task Workspace
    pub session_id: Option<Uuid>,    // caller-assigned when supported
    pub permission: PermissionMode,   // mapped per-adapter (see §8)
    pub allowed_tools: Vec<String>,   // Claude Code honors precisely
    pub extra_dirs: Vec<PathBuf>,     // always the launch cwd ∪ registered projects (`claude --add-dir`); source is read in place, not copied into cwd
    pub model: Option<String>,
}

pub struct SessionHandle {
    pub kind: RuntimeKind,
    pub session_id: String,           // uuid (Claude) / thread_id (Codex) / session_id (Cursor)
    pub transport: Transport,         // Subprocess { pid } | AppServer { conn_id } | Background { agent_id }
}

/// Normalized event — every adapter maps its native stream into this enum.
pub enum RuntimeEvent {
    SessionStarted { session_id: String },
    AssistantText  { text: String },
    Thinking       { text: String },
    ToolCallStarted   { id: String, name: String, input: serde_json::Value },
    ToolCallCompleted { id: String, output: serde_json::Value, exit_code: Option<i32> },
    ApprovalRequested { id: ApprovalId, kind: ApprovalKind, detail: serde_json::Value },
    TurnCompleted  { stop_reason: Option<String> },
    Result { is_error: bool, text: String, cost_usd: Option<f64>, usage: Usage },
    Error  { message: String },
}

pub enum ApprovalDecision { Accept, AcceptForSession, Decline, Cancel }
```

For managed execution, each adapter's job is: **spawn the right process/protocol, and
translate its native event stream into `RuntimeEvent`.** For native handoff it only
constructs the safe launch command and records return status. Policy, graph resolution,
capsule compilation, persistence, and review stay in the core.

---

## 5. Claude Code adapter

**Binary:** `claude` (2.1.220). Auth via existing login/keychain.

### 5.1 Native handoff (default)

The launcher starts `claude` in the project `cwd` and passes a short task brief that
points to the capsule's `brief.md`, `context.md`, and `references.md`. Methodus yields
the terminal (or opens a configured tmux pane), waits only for process return, and
then restores its graph/review TUI. It does not use `claude attach` or parse TUI output
to supervise a normal interactive session.

### 5.2 Non-interactive execution + event stream

```bash
claude --print --output-format stream-json --verbose \
       --session-id <uuid> \
       --permission-mode <mode> \
       [--allowed-tools "Write" "Bash(cat *)"] \
       [--add-dir <dir> ...] \
       "<prompt>"
```

- `--output-format stream-json` **requires `--verbose`** (hard error otherwise).
- Emits newline-delimited JSON objects. Key shapes (verified):

```jsonc
// session init
{"type":"system","subtype":"init","session_id":"...","cwd":"...","model":"...",
 "tools":[...],"mcp_servers":[...],"permissionMode":"default"}

// assistant thinking / text (streamed)
{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"..."}]}}
{"type":"assistant","message":{"content":[{"type":"text","text":"Four"}]}}

// tool use appears as a content block of an assistant message
{"type":"assistant","message":{"content":[
   {"type":"tool_use","name":"Write","input":{"file_path":"...","content":"..."}}]}}

// terminal result (one per turn)
{"type":"result","subtype":"success","is_error":false,"result":"...",
 "total_cost_usd":0.13,"permission_denials":[...],"session_id":"...",
 "terminal_reason":"completed","usage":{...}}
```

### 5.3 Session persistence + resume (verified)

- Pass a caller-generated UUID via `--session-id`. Persisted unless
  `--no-session-persistence`.
- Resume with `claude --print ... --resume <session-id> "<next prompt>"`.
  Confirmed: context is fully retained across separate process invocations
  (a secret stored in turn 1 was recalled after resume in turn 2).

### 5.4 Permission model (verified)

`--permission-mode` values: `acceptEdits`, `auto`, `bypassPermissions`, `manual`,
`dontAsk`, `plan`.

- **`manual`** — every tool call is blocked; the `result` event returns a
  structured `permission_denials[]` array containing `tool_name`, `tool_use_id`, and
  the full `tool_input`. This is the programmable approval hook.
- **`plan`** — the agent produces a plan file and does **not** execute (planning only).
- **`--allowed-tools`** — precise allow-list, e.g. `"Write"`, `"Bash(git *)"`.
  Verified: with `--allowed-tools "Write"` the blocked write succeeds and the file is
  created. This is how Methodus *grants* a previously-denied action.

**Approval flow in `--print` mode:** a turn runs to completion; denied tools appear in
`permission_denials`. Methodus policy inspects them, and if approved, issues a
**resume turn** with the widened `--allowed-tools`. (For truly interactive,
mid-turn approval, use the background/`--input-format stream-json` path — see §5.5.)

### 5.5 Persistent background daemon (verified)

Claude Code ships its **own** background agent manager:

```bash
claude --bg --name "<name>" "<prompt>"     # → prints an agent id, returns immediately
claude agents --json                       # → [{pid,id,sessionId,name,status,state,cwd,...}]
claude attach <id>                         # reattach in a terminal
claude logs <id>                           # recent output (raw ANSI; not for parsing)
claude stop <id>                           # stop the session
```

- `agents --json` gives structured status (`status: busy`, `state: working`) without
  a TTY — usable for polling.
- `logs` output is **full-screen ANSI TUI** — do **not** parse it. Use `--print`/
  `stream-json` for machine-readable output; use `--bg` only for lifecycle.

### 5.6 Streaming multi-turn input

`--input-format stream-json` accepts realtime streamed user messages on **stdin kept
open**. Closing stdin ends the session. This is the path for true interactive
takeover (mid-turn approval, steering) without process restarts.

### 5.7 Integration strategy

- **Short tasks:** `--print --output-format stream-json --verbose` one-shot; parse the
  JSONL; capture `result`.
- **Multi-turn / interactive:** keep a process with `--input-format stream-json` +
  `--output-format stream-json`, stdin held open; stream turns in, events out.
- **Detached background:** `--bg` + poll `agents --json`; resume with `--resume` when
  Methodus needs to inject a turn. (Reconcile: `--bg` and `--print` are different
  surfaces — prefer the `--print`/stdin-stream path for programmatic control, and
  treat `--bg` as a fallback for long unattended runs. See Open questions.)

---

## 6. Codex adapter

**Binary:** `codex` (0.146.1).

### 6.1 Native handoff (default)

Launch the user's normal `codex` TUI in the project directory with the bounded
capsule brief. The full context is available by path; Methodus does not interpose on
the conversation. `codex exec` below is a managed-execution path, not the daily
interactive default.

### 6.2 Simple managed path — `exec` + `exec resume` (verified)

```bash
codex exec --json [--sandbox <mode>] [-C <cwd>] [--ephemeral] "<prompt>"
codex exec resume --json <thread_id> "<next prompt>"
```

Clean JSONL event stream (verified):

```jsonc
{"type":"thread.started","thread_id":"019f..."}
{"type":"turn.started"}
{"type":"item.started","item":{"type":"command_execution","command":"...","status":"in_progress"}}
{"type":"item.completed","item":{"type":"command_execution","command":"...","aggregated_output":"...","exit_code":0,"status":"completed"}}
{"type":"item.completed","item":{"type":"agent_message","text":"..."}}
{"type":"turn.completed","usage":{"input_tokens":...,"output_tokens":...}}
```

- `--sandbox`: `read-only` | `workspace-write` | `danger-full-access`.
  Verified: `read-only` blocks writes at the FS layer (agent reports it can't write);
  `workspace-write` allows writes within the workspace roots.
- `codex exec resume <thread_id>` restores context across invocations (verified with
  the secret-word round-trip).
- **Limitation:** `codex exec` has **no** `--ask-for-approval`; permission is only the
  coarse sandbox level, and each resume is a fresh process (~100ms startup).

### 6.3 Full managed path — `app-server` JSON-RPC (verified protocol)

Codex exposes a complete app-server protocol (stdio / `unix://` / `ws://`):

```bash
codex app-server --listen unix://<path>        # or default stdio
codex app-server daemon start                   # managed local daemon
codex app-server generate-json-schema --out <dir>   # dump the full protocol schema
```

The schema (dumped and inspected) defines a real client/server JSON-RPC protocol.
Relevant **client → server** requests include:

```
initialize, thread/start, thread/resume, thread/fork,
turn/start, turn/steer, turn/interrupt,
thread/inject_items, thread/rollback, thread/compact/start,
skills/list, review/start, model/list, ...
```

Relevant **server → client** requests (the approval hooks):

```
item/commandExecution/requestApproval    → CommandExecutionRequestApprovalParams
item/fileChange/requestApproval          → FileChangeRequestApprovalParams
tool/requestUserInput                    → ToolRequestUserInputParams
```

Approval response (`CommandExecutionApprovalDecision`, verified in schema):

```
"accept" | "acceptForSession"
| {"acceptWithExecpolicyAmendment": {...}}
| {"applyNetworkPolicyAmendment": {...}}
| "decline" | "cancel"
```

Server → client **notifications** (the event stream) include:
`thread/started`, `turn/started`, `turn/completed`, `item/started`, `item/completed`,
`item/commandExecution/outputDelta`, `item/agentMessage/delta`,
`turn/diff/updated`, `turn/plan/updated`, `thread/tokenUsage/updated`,
`item/fileChange/patchUpdated`, etc.

**This gives Codex feature-parity with (or better than) Claude Code:** real-time,
structured, per-command approval; `turn/interrupt` mid-turn; a persistent daemon
connection with no per-turn process restart.

### 6.4 Integration strategy

- **Phase 1 (simple):** `codex exec --json` + `codex exec resume` — trivial to
  implement, matches the common `spawn`/`resume` contract.
- **Phase 2+ (full):** connect to `codex app-server` over a Unix socket, speak
  JSON-RPC, implement `InteractiveRuntime` (approval + interrupt + steer). Generate
  Rust types from `generate-json-schema` output.
- **Caveat:** app-server is marked *experimental*; pin the Codex version and
  regenerate the schema on upgrade.

---

## 7. Cursor adapter

**Binary:** `cursor agent` (Cursor 3.14.7).

### 7.1 Native handoff and managed execution

```bash
cursor agent --print --output-format stream-json \
       [--workspace <path>] [--resume <id>] [--force|--plan|--auto-review] \
       "<prompt>"
```

Event stream (verified):

```jsonc
{"type":"system","subtype":"init","session_id":"...","model":"Auto","permissionMode":"default"}
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"..."}]}}
{"type":"thinking","subtype":"delta","text":"..."}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"..."}]}}
{"type":"tool_call","subtype":"started","tool_call":{"shellToolCall":{"args":{"command":"...","skipApproval":false,...}}}}
{"type":"tool_call","subtype":"completed","tool_call":{"shellToolCall":{...,"result":{"success":{...}}}}}
{"type":"result","subtype":"success","is_error":false,"result":"...","session_id":"...","usage":{...}}
```

- `--resume <id>` restores context (verified: 2+2 then 3+3 same session).
- Tool calls are visible (`shellToolCall`, `readToolCall`, etc.) with parsed args.
  The stream even carries a `skipApproval` flag, but in `--print` mode approvals are
  effectively bypassed — there is **no structured denial + re-grant loop** like
  Claude Code's.

### 7.2 Permission model (coarse)

- Default `--print`: writes/shell run without prompting.
- `--plan` / `--mode plan`: read-only planning, no edits.
- `--mode ask`: read-only Q&A.
- `--sandbox enabled|disabled`: OS sandbox toggle.
- `--force` / `--yolo`: allow everything.
- `--auto-review`: server-side classifier auto-runs "safe" calls, prompts for the
  rest — but there is no CLI-level programmatic approval callback exposed.

**Conclusion:** Cursor supports only the **base `RuntimeAdapter`** contract, not
`InteractiveRuntime`. Methodus policy for Cursor must be enforced *before* dispatch
(choose `--plan` vs full run, restrict workspace, use OS sandbox), not via mid-turn
approval. No background daemon; long runs stay attached to the process.

### 7.3 Integration strategy

- Implement base `spawn` / `resume` / `stop` via `cursor agent --print
  --output-format stream-json`.
- Map policy to pre-dispatch mode selection (`--plan` for read-only tasks,
  `--sandbox enabled`, bounded `--workspace`).
- Mark `InteractiveRuntime` methods as `Unsupported`.

---

## 8. Permission mode mapping

Methodus has a single internal `PermissionMode`; each adapter maps it:

| Methodus intent | Claude Code | Codex (exec) | Codex (app-server) | Cursor |
|-----------------|-------------|--------------|--------------------|--------|
| Read-only / analyze | `plan` or `manual`+no grants | `--sandbox read-only` | `read-only` policy | `--plan` |
| Guarded (approve each side effect) | `manual` + resume-grant loop | ⚠️ not available | `requestApproval` callback | ⚠️ not available |
| Auto-accept edits, guard commands | `acceptEdits` | `--sandbox workspace-write` | policy profile | `--auto-review` (approx) |
| Trusted / full | `bypassPermissions` | `danger-full-access` | full profile | `--force` |

Where a cell is ⚠️, the executor cannot honor guarded approval; Methodus must either
downgrade to read-only, pre-authorize a bounded scope, or route the task to an
executor that supports guarding (Claude Code / Codex app-server).

## 9. Verification probes (re-run on version bump)

Keep these as throwaway checks; they are how every claim above was established.

```bash
# Claude Code
claude --print --output-format stream-json --verbose --session-id <uuid> "..."
claude --print --output-format stream-json --verbose --resume <uuid> "..."
claude --print --output-format stream-json --verbose --permission-mode manual "<write attempt>"   # → permission_denials[]
claude --print --output-format stream-json --verbose --allowed-tools "Write" "<write>"             # → succeeds
claude --bg --name t "..." ; claude agents --json ; claude stop <id>

# Codex
codex exec --json --sandbox read-only "..."           # write blocked
codex exec --json --sandbox workspace-write "..."      # write allowed
codex exec resume --json <thread_id> "..."             # context retained
codex app-server generate-json-schema --out /tmp/codex-schema   # protocol

# Cursor
cursor agent --print --output-format stream-json "..."
cursor agent --print --output-format stream-json --resume <id> "..."
cursor agent --print --output-format stream-json --plan "<write attempt>"   # planning only
```

## 10. Open questions

1. **Native terminal transfer:** settle on terminal suspension versus a new tmux pane
   as the default handoff target. It must return to Methodus reliably without parsing
   the agent TUI.
2. **Managed Claude surface:** settle on `--print` + `--input-format stream-json`
   versus `--bg`/`agents` only for optional managed/unattended runs.
3. **Codex app-server stability:** it is experimental. Decide whether Phase 1 ships
   only the `exec`/`exec resume` path and defers app-server to Phase 2.
4. **Cursor guarded approval:** confirm there is truly no programmatic approval
   callback in a headless mode (only `--print`/`--plan`/`--force`/`--auto-review`
   were found). If confirmed, Cursor stays base-contract only.
5. **Session id ownership:** Claude Code accepts a caller UUID (`--session-id`);
   Codex and Cursor mint their own. The core must store the executor-issued id from
   the `SessionStarted` event and not assume it can pre-assign for all executors.
