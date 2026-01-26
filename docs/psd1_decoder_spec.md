# PSD1 Decoder Specification

**Document Version:** 1.1
**Last Updated:** 2026-01-26
**Status:** Draft (実機検証済み: DT5730B, Serial: 990)
**Hardware Target:** DT5730 (DPP-PSD1, 8/16ch, 14-bit, 500 MS/s)

---

## 1. Overview

### 1.1 Purpose

DPP-PSD1 ファームウェア (x725/x730 シリーズ) の RAW データデコーダ仕様。
DELILA-RS DAQ の Reader コンポーネントに統合し、既存の PSD2 デコーダと同じ `EventData` 出力に変換する。

### 1.2 Hardware

| Property | Value |
|----------|-------|
| Model | DT5730 |
| Firmware | DPP-PSD (PSD1) |
| Library | CAEN_Dig1 (`libCAEN_Dig1.so`) |
| FELib URL | `dig1://` scheme |
| Channels | 8 or 16 |
| ADC Resolution | 14-bit |
| Sampling Rate | 500 MS/s |
| Word Size | **32-bit** (Little-Endian) |

### 1.3 Connection

DT5730 は CAEN FELib API を使用して接続する。PSD2 (VX2730) と同一の C API (`CAEN_FELib_Open`, `_GetValue`, `_SetValue`, `_SendCommand`) を使用するが、URL スキームが `dig1://` となる。

| Interface | URL Format |
|-----------|------------|
| USB | `dig1://caen.internal/usb?link_num=<N>` |
| Optical Link | `dig1://caen.internal/optical_link?link_num=<N>&conet_node=<M>` |
| Optical (A4818) | `dig1://caen.internal/usb_a4818?link_num=<N>&conet_node=<M>` |
| Optical (A3818) | `dig1://caen.internal/pcie_a3818?link_num=<N>&conet_node=<M>` |

**重要:** `dig1://` は `libCAEN_Dig1.so` に依存する。このライブラリは Linux/Windows のみ対応（macOS 非対応）。

### 1.4 Endpoint

PSD2 と同様に `/endpoint/RAW` を使用する。

```
GetReadDataFormatRAW:
  DATA  → U8[]   (raw bytes)
  SIZE  → SIZE_T (data size)
```

**注意:** PSD1 の RAW endpoint は `N_EVENTS` を返さない（PSD2 のみの機能）。

---

## 2. Data Format

### 2.1 Word Format

- **Word size:** 32-bit (4 bytes)
- **Byte order:** Little-Endian (x86 ネイティブ、バイトスワップ不要)
- **対比:** PSD2 は 64-bit Big-Endian

### 2.2 Data Hierarchy

PSD1 のデータは階層構造を持つ。PSD2 がフラットな構造であるのに対し、PSD1 はボード→チャンネルペア→イベントの3層構造。

```
┌─────────────────────────────────────────┐
│ Board Aggregate Block                    │
│ ┌─────────────────────────────────────┐ │
│ │ Board Header (4 words)              │ │
│ ├─────────────────────────────────────┤ │
│ │ Dual Channel Block (pair 0)         │ │
│ │ ┌─────────────────────────────────┐ │ │
│ │ │ Channel Header (2 words)       │ │ │
│ │ ├─────────────────────────────────┤ │ │
│ │ │ Event 0                        │ │ │
│ │ │ Event 1                        │ │ │
│ │ │ ...                            │ │ │
│ │ └─────────────────────────────────┘ │ │
│ ├─────────────────────────────────────┤ │
│ │ Dual Channel Block (pair 1)         │ │
│ │ ...                                 │ │
│ └─────────────────────────────────────┘ │
├─────────────────────────────────────────┤
│ Board Aggregate Block (next)             │
│ ...                                      │
└─────────────────────────────────────────┘
```

一回の ReadData で複数の Board Aggregate Block が含まれる場合がある。

---

## 3. Board Aggregate Header (4 words)

### 3.1 Word 0: Type + Size

```
┌──────────────────────────────────────────┐
│ 31  30  29  28 │ 27              ... 0   │
│  1   0   1   0 │   Aggregate Size        │
│  (Type=0xA)    │   (in 32-bit words)     │
└──────────────────────────────────────────┘
```

