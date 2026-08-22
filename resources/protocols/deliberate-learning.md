# Deliberate learning protocol

This protocol is injected into the focused Learn runtime prompt. It is not a
runtime Skill, is never installed in Claude Code or Codex, and does not create a
task workspace.

## Working contract

- Use only the source roots and files explicitly visible to the read-only runtime.
- Clarify the goal, scope, version, environment, and what is out of scope.
- Form 3–7 falsifiable questions and record competing explanations.
- Prefer specifications, official documentation, source code, logs, tests, and
  reproducible experiments. Label secondary material as a lead.
- Keep evidence, inference, uncertainty, and conflicts separate.
- Ask the user only questions that materially change the investigation.
- Before ending, return a structured CandidateSet in the assistant response. Do not
  write a return file or claim that Methodus has committed anything.

## Existing knowledge and integration

Before investigating external sources, read the Methodus-managed
`METHODUS_LEARN.md` snapshot in the runtime workspace. Search its inventory,
then read the complete Markdown body of every relevant committed or stale
Knowledge, Method, or Experience node from the listed graph directories.
Compare new evidence with those nodes before proposing a result. The runtime,
not Methodus's Rust layer, decides whether a conclusion is new, a revision,
duplicate, stale rule, or contradiction.

Every candidate must declare `disposition` as `new`, `revise`, `merge`,
`revalidate`, or `supersede`. Non-`new` candidates must include the exact
canonical `target` node ID and a facet-level `patch`. The runtime must never
edit graph files; these are proposals for human Review only.

When candidates are ready, include this machine-readable block (the surrounding
explanation may remain human-readable). The block is a proposal, never a commit:

```json
{"graph_review":{"searched":true,"relevant_nodes":[{"id":"knowledge/existing-node","reason":"same operational boundary"}],"no_match_reason":null},"candidates":[{"type":"knowledge","kind":"procedure","title":"...","summary":"...","disposition":"revise","target":"knowledge/existing-node","patch":"Update the Execute facet to ...","learn":"...","decide":"...","execute":"...","evidence":"...","outcome":"...","occurred_at":"...","tags":["..."]}],"relations":[{"from":"candidate-0","relation":"derived_from","to":"knowledge/existing-node"}],"unresolved_questions":[],"contradictions":[],"runtime_skills":[{"name":"...","runtime":"claude-code","outcome":"useful","reason":"..."}]}
```

## Synthesis contract

Classify the result as concept, procedure, system-flow, decision, or diagnosis.
Write zero or more atomic candidates. When 5W2H clarifies the topic, express
`what/why/who/when/where/how/how_much` naturally inside the candidate's `learn` text;
the machine contract only requires the `learn`, `decide`, `execute`, and `evidence`
facet strings. Split broad goals into linked atoms instead of forcing one giant note.
If evidence is insufficient, return no candidates and list the unresolved questions.
Relation endpoints may refer to `candidate-<index>`, an exact candidate title, or a
canonical node ID. Never invent a target merely to make the graph connected.

## Runtime skill observations

If the runtime used or recommended any external Skills, record them in the structured
CandidateSet response under an `experience` candidate's evidence. This is an
observation only; Methodus
will not install, copy, or manage those Skills. Include the runtime, skill name,
whether it helped, and a short reason so future work can receive a cautious hint.

Example:

```yaml
runtime_skills:
  - name: example-skill
    runtime: claude-code
    outcome: useful
    reason: helped inspect the protocol sources
```
