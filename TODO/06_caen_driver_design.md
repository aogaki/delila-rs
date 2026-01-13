# 06: CAEN FFI Driver Design (Phase 2)

**Status: DRAFT** (2026-01-13)

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

### PSD2 Format (64-bit words, Big Endian → Little Endian)

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

### File Structure

```
src/
├── reader/
│   ├── mod.rs              # Reader struct, two-task architecture
│   ├── caen/
│   │   ├── mod.rs          # Re-exports
│   │   ├── ffi.rs          # bindgen generated bindings
│   │   ├── handle.rs       # CaenHandle with RAII Drop
│   │   ├── error.rs        # CaenError enum
│   │   └── digitizer.rs    # Digitizer struct (unified Gen1/Gen2)
│   └── decoder/
│       ├── mod.rs          # Decoder enum + dispatch
│       ├── common.rs       # Shared types (RawData, DataType, etc.)
│       ├── psd2.rs         # PSD2 decoder (64-bit)
│       ├── psd1.rs         # PSD1 decoder (32-bit)
│       └── pha1.rs         # PHA1 decoder (32-bit)
```

### Core Types

```rust
// src/reader/decoder/common.rs

/// Raw data from digitizer
pub struct RawData {
    pub data: Vec<u8>,
    pub size: usize,
    pub n_events: u32,
}

/// Data classification
pub enum DataType {
    Start,
    Stop,
    Event,
    Unknown,
}

/// Decoder result for error handling
pub enum DecodeResult {
    Success,
    InvalidHeader,
    InsufficientData,
    CorruptedData,
    OutOfBounds,
}

/// Waveform configuration (PSD2)
pub struct WaveformConfig {
    pub ap1_signed: bool,
    pub ap2_signed: bool,
    pub ap1_mul_factor: u32,
    pub ap2_mul_factor: u32,
}
```

### Decoder Enum

```rust
// src/reader/decoder/mod.rs

#[derive(Clone)]
pub enum Decoder {
    Psd1(Psd1Decoder),
    Psd2(Psd2Decoder),
    Pha1(Pha1Decoder),
}

impl Decoder {
    /// Create decoder from firmware type detected via Device Tree
    pub fn from_firmware_type(fw_type: FirmwareType, config: DecoderConfig) -> Self {
        match fw_type {
            FirmwareType::Psd1 => Decoder::Psd1(Psd1Decoder::new(config)),
            FirmwareType::Psd2 => Decoder::Psd2(Psd2Decoder::new(config)),
            FirmwareType::Pha1 => Decoder::Pha1(Pha1Decoder::new(config)),
            _ => panic!("Unsupported firmware type"),
        }
    }

    /// Classify data type (Start/Stop/Event/Unknown)
    pub fn classify(&self, raw: &RawData) -> DataType {
        match self {
            Decoder::Psd1(d) => d.classify(raw),
            Decoder::Psd2(d) => d.classify(raw),
            Decoder::Pha1(d) => d.classify(raw),
        }
    }

    /// Decode raw data to events (pure function, stateless)
    pub fn decode(&self, raw: &RawData) -> Vec<EventData> {
        match self {
            Decoder::Psd1(d) => d.decode(raw),
            Decoder::Psd2(d) => d.decode(raw),
            Decoder::Pha1(d) => d.decode(raw),
        }
    }
}
```

### PSD2 Decoder Implementation Sketch

