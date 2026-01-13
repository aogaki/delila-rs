# DELILA-RS アーキテクチャ設計: 設定管理とデプロイメント

**Status**: Draft (叩き台)
**Date**: 2026-01-13
**Author**: Aogaki + Claude

---

## 1. 概要

DELILA-RSは2つのデプロイメントモードをサポートする：

| モード | 用途 | プロセス | 設定保存 |
|--------|------|----------|----------|
| **スタンドアローン** | 小規模実験、テスト、教育 | 単一（マルチスレッド） | メモリ + ローカルファイル |
| **分散システム** | 大規模実験、本番運用 | 複数（ZMQ接続） | MongoDB |

両モードで**同じコアロジック**と**同じUI**を使用し、設定ストレージのみが異なる。

---

## 2. 統一アーキテクチャ

### 2.1 レイヤー構成

```
┌─────────────────────────────────────────────────────────────────┐
│                        Frontend Layer                            │
│                                                                  │
│   ┌─────────────────────────────────────────────────────────┐   │
│   │              Web UI (Angular/React/Svelte)              │   │
│   │                                                          │   │
│   │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐   │   │
│   │  │ Config   │ │ Run      │ │ Monitor  │ │ History  │   │   │
│   │  │ Editor   │ │ Control  │ │ Dashboard│ │ Viewer   │   │   │
│   │  └──────────┘ └──────────┘ └──────────┘ └──────────┘   │   │
│   └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼ HTTP/WebSocket (REST API)
┌─────────────────────────────────────────────────────────────────┐
│                         API Layer                                │
│                                                                  │
│   ┌─────────────────────────────────────────────────────────┐   │
│   │                    REST API (OpenAPI)                    │   │
│   │                                                          │   │
│   │  POST /api/config/digitizer/{id}   - 設定更新            │   │
│   │  GET  /api/config/digitizer/{id}   - 設定取得            │   │
│   │  POST /api/control/start           - Run開始             │   │
│   │  POST /api/control/stop            - Run停止             │   │
│   │  GET  /api/status                  - 状態取得            │   │
│   │  WS   /api/events                  - リアルタイム監視    │   │
│   └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                       Service Layer                              │
│                                                                  │
│   ┌──────────────┐ ┌──────────────┐ ┌──────────────────────┐   │
│   │ ConfigService│ │ RunService   │ │ MonitorService       │   │
│   │              │ │              │ │                      │   │
│   │ - get/set    │ │ - start/stop │ │ - metrics            │   │
│   │ - validate   │ │ - state mgmt │ │ - live events        │   │
│   └──────┬───────┘ └──────────────┘ └──────────────────────┘   │
│          │                                                       │
│          ▼                                                       │
│   ┌──────────────────────────────────────────────────────────┐  │
│   │              ConfigStore (trait)                          │  │
│   │                                                           │  │
│   │  ┌─────────────────┐        ┌─────────────────────┐      │  │
│   │  │ InMemoryStore   │   OR   │ MongoConfigStore    │      │  │
│   │  │ (スタンドアローン) │        │ (分散システム)       │      │  │
│   │  └─────────────────┘        └─────────────────────┘      │  │
│   └──────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                        Core Layer                                │
│                                                                  │
│   ┌──────────────┐ ┌──────────────┐ ┌──────────────────────┐   │
│   │ Reader       │ │ Decoder      │ │ DataPipeline         │   │
│   │              │ │              │ │                      │   │
│   │ - CAEN FFI   │ │ - PSD2       │ │ - Merger             │   │
│   │ - Endpoint   │ │ - PSD1       │ │ - Recorder           │   │
│   │ - ReadLoop   │ │ - PHA1       │ │ - Monitor            │   │
│   └──────────────┘ └──────────────┘ └──────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

### 2.2 デプロイメント比較

```
┌─────────────────────────────────────────────────────────────────┐
│                    スタンドアローンモード                         │
│                                                                  │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                   Single Process                          │  │
│  │                                                            │  │
│  │   ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐     │  │
│  │   │ UI      │  │ API     │  │ Reader  │  │Recorder │     │  │
│  │   │ Thread  │  │ Thread  │  │ Thread  │  │ Thread  │     │  │
│  │   └────┬────┘  └────┬────┘  └────┬────┘  └────┬────┘     │  │
│  │        │            │            │            │           │  │
│  │        └────────────┴─────┬──────┴────────────┘           │  │
│  │                           │                                │  │
│  │                   ┌───────▼───────┐                       │  │
│  │                   │ Shared State  │                       │  │
│  │                   │ (Arc<RwLock>) │                       │  │
│  │                   └───────────────┘                       │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                                                  │
│  Config: InMemoryStore + JSON file save/load                    │
│  IPC: tokio channels (mpsc, broadcast)                          │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                      分散システムモード                           │
│                                                                  │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐        │
│  │ Reader   │  │ Reader   │  │ Merger   │  │ Recorder │        │
│  │ Process  │  │ Process  │  │ Process  │  │ Process  │        │
│  │          │  │          │  │          │  │          │        │
│  │ Dig #0   │  │ Dig #1   │  │          │  │          │        │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘        │
│       │             │             │             │               │
│       └─────────────┴──────┬──────┴─────────────┘               │
│                            │                                     │
│                    ┌───────▼───────┐                            │
│                    │    ZeroMQ     │                            │
│                    │   (PUB/SUB)   │                            │
│                    └───────────────┘                            │
│                                                                  │
│  ┌──────────┐         ┌──────────┐                              │
│  │ Operator │ ◄─────► │ MongoDB  │                              │
│  │ (Web UI) │         │          │                              │
│  └──────────┘         └──────────┘                              │
│                                                                  │
│  Config: MongoConfigStore                                        │
│  IPC: ZeroMQ (tmq)                                               │
└─────────────────────────────────────────────────────────────────┘
```

---

## 3. 設定データモデル

### 3.1 構造

```rust
/// システム全体の設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemConfig {
    /// バージョン（設定フォーマットの互換性管理）
    pub version: String,

    /// デジタイザ設定のリスト
    pub digitizers: Vec<DigitizerConfig>,

    /// Run設定
    pub run: RunConfig,

    /// 出力設定
    pub output: OutputConfig,
}

