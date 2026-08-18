# Module expert learning

Use when the user runs **`/learn`** with explicit **paths or URLs** — not to learn the ephemeral task workspace.

## Goal

Read user-specified sources in place, synthesize durable **knowledge**, draft a **skill**, record **experience**, and flag **mentor questions** for unclear areas.

Outputs live under Methodus home (`~/.methodus/faces/…/knowledge`, `skills/`, `experiences/`).  
Future **execution tasks** load committed knowledge/skills into their workspace — this workspace is scratch only.

## Workflow

1. **Sources** — read only paths/URLs listed in the task (and `@` attachments). Do not glob the task workspace for corpus material.
2. **Read in place** — use Read/Glob/Bash on those paths; fetch URLs when network is allowed.
3. **Synthesize** — `## Knowledge` with `###` subtopics, citing sources.
4. **Skill angle** — capture reusable procedure (how to navigate this module next time).
5. **Mentor gaps** — `## Open Questions (for Mentor)` for anything only the human expert can confirm.

## Required final output shape

```markdown
## Sources

- `path/or/url` — what you learned from it
- ...

## Knowledge

### {Subtopic}

{Stable facts. Cite sources.}

## Skill

{Reusable procedure — how to navigate this module next time. Numbered steps preferred.}

## Open Questions (for Mentor)

- {Specific question for the human mentor}

## Notes

- {Out of scope, follow-up reads}
```

## Rules

- **Never** treat the task workspace as the module under study.
- Do not invent specs — mark uncertainty and ask the mentor.
- Prefer evidence from the listed sources only.