| Field | Bits | Mask | Description |
|-------|------|------|-------------|
| Type | [31:28] | `0xF << 28` | Header type identifier = **0xA** |
| Aggregate Size | [27:0] | `0x0FFFFFFF` | Size of entire board block (words) |

### 3.2 Word 1: Board Info

```
┌──────────────────────────────────────────┐
│ 31:27 │ 26 │ 25:23 │ 22:8      │ 7:0    │
│BoardID│Fail│ Rsv   │LVDS Ptn   │DualChM │
└──────────────────────────────────────────┘
```

| Field | Bits | Mask/Shift | Description |
|-------|------|------------|-------------|
| Board ID | [31:27] | `0x1F << 27` | Board identifier (0-31) |
| Board Fail | [26] | `0x1 << 26` | Board failure flag |
| LVDS Pattern | [22:8] | `0x7FFF << 8` | LVDS pattern |
| Dual Channel Mask | [7:0] | `0xFF` | Active channel pairs (bit=pair enabled) |

**Dual Channel Mask:** ビット N が 1 の場合、チャンネルペア `(2N, 2N+1)` がアクティブ。
- DT5730 (8ch): ビット 0-3 が有効 → ペア (0,1), (2,3), (4,5), (6,7)
- DT5730 (16ch): ビット 0-7 が有効

### 3.3 Word 2: Counter

```
┌──────────────────────────────────────────┐
│ 31:23     │ 22:0                          │
│ Reserved  │ Aggregate Counter             │
└──────────────────────────────────────────┘
```

| Field | Bits | Mask | Description |
|-------|------|------|-------------|
| Aggregate Counter | [22:0] | `0x7FFFFF` | Monotonic counter |

### 3.4 Word 3: Time Tag

```
┌──────────────────────────────────────────┐
│ 31:0                                      │
│ Board Time Tag                            │
└──────────────────────────────────────────┘
```

| Field | Bits | Description |
|-------|------|-------------|
| Board Time Tag | [31:0] | Board-level time tag (full 32-bit) |

---

## 4. Dual Channel Header (2 words)

各チャンネルペアの先頭に配置される。

### 4.1 Word 0: Size

```
┌──────────────────────────────────────────┐
│ 31 │ 30:22    │ 21:0                      │
│  1 │ Reserved │ Dual Channel Size         │
└──────────────────────────────────────────┘
```

| Field | Bits | Mask | Description |
|-------|------|------|-------------|
| Header Flag | [31] | `0x1 << 31` | Always 1 |
| Dual Channel Size | [21:0] | `0x3FFFFF` | Size of this channel block (words) |

### 4.2 Word 1: Configuration

```
┌──────────────────────────────────────────────────────────────┐
│ 31 │ 30 │ 29 │ 28 │ 27  │ 26:24     │ 23:22 │ 21:19 │ 18:16 │ 15:0        │
│ DT │ EQ │ ET │ EE │ ES  │ExtraOpt   │  AP   │  DP2  │  DP1  │ numSampWave │
└──────────────────────────────────────────────────────────────┘
```

| Field | Bits | Mask/Shift | Description |
|-------|------|------------|-------------|
| DT (Dual Trace) | [31] | `0x1 << 31` | Dual trace enabled |
| EQ (Charge Enable) | [30] | `0x1 << 30` | Charge word enabled |
| ET (Time Enable) | [29] | `0x1 << 29` | Time tag enabled |
| EE (Extras Enable) | [28] | `0x1 << 28` | Extras word enabled |
| ES (Samples Enable) | [27] | `0x1 << 27` | Waveform enabled |
| Extra Option | [26:24] | `0x7 << 24` | Extras word format |
| Analog Probe | [23:22] | `0x3 << 22` | Analog probe type |
| Digital Probe 2 | [21:19] | `0x7 << 19` | DP2 selection |
| Digital Probe 1 | [18:16] | `0x7 << 16` | DP1 selection |
| Num Samples Wave | [15:0] | `0xFFFF` | Waveform samples / 8 |

**Enable Flags の意味:**

