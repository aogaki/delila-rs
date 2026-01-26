# PSD1 Decoder Implementation Plan

**Created:** 2026-01-26
**Status: COMPLETED** (2026-01-26)
**Spec:** `docs/psd1_decoder_spec.md`
**Hardware:** DT5730B (Serial: 990, DPP-PSD, USB, 8ch, 14-bit, 500 MS/s)
**DevTree:** `docs/devtree_examples/dt5730b_psd1_sn990.json`

---

## Overview

PSD1 ファームウェア (x725/x730 シリーズ) のデコーダを実装し、既存の Reader フレームワークに統合する。
PSD2 デコーダ (`src/reader/decoder/psd2.rs`) と同じアーキテクチャに従い、共通の `EventData` 出力に変換する。

**Key Differences from PSD2:**
- 32-bit Little-Endian words (vs. 64-bit Big-Endian)
- 階層構造: Board → Dual Channel → Event (vs. flat)
- Start/Stop シグナルなし（Operator コマンドで制御）
- チャンネルはペア単位でグループ化
- `dig1://` URL スキーム

---

## Phase 1: PSD1 デコーダコア実装 ✅ COMPLETED (2026-01-26)

### 1.1 目標
- PSD1 RAW データを `EventData` にデコードできる
- Board Aggregate → Dual Channel → Event の階層解析
- ユニットテスト完備

### 1.2 ファイル

**新規作成:**
- `src/reader/decoder/psd1.rs` — PSD1 デコーダ本体

**変更:**
- `src/reader/decoder/mod.rs` — `psd1` モジュールを公開

### 1.3 構造設計

```rust
// src/reader/decoder/psd1.rs

/// PSD1 decoder configuration
pub struct Psd1Config {
    pub time_step_ns: u64,    // DT5730 = 2
    pub module_id: u8,
    pub dump_enabled: bool,
}

/// PSD1 decoder
pub struct Psd1Decoder {
    config: Psd1Config,
    last_aggregate_counter: u32,
}

impl Psd1Decoder {
    pub fn new(config: Psd1Config) -> Self;
    pub fn with_defaults() -> Self;

    // Public API (same as Psd2Decoder)
    pub fn classify(&self, raw: &RawData) -> DataType;
    pub fn decode(&mut self, raw: &RawData) -> Vec<EventData>;
    pub fn set_dump_enabled(&mut self, enabled: bool);
}
```

### 1.4 内部メソッド

```rust
impl Psd1Decoder {
    // Board level
    fn decode_board_aggregate(&mut self, data: &[u8], offset: &mut usize)
        -> Result<Vec<EventData>, DecodeError>;
    fn decode_board_header(&self, data: &[u8], offset: &mut usize)
        -> Result<BoardHeader, DecodeError>;

    // Channel level
    fn decode_dual_channel_block(&self, data: &[u8], offset: &mut usize,
                                  pair_index: u8)
        -> Result<Vec<EventData>, DecodeError>;
    fn decode_dual_channel_header(&self, data: &[u8], offset: &mut usize)
        -> Result<DualChannelHeader, DecodeError>;

    // Event level
    fn decode_event(&self, data: &[u8], offset: &mut usize,
                    ch_info: &DualChannelHeader, pair_index: u8)
        -> Result<EventData, DecodeError>;
    fn decode_extras_word(&self, word: u32, extra_option: u8)
        -> (u16, u16, u32);  // (extended_time, fine_time, flags)
    fn decode_charge_word(&self, word: u32) -> (u16, u16, bool);
        // (charge_long, charge_short, pileup)
    fn decode_waveform(&self, data: &[u8], offset: &mut usize,
                       ch_info: &DualChannelHeader)
        -> Option<Waveform>;

    // Utility
    fn read_u32(&self, data: &[u8], offset: usize) -> u32;
    fn calculate_timestamp(&self, trigger_time_tag: u32, extended_time: u16,
                           fine_time: u16, extra_option: u8) -> f64;
}
```

