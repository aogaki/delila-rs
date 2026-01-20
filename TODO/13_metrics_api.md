# Operator API Metrics Implementation

**Status: Planning** (2026-01-20)

---

## 背景

### 現状の問題

- 各コンポーネント（Reader, Merger, Recorder, Monitor）は内部で`AtomicU64`カウンタを持ち、リアルタイムでメトリクスを追跡している
- しかし`GetStatus`レスポンスで`metrics: None`を返している
- フロントエンドは`ComponentMetrics`を受け取る準備ができているが、データが来ない

### 根本原因

メトリクスフレームワークは**50%実装済み**：

| 部分 | 状態 | 備考 |
|------|------|------|
| AtomicCounters基盤 | ✅ 完了 | lock-free、レート計算対応 |
| コンポーネント内カウンタ | ✅ 動作中 | Reader, Merger, Recorder全て |
| ComponentMetrics構造体 | ✅ 定義済み | シリアライズ可能 |
| **スナップショット機構** | ❌ 未実装 | GetStatus時のカウンタ取得なし |
| Operator集計ロジック | ✅ 実装済み | しかしデータなしで動作せず |
| フロントエンド表示 | ✅ 準備完了 | 型定義、集計ロジックあり |

**必要なこと**: コンポーネントの内部カウンタ → `GetStatus`レスポンスへのブリッジ

---

## 現状のデータフロー（壊れている）

```
Frontend                Operator              Components
   │                       │                      │
   │  GET /api/status      │                      │
   │──────────────────────►│                      │
   │                       │  GetStatus (ZMQ)     │
   │                       │─────────────────────►│
   │                       │                      │
   │                       │  Response            │
   │                       │  metrics: None  ❌   │
   │                       │◄─────────────────────│
   │                       │                      │
   │  { components: [...], │                      │
   │    total_events: 0 }  │                      │
   │◄──────────────────────│                      │
```

---

## 目標のデータフロー

```
Frontend                Operator              Components
   │                       │                      │
   │  GET /api/status      │                      │
   │──────────────────────►│                      │
   │                       │  GetStatus (ZMQ)     │
   │                       │─────────────────────►│
   │                       │                      │ snapshot counters
   │                       │  Response            │
   │                       │  metrics: Some({..}) │
   │                       │◄─────────────────────│
   │                       │  aggregate           │
   │  { components: [...], │                      │
   │    total_events: N,   │                      │
   │    event_rate: R }    │                      │
   │◄──────────────────────│                      │
```

---

## 実装計画

### Phase 1: コンポーネント側のメトリクス返却

各コンポーネントで`GetStatus`時にメトリクスをスナップショットして返す。

#### 1.1 Reader/Emulator (`src/reader/`)

**現状:**
- `AtomicU64`カウンタ: `received`, `sent`, `dropped`
- `GetStatus`で`metrics: None`

**実装:**
```rust
// command.rs または mod.rs
fn get_metrics_snapshot(&self) -> ComponentMetrics {
    ComponentMetrics {
        events_processed: self.stats.sent.load(Ordering::Relaxed),
        bytes_transferred: self.stats.bytes.load(Ordering::Relaxed),
        queue_size: 0,  // Reader has no queue
        queue_max: 0,
        event_rate: 0.0,  // 後で計算（Phase 2）
        data_rate: 0.0,
    }
}
```

- [ ] `handle_command_simple`でGetStatus時にメトリクスを含める
- [ ] テスト追加

#### 1.2 Merger (`src/merger/mod.rs`)

**現状:**
- `AtomicU64`カウンタ: `received`, `sent`, `dropped`
- `DashMap<u32, SourceStats>`: ソースごとの統計

**実装:**
- [ ] `get_metrics_snapshot()`メソッド追加
- [ ] GetStatus時にメトリクスを含める
- [ ] テスト追加

#### 1.3 Recorder (`src/recorder/mod.rs`)

**現状:**
- `AtomicStats`: `received_batches`, `received_events`, `written_events`, `written_bytes`, etc.

**実装:**
- [ ] `get_metrics_snapshot()`メソッド追加
- [ ] GetStatus時にメトリクスを含める
- [ ] テスト追加

#### 1.4 Monitor (`src/monitor/mod.rs`)

**現状:**
- 内部統計あり（ヒストグラム更新カウント等）

**実装:**
- [ ] `get_metrics_snapshot()`メソッド追加
- [ ] GetStatus時にメトリクスを含める
- [ ] テスト追加

