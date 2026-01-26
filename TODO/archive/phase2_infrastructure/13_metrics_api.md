# Operator API Metrics Implementation

**Status: COMPLETED** (2026-01-20)

---

## 実装サマリー

### 完了した実装

1. **CommandHandlerExt トレイトに `get_metrics()` メソッド追加**
   - `src/common/state.rs`: トレイトに `get_metrics()` を追加
   - `handle_command()` の GetStatus 処理で `get_metrics()` を呼び出し

2. **各コンポーネントに RateTracker 追加（1秒ごとの瞬時レート計算）**
   - `src/data_source_emulator/mod.rs`: RateTracker 追加、get_metrics() 実装
   - `src/reader/mod.rs`: RateTracker 追加、get_metrics() 実装
   - `src/recorder/mod.rs`: RateTracker 追加、get_metrics() 実装
   - `src/merger/mod.rs`: get_metrics() 実装（レート計算なし）
   - `src/monitor/mod.rs`: get_metrics() 実装（レート計算なし）

3. **Operator API 修正**
   - `src/operator/routes.rs`: Recorder を権威ソースとして使用（合計イベント/バイト）

4. **フロントエンド更新**
   - `operator.service.ts`: `recorderMetrics()`, `totalEvents()`, `totalRate()` を Recorder から取得
   - `status-panel.component.ts`: 各コンポーネントのメトリクス・レート表示
   - `app.ts`: Header で Operator の統計を使用
   - `run-info.component.ts`: 瞬時レート表示（`eve/s` 単位）

### RateTracker 実装パターン

```rust
struct RateTracker {
    prev_events: AtomicU64,
    prev_time: std::sync::Mutex<Option<Instant>>,
    current_rate: AtomicU64,
}

impl RateTracker {
    fn update(&self, current_events: u64) {
        let now = Instant::now();
        let mut prev_time_guard = self.prev_time.lock().unwrap();
        if let Some(prev_time) = *prev_time_guard {
            let elapsed = now.duration_since(prev_time).as_secs_f64();
            if elapsed >= 1.0 {
                let prev_events = self.prev_events.load(Ordering::Relaxed);
                let delta = current_events.saturating_sub(prev_events);
                let rate = (delta as f64 / elapsed) as u64;
                self.current_rate.store(rate, Ordering::Relaxed);
                self.prev_events.store(current_events, Ordering::Relaxed);
                *prev_time_guard = Some(now);
            }
        } else {
            self.prev_events.store(current_events, Ordering::Relaxed);
            *prev_time_guard = Some(now);
        }
    }
}
```

---

## 変更ファイル一覧

| ファイル | 変更内容 |
|----------|----------|
| `src/common/state.rs` | `get_metrics()` トレイトメソッド追加 |
| `src/data_source_emulator/mod.rs` | RateTracker、get_metrics() 実装 |
| `src/reader/mod.rs` | RateTracker、get_metrics() 実装 |
| `src/recorder/mod.rs` | RateTracker、get_metrics() 実装 |
| `src/merger/mod.rs` | get_metrics() 実装 |
| `src/monitor/mod.rs` | get_metrics() 実装 |
| `src/operator/routes.rs` | Recorder を権威ソースとして使用 |
| `web/operator-ui/src/app/services/operator.service.ts` | Recorder メトリクス使用 |
| `web/operator-ui/src/app/components/status-panel/` | 個別メトリクス表示 |
| `web/operator-ui/src/app/app.ts` | Header 統一 |
| `web/operator-ui/src/app/components/run-info/` | 瞬時レート表示 |

---

## 設計決定

| 決定 | 選択 | 理由 |
|------|------|------|
| 権威ソース | Recorder | 実際に記録されたイベント数が正確 |
| レート計算 | 1秒ごとの差分 | ラン平均より瞬時レートが有用 |
| RateTracker の Mutex | prev_time のみ | 最小限のロック、AtomicU64 でレート保持 |

---

## 完了条件チェック

- [x] 全コンポーネントが GetStatus で `metrics: Some({...})` を返す
- [x] Operator `/api/status` で集計メトリクスが表示される
- [x] フロントエンドのヘッダーにリアルタイム統計が表示される
- [x] Component Status に各コンポーネントの個別メトリクス表示
- [x] 1秒ごとの瞬時レート計算
- [x] CI パス（テスト更新済み）
