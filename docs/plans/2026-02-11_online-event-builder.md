# Event Builder オンライン化 実装計画

> **❌ SUPERSEDED**: `online_event_builder_v2.md` → `event_builder_unified.md` と 2 段階で置換済み。2026-08-04 に EB 自体が C++(root_sink 系譜)へ移行決定([TODO 66](../../TODO/event-builder/66_cpp_event_builder.md))。歴史的記録。

## Context

Event Builder は現在オフラインツール（ROOT ファイル → SliceBuilder → ROOT 出力）として実装済み。
March MVP では Merger PUB に SUB 接続するオンラインコンポーネントとして統合し、
リアルタイムでイベントビルドを実行して ROOT ファイルに出力する。

既存の `SliceBuilder`、`TimeSortBuffer`、`Hit::from_event_data()` は全てそのまま再利用する。
新規コードは主にコンポーネントの「殻」（4-task パターン）のみ。

## Architecture

```
Merger PUB (tcp://*:5557)
    ↓ (ZMQ SUB)
OnlineEventBuilder
    ├── Receiver Task:  ZMQ SUB → mpsc → ProcessTask
    ├── Process Task:   EventData→Hit変換 → TimeSortBuffer → SliceBuilder → WriterTask
    ├── Writer Task:    Vec<BuiltEvent> → ROOT file (oxyroot)
    └── Command Task:   ZMQ REP (Operator 制御)
```

## Phase 1: 設定インフラ (EB-3 partial)

### 1-1. `src/config/mod.rs` — EventBuilderNetworkConfig 追加

`NetworkConfig` 構造体に `event_builder: Option<EventBuilderNetworkConfig>` を追加。

```rust
pub struct EventBuilderNetworkConfig {
    pub subscribe: String,                    // Merger PUB address
    pub command: Option<String>,              // ZMQ REP (default: tcp://*:5595)
    pub output_dir: String,                   // ROOT file output dir
    pub coincidence_window_ns: f64,           // default 500
    pub slice_duration_ns: f64,               // default 10_000_000 (10ms)
    pub buffer_delay_ns: f64,                 // TimeSortBuffer delay (default 5_000_000 = 5ms)
                                              // ★ Gemini指摘: 100μs→5ms に拡大（OS/NWジッター対策）
    pub ch_settings_file: Option<String>,     // Channel config JSON path
    pub time_calib_file: Option<String>,      // Time calibration JSON path
    pub pipeline_order: u32,                  // default 3
}
```

### 1-2. TOML config に `[network.event_builder]` セクション追加

```toml
[network.event_builder]
subscribe = "tcp://localhost:5557"
command = "tcp://*:5595"
output_dir = "./data/events"
coincidence_window_ns = 500.0
slice_duration_ns = 10000000.0
pipeline_order = 3
```

## Phase 2: オンライン EB コアモジュール (EB-1, EB-2)

### 2-1. `src/event_builder/online.rs` — 新規作成（メインファイル）

Recorder (`src/recorder/mod.rs`) パターンに準拠した 4-task 構造。

**Task 1: Receiver Task** — `src/recorder/mod.rs:700` と同一パターン
- ZMQ SUB → `Message` デシリアライズ → mpsc channel
- Running 状態でなければ discard（ZMQ バッファ排出のため常に受信）
- `EventDataBatch` → `ProcessCommand::DataBatch`
- EOS → `ProcessCommand::EndOfStream`

**Task 2: Process Task** — Sliding Window パターン（Gemini 協議で修正）

元の設計では `slice_duration` ごとに `build_events()` を呼ぶ想定だったが、
**SliceBuilder の overlap 領域のトリガーが先送りされ消失する問題**が判明。
Sliding Window 方式で解決する。

**データフロー:**
- `EventData` → `Hit::from_event_data()` で変換（既存: `src/event_builder/hit.rs:37`）
- `TimeSortBuffer` にヒットを挿入（既存: `src/event_builder/time_sort.rs`）
- `drain_ready()` で時間ソート済みヒットを `staging_buffer: VecDeque<Hit>` に蓄積

