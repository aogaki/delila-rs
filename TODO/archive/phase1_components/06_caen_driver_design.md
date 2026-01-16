# 06: CAEN FFI Driver Design (Phase 2)

**Status: COMPLETED** (2026-01-13)

## Goal
Design and implement a safe Rust wrapper for CAEN digitizers based on analysis of the C++ DELILA2/lib/digitizer implementation.

---

## Part 1: C++ Implementation Analysis

### Class Hierarchy
```
IDigitizer (interface)
    ├── Digitizer1  → PSD1Decoder, PHA1Decoder  (旧世代 x725/x730, 32-bit words)
    ├── Digitizer2  → PSD2Decoder, AMaxDecoder  (新世代 x27xx, 64-bit words)
    └── Digitizer   (backward compatibility wrapper)

IDecoder (interface)
    ├── PSD1Decoder  - 32-bit word format
    ├── PSD2Decoder  - 64-bit word format
    ├── PHA1Decoder  - 32-bit word format
    └── AMaxDecoder  - Custom firmware
```

### Key Files Analyzed
| File | Purpose |
|------|---------|
| `IDecoder.hpp` | Interface: SetTimeStep, SetDumpFlag, SetModuleNumber, AddData, GetEventData |
| `PSD2Decoder.hpp/cpp` | 64-bit word decoder with waveform support |
| `PSD2Constants.hpp` | Bit masks, shifts, validation constants |
| `PSD2Structures.hpp` | Header/Event/Waveform info structs |
| `PSD1Decoder.hpp/cpp` | 32-bit word decoder with board/channel hierarchy |
| `PSD1Constants.hpp` | PSD1 specific constants |
| `PSD1Structures.hpp` | BoardHeaderInfo, DualChannelInfo |
| `MemoryReader.hpp` | Safe memory access utility |
| `DataValidator.hpp` | Validation utilities |
| `DecoderLogger.hpp` | Logging with DecoderResult enum |
| `EventData.hpp` | Output event structure |
| `RawData.hpp` | Input raw data container |

---

## Part 2: Data Format Details

### PSD2 Format (64-bit words, Native Little Endian on x86/ARM)

**注意**: C++ 実装では memcpy でそのまま読み込んでおり、バイトスワップは行っていない。
データは Little Endian でメモリに格納されている。

```
┌─────────────────────────────────────────────────────────────────────┐
│ Aggregate Header (1 word = 64 bits)                                  │
├─────────────────────────────────────────────────────────────────────┤
│ [63:60] Type = 0x2 (data)                                           │
│ [56]    Fail check                                                  │
│ [47:32] Aggregate counter (16 bits)                                 │
│ [31:0]  Total size in 64-bit words                                  │
└─────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────┐
│ Event First Word (64 bits)                                           │
├─────────────────────────────────────────────────────────────────────┤
│ [62:56] Channel (7 bits, 0-127)                                     │
│ [47:0]  Timestamp (48 bits)                                         │
└─────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────┐
│ Event Second Word (64 bits)                                          │
├─────────────────────────────────────────────────────────────────────┤
│ [63]    Last word flag                                              │
│ [62]    Waveform present flag                                       │
│ [60:50] Flags low priority (11 bits)                                │
│ [49:42] Flags high priority (8 bits)                                │
│ [41:26] Energy short (16 bits)                                      │
│ [25:16] Fine time (10 bits, /1024 scale)                            │
│ [15:0]  Energy long (16 bits)                                       │
└─────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────┐
│ Waveform Header (64 bits) - if waveform present                      │
├─────────────────────────────────────────────────────────────────────┤
│ [63]    Check1 = 1                                                  │
│ [62:60] Check2 = 0                                                  │
│ [45:44] Time resolution (0=1x, 1=2x, 2=4x, 3=8x)                    │
│ [43:28] Trigger threshold                                           │
│ [27:24] Digital probe 4 type                                        │
│ [23:20] Digital probe 3 type                                        │
│ [19:16] Digital probe 2 type                                        │
│ [15:12] Digital probe 1 type                                        │
│ [11:10] AP2 mul factor                                              │
│ [9]     AP2 signed                                                  │
│ [8:6]   Analog probe 2 type                                         │
│ [5:4]   AP1 mul factor                                              │
│ [3]     AP1 signed                                                  │
│ [2:0]   Analog probe 1 type                                         │
└─────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────┐
│ Waveform Size Word (64 bits)                                         │
├─────────────────────────────────────────────────────────────────────┤
│ [11:0]  Number of waveform words (samples = words * 2)              │
└─────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────┐
│ Waveform Data Word (64 bits) - repeated                              │
├─────────────────────────────────────────────────────────────────────┤
│ [63:32] Sample 2: [31] DP4, [30] DP3, [29:16] AP2, [15] DP2,        │
│                   [14] DP1, [13:0] AP1                               │
│ [31:0]  Sample 1: same layout                                       │
└─────────────────────────────────────────────────────────────────────┘

Start Signal: 4 words, [63:60]=0x3, [59:56]=0x0, ...
Stop Signal:  3 words, [63:60]=0x3, [59:56]=0x2, ...
```

