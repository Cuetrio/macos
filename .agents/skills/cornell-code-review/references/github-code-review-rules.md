## Code Review Rules

### Intent, drift, and scope

- Reconstruct the intended change from explicit instructions, the pull-request
  title/body, linked Issues and acceptance criteria, and the repository's
  canonical contracts before judging the diff. Do not infer intended scope from
  the final diff. Check both DNF (required behavior missing/partial or an
  explicit constraint violated) and bad UC (unrequested behavior, public
  contract, schema, dependency, configuration, or unrelated refactor changes).
  Do not penalize narrowly necessary tests, validation, documentation, or safety
  support.
- Use these ARCTIC-style drift bands as a review signal: 0-10 perfect alignment,
  11-25 minor, 26-50 moderate, 51-75 significant, 76-100 major. A score is not a
  merge gate: low drift never excuses a correctness/security defect, and high
  drift must be explained with concrete DNF or bad UC evidence. When a summary
  is available, report the score/classification; otherwise use it to prioritize
  findings. If intent evidence is insufficient, write `Not scored`.
- Read
  `.agents/skills/cornell-code-review/references/arctic-review-rubric.md` for
  the detailed scoring anchors, exceptions, Spotlight format, and finding
  critic.

### Spotlight and finding quality

- Prioritize correctness and reliability, data safety and security, current-MVP
  and public-contract compatibility, persistence/transaction integrity,
  CanvasDocumentV1 and legacy Markdown compatibility, significant performance,
  then architecture and maintainability. Suppress style-only findings.
- Focus first on high-blast-radius changed paths: deletion, backup, sanitization,
  API/DTO boundaries, Prisma transactions or migrations, Canvas save/read-back,
  server/client boundaries, concurrency, and failure handling.
- Publish a finding only when it is technically correct, intent-relevant,
  consistent with repository rules, actionable, senior-review-worthy,
  introduced or materially worsened by the diff, and tied to a concrete changed
  line, trigger, and impact.

### Current MVP contract

- Treat `../product/doc/macos/implementation/MVP_CONTRACT.md` as the source of truth for current
  routes, APIs, save, delete, review, Canvas, and acceptance behavior. Flag a
  change that silently alters those contracts or mixes Phase 2 work such as
  autosave, soft delete/Undo, dedicated review tasks, NoteCard/D&D, PDF export,
  or tag-management mutations into an MVP change.
  Safe path: update the MVP/Phase 2 boundary and acceptance criteria explicitly,
  then change implementation, documentation, and verification evidence together.

### Note and Canvas persistence

- A note save must preserve Notebook, Canvas, Cue, and Tag relations as one
  consistent operation. For `bodyMode="canvas"`, the canonical body is a
  validated `CanvasDocumentV1` and legacy `Notebook.body` stays empty; legacy
  Markdown notes must not be auto-converted. Page-size changes must preserve all
  existing element geometry, style, text, order, and search semantics.
  Safe path: use the shared contracts and the existing application/repository
  transaction boundary, regenerate Canvas `searchText` from canonical text
  elements, and verify save/read-back behavior.

### Local data and trust boundary

- This application is authentication-free and intended for local personal use.
  Do not describe a shared/public deployment as safe without an explicit threat
  model and access control. Keep user Markdown sanitized, keep backup/prune
  operations inside the intended SQLite/backup paths, and preserve the current
  confirmation-plus-physical-delete contract.
  Safe path: retain the sanitized Markdown renderer, test backup logic only with
  disposable paths, and treat any network exposure or authentication change as a
  separately reviewed product and security decision.
