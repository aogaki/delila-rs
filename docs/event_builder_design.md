# Event Builder 設計ドキュメント

**Version:** 2.0.0
**Date:** 2026-02-02
**Status:** ❌ OBSOLETE(歴史的記録)— Time Slice 方式は chunk_builder 統一パイプライン(SPEC v0.5+)で置換済み。さらに **2026-08-04: EB 実装言語を C++(root_sink 系譜)へ移行決定** — 現行仕様は [TODO/event-builder/SPECIFICATION.md](../TODO/event-builder/SPECIFICATION.md) (v0.8)、移行計画は [TODO/event-builder/66_cpp_event_builder.md](../TODO/event-builder/66_cpp_event_builder.md)。下記「実装言語 = Rust(oxyroot で ROOT 出力可能)」の前提は覆っている(oxyroot は書き込み圧縮未実装と 2026-08-04 に確定)

---

## 1. 概要

### 1.1 目的

ELIFANT-Event (C++ オフライン) を参考に、Rust で Event Builder を実装する。
オンライン・オフライン両方のモードをサポートし、oxyroot による ROOT ファイル出力を採用。

### 1.2 決定事項

| 項目 | 決定 | 根拠 |
|------|------|------|
| 実装言語 | Rust (delila-rs 内) | oxyroot で ROOT 出力可能、既存コードとの統合容易 |
| 入力フォーマット | DELILA v2 ファイル | 既存の recover.rs パーサー再利用可能 |
| 出力フォーマット | ROOT TTree (oxyroot) | 物理解析との互換性 |
| コインシデンスウィンドウ | 設定可能 (デフォルト ±500 ns) | 設定ファイルで指定 |
| チャンネル設定 | デフォルト + オーバーライド方式 | 全352チャンネルを明示的に記述する必要なし |
| 設定管理 | MongoDB + Web GUI (将来) | 既存 Operator UI との統合 |

---

## 2. アーキテクチャ

### 2.1 コンポーネント構成

```
┌────────────────────────────────────────────────────────────────────┐
│                     Event Builder Component                         │
│                                                                     │
│  ┌────────────┐  mpsc   ┌─────────────┐  mpsc   ┌────────────┐    │
│  │ Receiver   │ ──────▶ │ TimeSorter  │ ──────▶ │  Builder   │    │
│  │ (ZMQ/File) │ channel │ + Buffer    │ channel │ (L1/L2)    │    │
│  └────────────┘         └─────────────┘         └─────┬──────┘    │
│       │                       │                       │            │
│       │ High-speed            │ 1-sec buffer          │            │
│       │ No blocking           │ Sorting               ▼            │
│       ▼                       ▼                 ┌────────────┐    │
│  ┌────────────┐         ┌─────────────┐        │ Writer     │    │
│  │ Command    │◄───────▶│ State       │        │ (ROOT)     │    │
│  │ (ZMQ REP)  │  watch  │ (shared)    │        └────────────┘    │
│  └────────────┘ channel └─────────────┘                           │
│                                                                     │
│  入力: オフライン=DELILA v2 ファイル, オンライン=Event Bridge      │
│  出力: ROOT TTree (oxyroot)                                        │
└────────────────────────────────────────────────────────────────────┘
```

### 2.2 ディレクトリ構成

```
src/
├── event_builder/
│   ├── mod.rs                    # モジュールエクスポート
│   ├── hit.rs                    # Hit 構造体 (EventData → Hit 変換)
│   ├── built_event.rs            # BuiltEvent 出力構造体
│   ├── config.rs                 # ChSettings, TimeCalibration, デフォルト+オーバーライド
│   ├── time_sort.rs              # BTreeMap ベース時間ソートバッファ
│   ├── coincidence.rs            # L1 コインシデンス検出アルゴリズム
│   ├── time_calibration.rs       # Time alignment ヒストグラム (-t モード)
│   ├── histogram.rs              # 1D/2D ヒストグラムユーティリティ
│   ├── input/
│   │   ├── mod.rs                # InputSource trait
│   │   ├── zmq.rs                # ZMQ SUB 入力 (オンライン, 将来)
│   │   ├── delila.rs             # DELILA v2 ファイル入力 (オフライン)
│   │   └── root.rs               # ROOT TTree 入力 (ELIFANT-Event 互換)
│   └── output/
│       ├── mod.rs                # OutputSink trait
│       └── root.rs               # ROOT TTree 出力 (oxyroot)
└── bin/
    └── event_builder.rs          # CLI (-t, -l1 モード)
```

