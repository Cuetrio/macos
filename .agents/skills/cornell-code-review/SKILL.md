---
name: cornell-code-review
description: >-
  Review pull requests, branch diffs, commits, or uncommitted changes in
  ka-akehi/Cornell-Method. Use for strict reviews of the repository's Next.js
  App Router, React, TypeScript, Zod, Prisma/SQLite, CanvasDocumentV1,
  Markdown, backup, API contracts, architecture, tests, and MVP/Phase 2 scope.
  Do not implement fixes unless the user explicitly asks for implementation.
---

# Cornell Method code review

## Purpose

Perform a high-signal review of changes to Cornell Method Notebook.
Prioritize user-visible correctness, data safety, current MVP contract
compatibility, dependency direction, runtime behavior, and regression
prevention over style preferences or comment volume.

A strict review is not a large review. Report only issues that can be tied to a
concrete trigger, impact, and changed line.

## Default behavior

- Review only. Do not edit files, commit, push, or open a pull request unless the
  user explicitly requests a fix after the review.
- Review the requested diff rather than auditing the whole repository.
- Inspect unchanged callers, callees, contracts, persistence code, and tests
  only as far as necessary to prove or disprove a finding.
- Report problems introduced by the reviewed change or made materially worse by
  it. Do not turn unrelated pre-existing debt into findings.
- Use Japanese for the review unless the user requests another language.
- Put findings before summaries.
- Do not invent findings when no material problem is found.

## Review-excluded operational records

The following paths are operational history, handoff, or agent execution records.
They may be read as context when resuming work, but they are not code-review
targets unless the user explicitly asks to review those files.

- root `HANDOFF.md`
- root `HANDOFF_*.md`
- `summary/**`
- `codex-queue/**/summary/**`
- `codex-queue/**/summaries/**`

For excluded paths:

- do not emit P0, P1, P2, or P3 findings
- do not critique wording, links, completeness, freshness, or record structure
- do not use the file itself as evidence that implementation is incorrect
- do not treat it as the source of truth for product behavior, architecture, or
  acceptance criteria
- do not recommend creating a GitHub Issue
- do not let an excluded-file-only change affect the verdict

Use `.github/CODEX_REVIEW_SCOPE.md` as the canonical path list. If an excluded
record conflicts with current code or a canonical contract, judge the code
against the canonical contract and do not report the record mismatch as a
finding.

## Inputs

Resolve these from the request and repository state:

- review target: pull request, branch diff, commit, staged changes, or working tree
- comparison base, normally the pull request base or `develop` for feature work
- stated change intent and acceptance criteria
- changed files and affected runtime paths
- commands that can be run safely in the current environment

When the scope is not explicitly stated, review the smallest unambiguous change
set available. State the assumed scope in the result.

## Mandatory repository context

Before judging implementation choices, read the applicable repository
instructions and current contracts in this order:

1. Root `AGENTS.md`.
2. The current handoff file referenced by `AGENTS.md` under "最新引き継ぎ".
3. `../product/doc/common/requirements/PRODUCT_SPEC.md` for product principles, roadmap, and the
   local-first / desktop boundary.
4. `../product/doc/common/requirements/MVP_SYSTEM_SPEC.md` for current MVP business and functional
   requirements.
5. `../product/doc/macos/implementation/MVP_CONTRACT.md` for current MVP implementation and
   acceptance behavior.
6. `../product/doc/macos/technical/TARGET_ARCHITECTURE.md` for placement and dependency rules.
7. `../product/doc/macos/implementation/IMPLEMENTATION_STATUS.md` for implemented, partial, and
   runtime-unverified boundaries.
8. Relevant sections of `../product/doc/macos/testing/TEST_SCENARIOS.md` for changed behavior.
9. More specific `AGENTS.md` files and design documents when they govern the
   changed files. Summaries, handoffs, and task records may supply context but
   remain excluded review targets.

Apply this precedence:

- Current MVP implementation and acceptance: `MVP_CONTRACT.md`.
- Current MVP business/functional requirements: `MVP_SYSTEM_SPEC.md`.
- Placement and dependency direction: `TARGET_ARCHITECTURE.md`.
- Product principles and future roadmap: `PRODUCT_SPEC.md`.
- Static/runtime verification status: `IMPLEMENTATION_STATUS.md` and
  `TEST_SCENARIOS.md`.

Do not treat a Phase 2 roadmap item as a missing MVP feature. Do not treat an
implementation-status statement as stronger evidence than the current code or a
newer runtime result.

## Intent and drift model

Before reviewing implementation details, read
`references/arctic-review-rubric.md`.

Keep two evidence sets separate:

- **Intent evidence**: explicit instructions, PR/Issue text, acceptance criteria,
  approved decisions, and canonical repository contracts.
- **Actual-change evidence**: the diff, changed runtime paths, unchanged
  callers/callees, tests, and verification results.

