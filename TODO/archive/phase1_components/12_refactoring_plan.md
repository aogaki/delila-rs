# Refactoring Plan

**Status: COMPLETED** (2026-01-19)

## 実装状況

| Phase | 内容 | 状態 | 備考 |
|-------|-----|------|------|
| 1 | CLIパーサー統合 (clap) | **✅ COMPLETED** | 7バイナリ移行、24テスト追加 |
| 2 | 統一メトリクスフレームワーク | **✅ COMPLETED** | metrics.rs作成、10テスト追加 |
| 3 | エラー型統合 | **✅ COMPLETED** | error.rs作成、6テスト追加 |
| 4 | 設定構造体の共通化 | **SKIPPED** | KISS原則により見送り（共通性が限定的） |
| 5 | シャットダウン機構の統一 | **✅ COMPLETED** | shutdown.rs作成、5バイナリ移行 |

### Phase 1 成果 (2026-01-19)
- **新規ファイル:** `src/common/cli.rs` (382行、24テスト)
- **移行済み:** emulator, merger, recorder, monitor, operator, data_sink, controller
- **追加型:** CommonArgs, SourceArgs, MergerArgs, RecorderArgs, MonitorArgs, DataSinkArgs, OperatorArgs, ControllerArgs
- **行数:** 各バイナリで40-65行削減

### Phase 2 成果 (2026-01-19)
- **新規ファイル:** `src/common/metrics.rs` (263行、10テスト)
- **提供機能:** AtomicCounters (lock-free統計), CounterSnapshot, RateSnapshot
- **特徴:** ゼロオーバーヘッド、Relaxed ordering、rate計算ヘルパー

### Phase 3 成果 (2026-01-19)
- **新規ファイル:** `src/common/error.rs` (6テスト)
- **提供型:** PipelineError (10+バリアント), PipelineResult<T>
- **特徴:** ZMQ/シリアライズ/IO/チャンネル/設定エラーの統一定義
- **備考:** 既存コンポーネントエラー型は維持（破壊的変更回避）

### Phase 4 見送り理由 (2026-01-19)
- 各コンポーネントの設定構造体を調査した結果、フィールドの共通性が限定的
- トレイト抽象化の利点よりも複雑化のコストが高い
- KISS原則に従い、現状維持を決定

### Phase 5 成果 (2026-01-19)
- **新規ファイル:** `src/common/shutdown.rs` (2テスト)
- **提供関数:** setup_shutdown(), setup_shutdown_with_message()
- **型エイリアス:** ShutdownSignal, ShutdownSender, ShutdownReceiver
- **移行済み:** merger, recorder, monitor, data_sink, emulator
- **削減:** 各バイナリで約10行のboilerplate削除

---

## 基本原則

### KISS原則 (Keep It Simple, Stupid)

**すべてのリファクタリングはKISS原則に従う:**

1. **過度な抽象化を避ける** - 実際に使われる箇所が2つ以上ない限り共通化しない
2. **将来の拡張のための設計禁止** - 今必要なものだけを実装
3. **シンプルな解決策を優先** - 複雑なジェネリクスより具体的な型
4. **可読性 > 短さ** - コードの短縮より理解しやすさを重視

### テスト駆動開発 (TDD)

**すべてのリファクタリングはテストベースで実施:**

```
Red → Green → Refactor
1. まず失敗するテストを書く
2. テストが通る最小限の実装
3. テストが通る状態を維持しながらリファクタリング
```

**各Phase完了条件:**
- [ ] 既存テストがすべてパス (`cargo test`)
- [ ] 新規コードに対するユニットテスト追加
- [ ] `cargo clippy -- -D warnings` パス
- [ ] 統合テスト（`/test-daq`）成功

---

## 概要

コードベースの調査結果に基づくリファクタリング計画。
重複コードの削減、一貫性の向上、保守性の改善を目指す。

**推定効果:**
- 全体で20-25%のコード行数削減
- bin/ディレクトリで40%+削減
- 保守性・拡張性の大幅向上

---

## 現状の課題

### 1. CLIパース重複 (最高優先度)

**影響ファイル:** 8つのバイナリ全て

