# Digitizer System Specification

**Document Version:** 1.0
**Last Updated:** 2026-01-23
**Status:** Draft

---

## 1. Overview

### 1.1 Purpose

DELILA-RS DAQシステムにおけるデジタイザ制御サブシステムの詳細仕様書。
Web UIからのパラメータ設定、ハードウェア制御、状態監視の全体設計を定義する。

### 1.2 Scope

- CAEN デジタイザの設定・制御
- Web UI による設定管理
- MongoDB によるバージョン管理
- マスター/スレーブ同期制御

---

## 2. Hardware Support

### 2.1 Supported Digitizers

| Model | Firmware | Library | Channels | Status |
|-------|----------|---------|----------|--------|
| VX2730 | DPP-PSD2 | CAEN_FELib | 32 | **MVP Target** |
| VX2745 | DPP-PSD2 | CAEN_FELib | 64 | Future |
| VX2740 | DPP-PSD2 | CAEN_FELib | 64 | Future |
| VX2730 | DPP-PHA2 | CAEN_FELib | 32 | Future (CAEN未リリース) |
| x725 | DPP-PSD1 | CAEN_FELib | 8/16 | **Planning** |
| x730 | DPP-PSD1 | CAEN_FELib | 8/16 | **Planning** |
| x725 | DPP-PHA1 | CAEN_FELib | 8/16 | Future |
| x730 | DPP-PHA1 | CAEN_FELib | 8/16 | Future |
| VX1743 | Custom | CAEN Digitizer Library | 16 | Future (※1) |

**※1 VX1743について:**
- 特殊ファームウェアで位置検出計算を実行
- イベントビルダーとの統合が必要なため最後に実装

### 2.2 Connection Interfaces

FELibは2層構造で、スキームによって使用ライブラリが異なる（GD9764 Rev.2 参照）:
- **`dig2://`** → CAEN Dig2 ライブラリ（Digitizer 2.0: x27xx系）
- **`dig1://`** → CAEN Dig1 ライブラリ（Digitizer 1.0: x17xx, DT57xx系）

#### DIG2 接続（Digitizer 2.0: x2730, x2740, x2745）

| Interface | URL Example | Notes |
|-----------|------------|-------|
| Ethernet (IPv4) | `dig2://172.18.4.56` | **MVP** - IP直指定推奨 |
| Ethernet (IPv6) | `dig2://[2001:db8::1]` | |
| Ethernet (mDNS) | `dig2://caendgtz-eth-<pid>` | OS依存（Linuxは`.local`要） |
| USB 3.0 | `dig2://caendgtz-usb-<pid>` | `<pid>` = シリアル番号 |
| USB 3.0 (alt) | `dig2://caen.internal/usb/<pid>` | 同上、別形式 |
| OpenARM (embedded) | `dig2://caen.internal/openarm` | Docker内部IP 172.17.0.1 相当 |

#### DIG1 接続（Digitizer 1.0: x725, x730, DT5730 等）

authorityは `/eth_v4718` 以外すべて `caen.internal`。

| Interface | URL Example | Notes |
|-----------|------------|-------|
| USB (Direct) | `dig1://caen.internal/usb?link_num=<num>` | USB 2.0直結 |
| Optical Link | `dig1://caen.internal/optical_link?link_num=<num>` | CONET2（A3818等） |
| USB A4818 | `dig1://caen.internal/usb_a4818?link_num=<pid>` | USB-CONET2ブリッジ |
| A4818 + V2718 | `dig1://caen.internal/usb_a4818_v2718?link_num=<pid>&conet_node=<n>&vme_base_address=<addr>` | VME経由 |
| A4818 + V3718 | `dig1://caen.internal/usb_a4818_v3718?...` | VME経由 |
| A4818 + V4718 | `dig1://caen.internal/usb_a4818_v4718?...` | VME経由 |
| ETH V4718 | `dig1://<IP>/eth_v4718` | authorityはV4718のIP |
| USB V4718 | `dig1://caen.internal/usb_v4718?link_num=<pid>` | |

**クエリパラメータ（dig1）:**
- `link_num=<num>` — リンク番号（A4818/USB V4718ではPID）
- `conet_node=<num>` — CONETノード番号
- `vme_base_address=<addr>` — VMEベースアドレス（例: `0x32100000`）