/// デジタイザ設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigitizerConfig {
    /// デジタイザID (0-indexed)
    pub id: u32,

    /// 接続URL (e.g., "dig2://172.18.4.56")
    pub url: String,

    /// ファームウェアタイプ
    pub firmware: FirmwareType,

    /// グローバル設定
    pub global: GlobalDigitizerSettings,

    /// チャンネル設定（デフォルト + オーバーライド）
    pub channels: ChannelConfigSet,
}

/// ファームウェアタイプ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FirmwareType {
    PSD2,   // x27xx series, 64-bit
    PSD1,   // x725/x730, 32-bit
    PHA1,   // Pulse Height Analysis
    AMax,   // Custom firmware
}

/// グローバル設定（全チャンネル共通）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalDigitizerSettings {
    pub start_source: StartSource,
    pub gpio_mode: GpioMode,
    pub record_length: u32,
    // ...
}

/// チャンネル設定セット
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfigSet {
    /// デフォルト設定（全チャンネルに適用）
    pub default: ChannelConfig,

    /// 個別オーバーライド（チャンネル番号 -> 設定）
    #[serde(default)]
    pub overrides: HashMap<u8, ChannelConfigOverride>,
}

/// チャンネル設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub enabled: bool,
    pub dc_offset: u16,
    pub trigger_threshold: u16,
    pub pulse_polarity: PulsePolarity,
    pub gate_long_ns: u32,
    pub gate_short_ns: u32,
    pub pre_trigger_ns: u32,
    // ...
}

/// チャンネル設定のオーバーライド（Optionalフィールド）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelConfigOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_threshold: Option<u16>,
    // ... 他のフィールドも同様
}
```

### 3.2 JSON例

```json
{
  "version": "1.0",
  "digitizers": [
    {
      "id": 0,
      "url": "dig2://172.18.4.56",
      "firmware": "PSD2",
      "global": {
        "start_source": "SWcmd",
        "gpio_mode": "Run",
        "record_length": 1024
      },
      "channels": {
        "default": {
          "enabled": true,
          "dc_offset": 20,
          "trigger_threshold": 500,
          "pulse_polarity": "Negative",
          "gate_long_ns": 400,
          "gate_short_ns": 100,
          "pre_trigger_ns": 100
        },
        "overrides": {
          "0": { "trigger_threshold": 1000 },
          "5": { "enabled": false },
          "16": { "trigger_threshold": 800, "dc_offset": 30 }
        }
      }
    }
  ],
  "run": {
    "auto_save": true,
    "max_events_per_file": 1000000,
    "max_file_size_mb": 1024
  },
  "output": {
    "directory": "./data",
    "format": "msgpack",
    "compression": "none"
  }
}
```

---

## 4. ConfigStore トレイト

### 4.1 インターフェース定義

```rust
use async_trait::async_trait;

