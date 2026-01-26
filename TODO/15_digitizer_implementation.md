# Digitizer Implementation Plan

**Created:** 2026-01-23
**Status:** In Progress (Phase 1-4 Complete ✅, Phase 5 Master/Slave ✅)
**Spec:** `docs/digitizer_system_spec.md`

---

## Overview

VX2730 (DPP-PSD2) をMVPターゲットとして、Ethernet接続 (`dig2://`) でのデジタイザ制御を実装する。

**原則:**
1. **KISS** - 最小限の抽象化、動くコードを最短経路で
2. **TDD** - テストファーストで実装
3. **Clean Architecture** - 依存は内向き（KISSと競合する場合はKISS優先）

---

## Phase 1: FELib Connection Layer (基礎)

### 1.1 目標
- `dig2://` スキームでVX2730に接続・切断できる
- デバイス情報（モデル名、シリアル番号、ファームウェア）を取得できる

### 1.2 テストケース (TDD)
```rust
// tests/felib_connection_test.rs
#[test]
fn test_connect_disconnect() {
    let url = "dig2://172.18.4.56";
    let handle = FELibHandle::open(url).unwrap();
    assert!(handle.is_connected());
    drop(handle);  // RAII: Drop時に自動切断
}

#[test]
fn test_get_device_info() {
    let handle = FELibHandle::open(URL).unwrap();
    let info = handle.get_device_info().unwrap();
    assert_eq!(info.model, "VX2730");
    assert!(info.firmware.contains("PSD2"));
}

#[test]
fn test_invalid_url_returns_error() {
    let result = FELibHandle::open("dig2://invalid.host");
    assert!(matches!(result, Err(FELibError::ConnectionFailed(_))));
}
```

### 1.3 実装タスク
- [x] `src/reader/caen/handle.rs` - CaenHandleラッパー ✅
  - `open(url: &str) -> Result<Self, CaenError>`
  - `get_device_info() -> Result<DeviceInfo, CaenError>`
  - `is_connected() -> bool`
  - `Drop` trait for RAII cleanup
- [x] `src/reader/caen/error.rs` - エラー型 ✅
  - 全FELibエラーコード対応
- [x] 統合テスト: `tests/felib_integration_test.rs` ✅
  - 8テストケース（`#[ignore]`付き）
  - 実機テスト完了: VX2730 (Serial: 52622, DPP_PSD, 32ch, 14-bit)

### 1.4 依存関係
```
FELibHandle (Safe Rust)
    │
    ▼
ffi::CAEN_FELib_* (bindgen生成)
    │
    ▼
libCAEN_FELib.dylib/.so
```

---

## Phase 2: DevTree Read/Write (設定)

### 2.1 目標
- DevTreeからパラメータ値を読み取れる
- DevTreeにパラメータ値を書き込める
- パラメータのメタデータ（setinrun等）を取得できる

### 2.2 テストケース (TDD)
```rust
#[test]
fn test_read_parameter() {
    let handle = FELibHandle::open(URL).unwrap();
    let value: u32 = handle.read_param("/ch/0/par/DCOffset").unwrap();
    assert!(value <= 100);  // 0-100%
}

#[test]
fn test_write_parameter() {
    let handle = FELibHandle::open(URL).unwrap();
    handle.write_param("/ch/0/par/DCOffset", 50u32).unwrap();
    let value: u32 = handle.read_param("/ch/0/par/DCOffset").unwrap();
    assert_eq!(value, 50);
}

#[test]
fn test_get_parameter_metadata() {
    let handle = FELibHandle::open(URL).unwrap();
    let meta = handle.get_param_info("/ch/0/par/TriggerThr").unwrap();
    assert!(meta.setinrun);  // ランタイム変更可能
    assert_eq!(meta.datatype, "U32");
}
```

