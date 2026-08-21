# 04 — Development roadmap

This roadmap is the implementation order for the product contract. The active product
boundary is deliberately narrow: Methodus is a maintainer knowledge studio and a
read-only Agent sidecar. Ordinary coding sessions, task workspaces, MCP, Face,
arbitrary Skill management, and automatic Skill evolution are not roadmap items.
Focused Learn retains native runtime handoff because its investigation is inherently
multi-turn and runtime-owned.

## Current baseline (implemented)

The repository already has a usable vertical slice:

- no-subcommand launch opens the maintainer Learn TUI;
- Markdown graph sync covers legacy `graph/`, Personal, and `teams/*` roots;
- graph links, duplicate IDs, broken links, source fingerprints, and stale status are
  indexed/validated;
- Learn uses a selected runtime in an explicit permission mode (read-only by default)
  and asks it for a structured CandidateSet;
- runtime output is stored as a Learn run plus review-only candidate Markdown files;
- Review can inspect, commit Personal, reject, mark Team visibility, and merge a
  Knowledge candidate into an explicitly selected committed Knowledge target;
- the Team panel reports the selected Team Git state, validation issues, and diff, and
  writes a non-mutating publish plan;
- Learn runs persist state, events, executor IDs, unresolved questions, contradictions,
  and review-only CandidateSet drafts so a TUI restart can resume a run;
- Review supports edit, deprecate, stale revalidation, and a review audit trail;
- `methodus agent prepare/search/get/related/status` is read-only and bounded;
- `methodus setup` installs the one official connector Skill; `doctor` reports local
  health.

The baseline is intentionally not described as feature-complete. Missing behavior is
listed below so future work does not accidentally revive retired architecture.

## M0 — Boundary and migration

**Status: complete for the active path.**

- Keep the maintainer TUI as the only normal write surface.
- Keep Agent CLI queries read-only and migration-compatible.
- Keep old SQLite/domain tables readable for compatibility, but do not route new
  product behavior through task/session/workspace/Face/evolution modules.
- Keep Markdown/Git canonical and SQLite rebuildable.

## M1 — Canonical graph and validation

**Status: complete for the first product contract.**

Completed: Knowledge/Method/Experience Markdown parsing, typed links, Personal/Team
roots, lifecycle filtering, duplicate/broken-link validation, source fingerprint stale
marking, required frontmatter checks, orphan/unknown-relation health warnings, one-hop
graph navigation, and direct Markdown re-indexing. Legacy rows remain readable only for
compatibility; new behavior uses the Markdown graph.

## M2 — Read-only Agent protocol

**Status: complete for protocol v1; evaluation remains.**

Completed: `prepare`, `search`, `get`, `related`, and `status`; Markdown/JSON output;
type/kind/scope filters; token and item bounds; candidate/rejected exclusion; stale
warnings; read-only SQLite opening.

Remaining evaluation work is deliberately product validation rather than a new
architecture: add golden fixtures for diagnosis, design decision, change history,
research, document, and presentation goals; measure deterministic scoring and index
revision; and verify connector fallback when the home/index is unavailable.

## M3 — Official connector Skill

**Status: complete for the local connector lifecycle.**

Completed: one runtime-neutral connector Skill and `methodus setup` targets for Claude
Code, Codex, and Cursor. The connector calls the local CLI and contains no graph data.

Completed: ownership/version marker, drift-aware `doctor`, explicit uninstall, and
refusal to overwrite unrelated Skills. Remaining validation is runtime-specific
trigger/skip testing. Runtime permission enforcement remains inside the selected
runtime.

## M4 — Deliberate Learn

**Status: focused interaction, durable runs, and candidate generation complete; evidence
UX remains.**

Completed: runtime picker/direct selection, explicit Learn permission modes, protocol injection,
scope challenge instructions, structured CandidateSet extraction including typed
relations/unknowns/contradictions, durable state/events, executor IDs, run/candidate
writes, and TUI continuation after restart when the runtime can resume.

Completed: source manifests/fingerprints for `@` sources, extra read-only runtime
directories, durable event replay on TUI restart, and failure-preserving run state.
Remaining: surface fact/inference/contradiction/unknown claims in a dedicated Learn
view; support controlled Git/document/URL/log source adapters beyond local `@` paths;
and make partial-run retry/switch-runtime behavior explicit in the UI.

## M5 — Review and graph editing

**Status: core approve/reject/merge/edit/lifecycle path complete; richer draft UX remains.**

Completed: candidate detail, Personal commit, reject, explicit Personal → Team move,
explicit Knowledge merge target, candidate editing, CandidateSet relation
materialization including unresolved endpoints, one-hop graph inspection,
deprecate/revalidate actions with rationale, local review audit trail, and visible
two-step confirmation for destructive actions. Remaining: split/merge draft editing
UX and a dedicated relation editor.

## M6 — Team repository and publish

**Status: selected-Team status/diff/plan complete; repository management remains.**

Completed: selected Team status, branch/dirty/changed-file display, Team graph
validation, bounded Markdown diff, and local `publish-plan.md` generation. Methodus
does not commit, push, merge, or discard Git changes.

Next:

- configure multiple Team IDs and local repository paths beyond the local Team roots;
- detect conflicts and show actionable resolution guidance;
- validate Team-only evidence and visibility rules at publish time;
- show Personal → Team file moves as an explicit staged plan;
- add optional, user-confirmed commit only if it remains compatible with normal Git
  review and never pushes automatically.

## M7 — Retrieval and maintainer experience

**Status: planned.**

- evaluate retrieval precision, stale warnings, token bounds, and fallback behavior
  on a real engineering-query corpus;
- improve large-graph navigation with bounded neighborhood queries and lazy details;
- add accessibility, CJK/IME, filtering, and error-state regression fixtures;
- add performance budgets for graph sync and Agent CLI startup;
- only consider embeddings after deterministic retrieval has been measured and shown
  insufficient.

## Non-goals / permanent constraints

- no MCP server in the default architecture;
- no ordinary task workspace or repository copy managed by Methodus;
- no runtime handoff or proxying of ordinary coding sessions; focused Learn hands the
  terminal to the selected native runtime and never proxies its UI;
- no monitoring of ordinary developer transcripts;
- no automatic graph writes from consumer Agents;
- no arbitrary Skill install, generated Skill, marketplace, or Skill evolution;
- no autonomous Git push, merge, or silent Team publication.