| Flag | Description | 影響 |
|------|-------------|------|
| DT | Dual trace mode | 波形データに第2アナログプローブ含む |
| EQ | Charge enabled | チャージワード (charge_long + charge_short) が存在 |
| ET | Time tag enabled | トリガータイムタグワードが存在 |
| EE | Extras enabled | エクストラワード (extended time, flags, fine time) が存在 |
| ES | Samples enabled | 波形サンプルデータが存在 |

**Extra Option:**

| Value | Format | Description |
|-------|--------|-------------|
| 0b000 | Extended Time only | [31:16] = extended time, [15:0] = baseline×4 |
| 0b001 | Extended Time only | [31:16] = extended time, [15:0] = flags |
| **0b010** | **Extended + Flags + Fine Time** | **[31:16] = extended time, [15:10] = flags, [9:0] = fine time** |
| 0b011 | Reserved | |
| 0b100 | Total trigger counter | |
| 0b101-0b111 | Reserved | |

**推奨:** Extra option = 0b010 を使用。Extended time + Flags + Fine time の全情報を取得可能。

---

## 5. Event Structure

各イベントは可変長で、Enable flags に依存する。

### 5.1 Event Layout

```
Event:
  [Word 0] Trigger Time Tag (ET=1 の場合)
  [Word 1] Extras (EE=1 の場合)
  [Words ] Waveform data (ES=1 の場合, numSamplesWave*2 words)
  [Word N] Charge (EQ=1 の場合)
```

**注意:** 順序は `Time → Extras → Waveform → Charge` の固定順。

### 5.2 Trigger Time Tag Word (ET=1)

```
┌──────────────────────────────────────────┐
│ 31       │ 30:0                           │
│ Ch Flag  │ Trigger Time Tag               │
└──────────────────────────────────────────┘
```

| Field | Bits | Mask | Description |
|-------|------|------|-------------|
| Channel Flag | [31] | `0x1 << 31` | 0 = even channel, 1 = odd channel |
| Trigger Time Tag | [30:0] | `0x7FFFFFFF` | 31-bit coarse timestamp |

**Channel Flag:** デュアルチャンネルペア内で偶数 (0) / 奇数 (1) を識別。
最終チャンネル番号 = `pair * 2 + channel_flag`。

### 5.3 Extras Word (EE=1)

Extra option = 0b010 の場合:

```
┌──────────────────────────────────────────┐
│ 31:16            │ 15:10 │ 9:0            │
│ Extended Time    │ Flags │ Fine Time      │
└──────────────────────────────────────────┘
```

| Field | Bits | Mask | Description |
|-------|------|------|-------------|
| Extended Time | [31:16] | `0xFFFF << 16` | 上位16ビット時刻 |
| Flags | [15:10] | `0x3F << 10` | 6-bit event flags |
| Fine Time | [9:0] | `0x3FF` | 10-bit fine timestamp |

**Flags (6-bit):**

| Bit | Name | Description |
|-----|------|-------------|
| 5 | Trigger Lost | トリガー損失検出 |
| 4 | Over Range | 入力オーバーレンジ |
| 3 | 1024 Triggers | 1024トリガーカウンター |
| 2 | N Lost Triggers | 失われたトリガー数 |
| 1:0 | Reserved | |

### 5.4 Charge Word (EQ=1)

```
┌──────────────────────────────────────────┐
│ 31:16          │ 15      │ 14:0           │
│ Charge Long    │ Pileup  │ Charge Short   │
└──────────────────────────────────────────┘
```

| Field | Bits | Mask | Description |
|-------|------|------|-------------|
| Charge Long | [31:16] | `0xFFFF << 16` | Long gate charge (16-bit) |
| Pileup | [15] | `0x1 << 15` | Pileup detection flag |
| Charge Short | [14:0] | `0x7FFF` | Short gate charge (15-bit) |

**PSD2 との比較:**
- PSD1: `charge_long` [31:16] + `charge_short` [14:0] + pileup [15]
- PSD2: `energy` [15:0] + `energy_short` [25:16] (位置が異なる)

### 5.5 Minimum Event Size

Enable flags の組合せによるイベントサイズ:

| ET | EE | ES | EQ | Words | Description |
|----|----|----|-----|-------|-------------|
| 1 | 1 | 0 | 1 | 3 | 最小 (time + extras + charge) |
| 1 | 1 | 1 | 1 | 3 + N | 波形あり (N = numSamplesWave × 2) |
| 1 | 0 | 0 | 1 | 2 | extras なし |
| 0 | 0 | 0 | 1 | 1 | charge のみ |

**典型的な設定:** ET=1, EE=1, ES=0 or 1, EQ=1 (Extra option=0b010)

---

## 6. Waveform Data (ES=1)

### 6.1 Sample Packing

2 サンプルが 1 つの 32-bit word にパッキングされる。

```
┌──────────────────────────────────────────┐
│ 31 │ 30 │ 29:16                │ 15 │ 14 │ 13:0           │
│DP2b│DP1b│ Analog Sample (even) │DP2a│DP1a│ Analog Sample  │
│ s2 │ s2 │ 14-bit               │ s1 │ s1 │ (odd) 14-bit   │
└──────────────────────────────────────────┘

Lower half (bits [15:0]) = Sample 2N
Upper half (bits [31:16]) = Sample 2N+1
```

| Field | Bits | Mask | Description |
|-------|------|------|-------------|
| Analog Sample 1 | [13:0] | `0x3FFF` | 14-bit analog value (sample 2N) |
| Digital Probe 1 (s1) | [14] | `0x1 << 14` | DP1 for sample 2N |
| Digital Probe 2 (s1) | [15] | `0x1 << 15` | DP2 for sample 2N |
| Analog Sample 2 | [29:16] | `0x3FFF << 16` | 14-bit analog value (sample 2N+1) |
| Digital Probe 1 (s2) | [30] | `0x1 << 30` | DP1 for sample 2N+1 |
| Digital Probe 2 (s2) | [31] | `0x1 << 31` | DP2 for sample 2N+1 |

### 6.2 Total Waveform Words

```
total_samples = numSamplesWave × 8
total_words = numSamplesWave × 2  (since kSamplesPerWord = 2, kSamplesPerGroup = 8)
            = total_samples / 4
```

**注意:** `numSamplesWave` は `(実サンプル数 / 8)` の値。

### 6.3 Dual Trace Mode (DT=1)

DT=1 の場合、偶数/奇数サンプルが交互に異なるプローブデータを含む:
- 偶数サンプル (2N): Analog Probe 1
- 奇数サンプル (2N+1): Analog Probe 2

**PSD2 との違い:** PSD2 では analog_probe1 と analog_probe2 が独立した 14-bit フィールドとして別々に格納される。PSD1 では dual trace モードでインターリーブされる。

### 6.4 Analog Probe Types

| Value | Description |
|-------|-------------|
| 0 | Input signal |
| 1 | CFD |
| 2 | Baseline |
| 3 | Reserved |

### 6.5 Digital Probe Types

| Value | DP1 Description | DP2 Description |
|-------|----------------|----------------|
| 0 | Trigger | Gate Short |
| 1 | CFD Gate | Over threshold |
| 2 | RESERVED | Reserved |
| 3 | RESERVED | TRG validation |
| 4 | RESERVED | Reserved |
| 5 | RESERVED | Reserved |
| 6 | RESERVED | Reserved |
| 7 | RESERVED | Reserved |

---

## 7. Timestamp Calculation

### 7.1 Coarse Timestamp

PSD1 のタイムスタンプは 31-bit のトリガータイムタグに、16-bit の extended time を結合して 47-bit とする。

```
combined = (extended_time << 31) | trigger_time_tag
timestamp_ns = combined × time_step_ns
```

**time_step_ns:** DT5730 = 2 ns (500 MS/s)

### 7.2 Fine Timestamp (Extra option = 0b010)

10-bit fine time で補間。

```
fine_time_ns = fine_time × (time_step_ns / 1024.0)
final_timestamp_ns = timestamp_ns + fine_time_ns
```

DT5730 の場合: fine_time_ns = fine_time × (2.0 / 1024.0) ≈ 1.95 ps/LSB