**接続制限:** 同一ホスト名への接続は1つのみ。別インスタンスからOpenすると既存接続を強制切断。

**開発・テスト方針:**
- **MVP開発:** Ethernet接続 (`dig2://`) + USB接続 (`dig1://`, `dig2://`) を使用
- **Optical link:** Linux/Windowsマシンで実施（CAEN_DIG1ライブラリ必須）

### 2.3 Scale

- **Maximum digitizers:** 34 units
- **Maximum channels:** 34 × 16 = 544 channels (typical case)
- **Maximum event rate:** ~10 MHz (system total)
- **Note:** 実際のスループットはデジタイザのハードウェア性能に依存

---

## 3. Configuration Management

### 3.1 Source of Truth

**MongoDB が設定の唯一の信頼できるソース (Single Source of Truth)**

```
┌─────────────────────────────────────────────────────────────┐
│                        MongoDB                               │
│  ┌─────────────────┐  ┌─────────────────┐                  │
│  │ digitizer_configs│  │     runs        │                  │
│  │ (current config) │  │ (config snapshot)│                  │
│  └─────────────────┘  └─────────────────┘                  │
└─────────────────────────────────────────────────────────────┘
                    ↑
                    │ Web UI / Operator
                    │
┌─────────────────────────────────────────────────────────────┐
│ config.toml: システムトポロジーのみ                         │
│   - pipeline_order, bind addresses                          │
│   - デジタイザ設定は含まない（MongoDBから取得）             │
└─────────────────────────────────────────────────────────────┘
```

### 3.2 Digitizer Registration

**新規デジタイザ追加フロー:**
1. Web UI Settings → [+ Add Digitizer]
2. URL入力 (`dig2://172.18.4.56`)
3. [Connect & Detect] → モデル/FW自動検出
4. MongoDB に登録 + デフォルト設定作成
5. **Operator 再起動必要**（動的追加は将来実装）

### 3.3 MongoDB Schema

```javascript
// Collection: digitizer_configs (設定の信頼できるソース)
{
  _id: ObjectId,
  digitizer_id: 0,
  name: "LaBr3 Digitizer #1",
  url: "dig2://172.18.4.56",           // 接続URL
  firmware: "PSD2",
  num_channels: 32,
  is_master: false,                     // Master/Slave設定

  // Board settings
  board: {
    start_source: "SWcmd",
    global_trigger_source: "ITLA",
    test_pulse_period: 100000,
    test_pulse_width: 1000,
    record_length: 1024
  },

  // Channel settings
  channel_defaults: {
    enabled: "True",
    dc_offset: 50.0,
    polarity: "Negative",
    trigger_threshold: 100,
    gate_long_ns: 400,
    gate_short_ns: 100
  },
  channel_overrides: {
    "0": { trigger_threshold: 50 },
    "15": { enabled: "False" }
  },

  // Metadata
  updated_at: ISODate,
  is_template: false
}

// Collection: runs (既存を拡張)
{
  _id: ObjectId,
  run_number: 1,
  // ... existing fields ...

  // Run開始時のスナップショット（解析時に設定確認用）
  digitizer_snapshots: [
    {
      digitizer_id: 0,
      config_snapshot: { /* 全設定のコピー */ }
    }
  ]
}
```

**Note:** 変更履歴は保持しない（前後のRunスナップショットで差分確認可能）

### 3.4 Apply Button Behavior

**単一の [Apply] ボタン:**

| システム状態 | [Apply] の動作 |
|-------------|---------------|
| Not Running | MongoDBに保存 → 次回Configure時に使用 |
| Running | ハードウェアに適用 (setinrun=true のみ) + MongoDBに保存 |

**重要:** Applyは常にMongoDBに保存する。Run中の変更も次回Runで使われる。

---

## 4. DevTree Integration

### 4.1 DevTree Structure

DevTreeはデジタイザの全パラメータを階層的に記述するJSON構造。

```json
{
  "par": {
    "StartSource": {
      "accessmode": { "value": "READ_WRITE" },
      "datatype": { "value": "ENUM" },
      "setinrun": { "value": "false" },
      "description": { "value": "Start acquisition source" },
      "allowedvalues": ["SWcmd", "ITLA", "GPIO", "SIN", "LVDS"],
      "value": "SWcmd"
    },
    "GlobalTriggerSource": { ... }
  },
  "ch": {
    "0": {
      "par": {
        "TriggerThr": {
          "accessmode": { "value": "READ_WRITE" },
          "datatype": { "value": "NUMBER" },
          "setinrun": { "value": "true" },
          "minvalue": { "value": "0" },
          "maxvalue": { "value": "16383" },
          "increment": { "value": "1" },
          "value": "100"
        }
      }
    }
  }
}
```

