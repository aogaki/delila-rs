# タイムスタンプソートとファイル書き出し設計

## 現状の問題

1. **チャンネルごとのデータ順序**: CAENデジタイザはチャンネルごとにデータを返す
   - 同一チャンネル内は時系列順
   - 複数チャンネル/モジュールをまとめると時間順がぐちゃぐちゃ

2. **バッチ境界問題**: ファイルローテーション時に
   - 前のファイルの末尾と次のファイルの先頭でタイムスタンプ齟齬が発生
   - バッファ内の「古いイベント」が新しいファイルに書かれる

## 提案設計

### 1. ソートの実装場所

**選択肢:**

| 場所 | メリット | デメリット |
|------|----------|------------|
| **A. Recorder** | シンプル、他コンポーネント影響なし | Recorderのメモリ使用量増加 |
| **B. Merger** | 下流全体がソート済みデータ受信 | Mergerが複雑化、レイテンシ増加 |
| **C. 後処理ツール** | オンライン処理に影響なし | オフライン処理必須 |

**推奨: A. Recorder**
- Monitorはリアルタイム性重視でソート不要
- Recorderのみがファイル書き出しでソートが必要
- Mergerは「透過的なデータ転送」に専念すべき

### 2. バッファマージンによるソート戦略

```
時間軸 ─────────────────────────────────────────────────────────────►

バッファ内イベント（ソート後）:
├─────────────────────────────────────────────────────────────────────┤
│              書き出し対象 (95%)              │   5% マージン      │
│              ← 確定したイベント →            │   (次回に持ち越し) │
├──────────────────────────────────────────────┼────────────────────┤
│  t_min                                       │  t_max - margin    │
│                                              │                    │
└──────────────────────────────────────────────┴────────────────────┘

次のローテーション時:
1. 新しいバッチがバッファに追加される
2. 前回持ち越した5%と合わせて全体をソート
3. また末尾5%を残して書き出し
4. 連続性が保証される
```

**なぜ末尾のみマージンが必要か:**
- 先頭のイベントより古いイベントが後から来る可能性は低い
  （デジタイザのバッファ深さの範囲内で処理されるため）
- マージンが必要なのは「まだ古いイベントが届くかもしれない」末尾側のみ

**パラメータ:**
- `margin_ratio`: 0.05 (5%) - 設定可能
- `min_events_before_flush`: 最低イベント数（マージン計算用）

### 3. 具体的アルゴリズム

```rust
struct SortingBuffer {
    events: Vec<MinimalEventData>,
    margin_ratio: f64,           // 例: 0.05 (5%)
    min_buffer_size: usize,      // 最低バッファサイズ
    min_margin_count: usize,     // 最低マージンイベント数
}

impl SortingBuffer {
    /// バッチを追加
    fn add_batch(&mut self, batch: &MinimalEventDataBatch) {
        self.events.extend(batch.events.iter().cloned());
    }

    /// ソートして書き出し可能なイベントを返す（末尾マージンは保持）
    fn flush(&mut self) -> Vec<MinimalEventData> {
        if self.events.len() < self.min_buffer_size {
            return Vec::new();  // バッファ不足、まだ書き出さない
        }

        // 1. 全体をタイムスタンプでソート
        self.events.sort_by(|a, b|
            a.timestamp_ns.partial_cmp(&b.timestamp_ns).unwrap()
        );

        // 2. 末尾マージン計算
        let margin_count = (self.events.len() as f64 * self.margin_ratio) as usize;
        let margin_count = margin_count.max(self.min_margin_count);

        // 3. 書き出し範囲決定（末尾マージンを除く）
        let write_count = self.events.len().saturating_sub(margin_count);
        if write_count == 0 {
            return Vec::new();
        }

        // 4. 先頭から write_count 個を書き出し用に抽出
        let to_write: Vec<_> = self.events.drain(..write_count).collect();

        // 5. 残り（末尾マージン）は self.events に残る
        //    次回の flush で新しいイベントと合わせてソートされる

        to_write
    }

    /// Run終了時: 全イベントをソートして返す（マージンなし）
    fn flush_all(&mut self) -> Vec<MinimalEventData> {
        self.events.sort_by(|a, b|
            a.timestamp_ns.partial_cmp(&b.timestamp_ns).unwrap()
        );
        std::mem::take(&mut self.events)
    }
}
```

### 4. ファイル書き出しで考慮すべき追加事項

#### 4.1. メタデータヘッダー

各ファイルに以下を記録:

```rust
struct FileHeader {
    // 識別情報
    magic: [u8; 8],           // "DELILA01"
    version: u32,             // フォーマットバージョン

    // Run情報
    run_number: u32,
    exp_name: String,
    file_sequence: u32,

    // 時間情報
    file_start_time: u64,     // Unix time (ns)
    file_end_time: u64,       // 書き出し後に更新
    first_event_time: f64,    // 最初のイベントのtimestamp_ns
    last_event_time: f64,     // 最後のイベントのtimestamp_ns

    // 統計情報
    total_events: u64,
    events_per_channel: HashMap<(u8, u8), u64>,  // (module, channel) -> count

    // ソート情報
    is_sorted: bool,
    sort_margin_ratio: f64,
}
```

#### 4.2. データ整合性

```rust
struct FileFooter {
    // チェックサム
    data_checksum: u64,       // CRC64 or xxHash
    header_checksum: u64,

    // 書き出し完了フラグ
    write_complete: bool,

    // 最終統計
    actual_bytes_written: u64,
    actual_events_written: u64,
}
```

#### 4.3. ファイルフォーマット構造

```
┌─────────────────────────────────────────┐
│  Header (固定長 or 長さプレフィックス)   │
│  - Magic, Version, Metadata             │
├─────────────────────────────────────────┤
│  Data Block 1                           │
│  - Length prefix (u32)                  │
│  - MsgPack serialized batch             │
├─────────────────────────────────────────┤
│  Data Block 2                           │
│  - Length prefix (u32)                  │
│  - MsgPack serialized batch             │
├─────────────────────────────────────────┤
│  ...                                    │
├─────────────────────────────────────────┤
│  Footer (固定長)                        │
│  - Checksums, completion flag           │
└─────────────────────────────────────────┘
```

#### 4.4. クラッシュリカバリ

1. **二重書き込み戦略**:
   - ヘッダーを最初に書き、Footerを最後に書く
   - Footer不在 → 不完全ファイルと判定

2. **WAL (Write-Ahead Log)**:
   - 各バッチ書き込み前にWALに記録
   - クラッシュ後にWALから復旧可能

3. **Atomic rename**:
   - `.tmp`ファイルに書き込み
   - 完了後に正式ファイル名にrename

#### 4.5. ファイルローテーション条件

現状:
- サイズベース (1GB)
- 時間ベース (10分)

追加検討:
- **イベント数ベース**: 一定イベント数ごと（解析しやすい）
- **タイムスタンプ範囲ベース**: 物理時間で区切る（Run開始からの時間）

### 5. 性能考慮事項

#### 5.1. メモリ使用量

```
イベントサイズ: 22 bytes (MinimalEventData)
バッファサイズ: 1,000,000 events → 22 MB
マージン 5%: 50,000 events → 1.1 MB 保持

→ 現実的なメモリ使用量
```

#### 5.2. ソート性能

```rust
// Rustのsort_by: Timsort (O(n log n))
// 1M events @ 1GHz = ~20M comparisons ≈ 20ms
// 十分高速
```

#### 5.3. 書き込み性能

- BufWriter使用（現在64KB）→ 十分
- fsync頻度: バッチごとは不要、ファイルクローズ時のみ
- SSD想定: 500MB/s → 1GBファイル = 2秒

### 6. 推奨実装順序

1. **Phase 1**: SortingBuffer実装
   - 基本的なソートとマージン機能
   - Recorderに統合

2. **Phase 2**: ファイルヘッダー/フッター追加
   - メタデータ記録
   - 整合性チェック

3. **Phase 3**: 高度な機能
   - クラッシュリカバリ
   - イベント数ベースローテーション

## 決定事項

### マージン比率: 5%

1GBファイル ≈ 50Mイベント、5% = 2.5Mイベント。
デジタイザの最小バッファ単位（1024イベント/チャンネル）の約2500倍あり、十分なマージン。

### ファイルヘッダー形式: MsgPack

- データ本体と同じフォーマット（一貫性）
- コンパクト
- 既存の `rmp-serde` をそのまま使用可能

### チェックサム: xxHash64

- CRC64より高速
- 衝突耐性も十分
- Rustでは `xxhash-rust` クレートが利用可能

### fsync戦略: ファイルクローズ時のみ (2026-01-15更新)

**再検討の結果:**

現在のRecorder設計では:
1. SortingBufferが一定量（10,000イベント以上）溜まったらソート
2. ソート済みバッチをWriter taskに送信
3. Writer taskはバッチ単位でシリアライズ + 書き込み

この**バッチ単位の書き込み**では:
- 中間fsyncは「直近1バッチ分の保護」にしかならない
- クラッシュ時に失われるのはSortingBuffer内 + 最新バッチ程度
- fsyncのオーバーヘッドに見合わない