---

## 3. CLI インターフェース

```bash
# 初期化: 設定テンプレート生成
event_builder init --modules 11 --channels 32 --output event_builder.toml

# Time Calibration: チャンネル間時間差測定 (DELILA v2 入力)
event_builder time -i ./data/*.delila -c event_builder.toml -w 1000 -o timeSettings.json

# Time Calibration: ROOT TTree 入力 (ELIFANT-Event 互換)
event_builder time -i ./data/*.root --format root -c event_builder.toml -w 1000 -o timeSettings.json

# L1 Event Building: コインシデンスイベント構築
event_builder l1 -i ./data/*.delila -c event_builder.toml -t timeSettings.json -w 500 -o L1_output.root

# L1 Event Building: ROOT TTree 入力
event_builder l1 -i ./data/*.root --format root -c event_builder.toml -t timeSettings.json -w 500 -o L1_output.root

# L1 オンラインモード (将来)
event_builder l1 --online -i tcp://localhost:5600 -c event_builder.toml -t timeSettings.json
```

---

## 4. 主要データ構造

### 4.1 Hit (内部表現)

```rust
/// デコーダから来た1ヒットのデータ (EventData から変換)
pub struct Hit {
    pub module: u8,
    pub channel: u8,
    pub energy: u16,
    pub energy_short: u16,
    pub timestamp_ns: f64,
    pub with_ac: bool,  // イベント構築時に設定
}

impl Hit {
    /// delila-rs の EventData から変換
    pub fn from_event_data(event: &EventData) -> Self;

    /// 時間オフセットを適用
    pub fn apply_offset(&mut self, offset_ns: f64);

    /// チャンネルキー (module << 8 | channel)
    pub fn channel_key(&self) -> u16;
}
```

### 4.2 BuiltEvent (出力)

```rust
/// 構築済みイベント
pub struct BuiltEvent {
    pub event_id: u64,
    pub trigger_time: f64,
    pub trigger_module: u8,
    pub trigger_channel: u8,
    pub hits: Vec<EventHit>,
}

/// イベント内の1ヒット (相対時刻)
pub struct EventHit {
    pub module: u8,
    pub channel: u8,
    pub energy: u16,
    pub energy_short: u16,
    pub relative_time: f64,  // トリガー基準の相対時刻 [ns]
    pub with_ac: bool,
}
```

### 4.3 ROOT TTree 出力構造

```
TTree "Events"
├── EventID       : ULong64_t
├── TriggerTime   : Double_t
├── TriggerMod    : UChar_t
├── TriggerCh     : UChar_t
├── NHits         : UInt_t
├── HitMod[NHits]       : UChar_t[]
├── HitCh[NHits]        : UChar_t[]
├── HitEnergy[NHits]    : UShort_t[]
├── HitEnergyShort[NHits] : UShort_t[]
├── HitTime[NHits]      : Double_t[]
└── HitWithAC[NHits]    : Bool_t[]
```

---

## 5. チャンネル設定 (デフォルト + オーバーライド方式)

### 5.1 設計思想

ELIFANT-Event は全チャンネルを JSON で明示的に記述する方式だが、
352チャンネルの設定ファイルは管理が困難。

**デフォルト + オーバーライド方式:**
- グローバルなデフォルト値を定義
- 特定のチャンネル (Trigger, AC ペア等) のみオーバーライド
- 将来的に Web GUI + MongoDB で管理

### 5.2 設定ファイル形式 (TOML)

```toml
# event_builder.toml

[defaults]
is_trigger = false
trigger_priority = 1000  # 高い値 = 低優先度
threshold_adc = 0
has_ac = false
detector_type = "Unknown"

[time_calibration]
window_ns = 1000.0
reference_mod = 0
reference_ch = 0

[event_building]
coincidence_window_ns = 500.0
buffer_delay_ns = 1_000_000_000.0  # 1秒

# トリガーチャンネル指定
[[channels]]
module = 0
channel = 0
is_trigger = true
trigger_priority = 0  # 最高優先度
detector_type = "HPGe"
tags = ["HPGe", "Trigger"]

[[channels]]
module = 0
channel = 1
detector_type = "AC"
tags = ["AC"]

# HPGe と AC のペア
[[channels]]
module = 0
channel = 2
is_trigger = true
trigger_priority = 1
has_ac = true
ac_mod = 0
ac_ch = 1
detector_type = "HPGe"
tags = ["HPGe", "Trigger"]

# Si テレスコープ
[[channels]]
module = 1
channels = [0, 1, 2, 3, 4, 5, 6, 7]  # 複数チャンネル一括指定
detector_type = "Si"
tags = ["Si", "E_Sector"]
```