### 2.3 実装タスク
- [x] `src/reader/caen/handle.rs` - DevTree操作 ✅
  - `get_value(path: &str) -> Result<String, CaenError>`
  - `set_value(path: &str, value: &str) -> Result<(), CaenError>`
  - `get_param_info(path: &str) -> Result<ParamInfo, CaenError>`
  - `get_device_tree() -> Result<String, CaenError>` (JSON)
- [x] `ParamInfo` 構造体 ✅
  - name, datatype, access_mode, setinrun, min/max, allowed_values, unit
- [ ] 型安全パラメータアクセス（将来: 現在はString型で十分）

### 2.4 DevTreeパス例 (PSD2)
```
/par/startsource          # Board level (lowercase!)
/ch/0/par/dcoffset        # Channel level
/ch/0/par/triggerthr
/ch/0/par/gatelongt       # 't' suffix = time (ns)
/ch/0/par/gateshortt      # 't' suffix = time (ns)
/ch/0/par/chrecordlengtht # 't' suffix = time (ns)
/endpoint/raw/par/...     # Endpoint settings
```

**重要:** PSD2ファームウェアは**小文字**のパラメータ名を使用する。
- `s` suffix = サンプル数（例: `chrecordlengths`）
- `t` suffix = 時間(ns)（例: `chrecordlengtht`）
- ユーザーフレンドリーのため、`t` suffix（時間ベース）を優先使用する

---

## Phase 3: Configuration Storage & Apply (設定保存・適用)

### 3.1 目標
- **MongoDB**に設定を保存・読み込みできる（JSONファイルから移行）
- 設定のバージョン履歴を保持できる
- 設定をデジタイザに適用できる
- 適用結果（成功/失敗）を報告できる

### 3.2 MongoDB スキーマ

```javascript
// Collection: digitizer_configs (設定の信頼できるソース)
{
  _id: ObjectId,
  digitizer_id: 0,
  name: "LaBr3 Digitizer",
  url: "dig2://172.18.4.56",          // 接続URL
  firmware: "PSD2",
  num_channels: 32,
  is_master: false,                    // Master/Slave設定

  board: { ... },
  channel_defaults: { ... },
  channel_overrides: { "0": {...}, "5": {...} },

  updated_at: ISODate
}

// Collection: runs (既存を拡張 - Run開始時のスナップショット)
{
  run_number: 1,
  // ... existing fields ...
  digitizer_snapshots: [
    {
      digitizer_id: 0,
      config_snapshot: { /* 全設定のコピー */ }
    }
  ]
}
```

**Note:** 変更履歴は保持しない（前後Runのスナップショットで差分確認可能）

### 3.3 テストケース (TDD)

```rust
// MongoDB Storage Tests
#[tokio::test]
async fn test_save_config_to_mongodb() {
    let db = get_test_db().await;
    let config = DigitizerConfig::default();
    let repo = DigitizerConfigRepo::new(db);

    repo.save(&config).await.unwrap();

    let loaded = repo.get(config.digitizer_id).await.unwrap();
    assert_eq!(loaded.name, config.name);
}

#[tokio::test]
async fn test_save_run_snapshot() {
    let db = get_test_db().await;
    let run_repo = RunRepo::new(db.clone());
    let config_repo = DigitizerConfigRepo::new(db);

    // Get current configs
    let configs = config_repo.get_all().await.unwrap();

    // Create run with snapshots
    let run = run_repo.create_run(1, "test", &configs).await.unwrap();

    // Verify snapshots are stored
    assert_eq!(run.digitizer_snapshots.len(), configs.len());
}

// Hardware Apply Tests
#[test]
fn test_apply_board_config() {
    let handle = FELibHandle::open(URL).unwrap();
    let config = BoardConfig {
        start_source: "SWcmd".into(),
        record_length: 1024,
        ..Default::default()
    };
    handle.apply_board_config(&config).unwrap();
}

#[test]
fn test_apply_channel_config() {
    let handle = FELibHandle::open(URL).unwrap();
    let config = ChannelConfig {
        dc_offset: Some(50),
        trigger_threshold: Some(100),
        ..Default::default()
    };
    handle.apply_channel_config(0, &config).unwrap();
}

#[tokio::test]
async fn test_load_and_apply_config() {
    let db = get_test_db().await;
    let repo = DigitizerConfigRepo::new(db);

    let config = repo.get(0).await.unwrap();
    let handle = FELibHandle::open(&config.url).unwrap();
    let result = handle.apply_config(&config);
    assert!(result.is_ok());
}
```

