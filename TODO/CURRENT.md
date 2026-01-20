# Current Sprint - TODO Index

**Updated:** 2026-01-20

このファイルは現在のスプリントの概要を示すインデックスです。
Claudeセッション開始時に必ず読み込まれます。

---

## Active Tasks

| Priority | File | Status | Summary |
|----------|------|--------|---------|
| 1 | [11_operator_web_ui.md](11_operator_web_ui.md) | **In Progress** | Operator Web UI (Angular + Material) |
| 2 | [09_timestamp_sorting_design.md](09_timestamp_sorting_design.md) | **Phase 3完了** | タイムスタンプソートとファイル書き出し |

---

## Current Status: Web UI Sprint (2026-01-20)

### Recently Completed
- **Refactoring Plan** ✅ (2026-01-19) → `archive/phase1_components/`
  - Phase 1: CLIパーサー統合 (clap) - 7バイナリ移行、24テスト
  - Phase 2: 統一メトリクスフレームワーク - metrics.rs、10テスト
  - Phase 3: エラー型統合 - error.rs、6テスト
  - Phase 4: 設定構造体共通化 - KISS原則により見送り
  - Phase 5: シャットダウン機構統一 - shutdown.rs、5バイナリ移行

---

## Web UI Status (2026-01-20)

### Recently Completed
- **Phase 9: 同一pipeline_order並列実行** ✅ (2026-01-20)
  - 同じorderのコンポーネントを`join_all`で並列実行
  - Configure, Arm, Start, Stop すべてに適用
- **Phase 8: Pipeline順序制御** ✅ (2026-01-19)
  - sequential start（downstream first）
  - ZMQバッファドレイン修正
  - メモリ爆発問題解決
- **Phase 7: Run履歴・Comment永続化** ✅ (2026-01-19)
  - MongoDB統合（run history）
  - Comment auto-fill（last run → next run）
  - Run Notes（logbook機能）
  - ブラウザリロード時のcomment復元
- **Phase 6: Waveformタブ** ✅ (2026-01-19)
  - 波形表示コンポーネント（ECharts）
  - 複数チャンネル選択、Analog Probe 1/2 トグル
  - Shift+ホイール: X軸ズーム、Ctrl+ホイール: Y軸ズーム
  - Y軸固定範囲（±20000 ADC）
- **Phase 5: グリッド画像保存機能** ✅ (2026-01-19)
- **Phase 4: Gaussian Fitting** ✅ (2026-01-16)

### In Progress
- **Operator Web UI** (Angular + Material Design)
  - DAQ制御フロントエンド
  - 設計ドキュメント: `docs/architecture/operator_web_ui.md`
  - **実装済み:**
    - Monitorサブタブ（検出器ごとに設定を分離）
    - ヒストグラムグリッド（NxM、範囲保持）
    - ガウスフィッティング（JavaScript実装）✅
    - localStorage永続化
    - Waveformタブ（波形表示）✅
    - Run履歴・Comment永続化（MongoDB）✅
    - Pipeline順序制御（sequential start）✅
    - 同一pipeline_order並列実行 ✅

### Completed Features
- Emulator + Reader (CAEN FFI) + ZMQ pipeline
- Merger (zero-copy forwarding)
- Recorder (sorting + file format v2)
- Monitor (Web UI + REST API)
- Operator (control system + pipeline ordering)
- Digitizer configuration REST API
- `delila-recover` CLIツール (クラッシュリカバリ)
- EOS (End Of Stream) ベースの停止制御
- **EventData統一** (MinimalEventData廃止、Option<Waveform>対応)
- **MongoDB統合** (Run履歴、Comment永続化、Notes logbook)
- **Sequential Start** (downstream first、メモリ爆発防止)

---

## Design Decisions Made

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Sorting location | Recorder | Mergerは透過的転送に専念 |
| Margin ratio | 5% | 50Mイベント中2.5M、十分な余裕 |
| Header format | MsgPack | データ本体と一貫性 |
| Checksum | xxHash64 | CRC64より高速、十分な衝突耐性 |
| Intermediate fsync | 削除 | バッチ単位書き込みでは効果なし |
| Channel type | unbounded | データ欠損よりメモリ使用を優先 |
| Start/Stop order | pipeline_order | 上流から停止、下流から開始 |
| Sequential start | wait for Running | メモリ爆発防止、downstream first |
| Same-order parallel | join_all | 同一pipeline_orderは並列実行で高速化 |
| Chart library | ECharts | dataZoom、高パフォーマンス |
| Fitting | JavaScript (LM) | 4096bins/6params は数十ms |
| Fit UI | Hybrid (grid+modal) | サマリー表示+拡大モードで精密操作 |
| Monitor subtabs | Nested tabs | 検出器ごとに設定を分離 |
| State persistence | localStorage | ページリロードでも復元 |
| Run history | MongoDB | マルチクライアント同期、永続化 |
| Note timestamp | UNIX timestamp (i64) | BSONシンプル化、クエリ容易 |

---

## Archived

以下のタスクは `TODO/archive/phase1_components/` に移動済み:
- 06_caen_driver_design.md - CAEN FFIドライバ実装
- 07_digitizer_config_design.md - デジタイザ設定REST API
- 07_refactoring_plan.md - リファクタリング計画（旧）
- 08_monitor_component.md - Monitorコンポーネント
- 10_zero_copy_merger.md - ゼロコピーMerger
- **12_refactoring_plan.md** - コードベースリファクタリング（CLI, Metrics, Error, Shutdown）

---

## Notes

- **MVP目標:** 2026年3月中旬
- **現在のフェーズ:** Phase 1完了、Phase 2 (CAEN Driver) 実装済み
- **次のフェーズ:** Phase 3 (File Writer高度機能) + Phase 4 (Web UI拡充)