**Sliding Window ロジック（コア）:**
```
cursor = next_slice_start_time (初回は最初のヒットの時刻)
overlap = 2 × coincidence_window_ns  ★ Gemini指摘: jitter対策で2倍に拡大

1. staging_buffer に cursor + slice_duration + overlap を超えるデータが溜まるまで待つ
2. [cursor, cursor + slice_duration + overlap] のヒットを抽出（overlap 部分はコピー）
3. SliceBuilder::build_events() に渡す
   → SliceBuilder 内部: core=[cursor, cursor+slice_duration], overlap に先送りされるトリガーあり
4. ★ 出力フィルタ: trigger_time < cursor + slice_duration のイベントのみ保持
   （overlap 領域のトリガーを防御的に除外 → 次の呼び出しで core として処理）
5. cursor を slice_duration だけ進める
6. staging_buffer から cursor より古いヒットを prune
7. overlap 部分のヒットは次の呼び出しで core 領域に入り、先送りトリガーが処理される
```

**staging_buffer メモリ保護:**
- `staging_buffer.len()` がしきい値（例: 5秒分のデータ）を超えた場合は古いデータを drop + 警告ログ
- OOM 防止のためのセーフガード

**スパースデータ対応:** TimeSortBuffer の watermark が window を超えてもデータが不足する場合、
wall-clock タイムアウト（例: 2× slice_duration）で強制フラッシュ。

- EOS 受信時: `TimeSortBuffer::flush()` → 残データ全て `build_events()` → `CloseFile`

**★ SliceBuilder は spawn_blocking で実行**（Gemini 最終レビュー指摘）:
rayon の並列処理が tokio ランタイムをブロックするため、
`tokio::task::spawn_blocking` でラップ必須。Receiver/ZMQ ハートビートを阻害しない。

**Task 3: Writer Task** — ROOT ファイル出力（Gemini 協議 #3 で修正）

既存の `write_events_to_root()` はファイルを完全に開いて閉じる設計。
ストリーミング用に **ステートフルな `RootStreamWriter`** を新規作成する。

```rust
struct RootStreamWriter {
    // oxyroot file handle を保持
    fn new(path: &Path) -> Result<Self>   // ファイルオープン + TTree 定義
    fn append(&mut self, events: &[BuiltEvent]) -> Result<()>  // バッチ追記
    fn close(self) -> Result<()>          // TTree finalize + ファイルクローズ
}
```

- Run Start → `new()` でファイルオープン
- Running → `append()` でバッチ追記（`spawn_blocking` で呼び出し）
- Run Stop → `close()` で正常終了
- ファイル命名: `eb_run{XXXX}_{YYYY}_{ExpName}.root`
- ファイルローテーション: イベント数 or ファイルサイズ閾値で `close()` → `new()`

**★ oxyroot 検証事項**（Gemini 最終レビュー指摘）:
- oxyroot が TTree basket フラッシュ（close せずに中間書き出し）をサポートするか確認
- 未サポートの場合: append() が全イベントを RAM に溜める → OOM リスク
- **対策**: ファイルローテーション（1GB or 10万イベント）を初期実装から入れる

**Task 4: Command Task** — `run_command_task()` 使用（既存共通インフラ）
- `CommandHandlerExt` trait 実装
- `on_configure`: ch_settings + time_calibration を JSON ファイルからロード
  → `SliceBuilder::set_time_calibration(calib)` で適用
  → **time_calib_file は事前にオフラインで作成** (`event_builder time-calib` コマンド)
- `on_start`: SliceBuilder リセット + TimeSortBuffer クリア
- `on_stop`: FlushAndClose → 残バッファ書き出し
- `get_metrics`: received_hits, events_built, events_written, files_written

**時間キャリブレーション運用フロー:**
```
1. 初回ラン: DAQ で生データ取得 → Recorder が .delila 保存
2. オフライン: event_builder time-calib -i run*.root → timeSettings.json
3. オンラインEB設定: config.toml の time_calib_file = "config/timeSettings.json"
4. 以降のラン: オンラインEB が自動的にオフセット適用
```

**パラメータの区別（重要）:**
- `slice_duration_ns` (10ms): 処理バッチサイズ。大きいほどバッチ効率↑、レイテンシ↑
- `coincidence_window_ns` (500ns): **最終イベントの物理的受理窓**（±500ns = 合計1μs）
  → この値が最終 ROOT に書き出されるイベントの時間幅を決定
