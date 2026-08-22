---
name: methodus
version: 2
x-methodus-managed: true
description: Use the local Methodus engineering memory for substantial diagnosis, design, research, document, and presentation work. Inspect the reviewed graph through its read-only environment manifest; skip it for trivial mechanical edits.
---

# Methodus connector

Methodus is the team's reviewed engineering memory. It does not replace this runtime
and it does not contain the project source itself.

For substantial work involving diagnosis, incident investigation, design decisions,
code behavior/history, competitive research, formal documents, or presentations:

1. Call `methodus agent manifest --format json` (the `environment` alias is also
   supported). This returns the selected Team, Personal/Team directory structure, graph
   roots, revision, and complete consumer-visible inventory. It is an environment
   contract, not an answer or a preselected context bundle.
2. Use the user's question and the inventory's titles, summaries, tags, facets, and
   evidence references to decide which nodes are relevant. Methodus does not make that
   semantic selection for you.
3. Read the selected node bodies with
   `methodus agent get <node-id> --facet all --format markdown`. Use
   `methodus agent related <node-id>` to follow authored graph relationships. The
   absolute paths, directory structure, and graph roots in the manifest may be inspected
   directly when the runtime's read tools permit it.
4. Treat `stale` items as historical hypotheses. Revalidate them against the current
   repository before presenting them as current rules.
5. Cite Methodus node IDs and source references when they materially affect the answer.

Do not call this connector for trivial questions, formatting, or small mechanical edits
unless the user explicitly asks to use Methodus. If the CLI is unavailable, continue
the task normally and say that Methodus context was unavailable only when relevant.

This connector is read-only. Do not create, edit, promote, or publish Methodus content
from an Agent session. Maintainers use the Methodus TUI Learn and Review surfaces for
all writes. `search` remains available as an explicit lexical fallback, but must not
replace the manifest-first flow or be treated as a complete answer context.
