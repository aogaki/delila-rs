# oxyroot ROOT 出力ベンチマーク結果と設計判断

**Date:** 2026-02-25
**Benchmark tool:** `src/bin/oxyroot_bench.rs`
**Run with:** `cargo run --release --features root --bin oxyroot_bench`

> **⚠️ 追記 (2026-08-04): oxyroot は ROOT 書き込み圧縮が未実装。**
> `rcompress.rs::compress()` は `assert_eq!(compression, 1)` + 無圧縮パススルー
> (それ以外は `unimplemented!()`、zlib エンコーダはコメントアウトのまま)。
> 0.1.25(最新、2024-10)/GitHub master とも同一で、以後休眠。
> 実測: 同一 LCG データ 2M イベント(スカラー 5 ブランチ、生 28.0 MB)で
> oxyroot **28.1 MB(圧縮率 1.00)** vs C++ ROOT ZSTD-5 **18.1 MB** / zlib-1 18.7 MB。
> oxyroot 出力自体は ROOT で正常に読める(非圧縮なだけ)。
> 本文書のスループット値はこの前提(圧縮コストゼロ)で読むこと。
> この確定が EB の C++ 移行決定([TODO 66](../../TODO/event-builder/66_cpp_event_builder.md))
> の直接の根拠の一つになった。

## 背景

Online Event Builder が ROOT フォーマットでイベントを書き出す際の性能と設計方針を確定するために、
oxyroot クレート (v0.1.25) のパフォーマンステストを実施した。

主な検討事項:
- Stop 時にデータ書き出しで待たされないか
- oxyroot は逐次書き出し (TTree::Fill 相当) をサポートするか
- 書き出しスループットは実運用レートに対して十分か

## oxyroot 内部動作の調査結果

### バスケット自動フラッシュ: あり

oxyroot の `WriterTree::write()` 内部では、ブランチごとに 32KB のバスケットバッファを持ち、
満杯になると即座にディスクにフラッシュされる。データは `write()` 呼び出し中にインクリメンタルに
ディスクへ書かれる。

### 制約: `write()` は1回しか呼べない

`write()` の末尾で `close()` が呼ばれ、ファイルヘッダとメタデータが書き込まれる。
2回目の `close()` は `unimplemented!()` でパニックするため、`TTree::Fill()` のような
エントリ単位の追加書き込みはできない。

### `RootFile` / `WriterTree` は `Send` ではない

内部で `Rc` を使用しているため、スレッド間で渡せない。
書き込みは必ず単一スレッドで完結する必要がある。

## ベンチマーク結果

**環境:** macOS (Apple Silicon), SSD, release build

### 書き出し方式比較

| # | Format | Rate | Throughput | 備考 |
|---|--------|------|-----------|------|
| 1 | **Flat hits** (5 scalar branches, 10M entries) | **2.39 M hits/s** | 32.0 MB/s | 最速。スカラーブランチのみ |
| 2 | **Vec events** (11 branches w/ Vec, 1M entries) | **0.79 M events/s** | 113.4 MB/s | Vec<T> シリアライゼーションあり |
| 3 | Vec events file-per-batch (100k/file) | **0.79 M events/s** | 113.9 MB/s | Bench 2 と同等 |
| 4 | Vec events single file (直接 API) | **0.84 M events/s** | 121.9 MB/s | ベースライン |
| 5 | **Hit-per-row flat** (9 scalar, 3M rows) | **1.32 M hits/s** (0.44 M events/s) | 44.3 MB/s | rows 数が3倍 |
| 6 | Hit-per-row file-per-batch | **1.34 M hits/s** | 44.8 MB/s | Bench 5 と同等 |

### バッチサイズ感度 (Vec events format, 2M events)

| Batch | Files | Time (s) | M events/s | MB/s |
|------:|------:|---------:|-----------:|-----:|
| 10,000 | 200 | 2.59 | 0.77 | 112.8 |
| 50,000 | 40 | 2.55 | 0.78 | 113.3 |
| 100,000 | 20 | 2.53 | 0.79 | 114.2 |
| 500,000 | 4 | 2.57 | 0.78 | 112.5 |
| 1,000,000 | 2 | 2.55 | 0.78 | 113.1 |