- slice_duration と coincidence_window は独立。混同しないこと

**データ損失防止戦略（Gemini 協議 #2）:**

基本方針: **大容量バッファ + 非ブロッキング受信 + Recorder による安全ネット**

1. **大容量 bounded channel**: Receiver → Process を ~1000 バッチスロット
   （ヒットレート数 MHz 想定: 1000 batch × ~1000 hits × ~100 bytes ≈ 100MB、200ms 分バッファ）
2. **ZMQ HWM 調整**: SUB ソケットの RCVHWM を高い値に設定（マイクロバースト吸収）
3. **非ブロッキング Receiver**: `try_send()` 使用。チャンネル満杯時は drop + ログ
   → Receiver ループが止まらない → ZMQ バッファ溢れ防止
4. **ROOT バッチ書き込み**: Writer Task で events を大量にバッファリングしてから一括書き込み
   （oxyroot は同期的なので `spawn_blocking` で tokio ランタイムを阻害しない）
5. **Recorder が全生データを保存** → online EB で drop があっても offline 復元可能
6. **dropped_batches** を AtomicU64 で記録 → UI/metrics で確認可能

**統計**: `AtomicU64` ベース（Mutex なし）

### 2-2. `src/event_builder/mod.rs` — `pub mod online;` 追加

### 2-3. `Hit::from_event_data` に `with_ac: false` デフォルト確認
既存実装（hit.rs:37）が EventData → Hit 変換で `with_ac` をどう扱うか確認。
AC フラグは SliceBuilder 内で設定されるため、初期値 false で問題なし。

## Phase 3: バイナリ + Operator 統合 (EB-3)

### 3-1. `src/bin/online_event_builder.rs` — 新規バイナリ

Recorder バイナリ (`src/bin/recorder.rs`) と同一パターン:
- CLI 引数パース
- TOML 設定読み込み → `OnlineEventBuilderConfig` 構築
- `OnlineEventBuilder::new(config).run(shutdown).await`

`Cargo.toml` に追加:
```toml
[[bin]]
name = "online_event_builder"
path = "src/bin/online_event_builder.rs"
required-features = ["root"]
```

### 3-2. `src/bin/operator.rs` — EB コンポーネント登録

`build_components_from_config()` に Event Builder セクション追加:
```rust
if let Some(ref eb) = config.network.event_builder {
    components.push(ComponentConfig {
        name: "EventBuilder".to_string(),
        address: eb.command.replace("tcp://*:", "tcp://localhost:"),
        pipeline_order: eb.pipeline_order,  // 3
        ..
    });
}
```

自動的に `/api/status`、Configure/Arm/Start/Stop/Reset に参加。

### 3-3. `scripts/start_daq.sh` — EB 起動追加

TOML に `[network.event_builder]` がある場合のみ起動。

## Phase 4: テスト (EB-2, EB-6)

### Unit Tests (`src/event_builder/online.rs`)
- `test_hit_from_event_data_batch` — EventDataBatch → Vec<Hit> 変換
- `test_process_accumulate_and_flush` — TimeSortBuffer + SliceBuilder 連携
- `test_file_naming_pattern` — eb_run0042_0000_exp.root
- `test_drain_and_start_resets_state` — Start 時にバッファクリア

### Integration Test — ROOT リプレイ with シャッフル

既存の実データ（`/Users/aogaki/WorkSpace/ELIFANT2025/p91Zr/data/run0113_*.root`）を使用。
ただしこのデータは既に時間ソート済みで現実より「良い」データのため、
**意図的にシャッフルして送信し、TimeSortBuffer の実環境テストとする。**

#### テストデータ送信コンポーネント (`src/bin/eb_test_sender.rs`)

```
ROOT file → read_hits_from_root()
  → チャンク単位で読み込み（slice_duration × N 分、N>1）
  → チャンク内のヒットをランダムシャッフル（rand::seq::SliceRandom）
  → Hit → EventData 逆変換（impl From<&Hit> for EventData を新規実装）
  → EventDataBatch に変換して ZMQ PUB で送信
  → チャンクが空になったら次のチャンクを読み込み
  → 全データ送信後 EOS
```

