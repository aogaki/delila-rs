# TODO: Refactoring Plan

**Status:** PLANNING
**Created:** 2026-01-13

## 概要

現在のコードベースには大量のコード重複があり、保守性とテスト容易性に課題がある。
このドキュメントでは、段階的なリファクタリング計画を定義する。

---

## 現状分析

### コード重複の概要

| パターン | 重複箇所 | 重複率 | 推定削減行数 |
|---------|---------|-------|-------------|
| `handle_command()` | 4箇所 (Emulator/Reader/Merger/DataSink) | 85% | ~300行 |
| `command_task()` | 4箇所 | 95% | ~200行 |
| `SourceStats` | 2箇所 (Merger/DataSink) | 80% | ~80行 |
| `publish_message()` | 2箇所 (Emulator/Reader) | 90% | ~40行 |
| ZMQ接続パターン | 複数箇所 | 70% | ~50行 |
| **合計** | | | **~670行** |

### 現在のTrait定義

**ゼロ** - すべてモノリシックな構造体で実装

---

## リファクタリングフェーズ

### Phase 1: Command Infrastructure (優先度: 最高)

#### 1.1 SharedState の統一

**問題:** 各コンポーネントが同じ `SharedState` を独自に定義
```rust
// 4箇所で同じ定義
struct SharedState {
    state: ComponentState,
    run_config: Option<RunConfig>,
}
```

**解決策:** `common/state.rs` に移動
```rust
// src/common/state.rs
pub struct ComponentSharedState {
    pub state: ComponentState,
    pub run_config: Option<RunConfig>,
}
```

#### 1.2 CommandHandler Trait の導入

**問題:** `handle_command()` が4箇所で85%同一
```rust
// 現在: 4つのファイルで同じロジック
fn handle_command(state: &mut SharedState, cmd: Command) -> CommandResponse {
    match cmd {
        Command::Configure(cfg) => { /* 同じ */ }
        Command::Arm => { /* 同じ */ }
        // ...
    }
}
```

**解決策:** Trait + デフォルト実装
```rust
// src/common/command_handler.rs
pub trait CommandHandler {
    /// コンポーネント名（ログ用）
    fn component_name(&self) -> &'static str;

    /// 状態への参照を取得
    fn shared_state(&self) -> &ComponentSharedState;
    fn shared_state_mut(&mut self) -> &mut ComponentSharedState;

    /// GetStatusの拡張情報（オプション）
    fn status_details(&self) -> Option<String> { None }

    /// コンポーネント固有のconfigure処理（オプション）
    fn on_configure(&mut self, _config: &RunConfig) -> Result<(), String> { Ok(()) }

    /// コンポーネント固有のstart処理（オプション）
    fn on_start(&mut self) -> Result<(), String> { Ok(()) }

    /// コンポーネント固有のstop処理（オプション）
    fn on_stop(&mut self) -> Result<(), String> { Ok(()) }
}

// デフォルト実装（handle_commandのロジック）
pub fn handle_command<T: CommandHandler>(
    handler: &mut T,
    state_tx: &watch::Sender<ComponentState>,
    cmd: Command,
) -> CommandResponse {
    // 共通ロジック（~80行を1箇所に）
}
```

#### 1.3 CommandTask の統一

**問題:** `command_task()` が4箇所で95%同一

**解決策:** ジェネリック関数として抽出
```rust
// src/common/command_task.rs
pub async fn run_command_task<H, F>(
    command_address: String,
    handler: Arc<Mutex<H>>,
    state_tx: watch::Sender<ComponentState>,
    shutdown: broadcast::Receiver<()>,
    handle_fn: F,
) where
    H: Send + 'static,
    F: Fn(&mut H, &watch::Sender<ComponentState>, Command) -> CommandResponse + Send + 'static,
{
    // 共通のZMQ REP/REQループ（~80行を1箇所に）
}
```

---

### Phase 2: Statistics & Tracking (優先度: 高)

#### 2.1 SequenceTracker の統一

**問題:** `SourceStats` が Merger と DataSink で重複

**解決策:** `common/stats.rs` に統一
```rust
// src/common/stats.rs
pub struct SequenceTracker {
    pub last_sequence: Option<u64>,
    pub total_batches: u64,
    pub gaps_detected: u64,
    pub total_gap_size: u64,
    pub restart_count: u32,
}

impl SequenceTracker {
    pub fn update(&mut self, seq: u64) -> SequenceStatus {
        // 共通のギャップ検出ロジック
    }
}

pub enum SequenceStatus {
    Normal,
    Gap { missing: u64 },
    Restart,
}
```

#### 2.2 PerSourceStats の統一

```rust
// src/common/stats.rs
pub struct PerSourceStats {
    trackers: HashMap<u32, SequenceTracker>,
}

impl PerSourceStats {
    pub fn update(&mut self, source_id: u32, seq: u64) -> SequenceStatus;
    pub fn summary(&self) -> StatsSummary;
}
```