### 3.4 実装タスク

**MongoDB Storage:** ✅
- [x] `src/operator/digitizer_repository.rs` - MongoDB CRUD操作
  - `DigitizerConfigRepository::new(client: &Client, database: &str)`
  - `save_config(config, created_by, description) -> Result<DigitizerConfigDocument>`
  - `get_current_config(digitizer_id) -> Result<Option<DigitizerConfigDocument>>`
  - `list_current_configs() -> Result<Vec<DigitizerConfigDocument>>`
  - `get_config_history(digitizer_id, limit) -> Result<Vec<DigitizerConfigDocument>>`
  - `restore_version(digitizer_id, version) -> Result<DigitizerConfigDocument>`
- [x] `create_run_snapshot()` - Run作成時に全デジタイザ設定をスナップショット
- [x] `src/operator/routes.rs` - API追加
  - `POST /api/digitizers/:id/save-to-db` - MongoDB に保存（バージョン履歴付き）
  - `GET /api/digitizers/:id/history` - バージョン履歴取得
  - `POST /api/digitizers/:id/restore` - 特定バージョン復元
  - `GET /api/runs/:run_number/config` - Run時のスナップショット取得
  - `POST /api/digitizers/save-all` - 全設定をファイルに保存
- [x] Run start時に自動スナップショット作成

**Hardware Apply:** ✅
- [x] `src/reader/caen/handle.rs` - `apply_config()` メソッド（既存）
  - `apply_config(&self, config: &DigitizerConfig) -> Result<usize, CaenError>`
- [x] Configure時にファイルから設定読み込み・適用（`src/reader/mod.rs`）
- [x] 適用フロー: API → Save to disk → Configure → Reader loads and applies

### 3.5 設定適用順序
```
1. MongoDBから設定を読み込み
2. Reset (オプション)
3. Board parameters (/par/*)
4. Channel defaults → 全チャンネルに適用
5. Channel overrides → 個別チャンネルに上書き
6. Endpoint settings (/endpoint/*)
```

### 3.6 API変更

| Endpoint | 変更内容 |
|----------|---------|
| `GET /api/digitizers` | MongoDB から取得 |
| `GET /api/digitizers/:id` | MongoDB から取得 |
| `PUT /api/digitizers/:id` | MongoDB に保存 + Running時はハードウェアにも適用 |
| `POST /api/digitizers` | **新規:** デジタイザ登録（Web UIから追加） |

**[Apply] ボタンの動作:**
- Not Running: `PUT /api/digitizers/:id` → MongoDBに保存
- Running: `PUT /api/digitizers/:id` → MongoDBに保存 + ハードウェア適用 (setinrun=true のみ)

---

## Phase 4: Data Acquisition (データ取得)

### 4.1 目標
- ArmしてRunning状態にできる
- イベントデータを読み取れる
- Stopして停止できる

### 4.2 テストケース (TDD)
```rust
#[test]
fn test_arm_start_stop() {
    let handle = FELibHandle::open(URL).unwrap();
    handle.apply_config(&config).unwrap();

    handle.arm().unwrap();
    assert_eq!(handle.state(), AcqState::Armed);

    handle.start().unwrap();
    assert_eq!(handle.state(), AcqState::Running);

    handle.stop().unwrap();
    assert_eq!(handle.state(), AcqState::Idle);
}

#[test]
fn test_read_events() {
    let handle = FELibHandle::open(URL).unwrap();
    // ... setup with test pulse enabled ...
    handle.arm().unwrap();
    handle.start().unwrap();

    let events = handle.read_events(100).unwrap();
    assert!(!events.is_empty());

    for event in &events {
        assert!(event.timestamp > 0);
        assert!(event.channel < 32);
    }

    handle.stop().unwrap();
}
```