```rust
// src/reader/decoder/psd2.rs

/// PSD2 constants (64-bit words, big endian source)
mod constants {
    pub const WORD_SIZE: usize = 8;

    // Header
    pub const HEADER_TYPE_SHIFT: u32 = 60;
    pub const HEADER_TYPE_MASK: u64 = 0xF;
    pub const HEADER_TYPE_DATA: u64 = 0x2;
    pub const AGGREGATE_COUNTER_SHIFT: u32 = 32;
    pub const AGGREGATE_COUNTER_MASK: u64 = 0xFFFF;
    pub const TOTAL_SIZE_MASK: u64 = 0xFFFFFFFF;

    // Event first word
    pub const CHANNEL_SHIFT: u32 = 56;
    pub const CHANNEL_MASK: u64 = 0x7F;
    pub const TIMESTAMP_MASK: u64 = 0xFFFFFFFFFFFF;

    // Event second word
    pub const WAVEFORM_FLAG_SHIFT: u32 = 62;
    pub const ENERGY_SHORT_SHIFT: u32 = 26;
    pub const ENERGY_SHORT_MASK: u64 = 0xFFFF;
    pub const FINE_TIME_SHIFT: u32 = 16;
    pub const FINE_TIME_MASK: u64 = 0x3FF;
    pub const FINE_TIME_SCALE: f64 = 1024.0;
    pub const ENERGY_MASK: u64 = 0xFFFF;

    // Waveform
    pub const WAVEFORM_WORDS_MASK: u64 = 0xFFF;
    pub const ANALOG_PROBE_MASK: u32 = 0x3FFF;
}

#[derive(Clone)]
pub struct Psd2Decoder {
    time_step_ns: u32,
    module_id: u8,
}

impl Psd2Decoder {
    pub fn new(config: DecoderConfig) -> Self {
        Self {
            time_step_ns: config.time_step_ns,
            module_id: config.module_id,
        }
    }

    pub fn classify(&self, raw: &RawData) -> DataType {
        if raw.size < 3 * constants::WORD_SIZE {
            return DataType::Unknown;
        }
        if raw.size == 3 * constants::WORD_SIZE && self.is_stop_signal(raw) {
            return DataType::Stop;
        }
        if raw.size == 4 * constants::WORD_SIZE && self.is_start_signal(raw) {
            return DataType::Start;
        }
        DataType::Event
    }

    pub fn decode(&self, raw: &RawData) -> Vec<EventData> {
        // Byte swap: big endian → little endian
        let data = self.byte_swap_words(raw);

        // Validate header
        let header = self.read_u64(&data, 0);
        if !self.validate_header(header) {
            return vec![];
        }

        let total_size = (header & constants::TOTAL_SIZE_MASK) as usize;
        let mut events = Vec::with_capacity(total_size / 2);
        let mut word_index = 1;

        while word_index < total_size {
            if let Some(event) = self.decode_event(&data, &mut word_index) {
                events.push(event);
            }
        }

        // Sort by timestamp
        events.sort_by(|a, b| a.timestamp_ns.partial_cmp(&b.timestamp_ns).unwrap());
        events
    }

    fn byte_swap_words(&self, raw: &RawData) -> Vec<u8> {
        let mut data = raw.data.clone();
        for chunk in data.chunks_exact_mut(constants::WORD_SIZE) {
            chunk.reverse();
        }
        data
    }

    fn read_u64(&self, data: &[u8], word_index: usize) -> u64 {
        let offset = word_index * constants::WORD_SIZE;
        u64::from_le_bytes(data[offset..offset+8].try_into().unwrap())
    }

    // ... decode_event, decode_waveform, is_start_signal, is_stop_signal
}
```

### PSD1 Decoder Implementation Sketch

```rust
// src/reader/decoder/psd1.rs

mod constants {
    pub const WORD_SIZE: usize = 4;

    // Board header
    pub const HEADER_TYPE_SHIFT: u32 = 28;
    pub const HEADER_TYPE_MASK: u32 = 0xF;
    pub const HEADER_TYPE_DATA: u32 = 0xA;
    pub const AGGREGATE_SIZE_MASK: u32 = 0x0FFFFFFF;
    pub const DUAL_CHANNEL_MASK: u32 = 0xFF;
    pub const BOARD_HEADER_WORDS: usize = 4;

    // Channel header
    pub const CHANNEL_HEADER_WORDS: usize = 2;
    pub const SAMPLES_ENABLED_SHIFT: u32 = 27;
    pub const EXTRAS_ENABLED_SHIFT: u32 = 28;
    pub const CHARGE_ENABLED_SHIFT: u32 = 30;

    // Event
    pub const TRIGGER_TIME_MASK: u32 = 0x7FFFFFFF;
    pub const ODD_CHANNEL_SHIFT: u32 = 31;
}

#[derive(Clone)]
pub struct Psd1Decoder {
    time_step_ns: u32,
    module_id: u8,
}

impl Psd1Decoder {
    // PSD1 has hierarchical structure:
    // Board Aggregate → Dual Channel Blocks → Events

    pub fn decode(&self, raw: &RawData) -> Vec<EventData> {
        let mut events = Vec::new();
        let mut word_index = 0;
        let total_words = raw.size / constants::WORD_SIZE;

        // Process multiple board aggregate blocks
        while word_index < total_words {
            if let Some(mut block_events) = self.decode_board_aggregate(raw, &mut word_index) {
                events.append(&mut block_events);
            }
        }

        events.sort_by(|a, b| a.timestamp_ns.partial_cmp(&b.timestamp_ns).unwrap());
        events
    }

    // ... decode_board_aggregate, decode_channel_block, decode_event
}
```

