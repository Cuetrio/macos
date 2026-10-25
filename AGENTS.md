# Repository Instructions

このリポジトリは、ローカル個人利用向けの Cornell Method Notebook アプリです。

## Primary References

- 製品全体仕様・ロードマップの正本: [`product/doc/common/requirements/PRODUCT_SPEC.md`](../product/doc/common/requirements/PRODUCT_SPEC.md)
- 現行 MVP の業務・機能要件: [`product/doc/common/requirements/MVP_SYSTEM_SPEC.md`](../product/doc/common/requirements/MVP_SYSTEM_SPEC.md)
- 現行 MVP の実装・受け入れ契約: [`product/doc/macos/implementation/MVP_CONTRACT.md`](../product/doc/macos/implementation/MVP_CONTRACT.md)
- 実装状況: [`product/doc/macos/implementation/IMPLEMENTATION_STATUS.md`](../product/doc/macos/implementation/IMPLEMENTATION_STATUS.md)
- テスト観点: [`product/doc/macos/testing/TEST_SCENARIOS.md`](../product/doc/macos/testing/TEST_SCENARIOS.md)
- 設計書一覧と各文書の役割: [`product/doc/common/README.md`](../product/doc/common/README.md)
- 共通開発ルール: [`product/development/AGENTS.md`](../product/development/AGENTS.md)
- Task Summary 運用: [`product/summary/README.md`](../product/summary/README.md)
- 最新引き継ぎ: [`product/handoff/macos/HANDOFF_2026-09-07.md`](../product/handoff/macos/HANDOFF_2026-09-07.md)
- コードレビューの観点と基準: product の `.agents/skills/cornell-code-review/`

製品全体の方針と現行 MVP の契約が異なる場合、現行 MVP の実装判断では `MVP_CONTRACT.md` を優先します。将来機能は製品ロードマップとして扱い、現行 MVP に混ぜません。

## Development Policy

- 既存の Next.js App Router、React、Prisma、SQLite 構成を前提に進める。
- 作業前に `git status --short` を確認し、ユーザーの未コミット変更を戻さない。
- 依存関係、DB、UI、API の不整合を見つけた場合は、推測で隠さず明示する。
- 実装は小さく分割し、対象外のリファクタリングを避ける。
- 検証可能な作業では `npm run lint`、`npm run build`、Prisma コマンドなど、適切な確認を行う。
- 検証できない場合は、実行したコマンドと失敗理由を報告する。
- 長い調査や作業の完了要約は `../product/summary/macos/` 配下へ、`task-summary-template.md` の粒度で残し、raw log や長い command output をメイン会話へ戻さない。
- 再開時は関連 summary を先に読み、`Next Read` に記載された最小ファイルだけを確認する。
- 作業再開時は、最新の `HANDOFF_YYYY-MM-DD.md` を確認してから続きの作業を判断する。
- 新しい `HANDOFF_YYYY-MM-DD.md` を作成した場合は、`AGENTS.md` の「最新引き継ぎ」を新しいファイルへ更新し、古い `HANDOFF_YYYY-MM-DD.md` は削除する。

## Local Data and Trust Boundary

このアプリは認証のないローカル個人利用を前提とします。明示的な脅威モデルとアクセス制御なしに、共有・公開での運用を安全と扱いません。ユーザーの Markdown は sanitize したまま扱い、backup / prune は想定した SQLite・backup の path 内に限定し、確認後の物理削除の契約を維持します。

## 開発セッションと PR

開発セッションの起動方法と共通の作業方針は [`product/development/AGENTS.md`](../product/development/AGENTS.md) に従います。

- PR は Draft ではなく、レビュー可能な Open 状態で作成する。
- GitHub Issue の修正を含む PR では、修正した Issue ごとに PR 本文へ `Closes #[Issue番号]` を追加する。Issue の自動クローズ参照は PR 本文に記載する。
- `../product/summary/macos/` 配下の 作業要約 は PR の変更ファイルに含めない。summary は product 側に残し、PR 本文の変更説明や Issue の自動クローズ参照の代わりにはしない。