### 1.5 内部データ構造

```rust
/// Board Aggregate Header (4 words)
struct BoardHeader {
    aggregate_size: u32,      // [0:27]
    dual_channel_mask: u8,    // [0:7]
    board_id: u8,             // [27:31]
    board_fail: bool,         // [26]
    aggregate_counter: u32,   // [0:22]
    board_time_tag: u32,      // [0:31]
}

/// Dual Channel Header (2 words)
struct DualChannelHeader {
    aggregate_size: u32,      // [0:21]
    num_samples_wave: u16,    // [0:15] (samples/8)
    digital_probe1: u8,       // [16:18]
    digital_probe2: u8,       // [19:21]
    analog_probe: u8,         // [22:23]
    extra_option: u8,         // [24:26]
    samples_enabled: bool,    // [27] ES
    extras_enabled: bool,     // [28] EE
    time_enabled: bool,       // [29] ET
    charge_enabled: bool,     // [30] EQ
    dual_trace: bool,         // [31] DT
}
```

### 1.6 テストケース (TDD)

```rust
#[cfg(test)]
mod tests {
    // --- 基本テスト ---
    fn test_decoder_creation();
    fn test_decoder_with_config();

    // --- classify テスト ---
    fn test_classify_valid_board_header();     // type=0xA → Event
    fn test_classify_invalid_header();          // type≠0xA → Unknown
    fn test_classify_too_small();               // < 16 bytes → Unknown
    fn test_classify_always_event();            // PSD1 has no Start/Stop

    // --- Board Header テスト ---
    fn test_decode_board_header();
    fn test_decode_board_header_insufficient_data();
    fn test_board_aggregate_size();

    // --- Dual Channel Header テスト ---
    fn test_decode_dual_channel_header();
    fn test_dual_channel_enable_flags();
    fn test_dual_channel_extra_option();

    // --- Event テスト ---
    fn test_decode_single_event();              // 最小イベント (ET+EE+EQ)
    fn test_decode_channel_flag();              // even=0, odd=1
    fn test_decode_channel_pair_offset();       // pair*2 + flag
    fn test_decode_timestamp_calculation();     // extended + fine time
    fn test_decode_charge_word();               // long + short + pileup
    fn test_decode_extras_word_option2();       // ext_time + flags + fine
    fn test_decode_extras_word_option0();       // ext_time only

    // --- Waveform テスト ---
    fn test_decode_waveform_basic();
    fn test_decode_waveform_digital_probes();
    fn test_decode_waveform_dual_trace();

    // --- 複合テスト ---
    fn test_decode_multiple_events_in_pair();
    fn test_decode_multiple_channel_pairs();
    fn test_decode_multiple_board_aggregates();
    fn test_events_sorted_by_timestamp();

    // --- ヘルパー関数 ---
    fn make_board_header(size: u32, mask: u8, board_id: u8) -> Vec<u8>;
    fn make_dual_channel_header(size: u32, flags: DualChannelFlags) -> Vec<u8>;
    fn make_event(trigger_time: u32, odd: bool, extras: u32, charge: u32)
        -> Vec<u8>;
}
```

### 1.7 定数モジュール

`psd1.rs` 内に `mod constants` として PSD1 固有の定数を定義する（PSD2 と同じパターン）。