### Phase 2: 1秒ごとのレート計算

**現状の問題:**
- 現在のレート計算: `total_events / elapsed_secs`（ラン全体の平均）
- 最初にバーストが来ると200kHz等の高い値が出て、その後徐々に下がる
- 実際の瞬時レートは100kHz程度でも、平均が薄まるまで高く表示される

**解決策:** 各コンポーネントで1秒前のスナップショットを保持し、差分からレートを計算

**実装方法:**

```rust
// 各コンポーネントに追加
struct MetricsTracker {
    prev_snapshot: Option<(Instant, CounterSnapshot)>,
    current_rate: AtomicU64,  // 整数で保持（精度は十分）
}

impl MetricsTracker {
    fn update(&self, counters: &AtomicCounters) {
        let now = Instant::now();
        let current = counters.snapshot();

        if let Some((prev_time, prev_snap)) = &self.prev_snapshot {
            let elapsed = now.duration_since(*prev_time).as_secs_f64();
            if elapsed >= 1.0 {
                let rate = current.rate_from(prev_snap, elapsed);
                self.current_rate.store(rate.events_rate as u64, Ordering::Relaxed);
                self.prev_snapshot = Some((now, current));
            }
        } else {
            self.prev_snapshot = Some((now, current));
        }
    }

    fn get_rate(&self) -> f64 {
        self.current_rate.load(Ordering::Relaxed) as f64
    }
}
```

**既存インフラ活用:**
- `src/common/metrics.rs`の`rate_from()`メソッドが既に存在
- `CounterSnapshot`間の差分計算をサポート済み

**タスク:**
- [ ] 各コンポーネントに`MetricsTracker`を追加
- [ ] 1秒ごとにスナップショット更新（メインループで）
- [ ] GetStatus時に`current_rate`を返す

### Phase 3: フロントエンド確認

- [ ] `/api/status`のレスポンスにメトリクスが含まれることを確認
- [ ] ヘッダーの統計表示が正しく動作することを確認
- [ ] 各コンポーネントの個別メトリクス表示

---

## 技術的詳細

### CommandResponse.with_metrics()

既存の`with_metrics()`メソッドを使用:

```rust
// src/common/command.rs:220
pub fn with_metrics(mut self, metrics: super::ComponentMetrics) -> Self {
    self.metrics = Some(metrics);
    self
}
```

### 実装パターン

各コンポーネントで以下のパターンを適用:

```rust
// GetStatus handling
Command::GetStatus => {
    let metrics = self.get_metrics_snapshot();
    CommandResponse::success(&format!("State: {:?}", state))
        .with_metrics(metrics)
}
```

### lock-free要件

メトリクス取得はホットパスではないが、データ処理をブロックしてはいけない。
`AtomicU64::load(Ordering::Relaxed)`は問題なし。

---

## テスト戦略

### ユニットテスト

- [ ] 各コンポーネントの`get_metrics_snapshot()`が正しい値を返す
- [ ] GetStatusレスポンスにmetricsが含まれる

### 統合テスト

- [ ] DAQ起動 → Start → イベント生成 → GetStatus → metrics確認
- [ ] フロントエンドでの表示確認

---

## 影響範囲

### 変更が必要なファイル

| ファイル | 変更内容 |
|----------|----------|
| `src/reader/command.rs` | GetStatusでメトリクス返却 |
| `src/merger/mod.rs` | GetStatusでメトリクス返却 |
| `src/recorder/mod.rs` | GetStatusでメトリクス返却 |
| `src/monitor/mod.rs` | GetStatusでメトリクス返却 |

### 変更不要（既に実装済み）

- `src/common/mod.rs` - ComponentMetrics定義
- `src/common/command.rs` - with_metrics()メソッド
- `src/operator/routes.rs` - 集計ロジック
- フロントエンド - 表示ロジック

---

## 完了条件

1. [ ] 全コンポーネントがGetStatusで`metrics: Some({...})`を返す
2. [ ] Operator `/api/status`で集計メトリクスが表示される
3. [ ] フロントエンドのヘッダーにリアルタイム統計が表示される
4. [ ] CIパス（テスト追加）

---

## 参考

- `src/common/metrics.rs` - AtomicCountersフレームワーク
- `src/common/mod.rs:ComponentMetrics` - メトリクス構造体
- `web/operator-ui/src/app/models/types.ts` - フロントエンド型定義
