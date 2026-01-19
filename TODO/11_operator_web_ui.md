# Operator Web UI Implementation

**Status: In Progress** (2026-01-16)

**設計ドキュメント**: `docs/architecture/operator_web_ui.md`

---

## 完了済み

### Phase 0: Angular プロジェクトセットアップ ✅

- [x] Angular CLI で `web/operator-ui/` にプロジェクト作成
- [x] Angular Material インストール・設定
- [x] 基本レイアウト作成（2カラム）

### Control機能（現在のOperator UI）✅

- [x] `operator.service.ts` - HTTP クライアント実装
- [x] `timer.service.ts` - タイマーロジック
- [x] `status-panel` - コンポーネント状態表示
- [x] `control-panel` - コントロールボタン（Armボタン削除済み）
- [x] `run-info` - 現在のRun情報
- [x] `timer` - タイマー機能

### Phase 1: タブ構造導入 ✅

- [x] Angular Material Tabs導入
- [x] ルーティング設定（/control, /monitor, /waveform）
- [x] グローバルヘッダーコンポーネント作成
  - 常時表示: State, Events, Rate, Run#
- [x] NotificationService実装
- [x] 既存コンポーネントを`pages/control/`に移動
- [x] ヘッダーの統計表示連携

### Phase 3: Monitorタブ（ヒストグラム基本機能）✅

**設計変更 (2026-01-16):** Setup/View分離アーキテクチャを採用

- [x] ECharts (ngx-echarts@20.0.2) インストール・設定
- [x] Monitor API連携（histogram.service.ts）
- [x] **Setup/View分離構造** ← 新規
  - setup-tab: チャンネル配置設定、グリッドサイズ設定、「Create View」でビュー生成
  - view-tab: ヒストグラム最大表示、拡大ボタンのみ、設定変更不可
- [x] histogram-chart コンポーネント（ECharts描画）
  - バーチャート表示
  - dataZoomによる範囲選択
  - rangeChange イベント出力
- [x] 統計表示パネル（Total Events, Rate, Channels, Elapsed）
- [x] localStorage永続化（キー: delila-monitor-state）

### Phase 4: フィッティング機能 ✅

**実装日:** 2026-01-16

**方式:** 拡大ダイアログでレンジ選択・フィット実行

- [x] histogram-expand-dialog コンポーネント
  - [⤢] ボタンで開く（MatDialog）
  - 大きなチャートで精密な範囲選択（ドラッグ選択）
  - [Fit] [Clear Fit] [Reset Range] [Log/Linear] ボタン
- [x] ml-levenberg-marquardt インストール
- [x] fitting.service.ts 実装（TDD）
  - ガウス分布 + 線形バックグラウンド同時フィット
  - Levenberg-Marquardt アルゴリズム
  - χ²/ndf 計算
  - 誤差伝播（center, sigma, amplitude, netArea）
- [x] フィット曲線オーバーレイ描画（赤い線）
- [x] フィット結果テキスト表示（チャート右上）
  - Center ± error
  - Sigma ± error
  - FWHM (= 2.355 * sigma)
  - Net Area ± error
  - χ²/ndf
- [x] フィット結果のlocalStorage永続化
- [x] fitting.service.spec.ts テスト（TDD）

### Phase 5: グリッド画像保存機能 ✅

**実装日:** 2026-01-19

- [x] ツールバーに「Save Image」ボタン追加
- [x] 各チャートからECharts getDataURL()で画像取得
- [x] Canvas APIでグリッドレイアウト結合
- [x] PNGダウンロード機能

**実装ファイル:**
- `histogram-chart.component.ts`: `getDataURL()` メソッド追加
- `view-tab.component.ts`: Save Imageボタン + Canvas結合ロジック

---

### Phase 6: Waveformタブ ✅

**実装日:** 2026-01-19

- [x] 波形取得API連携（histogram.service.ts拡張）
- [x] WaveformPageComponent（ECharts描画）
- [x] Source/Channel選択ドロップダウン（複数選択可）
- [x] Analog Probe 1/2 トグル表示
- [x] チャートズーム機能
  - Shift+ホイール: X軸ズーム
  - Ctrl+ホイール: Y軸ズーム
  - 下部スライダー: X軸範囲
  - 右側スライダー: Y軸範囲
- [x] Y軸固定範囲（±20000 ADC）
- [x] 500msポーリング更新
- [x] waveform types定義（histogram.types.ts）

