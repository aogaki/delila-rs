# TODO: Refactoring Plan

**Status: PHASE 1 COMPLETED** (2026-01-13)
**Created:** 2026-01-13

## 概要

現在のコードベースには大量のコード重複があり、保守性とテスト容易性に課題がある。
このドキュメントでは、段階的なリファクタリング計画を定義する。

---

## Phase 1: Command Infrastructure (完了)

### 実装サマリー

| タスク | ステータス | 作成/変更ファイル |
|--------|----------|------------------|
| 1.1 SharedState統一 | **完了** | `src/common/state.rs` |
| 1.2 CommandHandlerExt導入 | **完了** | `src/common/state.rs` |
| 1.3 CommandTask統一 | **完了** | `src/common/command_task.rs` |
| Emulator移行 | **完了** | `src/data_source_emulator/mod.rs` |
| Reader移行 | **完了** | `src/reader/mod.rs` |
| Merger移行 | **完了** | `src/merger/mod.rs` |
| DataSink移行 | **完了** | `src/data_sink/mod.rs` |

### 新規作成モジュール

#### `src/common/state.rs`
- `ComponentSharedState` - 統一された共有状態構造体
- `CommandHandlerExt` trait - コンポーネント固有の拡張フック
- `handle_command()` - 共通のコマンドハンドリングロジック
- `handle_command_simple()` - 拡張なしのシンプル版

#### `src/common/command_task.rs`
- `run_command_task()` - ジェネリックなZMQ REPソケットハンドラ
- `run_command_task_with_state()` - ComponentSharedState用の簡易版

### 削減されたコード

- **4つの`handle_command()`メソッド** → 1つの共通実装
- **4つの`command_task()`メソッド** → 1つの共通実装
- **推定削減: ~500行**

### テスト結果

```
running 40 tests
test result: ok. 40 passed; 0 failed; 0 ignored
```

---

## 残りのフェーズ (保留)

### Phase 2: Statistics & Tracking (優先度: 低)

`SourceStats`の統一はMergerとDataSinkで構造が異なるため、必要性が出るまで保留。

### Phase 3: ZMQ Helpers (優先度: 低)

`publish_message()`の抽象化は、現時点では過剰設計の可能性あり。

### Phase 4: Error Handling (優先度: 低)

統一エラー型は後方互換性の観点から慎重に検討が必要。

### Phase 5: Component Trait (優先度: 将来)

動的コンポーネント管理は現時点では不要。

---

## 判断根拠

Phase 1完了後の評価:

**利点:**
- コード重複削減（約500行）
- 状態遷移ロジックの一元化
- バグ修正が1箇所で済む

**懸念点:**
- 4コンポーネントという小規模では過剰な抽象化の可能性
- MergerとDataSinkは`ext_state`と`shared_state`の2重構造に

**結論:**
Phase 1は価値があったが、残りのPhaseはROIが低い。
フロントエンド開発を優先すべき。

---

## 次のアクション

- [x] Phase 1.1: `src/common/state.rs` を作成
- [x] Phase 1.2: `CommandHandlerExt` trait を作成
- [x] Phase 1.3: `src/common/command_task.rs` を作成
- [x] 各コンポーネントを新しい共通モジュールを使用するようにリファクタリング
- [x] 既存テストが全てパスすることを確認
- [ ] **次: フロントエンド開発（デジタイザ設定・ヒストグラム表示）**