### 4.2 Key DevTree Fields

| Field | Description | Use in UI |
|-------|-------------|-----------|
| `accessmode` | READ_ONLY, READ_WRITE | 編集可否 |
| `setinrun` | true/false | Run中変更可否 |
| `datatype` | NUMBER, ENUM, STRING | 入力タイプ |
| `minvalue/maxvalue` | 数値範囲 | バリデーション |
| `increment` | ステップ値 | スライダー刻み |
| `allowedvalues` | ENUMの選択肢 | ドロップダウン |
| `description` | パラメータ説明 | ツールチップ |
| `uom/expuom` | 単位と指数 | 表示単位 |

### 4.3 DevTree Caching Strategy

```
Operator起動
    ↓
各デジタイザに接続
    ↓
FW type + version を取得
    ↓
キャッシュに同一FW+versionあり？
    ├─ Yes → キャッシュを使用
    └─ No  → DevTree取得してキャッシュ保存
```

**キャッシュキー:** `{firmware_type}_{firmware_version}` (例: `PSD2_1.0.57`)

---

## 5. State Machine & Control Flow

### 5.1 Component States

```
          ┌─────────────────────────────────────────────┐
          │                                             │
          ▼                                             │
       ┌──────┐  configure   ┌────────────┐           │
       │ Idle │ ──────────►  │ Configured │           │
       └──────┘              └────────────┘           │
          ▲                       │                   │
          │ reset                 │ arm               │
          │                       ▼                   │
          │                  ┌─────────┐              │
          │                  │  Armed  │              │
          │                  └─────────┘              │
          │                       │                   │
          │                       │ start             │
          │                       ▼                   │
          │                  ┌─────────┐              │
          └────────────────  │ Running │ ─────────────┘
                    stop     └─────────┘
```

### 5.2 Configure Sequence

```
User clicks [Configure]
    │
    ▼
┌─────────────────────────────────────────────────────────┐
│ For each digitizer (PARALLEL):                          │
│   1. Connect to hardware (FELib)                        │
│   2. Load config from MongoDB                           │
│   3. Apply parameters via SetValue()                    │
│   4. Verify applied values (optional GetValue)          │
│   5. Report success/failure                             │
└─────────────────────────────────────────────────────────┘
    │
    ▼
All success? ──No──► Show error dialog, ask user
    │                     │
    │ Yes                 ▼
    ▼               User chooses: Retry / Skip / Abort
State → Configured
```

### 5.3 Start Sequence (Master/Slave)

**重要:** PSD1とPSD2が混在するシステムでも、マスターは1台のみ。

```
User clicks [Start]
    │
    ▼
┌─────────────────────────────────────────────────────────┐
│ Step 1: Arm Phase                                       │
│   - PSD2: Arm ALL digitizers (parallel)                 │
│   - PSD1: Skip (auto-starts on Arm)                     │
│   - Wait for all PSD2 to reach Armed state              │
├─────────────────────────────────────────────────────────┤
│ Step 2: Start Phase                                     │
│   - If Master is PSD2: Start Master only                │
│   - If Master is PSD1: Arm Master                       │
│   - Slaves auto-start via TrgOut/GPIO/SIN cascade       │
└─────────────────────────────────────────────────────────┘
    │
    ▼
State → Running
```

**クロック同期:** 全デジタイザはマスタークロックを共有（外部クロック配信）

### 5.4 Signal Cascade Configuration

DELILA2/PSD2.conf, PSD1.conf を参照。
TrgOut → SIN のカスケード接続でスタート信号を伝搬。

```
[Master]  ──TrgOut──►  [Slave1]  ──TrgOut──►  [Slave2]  ──► ...
             │             │
             └─────────────┴─► SIN input on each slave
```

---

## 6. Web UI Specification

### 6.1 Settings Page Structure

