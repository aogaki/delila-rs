# データ出力検証 (Task B)

**Created:** 2026-01-26
**Status: COMPLETED** (2026-01-26)
**Priority:** High — 実験データの信頼性に直結

---

## 目的

Recorder が書き出した .delila ファイルが正しく読み戻せること、
および ROOT で解析できることを検証する。

---

## Step 1: E2E テスト (`tests/file_format_test.rs`)

チェックサム付きイベントを生成し、ファイル書き込み → 読み戻し → 全フィールド検証。
全フィールド値はシード付き乱数 (`StdRng::seed_from_u64()`) で生成し、再現性を確保。

### チェックサム方式

`flags` (u64) にチェックサムを埋め込み、`energy_short` を含む全スカラーフィールドを検証:

```rust
fn compute_checksum(ev: &EventData) -> u64 {
    let ts = ev.timestamp_ns.to_bits();
    (ev.module as u64)
        ^ ((ev.channel as u64) << 8)
        ^ ((ev.energy as u64) << 16)
        ^ ((ev.energy_short as u64) << 32)
        ^ ts
}
```

- **コンテナ:** `flags` (u64) — 全精度でチェックサムを格納
- **検証対象:** `module`, `channel`, `energy`, `energy_short`, `timestamp_ns`
- **ビットシフト:** XOR 衝突を防ぐため、各フィールドを異なるビット位置に配置
- **`to_bits()`:** f64 のビット表現 (u64) を使用し、fine time を含む全精度を検証

### テストケース

| # | テスト名 | 内容 |
|---|----------|------|
| 1 | `test_write_read_roundtrip` | 500イベント (seed=42) の roundtrip + チェックサム全数検証 |
| 2 | `test_checksum_detects_corruption` | バイナリ改ざん → ファイルレベルチェックサム不一致検出 |
| 3 | `test_multiple_batches_roundtrip` | 3バッチ (50+30+20=100イベント, seed=7)・異なるsource_id → 全バッチ読み戻し |
| 4 | `test_footer_statistics` | total_events, timestamp range, is_complete, data_bytes, validate() の正確性 |

### ファイル

- **作成:** `tests/file_format_test.rs`
- **使用API:** `delila_rs::recorder::{FileHeader, FileFooter, DataFileReader, ChecksumCalculator}`
- **使用API:** `delila_rs::common::{EventData, EventDataBatch}`
- **依存:** `rand = "0.8"` (`StdRng`, `SeedableRng`)

---

## Step 2: `delila-recover dump` サブコマンド

.delila ファイルを ROOT が読める flat binary に変換する。

```bash
cargo run --release --bin recover -- dump <file.delila> --output events.bin
```

### 出力フォーマット (flat binary, Little-Endian)

```
Header (16 bytes):
  magic:     "DLDUMP01" (8 bytes)
  n_events:  u64        (8 bytes)

Per event (22 bytes, fixed):
  module:       u8       (1 byte)
  channel:      u8       (1 byte)
  energy:       u16      (2 bytes)
  energy_short: u16      (2 bytes)
  flags:        u64      (8 bytes)
  timestamp_ns: f64      (8 bytes)
```

- 2パス方式: footer からイベント数取得 → flat binary 書き出し
- waveform はスキップ（データ量が膨大）。必要なら `--waveform` オプションで将来対応

### ファイル

- **変更:** `src/bin/recover.rs` — `Dump` サブコマンド追加 (~80行)

---

## Step 3: ROOT マクロ (`macros/read_dump.C`)

flat binary を ROOT TTree に読み込む。
(`macros/read_delila.C` は既に .delila 直接読み用として存在するため、`read_dump.C` として作成)

```bash
root -l 'macros/read_dump.C("events.bin")'
```

### TTree ブランチ (legacy Recorder 互換)

Tree 名: `DELILA_Tree`

| Branch | Type | Dump field |
|--------|------|------------|
| `Mod` | `UChar_t` (b) | module |
| `Ch` | `UChar_t` (b) | channel |
| `TimeStamp` | `ULong64_t` (l) | (uint64_t)timestamp_ns |
| `FineTS` | `Double_t` (D) | timestamp_ns |
| `ChargeLong` | `UShort_t` (s) | energy |
| `ChargeShort` | `UShort_t` (s) | energy_short |
| `RecordLength` | `UInt_t` (i) | 0 (waveform なし) |

### ファイル

- **作成:** `macros/read_dump.C` (~90行)

---

## 検証結果 (2026-01-26)

### 自動テスト

```
$ cargo test --test file_format_test
running 4 tests
test test_checksum_detects_corruption ... ok
test test_footer_statistics ... ok
test test_multiple_batches_roundtrip ... ok
test test_write_read_roundtrip ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

### DAQ integration test + recover 検証

```
$ # Emulator DAQ run (3秒)
$ recover validate ./data/run0001_0000_TestRun.delila
  Valid: true, Recoverable blocks: 11340, Recoverable events: 56,700,000

$ recover validate ./data/run0001_0001_TestRun.delila
  Valid: true, Recoverable blocks: 572, Recoverable events: 2,860,000

$ recover dump ./data/run0001_0001_TestRun.delila --output /tmp/events.bin
  Events written: 2,860,000
  Output size: 62,920,016 bytes (= 16 + 2,860,000 × 22 ✓)
```

### 検証の2層構造

| レイヤ | 方法 | 検証内容 |
|--------|------|----------|
| **ファイル整合性** | `recover validate` (xxHash64) | ディスク上のバイナリが書き込み時と一致 |
| **フィールド正確性** | E2E テスト (per-event XOR checksum) | write → read roundtrip で全フィールドが保存 |

### ROOT での読み込み

```bash
root -l 'macros/read_dump.C("/tmp/events.bin")'
# tree->Draw("ChargeLong");
# tree->Draw("ChargeLong:Ch","","colz");
```

---

## 変更ファイル一覧

| Action | File | Description |
|--------|------|-------------|
| Create | `tests/file_format_test.rs` | E2E テスト (4テスト, シード付き乱数) |
| Modify | `src/bin/recover.rs` | `Dump` サブコマンド追加 |
| Create | `macros/read_dump.C` | ROOT マクロ (legacy Recorder 互換 TTree) |