### 5.3 将来の Web GUI 統合

```
┌─────────────────────────────────────────────────────────────────┐
│                    Event Builder 設定 UI                         │
│                                                                  │
│  ┌─────────────────────────────────────────────────────────────┐│
│  │ Trigger Channels                                             ││
│  │ ┌──────┬─────────┬──────────┬───────────────┬─────────────┐ ││
│  │ │ Mod  │ Channel │ Priority │ Detector Type │ AC Pair     │ ││
│  │ ├──────┼─────────┼──────────┼───────────────┼─────────────┤ ││
│  │ │ 0    │ 0       │ 0        │ HPGe          │ 0:1         │ ││
│  │ │ 0    │ 2       │ 1        │ HPGe          │ 0:3         │ ││
│  │ │ ...  │ ...     │ ...      │ ...           │ ...         │ ││
│  │ └──────┴─────────┴──────────┴───────────────┴─────────────┘ ││
│  │ [+ Add Trigger]                                              ││
│  └─────────────────────────────────────────────────────────────┘│
│                                                                  │
│  ┌─────────────────────────────────────────────────────────────┐│
│  │ Event Building Parameters                                    ││
│  │ Coincidence Window: [____500____] ns                         ││
│  │ Buffer Delay:       [____1.0____] s                          ││
│  └─────────────────────────────────────────────────────────────┘│
│                                                                  │
│  [Save to MongoDB] [Export TOML] [Apply]                        │
└─────────────────────────────────────────────────────────────────┘
```

**MongoDB スキーマ:**
```json
{
  "_id": ObjectId("..."),
  "name": "CRIB2026_config",
  "created_at": ISODate("..."),
  "defaults": {
    "is_trigger": false,
    "threshold_adc": 0
  },
  "time_calibration": {
    "window_ns": 1000.0,
    "reference_mod": 0,
    "reference_ch": 0
  },
  "event_building": {
    "coincidence_window_ns": 500.0,
    "buffer_delay_ns": 1000000000.0
  },
  "channels": [
    { "module": 0, "channel": 0, "is_trigger": true, "trigger_priority": 0 }
  ]
}
```

---

## 6. アルゴリズム

### 6.1 Time Calibration (-t モード)

ELIFANT-Event の TimeAlignment クラスに相当。

**アルゴリズム:**
1. トリガーチャンネルを基準 (reference) として選択
2. 全ヒットペア (reference, target) で時間差を計算
3. 2D ヒストグラム (X: 時間差, Y: 検出器ID) を構築
4. 各検出器IDごとにX軸投影 → ピーク位置を検出
5. `offset = -peak_position` として timeSettings.json に出力

**ヒストグラムパラメータ:**
- X軸: 時間差 [-window_ns, +window_ns]
- Y軸: 検出器ID (0 ~ max_id)
- ビン数: window_ns (ns単位で1ビン)

### 6.2 L1 Event Building (-l1 モード) — Time Slice 方式

CBM/FLES で実績のある Time Slice 方式を採用。
並列処理が可能で、メモリ使用量が予測可能。

#### 6.2.1 Time Slice の概念

```
時間軸 ─────────────────────────────────────────────────────────▶

Slice N:   [========= Core =========][= Overlap =]
                                      ↑
                                 coincidence_window

Slice N+1:                     [= Overlap =][======= Core =======]
                                ↑
                           同じヒットが両スライスに存在
```

**パラメータ:**
- `slice_duration`: 10 ms (デフォルト)
- `overlap`: coincidence_window (例: 500 ns)

#### 6.2.2 境界処理ルール

| トリガー位置 | 処理 | 理由 |
|-------------|------|------|
| Core 領域内 | このスライスで処理 | 全ての coincident hits が含まれる |
| Overlap 領域内 | **次スライスで処理** | 重複防止 |

```
Slice N の処理範囲:
  Core:    [slice_start, slice_end - overlap)  ← トリガー処理対象
  Overlap: [slice_end - overlap, slice_end)    ← ヒットは含むがトリガーはスキップ
```

#### 6.2.3 アルゴリズム