```rust
mod constants {
    pub const WORD_SIZE: usize = 4;  // 32-bit

    pub mod board_header {
        pub const TYPE_SHIFT: u32 = 28;
        pub const TYPE_MASK: u32 = 0xF;
        pub const TYPE_DATA: u32 = 0xA;
        pub const AGGREGATE_SIZE_MASK: u32 = 0x0FFF_FFFF;
        pub const DUAL_CHANNEL_MASK_MASK: u32 = 0xFF;
        pub const BOARD_FAIL_SHIFT: u32 = 26;
        pub const BOARD_ID_SHIFT: u32 = 27;
        pub const BOARD_ID_MASK: u32 = 0x1F;
        pub const COUNTER_MASK: u32 = 0x7F_FFFF;
        pub const HEADER_SIZE_WORDS: usize = 4;
    }

    pub mod channel_header {
        pub const DUAL_CHANNEL_SIZE_MASK: u32 = 0x3F_FFFF;
        pub const NUM_SAMPLES_MASK: u32 = 0xFFFF;
        pub const HEADER_SIZE_WORDS: usize = 2;
        // ... enable flags shifts
    }

    pub mod event {
        pub const TRIGGER_TIME_MASK: u32 = 0x7FFF_FFFF;
        pub const CHANNEL_FLAG_SHIFT: u32 = 31;
        pub const FINE_TIME_MASK: u32 = 0x3FF;
        pub const FLAGS_SHIFT: u32 = 10;
        pub const FLAGS_MASK: u32 = 0x3F;
        pub const EXTENDED_TIME_SHIFT: u32 = 16;
        pub const EXTENDED_TIME_MASK: u32 = 0xFFFF;
        pub const CHARGE_SHORT_MASK: u32 = 0x7FFF;
        pub const PILEUP_SHIFT: u32 = 15;
        pub const CHARGE_LONG_SHIFT: u32 = 16;
        pub const CHARGE_LONG_MASK: u32 = 0xFFFF;
    }

    pub mod waveform {
        pub const ANALOG_SAMPLE_MASK: u32 = 0x3FFF;
        pub const DP1_SHIFT: u32 = 14;
        pub const DP2_SHIFT: u32 = 15;
        pub const SECOND_SAMPLE_SHIFT: u32 = 16;
        pub const SAMPLES_PER_WORD: usize = 2;
        pub const SAMPLES_PER_GROUP: usize = 8;
    }
}
```

---

## Phase 2: Reader 統合 ✅ COMPLETED (2026-01-26)

### 2.1 目標
- `decode_loop()` で PSD1 デコーダを選択できる
- `SourceType::Psd1` → `FirmwareType::Psd1` のマッピング
- config.toml で `type = "psd1"` 指定可能

### 2.2 変更ファイル

- `src/reader/mod.rs`
  - `decode_loop()` の `FirmwareType::Psd1` ブランチを実装
  - `from_config()` で `SourceType::Psd1` → `FirmwareType::Psd1` マッピング
  - `classify` + `decode` は PSD2 と同じインターフェース

### 2.3 実装詳細

```rust
// src/reader/mod.rs - decode_loop() 変更

let mut decoder: Box<dyn Decoder> = match config.firmware {
    FirmwareType::Psd2 => Box::new(Psd2Decoder::new(psd2_config)),
    FirmwareType::Psd1 => Box::new(Psd1Decoder::new(psd1_config)),
    FirmwareType::Pha1 => return Err(...),
};
```

**Alternative (KISS):** trait object を使わず、enum dispatch で実装する方が簡潔。

```rust
enum DecoderKind {
    Psd2(Psd2Decoder),
    Psd1(Psd1Decoder),
}

impl DecoderKind {
    fn classify(&self, raw: &RawData) -> DataType { ... }
    fn decode(&mut self, raw: &RawData) -> Vec<EventData> { ... }
}
```

**推奨:** enum dispatch (KISS 原則)。PSD1/PSD2/PHA1 の 3 種類のみなので trait object は過剰。

### 2.4 Config マッピング

```rust
// src/reader/mod.rs - from_config()
let firmware = match source.source_type {
    SourceType::Psd2 => FirmwareType::Psd2,
    SourceType::Psd1 => FirmwareType::Psd1,
    SourceType::Pha1 => FirmwareType::Pha1,
    SourceType::Emulator => unreachable!(),
};
```

### 2.5 テストケース

