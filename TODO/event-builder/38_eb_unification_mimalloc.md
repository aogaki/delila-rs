# Event Builder 統一パイプライン

**Created:** 2026-02-23
**Updated:** 2026-08-04
**Status: 🧊 FROZEN — Phase 0-4 完了(Phase 4 = Online EB 移行、2026-05-25)。Phase 5(レガシー削除)/ Phase 6(Operator 統合拡張)は中止**: EB は C++ (root_sink 系譜) へ移行決定([TODO 66](66_cpp_event_builder.md)、2026-08-04)。Rust 統一パイプラインは参照実装として凍結(テストは維持)。凍結時点の既知の制約 = oxyroot 書き込み圧縮未実装(ROOT 出力は常に非圧縮)
**Priority:** — (凍結)
**Plan file:** [`docs/plans/event_builder_unified.md`](../../docs/plans/event_builder_unified.md) (Gemini 2回レビュー済)

## Context

Online EB (`online.rs`) と Offline EB (`slice_builder.rs` + `bin/event_builder.rs`) が別実装。
**目標:** 「オンラインとオフラインで完全に同じ処理」— `chunk_builder` をコアエンジンとし、
`HitSource` trait で入力ソースだけを差し替える統一パイプラインを構築する。

**Supersedes:** `docs/plans/online_event_builder_v2.md`

---

## Phase 0: Time Calibration ヒストグラム出力 — ✅ 完了 (2026-02-26)

- `root_io.rs`: `write_time_histograms_to_root()` 追加 (TTree: Module, Channel, BinCenters, Counts, Entries, PeakPosition)
- `bin/event_builder.rs`: `--hist-output` CLI オプション (デフォルト: `timeAlignment.root`)
- `macros/plot_time_alignment.C`: ROOT マクロ (2D heatmap + 1D projections)
- テスト: `test_write_time_histograms` (feature="root")

## Phase 1: HitSource trait + DelilaFileHitSource + TriggerConfig — ✅ 完了 (2026-02-26)

**新規:** `src/event_builder/source.rs`
- `HitBatch` enum: `Hits(Vec<Hit>)` / `Eos`
- `SourceError` enum: `Timeout` / `Disconnected`
- `HitSource` trait: `next_batch(timeout)` + `name()`
- `DelilaFileHitSource`: .delila → `DataFileReader` → `Hit::from_event_data()` バッチストリーミング
- `RootFileHitSource` (feature="root"): oxyroot で 1 ファイルずつロード → batch_size スライス

**変更:** `src/event_builder/chunk_builder.rs`
- `TriggerConfig::from_channel_config(&ChannelConfig, f64) -> Self` (pure 関数)

**テスト:** 12 tests (3 basic, 2 ROOT, 2 delila integration, 3 TriggerConfig, 2 error)

## Phase 2: EventBuilderPipeline 抽出 — ✅ 完了 (2026-02-26)

**新規:** `src/event_builder/pipeline.rs`
- `PipelineConfig`, `PipelineStats`, `EventBuilderPipeline`
- `sorter_thread()`: HitSource polling (100ms) + time calibration 適用 + sort_and_split/flush
- `worker_thread()`: build_events_from_chunk + atomic event ID
- `writer_thread()`: ROOT file rotation (events_per_file ごと)
- crossbeam bounded(16) Sorter→Workers, bounded(64) Workers→Writers

**テスト:** 2 integration tests (empty source, event building)

## Phase 3: Offline CLI 書き直し — ✅ 完了 (2026-02-26)

**変更:** `src/bin/event_builder.rs`
- `Build` サブコマンド: `.delila` ファイル直接入力 (ROOT 変換不要)
- 削除: `--slice-duration`, `--tree-name`, `--max-hits`
- 追加: `--run-id`, `--workers`, `--writers`, `--events-per-file`
- 出力: ディレクトリ指定 + ファイルローテーション
- `run_event_building()` → `DelilaFileHitSource` + `EventBuilderPipeline`

**検証:** 489 tests pass, clippy clean

---

## Phase 4: Online EB 移行 — 📋 未着手

**変更予定:** `src/event_builder/online.rs`
- `ZmqHitSource`: crossbeam::unbounded receiver ラップ + `impl HitSource`
- async→sync ブリッジ: crossbeam unbounded (tokio 内から send がブロックしない)
- `OnlineEventBuilder::run()` → `ZmqHitSource` + `EventBuilderPipeline::run()` (spawn_blocking)
- `receiver_task()` 維持 (ZMQ 固有)、`sorter_thread/worker_thread/writer_thread` 削除

**注意:** Gemini 指摘 — crossbeam bounded は tokio worker thread をブロック → unbounded 必須

## Phase 5: レガシーコード削除 — 📋 未着手

**削除対象:**
- `src/event_builder/slice_builder.rs` (482 lines)
- `src/event_builder/time_slice.rs` (324 lines)
- `src/event_builder/l1_builder.rs`
- `src/event_builder/time_sort.rs`

**mod.rs 更新:** `pub use SliceBuilder/L1Builder` 削除

**検証:**
```bash
cargo fmt && cargo clippy -- -D warnings && cargo test
cargo test --features root
cargo build --release --features root --bin event_builder
```

---

## Phase 6: Operator 統合 (別タスク — MVP 後でも可)

- TOML `[event_builder]` セクション追加
- Operator に EB コンポーネント登録 (Configure/Start/Stop)
- REST API: `/api/event_builder/status`

---

## mimalloc 導入 — 保留

Phase 1-5 の統一完了後に検討。現時点では chunk_builder 自体のパフォーマンスで十分。

---

## 変更ファイル一覧

| ファイル | 操作 | Phase |
|---------|------|-------|
| `src/event_builder/source.rs` | **新規作成** ✅ | 1 |
| `src/event_builder/pipeline.rs` | **新規作成** ✅ | 2 |
| `macros/plot_time_alignment.C` | **新規作成** ✅ | 0 |
| `src/event_builder/root_io.rs` | 変更 ✅ | 0 |
| `src/event_builder/chunk_builder.rs` | 変更 ✅ | 1 |
| `src/event_builder/mod.rs` | 変更 ✅ | 1, 2 |
| `src/bin/event_builder.rs` | 変更 ✅ | 0, 3 |
| `src/event_builder/online.rs` | 大幅変更 | 4 |
| `src/event_builder/slice_builder.rs` | **削除** | 5 |
| `src/event_builder/time_slice.rs` | **削除** | 5 |
| `src/event_builder/l1_builder.rs` | **削除** | 5 |
| `src/event_builder/time_sort.rs` | **削除** | 5 |

## Gemini レビュー結果 (2026-02-26)

| # | 指摘 | 対応 |
|---|------|------|
| 1 | async→sync ブリッジ: crossbeam bounded は NG | → crossbeam unbounded |
| 2 | RootFileHitSource: 全ファイル一括ロード禁止 | → 1ファイルずつロード |
| 3 | HitSource に `name()` 追加推奨 | → 採用 |
| 4 | メモリ使用量テスト追加 | → 採用 |
| 5 | TriggerConfig edge case (AC module 128, 空設定) | → テスト追加済み |