```
Settings Page
├── Tab: Digitizers
│   ├── Digitizer Selector (dropdown)
│   ├── Connection Status (Online/Offline/Error)
│   ├── Hardware Info (model, serial, FW version)
│   ├── Board Settings Card
│   │   ├── Start Source
│   │   ├── Global Trigger Source
│   │   ├── Test Pulse Period/Width
│   │   └── [Advanced...] expandable
│   ├── Channel Defaults Card
│   │   ├── DC Offset, Polarity, Threshold
│   │   ├── Gate Long/Short
│   │   └── Trigger Sources
│   ├── Channel Overrides Card
│   │   ├── Channel chips (click to add override)
│   │   └── Expansion panels per channel
│   ├── Hardware Status Card
│   │   ├── Temperature
│   │   ├── Voltage
│   │   ├── PLL Lock Status
│   │   └── Acquisition Status
│   └── Action Buttons
│       ├── [Reset] - Reload from file
│       ├── [Apply] - Send to hardware (if Running/Configured)
│       └── [Save]  - Save to file/MongoDB
│
├── Tab: Templates
│   ├── Template list
│   ├── Apply template to digitizer
│   └── Create template from current config
│
└── Tab: Emulator (existing)
```

### 6.2 Parameter Display Modes

| Mode | Target User | Displayed Parameters |
|------|-------------|---------------------|
| Basic | General users | Essential parameters (~15 items) |
| Advanced | Power users | All parameters from DevTree |
| Custom | Configurable | User-selected parameters |

**切り替え:** Settings icon → Parameter display mode

#### Basic Mode Parameters (PSD2)

| Category | Parameters |
|----------|------------|
| Input | DC Offset, Polarity, Gain |
| Trigger | Trigger Method (LED/CFD), Trigger Threshold |
| CFD | CFD Delay, CFD Fraction |
| Gate | Gate Long, Gate Short |
| Processing | Smoothing Factor |
| Waveform | Analog Probe 1/2, Digital Probe 1/2 |
| Control | Channel Enable, Event Trigger Source |

**Waveform Probe設定例:**
- Analog Probe 1: Input Signal
- Analog Probe 2: CFD Output
- Digital Probe 1: Long Gate
- Digital Probe 2: Short Gate

### 6.3 Channel Display Layout

**Basic View (テーブル形式):**
```
           Ch0   Ch1   Ch2   Ch3   ... Ch31
─────────────────────────────────────────────
Enable     [✓]   [✓]   [✓]   [ ]   ...
DC Offset  50    50    45    50    ...
Threshold  100   100   150   100   ...
Gate Long  400   400   400   400   ...
Gate Short 100   100   100   100   ...
─────────────────────────────────────────────
```

- 横軸: チャンネル (0-31)
- 縦軸: パラメータ
- 各セルは編集可能
- デザインは実装後にテストしながら調整

### 6.3 Runtime Parameter Editing

```
Run中のパラメータ変更:

1. setinrun=true のパラメータのみ編集可能
2. 変更 → [Apply] → 即座にハードウェアに適用
3. UI表示:
   - setinrun=true: 通常の入力フィールド
   - setinrun=false: Disabled (グレーアウト) + tooltip説明
```

### 6.4 Hardware Status Display

```typescript
interface DigitizerStatus {
  connected: boolean;
  state: 'Idle' | 'Configured' | 'Armed' | 'Running' | 'Error';

  // Monitoring (read from /mon/ path in DevTree)
  temperature_celsius: number;
  voltage_vccint: number;
  voltage_vccaux: number;
  pll_lock_status: boolean;
  acquisition_status: string;

  // Connection info
  firmware_type: string;
  firmware_version: string;
  serial_number: string;
  model_name: string;
}
```

---

## 7. API Specification

### 7.1 Digitizer Config API

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/digitizers` | List all configs |
| GET | `/api/digitizers/:id` | Get specific config |
| PUT | `/api/digitizers/:id` | Update config (memory) |
| POST | `/api/digitizers/:id/save` | Save to disk |
| POST | `/api/digitizers/:id/apply` | Apply to hardware |
| GET | `/api/digitizers/:id/status` | Get hardware status |
| GET | `/api/digitizers/:id/devtree` | Get DevTree |

### 7.2 Template API

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/templates` | List templates |
| POST | `/api/templates` | Create template |
| POST | `/api/templates/:name/apply/:digitizer_id` | Apply template |