### 4.3 実装タスク ✅

**データ取得制御:**
- [x] `src/reader/caen/handle.rs` - CaenHandle
  - `send_command("/cmd/armacquisition")` - Arm
  - `send_command("/cmd/swstartacquisition")` - Start
  - `send_command("/cmd/disarmacquisition")` - Stop
  - `configure_endpoint() -> EndpointHandle` - エンドポイント設定
- [x] `EndpointHandle` - データ読み取り
  - `has_data(timeout_ms) -> bool` - データ確認
  - `read_data(timeout_ms, buffer_size) -> Option<RawData>` - 生データ読み取り
- [x] `RawData` 構造体（data, size, n_events）

**イベントデコーダー:**
- [x] `src/reader/decoder/psd2.rs` - PSD2デコーダー
  - `Psd2Decoder::decode(&mut self, raw: &RawData) -> Vec<EventData>`
  - Start/Stop signal検出
  - 64-bit word format解析
  - タイムスタンプソート

**統合テスト:**
- [x] `tests/felib_integration_test.rs`
  - `test_configure_endpoint` - エンドポイント設定
  - `test_arm_disarm` - Arm/Disarm
  - `test_arm_start_stop` - フルサイクル
  - `test_read_data_with_test_pulse` - テストパルスでデータ取得
  - `test_decode_test_pulse_events` - デコード検証

### 4.4 データフロー
```
FELib ReadData
    │
    ▼
RawEvent (生バイト列)
    │
    ▼
EventData (共通フォーマット)
    │
    ▼
ZMQ PUB → Merger
```

---

## Phase 5: Reader Integration + Master/Slave (統合)

### 5.1 目標
- 既存のReaderフレームワークにDigitizerを統合
- Operatorからの制御（Configure/Arm/Start/Stop）に応答
- ZMQ経由でイベントを送信
- **Master/Slave 同期スタート（MVP必須）**
- **PSD1 対応（MVP必須）**

### 5.2 テストケース (TDD)
```rust
#[tokio::test]
async fn test_digitizer_reader_lifecycle() {
    let config = ReaderConfig {
        source_type: SourceType::Psd2,
        digitizer_id: 0,  // MongoDBから設定取得
        ..Default::default()
    };

    let reader = DigitizerReader::new(config).await.unwrap();

    reader.configure(RunConfig { run_number: 1, .. }).await.unwrap();
    assert_eq!(reader.state(), ComponentState::Configured);

    reader.arm().await.unwrap();
    assert_eq!(reader.state(), ComponentState::Armed);

    reader.start().await.unwrap();
    assert_eq!(reader.state(), ComponentState::Running);

    tokio::time::sleep(Duration::from_millis(100)).await;

    reader.stop().await.unwrap();
    assert_eq!(reader.state(), ComponentState::Configured);
}

#[tokio::test]
async fn test_master_slave_start_sequence() {
    // Master (PSD2) + Slave (PSD2) の同期スタート
    let master_config = DigitizerConfig { is_master: true, firmware: "PSD2", .. };
    let slave_config = DigitizerConfig { is_master: false, firmware: "PSD2", .. };

    // Arm all digitizers (parallel)
    // Start master only → Slaves auto-start via TrgOut cascade
}

#[tokio::test]
async fn test_psd1_master_start() {
    // PSD1がマスターの場合: Start時にArmする
    let master_config = DigitizerConfig { is_master: true, firmware: "PSD1", .. };
    // Arm = Start for PSD1
}
```

### 5.3 実装タスク

**Reader統合:**
- [x] `src/reader/mod.rs` - Reader already supports digitizer via CaenHandle ✅
  - 既存FFIを流用・拡張（新規作成不要）
  - 非同期イベント読み取りループ（2-task architecture）
