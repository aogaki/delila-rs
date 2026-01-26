# PSD2 デコーダ バグフィックス

**Created:** 2026-01-26
**Priority:** P0 (最高優先度 - データ正確性に直結)
**Status: COMPLETED** (2026-01-26)
**Related:** `src/reader/decoder/psd2.rs`, `src/reader/mod.rs`, `src/reader/caen/handle.rs`

---

## 背景

Linux 移行後の実機検証 (VX2730, S/N 52622) で PSD2 デコーダの問題を発見。
C++ リファレンス実装 (`external/caen-dig2/src/endpoints/dpppsd.cpp`) との比較で
以下のバグを確認した。

---

## Bug A: Single-word イベント未対応 [P0]

- [x] 実装
- [x] テスト

**ファイル:** `src/reader/decoder/psd2.rs` (`decode_event`)

**問題:**
C++ の `decode_hit()` (dpppsd.cpp:240-257) では first word の bit 63 (`last_word`) を
チェックし、1 の場合は **1ワード圧縮形式** で処理する。Rust デコーダはこのフラグを
チェックせず、常に 2ワード形式を仮定している。

**Single-word event layout (C++ dpppsd.cpp:244-257):**
```
bit63: last_word=1
bits[62:56]: channel (7 bits)
bits[55:48]: flag_high_priority (8 bits)  ← 通常の special_event+tbd_1 の位置
bits[47:16]: timestamp_reduced (32 bits)  ← 48bit→32bit に短縮
bits[15:0]:  energy (16 bits)
```

**Standard 2-word event layout:**
```
Word 1: [63:last=0][62:56 channel][55 special_event][54:48 tbd][47:0 timestamp]
Word 2: [63:last][62 waveform][61:50 flags_low][49:42 flags_high]
        [41:26 energy_short][25:16 fine_time][15:0 energy]
```

**影響:** 高レートやフラッシュ時に single-word event が来ると、次のイベントの
first word を second word として誤読し、デコーダが完全にデシンクする → データ全損。

**修正方針:**
1. `decode` メソッドで first word 読み取り後に `last_word` (bit 63) をチェック
2. `last_word=1` の場合は single-word デコード関数を呼ぶ
3. `last_word=0` の場合は現在の 2-word+ デコードを続行
4. Single-word event には `energy_short`, `fine_time`, `flags_low`, waveform がない

```rust
// 修正案 (概要)
fn decode_event(&self, data: &[u8], word_index: &mut usize) -> Option<EventData> {
    let first_word = self.read_u64(data, *word_index);
    *word_index += 1;

    let is_last_word = ((first_word >> 63) & 0x1) != 0;

    if is_last_word {
        // Single-word event
        return self.decode_single_word_event(first_word);
    }

    // Standard 2+ word event (existing logic)
    let special_event = ((first_word >> 55) & 0x1) != 0;
    if special_event {
        // Skip special events (statistics, not physics data)
        // Still need to consume remaining words (2nd word + extras)
        ...
        return None;
    }
    ...
}
```

---

## Bug B: Special Event 未フィルタ [P0]

- [x] 実装
- [x] テスト

**ファイル:** `src/reader/decoder/psd2.rs` (`decode_event`)

**問題:**
C++ (dpppsd.cpp:263, 419-426) では first word の bit 55 (`special_event`) が 1 の
イベントは統計データ（デッドタイム、トリガーカウント）であり、ユーザーに渡さない。
Rust デコーダはこのフラグを無視して全データを物理イベントとして処理している。

**影響:** 統計イベントが物理データに混入。energy/timestamp がゴミ値になる。

**修正方針:**
1. First word の bit 55 をチェック
2. `special_event=1` の場合は extra word (3rd word) を読み飛ばして `None` を返す
3. Extra word は `last_word` bit (bit 63) が 1 になるまで連続する可能性がある

```rust
// C++ の extra word 構造:
// bit 63: last_word
// bits[62:60]: extra_type (3 bits) - 0=wave_info, 1=time_info, 2=counter_info
// bits[59:0]: extra_data (60 bits)
```

**注意:** Bug A の single-word event 対応と同じ関数内で修正するため、
一緒に実装するのが効率的。

---

## Bug C: STOP シグナルのサイレント無視 [P1]

- [x] 実装
- [x] テスト

**ファイル:** `src/reader/caen/handle.rs` (`EndpointHandle::read_data`)

**問題:**
`read_data()` が STOP (ret=-12) を `Ok(None)` で返す (handle.rs:598-600)。
しかし `read_loop` (mod.rs:481-485) は `Err(e)` で STOP をチェックしている。
結果、ハードウェアからの STOP シグナルが検出されず、タイムアウトとして扱われる。

**修正方針 (2案):**

**案1: read_data で Err を返す (推奨)**
```rust
} else if ret == -12 {
    // Stop signal - propagate as error for caller to handle
    Err(CaenError::from_code(ret).unwrap_or(CaenError {
        code: ret,
        name: "Stop".to_string(),
        description: "Acquisition stopped".to_string(),
    }))
}
```

**案2: 専用の戻り値を追加**
```rust
pub enum ReadResult {
    Data(RawData),
    Timeout,
    Stop,
}
```

案1 が既存の read_loop のエラーハンドリングと一致するため、最小変更で済む。

---

## Bug D: FLAGS_LOW_PRIORITY マスク 11bit → 12bit [P2]

- [x] 実装
- [x] テスト