### PSD1 Format (32-bit words, Little Endian)

```
┌─────────────────────────────────────────────────────────────────────┐
│ Board Aggregate Header (4 words = 128 bits)                          │
├─────────────────────────────────────────────────────────────────────┤
│ Word 0: [31:28] Type = 0xA, [27:0] Aggregate size                   │
│ Word 1: [31:27] Board ID, [26] Fail, [22:8] LVDS, [7:0] Ch mask     │
│ Word 2: [22:0] Aggregate counter                                    │
│ Word 3: [31:0] Board time tag                                       │
└─────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────┐
│ Dual Channel Header (2 words = 64 bits)                              │
├─────────────────────────────────────────────────────────────────────┤
│ Word 0: [31] Header flag, [21:0] Aggregate size                     │
│ Word 1: [31] DT, [30] EQ, [29] ET, [28] EE, [27] ES,                │
│         [26:24] Extra option, [23:22] AP, [21:19] DP2, [18:16] DP1, │
│         [15:0] Samples/8                                            │
└─────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────┐
│ Event Data (variable size)                                           │
├─────────────────────────────────────────────────────────────────────┤
│ Trigger Word:  [31] Odd channel, [30:0] Trigger time tag            │
│ Extras Word:   [31:16] Extended time, [15:10] Flags, [9:0] Fine TS  │
│ Waveform:      2 samples per word (16 bits each)                    │
│ Charge Word:   [31:16] Long, [15] Pileup, [14:0] Short              │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Part 3: Rust Implementation Design

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Reader                                       │
│                                                                      │
│  ┌────────────────────┐         ┌────────────────────────────────┐  │
│  │   ReadLoop Task    │         │       DecodeLoop Task          │  │
│  │   (high priority)  │         │       (normal priority)        │  │
│  │                    │         │                                │  │
│  │  loop {            │         │  loop {                        │  │
│  │    raw = dig.read()│ ──────► │    raw = rx.recv()             │  │
│  │    tx.send(raw)    │  mpsc   │    events = decoder.decode()   │  │
│  │  }                 │ channel │    publish(events)             │  │
│  │                    │         │  }                             │  │
│  └────────────────────┘         └────────────────────────────────┘  │
│           │                                    │                     │
│           ▼                                    ▼                     │
│     Digitizer                              Decoder                   │
│     (CAEN FFI)                          (enum dispatch)              │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

**重要**: ReadLoop と DecodeLoop を分離する理由は、デコード処理がブロックされても HW バッファが溢れないようにするため。

### File Structure (実装済み)

```
src/
├── reader/
│   ├── mod.rs              # Reader struct with two-task architecture ✅
│   ├── caen/
│   │   ├── mod.rs          # Re-exports ✅
│   │   ├── ffi.rs          # bindgen generated bindings ✅
│   │   ├── handle.rs       # CaenHandle with RAII Drop ✅
│   │   ├── error.rs        # CaenError enum ✅
│   │   ├── wrapper.h       # Header for bindgen ✅
│   │   └── wrapper.c       # C wrapper for variadic functions ✅
│   └── decoder/
│       ├── mod.rs          # Decoder re-exports ✅
│       ├── common.rs       # Shared types (RawData, DataType, EventData) ✅
│       ├── psd2.rs         # PSD2 decoder (64-bit) ✅
│       ├── psd1.rs         # PSD1 decoder (32-bit) (後回し)
│       └── pha1.rs         # PHA1 decoder (32-bit) (後回し)
├── bin/
│   └── reader.rs           # Reader binary ✅
```

---

## Part 4: Implementation Progress

### Phase 2B: CAEN FFI (Hardware必要) - **COMPLETED** ✅

- [x] Set up bindgen for CAEN_FELib headers
- [x] Implement CaenHandle with RAII Drop
- [x] Implement CaenError enum
- [x] Implement EndpointHandle for data readout
- [x] **C wrapper for variadic ReadData function** (macOS ARM64 issue fix)
- [x] Test with real hardware (VX2730, DPP_PSD firmware)

#### 実装詳細

**問題**: `CAEN_FELib_ReadData` は variadic 関数 (`int CAEN_FELib_ReadData(uint64_t handle, int timeout, ...)`)
であり、Rust から直接呼び出すと macOS ARM64 でセグフォが発生。

**解決策**: C ラッパー関数を作成し、`cc` クレートでコンパイル。

```c
// wrapper.c
int caen_read_data_raw(uint64_t handle, int timeout, uint8_t* data, size_t* size, uint32_t* n_events) {
    return CAEN_FELib_ReadData(handle, timeout, data, size, n_events);
}
```

#### テスト結果 (2026-01-13)

```
===========================================
CAEN Digitizer Info
===========================================
Connecting to: dig2://172.18.4.56
[OK] Connected successfully

--- Device Information ---
  ModelName           : VX2730
  SerialNum           : 52622
  FwType              : DPP_PSD
  FPGA_FwVer          : 1.0.57
  NumCh               : 32
  ADC_SamplRate       : 500
  ADC_Nbit            : 14