**バッチサイズに対してスループットはほぼ一定。** ファイル open/close のオーバーヘッドは無視できる。

## 分析

### Vec events vs Hit-per-row

- Vec events: 1M entries × 11 branches (うち 6 が Vec) → 0.79 M events/s
- Hit-per-row: 3M entries × 9 branches (全スカラー) → 1.32 M rows/s = 0.44 M events/s

**Vec events 形式のほうが高速。** oxyroot はブランチ数 × entries 数にほぼ線形にスケールする。
Vec 形式は entries 数が少ない（イベント数 = hit数/multiplicity）ため、Vec のシリアライゼーション
コストを考慮しても結果的に高速。

### 実運用レートとの比較

実運用条件:
- DAQ: ~3 MHz hit rate (6 modules × 500 kHz/module)
- コインシデンスウィンドウ: 100 ns
- 予想イベントレート: **~300 k events/s** (最もレートの低いトリガーチャンネルに依存)

| 方式 | 必要 (k events/s) | 1 writer (k events/s) | マージン |
|------|-------------------:|----------------------:|---------:|
| Vec events | 300 | 790 | **2.6x** |
| 4 writers 並列 | 300 | 3,160 | **10.5x** |

**1 writer でも十分。4 writers なら余裕。**

### Stop 時の遅延

File-per-batch (100k events/file) の場合:
- Stop 時に残るのは最大 100k events 未満
- 100k events の書き出し: ~127 ms
- **Stop は一瞬で完了する**

## 設計判断

### 確定: Vec events + file-per-batch

```
Online EB Pipeline:

Merger ZMQ → Sorter (Safe Horizon 50ms) → Workers (Event Build) → Writers (oxyroot)
                                                                     ↓
                                                    eb_run0001_0000_events.root (100k events)
                                                    eb_run0001_0001_events.root (100k events)
                                                    ...
                                                    eb_run0001_NNNN_events.root (残り)
```

- **出力形式:** BuiltEvent with Vec branches (EventID, TriggerTime, Mod[], Ch[], Energy[], ...)
- **バッチサイズ:** 100,000 events/file (デフォルト、設定可能)
- **Writers:** 1-2 で十分 (300k/s に対して 0.79 M/s per writer)
- **Run 後の統合:** `hadd` で 1 ファイルに結合 (オプション)

### クラッシュ耐性について

- oxyroot のファイルは `close()` まで有効な ROOT ファイルにならない
- **問題ない**: raw データは `.delila` 形式で常に保存される
- オフライン EB で同一結果を再現可能（オンラインとオフラインは同じアルゴリズム）
- 最悪でも失われるのは最後の 1 バッチ (100k events, ~333ms 分)

### 代替案の却下理由

| 案 | 却下理由 |
|----|---------|
| ROOT クレート自作 | 非現実的。ROOT フォーマットは複雑すぎる (TKey/TDirectory/TBasket/StreamerInfo) |
| C++ ROOT via FFI | CMake 依存管理が Cargo エコシステムと相性が悪い。oxyroot で十分 |
| Hit-per-row flat format | Vec events より遅い (0.44 vs 0.79 M events/s) |
| blocking channel iterator | メリットなし。file-per-batch と同等速度で、クラッシュ耐性が低い |

## ベンチマーク再現手順

```bash
# ビルド
cargo build --release --features root --bin oxyroot_bench

# 実行
cargo run --release --features root --bin oxyroot_bench

# Linux マシンでの実行 (SSD 速度が異なるため再測定推奨)
ssh daq@172.18.4.76 'cd ~/delila-rs && cargo run --release --features root --bin oxyroot_bench'
```

## 関連ドキュメント

- [Online Event Builder v2 設計書](online_event_builder_v2.md)
- [Event Builder 仕様](../../TODO/event-builder/SPECIFICATION.md)
- [Event Bridge Wire Format](../event_bridge_wire_format.md)