**ファイル:** `src/reader/decoder/psd2.rs` (constants)

**問題:**
```rust
// 現在 (11 bits)
pub const FLAGS_LOW_PRIORITY_MASK: u64 = 0x7FF;

// 正しい (12 bits, C++ dpppsd.hpp:166 flag_low_priority{12})
pub const FLAGS_LOW_PRIORITY_MASK: u64 = 0xFFF;
```

flags 結合部分も修正:
```rust
// 現在
let flags = ((flags_high << 11) | flags_low) as u32;

// 修正
let flags = ((flags_high << 12) | flags_low) as u32;
```

---

## Bug E: Waveform データの欠落 [P2]

- [x] 実装
- [x] テスト

**ファイル:** `src/reader/mod.rs` (`Reader::convert_event`)

**問題:**
`convert_event()` が `CommonEventData::new()` を使い、waveform を常に `None` にする。
デコーダの `Waveform` と共通の `Waveform` は同一フィールドだが別の型。

**修正方針:**
```rust
fn convert_event(event: &EventData) -> CommonEventData {
    if let Some(ref wf) = event.waveform {
        CommonEventData::with_waveform(
            event.module, event.channel,
            event.energy, event.energy_short,
            event.timestamp_ns, event.flags as u64,
            CommonWaveform {
                analog_probe1: wf.analog_probe1.clone(),
                analog_probe2: wf.analog_probe2.clone(),
                digital_probe1: wf.digital_probe1.clone(),
                digital_probe2: wf.digital_probe2.clone(),
                digital_probe3: wf.digital_probe3.clone(),
                digital_probe4: wf.digital_probe4.clone(),
                time_resolution: wf.time_resolution,
                trigger_threshold: wf.trigger_threshold,
            },
        )
    } else {
        CommonEventData::new(
            event.module, event.channel,
            event.energy, event.energy_short,
            event.timestamp_ns, event.flags as u64,
        )
    }
}
```

**将来検討:** decoder::Waveform と common::Waveform を統一するか、From トレイトを実装。

---

## 実装順序

```
1. Bug A + B (同一関数内、セットで修正)
   └── decode_event() にlast_word/special_event チェック追加
   └── decode_single_word_event() 新規追加
   └── skip_extra_words() 新規追加
   └── テスト: 各イベント形式のバイナリデータを用意

2. Bug C (read_data の STOP 処理)
   └── read_data() の ret=-12 を Err に変更
   └── テスト: STOP コードで Err が返ることを確認

3. Bug D (FLAGS マスク修正)
   └── 定数とフラグ結合ロジック修正
   └── テスト: 既存テストの期待値更新

4. Bug E (Waveform 変換)
   └── convert_event() 修正
   └── テスト: waveform 付きイベントの変換テスト
```

---

## テスト方針

各バグ修正には以下のテストを追加:

1. **ユニットテスト** (psd2.rs 内): バイナリデータから各形式のデコードを検証
2. **Integration test**: 実機でテストパルス読み出し→デコード→energy 非ゼロ確認
3. `cargo test` + `cargo clippy` 全パス確認

---

## 参考資料

| 資料 | 場所 |
|------|------|
| C++ PSD2 デコーダ | `external/caen-dig2/src/endpoints/dpppsd.cpp` |
| C++ PSD2 ビットレイアウト | `external/caen-dig2/include/endpoints/dpppsd.hpp:155-174` |
| Rust PSD2 デコーダ | `src/reader/decoder/psd2.rs` |
| CAEN FELib エラーコード | `src/reader/caen/error.rs:74-92` |
| DELILA2 PSD2 デコーダ | `legacy/DELILA2/lib/digitizer/src/PSD2Decoder.cpp` |
| DELILA2 PSD2 定数 | `legacy/DELILA2/lib/digitizer/include/PSD2Constants.hpp` |

---

## Implementation Summary (2026-01-26)

### 変更ファイル

| File | Changes |
|------|---------|
| `src/reader/decoder/psd2.rs` | Bug A+B+D: decode_event リファクタリング、定数追加、テスト6件追加 |
| `src/reader/caen/handle.rs` | Bug C: read_data() の ret=-12 を Err に変更 |
| `src/reader/mod.rs` | Bug E: convert_event() で waveform を変換 |

### Key Decisions

- **Bug A**: `decode_single_word_event()` を新設。`SINGLE_WORD_FLAG_HIGH_SHIFT=48` (first word の位置)
- **Bug B**: extra word を `last_word` bit でループ消費してから `special_event` チェック
- **Bug C**: `Ok(None)` → `Err(CaenError)` に変更。既存の `read_loop` の `e.code == STOP` パスに合致
- **Bug D**: `FLAGS_LOW_PRIORITY_MASK` 0x7FF→0xFFF、shift 11→12。DELILA2 C++ にも同じバグあり
- **Bug E**: `CommonWaveform` import 追加、フィールド clone で変換
- **MIN_DATA_SIZE**: 3→2 words に変更 (single-word event 対応)

### Test Results

- 220 tests passed, 0 failed (6 new tests added)
- cargo clippy: no warnings
- 新規テスト: single_word_event, special_event_filtered, special_event_with_following_normal, flags_12bit_mask, mixed_single_and_standard, convert_event_with_waveform

### DELILA2 との関係

Bug A, B, D は DELILA2 C++ にも存在する同一のバグ。
ユーザー設定 (`EnDataReduction`, `EnStatEvents`) がデフォルト `False` のため顕在化していなかった。
