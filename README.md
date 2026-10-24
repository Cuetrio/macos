# Cornell Method Notebook MVP

ローカルで個人利用する Cornell Method Notebook アプリです。Next.js App Router、React、Prisma、SQLite を使い、学習ノートの作成から検索、閲覧、編集、復習、バックアップまでを扱います。

MVP の主な機能:

- ノート作成
- 一覧検索（title、Summary、Cue、legacy Markdown 本文、Canvas text）
- 詳細閲覧 / 編集 / 復習モード
- Canvas 本文の editor / viewer
- Cue / Summary の Markdown 入力と安全な preview
- 既存 Markdown 本文の互換表示
- タグ登録とタグ検索
- SQLite DB バックアップ

現行の利用経路は、認証なしでローカル PC 上に起動し、SQLite へ保存する経路だけです。依存関係の取得を除き、起動と利用にネットワーク接続や外部サービスは必要ありません。

## 現行の状態と作業入口

2026-08-08 時点の実装状態は [`product/doc/macos/implementation/IMPLEMENTATION_STATUS.md`](../product/doc/macos/implementation/IMPLEMENTATION_STATUS.md) で確認します。

MVP の受け入れ項目と確認済み範囲は [`product/doc/macos/testing/TEST_SCENARIOS.md`](../product/doc/macos/testing/TEST_SCENARIOS.md) のチェックリストと受け入れ証跡マトリクスで確認します。

最新の Browser runtime QA の制約、再開条件、`Next Read` は product 側の macOS handoff に記録しています。

Gate 0（人力 MVP 結合テスト）は未通過です。

最新の handoff では Browser backend が利用できず、必須の UI runtime QA に `BLOCKED` または `NOT RUN` が残っています。

過去の screenshot、静的確認、部分的な runtime 確認、queue の完了表示だけでは Gate 0 の通過と判定しません。

Gate 0 の完了条件と、その後の実装順は [`product/doc/macos/implementation/POST_MVP_IMPLEMENTATION_PLAN.md`](../product/doc/macos/implementation/POST_MVP_IMPLEMENTATION_PLAN.md) を参照します。

Gate 0 を発注者が明示的に閉じるまで、Phase 2、Mac desktop、PDF export、部分消しゴムなどの coding task を投入しません。

主な現行実装パスは次のとおりです。

| 責務 | 現行パス |
| --- | --- |
| route | `src/app/notes/page.tsx`、`src/app/notes/new/page.tsx`、`src/app/notes/[id]/page.tsx`、`src/app/backup/page.tsx` |
| ノート UI | `src/modules/notes/ui/components/editor/**`、`src/modules/notes/ui/components/detail/**`、`src/modules/notes/ui/components/canvas/**` |
| Canvas 共通契約・Fabric adapter | `src/shared/canvas/**` |
| ノート保存・検索 | `src/server/notes/infrastructure/**` |
| バックアップ | `src/app/api/backups/route.ts`、`src/server/backup/application/**`、`src/server/backup/infrastructure/local-sqlite-backup-provider.js`、`src/modules/backup/ui/components/backup-page.tsx` |

上表のパスは、2026-08-08 の作業ツリーで `rg --files src` により確認した現行入口です。

`src/lib/backup/index.js` は既存 import 互換の再エクスポートであり、バックアップ実装の主入口として扱いません。

## 開発セッションの起動

ホームの共通基盤から `cd ~ && make cornell codex` または `cd ~ && make cornell claude` で起動します。状態確認は `cd ~ && make cornell status`、停止は `cd ~ && make cornell stop` を使います。作業方針は [`product/development/AGENTS.md`](../product/development/AGENTS.md) を参照します。

## セットアップ

前提:

- Node.js 24.22.1 LTS（`.nvmrc`）
- npm

依存関係をインストールします。

```bash
npm install
```

現行製品でサポートする `DATABASE_URL` は、ローカル SQLite ファイルを指す `file:` URL だけです。設定は任意で、未指定時はプロジェクトルートの `file:./dev.db` を Prisma CLI、Next runtime、`npm run backup:copy` で共有します。別の SQLite ファイルを使う場合は、shell または未追跡の `.env` に `file:` URL を設定します。shell の値は `.env` より優先されます。

`DATABASE_URL` の空文字、空白のみ、パスが空の `file:` URL はエラーになります。`file:` 以外の URL は現行製品のサポート対象外です。Next runtime には過去の検討資産が残っており、一部のサポート対象外 URL を受理します。空値や不正な SQLite URL から別 DB への fallback は行いません。`.env` が存在していて読み込めない場合も、fallback せずエラーになります。別のパスを使う場合は、同じプロジェクトルートを基準にした SQLite URL を設定してください。

