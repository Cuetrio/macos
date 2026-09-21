# ARCTIC-style review rubric for Cornell Method

## Purpose and scope

Use this rubric to review AI-generated or human-authored diffs by separating:

1. the intended change;
2. the changes the diff actually makes;
3. material drift between those two;
4. the changed areas that deserve the most reviewer attention.

This is an ARCTIC-inspired repository workflow based on the public method
described in arXiv:2607.29516v1. It does not claim to reproduce unpublished
production prompts, source code, datasets, or scoring implementation from that
work. The calibration below is specific to Cornell Method.

The drift score is a review signal, not a merge gate. A low-drift change can
still contain a P0/P1 defect, and a high-drift change is not automatically
unsafe unless the drift creates a concrete contract, correctness, safety,
performance, or reviewability problem.

## Evidence separation

Keep intent evidence and implementation evidence separate.

### Intent evidence

Reconstruct intent before using the diff body as evidence. Prefer, in order:

1. explicit user instructions;
2. pull-request title and body;
3. linked Issue, acceptance criteria, and approved design decisions;
4. current canonical repository contracts:
   - `../product/doc/common/requirements/PRODUCT_SPEC.md` for product principles and roadmap;
   - `../product/doc/common/requirements/MVP_SYSTEM_SPEC.md` for current MVP business and
     functional requirements;
   - `../product/doc/macos/implementation/MVP_CONTRACT.md` for current MVP implementation and
     acceptance behavior;
   - `../product/doc/macos/technical/TARGET_ARCHITECTURE.md` for placement and dependency rules;
   - relevant `../product/doc/macos/testing/TEST_SCENARIOS.md` for verification expectations;
5. agent plan or conversation when it is available and authoritative.

Do not infer the intended scope from the final diff itself. The diff is evidence
of what changed, not evidence that the change was requested.

Record:

- required outcomes;
- explicit constraints and invariants;
- explicitly prohibited or out-of-scope work;
- narrowly necessary supporting work;
- acceptance evidence expected;
- unresolved ambiguity.

When intent evidence is too weak or contradictory, mark drift as `Not scored`
and explain the missing evidence rather than inventing a target.

### Actual-change evidence

After the intent record is fixed, backtranslate the diff into a concise list of
what it actually changes. Cover only observable or well-supported effects:

- user-visible behavior;
- API, DTO, validation, and error contracts;
- persistence, transactions, schema, and migration behavior;
- CanvasDocumentV1 and legacy Markdown compatibility;
- control flow, state, concurrency, and failure handling;
- dependencies, configuration, and deployment/trust boundaries;
- tests, documentation, and verification evidence.

Separate observed facts from inferences. Inspect relevant unchanged callers,
callees, contracts, guards, and tests before asserting runtime impact.

## Drift classes

### DNF: Did Not Follow

Classify as DNF when the change:

- omits a required outcome;
- implements only part of a required outcome;
- violates an explicit constraint or invariant;
- solves a materially different problem;
- claims completion without the required acceptance evidence.

### UC: Unrequested Change

Classify as UC when the change adds work outside the reconstructed intent.

Do not penalize narrowly scoped supporting changes that are necessary to deliver
or verify the requested behavior, such as:

- focused regression tests;
- validation required by the existing contract;
- safe error handling for a newly introduced failure path;
- documentation or acceptance evidence that must change with the behavior;
- a small safety fix that is inseparable from the requested change.

Treat an unrequested change as bad UC when it materially increases risk or
review surface, for example:

- changes public APIs, DTOs, persistence shape, or migrations without need;
- mixes Phase 2 behavior into a current-MVP change;
- adds a dependency, configuration surface, or network exposure without need;
- performs unrelated refactoring or file movement;
- changes deletion, backup, sanitization, or trust-boundary behavior;
- auto-converts legacy Markdown or mutates Canvas geometry outside the request;
- removes or weakens tests, validation, guards, or compatibility behavior.

## Drift scoring

Choose the band holistically. Do not average dimensions mechanically.