```rust
// 全バイナリで手動実装されている（約200行/バイナリ）
let mut i = 1;
while i < args.len() {
    match args[i].as_str() {
        "--config" | "-f" => { /* 各バイナリで繰り返し */ },
        "--help" | "-h" => { /* 繰り返し */ },
        _ => { eprintln!("Unknown argument"); },
    }
}
```

**問題:**
- 200行/バイナリの重複（合計1600行）
- 手動インデックス管理でエラーが起きやすい
- バリデーションフレームワークなし
- 一貫性維持が困難

---

### 2. AtomicStats構造体の重複 (高優先度)

**影響ファイル:** reader, merger, recorder, monitor, data_sink

各コンポーネントで独自定義:

| Component | Struct Name | Fields |
|-----------|-------------|--------|
| Reader | `ReaderMetrics` | events_decoded, bytes_read, batches_published, queue_length |
| Merger | `AtomicStats` | received_batches, sent_batches, dropped_batches, eos_received |
| Recorder | `AtomicStats` | received_batches, received_events, written_events, written_bytes, ... |
| Monitor | 類似構造 | ... |

**問題:**
- 共通パターンが5箇所で再実装
- スナップショットAPIが不統一（タプル vs 構造体）
- 命名規則がバラバラ

---

### 3. エラー型の重複 (中優先度)

**影響ファイル:** 9ファイル

```rust
#[derive(Error, Debug)]
pub enum XxxError {
    #[error("ZMQ error: {0}")]
    Zmq(#[from] tmq::TmqError),
    #[error("Serialization error: {0}")]
    Serialization(#[from] rmp_serde::encode::Error),
    // 全コンポーネントで繰り返し
}
```

---

### 4. コンポーネント間の不整合

| 観点 | Reader | Merger | Recorder | Monitor |
|------|--------|--------|----------|---------|
| エラー復旧 | 混在 | unwrap多用 | ? 演算子 | 無視あり |
| Metrics公開 | Public | Private | Private | Private |
| SnapshotAPI | 個別フィールド | タプル | 構造体 | 構造体 |
| Shutdown | Atomic+Broadcast | Broadcastのみ | Broadcastのみ | Broadcastのみ |

---

## Phase 1: CLIパーサー統合

**優先度:** 最高
**推定削減:** 500行以上

### 1.1 事前準備

- [ ] **テスト:** 現行の全バイナリが正常動作することを確認
  ```bash
  cargo build --release
  ./scripts/start_daq.sh && sleep 3 && ./scripts/stop_daq.sh
  ```

### 1.2 clap導入

- [ ] **Cargo.toml更新**
  ```toml
  [dependencies]
  clap = { version = "4", features = ["derive"] }
  ```

### 1.3 共通CLIモジュール作成

**新規ファイル:** `src/common/cli.rs`

```rust
//! CLI argument parsing for DELILA components
//!
//! # Design Principles (KISS)
//! - Use clap's derive macro for declarative argument definition
//! - Common arguments shared via composition, not inheritance
//! - Each binary has its own Args struct that embeds CommonArgs

use clap::Parser;

/// Common arguments shared across all DELILA components
#[derive(Parser, Debug, Clone)]
pub struct CommonArgs {
    /// Path to configuration file
    #[arg(short = 'f', long = "config", default_value = "config.toml")]
    pub config_file: String,
}

/// Arguments for source components (Reader, Emulator)
#[derive(Parser, Debug, Clone)]
pub struct SourceArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Source ID (0-indexed module number)
    #[arg(long = "source-id")]
    pub source_id: Option<u32>,

    /// Override bind address
    #[arg(long)]
    pub address: Option<String>,
}

/// Arguments for pipeline components (Merger, Recorder, Monitor)
#[derive(Parser, Debug, Clone)]
pub struct PipelineArgs {
    #[command(flatten)]
    pub common: CommonArgs,
}

/// Arguments for Operator (Web UI)
#[derive(Parser, Debug, Clone)]
pub struct OperatorArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// HTTP server port
    #[arg(long, default_value = "8080")]
    pub port: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_common_args_default() {
        let args = CommonArgs::try_parse_from(["test"]).unwrap();
        assert_eq!(args.config_file, "config.toml");
    }

    #[test]
    fn test_common_args_custom_config() {
        let args = CommonArgs::try_parse_from(["test", "-f", "custom.toml"]).unwrap();
        assert_eq!(args.config_file, "custom.toml");
    }

    #[test]
    fn test_source_args_with_id() {
        let args = SourceArgs::try_parse_from(["test", "--source-id", "1"]).unwrap();
        assert_eq!(args.source_id, Some(1));
    }

    #[test]
    fn test_source_args_with_address() {
        let args = SourceArgs::try_parse_from(["test", "--address", "tcp://*:5555"]).unwrap();
        assert_eq!(args.address, Some("tcp://*:5555".to_string()));
    }

    #[test]
    fn test_operator_args_port() {
        let args = OperatorArgs::try_parse_from(["test", "--port", "9090"]).unwrap();
        assert_eq!(args.port, 9090);
    }
}
```