--- Data Readout Test ---
  [READ 1] size: 32 bytes, n_events: 1       <- Start Signal
  [READ 2] size: 659336 bytes, n_events: 1   <- Event Data
  [READ 3] size: 657704 bytes, n_events: 1
  [READ 4] size: 652808 bytes, n_events: 1
  [READ 5] size: 646280 bytes, n_events: 1

  --- Summary ---
  Total reads:  5
  Total bytes:  2616160
  Total events: 5
  [OK] Data readout successful!
```

### Phase 2A: Decoder Implementation (Hardware不要) - **COMPLETED** ✅

- [x] Create `src/reader/decoder/mod.rs` with Decoder enum
- [x] Create `src/reader/decoder/common.rs` with shared types
- [x] Implement `src/reader/decoder/psd2.rs`
  - [x] Header validation
  - [x] Event decoding (channel, timestamp, energy)
  - [x] Waveform decoding
  - [x] Start/Stop signal detection
  - [x] **Dump機能 (デバッグ用)**
- [ ] Implement `src/reader/decoder/psd1.rs` (後回し)
- [x] Unit tests with captured raw data

### Phase 2C: Reader Integration - **COMPLETED** ✅

- [x] Implement Reader with two-task architecture
- [x] ReadLoop task (spawn_blocking for CAEN read)
- [x] DecodeLoop task (decode + ZMQ publish)
- [x] Integration with existing control system (5-state machine, command handling)
- [x] Create bin/reader.rs binary
- [x] Unit tests

#### 実装詳細 (2026-01-13)

**アーキテクチャ:**
- `tokio::sync::mpsc::unbounded_channel` でReadLoop→DecodeLoop間のデータ転送
- `tokio::task::spawn_blocking` でCAEN FFI読み取りをブロッキングスレッドで実行
- `watch::channel` でコンポーネント状態をタスク間で共有
- `broadcast::channel` でシャットダウンシグナルを配信

**コンポーネント:**
- `Reader` struct: メイン構造体、Emulatorと同様のパターン
- `ReaderConfig`: URL, アドレス, ファームウェアタイプなどの設定
- `ReaderMetrics`: events_decoded, bytes_read, batches_published, queue_length
- `ReadLoop`: CAEN FFI読み取り専用（ブロッキング）
- `DecodeLoop`: デコード + ZMQ publish（非同期）
- `command_task`: REQ/REPコマンド処理

**使用方法:**
```bash
cargo run --bin reader -- --url dig2://172.18.4.56
cargo run --bin reader -- --url dig2://172.18.4.56 --source-id 0 --module-id 0
```

---

## Part 5: Dependencies

```toml
[build-dependencies]
bindgen = "0.70"
cc = "1"  # C wrapper compilation

[dependencies]
# 既存の依存関係に追加なし
# Decoder は標準ライブラリのみで実装可能
```

---

## Part 6: Key Learnings

### macOS ARM64 での variadic FFI 問題

- Rust は variadic C 関数を直接呼び出せない（ABI の問題）
- 解決策: C ラッパー関数を作成し、具体的な引数で呼び出す
- `cc` クレートで `build.rs` からコンパイル

### VX2730 RAW データのエンディアン

**重要な発見 (2026-01-13)**: VX2730 の RAW モードデータは **Big Endian** 形式で送信される。

#### 症状
- Rust で `u64::from_le_bytes()` を使用すると、ヘッダタイプが `0x8` と解釈される
- 期待値は `0x2` であり、ビット位置が明らかにずれている

#### 原因
- VX2730 (x27xx シリーズ) の RAW エンドポイントデータは Big Endian 形式
- 64-bit ワードの最上位バイトがデータ配列の先頭に来る

#### 解決策
```rust
// 正しい実装
fn read_u64(&self, data: &[u8], word_index: usize) -> u64 {
    let offset = word_index * 8;
    u64::from_be_bytes(data[offset..offset + 8].try_into().unwrap())
}
```

#### C++ 実装との比較
C++ の実装は `memcpy` を使用しているが、**間違っていない**。

```cpp
uint64_t headerWord = 0;
std::memcpy(&headerWord, rawData->data.data(), sizeof(uint64_t));
auto headerType = (headerWord >> 60) & 0xF;  // これでも正しく動作
```

**理由**: `memcpy` は Little Endian マシン (x86/ARM64) 上でバイト配列をそのまま `uint64_t` にコピーする。
これにより、Big Endian データが「反転」された形で整数値として解釈されるが、
C++ の定数（シフト量やマスク）はこの「反転後の値」に対して定義されているため、結果的に正しく動作する。

両方のアプローチは正しい：
- **C++**: `memcpy` + Little Endian マシンでの暗黙的バイトスワップ
- **Rust**: `from_be_bytes()` で明示的に Big Endian として解釈

#### 教訓
- CAENドキュメントにはエンディアンの明確な記述がない場合がある
- 実際のデータのビットパターンを確認することが重要
- デバッグ用のダンプ機能は必須

---

## Reference

- C++ implementation: `DELILA2/lib/digitizer/`
- CAEN FELib documentation
- CAEN digitizer user manuals (PSD firmware format)
