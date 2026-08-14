# Legacy — Methodus v1 (Prompt Agent)

This directory preserves the **first generation** of Methodus for historical and
conceptual reference. It is **not** part of the current product.

## What v1 was

`methodus-v1-prompt-agent.md` was a single-file agent definition — a pure prompt
installed into Claude Code / Cursor agent directories. It had no runtime code,
no daemon, and no persistent state beyond a `~/.methodus/experience.json` file.
It planned a skill sequence for the host agent to execute, then recorded a
workflow pattern on completion.

## Why it was superseded

v1 encoded all logic inside a prompt: skill discovery, planning, replanning,
and experience recording were instructions to a model, not enforceable software.
This is exactly the anti-pattern the v2 design calls out
("Prompt is an interface, not a database"). v2 is a persistent Rust daemon that
owns state, policy, and orchestration in real code, and drives an external
executor (Claude Code / Codex / Cursor) as swappable "hands".

See `../design/00-product.md` for the v2 product contract and the rest of
`../design/` for the technical design.

## Reusable conceptual assets from v1

Two ideas from v1 carry forward into v2 and should be mined, not rewritten from
scratch:

1. **Skill discovery scan pattern** — the `~/.{platform}/plugins/*/skills/`,
   `~/.{platform}/skills/`, `~/.{platform}/commands/`, and project-local
   `.{platform}/skills/` scan logic is a proven starting point for the v2
   rule-based Skill scanner.
2. **`experience.json` `workflow_patterns` schema** — keyword-matched workflow
   records are a minimal precursor to v2's Experience/Knowledge model; the v2
   schema can import them.