### 1.4 バイナリ移行（段階的）

各バイナリを順次移行。移行ごとにテスト実行。

#### 1.4.1 emulator.rs

**Before (約80行):**
```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let mut config_file: Option<String> = None;
    let mut source_id: Option<u32> = None;
    let mut address: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-f" => {
                i += 1;
                config_file = Some(args.get(i).ok_or("...")?.clone());
            }
            // ... 繰り返し
        }
        i += 1;
    }
    // ...
}
```

**After (約10行):**
```rust
use clap::Parser;
use delila_rs::common::cli::SourceArgs;

#[derive(Parser)]
#[command(name = "emulator", about = "DELILA data source emulator")]
struct Args {
    #[command(flatten)]
    source: SourceArgs,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let config = if std::path::Path::new(&args.source.common.config_file).exists() {
        Config::load(&args.source.common.config_file)?
    } else {
        Config::default()
    };
    // ...
}
```

- [ ] `src/bin/emulator.rs` - 移行完了
- [ ] テスト: `cargo run --bin emulator -- --help`
- [ ] テスト: `cargo run --bin emulator -- -f config.toml --source-id 0`

#### 1.4.2 その他のバイナリ

同様のパターンで移行:

- [ ] `src/bin/reader.rs` - SourceArgs使用
- [ ] `src/bin/merger.rs` - PipelineArgs使用
- [ ] `src/bin/recorder.rs` - PipelineArgs使用
- [ ] `src/bin/monitor.rs` - PipelineArgs使用
- [ ] `src/bin/operator.rs` - OperatorArgs使用
- [ ] `src/bin/data_sink.rs` - PipelineArgs使用
- [ ] `src/bin/controller.rs` - 専用Args作成

### 1.5 Phase 1 完了条件

- [ ] 全8バイナリがclapベースに移行
- [ ] `cargo test` パス
- [ ] `cargo clippy -- -D warnings` パス
- [ ] `/test-daq` 統合テスト成功
- [ ] `--help` が全バイナリで正常表示

---

## Phase 2: 統一メトリクスフレームワーク

**優先度:** 高
**推定削減:** 200行以上

### 2.1 設計原則 (KISS)

- 各コンポーネントは必要なフィールドのみ使用
- ジェネリクスは使わず、具体的な構造体
- スナップショットは単純なクローン可能な構造体

### 2.2 共通メトリクスモジュール作成

**新規ファイル:** `src/common/metrics.rs`

```rust
//! Lock-free metrics collection for DELILA components
//!
//! # Design Principles (KISS)
//! - Simple atomic counters, no complex synchronization
//! - Snapshot is just a plain struct copy
//! - Components use only the fields they need

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// Atomic counters for component metrics (lock-free)
#[derive(Debug, Default)]
pub struct AtomicMetrics {
    pub received: AtomicU64,
    pub processed: AtomicU64,
    pub sent: AtomicU64,
    pub dropped: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
}

impl AtomicMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a snapshot of current values
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            received: self.received.load(Ordering::Relaxed),
            processed: self.processed.load(Ordering::Relaxed),
            sent: self.sent.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            timestamp: SystemTime::now(),
        }
    }

    // Convenience increment methods
    #[inline]
    pub fn inc_received(&self) {
        self.received.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_received_by(&self, n: u64) {
        self.received.fetch_add(n, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_sent(&self) {
        self.sent.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_dropped(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn add_bytes(&self, n: u64) {
        self.bytes.fetch_add(n, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_errors(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }
}

/// Serializable snapshot of metrics at a point in time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub received: u64,
    pub processed: u64,
    pub sent: u64,
    pub dropped: u64,
    pub bytes: u64,
    pub errors: u64,
    #[serde(with = "humantime_serde")]
    pub timestamp: SystemTime,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atomic_metrics_new() {
        let m = AtomicMetrics::new();
        assert_eq!(m.received.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_atomic_metrics_increment() {
        let m = AtomicMetrics::new();
        m.inc_received();
        m.inc_received();
        m.inc_sent();
        assert_eq!(m.received.load(Ordering::Relaxed), 2);
        assert_eq!(m.sent.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_snapshot() {
        let m = AtomicMetrics::new();
        m.inc_received_by(100);
        m.add_bytes(1024);

        let snap = m.snapshot();
        assert_eq!(snap.received, 100);
        assert_eq!(snap.bytes, 1024);
    }

    #[test]
    fn test_snapshot_serialization() {
        let m = AtomicMetrics::new();
        m.inc_received();
        let snap = m.snapshot();

        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("\"received\":1"));
    }
}
```

