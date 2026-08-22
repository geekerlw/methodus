# 00 — Product contract

## 1. Positioning

Methodus is an **engineering knowledge and method studio for agent-assisted software
teams**.

A small number of maintainers use its TUI to investigate a topic, challenge
assumptions, verify sources, structure conclusions, and publish a reviewed Personal or
Team knowledge graph. Ordinary developers remain in Claude Code, Codex, or another
agent runtime. An official connector Skill calls the local Methodus CLI to retrieve
relevant Method, Knowledge, and Experience.

Methodus is not:

- a replacement coding agent or general agent chat client;
- an MCP server;
- a task workspace or repository-copy manager;
- a generic index over every file in a company;
- an automatic Skill generator or Skill marketplace;
- an autonomous system that rewrites canonical knowledge after every session.

The product deliberately separates two moments of work. Maintainers use Methodus to
use, learn, curate, and publish durable engineering memory; developers use Claude
Code, Codex, Cursor, or another native runtime for the actual task. Ordinary TUI input
is a Use question against the reviewed graph. Both Use and Learn start native runtimes
from Methodus-managed workspaces; Learn state and return artifacts remain under
`runs/<learn-id>/`. The selected runtime owns the multi-turn conversation, and Methodus
returns only after that runtime exits.

## 2. Users and value

### 2.1 Knowledge maintainer

Usually one to three people. A maintainer:

- conducts interactive Learn sessions in the TUI;
- provides code, Git history, docs, logs, incidents, or prior conclusions as sources;
- reviews proposed Knowledge, Method, Experience, and graph relations;
- promotes Personal content into Team;
- detects stale, conflicting, duplicate, or broken content;
- prepares a validated, reviewable Team publish plan that can be committed through
  normal Git tooling.

Methodus must let one maintainer maintain substantially more trustworthy engineering
memory than a hand-written wiki.

### 2.2 Knowledge consumer

An ordinary developer does not need to open Methodus. They:

- install the Methodus CLI and official connector Skill;
- continue working in their preferred native agent runtime;
- receive reviewed engineering context through CLI calls made by the Skill;
- choose runtime permissions themselves; Methodus never edits permission policy.

The consumer value is better diagnosis and design reasoning without repeatedly asking
an expert for hidden history.

## 3. Core assets

### 3.1 Knowledge

A reusable, reviewed engineering conclusion: a concept, signal meaning, design
decision, constraint, diagnostic rule, or change explanation.

Knowledge commonly uses these facets:

| Facet | Purpose |
|---|---|
| `Learn` | Human understanding; often 5W2H, examples, analogy, and open questions |
| `Decide` | Applicability, alternatives, boundaries, and trade-offs |
| `Execute` | Compact steps, conditions, branches, checks, and stopping criteria for an agent |
| `Evidence` | Sources, revisions, validation, contradictions, and freshness |

5W2H is a useful Learn facet, not the atom boundary and not a mandatory template for
every node.

Important engineering kinds include:

- `design-decision`: background, constraints, choice, rejected alternatives,
  consequences, and invalidation conditions;
- `diagnostic-signal`: log/metric meaning, emission conditions, possible causes,
  validation steps, and false positives;
- `change-narrative`: previous behavior, new behavior, motivation, affected scope,
  commits, and compatibility risks;
- `procedure`: a reusable diagnostic or operational path.

### 3.2 Method

A runtime-independent way to perform a class of work. A Method defines phases,
questions, evidence standards, output contract, and quality checks. Examples include
abnormal-shutdown diagnosis, competitive research, technical document writing, and
presentation development.

A Method is not a runtime Skill. Claude/Codex owns concrete tools and Skills; Methodus
only provides the reviewed way of working.

### 3.3 Experience

A specific case with evidence and outcome: an incident investigation, a design attempt,
or a previous application of a Method. Experience should validate, contradict, or
refine reusable Knowledge. A Learn transcript is not automatically an Experience.

Experience may also record which runtime Skill was useful during a case. This is a
reviewable observation for future suggestions, not a Skill registry, installer, or
automatic Skill-evolution mechanism.

### 3.4 Source

Code, Git commits/diffs, docs, PRs, log specifications, incident material, and URLs are
evidence sources. Methodus may inspect them during Learn, but does not automatically
publish them as graph knowledge. Team content stores conclusions plus traceable source
references.

## 4. Graph semantics

Typed relations must help diagnosis and design rather than merely create a visual
network. The initial vocabulary includes:

- `requires`, `next_step`, `alternative_to`, `conflicts_with`;
- `indicates`, `emitted_by`, `caused_by`, `affects`;
- `implemented_by`, `introduced_by`, `supersedes`;
- `validated_by`, `contradicted_by`, `derived_from`.

Example:

```text
Method / abnormal shutdown diagnosis
  ├─ first_step   → Knowledge / previous shutdown reason
  ├─ next_step    → Knowledge / pre-shutdown crash detection
  └─ validated_by → Experience / device X power-loss incident
```

## 5. Product workflows

### 5.1 Learn and publish