### 7.3 PSD2 との比較

| | PSD1 | PSD2 |
|--|------|------|
| Coarse bits | 31 + 16 = 47 | 48 |
| Fine bits | 10 | 10 |
| Time step | 2 ns (DT5730) | 8 ns (VX2730) |
| Calculation | `(ext << 31 + ttt) × step + fine × (step/1024)` | `timestamp × step + fine × (step/1024)` |
| Fine time multiplier | `time_step / 1024.0` | `time_step / 1024.0` |

---

## 8. Start/Stop Signal Handling

### 8.1 PSD1 の特徴

**PSD1 には明示的な Start/Stop シグナルが RAW データに含まれない。**

PSD2 では特定のビットパターンで Start/Stop を検出するが、PSD1 (dig1) ではそのようなシグナルは存在しない。

### 8.2 対応方針

- `classify()` は全データを `DataType::Event` として分類（ヘッダータイプ 0xA 確認のみ）
- Start/Stop は Operator からの ZMQ コマンドで制御
- デコーダは常に `DataType::Event` を返す（`Start`/`Stop` を返さない）

---

## 9. PSD1 vs PSD2 比較表

| Feature | PSD1 (DT5730) | PSD2 (VX2730) |
|---------|---------------|---------------|
| **Word size** | 32-bit | 64-bit |
| **Byte order** | Little-Endian | Big-Endian |
| **Data structure** | Hierarchical (Board → Channel Pair → Event) | Flat (Header → Events) |
| **Board header** | 4 × 32-bit words (type=0xA) | 1 × 64-bit word (type=0x2) |
| **Channel grouping** | Dual channel pairs (8 max) | None (flat event list) |
| **Timestamp** | 31-bit TTT + 16-bit extended = 47-bit | 48-bit direct |
| **Fine time** | 10-bit (same) | 10-bit (same) |
| **Energy** | charge_long[31:16] + charge_short[14:0] | energy[15:0] + energy_short[25:16] |
| **Pileup flag** | Charge word bit[15] | High priority flags bit[0] |
| **Flags** | 6-bit (extras word) | 12+8 = 20-bit (low+high priority) |
| **Waveform** | 14-bit + 2 DP per sample, 2 samples/word | 14-bit AP1 + AP2 + 4 DP per sample |
| **Dual trace** | Interleaved (even=AP1, odd=AP2) | Separate fields (AP1, AP2) |
| **Digital probes** | 2 (DP1, DP2) | 4 (DP1-DP4) |
| **Start/Stop signal** | None in data | Explicit bit patterns |
| **Single-word event** | N/A | Supported |
| **Special event** | N/A | Statistics events |
| **FELib URL** | `dig1://` | `dig2://` |
| **Library** | `libCAEN_Dig1.so` | `libCAEN_Dig2.so` |
| **Time step** | 2 ns | 8 ns |
| **ADC bits** | 14-bit | 14-bit (or 16-bit depending on model) |
| **Channels** | 8 or 16 | 32 or 64 |

---

## 10. EventData Mapping

PSD1 デコーダの出力は既存の `EventData` 構造体にマッピングする。

```rust
pub struct EventData {
    pub timestamp_ns: f64,      // ← (ext << 31 + ttt) × step + fine × (step/1024)
    pub module: u8,             // ← config.module_id
    pub channel: u8,            // ← pair * 2 + channel_flag
    pub energy: u16,            // ← charge_long (16-bit)
    pub energy_short: u16,      // ← charge_short (15-bit, zero-extended to u16)
    pub fine_time: u16,         // ← fine_time (10-bit)
    pub flags: u32,             // ← 6-bit flags mapped to u32
    pub waveform: Option<Waveform>,  // ← analog + digital probe data
}
```

### 10.1 Flags Mapping

PSD1 の 6-bit flags を PSD2 互換の `flags: u32` にマッピング:

| PSD1 Bit | PSD1 Meaning | EventData flags bit |
|----------|-------------|---------------------|
| 5 | Trigger Lost | bit 5 |
| 4 | Over Range | bit 4 |
| 3 | 1024 Triggers | bit 3 |
| 2 | N Lost Triggers | bit 2 |
| 15 (charge) | Pileup | bit 15 |