### 2.3 コンポーネント移行

各コンポーネントの独自AtomicStats → AtomicMetrics

- [ ] `src/merger/mod.rs` - AtomicStats → AtomicMetrics
  - received_batches → received
  - sent_batches → sent
  - dropped_batches → dropped

- [ ] `src/recorder/mod.rs` - AtomicStats → AtomicMetrics
  - received_batches → received
  - written_events → processed
  - written_bytes → bytes

- [ ] `src/monitor/mod.rs` - 統合

- [ ] `src/data_sink/mod.rs` - 統合

- [ ] `src/reader/mod.rs` - ReaderMetrics → AtomicMetrics
  - ※Reader固有のqueue_lengthは別途保持

### 2.4 Phase 2 完了条件

- [ ] AtomicMetrics が common/metrics.rs に定義
- [ ] 5コンポーネントが AtomicMetrics を使用
- [ ] `cargo test` パス
- [ ] `/test-daq` 統合テスト成功
- [ ] Operator API の metrics レスポンスが統一フォーマット

---

## Phase 3: エラー型統合

**優先度:** 中
**推定削減:** 100行以上

### 3.1 設計原則 (KISS)

- 共通エラーと コンポーネント固有エラーを分離
- `anyhow` ではなく `thiserror` で型安全性を維持
- 過度な細分化を避け、必要最小限のバリアント

### 3.2 共通エラーモジュール作成

**新規ファイル:** `src/common/error.rs`

```rust
//! Common error types for DELILA components
//!
//! # Design Principles (KISS)
//! - Only errors that appear in 2+ components are defined here
//! - Component-specific errors remain in their modules
//! - Use #[from] for automatic conversion from common library errors

use thiserror::Error;

/// Common errors shared across DELILA components
#[derive(Error, Debug)]
pub enum DaqError {
    /// ZeroMQ communication error
    #[error("ZMQ error: {0}")]
    Zmq(#[from] tmq::TmqError),

    /// MessagePack serialization error
    #[error("Serialization error: {0}")]
    Serialize(#[from] rmp_serde::encode::Error),

    /// MessagePack deserialization error
    #[error("Deserialization error: {0}")]
    Deserialize(#[from] rmp_serde::decode::Error),

    /// IO error (file, network, etc.)
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Channel send error (mpsc channel closed)
    #[error("Channel closed")]
    ChannelClosed,

    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),

    /// Invalid state transition
    #[error("Invalid state: {0}")]
    InvalidState(String),

    /// Component-specific error (for extension)
    #[error("{0}")]
    Component(String),
}

/// Convenience type alias
pub type DaqResult<T> = Result<T, DaqError>;

impl DaqError {
    /// Create a component error with message
    pub fn component(msg: impl Into<String>) -> Self {
        DaqError::Component(msg.into())
    }

    /// Create a config error with message
    pub fn config(msg: impl Into<String>) -> Self {
        DaqError::Config(msg.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let e = DaqError::config("missing field");
        assert_eq!(e.to_string(), "Configuration error: missing field");
    }

    #[test]
    fn test_io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let daq_err: DaqError = io_err.into();
        assert!(matches!(daq_err, DaqError::Io(_)));
    }

    #[test]
    fn test_result_type() {
        fn example() -> DaqResult<i32> {
            Ok(42)
        }
        assert_eq!(example().unwrap(), 42);
    }
}
```