### 7.3 Config Version API (MongoDB)

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/digitizers/:id/versions` | List versions |
| GET | `/api/digitizers/:id/versions/:version` | Get specific version |
| POST | `/api/digitizers/:id/versions` | Save new version |
| POST | `/api/digitizers/:id/rollback/:version` | Rollback to version |

---

## 8. Error Handling

### 8.1 Configuration Errors

| Error Type | Handling |
|------------|----------|
| Connection failed | Show error, offer retry |
| Parameter rejected | Show which param failed, ask user |
| Partial failure | List failed params, ask Continue/Abort |
| Timeout | Show error, offer retry |

### 8.2 Runtime Errors

| Error Type | Handling |
|------------|----------|
| Connection lost | Stop acquisition, show error, require user intervention |
| Parameter change failed | Show error, revert UI to current value |
| Hardware error | Show diagnostic info, stop acquisition |

**重要:** 接続断時の自動再接続は行わない（タイムスタンプ整合性のため）

---

## 9. Implementation Phases

### Phase 1: MVP (March 2026)

**必須機能:**
- [ ] VX2730 (PSD2) Ethernet接続 (`dig2://`)
- [ ] x725/x730 (PSD1) 対応
- [ ] MongoDB 設定管理（JSONファイルから移行）
- [ ] Configure-time パラメータ適用
- [ ] **Master/Slave 同期スタート**（必須）
- [ ] Run開始時の設定スナップショット
- [ ] Web UI: 単一[Apply]ボタン（保存+適用）
- [ ] Runtime パラメータ変更（setinrun=true）

### Phase 2: Enhanced (April 2026)

- [ ] テンプレートシステム
- [ ] 温度/電圧モニタリング
- [ ] DevTree-based 動的UI（Advancedモード）
- [ ] 動的デジタイザ追加（Operator再起動不要）

### Phase 3: Extended Hardware (May+ 2026)

- [ ] VX1743 (CAEN Digitizer Library)
- [ ] USB/Optical link support
- [ ] Advanced monitoring (temperature, voltage plots)

---

## 10. References

- **x2730 DPP-PSD CUP Documentation (v2024092000):** `legacy/documentation_2024092000-2/`
  - `index.html` - Introduction, parameter structure, levels
  - `a00101.html` - Supported Commands
  - `a00102.html` - Supported Endpoints (Raw, DPPPSD, Stats)
  - `a00103.html` - Supported Parameters (全パラメータ詳細)
- **PSD1 Decoder Specification:** `docs/psd1_decoder_spec.md`
- CAEN FELib User Guide: `legacy/GD9764_FELib_User_Guide.pdf`
- DELILA2 C++ Implementation (`DELILA2/PSD2.conf`, `DELILA2/PSD1.conf`)
- DevTree JSON examples: `docs/devtree_examples/`

---

## Appendix A: FELib Commands (VX2730 DPP-PSD)

Path: `/cmd/<CommandName>`

| Command | Level | SetInRun | Description |
|---------|-------|----------|-------------|
| Reset | DIG | No | ボードをリセット（レジスタをデフォルトに） |
| Reboot | DIG | Yes | 4秒後にボードを再起動 |
| ClearData | DIG | No | メモリからデータをクリア |
| ArmAcquisition | DIG | No | 取得をアーム |
| DisarmAcquisition | DIG | Yes | 取得をディスアーム |
| SwStartAcquisition | DIG | Yes | ソフトウェアで取得開始 |
| SwStopAcquisition | DIG | Yes | 取得を強制停止 |
| SendSWTrigger | DIG | Yes | ソフトウェアトリガー送信 |
| SendChSWTrigger | CH | Yes | チャンネル別ソフトウェアトリガー |
| ReloadCalibration | DIG | Yes | キャリブレーションを再読込 |

---

## Appendix B: Data Endpoints

### Raw Endpoint (`/endpoint/raw`)

生データ形式。Big-endian 64-bit words。

| Field | Type | Description |
|-------|------|-------------|
| DATA | U8[] | Raw Data |
| SIZE | SIZE_T | データサイズ |
| N_EVENTS | U32 | イベント数 |

### DPPPSD Endpoint (`/endpoint/dpppsd`)

デコード済みイベント形式。

