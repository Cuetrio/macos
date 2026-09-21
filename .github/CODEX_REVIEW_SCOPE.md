# Codex review scope

このファイルは、Codex Code Reviewで扱う変更範囲の共通ルールを定義します。

## レビュー対象外

次のパスは作業履歴、引き継ぎ、エージェント実行記録であり、製品コード、仕様書、受け入れ契約ではありません。

- ルート直下の `HANDOFF.md`
- ルート直下の `HANDOFF_*.md`
- `summary/**`
- `codex-queue/**/summary/**`
- `codex-queue/**/summaries/**`

これらのファイルは必要に応じて作業再開の文脈として読むことはできますが、通常のレビューでは次を行いません。

- P0、P1、P2、P3のFindingを出さない
- 文章、参照先、記録の完全性、古さを指摘しない
- Issue化しない
- マージ可否の根拠にしない
- 仕様、設計、受け入れ契約の正本として扱わない

ユーザーが対象ファイルのレビューを明示的に依頼した場合だけ、上記の除外を解除します。

## 正本

実装判断とレビューでは、変更内容に応じて次の正本を優先します。

- 現行MVP契約: `../product/doc/macos/implementation/MVP_CONTRACT.md`
- Target Architecture: `../product/doc/macos/technical/TARGET_ARCHITECTURE.md`
- 実装状況: `../product/doc/macos/implementation/IMPLEMENTATION_STATUS.md`
- テスト観点: `../product/doc/macos/testing/TEST_SCENARIOS.md`
- 製品ロードマップ: `AGENTS.md`
