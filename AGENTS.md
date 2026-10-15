# Repository Instructions

このリポジトリは、ローカル個人利用向けの Cornell Method Notebook アプリです。

## Primary References

- 製品全体仕様・ロードマップの正本: [`product/doc/common/requirements/PRODUCT_SPEC.md`](../product/doc/common/requirements/PRODUCT_SPEC.md)
- 現行 MVP の業務・機能要件: [`product/doc/common/requirements/MVP_SYSTEM_SPEC.md`](../product/doc/common/requirements/MVP_SYSTEM_SPEC.md)
- 現行 MVP の実装・受け入れ契約: [`product/doc/macos/implementation/MVP_CONTRACT.md`](../product/doc/macos/implementation/MVP_CONTRACT.md)
- 実装状況: [`product/doc/macos/implementation/IMPLEMENTATION_STATUS.md`](../product/doc/macos/implementation/IMPLEMENTATION_STATUS.md)
- テスト観点: [`product/doc/macos/testing/TEST_SCENARIOS.md`](../product/doc/macos/testing/TEST_SCENARIOS.md)
- 設計書一覧: [`product/doc/common/README.md`](../product/doc/common/README.md)
- 共通開発ルール: [`product/development/AGENTS.md`](../product/development/AGENTS.md)
- Task Summary 運用: [`product/summary/README.md`](../product/summary/README.md)
- 最新引き継ぎ: [`product/handoff/macos/HANDOFF_2026-09-07.md`](../product/handoff/macos/HANDOFF_2026-09-07.md)

### 仕様書の役割分担

`product/doc/common/requirements/PRODUCT_SPEC.md` は、製品全体の概要、製品原則、MVP / Phase 2 / 将来ロードマップ、Mac desktop / local-first の配布・保存方針を管理します。`product/doc/common/requirements/MVP_SYSTEM_SPEC.md` は現行 MVP の業務要件・機能要件を管理し、`product/doc/macos/implementation/MVP_CONTRACT.md` は現行 MVP の route、API、保存・削除・復習方式、実装・受け入れ判断の正本です。`product/doc/macos/technical/`、`product/doc/macos/data/`、`product/doc/macos/api/`、`product/doc/macos/screens/`、`product/doc/macos/workflows/`、`product/doc/macos/testing/` は各詳細設計・検証観点を担います。

製品全体の方針と現行 MVP の契約が異なる場合、現行 MVP の実装判断では `MVP_CONTRACT.md` を優先します。将来機能は製品ロードマップとして扱い、現行 MVP に混ぜません。

## Development Policy

- 既存の Next.js App Router、React、Prisma、SQLite 構成を前提に進める。
- 作業前に `git status --short` を確認し、ユーザーの未コミット変更を戻さない。
- 依存関係、DB、UI、API の不整合を見つけた場合は、推測で隠さず明示する。
- 実装は小さく分割し、対象外のリファクタリングを避ける。
- 検証可能な作業では `npm run lint`、`npm run build`、Prisma コマンドなど、適切な確認を行う。
- 検証できない場合は、実行したコマンドと失敗理由を報告する。
- 長い調査や作業の完了要約は `../product/summary/macos/` 配下へ残し、raw log や長い command output をメイン会話へ戻さない。
- 再開時は関連 summary を先に読み、`Next Read` に記載された最小ファイルだけを確認する。
- 作業再開時は、最新の `HANDOFF_YYYY-MM-DD.md` を確認してから続きの作業を判断する。
- 新しい `HANDOFF_YYYY-MM-DD.md` を作成した場合は、`AGENTS.md` の「最新引き継ぎ」を新しいファイルへ更新し、古い `HANDOFF_YYYY-MM-DD.md` は削除する。

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

## 開発セッションの起動

共通の作業方針は [`product/development/AGENTS.md`](../product/development/AGENTS.md) を参照します。開発セッションはホームの共通基盤から起動します。

```sh
cd ~
make cornell codex
# Claude を使う場合
make cornell claude
make cornell status
make cornell stop
```

ユーザーと相談して task を具体化します。必要に応じて `../product/summary/macos/task-summary-template.md` の粒度で完了要約を残します。

### PR 作成ルール

- PR は Draft ではなく、レビュー可能な Open 状態で作成する。
- GitHub Issue の修正を含む PR では、修正した Issue ごとに PR 本文へ `Closes #[Issue番号]` を追加する。Issue の自動クローズ参照は PR 本文に記載する。
- `../product/summary/macos/` 配下の 作業要約 は PR の変更ファイルに含めない。PR 作成前に summary ファイルをステージ対象から除外する。summary は product 側に残し、PR 本文の変更説明や Issue の自動クローズ参照の代わりにはしない。

---

## Product Boundary Summary

製品全体の方針・ロードマップは [`product/doc/common/requirements/PRODUCT_SPEC.md`](../product/doc/common/requirements/PRODUCT_SPEC.md) を参照します。製品は個人向け local-first を基本とし、クラウド DB・認証・外部 API を必須にせず、将来の主な配布経路を Mac desktop とします。開発・検証用の Next.js Web 起動形態は維持します。

現行 MVP は、`/notes`、`/notes/new`、`/notes/[id]`、`/backup` を中心に、Cue / Summary と自由配置 Canvas のノート、明示保存、手動 SQLite backup、確認後の物理削除を提供します。自動保存、Undo / soft delete、専用復習タスク、NoteCard / D&D、PDF export、自動バックアップは Phase 2 以降です。Mac desktop 化では `app bundle` と `user data directory` を分離し、SQLite を運用上の正本として file export / backup を段階導入します。Electron / Tauri + Node.js sidecar の選定は Desktop PoC 後に判断します。