Reconstruct intent before using the diff body. Do not treat the code that was
written as proof that it was requested. If intent is missing or contradictory,
mark Drift as `Not scored` instead of inventing a target.

Classify material misalignment as:

- **DNF (Did Not Follow)**: required behavior is missing or partial, an explicit
  constraint is violated, or the change solves a different problem.
- **UC (Unrequested Change)**: the diff adds behavior, contracts, schema,
  dependencies, configuration, or refactoring outside the intended scope.

Do not penalize narrowly necessary tests, validation, documentation, or safety
support. Use the rubric's 0-100 bands as a review-priority signal, never as a
standalone merge gate.

## Review workflow

### 1. Establish the change boundary

Inspect the diff summary and changed filenames first. Remove review-excluded
operational records from the review set before classifying the change. Determine
whether the remaining change affects any of these areas:

- Next.js page, layout, or Route Handler
- notes UI, hook, model, remote client, or contract
- server application, repository, presenter, Prisma schema, or migration
- Canvas document, renderer, Fabric adapter, history, toolbar, or pointer logic
- Markdown rendering
- backup filesystem handling
- shared HTTP, date-only, validation, or UI primitives
- canonical documentation or acceptance evidence

Do not begin with naming or formatting comments.

### 2. Reconstruct the intended change

Before interpreting the diff, write a compact internal intent record from the
request, PR/Issue, acceptance criteria, approved decisions, and canonical
contracts:

- required outcomes;
- explicit constraints and invariants;
- prohibited or out-of-scope work;
- narrowly necessary supporting work;
- expected acceptance evidence;
- unresolved ambiguity.

Do not infer intent from the final implementation. When the evidence is
insufficient, keep the ambiguity explicit and do not score Drift.

### 3. Backtranslate the actual changes

After the intent record is fixed, translate the diff into a concise list of the
changes it actually makes:

- user-visible behavior;
- API, DTO, validation, and error contracts;
- persistence, transaction, schema, and migration behavior;
- CanvasDocumentV1 and legacy Markdown compatibility;
- state, concurrency, failure handling, and significant performance;
- dependencies, configuration, and trust boundaries;
- tests, documentation, and verification evidence.

Separate observed facts from inference. Inspect unchanged code only as far as
needed to prove the actual effect.

### 4. Assess DNF, UC, and Drift

Compare the intended outcomes with the backtranslated changes using
`references/arctic-review-rubric.md`.

- Identify every material DNF item.
- Separate beneficial/necessary support from bad UC.
- Assign a 0-100 score and classification only when intent evidence is adequate.
- Explain the evidence behind the band; never report a bare score.
- Do not turn harmless scope differences into findings.
- Do not let low Drift suppress correctness, security, or data-safety findings.

### 5. Reconstruct the affected path

For each changed behavior, trace the relevant path end to end where applicable:

`page/UI -> model/payload -> remote -> Route Handler -> application -> repository -> Prisma/filesystem -> presenter/DTO -> UI`

For Canvas changes, also trace:

`CanvasDocumentV1 -> editor/runtime/adapter -> document change -> validation -> persistence -> mapper -> viewer/editor restore`

Use this trace to find contract drift, dropped fields, stale state, non-atomic
writes, and incorrect dependency direction.

### 6. Test adversarial cases mentally and, when safe, at runtime

At minimum consider:

- empty, omitted, null, invalid, duplicate, minimum, and maximum values
- repeated submit, rapid interaction, stale async completion, and partial failure
- month-end/year-end and local-time/UTC boundaries for date-only values
- create, update, read-back, review, delete, and not-found behavior
- Canvas save/reload, undo/redo, pointer lifecycle, resize, malformed JSON, and
  legacy Markdown compatibility
- backup filename collision, missing DB, invalid `DATABASE_URL`, pruning, and
  filesystem failure
- mobile width, keyboard operation, focus, labels, and error announcements

### 7. Run safe verification

Use the repository's actual scripts. The normal static verification set is:

```bash
npm run lint
npx tsc --noEmit
npm run build
```

When Prisma schema, client usage, or migration files change, also consider:

```bash
npx prisma validate
npm run prisma:generate
```

Safety rules:

- Do not run `npm run prisma:migrate` against the user's normal database merely
  for review. Use an isolated temporary database when migration execution is
  necessary and available.
- Do not create, prune, or overwrite backups against the user's normal `dev.db`.
  Test backup behavior with a temporary project root and disposable SQLite file.
- Do not invent `npm test` or a Playwright command when the repository does not
  define it. Locate an existing command or state that runtime verification was
  not executed.
- A successful lint, typecheck, or build is not proof that Canvas pointer,
  Fabric lifecycle, wheel/touch, responsive layout, or save/reload behavior
  works in a browser.

Record every command run and its result. Separate verification failures caused
by the change from environment limitations.