**実装ファイル:**
- `web/operator-ui/src/app/pages/waveform/waveform.component.ts`
- `web/operator-ui/src/app/services/histogram.service.ts`
- `web/operator-ui/src/app/models/histogram.types.ts`

**バックエンド実装（前回完了）:**
- `src/monitor/mod.rs`: waveform storage + API
- `src/data_source_emulator/mod.rs`: waveform generation
- `src/config/mod.rs`: waveform設定フィールド

---

## 未着手

### Phase 7: rust-embed統合

- [ ] `rust-embed` クレートで静的ファイル埋め込み
- [ ] Operatorバイナリでの配信設定
- [ ] ルーティング設定（`/` で UI、`/api/*` で API）

### バックエンド改善（優先度低）

- [ ] **Operator APIでmetricsを返す**
  - 現状: 各コンポーネントの`GetStatus`が`metrics: None`を返している
  - 問題: ヘッダーでevent数・rateが0表示だった
  - 暫定対応: フロントエンドでMonitor APIから統計取得（2026-01-16実装済み）
  - 根本対応: 各コンポーネントで`CommandResponse`に`ComponentMetrics`を設定
    - Reader: `events_processed`, `event_rate`
    - Merger: forwarded events
    - Recorder: written events
    - Monitor: processed events

---

## テスト戦略

Web UIの特性を考慮し、以下の段階的アプローチを採用する。

### 方針

| レイヤー | テスト手法 | タイミング |
|---------|----------|-----------|
| **サービス層** (fitting, histogram) | TDD | 実装前にテスト作成 |
| **コンポーネント層** | 統合テスト | 実装後に追加 |
| **E2E** | 手動 + Playwright | 主要フロー確認時 |

### TDD対象（数値計算・ロジック）

- `fitting.service.ts` - ガウスフィッティング計算
- `histogram.service.ts` - API呼び出し（mock）
- `monitor-tabs.service.ts` - localStorage永続化ロジック

### 実装優先（UI）

- コンポーネント（histogram-grid, histogram-cell等）
- 後からコンポーネント統合テストを追加

### 理由

1. UIは触って確認する方が早い（ヒストグラムの見た目、ズーム操作感）
2. 要件が流動的（触りながら調整が発生しやすい）
3. 数値計算ロジックはTDDの効果が高い（C++と同様）

---

## 設計決定事項

| 項目 | 決定 | 理由 |
|------|------|------|
| チャートライブラリ | ECharts (ngx-echarts) | dataZoom機能、高パフォーマンス |
| フィッティング | JavaScript (ml-levenberg-marquardt) | 4096bins/6パラメータは数十ms |
| フィットUI | ハイブリッド方式 | グリッドでサマリー、拡大モードで精密操作 |
| グリッドレイアウト | ユーザー指定NxM | 柔軟性 |
| チャンネル選択 | ドロップダウン | セルごとの配置指定が容易 |
| フィット結果 | 画面表示のみ | 将来的にファイル出力追加可 |
| Monitorサブタブ | ネストタブ構造 | 検出器ごとに設定を分離 |
| 状態永続化 | localStorage | ページリロードでも設定復元 |

---

## 型定義

### サブタブ状態

```typescript
// localStorage key: 'delila-monitor-tabs'
interface MonitorTabsState {
  tabs: MonitorTab[];
  activeTabId: string;
}

interface MonitorTab {
  id: string;              // UUID
  name: string;            // "CRIB", "LaBr3", etc.
  gridRows: number;        // 1-4
  gridCols: number;        // 1-4
  cells: HistogramCell[];  // gridRows * gridCols 個
}

interface HistogramCell {
  sourceId: number | null;   // null = 空セル
  channelId: number | null;
  xRange: { min: number; max: number } | 'auto';
  yRange: { min: number; max: number } | 'auto';
  isLocked: boolean;
}
```

### フィット結果

```typescript
interface GaussianFitResult {
  amplitude: number;
  center: number;
  sigma: number;
  leftLine: { slope: number; intercept: number };
  rightLine: { slope: number; intercept: number };
  bgLine: { slope: number; intercept: number };
  fwhm: number;
  area: number;
  chi2: number;
}
```

---

## 参考

- 設計ドキュメント: `docs/architecture/operator_web_ui.md`
- Legacy UI: `legacy/DELILA-Controller/`
- Legacy API: `legacy/DELILA-WebAPI/`
