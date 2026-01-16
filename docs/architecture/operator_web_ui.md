# Operator Web UI Architecture

## Overview

DAQ制御用のWebフロントエンド。Angular + Material Design で実装。

## 目的

1. **モニタリング**: 各コンポーネントの状態をリアルタイム表示
2. **運用制御**: Configure → Start → Stop のワークフロー（Armは自動実行）

## UI レイアウト

```
┌─────────────────────────────────────────────────────────────────┐
│  DELILA DAQ Control                    [Exp: TestExp] [Run: 42] │
├────────────────────────────────┬────────────────────────────────┤
│  Status Panel                  │  Control Panel                 │
│  ┌──────────────────────────┐  │  ┌──────────────────────────┐  │
│  │ Emulator-0: Running  ●   │  │  │ Exp Name: [TestExp    ]  │  │
│  │ Emulator-1: Running  ●   │  │  │ Run #:    [42         ]  │  │
│  │ Merger:     Running  ●   │  │  │ Comment:  [          ]   │  │
│  │ Recorder:   Running  ●   │  │  ├──────────────────────────┤  │
│  │ Monitor:    Running  ●   │  │  │ [Configure] [Start]      │  │
│  └──────────────────────────┘  │  │ [Stop]      [Reset]      │  │
│                                │  │                          │  │
│  Run Info                      │  └──────────────────────────┘  │
│  ┌──────────────────────────┐  │                                │
│  │ Run #42                  │  │  Timer                         │
│  │ Started: 14:32:15        │  │  ┌──────────────────────────┐  │
│  │ Elapsed: 00:05:23        │  │  │ Duration: [__] min       │  │
│  │ Events: 1,234,567        │  │  │ [x] Auto Stop            │  │
│  │ Rate: 3.8 Mevt/s         │  │  │ [Start Timer]            │  │
│  └──────────────────────────┘  │  └──────────────────────────┘  │
│                                │                                │
│  Current State: Running        │                                │
└────────────────────────────────┴────────────────────────────────┘
```

## コンポーネント構成

```
web/operator-ui/src/app/
├── app.component.ts              # ルートコンポーネント
├── app.routes.ts                 # ルーティング
├── components/
│   ├── status-panel/             # 各コンポーネントの状態表示
│   │   └── status-panel.component.ts
│   ├── control-panel/            # Configure/Arm/Start/Stop ボタン
│   │   └── control-panel.component.ts
│   ├── run-info/                 # Run番号・時間・イベント数表示
│   │   └── run-info.component.ts
│   ├── timer/                    # タイマー機能
│   │   └── timer.component.ts
│   └── run-config/               # Exp名・Run番号・コメント入力
│       └── run-config.component.ts
├── services/
│   ├── operator.service.ts       # HTTP クライアント
│   └── timer.service.ts          # タイマーロジック
└── models/
    └── types.ts                  # 型定義
```

## 状態遷移とボタン制御

### 状態遷移図

```
Idle → [Configure] → Configured → [Start] → Running
  ↑                      ↓                      ↓
  └──── [Reset] ←───────┴───── [Stop] ←───────┘

※ Start時にバックエンドが自動的にArmを実行
```

### ボタン有効化テーブル

**注意**: Armボタンは削除され、StartがConfigured状態から直接実行可能になりました。
バックエンドが自動的にArmを実行してからStartします。

| 状態 | Configure | Start | Stop | Reset |
|------|-----------|-------|------|-------|
| Idle | ✅ | ❌ | ❌ | ❌ |
| Configured | ❌ | ✅ | ❌ | ✅ |
| Armed | ❌ | ✅ | ❌ | ✅ |
| Running | ❌ | ❌ | ✅ | ❌ |
| Error | ❌ | ❌ | ❌ | ✅ |

## 機能詳細

### 1. ステータス表示

- **ポーリング間隔**: 1秒
- **表示項目**: コンポーネント名、状態、オンライン/オフライン
- **エラー表示**: 状態を赤色表示 + ホバーでエラー詳細

### 2. Run番号管理

- **Start時にRun番号を送信**: ConfigureではなくStart APIでrun_numberを渡す
  - 理由: Configureはハードウェア設定を含み時間がかかるため
  - Legacy APIの `/DELILA/start/{runNo}` と同様の設計
- Stop後、次のStart時にRun番号を +1（Auto Incrementが有効な場合）
- フロントエンド側で管理
- チェックボックスで有効/無効切り替え可能

### 3. タイマー機能

- **入力**: 分単位
- **範囲**: 数分〜120分
- **モード**:
  - アラームのみ: 時間経過後にブラウザ通知 + 音
  - 自動Stop: チェックボックス有効時、自動で Stop API 呼び出し
- **アラーム**: ダイアログ + 音声ファイル

### 4. Run情報表示

- Run番号
- 開始時刻
- 経過時間（リアルタイム更新）
- イベント数（API から取得）
- イベントレート

## API 連携

### 使用エンドポイント

| エンドポイント | メソッド | 用途 |
|---------------|---------|------|
| `/api/status` | GET | 全コンポーネント状態取得 |
| `/api/configure` | POST | Configure (exp_name) |
| `/api/arm` | POST | Arm（通常UIからは使用しない） |
| `/api/start` | POST | Start (run_number) |
| `/api/stop` | POST | Stop |
| `/api/reset` | POST | Reset |

### レスポンス型

```typescript
interface SystemStatus {
  components: ComponentStatus[];
  system_state: SystemState;
}

interface ComponentStatus {
  name: string;
  address: string;
  state: ComponentState;
  run_number?: number;
  metrics?: ComponentMetrics;
  error?: string;
  online: boolean;
}

interface ComponentMetrics {
  events_processed: number;
  bytes_transferred: number;
  queue_size: number;
  queue_max: number;
  event_rate: number;
}
```

## デプロイ方式

### 本番環境

**方式A: rust-embed 埋め込み（デフォルト）**
- Angular ビルド成果物をバイナリに埋め込み
- 単一バイナリで完結

**方式B: 外部ディレクトリ（オプション）**
- `--static-dir /var/www/delila-ui` で指定
- UI のみ更新可能

### 開発環境

- `ng serve` (localhost:4200) でホットリロード
- CORS は Operator 側で設定済み

## 将来拡張（Phase 2）

### MongoDB 連携

```typescript
interface RunLog {
  run_number: number;
  start: number;        // Unix timestamp (ms)
  stop: number;         // Unix timestamp (ms), 0 = running
  exp_name: string;
  comment: string;
  // 将来追加
  source?: string;
  distance?: string;
}
```

### 追加エンドポイント

| エンドポイント | メソッド | 用途 |
|---------------|---------|------|
| `/api/runs` | POST | ラン記録作成 |
| `/api/runs` | GET | ラン履歴取得 |
| `/api/runs/{id}` | PUT | ラン記録更新 |

## 技術スタック

| カテゴリ | 選択 | 理由 |
|---------|------|------|
| フレームワーク | Angular 17+ | standalone components, signals |
| UI ライブラリ | Angular Material | Material Design, 豊富なコンポーネント |
| HTTP | HttpClient | Angular 標準 |
| 状態管理 | Signals | シンプル、RxJS 併用 |

## 参考実装

- Legacy Controller: `legacy/DELILA-Controller/`
- Legacy WebAPI: `legacy/DELILA-WebAPI/`