### 8. Build Spotlight and validate candidates

Rank at most five changed areas that deserved the deepest inspection. Prioritize
correctness, data safety, security, current-MVP/public contracts,
persistence/transactions, Canvas/legacy compatibility, concurrency, and
significant performance before architecture or maintainability.

Spotlight is not a finding quota. For each area, record the evidence inspected
and whether it produced a finding, was cleared, or remains a residual risk.

Before publishing any candidate, apply both the ARCTIC-style critic in
`references/arctic-review-rubric.md` and the Finding acceptance test below.
Suppress style-only, speculative, unsupported, and low-value comments.

## Detailed checklist

Before writing findings:

1. read `references/arctic-review-rubric.md` for intent reconstruction,
   DNF/UC, Drift, Spotlight, and candidate-critic rules;
2. read `references/review-checklist.md` and apply only the sections relevant to
   the changed files and runtime path.

Both references are search aids, not quotas. Do not create a finding merely
because an item exists.

## Severity

Use these priorities:

- **P0**: immediate data loss, broad secret exposure, arbitrary code execution,
  or application-wide failure.
- **P1**: a likely core-flow regression, stored-data corruption, XSS, destructive
  backup/delete behavior, broken transaction boundary, or serious server/client
  boundary failure.
- **P2**: a real edge-case bug, contract drift, important architecture regression,
  significant performance issue, or missing regression coverage with credible
  impact.
- **P3**: optional maintainability or readability improvement. Keep these out of
  Findings unless the user asks for exhaustive notes.

Do not inflate severity because a rule sounds important. Base it on trigger,
reach, impact, and recoverability.

## Finding acceptance test

Include a finding only when all are true:

1. The reviewed change introduces or materially worsens the issue.
2. The exact changed file and line range can be identified.
3. A concrete triggering input, state, or operation order can be stated.
4. The user or system impact can be explained.
5. The evidence survives inspection of relevant callers, contracts, and guards.
6. A practical fix direction and verification scenario can be proposed.
7. The issue affects the intended change or a material repository invariant
   crossed by the diff.
8. The comment is important enough for a senior engineer to raise and is not
   merely style, preference, or speculative cleanup.

Mark uncertain inferences as assumptions. Prefer `Confidence: Medium` or omit the
finding when evidence is insufficient.

## Output format

Use this structure:

```text
## Findings

### [P1] Concrete title
- Location: `path/to/file.ts:line`
- Trigger: Exact input, state, or ordering
- Impact: User or system consequence
- Evidence: Relevant code path, contract, or command result
- Suggested fix: Smallest root-cause fix direction
- Test: Specific regression or runtime scenario
- Review heuristic: Reusable lesson for a human reviewer
- Confidence: High | Medium

## Intent and drift assessment
- Intent evidence: PR / Issue / acceptance criteria / canonical contract
- Required outcomes: concise list
- DNF: material unmet or partial requirements, or `該当なし`
- Bad UC: material unrequested changes, or `該当なし`
- Necessary support not penalized: concise list or `該当なし`
- Drift: `18 / 100 - Minor` or `Not scored`
- Rationale: evidence for the band
- Confidence: High | Medium | Low

## Spotlight
1. `path or runtime area`
   - Why: high-value review reason
   - Evidence inspected: callers, contracts, tests, runtime result
   - Result: Finding emitted | Cleared | Residual risk
   - Related finding: identifier or `なし`

## Contract and scope assessment
- State whether the change matches the current MVP contract and whether any
  Phase 2 behavior was mixed in.

## Architecture assessment
- State only consequential placement/dependency findings or say that none were
  found.

## Verification
- Commands run and results
- Runtime checks performed
- Checks not run and why
- Residual risk

## Verdict
- BLOCK: P0/P1 makes merge unsafe
- REQUEST CHANGES: P2 should be fixed before merge
- APPROVE WITH NOTES: only non-blocking residual risk
- APPROVE: no material issue found
```

Order findings by severity, then by confidence and user impact. Keep one root
cause per finding. Do not bury findings below a general summary.

Drift is not a severity substitute. A low-drift P1 still blocks. High Drift
without a concrete defect or violated contract is reported as a scope concern,
not automatically promoted to P0/P1/P2.

When no finding qualifies, write:

```text
## Findings
重大な問題は確認できませんでした。
```

Then still report Intent and Drift, Spotlight, the reviewed scope, verification
performed, and any runtime area that remains unverified.

## Invocation examples

```text
$cornell-code-review developとの差分を厳格にレビューしてください。
```

```text
$cornell-code-review このPRを、MVP契約、Target Architecture、Canvas保存互換性、
ブラウザruntimeの観点からレビューしてください。
```

```text
$cornell-code-review 未コミット変更をレビューしてください。コードは変更せず、
P0〜P2のFindingと不足テストだけを出してください。
```
