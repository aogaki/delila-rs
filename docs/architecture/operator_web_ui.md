# Operator Web UI Architecture

## Overview

DAQ制御用のWebフロントエンド。Angular + Material Design で実装。
**タブベースSPA**として、Control/Monitor/Waveform機能を統合。

## 目的

1. **運用制御**: Configure → Start → Stop のワークフロー（Armは自動実行）
2. **モニタリング**: 各コンポーネントの状態をリアルタイム表示
3. **データ可視化**: ヒストグラム・波形のリアルタイム表示

## 設計方針

### タブベースSPAを選択した理由

| 方式 | メリット | デメリット |
|------|---------|------------|
| **タブベースSPA** ✅ | 各タブが独立、画面広く使える、段階的実装可能 | タブ切り替えが必要 |
| シングルページ | 全情報同時表示 | 画面が狭くなる、情報過多 |
| 別アプリ分離 | 独立運用可能 | 状態共有困難、複数URL管理 |

### グローバル通知で切り替え問題を解決

どのタブにいても重要な情報は見える：
- **ヘッダー**: 常時表示（State, Events, Rate, Run#）
- **通知**: タイマー終了、エラー発生、Run停止

---

## UI レイアウト

### 全体構成

```
┌─────────────────────────────────────────────────────────────────┐
│  DELILA DAQ    Running ● | 1.2M events | 3.8 Mev/s   [Run: 42] │  ← ヘッダー（常時表示）
├─────────────────────────────────────────────────────────────────┤
│  [Control]  [Monitor]  [Waveform]  [Settings]                   │  ← タブ
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│                    ← 選択タブの内容 →                            │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### ヘッダー詳細

```
┌─────────────────────────────────────────────────────────────────┐
│  DELILA DAQ    [State●]  [Events]  [Rate]        [Exp] [Run#]  │
│                Running●   1.2M      3.8 Mev/s    CRIB   42     │
└─────────────────────────────────────────────────────────────────┘
     ↑              ↑         ↑          ↑           ↑      ↑
   ロゴ          状態      イベント数   レート      実験名  Run番号
                (色付き)   (Running時)  (Running時)
```

- **状態**: 色付きバッジ（緑=Running, 青=Configured, 灰=Idle, 赤=Error）
- **Events/Rate**: Running状態の時のみ表示
- **Exp/Run#**: 常時表示

---

## タブ構成

### 1. Control タブ（現在のOperator UI）

```
┌───────────────────────────────┬───────────────────────────────┐
│  Component Status             │  Run Control                  │
│  ┌─────────────────────────┐  │  ┌─────────────────────────┐  │
│  │ Emulator-0: Running ●   │  │  │ Exp Name: [CRIB      ]  │  │
│  │ Emulator-1: Running ●   │  │  │ Run #:    [42        ]  │  │
│  │ Merger:     Running ●   │  │  │ Comment:  [         ]   │  │
│  │ Recorder:   Running ●   │  │  ├─────────────────────────┤  │
│  │ Monitor:    Running ●   │  │  │ [Configure] [Start]     │  │
│  └─────────────────────────┘  │  │ [Stop]      [Reset]     │  │
│                               │  └─────────────────────────┘  │
│  Run Info                     │                               │
│  ┌─────────────────────────┐  │  Timer                        │
│  │ Run: 42                 │  │  ┌─────────────────────────┐  │
│  │ Started: 14:32:15       │  │  │ Duration: [__] min      │  │
│  │ Elapsed: 00:05:23       │  │  │ [x] Auto Stop           │  │
│  │ Events: 1,234,567       │  │  │ [Start with Timer]      │  │
│  │ Rate: 3.8 Mev/s         │  │  └─────────────────────────┘  │
│  └─────────────────────────┘  │                               │
└───────────────────────────────┴───────────────────────────────┘
```

### 2. Monitor タブ（ヒストグラム + 統計）

#### ネストタブ構造

Monitorタブ内にサブタブを持ち、検出器ごとに異なるヒストグラム設定を管理できる。

```
┌─────────────────────────────────────────────────────────────────┐
│  DELILA DAQ    Running ● | 1.2M events | 3.8 Mev/s   [Run: 42] │
├─────────────────────────────────────────────────────────────────┤
│  [Control]  [Monitor]  [Waveform]  [Settings]                   │  ← メインタブ
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │ [CRIB] [LaBr3] [Si Array] [+]                       [×]  │  │  ← サブタブ
│  ├──────────────────────────────────────────────────────────┤  │
│  │  Grid: [2] x [2]    [Apply Range to All]  [Reset All]    │  │
│  │  ┌─────────────┐  ┌─────────────┐                        │  │
│  │  │ [Src0/Ch0▼] │  │ [Src0/Ch1▼] │                        │  │
│  │  │  ▁▂▃▅▇█     │  │  ▁▂▃▅▇█     │                        │  │
│  │  │  [Fit][Clr] │  │  [Fit][Clr] │                        │  │
│  │  └─────────────┘  └─────────────┘                        │  │
│  │  ┌─────────────┐  ┌─────────────┐                        │  │
│  │  │ [Src0/Ch2▼] │  │ [Src0/Ch3▼] │                        │  │
│  │  │  ▁▂▃▅▇█     │  │  ▁▂▃▅▇█     │                        │  │
│  │  │  [Fit][Clr] │  │  [Fit][Clr] │                        │  │
│  │  └─────────────┘  └─────────────┘                        │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                 │
│  Statistics                        Per-Source Stats             │
│  ┌───────────────────────────┐     ┌───────────────────────────┐│
│  │ Total Events: 1,234,567   │     │ Source 0:  600,000  (49%) ││
│  │ Event Rate:   3.8 Mev/s   │     │ Source 1:  634,567  (51%) ││
│  └───────────────────────────┘     └───────────────────────────┘│
└─────────────────────────────────────────────────────────────────┘
```

#### サブタブ操作

| 操作 | 説明 |
|------|------|
| **[+]** | 新しいサブタブを追加（名前入力ダイアログ） |
| **[×]** | 現在のサブタブを削除（確認ダイアログ、最後の1つは削除不可） |
| **ダブルクリック** | タブ名をリネーム |
| **ドラッグ** | タブの順序を変更（将来実装） |

#### 状態永続化（localStorage）

サブタブの設定はブラウザのlocalStorageに保存され、ページリロード後も復元される。

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
  // フィット結果は永続化しない（データ依存のため）
}
```

**永続化タイミング:**
- サブタブ追加/削除/リネーム時
- グリッドサイズ変更時
- チャンネル選択変更時
- 範囲ロック状態変更時

**将来拡張:** サーバー側保存で複数PC間での設定共有

#### グリッドレイアウト機能

- **行数・列数指定**: ユーザーが NxM グリッドを指定（1x1 〜 4x4）
- **チャンネル選択**: 各セルにドロップダウンで Source/Channel を選択
- **空セル**: 「--」を選択で空欄表示

```
チャンネル選択ドロップダウン:
[Source 0 / Ch 0  ▼]
├── Source 0 / Ch 0
├── Source 0 / Ch 1
├── ...
├── Source 1 / Ch 0
├── Source 1 / Ch 1
└── -- (空欄)
```

#### 範囲保持機能

ユーザーがズーム/ドラッグで範囲変更した場合、オートリフレッシュ時も範囲を維持する。

```typescript
interface HistogramState {
  sourceId: number;
  channelId: number;
  xRange: { min: number; max: number } | 'auto';
  yRange: { min: number; max: number } | 'auto';
  isLocked: boolean;  // ロック中は自動更新でも範囲維持
}
```

- ユーザーがドラッグ/ズームで範囲変更 → `isLocked = true`
- リフレッシュ時もロック状態なら範囲維持
- 「Reset」ボタンで `isLocked = false` に戻す

#### 範囲一括適用

```
[Apply Range to All] ボタン
  ↓
現在選択中（フォーカス中）のヒストグラムの範囲を
同一ページ内の全ヒストグラムに適用
```

#### フィッティング機能 ✅ (実装済み: 2026-01-16)

ガウス分布 + 線形バックグラウンドによるピークフィッティング。
**拡大ダイアログ方式**: [⤢]ボタンでダイアログを開き、レンジ選択・フィット実行。

##### フィット操作フロー

```
グリッド表示                              拡大ダイアログ（MatDialog）
┌─────────────┐                          ┌─────────────────────────────────────┐
│ [Src0/Ch0▼] │                          │  Source 0 / Channel 0         [×]  │
│  ▁▂▃▅▇█     │   [⤢] ボタン            │  [Fit] [Clear Fit] [Reset] [Log]   │
│             │  ─────────────────►      │  ┌─────────────────────────────────┐│
│         [⤢] │                          │  │        ▁▂▃▅▇█▇▅▃▂▁              ││
└─────────────┘                          │  │      (大きなチャート)            ││
                                         │  │      ドラッグで範囲選択          ││
                                         │  │      赤線: フィット曲線          ││
                                         │  │  ┌───────────────────┐           ││
                                         │  │  │Center: 1523.4±2.1 │           ││
                                         │  │  │Sigma:  45.2±1.8   │ 右上表示  ││
                                         │  │  │FWHM:   106.5      │           ││
                                         │  │  │Area:   12345±234  │           ││
                                         │  │  │χ²/ndf: 1.23       │           ││
                                         │  │  └───────────────────┘           ││
                                         │  └─────────────────────────────────┘│
                                         │  Total: 1,234,567  UF: 0  OF: 123   │
                                         └─────────────────────────────────────┘
```

##### 操作方法

| 操作 | 場所 | 説明 |
|------|------|------|
| **[⤢] ボタン** | グリッドセル | 拡大ダイアログを開く |
| **ドラッグ選択** | 拡大ダイアログ | フィット範囲を選択（X軸のみ） |
| **Ctrl+スクロール** | 拡大ダイアログ | X軸ズーム |
| **[Fit]** | 拡大ダイアログ | フィット実行 |
| **[Clear Fit]** | 拡大ダイアログ | フィット結果をクリア |
| **[Reset Range]** | 拡大ダイアログ | 範囲をautoに戻す |
| **[Log/Linear]** | 拡大ダイアログ | Y軸スケール切り替え |
| **[×] / ダイアログ外クリック** | 拡大ダイアログ | 閉じる（結果は保持） |

##### フィッティングモデル

```
    ガウス分布 + 線形バックグラウンド

    y = A * exp(-0.5 * ((x - μ) / σ)²) + m*x + b

    パラメータ:
    - A (amplitude): ガウス振幅
    - μ (center): ピーク中心位置
    - σ (sigma): 標準偏差
    - m (slope): BGの傾き
    - b (intercept): BGの切片
```

```typescript
// ViewCellFitResult (localStorage永続化用)
interface ViewCellFitResult {
  center: number;
  centerError: number;
  sigma: number;
  sigmaError: number;
  fwhm: number;              // = 2.355 * sigma
  netArea: number;           // ガウス部分の積分
  netAreaError: number;
  chi2: number;
  ndf: number;
  bgLine: { slope: number; intercept: number };
  amplitude: number;
}
```

##### フィッティング手順

1. グリッドセルの[⤢]ボタンをクリック → 拡大ダイアログを開く
2. 拡大チャート上でピーク範囲をドラッグ選択（Xレンジがロック）
3. [Fit] ボタンでフィット実行
   - 選択範囲内のデータのみを使用
   - ガウス + 線形BGを同時にLevenberg-Marquardt法でフィット
   - 赤い曲線でフィット結果を描画
   - 右上に結果テキストを表示
4. ダイアログを閉じる（フィット結果はViewCellに保存、localStorage永続化）

##### 技術選択

- **計算場所:** JavaScript（ml-levenberg-marquardt）でフロントエンド計算
- **性能:** 4096 bins、5パラメータのフィットは数十ms以内で完了
- **利点:** APIラウンドトリップ不要でレスポンス良好
- **テスト:** TDDで実装（fitting.service.spec.ts）

##### 実装ファイル

| ファイル | 説明 |
|---------|------|
| `histogram-expand-dialog.component.ts` | 拡大ダイアログ（MatDialog） |
| `fitting.service.ts` | Levenberg-Marquardtフィット計算 |
| `fitting.service.spec.ts` | フィットロジックのユニットテスト |
| `histogram-chart.component.ts` | フィット曲線描画 + 結果テキスト表示 |
| `histogram.types.ts` | ViewCellFitResult型定義 |

##### 将来拡張（未実装）

- グリッドセルにフィットサマリー表示（C:xxxx, FWHM:xxx）
- フィット済みセルの視覚的区別（青枠など）
- フィット結果のCSVエクスポート

### 3. Waveform タブ（波形表示）

```
┌─────────────────────────────────────────────────────────────────┐
│  Latest Waveform                     Source: [0 ▼]  Ch: [0 ▼]   │
│  ┌─────────────────────────────────────────────────────────────┐│
│  │                                                             ││
│  │        ╱╲                                                   ││
│  │       ╱  ╲                                                  ││
│  │      ╱    ╲                                                 ││
│  │     ╱      ╲____                                            ││
│  │ ___╱            ╲___________________________________        ││
│  │                                                             ││
│  │ ─────────────────────────────────────────────────────────  ││
│  │ 0        100       200       300       400       500 [ns]  ││
│  └─────────────────────────────────────────────────────────────┘│
│                                                                 │
│  Waveform Info                                                  │
│  ┌─────────────────────────────────────────────────────────────┐│
│  │ Timestamp: 1705412345678901234  |  Energy: 1842  |  PSD: 0.3││
│  └─────────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────────┘
```

### 4. Settings タブ（将来実装）

- デジタイザ設定
- トリガー設定
- チャンネル有効/無効

---

## コンポーネント構成

```
web/operator-ui/src/app/
├── app.ts                        # ルートコンポーネント（タブ管理）
├── app.routes.ts                 # ルーティング
├── layout/
│   └── header/                   # グローバルヘッダー
│       └── header.component.ts
├── pages/
│   ├── control/                  # Controlタブ
│   │   └── control.component.ts
│   ├── monitor/                  # Monitorタブ
│   │   └── monitor.component.ts
│   ├── waveform/                 # Waveformタブ
│   │   └── waveform.component.ts
│   └── settings/                 # Settingsタブ（将来）
│       └── settings.component.ts
├── components/
│   ├── status-panel/             # コンポーネント状態表示
│   ├── control-panel/            # Configure/Start/Stop ボタン
│   ├── run-info/                 # Run情報表示
│   ├── timer/                    # タイマー機能
│   ├── monitor-subtabs/          # Monitorサブタブ管理（追加/削除/リネーム）
│   ├── histogram-grid/           # ヒストグラムグリッドコンテナ
│   ├── histogram-cell/           # 個別ヒストグラムセル（サマリー表示）
│   ├── histogram-chart/          # ヒストグラム描画（ECharts）
│   ├── histogram-expand-dialog/  # 拡大/フィットモード（モーダル）
│   ├── fit-result/               # フィット結果表示パネル
│   └── waveform-chart/           # 波形表示
├── services/
│   ├── operator.service.ts       # Operator API クライアント
│   ├── monitor.service.ts        # Monitor API クライアント
│   ├── monitor-tabs.service.ts   # サブタブ状態管理 + localStorage永続化
│   ├── histogram.service.ts      # ヒストグラムデータ取得
│   ├── fitting.service.ts        # ガウスフィッティング計算
│   ├── timer.service.ts          # タイマーロジック
│   └── notification.service.ts   # グローバル通知
└── models/
    ├── types.ts                  # 型定義
    └── histogram.types.ts        # ヒストグラム・サブタブ関連型定義
```

### ヒストグラム関連コンポーネント詳細

```
monitor-subtabs/
├── monitor-subtabs.component.ts   # サブタブバー（追加/削除/リネーム）
├── monitor-subtabs.component.html
├── monitor-subtabs.component.scss
├── add-tab-dialog/                # タブ追加ダイアログ
│   └── add-tab-dialog.component.ts
└── rename-tab-dialog/             # タブリネームダイアログ
    └── rename-tab-dialog.component.ts

histogram-grid/
├── histogram-grid.component.ts   # グリッドレイアウト管理
├── histogram-grid.component.html # NxMグリッド描画
└── histogram-grid.component.scss # グリッドスタイル

histogram-cell/
├── histogram-cell.component.ts   # 個別セル（チャンネル選択、サマリー表示、拡大ボタン）
├── histogram-cell.component.html
└── histogram-cell.component.scss

histogram-chart/
├── histogram-chart.component.ts  # ECharts描画、ズーム/範囲管理
├── histogram-chart.component.html
└── histogram-chart.component.scss

histogram-expand-dialog/
├── histogram-expand-dialog.component.ts   # 拡大モード（モーダル）
├── histogram-expand-dialog.component.html # 大きなチャート + フィットUI
└── histogram-expand-dialog.component.scss

fit-result/
├── fit-result.component.ts       # フィット結果表示パネル（詳細版）
├── fit-result.component.html
└── fit-result.component.scss
```

---

## グローバル通知システム

### 通知種類

| イベント | 通知方法 | 詳細 |
|---------|---------|------|
| タイマー終了 | Snackbar + 音 + ブラウザ通知 | 手動で閉じるまで表示 |
| エラー発生 | Snackbar + ヘッダー赤表示 | 5秒後自動消去 |
| Auto-stop完了 | Snackbar | 3秒後自動消去 |
| 接続エラー | Snackbar | 継続表示 |

### 実装

```typescript
@Injectable({ providedIn: 'root' })
export class NotificationService {
  constructor(private snackBar: MatSnackBar) {}

  notifyTimerComplete(autoStopped: boolean) {
    this.playAlarmSound();
    const message = autoStopped
      ? 'Timer completed - Run stopped automatically'
      : 'Timer completed!';
    this.snackBar.open(message, 'OK', { duration: 0 });
    this.showBrowserNotification('DELILA DAQ', message);
  }

  notifyError(message: string) {
    this.snackBar.open(`Error: ${message}`, 'Close', {
      duration: 5000,
      panelClass: 'error-snackbar'
    });
  }
}
```

---

## 状態遷移とボタン制御

### 状態遷移図

```
Idle → [Configure] → Configured → [Start] → Running
  ↑                      ↓                      ↓
  └──── [Reset] ←───────┴───── [Stop] ←───────┘

※ Start時にバックエンドが自動的にArmを実行
```

### ボタン有効化テーブル

| 状態 | Configure | Start | Stop | Reset |
|------|-----------|-------|------|-------|
| Idle | ✅ | ❌ | ❌ | ❌ |
| Configured | ❌ | ✅ | ❌ | ✅ |
| Armed | ❌ | ✅ | ❌ | ✅ |
| Running | ❌ | ❌ | ✅ | ❌ |
| Error | ❌ | ❌ | ❌ | ✅ |

---

## API 連携

### Operator API（既存）

| エンドポイント | メソッド | 用途 |
|---------------|---------|------|
| `/api/status` | GET | 全コンポーネント状態取得 |
| `/api/configure` | POST | Configure (exp_name) |
| `/api/start` | POST | Start (run_number) |
| `/api/stop` | POST | Stop |
| `/api/reset` | POST | Reset |

### Monitor API（追加予定）

| エンドポイント | メソッド | 用途 |
|---------------|---------|------|
| `/histogram` | GET | ヒストグラムデータ取得 |
| `/waveform` | GET | 最新波形取得 |
| `/stats` | GET | 統計情報取得 |

---

## 技術スタック

| カテゴリ | 選択 | 理由 |
|---------|------|------|
| フレームワーク | Angular 17+ | standalone components, signals |
| UI ライブラリ | Angular Material | Material Design, タブ/Snackbar |
| チャートライブラリ | ngx-charts or ECharts | Material風、Angular統合 |
| HTTP | HttpClient | Angular 標準 |
| 状態管理 | Signals | シンプル、RxJS 併用 |

### チャートライブラリ比較

| ライブラリ | Material風 | Angular統合 | リアルタイム | ズーム/範囲 | 選択 |
|-----------|-----------|-------------|-------------|-------------|------|
| ngx-charts | ◎ | ◎ | ○ | △ | - |
| **ECharts (ngx-echarts)** | ○ | ○ | ◎ | **◎** | **✅ 採用** |
| Chart.js (ng2-charts) | △ | ○ | ○ | ○ | - |
| Plotly.js | △ | △ | ◎ | ◎ | - |

**ECharts採用理由:**
- **dataZoom**: 組み込みのズーム/パン機能で範囲選択が容易
- **高パフォーマンス**: 大量データポイント（4096 bins）でも60fps維持
- **カスタマイズ性**: フィット曲線のオーバーレイ描画が容易
- **イベントハンドリング**: ズーム変更イベントで範囲ロック状態を管理

### フィッティングライブラリ

| ライブラリ | 用途 | 備考 |
|-----------|------|------|
| **ml-levenberg-marquardt** | 非線形最小二乗法 | ガウスフィッティング用 |
| simple-statistics | 線形回帰 | 左右直線フィット用（オプション） |

---

## デプロイ方式

### 本番環境

**方式A: rust-embed 埋め込み（推奨）**
- Angular ビルド成果物をバイナリに埋め込み
- 単一バイナリで完結
- Operator + Monitor UIを1つのバイナリで配信

**方式B: 外部ディレクトリ（オプション）**
- `--static-dir /var/www/delila-ui` で指定
- UI のみ更新可能

### 開発環境

- `ng serve` (localhost:4200) でホットリロード
- CORS は Operator 側で設定済み

---

## 実装フェーズ

### Phase 1: タブ構造導入
- [ ] Angular Material Tabs導入
- [ ] ルーティング設定（/control, /monitor, /waveform）
- [ ] グローバルヘッダーコンポーネント
- [ ] NotificationService

### Phase 2: 現在のUIをControlタブに移行
- [ ] 既存コンポーネントをpages/control/に移動
- [ ] ヘッダーの統計表示連携

### Phase 3: Monitorタブ実装（ヒストグラム基本機能）
- [ ] ECharts (ngx-echarts) インストール・設定
- [ ] Monitor API連携（histogram.service.ts）
- [ ] histogram-grid コンポーネント（NxMグリッド）
- [ ] histogram-cell コンポーネント（チャンネル選択ドロップダウン）
- [ ] histogram-chart コンポーネント（ECharts描画）
- [ ] 統計表示パネル

### Phase 4: Monitorサブタブ機能
- [ ] monitor-subtabs コンポーネント（タブバー）
- [ ] monitor-tabs.service.ts（サブタブ状態管理）
- [ ] サブタブ追加ダイアログ（名前入力）
- [ ] サブタブ削除（確認ダイアログ、最後の1つは削除不可）
- [ ] サブタブリネーム（ダブルクリック）
- [ ] localStorage永続化（タブ設定の保存・復元）

### Phase 5: ヒストグラム高度機能
- [ ] 範囲保持機能（ズーム時のロック状態管理）
- [ ] 「Apply Range to All」ボタン（範囲一括適用）
- [ ] 「Reset All」ボタン（全ヒストグラムを自動範囲に戻す）
- [ ] オートリフレッシュ時の範囲維持

### Phase 6: フィッティング機能（ハイブリッド方式）
- [ ] histogram-expand-dialog コンポーネント（拡大モード/モーダル）
- [ ] ダブルクリック / [⤢] ボタンで拡大モードを開く
- [ ] ml-levenberg-marquardt インストール
- [ ] fitting.service.ts（ガウス + 直線フィット計算）
- [ ] 拡大モードでのピーク範囲選択UI（ドラッグ選択）
- [ ] フィット曲線オーバーレイ描画（拡大モード + グリッドセル）
- [ ] fit-result コンポーネント（Center, Sigma, FWHM, Area, χ²表示）
- [ ] グリッドセルにフィットサマリー表示（C:xxxx, FWHM:xxx）
- [ ] フィット済みセルの視覚的区別（青枠など）

### Phase 7: Waveformタブ実装
- [ ] 波形取得API連携
- [ ] 波形チャートコンポーネント（ECharts）
- [ ] Source/Channel選択

### Phase 8: rust-embed統合
- [ ] Angularビルドをバイナリに埋め込み
- [ ] 静的ファイル配信ルート追加
- [ ] `/api/*` と `/` のルーティング設定

---

## 将来拡張

### MongoDB 連携（Phase 2）

```typescript
interface RunLog {
  run_number: number;
  start: number;        // Unix timestamp (ms)
  stop: number;         // Unix timestamp (ms), 0 = running
  exp_name: string;
  comment: string;
}
```

### 追加エンドポイント

| エンドポイント | メソッド | 用途 |
|---------------|---------|------|
| `/api/runs` | POST | ラン記録作成 |
| `/api/runs` | GET | ラン履歴取得 |
| `/api/runs/{id}` | PUT | ラン記録更新 |

---

## 参考実装

- Legacy Controller: `legacy/DELILA-Controller/`
- Legacy WebAPI: `legacy/DELILA-WebAPI/`
