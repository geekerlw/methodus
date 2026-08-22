# Existing knowledge and integration contract

The file `METHODUS_LEARN.md` is the Methodus graph snapshot for this Learn turn.
Read it before investigating external sources. It lists the consumer-visible
Knowledge, Method, and Experience nodes, their exact IDs, statuses, facets,
source references, and the read-only graph directories that contain the full
Markdown bodies.

The runtime owns this retrieval and reasoning step:

- Search the inventory for likely matches before proposing any new durable
  conclusion. Read the complete Markdown body of every relevant node; do not
  rely on its summary alone.
- Compare the new evidence with the relevant nodes claim by claim. Distinguish
  an actually new conclusion from a narrower revision, duplicate, stale rule,
  or contradiction.
- Treat `committed` nodes as current graph knowledge and label `stale` nodes as
  stale. Candidate, rejected, and deprecated files are not canonical evidence.
- Every candidate must declare an integration disposition: `new`, `revise`,
  `merge`, `revalidate`, or `supersede`.
- For `revise`, `merge`, `revalidate`, or `supersede`, set `target` to the exact
  existing node ID and write a facet-level `patch` describing the proposed
  change. Do not silently rewrite the target.
- For `new`, leave `target` and `patch` empty unless the patch is useful as an
  explanatory delta. Add a typed relation to an existing node when that
  relationship matters.
- If no relevant existing node is found, say what was searched and why it was
  not a match. Do not invent a target merely to make the graph connected.
- Never edit graph files. Return proposals only; Methodus sends them to human
  Review, where the maintainer decides whether to create, revise, merge,
  revalidate, supersede, or reject them.

The final CandidateSet must include `graph_review` with `searched: true` and a
short list of relevant node IDs (or `no_match_reason`). Each candidate must
include `disposition`, `target`, and `patch` fields.