---

### Phase 3: ZMQ Helpers (優先度: 中)

#### 3.1 MessagePublisher の抽出

**問題:** Emulator と Reader で `publish_message()` が重複

**解決策:** 共通ヘルパー
```rust
// src/common/zmq_helpers.rs
pub struct MessagePublisher {
    socket: publish::Publish,
    metrics: Option<Arc<PublishMetrics>>,
}

impl MessagePublisher {
    pub async fn publish(&mut self, message: &Message) -> Result<(), ZmqError>;
    pub async fn publish_batch(&mut self, batch: MinimalEventDataBatch) -> Result<(), ZmqError>;
    pub async fn publish_eos(&mut self, source_id: u32) -> Result<(), ZmqError>;
    pub async fn publish_heartbeat(&mut self, hb: Heartbeat) -> Result<(), ZmqError>;
}
```

---

### Phase 4: Error Handling (優先度: 中)

#### 4.1 統一エラー型

**問題:** 4つの異なるエラー型 (EmulatorError, ReaderError, MergerError, DataSinkError)

**解決策:** 統一エラー型
```rust
// src/common/error.rs
#[derive(Debug, Error)]
pub enum DaqError {
    #[error("ZMQ error: {0}")]
    Zmq(#[from] tmq::TmqError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] rmp_serde::encode::Error),

    #[error("Deserialization error: {0}")]
    Deserialization(#[from] rmp_serde::decode::Error),

    #[error("State transition error: cannot go from {from} to {to}")]
    StateTransition { from: ComponentState, to: ComponentState },

    #[error("Channel send error")]
    ChannelSend,

    #[error("Hardware error: {0}")]
    Hardware(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Timeout")]
    Timeout,
}
```

**注意:** 既存エラー型は後方互換性のため `type EmulatorError = DaqError;` としてエイリアス化可能

---

### Phase 5: Component Trait (優先度: 低/将来)

#### 5.1 DaqComponent Trait

**目的:** 将来的なポリモーフィック管理のための基盤

```rust
// src/common/component.rs
#[async_trait]
pub trait DaqComponent: Send + Sync {
    fn name(&self) -> &str;
    fn state(&self) -> ComponentState;
    async fn run(self, shutdown: broadcast::Receiver<()>) -> Result<(), DaqError>;
}
```

**利点:**
- 動的コンポーネント管理（設定に基づいて起動するコンポーネントを選択）
- テスト時のモック化が容易
- 将来的なプラグインシステムの基盤

**リスク:** 大規模な変更が必要、現時点では優先度低

---

## 実装順序

```
Phase 1.1: SharedState統一        [1日] → 変更箇所: 4ファイル
    ↓
Phase 1.2: CommandHandler導入    [2日] → 変更箇所: 5ファイル (common + 4コンポーネント)
    ↓
Phase 1.3: CommandTask統一       [1日] → 変更箇所: 5ファイル
    ↓
Phase 2.1-2.2: Stats統一         [1日] → 変更箇所: 3ファイル
    ↓
Phase 3: ZMQ Helpers             [1日] → 変更箇所: 3ファイル
    ↓
Phase 4: Error統一               [0.5日] → 変更箇所: 全ファイル（エイリアス化で低リスク）
    ↓
Phase 5: Component Trait         [将来] → 大規模変更
```

---

## 期待効果

| 指標 | 現在 | Phase 1後 | 全Phase後 |
|-----|------|----------|----------|
| 重複コード行数 | ~670行 | ~170行 | ~50行 |
| テストカバレッジ | 個別テスト | 共通部分の集中テスト | 高カバレッジ |
| 新コンポーネント追加工数 | 高 | 中 | 低 |
| バグ修正工数 | 4箇所修正 | 1箇所修正 | 1箇所修正 |

---

## リスクと対策

### リスク1: 後方互換性の破壊

**対策:**
- 既存の公開APIはそのまま維持
- 内部実装のみをtraitに委譲
- エラー型はエイリアスで互換性維持

### リスク2: パフォーマンス低下

**対策:**
- Trait objectではなくジェネリクスを使用（モノモーフィック化）
- ベンチマーク比較を実施

### リスク3: 実装中のシステム不安定

**対策:**
- 各Phaseの完了時にフルテスト
- 段階的なマージ（1 Phase = 1 PR）

---

## 次のアクション

- [ ] Phase 1.1: `src/common/state.rs` を作成
- [ ] Phase 1.2: `src/common/command_handler.rs` を作成
- [ ] Phase 1.3: `src/common/command_task.rs` を作成
- [ ] 各コンポーネントを新しい共通モジュールを使用するようにリファクタリング
- [ ] 既存テストが全てパスすることを確認