| Field | Type | Description |
|-------|------|-------------|
| CHANNEL | U8 | チャンネル番号 (7 bits) |
| TIMESTAMP | U64 | タイムスタンプ (48 bits, 1 LSB = 8 ns) |
| TIMESTAMP_NS | DOUBLE | タイムスタンプ (ns) |
| FINE_TIMESTAMP | U16 | ファインタイムスタンプ (10 bits, 1 LSB = 7.8125 ps) |
| ENERGY | U16 | エネルギー (Long gate) |
| ENERGY_SHORT | U16 | Short gate エネルギー |
| FLAGS_LOW_PRIORITY | U16 | Low priority flags (12 bits) |
| FLAGS_HIGH_PRIORITY | U16 | High priority flags (8 bits) |
| ANALOG_PROBE_1 | I32[] | Analog probe 1 波形 |
| ANALOG_PROBE_2 | I32[] | Analog probe 2 波形 |
| DIGITAL_PROBE_1-4 | U8[] | Digital probe 波形 |
| WAVEFORM_SIZE | SIZE_T | 波形サンプル数 |
| BOARD_FAIL | BOOL | ボードエラーフラグ |

### Stats Endpoint (`/endpoint/stats`)

統計情報。

| Field | Type | Description |
|-------|------|-------------|
| REAL_TIME | U64 | リアルタイム (clock steps) |
| REAL_TIME_NS | U64 | リアルタイム (ns) |
| DEAD_TIME | U64 | デッドタイム (clock steps) |
| DEAD_TIME_NS | U64 | デッドタイム (ns) |
| LIVE_TIME | U64 | ライブタイム (clock steps) |
| LIVE_TIME_NS | U64 | ライブタイム (ns) |
| TRIGGER_CNT | U32 | トリガーカウント |
| SAVED_EVENT_CNT | U32 | 保存イベントカウント |

---

## Appendix C: Event Flags

### High Priority Flags

| Bit | Name | Description |
|-----|------|-------------|
| 0 | Pile-Up | パイルアップイベント |
| 2 | Event Saturation | 入力ダイナミクス飽和 |
| 3 | Post saturation event | ADCVetoWidth時間中のイベント |
| 4 | Charge overflow | 積分電荷オーバーフロー |
| 5 | SCA selected event | SCAウィンドウ内イベント |
| 6 | Event with fine timestamp | ファインタイムスタンプ計算済み |

### Low Priority Flags

| Bit | Name | Description |
|-----|------|-------------|
| 0 | External inhibit | 外部インヒビット中の波形 |
| 1 | Under-saturation | アンダーサチュレーション |
| 2 | Oversaturation | オーバーサチュレーション |
| 3 | External trigger | TRG-INからの外部トリガー |
| 4 | Global trigger | グローバルトリガー条件 |
| 5 | Software trigger | ソフトウェアトリガー |
| 6 | Self trigger | チャンネルセルフトリガー |
| 7 | LVDS trigger | LVDSコネクタからの外部トリガー |
| 8 | 64 channel trigger | 他チャンネル組合せトリガー |
| 9 | ITLA trigger | ITLAロジックトリガー |
| 10 | ITLB trigger | ITLBロジックトリガー |

---

## Appendix D: Parameter Naming Convention (PSD2)

**重要:** PSD2ファームウェアは**小文字**のパラメータ名を使用する。

| Suffix | Meaning | Example |
|--------|---------|---------|
| (なし) | 基本パラメータ | `dcoffset`, `triggerthr` |
| `s` | サンプル数 | `chrecordlengths`, `gatelonglengths` |
| `t` | 時間 (ns) | `chrecordlengtht`, `gatelonglengtht` |

**推奨:** ユーザーフレンドリーのため `t` suffix（時間ベース）を優先使用する。

---

## Appendix E: DevTree Example (VX2730 PSD2)

保存場所: `docs/devtree_examples/vx2730_psd2_v1.0.57.json`

## Appendix B: Master/Slave Wiring Diagram

```
                    ┌─────────────┐
                    │   Master    │
                    │   VX2730    │
                    └──────┬──────┘
                           │ TrgOut
                           ▼
              ┌────────────┴────────────┐
              │                         │
        ┌─────▼─────┐             ┌─────▼─────┐
        │  Slave 1  │             │  Slave 2  │
        │  VX2730   │             │  VX2730   │
        └─────┬─────┘             └───────────┘
              │ TrgOut
              ▼
        ┌───────────┐
        │  Slave 3  │
        │  VX2730   │
        └───────────┘
```