```text
maintainer states a learning goal in the TUI
  → Methodus records the run, sources, protocol, and return-artifact path
  → terminal hands off to the selected native learning runtime
  → learning runtime questions scope and assumptions across multiple turns
  → maintainer attaches selected evidence sources
  → runtime investigates, contrasts, and seeks counterexamples
  → maintainer confirms consequential judgments
  → runtime writes a candidate set and typed relations to the run return artifact
  → Methodus resumes and imports the review-only candidates
  → maintainer splits, merges, edits, accepts, or rejects
  → accepted nodes become Personal
  → explicit promotion publishes selected nodes to Team through Git
```

One Learn run may propose zero or more Knowledge, Method, and Experience nodes. The
research record remains attached to the run; it is not forced into the graph.

### 5.2 Methodus Use

```text
maintainer asks a question in the TUI
  → Methodus creates a managed Use workspace and prepares a graph environment, Team directory structure, and inventory contract
  → selected native Runtime receives the graph directories and the question
  → TUI hands over the terminal to the Runtime
  → Runtime reads, reasons, and answers with facts, inferences, recommendations, and uncertainty
  → if committed evidence is insufficient, Runtime writes one learning recommendation return object
  → TUI is restored; the recommendation enters /attention and can become a Learn Goal
  → otherwise follow-up questions can resume the same native Use session
```

Use is an answer surface, not a canonical graph write path. Methodus prepares the
environment but does not preselect an answer with a local keyword heuristic. The
Runtime reads the complete relevant Markdown nodes itself. Temporary notes and plans
belong in the Methodus-managed Use workspace; graph content, project files, and
explicit evidence remain protected. Use never creates candidates or treats a stale
node as a current rule without warning.

When Use cannot answer from committed evidence, its contract forbids invention and
requires a concrete `learning_recommended` Learn task. Methodus records that return
as a HumanAttention item rather than letting the Runtime create a Goal directly. The
maintainer can accept the task or edit it in `/attention`; acceptance uses the same
default cadence, budget, and first-turn scheduling as `/goal <text>`.

### 5.3 Agent consumption

```text
developer asks Claude/Codex to diagnose or design
  → connector Skill recognizes a high-value task
  → Skill calls `methodus agent manifest`
  → CLI returns graph roots and the complete consumer-visible inventory
  → native agent selects relevant nodes and calls get/related
  → native runtime performs the work and owns all user interaction
```

Methodus does not create a task workspace, copy graph files, launch the runtime, read
the ordinary agent transcript, or manage the runtime's permissions.

### 5.3 Team distribution

Personal and Team are separate roots. Team is a normal Markdown Git repository:

- maintainers can use Methodus or edit Markdown directly;
- normal commits and pull requests remain valid review mechanisms;
- Methodus validates schema, links, duplicates, source references, and freshness;
- consumers sync a Team repository locally and query a read-only index;
- a hosted account or proprietary sync service is not required for the first product.

## 6. Trust and freshness

Canonical lifecycle:

```text
candidate → committed → stale → committed (after revalidation)
                       ↘ deprecated
candidate → rejected
```

- `candidate`: model- or human-proposed; never returned to ordinary Agent queries.
- `committed`: reviewed and eligible for consumption.
- `stale`: a referenced source changed or could not be revalidated. It may be returned
  only when highly relevant, with a prominent warning.
- `deprecated`: retained for change-history queries, never supplied as a current rule.
- `rejected`: retained for audit and excluded from consumption.

Source changes never rewrite conclusions automatically. Methodus detects risk;
maintainers decide how to update knowledge.

## 7. Product surfaces

### Maintainer TUI

- default Use question surface and Runtime synthesis from a Methodus-managed workspace;
- explicit Learn launcher and native-runtime handoff;
- Knowledge, Method, Experience, and graph browsing;
- candidate-set review and editing; richer split/relation editing is an incremental
  maintainer-surface extension;
- Review, conflict resolution, Personal/Team promotion;
- stale/source-health inspection;
- Team repository status, validation, diff, and publish-plan generation;
- learning runtime selection and learning-session resume.

The home screen is an agent-like Use conversation. `/learn` is the explicit deliberate
learning entry point; slash commands also switch to the management panels. Methodus is
not a general coding-agent shell.

### Agent CLI

A small, stable, non-interactive, read-only interface under `methodus agent`:

- `manifest`, `search`, `get`, `related`, and `status`;
- Markdown output by default and structured JSON on request;
- bounded result sizes, explicit node IDs, lifecycle state, evidence, and rationale.

### Connector Skill

Methodus ships one connector Skill for each supported runtime format. It teaches the
agent when and how to use the CLI. Methodus does not install, select, generate, evolve,
or approve arbitrary runtime Skills.

The shipped connector is the only Skill Methodus owns. It contains no graph data and
cannot write Personal or Team content.

## 8. Success criteria

Methodus succeeds when:

- one maintainer can turn engineering evidence into reviewed reusable conclusions;
- a developer's Agent retrieves the right diagnostic/design context without learning
  the Methodus TUI;
- every returned claim is traceable to status, source, and version;
- changed evidence produces visible stale state rather than silent misinformation;
- Team content remains readable, editable, reviewable, and distributable without
  Methodus-specific cloud infrastructure;
- the connector adds no graph content and ordinary Agent work remains fully native.
