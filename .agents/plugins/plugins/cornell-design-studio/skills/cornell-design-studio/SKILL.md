---
name: cornell-design-studio
description: Use when designing, reviewing, or tasking UI/UX work for the Cornell Method Notebook repository, especially when the user wants a Google Stitch or Claude Design style workflow inside Codex.
---

# Cornell Design Studio

Use this skill to run a local design loop for the Cornell Method Notebook app. The goal is to approximate Google Stitch / Claude Design operations with repository-native artifacts: design briefs, UI variants, review matrices, implementation handoffs, Worker tasks, and synchronized design documents.

## Required Context

Before making design decisions, read the smallest relevant set from:

- `AGENTS.md`
- `../product/doc/macos/screens/MVP_SCREEN_DESIGN.md`
- `../product/doc/macos/screens/MVP_SCREEN_INVENTORY.md`
- `../product/doc/macos/workflows/MVP_WORKFLOW_DESIGN.md`
- `../product/doc/common/requirements/MVP_SYSTEM_SPEC.md`
- `../product/doc/macos/technical/MVP_DESIGN_TOOLING_GUIDE.md`
- Related route/component files under `src/app/`

For implementation tasking, also read:

- `../product/development/AGENTS.md`

## Workflow

1. Intake
   - Capture the target screen, user goal, MVP / Phase 2 boundary, constraints, and existing references.
   - Use `../product/templates/design-studio/design-brief.md` when a durable brief is useful.

2. Generate Options
   - Produce 2 or 3 text wireframe variants when the screen direction is unclear.
   - Keep variants implementable with the existing Next.js App Router, React, Tailwind, and current dependencies.
   - Avoid marketing-page composition unless the user explicitly asks for a landing page.

3. Review
   - Compare variants against the MVP design docs, accessibility, local personal-use workflow, validation states, empty states, error states, and mobile layout.
   - Record decisions with `../product/templates/design-studio/variant-review.md` when the choice should be auditable.

4. Handoff
   - Convert the selected option into either a Worker task or a direct implementation plan.
   - Use `../product/templates/design-studio/implementation-handoff.md` for implementation-ready details.
- Start the shared Worker platform from home with `cd ~ && make cornell codex` or `cd ~ && make cornell claude`. Check with `cd ~ && make cornell status`; stop with `cd ~ && make cornell stop`. Preserve the implementation repository and product summary boundaries.

5. Verify
   - Prefer browser verification for UI changes when a dev server can run.
   - If browser verification is unavailable, document the reason and fall back to `npm run lint`, `npm run build`, and code review.

6. Sync Docs
   - Update the relevant design document after a decision or implementation changes behavior.
   - Keep `AGENTS.md` as the specification source of truth.

## Constraints

- Do not send real notebook data, local DB contents, secrets, env vars, or backup files to external services.
- Do not treat generated UI ideas as accepted requirements until the user or existing design docs support them.
- Do not add npm dependencies just to imitate an external design tool.
- Do not introduce unrelated redesigns while implementing a selected screen change.

## Output Style

When responding to the user, keep the result actionable:

- State the target screen or workflow.
- Show the recommended option first.
- Mention important tradeoffs.
- List files or Worker tasks that need to change.
- Call out unresolved decisions explicitly.