- [x] Operator連携 ✅
  - `SourceType::Psd2`, `SourceType::Psd1` のハンドリング
  - Configure時にJSONファイルから設定を読み込み・適用
- [ ] 接続断検知（**自動再接続しない** - タイムスタンプ整合性のため）

**Master/Slave同期:** ✅
- [x] `is_master` フラグに基づくスタートシーケンス
  - `SourceNetworkConfig.is_master` - config.tomlで設定
  - `DigitizerConfig.is_master` + `sync` - JSONで詳細設定
  - Operator: Arm all → Start master only (slaves auto-start via TrgOut)
- [x] TrgOut/SIN カスケード設定
  - `SyncConfig` struct (trgout_source, sin_source, start_source)
  - `DigitizerConfig::new_master()`, `new_slave()` ヘルパー
  - `to_caen_parameters()` で自動生成
- [x] ComponentClient.start_all_sequential() 更新
  - Master/Slaveモード検出
  - Slave digitizersへのStart送信スキップ
- [x] 統合テスト追加
  - `test_master_sync_config`
  - `test_slave_sync_config`
  - `test_sync_parameters_readback`

### 5.4 アーキテクチャ
```
┌─────────────────────────────────────────────────────────┐
│                    DigitizerReader                       │
│                                                          │
│  ┌──────────────┐  mpsc   ┌──────────────┐             │
│  │ ReadLoop     │ ───────►│ ZMQ Sender   │             │
│  │ (FELib)      │ channel │              │             │
│  └──────────────┘         └──────────────┘             │
│         │                                               │
│         │ Arc<AtomicU64>                               │
│         ▼                                               │
│  ┌──────────────┐                                      │
│  │ Stats        │◄─── Metrics API                      │
│  └──────────────┘                                      │
│                                                          │
│  ┌──────────────┐                                      │
│  │ Command Task │◄─── ZMQ REP (Configure/Arm/Start/Stop)│
│  └──────────────┘                                      │
└─────────────────────────────────────────────────────────┘
```

---

## Phase 6: Web UI Settings (UI)

### 6.1 目標
- デジタイザ設定をブラウザから編集できる
- Basic/Advanced モードの切り替え
- ランタイム変更可能パラメータの即時適用

### 6.2 実装タスク
- [ ] Angular: DigitizerSettingsComponent拡張
  - チャンネルテーブルビュー（横: チャンネル、縦: パラメータ）
  - setinrun=true のパラメータのみ実行中編集可能
  - 単一 [Apply] ボタン（保存 + Running時はハードウェア適用）
- [ ] デジタイザ登録UI
  - [+ Add Digitizer] ボタン
  - URL入力 → 接続・検出 → MongoDB登録
- [ ] REST API拡張（Phase 3で実装済みのAPIを使用）
  - `PUT /api/digitizers/:id/param/:path` - 単一パラメータ変更（ランタイム）
- [ ] バリデーション
  - min/max 範囲チェック
  - enum値の選択肢表示

### 6.3 UIモックアップ
```
┌─────────────────────────────────────────────────────────┐
│ Digitizer: VX2730-001 (ID: 0)            [Basic ▼]     │
├─────────────────────────────────────────────────────────┤
│ Board Settings                                          │
│   Start Source: [SWcmd ▼]   Test Pulse: [Off ▼]        │
│   Record Length: [1024    ]                             │
├─────────────────────────────────────────────────────────┤
│ Channel Settings                                        │
│ ┌─────┬───────┬───────┬───────┬───────┬─────┐         │
│ │     │ Ch 0  │ Ch 1  │ Ch 2  │ Ch 3  │ ... │         │
│ ├─────┼───────┼───────┼───────┼───────┼─────┤         │
│ │ En  │  [✓]  │  [✓]  │  [✓]  │  [ ]  │     │         │
│ │ DC% │  50   │  50   │  45   │  50   │     │         │
│ │ Thr │  100  │  100  │  150  │  100  │     │         │
│ └─────┴───────┴───────┴───────┴───────┴─────┘         │
├─────────────────────────────────────────────────────────┤
│                                     [Reset] [Apply]     │
│  ※ Apply = MongoDBに保存 (+ Running時はハードウェア適用) │
└─────────────────────────────────────────────────────────┘
```