**Note:** Pileup フラグは charge word の bit[15] から取得。

### 10.2 Waveform Mapping

```rust
pub struct Waveform {
    pub analog_probe1: Vec<i16>,   // ← 14-bit analog (sign-extended)
    pub analog_probe2: Vec<i16>,   // ← DT=1 の場合のみ
    pub digital_probe1: Vec<u8>,   // ← DP1
    pub digital_probe2: Vec<u8>,   // ← DP2
    pub digital_probe3: Vec<u8>,   // ← PSD1では未使用 (empty)
    pub digital_probe4: Vec<u8>,   // ← PSD1では未使用 (empty)
    pub time_resolution: u8,       // ← 0 (PSD1には該当フィールドなし)
    pub trigger_threshold: u16,    // ← 0 (PSD1には該当フィールドなし)
}
```

---

## 11. Error Handling

### 11.1 Validation Points

| Check | Action |
|-------|--------|
| Data size not multiple of 4 | Skip entire buffer |
| Board header type ≠ 0xA | Skip entire buffer |
| Aggregate size > remaining data | Skip to next board block |
| Dual channel size > remaining data | Clamp to board block end |
| Insufficient data for event | Skip event |

### 11.2 Recovery Strategy

- Board aggregate block レベルでエラー回復（次のブロックへスキップ）
- 部分的にデコードされたイベントは破棄
- エラーカウンタでデコードエラー率を監視

---

## 12. References

| Document | Location | Description |
|----------|----------|-------------|
| PSD1 C++ Constants | `legacy/DELILA2/lib/digitizer/include/PSD1Constants.hpp` | ビットマスク定義 |
| PSD1 C++ Structures | `legacy/DELILA2/lib/digitizer/include/PSD1Structures.hpp` | データ構造体 |
| PSD1 C++ Decoder | `legacy/DELILA2/lib/digitizer/src/PSD1Decoder.cpp` | リファレンス実装 |
| PSD2 Rust Decoder | `src/reader/decoder/psd2.rs` | PSD2 Rust 実装 (参考) |
| FELib User Guide | `legacy/GD9764_FELib_User_Guide.pdf` | FELib API + URL schemes |
| Digitizer System Spec | `docs/digitizer_system_spec.md` | システム全体仕様 |
| DT5730B DevTree | `docs/devtree_examples/dt5730b_psd1_sn990.json` | 実機 DevTree (151KB) |

---

## Appendix A: DT5730B 実機検証結果

### A.1 デバイス情報

| Property | Value |
|----------|-------|
| Model | DT5730B |
| Serial | 990 |
| Firmware | DPP-PSD |
| Family Code | XX730 |
| Form Factor | DESKTOP |
| Channels | 8 |
| ADC Bits | 14 |
| Energy Bits | 15 |
| Sample Rate | 500 MHz |
| License | **INVALID LICENSE** (30分タイムアウト) |
| PLL Locked | TRUE |

### A.2 FELib Commands (PSD1 vs PSD2)

| Command | PSD1 (DT5730) | PSD2 (VX2730) |
|---------|---------------|---------------|
| `/cmd/armacquisition` | ✅ | ✅ |
| `/cmd/disarmacquisition` | ✅ | ✅ |
| `/cmd/cleardata` | ✅ | ✅ |
| `/cmd/reset` | ✅ | ✅ |
| `/cmd/sendswtrigger` | ✅ | ✅ |
| `/cmd/calibrateadc` | ✅ | ❌ |
| `/cmd/swstartacquisition` | **❌ なし** | ✅ |
| `/cmd/swstopacquisition` | **❌ なし** | ✅ |
| `/cmd/reboot` | ❌ | ✅ |
| `/cmd/sendchswtrigger` | ❌ | ✅ |
| `/cmd/reloadcalibration` | ❌ | ✅ |

**重要:** PSD1 には `SwStartAcquisition` がない。Arm すると `startmode` に応じて自動的に開始する:
- `START_MODE_SW`: Arm 即開始
- `START_MODE_S_IN`: 外部信号 (S-IN) 待ち
- `START_MODE_FIRST_TRG`: 最初のトリガーで開始