```rust
fn process_slice(slice: &TimeSlice, triggers: &[Channel]) -> Vec<BuiltEvent> {
    let core_end = slice.end_ns - slice.overlap_ns;
    let mut events = Vec::new();

    for hit in slice.hits.iter() {
        // トリガーチャンネルでない場合はスキップ
        if !triggers.contains(&(hit.module, hit.channel)) {
            continue;
        }

        // Overlap 領域内のトリガーは次スライスで処理
        if hit.timestamp_ns >= core_end {
            continue;
        }

        // 先行トリガーチェック（時間優先）
        if has_prior_trigger_in_window(slice, hit) {
            continue;
        }

        // コインシデンスウィンドウ内のヒットを収集
        let event = build_event(slice, hit);
        events.push(event);
    }

    events
}
```

#### 6.2.4 並列処理

```
ファイル群 ──┬──▶ [Slice 0] ──▶ rayon ──▶ [Events 0]
             ├──▶ [Slice 1] ──▶ rayon ──▶ [Events 1]  ──▶ 結合 ──▶ ROOT
             ├──▶ [Slice 2] ──▶ rayon ──▶ [Events 2]
             └──▶ ...
```

**利点:**
- スライス単位で独立処理可能
- rayon による自動並列化
- メモリ使用量 = slice_duration × hit_rate × hit_size

#### 6.2.5 ダブルカウント防止

**方式: 時間優先 + オーバーラップ境界**

1. **時間優先:** 同一スライス内で先行トリガーがあればスキップ
2. **オーバーラップ境界:** Overlap 領域のトリガーは次スライスで処理

```
時間軸 ──────────────────────────────────────────▶
              T1        T2
              ●─────────●
              │◀─window─▶│

ケース1: T1, T2 が同一スライスの Core 内
  → T1 がイベント構築、T2 はスキップ（T1 が先行）

ケース2: T1 が Core 内、T2 が Overlap 内
  → T1 がイベント構築（現スライス）
  → T2 は次スライスで処理（先行トリガーなしなら構築）
```

### 6.3 Time Sort Buffer

**目的:** 非同期到着するヒットを時間順に整列

**実装:**
- BTreeMap<OrderedFloat<f64>, Vec<Hit>> で O(log n) 挿入・抽出
- watermark = 最新タイムスタンプ
- cutoff = watermark - buffer_delay
- cutoff より古いヒットを時間順で取り出し

**メモリ考慮:**
- 2 MHz × 1 秒 = 2M ヒット
- Hit サイズ ≈ 24 bytes
- バッファサイズ ≈ 48 MB (10GB 上限に対して十分な余裕)

---

## 7. ROOT TTree 入力形式 (ELIFANT-Event 互換)

### 7.1 TTree 構造

**TTree名:** `ELIADE_Tree`

| Branch | Type | 説明 |
|--------|------|------|
| `Mod` | UChar_t (u8) | モジュール ID |
| `Ch` | UChar_t (u8) | チャンネル ID |
| `TimeStamp` | ULong64_t (u64) | 粗タイムスタンプ |
| `FineTS` | Double_t (f64) | 精密タイムスタンプ [ns] |
| `ChargeLong` | UShort_t (u16) | 長ゲート積分 |
| `ChargeShort` | UShort_t (u16) | 短ゲート積分 |
| `RecordLength` | UInt_t (u32) | 波形長 (Event Builder では無視) |
| `Signal[RecordLength]` | UShort_t[] | 波形データ (Event Builder では無視) |

### 7.2 検証用データ

- **パス:** `/Users/aogaki/WorkSpace/ELIFANT2025/p91Zr/data/`
- **ファイル:** `run0113_XXXX_p_91Zr.root` (205 ファイル)
- **サイズ:** 約 390 MB/ファイル, 41M イベント/ファイル
- **実験:** p+91Zr

---

## 8. 参照ファイル

| 目的 | ファイル |
|------|---------|
| C++ L1 アルゴリズム | legacy/ELIFANT-Event/src/L1EventBuilder.cpp |
| C++ Time Calibration | legacy/ELIFANT-Event/src/TimeAlignment.cpp |
| C++ ChSettings | legacy/ELIFANT-Event/include/ChSettings.hpp |
| EventData 構造体 | src/common/mod.rs |
| Lock-free タスクパターン | src/recorder/mod.rs |
| DELILA v2 パーサー | src/bin/recover.rs |
| oxyroot 使用例 | tools/amax_viewer/src/main.rs |

---

## 9. 変更履歴

| 日付 | バージョン | 変更内容 |
|------|-----------|----------|
| 2026-02-02 | 1.0.0 | 初版作成 (Moving Time Window 方式) |
| 2026-02-02 | 2.0.0 | **Time Slice 方式に変更** — 並列処理対応、オーバーラップ境界処理 |