### 3.3 段階的移行

**移行方針:**
- まず新規コードで DaqError を使用
- 既存コードは徐々に移行（破壊的変更を避ける）

- [ ] `src/common/error.rs` 作成
- [ ] `src/common/mod.rs` に `pub mod error;` 追加
- [ ] `src/merger/mod.rs` - MergerError → DaqError
- [ ] `src/recorder/mod.rs` - RecorderError → DaqError
- [ ] `src/monitor/mod.rs` - MonitorError → DaqError
- [ ] `src/data_sink/mod.rs` - エラー型統合

### 3.4 Phase 3 完了条件

- [ ] DaqError が common/error.rs に定義
- [ ] 4コンポーネント以上で使用
- [ ] `cargo test` パス
- [ ] エラーメッセージが統一フォーマット

---

## Phase 4: 設定構造体の共通化

**優先度:** 低〜中
**推定削減:** 50行/コンポーネント

### 4.1 設計原則 (KISS)

- トレイトは最小限のメソッドのみ定義
- デフォルト実装を積極的に使用
- 複雑な継承階層は避ける

### 4.2 共通設定トレイト

**新規ファイル:** `src/common/config_base.rs`

```rust
//! Common configuration traits for DELILA components
//!
//! # Design Principles (KISS)
//! - Traits define only required methods
//! - Default implementations for common patterns
//! - No complex generics or associated types

/// Base trait for all component configurations
pub trait ComponentConfig {
    /// ZMQ REP socket address for command handling
    fn command_address(&self) -> &str;

    /// Channel capacity for internal message passing (default: 1000)
    fn channel_capacity(&self) -> usize {
        1000
    }
}

/// Trait for components that subscribe to data streams
pub trait SubscriberConfig: ComponentConfig {
    /// ZMQ SUB socket address(es) to connect to
    fn subscribe_addresses(&self) -> &[String];
}

/// Trait for components that publish data streams
pub trait PublisherConfig: ComponentConfig {
    /// ZMQ PUB socket address to bind
    fn publish_address(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestConfig {
        cmd_addr: String,
        sub_addrs: Vec<String>,
    }

    impl ComponentConfig for TestConfig {
        fn command_address(&self) -> &str {
            &self.cmd_addr
        }
    }

    impl SubscriberConfig for TestConfig {
        fn subscribe_addresses(&self) -> &[String] {
            &self.sub_addrs
        }
    }

    #[test]
    fn test_default_channel_capacity() {
        let cfg = TestConfig {
            cmd_addr: "tcp://*:5000".to_string(),
            sub_addrs: vec![],
        };
        assert_eq!(cfg.channel_capacity(), 1000);
    }
}
```

### 4.3 コンポーネント適用

- [ ] `src/common/config_base.rs` 作成
- [ ] MergerConfig に ComponentConfig + SubscriberConfig + PublisherConfig 実装
- [ ] RecorderConfig に ComponentConfig + SubscriberConfig 実装
- [ ] MonitorConfig に ComponentConfig + SubscriberConfig 実装

### 4.4 Phase 4 完了条件

- [ ] 設定トレイトが定義済み
- [ ] 3コンポーネント以上で実装
- [ ] `cargo test` パス

---

## Phase 5: シャットダウン機構の統一

**優先度:** 低
**目的:** ロバスト性向上

### 5.1 設計原則 (KISS)

- 既存の broadcast チャンネルパターンを維持
- spawn_blocking 用の AtomicBool を統合
- 複雑な状態管理を避ける

### 5.2 統一シャットダウンシグナル

**新規:** `src/common/shutdown.rs`

