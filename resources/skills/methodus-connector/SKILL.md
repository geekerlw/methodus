---
name: methodus
version: 1
x-methodus-managed: true
description: Use the local Methodus engineering memory for substantial diagnosis, design, research, document, and presentation work. Retrieve reviewed Methods, Knowledge, and Experience with the read-only methodus agent CLI; skip it for trivial mechanical edits.
---

# Methodus connector

Methodus is the team's reviewed engineering memory. It does not replace this runtime
and it does not contain the project source itself.

For substantial work involving diagnosis, incident investigation, design decisions,
code behavior/history, competitive research, formal documents, or presentations:

1. Call `methodus agent prepare --goal "<the user's current goal>" --budget 1200`.
2. Use the returned Method and Execute/Decide facets as working context.
3. Call `methodus agent search`, `get`, or `related` only when the returned lazy IDs
   or evidence require more detail.
4. Treat `stale` items as historical hypotheses. Revalidate them against the current
   repository before presenting them as current rules.
5. Cite Methodus node IDs and source references when they materially affect the answer.

Do not call this connector for trivial questions, formatting, or small mechanical edits
unless the user explicitly asks to use Methodus. If the CLI is unavailable, continue
the task normally and say that Methodus context was unavailable only when relevant.

This connector is read-only. Do not create, edit, promote, or publish Methodus content
from an Agent session. Maintainers use the Methodus TUI Learn and Review surfaces for
all writes.
