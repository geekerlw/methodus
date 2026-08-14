# 04 — Implementation Roadmap

Revised phased plan. This **reorders the original spec's phasing** based on three
conclusions from the design discussion:

1. **Walking skeleton first.** Do not build a full observability platform (the
   original spec's Phase 0: config + full SQLite schema + event bus + 30 event types +
   doctor + CLI) before proving any value. Build the thinnest end-to-end spine first,
   then thicken.
2. **Single always-on process — no daemon/client split.** Persistence comes from
   keeping the one `methodus` process open (in `tmux`), plus executor-native session
   resume for restart recovery. The daemon/client split is a deferred, optional
   refactor enabled by keeping `methodus-core` UI-free (see `02-architecture.md` §0).
3. **No PTY.** The spike showed all executors expose structured non-interactive modes
   with resume.

Guiding metric: **the fastest path to something the user runs daily on real work.**
Sustained personal use is the precondition for the learning loop to ever produce
value; a beautiful architecture nobody uses accumulates no knowledge.

---

## Spike (done — 2026-08-14)

Verified all three executors support non-interactive structured output + session
resume, and that no PTY is required. Findings recorded in
[`01-runtime-adapters.md`](./01-runtime-adapters.md). **This retires the biggest
technical risk in the original design.**

---

## Milestone M0 — Project skeleton & the one hard decision

**Goal:** a compiling Cargo workspace and the store engine decided.

- `cargo` workspace with the crates from `02-architecture.md` (empty but compiling):
  `methodus-domain`, `methodus-store`, `methodus-runtime`, `methodus-core` (library),
  and the single `methodus` binary.
- `methodus-domain`: core enums + `Task`/`Session` structs + state-transition
  functions (pure, unit-tested).
- Decide `sqlx` vs `rusqlite`; wire the first migration; `methodus init` creates
  `~/.methodus/` + `state.db`.
- CI: `cargo build`, `cargo test`, `cargo clippy`, `cargo fmt --check`.

**Acceptance:** `methodus init` on an empty machine creates the home dir + DB;
`cargo test` passes on the domain state machine; the workspace builds clean.

---

## Milestone M1 — Vertical walking skeleton (Claude Code, single process)

The single most important milestone. Proves the **brain → hands → memory** spine
end to end, with the least code, inside one process.

**Scope:**

- One `methodus` process runs the core `Engine` in-process (no socket, no daemon).
  A single-instance lock (`~/.methodus/methodus.lock`) prevents two drivers.
- `methodus task create "<goal>" [--face X]` → task row; rule resolver picks a single
  Face (hard-coded/seed is fine) and produces `selected-context.md`.
- Workspace Builder creates `~/.methodus/workspaces/<task-id>/` with path-safety
  checks.
- **Claude Code adapter** implementing base `RuntimeAdapter` via
  `claude --print --output-format stream-json --verbose --session-id ...`; normalize
  the JSONL into `RuntimeEvent`; persist events + transcript.
- `methodus run <task-id>` runs the session inside the process and streams
  `RuntimeEvent`s to the terminal live.
- On completion, persist result + a raw `Experience` record (file + index row).
- **Persistence check:** run the process in `tmux`; detach the terminal; the run
  continues; re-attach the `tmux` window and see it still going / completed.
- **Restart recovery (minimal):** capture and persist the Claude `session_id`; after a
  process kill+restart, reconcile via `claude agents --json` and offer `--resume`.

**Deliberately excluded:** learning loop, curiosity, multi-Face, Codex/Cursor,
rich TUI, fine-grained approval (use `plan`/`acceptEdits` for now).

**Acceptance (maps to `00-product.md` §12 scenarios A + D):** submit a task; Methodus drives Claude
Code in an isolated workspace; detach the `tmux` window and reattach later to see the
still-running/completed session; view transcript + result + the Experience record.
Kill and restart the `methodus` process mid-run and confirm it reconciles the session
(reattach or offer `--resume`).

---

## Milestone M2 — Persistence hardening, policy & approval, second executor

Thicken the spine into a dependable system and prove executor-agnosticism.

**Scope:**

- Full event log + append-only guarantees + idempotent handlers; `methodus events
  tail` (read-only, safe from a second terminal).
- Crash-recovery reconciliation per `02-architecture.md` §6 (Claude `agents --json`;
  `--resume`).
