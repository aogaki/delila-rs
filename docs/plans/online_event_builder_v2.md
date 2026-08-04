# Online Event Builder v2 — 再設計

**Status: ❌ SUPERSEDED** by `event_builder_unified.md` (2026-02-26)。さらに 2026-08-04 に EB 自体が C++ 移行決定([TODO 66](../../TODO/event-builder/66_cpp_event_builder.md))
**Date: 2026-02-11**
**Branch: feature/online-eb-v2**

## 背景

v1 (online.rs) は複雑すぎ、パフォーマンスが不十分:
- TimeSortBuffer (BTreeMap) + SliceBuilder + TimeSlice + rayon + spawn_blocking
- 41M hits (37秒分) の処理に数分かかる
- CPU利用率が低く、無駄な処理が多い

legacy/ELIFANT-Event は 2時間のデータを10分で処理できるシンプルな設計。
同じ「チャンク＋ソート＋オーバーラップ」方式で一から作り直す。

## 設計 (Gemini との協議済み)

### 「2つのオーバーラップ」問題

混同してはならない2つの時間制約:

1. **Network Disorder Buffer (30-50ms)**: データの到着順序が保証されない。
   Sorter は `latest_ts - safe_horizon` より古いデータのみ dispatch。
   → 遅延ヒットの取りこぼしを防止。

2. **Coincidence Overlap (100ns)**: チャンク境界でのイベント構築に必要。
   Worker は core 領域のトリガーのみ emit。
   → safe_horizon >> coincidence_window なので自動的に保証される。

### アーキテクチャ

```
[Receiver]      [Sorter]         [Workers]        [Writer]
 tokio task    std::thread     std::thread×N     std::thread
    │               │               │               │
 ZMQ SUB      accumulate       build_events     ROOT file
    │          + sort              │             (rotation)
    │          + safe_cut          │
    ▼               ▼               ▼               ▼
  Vec<Hit> ──→ SortedChunk ──→ Vec<BuiltEvent> ──→ TTree
 (tokio mpsc) (crossbeam)      (crossbeam)
```

### Sorter の核心ロジック

```rust
buffer.sort_unstable_by(|a, b| a.timestamp_ns.total_cmp(&b.timestamp_ns));

let latest_ts = buffer.last().unwrap().timestamp_ns;
let core_end = latest_ts - safe_horizon_ns; // 50ms

let core_end_idx = buffer.partition_point(|h| h.timestamp_ns < core_end);
let retained = buffer[core_end_idx..].to_vec(); // 唯一の clone (~600KB)

let chunk = SortedChunk {
    hits: std::mem::replace(&mut buffer, retained), // O(1) move
    core_end,
};
chunk_tx.send(chunk);
```

### イベントビルドアルゴリズム (legacy と同じ)

1. ソート済みヒットを順にスキャン
2. trigger channel 発見 → ts >= core_end ならスキップ
3. backward scan で prior trigger チェック (pile-up rejection)
4. partition_point で ±coincidence_window 内のヒット範囲を特定
5. coincident hits 内を scan して AC 判定
6. BuiltEvent を emit

### EOS 処理

1. Receiver が EOS 検出 → Sorter に通知
2. Sorter が残り全データを flush (core_end = f64::MAX)
3. Workers に Terminate シグナル (poison pill)
4. 全 Worker 完了後、Writer に CloseFile 送信

## 実装ステップ

### Step 1: chunk_builder.rs — pure イベントビルド関数
- SortedChunk, TriggerConfig 構造体
- build_events_from_chunk(): ソート済みヒット → BuiltEvent
- 単体テスト (trigger, pile-up, boundary, AC)

### Step 2: sort_and_split() — ソート＋Safe Horizon 分割
- sort_unstable_by + total_cmp
- partition_point で safe 境界分割
- 単体テスト

### Step 3: オフライン検証 — ROOT データで比較
- read_hits_from_root → shuffle → sort_and_split → build → 比較

### Step 4: online.rs — パイプライン構築
- Receiver (tokio) → Sorter (std::thread) → Workers (crossbeam) → Writer

### Step 5: 統合テスト
- eb_test_sender → online_event_builder → ROOT files
- dropped_batches = 0, 処理時間 ≤ データ時間幅

## ファイル

| 操作 | ファイル |
|---|---|
| 削除 | online.rs, time_sort.rs, time_slice.rs |
| 新規 | chunk_builder.rs, online.rs (新) |
| 変更なし | hit.rs, built_event.rs, config.rs, root_io.rs, slice_builder.rs, l1_builder.rs |

## 依存追加
- `crossbeam-channel`

## ROOT 出力ベンチマーク結果 (2026-02-25)

oxyroot v0.1.25 のパフォーマンステストを実施。
詳細: [oxyroot_benchmark_results.md](oxyroot_benchmark_results.md)

### 確定事項

- **出力形式:** Vec events + file-per-batch (100k events/file)
- **スループット:** 0.79 M events/s (1 writer), 実運用 300k events/s に対して 2.6x マージン
- **Writers:** 1-2 で十分 (4 は不要)
- **Stop 遅延:** ~127ms (最後のバッチ flush のみ)
- **バッチサイズ感度:** 10k-1M で一定。デフォルト 100k で問題なし
- **クラッシュ耐性:** raw .delila は常に保存、オフライン EB で再現可能のため問題なし

### 却下した代替案

- ROOT クレート自作 → 非現実的
- C++ ROOT FFI → CMake 地獄、oxyroot で十分
- Hit-per-row flat → Vec events より遅い (0.44 vs 0.79 M events/s)

## Gemini 合意事項
- sort_unstable_by + total_cmp で十分
- Safe Horizon = 50ms
- Single Writer (ROOT TTree 非スレッドセーフ) → **1-2 Writers に変更 (bench結果)**
- Worker × 4
- メモリリサイクルは将来最適化
- f64 timestamp は ~104日まで精度保証