---

## Phase 7: Future Enhancements (将来)

### 7.1 Configuration Templates
- [ ] デフォルトテンプレート機能
  - 実験タイプ別のプリセット（NaI, LaBr3, HPGe等）
  - ユーザー定義テンプレート
- [ ] インポート/エクスポート

### 7.2 Hardware Monitoring
- [ ] 温度監視（定期ポーリング）
- [ ] 電圧監視
- [ ] Monitor UIへの表示

### 7.3 Dynamic Digitizer Management
- [ ] 再起動なしでのデジタイザ追加
- [ ] トポロジー管理UI
- [ ] ホットプラグ検出

---

## Implementation Order (推奨順序)

```
Phase 1 (FELib Connection)
    │
    ▼
Phase 2 (DevTree Read/Write)
    │
    ▼
Phase 3 (Configuration Storage & Apply)
    │
    ▼
Phase 4 (Data Acquisition)
    │
    ▼
Phase 5 (Reader + Master/Slave + PSD1)  ←── MVP完了ライン
    │
    ▼
Phase 6 (Web UI Settings)
    │
    ▼
Phase 7 (Future: Templates, Monitoring, Dynamic)
```

**MVP範囲:** Phase 1〜5（Master/Slave同期、PSD1対応含む）
**Post-MVP:** Phase 6〜7

---

## Testing Strategy

### Unit Tests
- 各Phase内のモジュール単体テスト
- モック使用でFELib依存を排除（CI対応）

### Integration Tests
- `#[ignore]` 付きで実機テスト
- `cargo test -- --ignored` で実機接続時のみ実行

### E2E Tests
- Emulator + Digitizer混在環境でのパイプラインテスト
- 手動テスト手順書

---

## Files to Create/Modify

### New Files
```
src/reader/caen/
├── felib_handle.rs      # Phase 1
├── felib_error.rs       # Phase 1
├── devtree.rs           # Phase 2
├── config_apply.rs      # Phase 3
├── acquisition.rs       # Phase 4
└── mod.rs               # エクスポート

src/reader/
└── digitizer.rs         # Phase 5

tests/
├── felib_connection_test.rs  # Phase 1
├── devtree_test.rs           # Phase 2
└── digitizer_integration_test.rs  # Phase 5
```

### Modified Files
```
src/reader/mod.rs        # DigitizerReader追加
src/operator/client.rs   # SourceType::Psd2 ハンドリング
config/digitizer_0.json  # サンプル設定
```

---

## Notes

- **KISS:** 過度な抽象化を避ける。動くコードを優先。
- **TDD:** 各Phaseでテストを先に書く。
- **実機テスト:** `dig2://172.18.4.56` （VX2730, Serial: 52622, DPP_PSD, 32ch, 14-bit）

## References

| Document | Location | Description |
|----------|----------|-------------|
| **x2730 DPP-PSD CUP Documentation** | `legacy/documentation_2024092000-2/` | **公式ドキュメント (v2024092000)** |
| FELib User Guide | `legacy/GD9764_FELib_User_Guide.pdf` | FELib API詳細 |
| C++ Reference | `DELILA2/` | 既存実装参考 |
| System Spec | `docs/digitizer_system_spec.md` | 設計仕様 |

### CUP Documentation Structure

| File | Contents |
|------|----------|
| `index.html` | Introduction, parameter paths, levels |
| `a00101.html` | Commands (Reset, Arm, Start, Stop, etc.) |
| `a00102.html` | Endpoints (Raw, DPPPSD, Stats), Event Flags |
| `a00103.html` | **全パラメータ詳細仕様** |