```rust
#[tokio::test]
async fn test_psd1_reader_config() {
    // SourceType::Psd1 → FirmwareType::Psd1 マッピング確認
}

#[tokio::test]
async fn test_psd1_decode_loop_creates_decoder() {
    // decode_loop が PSD1 デコーダを正常に作成
}
```

### Phase 1 & 2 Implementation Summary (2026-01-26)

**Files Modified:**
- `src/reader/decoder/psd1.rs` — PSD1 デコーダ本体 (既存、Phase 1 で完成)
- `src/reader/decoder/mod.rs` — `pub mod psd1;` + re-exports (既存)
- `src/reader/mod.rs` — DecoderKind enum, from_config() mapping, decode_loop(), read_loop() PSD1 対応

**Files Created:**
- `config/config_psd1_test.toml` — PSD1 テスト用 DAQ 設定
- `config/digitizers/psd1_test.json` — PSD1 デジタイザパラメータ

**Key Design Decisions:**
1. **DecoderKind enum dispatch** (KISS): trait object ではなく enum で PSD1/PSD2 を切り替え
2. **from_config() mapping**: `SourceType::Psd1` → `FirmwareType::Psd1`, Emulator/Zle は `None` 返却
3. **PSD1 Arm=Start**: `read_loop()` で `FirmwareType::Psd1` の場合は `swstartacquisition` をスキップ

**Test Results:** 269 tests pass, 0 failures, clippy clean

---

## Phase 3: 実機検証

### 3.1 前提条件
- DT5730 USB 接続で動作
- `libCAEN_Dig1.so` がインストール済み (確認済み ✅)
- 30分ライセンスタイムアウトあり（再起動で対応）

### 3.2 検証項目

| # | テスト | 確認内容 |
|---|--------|---------|
| 1 | 接続テスト | `dig1://` で FELib Open 成功 |
| 2 | デバイス情報取得 | モデル名、シリアル番号、FW バージョン |
| 3 | パラメータ読み書き | DevTree 経由で設定可能 |
| 4 | データ読み出し | テストパルスでイベント取得 |
| 5 | デコード検証 | タイムスタンプ、エネルギー、チャンネル |
| 6 | 波形検証 | analog probe + digital probe |
| 7 | パイプライン検証 | Reader → Merger → Recorder → Monitor |
| 8 | ヒストグラム確認 | Web UI でスペクトル表示 |

### 3.3 テスト用 config

```toml
# config/config_psd1_test.toml
[operator]
experiment_name = "PSD1_Test"

[network]
[[network.sources]]
id = 0
name = "psd1-dt5730"
type = "psd1"
bind = "tcp://*:5555"
command = "tcp://*:5560"
digitizer_url = "dig1://caen.internal/usb?link_num=0"
config_file = "config/digitizers/psd1_test.json"
pipeline_order = 1

[network.merger]
subscribe = ["tcp://localhost:5555"]
publish = "tcp://*:5557"
command = "tcp://*:5570"
pipeline_order = 2

[network.recorder]
subscribe = "tcp://localhost:5557"
command = "tcp://*:5580"
output_dir = "./data"
pipeline_order = 3

[network.monitor]
subscribe = "tcp://localhost:5557"
command = "tcp://*:5590"
http_port = 8081
pipeline_order = 3
```

### 3.4 デジタイザ設定テンプレート

```json
// config/digitizers/psd1_test.json
{
  "name": "PSD1 Test",
  "firmware": "PSD1",
  "board": {
    "RecordLengthS": "1024",
    "EventAggregation": "0",
    "AcqMode": "LIST"
  },
  "channel_defaults": {
    "ChEnable": "true",
    "DCOffset": "50",
    "InputPolarity": "Negative",
    "TriggerThr": "100",
    "GateLongLengthS": "50",
    "GateShortLengthS": "12"
  },
  "channel_overrides": {}
}
```

**注意:** PSD1 のパラメータ名は PSD2 と異なる可能性がある。実機接続後に DevTree を取得して確認する。

---

## Implementation Order

