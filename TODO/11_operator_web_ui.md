# Operator Web UI Implementation

**Status: In Progress** (2026-01-16)

**設計ドキュメント**: `docs/architecture/operator_web_ui.md`

## Phase 1: Angular プロジェクトセットアップ ✅

- [x] Angular CLI で `web/operator-ui/` にプロジェクト作成
- [x] Angular Material インストール・設定
- [x] 基本レイアウト作成（2カラム）

## Phase 2: サービス層 ✅

- [x] `operator.service.ts` - HTTP クライアント実装
  - `getStatus()` - 1秒ポーリング
  - `configure()`, `start(runNumber)`, `stop()`, `reset()`
  - ~~`arm()`~~ 削除 - Start時にバックエンドが自動実行
- [x] `timer.service.ts` - タイマーロジック
  - カウントダウン
  - アラーム通知
  - 自動Stop連携

## Phase 3: コンポーネント実装 ✅

- [x] `status-panel` - コンポーネント状態表示
  - 状態に応じた色分け（緑/黄/赤/灰）
  - エラー時ホバーで詳細表示
  - オンライン/オフライン表示
- [x] `control-panel` - コントロールボタン
  - 状態に応じたボタン有効/無効
  - 厳密な状態遷移検証
  - **Armボタン削除** - Startから自動実行
- [x] `run-config` - Run設定入力（control-panelに統合）
  - Exp名、Run番号、コメント
  - 自動インクリメント チェックボックス
- [x] `run-info` - 現在のRun情報
  - Run番号、開始時刻、経過時間
  - イベント数、レート（API連携は将来実装）
- [x] `timer` - タイマー機能
  - 分単位入力
  - 自動Stop チェックボックス
  - アラーム（音 + ダイアログ）
  - "Start with Timer" ボタン

## Phase 4: Rust 側変更 ⏳

- [ ] `rust-embed` クレートで静的ファイル埋め込み
- [ ] `--static-dir` オプション追加
- [ ] ルーティング設定（`/` で UI、`/api/*` で API）

## Phase 5: 統合テスト ✅ (基本動作確認済み)

- [x] DAQ起動 → UI から Configure/Start/Stop
- [x] タイマー自動Stop テスト
- [ ] エラー表示テスト

## Phase 6 (将来): MongoDB 連携

- [ ] MongoDB Docker セットアップ
- [ ] Rust 側 MongoDB クライアント追加
- [ ] `/api/runs` エンドポイント追加
- [ ] Angular 側ラン履歴表示コンポーネント

---

## Implementation Summary (2026-01-16)

### 主要な設計決定

1. **Armボタン削除**: UIからArmボタンを削除。StartボタンがConfigured状態から直接実行可能。
   バックエンドが自動的にArmを実行してからStartを行う。

2. **Start時のRun番号渡し**: ConfigureではなくStart APIでrun_numberを送信。
   - 理由: Configureはハードウェア設定を含み時間がかかる
   - Legacy APIの `/DELILA/start/{runNo}` と同様の設計
   - これにより、Runごとに再Configureなしでrun_numberを変更可能

### 変更ファイル

**Rust (Backend)**:
- `src/common/command.rs` - `Command::Start { run_number: u32 }` に変更
- `src/common/state.rs` - `on_start(run_number)` にシグネチャ変更、Start時にrun_config更新
- `src/recorder/mod.rs` - Start時にrun_numberを受け取り、run_configを更新
- `src/merger/mod.rs` - `on_start` シグネチャ更新
- `src/monitor/mod.rs` - `on_start` シグネチャ更新
- `src/operator/mod.rs` - `StartRequest` 構造体追加
- `src/operator/routes.rs` - `/api/start` エンドポイントでrun_number受け取り
- `src/operator/client.rs` - `start()`, `start_all()`, `start_all_sync()` にrun_number引数追加
- `src/bin/controller.rs` - CLIで `--run` 引数を必須に

**Angular (Frontend)**:
- `operator.service.ts` - `start(runNumber)` メソッドでrun_number送信
- `control-panel.component.ts` - Armボタン削除、Start時にrunNumber渡し
- `app.ts` - タイマー開始時にrunNumber渡し
- `types.ts` - ButtonStatesからarm削除

## 参考

- Legacy UI: `legacy/DELILA-Controller/`
- Legacy API: `legacy/DELILA-WebAPI/`
- 設計: `docs/architecture/operator_web_ui.md`
