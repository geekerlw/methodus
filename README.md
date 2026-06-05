# methodus

A goal-directed planning agent for software development. Dynamically composes available skills into an execution plan, adapts on failure, and learns from past workflows.

Works standalone — no other tooling required.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/geekerlw/methodus/main/install.sh | bash
```

Installs `methodus.md` to the agents directory of any supported platform detected on your machine. Creates `~/.methodus/` for the experience store.

## Usage

In Claude Code or Cursor, describe any open-ended development goal:

```
@methodus refactor the auth module and add unit tests
```

methodus will:
1. **Clarify** — invoke a clarification skill if available, otherwise ask up to 3 focused questions if the goal is ambiguous
2. **Discover** — scan all installed skills across detected platforms
3. **Load experience** — surface past workflow hints from `~/.methodus/experience.json`
4. **Plan** — show a step-by-step skill sequence for your confirmation
5. **Execute** — run each step, replanning on failure (max 2 replans)
6. **Learn** — record the outcome to improve future plans

## How it works

- **Skill discovery** — for each supported platform, scans `~/.{platform}/plugins/*/skills/`, `~/.{platform}/skills/`, `~/.{platform}/commands/`, and project-local `.{platform}/skills/`. Works with any SKILL.md-compatible skill, regardless of origin.
- **Experience store** — `~/.methodus/experience.json` accumulates keyword-matched workflow patterns across all your projects.
- **Replanning** — on step failure, checks for known alternatives and replans remaining steps (max 2 replans per run).

## Supported platforms

| Platform | Agent directory |
|----------|----------------|
| Claude Code | `~/.claude/agents/methodus.md` |
| Cursor | `~/.cursor/agents/methodus.md` |