**結論:** 中間fsyncは削除。ファイルクローズ時のみ `sync_data()` を実行。

---

## ストレージベンチマーク結果 (参考: 2026-01-14実施)

**測定条件:**
- バッチサイズ: 1.1MB (50,000イベント × 22バイト)
- 100回反復

### NVMe SSD (/tmp)

| モード | スループット | レイテンシ | 最大レート | 2MHz | 10MHz |
|--------|-------------|-----------|-----------|------|-------|
| fsync なし | 4507.1 MB/s | 0.24 ms | 204.9M evt/s | OK | OK |
| fsync 毎バッチ | 219.4 MB/s | 5.01 ms | 10.0M evt/s | OK | NG |
| fsync 5バッチ毎 | 826.8 MB/s | 1.33 ms | 37.6M evt/s | OK | OK |

### USB HDD (/Volumes/Data20TB)

| モード | スループット | レイテンシ | 最大レート | 2MHz | 10MHz |
|--------|-------------|-----------|-----------|------|-------|
| fsync なし | 297.4 MB/s | 3.70 ms | 13.5M evt/s | OK | OK |
| fsync 毎バッチ | 36.9 MB/s | 29.80 ms | 1.7M evt/s | **NG** | NG |

※ 現在の実装ではfsyncなしモード相当（クローズ時のみsync）

---

## Phase 2 実装完了 (2026-01-14)

### ファイルフォーマット v2 (`*.delila`)

```
┌─────────────────────────────────────────┐
│  Header                                  │
│  - Magic: "DELILA02" (8 bytes)          │
│  - Length prefix: u32 LE (4 bytes)      │
│  - MsgPack: FileHeader struct           │
├─────────────────────────────────────────┤
│  Data Block 1                           │
│  - Length prefix: u32 LE (4 bytes)      │
│  - MsgPack: MinimalEventDataBatch       │
├─────────────────────────────────────────┤
│  Data Block 2 ...                       │
├─────────────────────────────────────────┤
│  Footer (固定 64 bytes)                  │
│  - Magic: "DLEND002" (8 bytes)          │
│  - data_checksum: u64 (xxHash64)        │
│  - total_events: u64                    │
│  - data_bytes: u64                      │
│  - first_event_time_ns: f64             │
│  - last_event_time_ns: f64              │
│  - file_end_time_ns: u64                │
│  - write_complete: u8 (1=complete)      │
│  - reserved: 7 bytes                    │
└─────────────────────────────────────────┘
```

### 実装ファイル

- `src/recorder/format.rs`: FileHeader, FileFooter, ChecksumCalculator
- `src/recorder/mod.rs`: FileWriter更新（ヘッダ/フッタ書き込み）

### FileHeader 構造体

```rust
pub struct FileHeader {
    pub version: u32,              // FORMAT_VERSION = 2
    pub run_number: u32,
    pub exp_name: String,
    pub file_sequence: u32,
    pub file_start_time_ns: u64,   // Unix timestamp (ns)
    pub comment: String,
    pub sort_margin_ratio: f64,
    pub is_sorted: bool,
    pub source_ids: Vec<u32>,
    pub metadata: HashMap<String, String>,
}
```

### FileFooter 構造体 (固定64バイト)

| オフセット | サイズ | フィールド | 説明 |
|-----------|--------|----------|------|
| 0 | 8 | magic | "DLEND002" |
| 8 | 8 | data_checksum | xxHash64 of data blocks |
| 16 | 8 | total_events | イベント総数 |
| 24 | 8 | data_bytes | データブロック総バイト数 |
| 32 | 8 | first_event_time_ns | 最初のイベント時刻 |
| 40 | 8 | last_event_time_ns | 最後のイベント時刻 |
| 48 | 8 | file_end_time_ns | ファイル完了時刻 |
| 56 | 1 | write_complete | 1=正常完了, 0=クラッシュ |
| 57 | 7 | reserved | 将来拡張用 |

### チェックサム計算

```rust
pub struct ChecksumCalculator {
    state: u64,
    bytes_processed: u64,
}
```

- 各データブロック（長さプレフィックス + MsgPackデータ）を更新
- xxHash64のブロックハッシュをXOR + ローテーションで結合
- 最終値 = state ^ bytes_processed

### クラッシュ検出

1. Footerの `write_complete` フラグが0 → 不完全ファイル
2. Footerの magic が不正 → ファイル破損
3. チェックサム不一致 → データ破損

### 今後の拡張

- Phase 3: クラッシュリカバリツール実装
- Phase 3: イベント数ベースローテーション