使用できる relative / absolute URL は、`file:./relative.db`、`file:/absolute/path.db`、`file:///absolute/path.db` です。query (`?`)、fragment (`#`)、`file://host/path` の authority は拒否します。パス中の `%20` などの percent encoding は decode せず、`encoded%20name.db` という実ファイル名として扱います。

Prisma Client を生成し、SQLite DB を作成します。

```bash
npm run prisma:generate
npm run prisma:migrate
```

MVP は seed を使いません。`package.json` に seed script はなく、初期データは `/notes/new` の UI または `POST /api/notes` から作成します。

開発サーバーを起動します。

```bash
npm run dev
```

ブラウザで次を開きます。

```text
http://localhost:3000/notes
```

## 主な npm Scripts

| コマンド | 用途 |
| --- | --- |
| `npm run dev` | Next.js 開発サーバーを起動 |
| `npm run build` | 本番ビルドを webpack で実行 |
| `npm run lint` | ESLint を実行 |
| `npm run prisma:generate` | Prisma Client を生成 |
| `npm run prisma:migrate` | Prisma migration を適用し SQLite DB を作成 / 更新 |
| `npm run backup:copy` | プロジェクトルートの `.env`（shell の `DATABASE_URL` があればそちらを優先）と同じ SQLite DB を `backup/` 配下へコピー |
| `npm run diagrams:build` | Mermaid 図を `.mmd` に抽出し、SVG を生成 |

### 過去の検討資産

リポジトリには、現行方針の決定前に作成した Vercel、Supabase、PostgreSQL、hosted Basic Auth 関連の設定や script が残っています。これらは採用しない検討履歴と legacy tooling であり、現行のセットアップ、利用手順、運用経路、製品ロードマップには含めません。資産を削除するか保管するかは、別の read-only 棚卸し後に判断します。

## 主要画面

| パス | 画面 |
| --- | --- |
| `/notes` | ノート一覧。フリーワード、日付、タグ、復習対象で検索 |
| `/notes/new` | Canvas 本文、Markdown の Cue / Summary を使うノート作成 |
| `/notes/[id]` | Canvas または既存 Markdown 本文の詳細。閲覧、編集、復習モードを切り替え |
| `/backup` | SQLite DB バックアップの作成と一覧確認 |

## MVP 受け入れ材料