### A.3 Start Mode と Master/Slave

| Mode | Description | Master/Slave での使い方 |
|------|-------------|------------------------|
| `START_MODE_SW` | ソフトウェア (Arm = Start) | **Master** |
| `START_MODE_S_IN` | S-IN 端子の信号で開始 | **Slave** |
| `START_MODE_FIRST_TRG` | 最初のトリガーで開始 | 使用しない |

**出力選択 (`/par/out_selection`):**
- `OUT_PROPAGATION_RUN`: Run 状態を出力 → Slave の S-IN に接続

### A.4 DevTree パラメータ名対応 (PSD1 vs PSD2)

**重要:** PSD1 は `ch_` プレフィックス + アンダースコア区切りの命名規則を使用。

| 機能 | PSD1 (DT5730) | PSD2 (VX2730) |
|------|---------------|---------------|
| Start source | `/par/startmode` | `/par/startsource` |
| Record length | `/par/reclen` | `/par/chrecordlengths` |
| Event aggregation | `/par/eventaggr` | `/par/eventpraggreg` |
| Waveform enable | `/par/waveforms` | (endpoint parameter) |
| Extras enable | `/par/extras` | (automatic) |
| I/O level | `/par/iolevel` | (not available) |
| Out selection | `/par/out_selection` | `/par/trgoutsource` |
| External trigger | `/par/trg_ext_enable` | (in startsource) |
| SW trigger | `/par/trg_sw_enable` | (in startsource) |
| Ch enable | `/ch/N/par/ch_enabled` | `/ch/N/par/chenabled` |
| DC offset | `/ch/N/par/ch_dcoffset` | `/ch/N/par/dcoffset` |
| Threshold | `/ch/N/par/ch_threshold` | `/ch/N/par/triggerthr` |
| Polarity | `/ch/N/par/ch_polarity` | `/ch/N/par/chpolarity` |
| Gate long | `/ch/N/par/ch_gate` | `/ch/N/par/gatelongt` |
| Gate short | `/ch/N/par/ch_gateshort` | `/ch/N/par/gateshortt` |
| Gate pre | `/ch/N/par/ch_gatepre` | `/ch/N/par/gatetoffset` |
| CFD delay | `/ch/N/par/ch_cfd_delay` | `/ch/N/par/cfddelay` |
| CFD fraction | `/ch/N/par/ch_cfd_fraction` | `/ch/N/par/cfdfraction` |
| Extras opt | `/ch/N/par/ch_extras_opt` | (automatic) |
| Self trigger | `/ch/N/par/ch_self_trg_enable` | `/ch/N/par/evttriggersrc` |
| Input dynamic | `/ch/N/par/ch_indyn` | `/ch/N/par/indynamic` |
| Discriminator | `/ch/N/par/ch_discr_mode` | `/ch/N/par/discrmode` |
| Energy gain | `/ch/N/par/ch_energy_cgain` | `/ch/N/par/chargesens` |

### A.5 Extras Option 値

| Enum Name | Extra Option | Format |
|-----------|-------------|--------|
| `EXTRAS_OPT_TT48_BL4` | 0b000 | Extended time + baseline×4 |
| `EXTRAS_OPT_TT48_FLAGS` | 0b001 | Extended time + flags |
| **`EXTRAS_OPT_TT48_FLAGS_FINETT`** | **0b010** | **Extended time + flags + fine time (推奨)** |
| `EXTRAS_OPT_LOSTTRG_TOTTRG` | 0b011 | Lost trigger + total trigger |
| `EXTRAS_OPT_SBZC_SAZC` | 0b100 | Sample before/after zero-crossing |

### A.6 Endpoints

| Endpoint | Description |
|----------|-------------|
| `/endpoint/raw` | RAW データ (Board Aggregate format) |
| `/endpoint/dpppsd` | デコード済みイベント |
| `/endpoint/par` | エンドポイント共通パラメータ |
| `/endpoint/handle` | (internal) |

**`/endpoint/par/activeendpoint`:** 現在は `dpppsd`。RAW を使うには変更が必要かもしれない。