---

## Part 4: EventData Structure (Rust)

```rust
// Extend existing src/common/mod.rs

/// Waveform data from digitizer
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Waveform {
    pub analog_probe1: Vec<i32>,
    pub analog_probe2: Vec<i32>,
    pub digital_probe1: Vec<u8>,
    pub digital_probe2: Vec<u8>,
    pub digital_probe3: Vec<u8>,
    pub digital_probe4: Vec<u8>,

    pub time_resolution: u8,
    pub down_sample_factor: u8,
    pub analog_probe1_type: u8,
    pub analog_probe2_type: u8,
    pub digital_probe1_type: u8,
    pub digital_probe2_type: u8,
    pub digital_probe3_type: u8,
    pub digital_probe4_type: u8,
}

/// Extended EventData with waveform support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigitizerEventData {
    // Timing
    pub timestamp_ns: f64,

    // Identification
    pub module: u8,
    pub channel: u8,

    // Energy
    pub energy: u16,
    pub energy_short: u16,

    // Flags
    pub flags: u64,

    // Waveform (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waveform: Option<Waveform>,
}

impl DigitizerEventData {
    pub const FLAG_PILEUP: u64 = 0x01;
    pub const FLAG_TRIGGER_LOST: u64 = 0x02;
    pub const FLAG_OVER_RANGE: u64 = 0x04;

    pub fn has_pileup(&self) -> bool {
        (self.flags & Self::FLAG_PILEUP) != 0
    }
}
```

---

## Part 5: Implementation Tasks

### Phase 2A: Decoder Implementation (Hardware不要)

- [ ] Create `src/reader/decoder/mod.rs` with Decoder enum
- [ ] Create `src/reader/decoder/common.rs` with shared types
- [ ] Implement `src/reader/decoder/psd2.rs`
  - [ ] Byte swap (big endian → little endian)
  - [ ] Header validation
  - [ ] Event decoding (channel, timestamp, energy)
  - [ ] Waveform decoding
  - [ ] Start/Stop signal detection
- [ ] Implement `src/reader/decoder/psd1.rs`
  - [ ] Board aggregate parsing
  - [ ] Dual channel header parsing
  - [ ] Event decoding with extended timestamp
  - [ ] Waveform decoding (different format)
- [ ] Unit tests with captured raw data

### Phase 2B: CAEN FFI (Hardware必要)

- [ ] Set up bindgen for CAEN_FELib headers
- [ ] Implement CaenHandle with RAII Drop
- [ ] Implement CaenError enum
- [ ] Implement Digitizer struct
- [ ] Test with real hardware

### Phase 2C: Reader Integration

- [ ] Implement Reader with two-task architecture
- [ ] ReadLoop task (spawn_blocking for CAEN read)
- [ ] DecodeLoop task (decode + ZMQ publish)
- [ ] Integration with existing control system

---

## Part 6: Recommendations

### bindgen について

**質問への回答**: はい、bindgen のセットアップは実機で進めるのが良いです。

理由:
1. CAEN_FELib.h のパスはインストール環境依存
2. 生成されたバインディングのテストには実機が必要
3. エラーコードの挙動確認も実機で行うべき

### 実装順序の推奨

1. **Decoder を先に実装** (Phase 2A)
   - C++ のキャプチャデータがあれば単体テスト可能
   - FFI なしで純粋な Rust コードとして開発
   - C++ 実装との比較検証が容易

2. **FFI は実機作業時に** (Phase 2B)
   - 実機がある環境で bindgen セットアップ
   - Digitizer struct の実装とテスト
   - Reader 統合

---

## Dependencies

```toml
[build-dependencies]
bindgen = "0.69"  # Phase 2B で追加

[dependencies]
# 既存の依存関係に追加なし
# Decoder は標準ライブラリのみで実装可能
```

## Reference

- C++ implementation: `DELILA2/lib/digitizer/`
- CAEN FELib documentation
- CAEN digitizer user manuals (PSD firmware format)
