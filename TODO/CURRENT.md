# Current Sprint - TODO Index

**Updated:** 2026-01-14

このファイルは現在のスプリントの概要を示すインデックスです。
Claudeセッション開始時に必ず読み込まれます。

---

## Active Tasks

| Priority | File | Status | Summary |
|----------|------|--------|---------|
| 1 | [09_timestamp_sorting_design.md](09_timestamp_sorting_design.md) | **Phase 1完了** | タイムスタンプソートとファイル書き出し |
| 2 | [07_digitizer_config_design.md](07_digitizer_config_design.md) | 作業中 | デジタイザ設定のWeb UI設計 |
| 3 | [08_monitor_component.md](08_monitor_component.md) | **完了** | Monitorコンポーネント実装 |

---

## Current Focus: Recorder Enhancement

### Phase 1: SortingBuffer実装 ✅ 完了 (2026-01-14)
- [x] `SortingBuffer` struct を `src/recorder/mod.rs` に追加
- [x] 5%末尾マージン戦略
- [x] Recorderをlock-freeタスク分離アーキテクチャに修正
  - Receiver task: ZMQ SUB → mpsc channel (non-blocking)
  - Sorter task: バッファリング + ソート
  - Writer task: File I/O (fsync)
- [x] fsync設定 (`fsync_interval_batches`, デフォルト0=HDD向け)
- [x] CLI引数 `--fsync` 追加

### Phase 2: ファイルヘッダー/フッター (次のタスク)
- MsgPack形式のメタデータ
- xxHash64チェックサム

### Phase 3: 高度な機能 (将来)
- クラッシュリカバリ
- イベント数ベースローテーション

---

## Design Decisions Made

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Sorting location | Recorder | Mergerは透過的転送に専念 |
| Margin ratio | 5% | 50Mイベント中2.5M、十分な余裕 |
| Header format | MsgPack | データ本体と一貫性 |
| Checksum | xxHash64 | CRC64より高速、十分な衝突耐性 |
| fsync interval | 5 batches | NVMe: 37.6M evt/s、HDD: 8.1M evt/s |

---

## Completed (Ready for Archive)

- [x] **07_refactoring_plan.md** - Phase 1完了、残りは保留
- [x] **08_monitor_component.md** - 全機能実装完了
- [x] **10_zero_copy_merger.md** - ゼロコピー実装、2MHz+でドロップなし達成

---

## Notes

- **MVP目標:** 2026年3月中旬
- **現在のフェーズ:** Phase 1 (Emulator + ZMQ + Web Monitor) ほぼ完了
- **次のフェーズ:** Phase 2 (CAEN Driver) + Phase 3 (File Writer)