/// 設定ストレージの抽象化
#[async_trait]
pub trait ConfigStore: Send + Sync {
    // === システム設定 ===

    /// システム全体の設定を取得
    async fn get_system_config(&self) -> Result<SystemConfig, ConfigError>;

    /// システム全体の設定を保存
    async fn set_system_config(&self, config: &SystemConfig) -> Result<(), ConfigError>;

    // === デジタイザ設定 ===

    /// 特定デジタイザの設定を取得
    async fn get_digitizer_config(&self, id: u32) -> Result<DigitizerConfig, ConfigError>;

    /// 特定デジタイザの設定を保存
    async fn set_digitizer_config(&self, id: u32, config: &DigitizerConfig) -> Result<(), ConfigError>;

    /// デジタイザ設定を検証
    async fn validate_digitizer_config(&self, config: &DigitizerConfig) -> Result<ValidationResult, ConfigError>;

    // === Run設定 ===

    /// Run設定を取得
    async fn get_run_config(&self) -> Result<RunConfig, ConfigError>;

    /// Run設定を保存
    async fn set_run_config(&self, config: &RunConfig) -> Result<(), ConfigError>;

    // === 永続化 ===

    /// 設定をファイルに保存（スタンドアローン用）
    async fn save_to_file(&self, path: &Path) -> Result<(), ConfigError>;

    /// 設定をファイルから読み込み（スタンドアローン用）
    async fn load_from_file(&self, path: &Path) -> Result<(), ConfigError>;
}
```

### 4.2 InMemoryConfigStore（スタンドアローン用）

```rust
pub struct InMemoryConfigStore {
    config: Arc<RwLock<SystemConfig>>,
    dirty: AtomicBool,  // 未保存の変更があるか
}

impl InMemoryConfigStore {
    pub fn new() -> Self {
        Self {
            config: Arc::new(RwLock::new(SystemConfig::default())),
            dirty: AtomicBool::new(false),
        }
    }

