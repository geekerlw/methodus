# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repo is

methodus is a single-file agent definition (`methodus.md`) plus a shell installer (`install.sh`). There is no build step, no package manager, and no runtime code — the deliverables are plain text files.

## Architecture

- **`methodus.md`** — the agent definition. YAML frontmatter (name, model, description) followed by a Markdown system prompt. Installed to `~/.claude/agents/` and/or `~/.cursor/agents/` on the user's machine.
- **`install.sh`** — downloads `methodus.md` from the GitHub raw URL and copies it to the agents directory of any supported platform detected (`~/.claude/`, `~/.cursor/`). Also creates `~/.methodus/` for the experience store.

The agent itself operates in six phases at runtime: clarify → discover skills → load experience → plan → execute → update experience. The experience store lives at `~/.methodus/experience.json` on the user's machine (never in this repo).

## Agent file conventions

- Frontmatter fields: `name`, `model`, `description`. Keep `model: claude-sonnet-4-6` unless intentionally changing the target model.
- Skill discovery uses a platform-parameterized pattern — `~/.{platform}/plugins/*/skills/*/SKILL.md` etc. — so adding a new platform means adding one entry to the platform list in Phase 1, not new scan paths.
- The exclude list in Phase 1 (meta-skills) must be kept in sync if new meta-skills are added.

## Testing changes

There is no automated test suite. Verify changes by:

1. Running `bash install.sh` locally to confirm it copies `methodus.md` to the correct agent directory.
2. Invoking `@methodus <goal>` in Claude Code to exercise the agent end-to-end.
3. Checking that `~/.methodus/experience.json` is created/updated after a completed run.