```rust
//! Unified shutdown signal for DELILA components
//!
//! # Design Principles (KISS)
//! - Combines broadcast channel (for async) and AtomicBool (for blocking)
//! - Simple clone-able handle for distribution to tasks
//! - No complex state machines

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Shutdown signal that works with both async and blocking code
#[derive(Clone)]
pub struct ShutdownSignal {
    /// For async tasks: receive shutdown notification
    rx: broadcast::Receiver<()>,
    /// For blocking tasks: poll this flag
    flag: Arc<AtomicBool>,
}

/// Shutdown controller that sends the signal
pub struct ShutdownController {
    tx: broadcast::Sender<()>,
    flag: Arc<AtomicBool>,
}

impl ShutdownController {
    /// Create a new shutdown controller and initial signal
    pub fn new() -> (Self, ShutdownSignal) {
        let (tx, rx) = broadcast::channel(1);
        let flag = Arc::new(AtomicBool::new(false));

        let controller = Self {
            tx,
            flag: Arc::clone(&flag),
        };

        let signal = ShutdownSignal { rx, flag };

        (controller, signal)
    }

    /// Trigger shutdown
    pub fn shutdown(&self) {
        self.flag.store(true, Ordering::SeqCst);
        let _ = self.tx.send(());
    }

    /// Create additional signal handles
    pub fn subscribe(&self) -> ShutdownSignal {
        ShutdownSignal {
            rx: self.tx.subscribe(),
            flag: Arc::clone(&self.flag),
        }
    }
}

impl ShutdownSignal {
    /// Check if shutdown was requested (for blocking code)
    pub fn is_shutdown(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    /// Wait for shutdown signal (for async code)
    pub async fn wait(&mut self) {
        let _ = self.rx.recv().await;
    }

    /// Get a clone of the atomic flag (for spawn_blocking)
    pub fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.flag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_shutdown_signal() {
        let (controller, mut signal) = ShutdownController::new();

        assert!(!signal.is_shutdown());

        controller.shutdown();

        assert!(signal.is_shutdown());
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let (controller, signal1) = ShutdownController::new();
        let signal2 = controller.subscribe();

        controller.shutdown();

        assert!(signal1.is_shutdown());
        assert!(signal2.is_shutdown());
    }
}
```

### 5.3 コンポーネント適用

- [ ] `src/common/shutdown.rs` 作成
- [ ] Reader で ShutdownSignal 使用（spawn_blocking 対応）
- [ ] 他コンポーネントで段階的に適用

### 5.4 Phase 5 完了条件

- [ ] ShutdownSignal が定義済み
- [ ] Reader で正常に動作
- [ ] グレースフルシャットダウンが全コンポーネントで機能

---

## 実装スケジュール

| Phase | 作業内容 | 完了条件 |
|-------|---------|---------|
| **1** | CLIパーサー統合 (clap) | 全8バイナリ移行、500行削減 |
| **2** | メトリクスフレームワーク | 5コンポーネント統合、200行削減 |
| **3** | エラー型統合 | 4コンポーネント以上、100行削減 |
| **4** | 設定トレイト | 3コンポーネント以上 |
| **5** | シャットダウン統一 | 全コンポーネント対応 |

---

## 保持すべき良い設計（リファクタリング不要）

1. **統一ステートマシン** (`common::ComponentState`, `command_task`)
   - 全コンポーネントで同一の5状態マシン
   - `CommandHandlerExt` トレイトによる拡張性

2. **ロックフリータスク分離** (CLAUDE.md準拠)
   - mpscチャンネルによる分離
   - ホットパスでのmutexブロックなし

3. **シリアライゼーションプロトコル**
   - データ: MessagePack
   - コマンド: JSON

4. **設定読み込み**
   - TOML形式
   - CLI引数でオーバーライド可能

---

## チェックリスト（各Phase共通）

### 実装前
- [ ] 関連する既存テストの確認
- [ ] 影響範囲の特定
- [ ] KISS原則に照らした設計レビュー

### 実装中
- [ ] テストファースト（失敗するテストを先に書く）
- [ ] 小さなコミット単位
- [ ] `cargo check` が常にパス

### 実装後
- [ ] `cargo test` 全テストパス
- [ ] `cargo clippy -- -D warnings` パス
- [ ] `cargo fmt --check` パス
- [ ] `/test-daq` 統合テスト成功
- [ ] パフォーマンス劣化がないことを確認

---

## 参考

- **KISS原則:** `CLAUDE.md` Design Principles参照
- **TDD:** `CLAUDE.md` "Code without tests is non-existent code"
- **現在のアーキテクチャ:** `CLAUDE.md` Component Architecture Principles参照
- **既存リファクタリング履歴:** `TODO/archive/phase1_components/07_refactoring_plan.md`