- **Policy engine + guarded approval loop** for Claude Code: `--permission-mode
  manual` → structured `permission_denials` → an in-process `approval.requested`
  event → user decides (`once|session|deny|abort`) → resume with widened
  `--allowed-tools`. (`00-product.md` §12 scenario B.)
- **Codex adapter** (base contract) via `codex exec --json` + `codex exec resume`,
  proving the `RuntimeAdapter` trait holds across executors.
- Resolver reads real seed Faces/Methods/Skills from `resources/` + `~/.methodus/`;
  records rationale + confidence; low-confidence choices surfaced to the user.
- Reuse v1 skill-discovery scan logic (see `../legacy/README.md`) for the Skill
  scanner.

**Acceptance:** a task requiring a risky action pauses in `waiting_user` with a clear
scope; approve → continues, deny → safe return, both logged. The same task type runs
on Codex by switching `--runtime`. Process restart during a session recovers state.

---

## Milestone M3 — First-class TUI

Make the daily-driver UI real so dogfooding is pleasant (this is what drives
knowledge accumulation). The TUI is part of the **same process** as the Engine.

**Scope:** `ratatui` TUI subscribing to the in-process event bus — Dashboard
(queue/tasks/pending approvals/pending questions), Tasks, Session (live transcript +
input + approve), Faces. Because UI and Engine share a process, "the TUI is the app";
keep it open in `tmux` to keep Methodus running.

**Acceptance:** the full task loop (create → resolve → run → approve → view result) is
completable from the TUI; detaching the `tmux` window keeps sessions running.

> **Dogfood gate:** from M3 on, use Methodus for real work (e.g. NXM/embedded tasks).
> This produces the Experience corpus that M4 depends on.

---

## Milestone M4 — Controlled learning & curiosity (the actual bet)

Only now, and only with a real Experience corpus from dogfooding.

**Scope (the Learning + Curiosity loops, `00-product.md` §4.2–4.3):** Learning Queue + scheduler (event/threshold/idle);
`extract_experience` → `detect_gaps` → `propose_knowledge`; Question state machine +
valuation; candidate Knowledge with conflict detection; TUI Review page. No unbounded
loops — every job is budgeted, retryable, cancelable, and recoverable. The scheduler
runs inside the always-on process; background work happens while the process is open.

**Acceptance (`00-product.md` §12 scenario C):** task completion enqueues learning jobs; committed
knowledge is never silently overwritten; a high-value repeated unknown surfaces as a
Question; answering it produces sourced candidate Knowledge.

> **Revisit the daemon/client split here.** If, and only if, "Methodus must do
> background work with no window open at all" becomes a hard requirement, evaluate
> promoting the process to a detached background service. `methodus-core` being
> UI-free makes this a wrapping change, not a rewrite. Until then, stay single-process.

---

## Milestone M5+ — Deferred (unchanged from spec)

Multi-Face composition & dynamic Methods; Evolution with human-approved
versioned upgrades; Codex **app-server** full `InteractiveRuntime`
(real-time approval + interrupt + steer); Cursor adapter; advanced collaboration,
research, desktop (Tauri) client, remote nodes.

The Codex app-server client and a future Tauri desktop client are enabled by the
architecture (structured event streams, UI-free core) but are **not** MVP. A desktop
client is also one of the triggers that would justify the optional daemon/client split.

---

## Definition of Done (per feature)

A feature is done only when it has: an explicit data model + state transitions;
success/failure/cancel/restart-recovery paths; events + logs; a policy boundary +
error handling; at least one unit test and one integration/e2e test where applicable;
structured executor output that is parsed and validated (never format-by-luck); no
mutation of the user's global executor config; and UI state decoupled from session
lifecycle (the UI observing the Engine, not owning sessions directly).

## Sequencing rationale (summary)

| Spec order | Revised order | Why |
|------------|---------------|-----|
| Phase 0: full platform first | M0 minimal + M1 vertical slice | prove value before breadth |
| Persistent daemon (`methodusd`) | Single always-on process (kept in tmux) | simpler; executor resume covers restart recovery; daemon split deferred |
| PTY adapter | No PTY; SDK/JSON adapters | spike showed structured modes exist |
| Learning in Phase 2 | Learning in M4, after dogfooding | needs a real corpus to be meaningful |
| One executor then expand | Claude Code (M1) → Codex (M2) | vertical depth before abstraction breadth |