受け入れ証跡の正本は [受け入れ証跡マトリクス](../product/doc/macos/testing/TEST_SCENARIOS.md#受け入れ証跡マトリクス) です。各記録には、route、画面状態、viewport または API / CLI / 静的照合、確認日、fixture の扱い、判定、参照 product summary / 根拠を記載しています。

2026-07-05 時点の MVP 主要 UI フローは Playwright Chromium で検証済みです。確認結果は `../product/summary/macos/20260705/mvp-ui-flow-reverification-report.md` を参照してください。この PASS の対象は当時の route-level flow であり、Canvas 導入後の現行 UI に対する Gate 0 の証拠ではありません。NTE-020 の edit レイアウト全 viewport 確認と NTE-030 の mobile runtime 確認も含みません。

実操作デモ（Canvas 導入前の履歴）: [一覧 → 詳細 → 編集 → 保存 → 閲覧 / 再読込（WebM）](../product/doc/macos/assets/demos/mvp-note-flow.webm)（1280 × 900）。

次の画面例は Canvas 導入前の Markdown 本文 UI を含む履歴資料です。現行の新規本文を textarea / preview とする仕様や、現行 Canvas UI の runtime PASS を示すものではありません。

画面例（履歴）:

| 画面 | スクリーンショット |
| --- | --- |
| `/notes`: ノート一覧、検索、日付 / タグフィルタ、範囲 validation | [1440px](../product/doc/macos/assets/screenshots/runtime-notes-list-1440.png) |
| `/notes/new`: Canvas 導入前の新規作成、既存タグ候補選択、自由入力タグ追加 | [375px](../product/doc/macos/assets/screenshots/nte020-policy-c-new-375.png) / [768px](../product/doc/macos/assets/screenshots/nte020-policy-c-new-768.png) / [1280px](../product/doc/macos/assets/screenshots/nte020-policy-c-new-1280.png) / [1440px](../product/doc/macos/assets/screenshots/nte020-policy-c-new-1440.png) |
| `/notes/[id]`: Canvas 導入前の閲覧、編集保存、復習モード、削除 | [閲覧 1440px](../product/doc/macos/assets/screenshots/runtime-note-detail-view-1440.png) / [編集保存 1440px（主要 UI flow）](../product/doc/macos/assets/screenshots/runtime-note-detail-edit-1440.png) / [復習 1440px](../product/doc/macos/assets/screenshots/runtime-note-detail-review-1440.png) |
| `/backup`: バックアップ一覧表示、バックアップ作成 | [1440px](../product/doc/macos/assets/screenshots/runtime-backup-1440.png) |

### NTE-030 runtime screenshot の確認内容（Canvas 導入前の履歴）

当時の 1440px 実画面では、閲覧／復習がタイトル・学習日／学習元／タグのメタ情報 → Cornell（Cue／本文）→ Summary の基本構造を共有することを確認しました。復習時の本文表示 / 再マスクも当時の Markdown 本文 UI に対する記録です。現行の Canvas viewer / editor の受け入れ判定には使いません。

375 / 768px の閲覧 / 復習 runtime は未確認です。現行コードでは、復習時の本文・Summary 内容の初期非表示と、本文表示後だけ Summary を開ける制御を静的に確認していますが、Browser runtime では未確認です。詳細は証跡マトリクスの `NTE030-MOBILE-375-768` と `MVP-REVIEW-SCREEN-DEFAULT-001` を参照してください。

### QA 証跡の確認済み範囲と未確認範囲

- 確認済み（Canvas 導入前の履歴）: 2026-07-05 の主要 UI flow、Notes CRUD / validation / review / search、Markdown sanitize / checkbox、`npm run backup:copy`。NTE-020 の `/notes/new` は 375 / 768 / 1280 / 1440px、NTE-030 の `/notes/[id]` 閲覧 / 復習は 1440px を確認済みです。判定値は履歴として保持し、現行 Canvas UI の Gate 0 判定へ読み替えません。
- 未確認（Canvas 導入前の観点を含む履歴）: NTE-020 の `/notes/[id]` edit runtime、NTE-020 の 375px 長い Markdown / 長いタグ / 長い field error、NTE-030 の 375 / 768px 閲覧 / 復習 runtime。2026-07-05 の編集保存フローと、NTE-020 Policy C の edit レイアウト QA は別の確認単位です。
- 過去の静的差分（2026-07-16 時点の履歴）: 当時の静的照合では、新規 `nextReviewDate = noteDate + 7日` 初期値と復習時 Summary の初期非表示を `FAIL（静的照合）` と記録しました。`MVP-GAP-001` と `MVP-GAP-002` の判定値は履歴として保持しますが、現行契約との差分を示すものではありません。
- 現行コードの静的確認: 復習開始時の本文・Summary 内容の初期非表示、本文表示後の Summary 開示、復習開始時点の `Asia/Tokyo` の現在日付 + 7日の初期値、保存済み `nextReviewDate` の非再利用、変更・クリア、成功応答値の反映を確認しています。
- 現行 Browser runtime: 最新 QA は Browser backend を利用できず未実施です。静的確認と過去の screenshot を現行 runtime PASS に読み替えず、Gate 0 は未通過のままです。
- Phase 2: autosave、Undo / soft delete、専用復習タスク、NoteCard / D&D、PDF export、タグ管理 UI などは MVP の PASS に含めず、`TEST_SCENARIOS.md` の Phase 2 節で管理します。

### NTE-020 方針Cの実画面確認（Canvas 導入前の履歴）

NTE-020 方針Cの旧 Markdown 新規ノート作成画面を確認したスクリーンショットです。現行の Canvas editor / viewer 仕様や Gate 0 の証拠には使いません。`/notes/[id]` の編集画面の確認結果も含みません。

| Viewport | 確認内容 | スクリーンショット |
| --- | --- | --- |
| 375px | Cornell部分のみ横スクロールを許容。 | ![NTE-020 方針C 新規作成 375px](../product/doc/macos/assets/screenshots/nte020-policy-c-new-375.png) |
| 768px | タブレット幅での新規作成画面。 | ![NTE-020 方針C 新規作成 768px](../product/doc/macos/assets/screenshots/nte020-policy-c-new-768.png) |
| 1280px | Cue / Note 約30% / 70%、旧本文 textarea と Markdown Preview の横並び。 | ![NTE-020 方針C 新規作成 1280px](../product/doc/macos/assets/screenshots/nte020-policy-c-new-1280.png) |
| 1440px | Cue / Note 約30% / 70%、旧本文 textarea と Markdown Preview の横並び。 | ![NTE-020 方針C 新規作成 1440px](../product/doc/macos/assets/screenshots/nte020-policy-c-new-1440.png) |

過去の画面資料を再現する必要がある場合は、開発サーバーを起動してから主要画面を開き、画像を product 側の `doc/macos/assets/screenshots/` 配下へ保存してください。

この手順は、旧画面資料の再現方法を残したものです。

現行 Gate 0 の再開指示ではありません。

```bash
npm run dev -- -H 127.0.0.1 -p 3000
```

撮影対象:

- `http://127.0.0.1:3000/notes`
- `http://127.0.0.1:3000/notes/new`
- `http://127.0.0.1:3000/notes/[id]`
- `http://127.0.0.1:3000/backup`

`/notes/[id]` は既存データが必要です。空 DB の場合は `/notes/new` から検証用ノートを一時作成し、撮影後に詳細画面の削除操作または `DELETE /api/notes/:id` で削除してください。

## 基本操作

1. `/notes/new` でタイトル、学習日、Markdown の Cue / Summary、Canvas 本文、タグ、次回復習日を入力します。新規ノートの次回復習日は `noteDate + 7日` で始まり、保存前に変更またはクリアできます。
2. 保存すると `/notes/[id]` の詳細画面へ移動します。
3. `/notes` で作成済みノートを検索します。
4. 詳細画面で閲覧、編集、復習モードを切り替えます。
5. 復習モードでは Cue だけを想起情報として先に確認します。本文と Summary の内容は初期非表示で、本文を表示して確認した後に Summary を開きます。本文表示前は Summary を開けません。
6. 復習用の次回復習日は、復習モードへ入った時点の `Asia/Tokyo` の現在日付 + 7日で始まり、保存済み値を再利用しません。復習完了前に変更またはクリアでき、成功後は API 応答値が画面へ反映されます。
7. `/backup` または `npm run backup:copy` で SQLite DB をバックアップします。

## ノートの削除と復元

ノートの削除は詳細画面で確認 UI を表示し、確認後に `DELETE /api/notes/:id` を実行して Notebook を物理削除します。NotebookCanvas、Cue、NotebookTag の関連も cascade で削除されます。MVP では削除後の Undo / 復元を保証しません。

5 秒 Undo Snackbar、ソフトデリート、Undo buffer（`SoftDeleteBuffer`）、`/api/undo`、期限切れ後の purge は現行 MVP には含まれず、Phase 2 以降の機能です。

## データベース

MVP の DB は Prisma + SQLite です。主なモデルは次のとおりです。

- `Notebook`: ノート本体
- `NotebookCanvas`: Canvas 本文の `CanvasDocumentV1` JSON と一覧検索用 `searchText`
- `Cue`: キーワード / 質問
- `Tag`: タグ
- `NotebookTag`: ノートとタグの中間テーブル

新規ノートは `bodyMode=canvas` で作成します。`Notebook.body` は空文字とし、本文の正本を `NotebookCanvas.documentJson` 内の `CanvasDocumentV1` に保存します。`bodyMode=markdown` と `Notebook.body` は既存ノートの互換表示に限り、Canvas へ自動変換しません。Cue と Summary は Markdown のまま編集・保存し、sanitize した preview で表示します。

一覧のフリーワード検索は title、Summary、Cue、legacy Markdown の `Notebook.body`、Canvas text 要素から生成した `NotebookCanvas.searchText` を対象にします。Canvas の用紙寸法だけを変更した場合は text 要素が変わらないため、`searchText` も変えません。

2026-07-18 の `prisma/migrations/20260718011243_remove_notebook_overview/migration.sql` 適用により、Notebook の旧 overview 列は削除済みです。現行のノート項目はタイトル、学習日、学習元、タグ、Cue、Canvas または legacy Markdown 本文、Summary、次回復習日、最終復習日時です。

DB の作成と migration は次で行います。

```bash
npm run prisma:migrate
```

MVP セットアップで seed 実行は必要ありません。空の DB から開始し、検証用データは UI/API 操作で作成します。

Prisma Client の再生成は次で行います。

```bash
npm run prisma:generate
```

## バックアップ

バックアップは SQLite DB ファイルを `backup/` 配下へコピーします。

作成方法:

- 画面から作成: `/backup`
- コマンドで作成: `npm run backup:copy`

`npm run backup:copy` は開始時にプロジェクト直下の `.env` を読み込みます。shell の `DATABASE_URL` は上書きしません。

空・空白・非 `file:` URL・空 path の URL、query / fragment 付き URL、authority 付き `file://` URL はエラーになります。custom path の DB が存在しない場合も同じです。fallback の `dev.db` はコピーしません。

runtime と同様に、percent-encoded path は decode せず、URL の文字列どおりの実ファイル名を対象にします。

仕様:

- バックアップファイルは `backup/` 配下に `.db` として作成されます。
- 最新 3 世代を保持します。
- 4 世代目以降は古いものから削除されます。
- バックアップファイルからの自動復元は MVP 外です。

復元する場合は、アプリを停止してから、バックアップされた `.db` を手動で DB ファイルの場所へ戻します。これはバックアップファイルの手動復旧であり、ノート削除後の Undo / 個別復元機能ではありません。

## 検証

基本的な検証コマンド:

```bash
npm run prisma:generate
npm run prisma:migrate
npm run lint
npm run build
```

主要フローの手動確認:

- `/notes/new` で Canvas 本文と Markdown の Cue / Summary を持つノートを作成できる。
- `/notes` で title、Summary、Cue、legacy Markdown 本文、Canvas `searchText` のフリーワード検索と、日付、タグ、復習対象による絞り込みができる。
- `/notes/[id]` で Canvas viewer または legacy Markdown 互換表示の本文を閲覧できる。
- 詳細画面で Canvas editor または legacy Markdown 互換 UI を使って編集し、保存できる。
- 詳細画面の復習モードで Cue → 本文表示・確認 → Summary 開示の順序を保ち、次回復習日を変更またはクリアして復習済み更新ができる。復習開始時は `Asia/Tokyo` の現在日付 + 7日を初期表示し、成功後は API 応答値を反映する。
- `/backup` または `npm run backup:copy` でバックアップを作成できる。

## 設計書

MVP の仕様と設計は、次の文書を参照してください。

- `../product/doc/common/README.md`
- `../product/doc/common/requirements/PRODUCT_SPEC.md`
- `../product/doc/common/requirements/MVP_SYSTEM_SPEC.md`
- `../product/doc/macos/implementation/MVP_CONTRACT.md`
- `../product/doc/macos/workflows/MVP_WORKFLOW_DESIGN.md`
- `../product/doc/macos/screens/MVP_SCREEN_INVENTORY.md`
- `../product/doc/macos/diagrams/MVP_UML_DESIGN.md`
- `../product/doc/macos/technical/MVP_DESIGN_TOOLING_GUIDE.md`
- `../product/doc/common/design-studio/README.md`
- `../product/doc/macos/api/MVP_API_DESIGN.md`
- `../product/doc/macos/data/MVP_DATA_DESIGN.md`
- `../product/doc/macos/testing/TEST_SCENARIOS.md`

## Design Studio

画面案の作成、比較、実装への受け渡しを Codex 内で行う repo-local plugin とテンプレートです。Google Stitch / Claude Design に似た流れを扱います。

初回のみ Codex CLI で marketplace と plugin を追加します。

```bash
codex plugin marketplace add /Users/kazuya/Desktop/自己学習/Cornell-Method
codex plugin add cornell-design-studio@cornell-method-local
```

運用手順とテンプレートは `../product/doc/common/design-studio/README.md` を参照してください。

## 既知の注意

MVP では次の機能は対象外です。

- 自動保存 / 下書き
- Undo / ソフトデリート復元（Phase 2）
- PDF export
- 専用復習タスク画面
- D&D によるカード並び替え
- バックアップからの自動復元
- 認証、ユーザー管理、共有、外部同期

`NoteCard`、`CueCard`、`NoteCueLink` は現行 MVP に存在しません。Canvas 本文との関係は Phase 2 の未決事項であり、Gate 0 通過後に Canvas 維持、カード併用、カード不採用を比較します。legacy `Notebook.body` の order 0 カード化や Canvas からの自動変換は決定していません。

部分消しゴムは [`product/doc/macos/designs/CANVAS_PARTIAL_ERASER_DESIGN.md`](../product/doc/macos/designs/CANVAS_PARTIAL_ERASER_DESIGN.md) の将来提案です。

Gate 0 通過後に現行パスと正本の状態を再確認してから、投入可否を判断します。

network restricted 環境でもビルドできるよう、Google Fonts を取得せず system font stack を使います。Next.js 16 の Turbopack build は一部 sandbox の port bind 制限に当たるため、`npm run build` では `next build --webpack` を実行します。

2026-06-21 時点で `npm audit --audit-level=moderate` は moderate 3 件を報告します。対象は `brace-expansion` と Next.js 経由の `postcss` です。依存更新は MVP final verification では実施していません。