**パラメータ:**
- `--input`: ROOT ファイルパス（複数可）
- `--chunk-size-ns`: シャッフル単位の時間幅（default: slice_duration × 3）
- `--publish`: ZMQ PUB アドレス（default: tcp://*:5557）
- `--rate-limit`: 送信レート制限（optional、リアルタイムシミュレーション用）

**テストフロー:**
```
eb_test_sender (PUB:5557) → OnlineEventBuilder (SUB:5557, ROOT出力)
```
Operator 不要のスタンドアロンテスト。

#### 検証方法
1. 同じ run0113 データに対して **オフライン EB** と **オンライン EB** の出力を比較
2. イベント数の一致確認
3. トリガータイム・ヒット構成の一致確認（シャッフルの影響で微小な差異は許容）
4. `TimeSortBuffer::buffer_delay_ns` を変えてロバスト性をテスト

### E2E Test (EB-6)
- PSD1 実データ（172.18.4.147）で full pipeline テスト

## 再利用する既存コード

| コンポーネント | ファイル | 関数/構造体 |
|---|---|---|
| Hit 変換 | `src/event_builder/hit.rs:37` | `Hit::from_event_data()` |
| 時間ソート | `src/event_builder/time_sort.rs` | `TimeSortBuffer` (insert/drain_ready/flush) |
| イベントビルド | `src/event_builder/slice_builder.rs:103` | `SliceBuilder::build_events()` |
| ROOT 出力 | `src/event_builder/root_io.rs` | `write_events_to_root()` |
| コマンドハンドラ | `src/common/command_task.rs` | `run_command_task()` |
| 状態機械 | `src/common/state.rs` | `CommandHandlerExt` trait |

## 新規作成ファイル

| ファイル | 内容 |
|---|---|
| `src/event_builder/online.rs` | オンライン EB コンポーネント（4-task） |
| `src/bin/online_event_builder.rs` | CLI バイナリ |
| `src/bin/eb_test_sender.rs` | ROOT リプレイ + シャッフル送信ツール |

## 修正ファイル

| ファイル | 変更内容 |
|---|---|
| `src/config/mod.rs` | `EventBuilderNetworkConfig` + `NetworkConfig` にフィールド追加 |
| `src/event_builder/mod.rs` | `pub mod online;` 追加 |
| `src/bin/operator.rs` | EB コンポーネント登録 |
| `scripts/start_daq.sh` | EB プロセス起動追加 |
| `Cargo.toml` | `[[bin]]` エントリ追加 |
| config TOML ファイル | `[network.event_builder]` セクション追加 |

## 性能見積り（ヒットレート数 MHz 想定）

| リソース | 推定負荷 | 備考 |
|---|---|---|
| CPU (全体) | 30-80% of 1 core | rayon で複数コアに分散 |
| メモリ (バッファ) | ~1 GB 割当て可 | bounded channel + staging_buffer |
| TimeSortBuffer | ~25,000 エントリ | buffer_delay=5ms（100μsから拡大）, BTreeMap O(log 25k)≈15 |
| ROOT 書き出し | I/O bound | ビルド済みイベント 10-100 kHz |

## Verification

1. `cargo build --release --features root` — コンパイル成功
2. `cargo clippy --features root -- -D warnings` — 警告なし
3. `cargo test --features root` — 全テスト通過
4. **ROOT リプレイテスト**: `eb_test_sender --input run0113_*.root` → OnlineEB → ROOT 出力
   - オフライン EB の出力とイベント数・トリガー一致を確認
   - シャッフル送信で TimeSortBuffer のロバスト性検証
5. **パフォーマンスベンチマーク**: eb_test_sender を MHz レートで送信
   - CPU 使用率、メモリ使用量、dropped_batches 数を記録
   - ボトルネック特定 → 必要に応じ最適化
6. Emulator + Merger + OnlineEB でデータフロー確認
7. `/api/status` で EventBuilder が表示される
8. Run Start → Run Stop → `data/events/` に ROOT ファイル出力確認
9. ROOT ファイルを TBrowser で開いて TTree 構造検証