| Score | Classification | Anchor |
| ---: | --- | --- |
| 0-10 | Perfect alignment | All material intent is met; explicit constraints are preserved; no bad UC. |
| 11-25 | Minor drift | Intent is substantially complete; only small partials or low-risk scope expansion exist. |
| 26-50 | Moderate drift | Most intent is met, but a substantive requirement is partial/unmet or notable bad UC expands behavior or review surface. |
| 51-75 | Significant drift | Only part of the intent is met, a core constraint is violated, or risky/broad UC materially changes contracts or behavior. |
| 76-100 | Major drift | The change largely solves the wrong problem, misses most core outcomes, removes required behavior, or introduces major breaking/unrequested change. |

Select a point inside the band using:

- importance of the affected requirement;
- blast radius;
- reversibility and data impact;
- public-contract or persistence impact;
- amount of added review surface;
- confidence in the evidence.

Report:

- score and classification;
- DNF items;
- bad UC items;
- beneficial/necessary supporting changes that were not penalized;
- a short rationale;
- confidence: High, Medium, or Low.

A score must never hide the underlying evidence.

## Cornell Method calibration examples

Usually no drift:

- adding focused tests for the requested behavior;
- preserving the physical-delete MVP contract while fixing delete validation;
- adding validation that the existing API contract already requires;
- updating acceptance evidence alongside an intentional contract change.

Usually material drift:

- introducing autosave, soft delete/Undo, dedicated review tasks, NoteCard/D&D,
  PDF export, or tag-management mutations into an unrelated MVP fix;
- splitting Notebook, Canvas, Cue, or Tag persistence without a requested and
  safe transaction-boundary change;
- adding a Prisma migration for a Canvas page-size-only change;
- auto-converting legacy Markdown notes;
- changing the authentication-free local-only trust boundary;
- broad refactoring, dependency replacement, or configuration changes unrelated
  to the requested outcome.

## Spotlight ranking

Create a short ranked list of the changed areas that deserved the deepest
inspection. Spotlight is not a quota for findings and must not invent defects.

Prioritize:

1. correctness, data loss, corruption, security, sanitization, backup, deletion;
2. current-MVP contract, public API, transaction, persistence, and compatibility;
3. concurrency, stale state, error paths, and significant performance risk;
4. server/client boundaries and dependency direction;
5. maintainability only when it creates concrete future defect or review risk.

For each of at most five spotlight items, report:

- location or affected path;
- why it has high review value;
- evidence inspected;
- result: `Finding emitted`, `Cleared`, or `Residual risk`;
- related finding identifier when one exists.

Do not repeat the complete finding body in Spotlight.

## Candidate-finding critic

Before publishing a finding, require all of the following:

1. **Technical correctness**: the claim survives code-path and contract checks.
2. **Intent relevance**: it affects the requested change or a material invariant
   crossed by the diff.
3. **Repository fit**: it follows Cornell Method's current MVP, architecture,
   safety, and review-scope rules.
4. **Actionability**: a concrete trigger, impact, fix direction, and verification
   scenario can be stated.
5. **Senior-review value**: it is important enough to interrupt the author and
   is not merely style, preference, or speculative cleanup.
6. **Changed-line attribution**: the issue was introduced or materially worsened
   by the reviewed change and can be tied to a changed line or hunk.

Suppress the candidate when the evidence is insufficient. Use a confidence
label rather than overstating an inference.

## Output contract

Keep actionable findings first, then provide:

1. `## Intent and drift assessment`;
2. `## Spotlight`;
3. contract and scope assessment;
4. architecture assessment;
5. verification;
6. verdict.

When no intent evidence is available, write `Drift: Not scored`. When no
material DNF or bad UC exists, state `該当なし`. When a spotlight area was
inspected and cleared, say so rather than omitting all evidence of the check.

## Provenance

Method inspiration:

- *From Code Review to Code Critique: Intent, Drift, and Spotlight for
  AI-Generated Diffs at Scale*, arXiv:2607.29516v1.

The repository-specific thresholds, examples, output contract, and critic gates
in this file are operational policy for Cornell Method.