    pub fn with_config(config: SystemConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            dirty: AtomicBool::new(false),
        }
    }

    /// 未保存の変更があるか
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ConfigStore for InMemoryConfigStore {
    async fn get_system_config(&self) -> Result<SystemConfig, ConfigError> {
        Ok(self.config.read().await.clone())
    }

    async fn set_system_config(&self, config: &SystemConfig) -> Result<(), ConfigError> {
        *self.config.write().await = config.clone();
        self.dirty.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn save_to_file(&self, path: &Path) -> Result<(), ConfigError> {
        let config = self.config.read().await;
        let json = serde_json::to_string_pretty(&*config)?;
        tokio::fs::write(path, json).await?;
        self.dirty.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn load_from_file(&self, path: &Path) -> Result<(), ConfigError> {
        let json = tokio::fs::read_to_string(path).await?;
        let config: SystemConfig = serde_json::from_str(&json)?;
        *self.config.write().await = config;
        self.dirty.store(false, Ordering::SeqCst);
        Ok(())
    }

    // ... 他のメソッド
}
```

### 4.3 MongoConfigStore（分散システム用）

```rust
pub struct MongoConfigStore {
    client: mongodb::Client,
    database: String,
    collection: String,
}

impl MongoConfigStore {
    pub async fn new(uri: &str, database: &str) -> Result<Self, ConfigError> {
        let client = mongodb::Client::with_uri_str(uri).await?;
        Ok(Self {
            client,
            database: database.to_string(),
            collection: "config".to_string(),
        })
    }

    fn collection(&self) -> mongodb::Collection<Document> {
        self.client
            .database(&self.database)
            .collection(&self.collection)
    }
}

#[async_trait]
impl ConfigStore for MongoConfigStore {
    async fn get_digitizer_config(&self, id: u32) -> Result<DigitizerConfig, ConfigError> {
        let filter = doc! { "type": "digitizer", "id": id };
        let doc = self.collection()
            .find_one(filter, None)
            .await?
            .ok_or(ConfigError::NotFound(format!("digitizer {}", id)))?;

        let config: DigitizerConfig = bson::from_document(doc)?;
        Ok(config)
    }

    async fn set_digitizer_config(&self, id: u32, config: &DigitizerConfig) -> Result<(), ConfigError> {
        let filter = doc! { "type": "digitizer", "id": id };
        let doc = bson::to_document(config)?;
        let update = doc! { "$set": doc };

        self.collection()
            .update_one(filter, update, UpdateOptions::builder().upsert(true).build())
            .await?;

        Ok(())
    }

    // save_to_file/load_from_file は MongoDB では no-op または export/import
    async fn save_to_file(&self, _path: &Path) -> Result<(), ConfigError> {
        // MongoDB では設定は既に永続化されている
        Ok(())
    }

    async fn load_from_file(&self, _path: &Path) -> Result<(), ConfigError> {
        // MongoDB では不要（起動時にDBから読み込み）
        Ok(())
    }
}
```

---

## 5. スタンドアローンモード詳細

### 5.1 プロセス構成

```
┌─────────────────────────────────────────────────────────────────┐
│                     Standalone Process                          │
│                                                                  │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │                    Main Thread                           │    │
│  │                                                          │    │
│  │   - Tokio Runtime                                        │    │
│  │   - Signal handling (Ctrl+C)                             │    │
│  │   - Graceful shutdown coordination                       │    │
│  └─────────────────────────────────────────────────────────┘    │
│                              │                                   │
│              ┌───────────────┼───────────────┐                  │
│              ▼               ▼               ▼                  │
│  ┌───────────────┐  ┌───────────────┐  ┌───────────────┐       │
│  │  API Task     │  │  Reader Task  │  │ Pipeline Task │       │
│  │               │  │               │  │               │       │
│  │  - HTTP server│  │  - CAEN FFI   │  │  - Decode     │       │
│  │  - WebSocket  │  │  - ReadLoop   │  │  - Merge      │       │
│  │  - REST API   │  │               │  │  - Record     │       │
│  └───────┬───────┘  └───────┬───────┘  └───────┬───────┘       │
│          │                  │                  │                 │
│          └──────────────────┼──────────────────┘                 │
│                             ▼                                    │
│              ┌─────────────────────────────┐                    │
│              │      Shared State           │                    │
│              │                             │                    │
│              │  - ConfigStore (Arc)        │                    │
│              │  - SystemState (Arc<RwLock>)│                    │
│              │  - Event channels           │                    │
│              └─────────────────────────────┘                    │
└─────────────────────────────────────────────────────────────────┘
```

### 5.2 スレッド間通信

```rust
/// スタンドアローンモードの共有状態
pub struct StandaloneState {
    /// 設定ストア
    pub config: Arc<InMemoryConfigStore>,

    /// システム状態
    pub state: Arc<RwLock<SystemState>>,

    /// Raw data channel: Reader → Pipeline
    pub raw_data_tx: mpsc::Sender<RawDataBatch>,
    pub raw_data_rx: mpsc::Receiver<RawDataBatch>,

    /// Event channel: Pipeline → Monitor/Recorder
    pub event_tx: broadcast::Sender<EventBatch>,

    /// Control channel: API → All components
    pub control_tx: broadcast::Sender<ControlCommand>,

    /// Status channel: All components → API
    pub status_tx: mpsc::Sender<ComponentStatus>,
    pub status_rx: mpsc::Receiver<ComponentStatus>,
}

#[derive(Debug, Clone)]
pub enum ControlCommand {
    Start { run_number: u32 },
    Stop,
    Pause,
    Resume,
    UpdateConfig { digitizer_id: u32 },
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct ComponentStatus {
    pub component: ComponentType,
    pub state: ComponentState,
    pub metrics: ComponentMetrics,
    pub timestamp: SystemTime,
}
```

### 5.3 起動シーケンス

```rust
async fn run_standalone(config_path: Option<PathBuf>) -> Result<()> {
    // 1. 設定の読み込み
    let config_store = Arc::new(InMemoryConfigStore::new());
    if let Some(path) = config_path {
        config_store.load_from_file(&path).await?;
    }

    // 2. 共有状態の初期化
    let (raw_tx, raw_rx) = mpsc::channel(1000);
    let (event_tx, _) = broadcast::channel(1000);
    let (control_tx, _) = broadcast::channel(100);
    let (status_tx, status_rx) = mpsc::channel(100);

    let state = Arc::new(StandaloneState {
        config: config_store.clone(),
        state: Arc::new(RwLock::new(SystemState::Idle)),
        raw_data_tx: raw_tx,
        raw_data_rx: raw_rx,
        event_tx: event_tx.clone(),
        control_tx: control_tx.clone(),
        status_tx,
        status_rx,
    });

    // 3. 各タスクの起動
    let api_handle = tokio::spawn(run_api_server(state.clone()));
    let reader_handle = tokio::spawn(run_reader_task(state.clone()));
    let pipeline_handle = tokio::spawn(run_pipeline_task(state.clone()));

    // 4. シグナル待機
    tokio::select! {
        _ = signal::ctrl_c() => {
            info!("Shutdown signal received");
        }
        result = api_handle => {
            error!("API server exited: {:?}", result);
        }
    }

    // 5. Graceful shutdown
    control_tx.send(ControlCommand::Shutdown)?;

    // 6. 設定の保存（変更があれば）
    if config_store.is_dirty() {
        if let Some(path) = config_path {
            config_store.save_to_file(&path).await?;
        }
    }

    Ok(())
}
```

---

## 6. 分散システムモード詳細

### 6.1 プロセス構成

```
┌───────────────────────────────────────────────────────────────────────────┐
│                          分散システム全体図                                 │
│                                                                            │
│   ┌──────────────┐   ┌──────────────┐   ┌──────────────┐                  │
│   │   Reader 0   │   │   Reader 1   │   │   Reader N   │                  │
│   │              │   │              │   │              │                  │
│   │  ┌────────┐  │   │  ┌────────┐  │   │  ┌────────┐  │                  │
│   │  │CAEN FFI│  │   │  │CAEN FFI│  │   │  │CAEN FFI│  │                  │
│   │  └────────┘  │   │  └────────┘  │   │  └────────┘  │                  │
│   │       │      │   │       │      │   │       │      │                  │
│   │  ┌────▼────┐ │   │  ┌────▼────┐ │   │  ┌────▼────┐ │                  │
│   │  │ Decoder │ │   │  │ Decoder │ │   │  │ Decoder │ │                  │
│   │  └────┬────┘ │   │  └────┬────┘ │   │  └────┬────┘ │                  │
│   │       │      │   │       │      │   │       │      │                  │
│   │  ┌────▼────┐ │   │  ┌────▼────┐ │   │  ┌────▼────┐ │                  │
│   │  │ ZMQ PUB │ │   │  │ ZMQ PUB │ │   │  │ ZMQ PUB │ │                  │
│   │  └────┬────┘ │   │  └────┬────┘ │   │  └────┬────┘ │                  │
│   └───────│──────┘   └───────│──────┘   └───────│──────┘                  │
│           │                  │                  │                          │
│           └──────────────────┼──────────────────┘                          │
│                              │                                              │
│                              ▼                                              │
│                    ┌──────────────────┐                                    │
│                    │      Merger      │                                    │
│                    │                  │                                    │
│                    │  ┌────────────┐  │                                    │
│                    │  │  ZMQ SUB   │  │                                    │
│                    │  └─────┬──────┘  │                                    │
│                    │        │         │                                    │
│                    │  ┌─────▼──────┐  │                                    │
│                    │  │ Time Sort  │  │                                    │
│                    │  └─────┬──────┘  │                                    │
│                    │        │         │                                    │
│                    │  ┌─────▼──────┐  │                                    │
│                    │  │  ZMQ PUB   │  │                                    │
│                    │  └─────┬──────┘  │                                    │
│                    └────────│─────────┘                                    │
│                             │                                               │
│              ┌──────────────┼──────────────┐                               │
│              ▼              ▼              ▼                               │
│   ┌──────────────┐  ┌──────────────┐  ┌──────────────┐                    │
│   │   Recorder   │  │   Monitor    │  │   Analyzer   │                    │
│   │              │  │              │  │   (future)   │                    │
│   │  - File I/O  │  │  - Web UI    │  │              │                    │
│   │  - MsgPack   │  │  - Histogram │  │              │                    │
│   └──────────────┘  └──────────────┘  └──────────────┘                    │
│                                                                            │
│                         ┌──────────────┐                                   │
│                         │   Operator   │                                   │
│                         │   (Web UI)   │                                   │
│                         │              │                                   │
│                         │  ┌────────┐  │                                   │
│                         │  │REST API│  │                                   │
│                         │  └────┬───┘  │       ┌──────────────┐           │
│                         │       │      │ ◄───► │   MongoDB    │           │
│                         │  ┌────▼───┐  │       │              │           │
│                         │  │ZMQ CMD │  │       │  - Config    │           │
│                         │  └────────┘  │       │  - Run Info  │           │
│                         └──────────────┘       │  - History   │           │
│                                                └──────────────┘           │
└───────────────────────────────────────────────────────────────────────────┘
```

### 6.2 設定の伝播

```
┌──────────────────────────────────────────────────────────────────────────┐
│                         設定変更の流れ                                     │
│                                                                           │
│   User                                                                    │
│     │                                                                     │
│     │ 1. Web UIで設定変更                                                  │
│     ▼                                                                     │
│   ┌──────────────┐                                                        │
│   │   Operator   │                                                        │
│   │   (Web UI)   │                                                        │
│   └──────┬───────┘                                                        │
│          │                                                                │
│          │ 2. POST /api/config/digitizer/0                                │
│          ▼                                                                │
│   ┌──────────────┐                                                        │
│   │  REST API    │                                                        │
│   │  Handler     │                                                        │
│   └──────┬───────┘                                                        │
│          │                                                                │
│          │ 3. MongoConfigStore.set_digitizer_config()                     │
│          ▼                                                                │
│   ┌──────────────┐                                                        │
│   │   MongoDB    │ ◄─── 4. Change Stream (optional)                       │
│   └──────┬───────┘                                                        │
│          │                                                                │
│          │ 5. ZMQ Command: ConfigUpdated { digitizer_id: 0 }              │
│          ▼                                                                │
│   ┌──────────────┐                                                        │
│   │   Reader 0   │                                                        │
│   │              │                                                        │
│   │ 6. Fetch new config from MongoDB                                      │
│   │ 7. Apply to digitizer (if safe to do so)                              │
│   └──────────────┘                                                        │
│                                                                           │
│   Note: Run中の設定変更は制限される（安全でないパラメータは拒否）             │
└──────────────────────────────────────────────────────────────────────────┘
```

---

## 7. フロントエンド設計

### 7.1 共通コンポーネント

```
src/
├── components/
│   ├── config/
│   │   ├── DigitizerConfigEditor.tsx    # デジタイザ設定エディタ
│   │   ├── ChannelConfigTable.tsx       # チャンネル設定テーブル
│   │   ├── GlobalSettingsForm.tsx       # グローバル設定フォーム
│   │   └── ConfigValidationStatus.tsx   # バリデーション結果表示
│   │
│   ├── control/
│   │   ├── RunControlPanel.tsx          # Start/Stop/Pause ボタン
│   │   ├── RunStatusDisplay.tsx         # Run状態表示
│   │   └── RunNumberInput.tsx           # Run番号入力
│   │
│   ├── monitor/
│   │   ├── LiveEventDisplay.tsx         # リアルタイムイベント表示
│   │   ├── RateChart.tsx                # イベントレートグラフ
│   │   ├── ChannelHistogram.tsx         # チャンネル別ヒストグラム
│   │   └── SystemMetrics.tsx            # システムメトリクス
│   │
│   └── common/
│       ├── StatusIndicator.tsx          # 状態インジケータ
│       ├── ErrorDisplay.tsx             # エラー表示
│       └── LoadingSpinner.tsx           # ローディング表示
│
├── services/
│   ├── api.ts                           # REST API クライアント
│   ├── websocket.ts                     # WebSocket クライアント
│   └── config.ts                        # 設定管理
│
└── pages/
    ├── Dashboard.tsx                    # メインダッシュボード
    ├── Configuration.tsx                # 設定ページ
    ├── Monitor.tsx                      # モニタリングページ
    └── History.tsx                      # 履歴ページ
```

### 7.2 API クライアント

```typescript
// services/api.ts

interface ApiClient {
  // Config
  getSystemConfig(): Promise<SystemConfig>;
  getDigitizerConfig(id: number): Promise<DigitizerConfig>;
  setDigitizerConfig(id: number, config: DigitizerConfig): Promise<void>;
  validateConfig(config: DigitizerConfig): Promise<ValidationResult>;

  // Control
  startRun(runNumber: number): Promise<void>;
  stopRun(): Promise<void>;
  pauseRun(): Promise<void>;
  resumeRun(): Promise<void>;

  // Status
  getStatus(): Promise<SystemStatus>;

  // WebSocket for live updates
  connectWebSocket(): WebSocket;
}

// 実装は環境によって切り替え
export function createApiClient(baseUrl: string): ApiClient {
  return new HttpApiClient(baseUrl);
}

// Tauri用（IPC経由）
export function createTauriApiClient(): ApiClient {
  return new TauriApiClient();
}
```

---

## 8. 実装ロードマップ

### Phase 1: Core Infrastructure（現在〜2月中旬）

| タスク | 優先度 | 依存関係 |
|--------|--------|----------|
| ConfigStore トレイト定義 | High | なし |
| InMemoryConfigStore 実装 | High | ConfigStore |
| DigitizerConfig 構造体 | High | なし |
| 設定バリデーション | Medium | DigitizerConfig |
| 設定→デジタイザ適用 | High | CaenHandle |

### Phase 2: Standalone Mode（2月中旬〜3月初旬）

| タスク | 優先度 | 依存関係 |
|--------|--------|----------|
| Reader Task（ReadLoop + Decode） | High | Phase 1 |
| Pipeline Task（Record） | High | Phase 1 |
| 共有状態管理 | High | Phase 1 |
| REST API サーバー | Medium | Phase 1 |
| WebSocket イベント配信 | Medium | REST API |
| Tauri統合（オプション） | Low | REST API |

### Phase 3: Distributed Mode（3月初旬〜）

| タスク | 優先度 | 依存関係 |
|--------|--------|----------|
| MongoConfigStore 実装 | High | ConfigStore |
| 既存ZMQパイプライン統合 | High | Phase 2 |
| 設定変更伝播メカニズム | Medium | MongoConfigStore |
| Multi-Reader 同期 | Medium | Phase 2 |

### Phase 4: Frontend（並行作業）

| タスク | 優先度 | 依存関係 |
|--------|--------|----------|
| 基本レイアウト | High | なし |
| 設定エディタ | High | REST API |
| Run制御パネル | High | REST API |
| リアルタイムモニタ | Medium | WebSocket |
| ヒストグラム表示 | Medium | WebSocket |

---

## 9. 未解決の設計課題

### 9.1 Run中の設定変更

- **問題**: Run中にトリガー閾値などを変更したい場合がある
- **選択肢**:
  1. Run中は全ての設定変更を禁止
  2. 安全なパラメータのみ変更可能（ホワイトリスト）
  3. 一時停止→変更→再開のフロー

### 9.2 設定のバージョニング

- **問題**: 設定フォーマットの変更時に後方互換性をどう保つか
- **選択肢**:
  1. バージョン番号でマイグレーション
  2. スキーマ進化（追加フィールドはOptional）
  3. 両方の組み合わせ

### 9.3 マルチデジタイザの同期

- **問題**: 複数デジタイザのStart/Stopを同期するか
- **選択肢**:
  1. GPIO/LVDS同期（ハードウェア）
  2. ソフトウェア同期（許容できる遅延の範囲で）
  3. 独立動作（タイムスタンプで後からソート）

### 9.4 エラーリカバリ

- **問題**: デジタイザ接続断、MongoDB接続断時の動作
- **選択肢**:
  1. 自動再接続（リトライ回数制限付き）
  2. エラー状態で停止（手動復旧）
  3. 部分的継続（一部デジタイザが落ちても他は継続）

---

## 10. 参考資料

- DELILA2 C++実装: `DELILA2/lib/digitizer/`
- CAEN FELib ドキュメント
- MongoDB Rust Driver: https://docs.rs/mongodb
- Tauri: https://tauri.app/
- Axum: https://docs.rs/axum