```
Phase 1: PSD1 デコーダコア
  ├── 1a. constants モジュール
  ├── 1b. 内部構造体 (BoardHeader, DualChannelHeader)
  ├── 1c. read_u32 + board header decode
  ├── 1d. dual channel header decode
  ├── 1e. event decode (time + extras + charge)
  ├── 1f. waveform decode
  ├── 1g. classify + decode (public API)
  └── 1h. テスト全て pass
          ↓
Phase 2: Reader 統合
  ├── 2a. DecoderKind enum (or trait)
  ├── 2b. decode_loop() PSD1 ブランチ
  ├── 2c. from_config() マッピング
  └── 2d. テスト pass
          ↓
Phase 3: 実機検証
  ├── 3a. dig1:// 接続テスト
  ├── 3b. デバイス情報取得
  ├── 3c. テストパルスデータ取得
  ├── 3d. デコード結果検証
  ├── 3e. パイプライン E2E テスト
  └── 3f. ヒストグラム確認
```

---

## 実機検証結果 (2026-01-26)

### 接続確認 ✅
- URL: `dig1://caen.internal/usb?link_num=0` で接続成功
- `CaenHandle::open()` はそのまま動作（FELib API 共通）
- DevTree 取得成功 (151KB) → `docs/devtree_examples/dt5730b_psd1_sn990.json`

### 重要な発見

**1. コマンドの違い:**
- PSD1 には `SwStartAcquisition` / `SwStopAcquisition` がない
- `ArmAcquisition` で自動開始（`startmode` = `START_MODE_SW` の場合）
- `DisarmAcquisition` で停止
- → Reader の `start()`/`stop()` 実装を PSD1 用に分岐必要 → ✅ 実装済み

**2. パラメータ名の違い:**
- PSD1: `ch_` プレフィックス + アンダースコア区切り（例: `ch_dcoffset`, `ch_threshold`）
- PSD2: プレフィックスなし + 連結（例: `dcoffset`, `triggerthr`）
- → `apply_config()` で PSD1/PSD2 別のパス名マッピングが必要 → ✅ 実装済み

**3. Energy bits:**
- PSD1: `energy_nbit = 15` (charge_short は 15-bit)
- PSD2: `energy_nbit = 16`

**4. Active endpoint:**
- `/endpoint/par/activeendpoint` = `dpppsd` (デフォルト)
- RAW 読み出しには変更が必要かもしれない（要確認）→ ✅ RAW endpoint で正常動作

**5. Endpoint データフォーマット (DIG1 vs DIG2):**
- DIG1: `DATA` + `SIZE` のみ (N_EVENTS なし)
- DIG2: `DATA` + `SIZE` + `N_EVENTS`
- → `configure_endpoint(include_n_events: bool)` で分岐 → ✅ 実装済み

**6. Watch Channel 状態スキップ:**
- Tokio `watch::Receiver` は最新値のみ保持
- 10ms ポーリングで Armed 中間状態をスキップ (Configured→Running)
- → `(_, ComponentState::Running)` パターンで Combined 遷移に対応 → ✅ 実装済み

**7. PSD1 パラメータ値フォーマット:**
- ポラリティ: PSD1=`POLARITY_NEGATIVE`, PSD2=`Negative`
- 有効/無効: PSD1=`TRUE`/`FALSE`, PSD2=`True`/`False` (大文字小文字は許容)
- Extras: ボードレベル=`TRUE`/`FALSE`, チャンネルレベル=`EXTRAS_OPT_*`
- → コード側でポラリティ値マッピング, config 側で正しい値指定 → ✅ 実装済み

### Phase 3 実装サマリ

**Files Modified:**
- `src/reader/caen/handle.rs` — `configure_endpoint(include_n_events: bool)` DIG1/DIG2 分岐
- `src/reader/mod.rs` — Arm/Start FSM: START_MODE_SW 対応, `(_, Running)` パターン
- `src/config/digitizer.rs` — PSD1 ポラリティ値マッピング (`Negative` → `POLARITY_NEGATIVE`)
- `src/bin/caen_info.rs` — `configure_endpoint(true)` 更新
- `tests/felib_integration_test.rs` — `configure_endpoint(true)` 更新
- `config/digitizers/psd1_test.json` — 正しいパラメータ値に修正

**Test Results:** 270 tests pass, 0 failures

**Hardware Verification Results:**
| Test | Result |
|------|--------|
| dig1:// 接続 | ✅ USB 接続成功 |
| Endpoint 設定 | ✅ DATA+SIZE (N_EVENTS なし) |
| パラメータ適用 | ✅ 14/14 成功, 0 エラー |
| START_MODE_SW Arm/Start | ✅ ArmAcquisition で開始 |
| データ読み出し | ✅ ~10,400 evt/s (10kHz パルサー) |
| デコード検証 | ✅ ch4 のみ検出, エネルギー分布正常 |
| パイプライン E2E | ✅ Reader→Merger→Recorder→Monitor 全動作 |
| ヒストグラム表示 | ✅ energy mean=885.4, stdev=8.9, range=[864, 910] |

---

## Risk & Considerations

### dig1:// URL とライブラリ ✅ 解決
- `CaenHandle::open("dig1://...")` で正常接続確認済み
- `libCAEN_Dig1.so` インストール済み ✅

### Arm = Start 問題
- PSD1 は `ArmAcquisition` で即座に開始（`startmode = START_MODE_SW` の場合）
- PSD2 では Arm → Start が分離されている
- **対策:** Reader の FSM で PSD1 の場合は Arm = Start として扱う
- Master/Slave: Slave は `startmode = START_MODE_S_IN` で外部信号待ち

### N_EVENTS の不在
- PSD1 の RAW endpoint は `N_EVENTS` フィールドを返さない可能性がある
- `RawData::n_events` を 0 に設定し、デコーダ側でイベント数を算出する
- 既存の `RawData::from(CaenRawData)` を確認し、`n_events = 0` でも動作することを保証

### 30分ライセンスタイムアウト
- ユーザー側の問題。テスト時はタイムアウト前にデジタイザを再起動
- DAQ ソフトウェア側の対応は不要
- `licensestatus: INVALID LICENSE` 確認済み

### パラメータ名の差異 ✅ 調査済み
- PSD1: `ch_` プレフィックス（例: `ch_dcoffset`, `ch_threshold`, `ch_gate`）
- PSD2: プレフィックスなし（例: `dcoffset`, `triggerthr`, `gatelongt`）
- 完全な対応表は `docs/psd1_decoder_spec.md` Appendix A.4 参照
- `apply_config()` の PSD1 対応は Phase 3 で実装

---

## Files Summary

| Action | File | Description |
|--------|------|-------------|
| **Create** | `src/reader/decoder/psd1.rs` | PSD1 デコーダ本体 |
| **Modify** | `src/reader/decoder/mod.rs` | `pub mod psd1;` 追加 |
| **Modify** | `src/reader/mod.rs` | `decode_loop()` PSD1 対応, `from_config()` マッピング |
| **Create** | `config/config_psd1_test.toml` | テスト用設定ファイル |
| **Create** | `config/digitizers/psd1_test.json` | デジタイザ設定テンプレート |

---

## References

| Document | Location |
|----------|----------|
| **PSD1 Decoder Spec** | `docs/psd1_decoder_spec.md` |
| PSD1 C++ Reference | `legacy/DELILA2/lib/digitizer/src/PSD1Decoder.cpp` |
| PSD1 C++ Constants | `legacy/DELILA2/lib/digitizer/include/PSD1Constants.hpp` |
| PSD2 Rust Decoder | `src/reader/decoder/psd2.rs` |
| FELib User Guide | `legacy/GD9764_FELib_User_Guide.pdf` |
| Digitizer System Spec | `docs/digitizer_system_spec.md` |
