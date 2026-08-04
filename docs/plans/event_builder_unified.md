# Event Builder 統一設計書

**Date:** 2026-02-26
**Status:** 🧊 FROZEN (2026-08-04) — Phase 0-4 実装完了(APPROVED、Gemini レビュー 2回)。Phase 5(レガシー削除)/Phase 6 は中止: EB は C++(root_sink 系譜)へ移行決定 → [TODO/event-builder/66_cpp_event_builder.md](../../TODO/event-builder/66_cpp_event_builder.md)。「Online/Offline 同一パイプライン」という本設計の中心思想は C++ 側に引き継がれる(TDelila が ZMQ と .delila の両方を読む)
**Supersedes:** `online_event_builder_v2.md`, `TODO/event-builder/38_eb_unification_mimalloc.md`

---

## 1. 目標

**Online と Offline で完全に同一のイベントビルドパイプラインを使用する。**

- 唯一のイベントビルドエンジン: `build_events_from_chunk()`
- 唯一のパイプライン: `Sorter → Workers → Writers`
- 入力ソースだけが異なる: `HitSource` trait で抽象化
- Time Calibration はオフラインで事前測定、人間が確認してからオンラインに適用

### 設計原則

1. **同一パイプライン**: Online と Offline で Sorter → Workers → Writers の全段を共有
2. **入力抽象化**: `HitSource` trait で ZMQ / ROOT ファイルを差し替え
3. **スケーラビリティ**: メモリ使用量がデータサイズに依存しないストリーム設計
4. **決定性**: 同一入力・同一設定 → 同一イベント集合（順序はスレッド依存）

## 2. 運用フロー

```
[準備段階 — 実験開始前]
  1. テストラン（Tune Up or 短い Run）を取得
  2. delila2root で .delila → ROOT 変換（既存ツール）
  3. event_builder time-calib で時間オフセット測定 → timeSettings.json
  4. 物理屋がオフセット値を確認・承認
  5. config.toml の [event_builder] に timeSettings.json のパスを設定

[本番]
  6. Operator が Online EB を起動 → timeSettings.json を読み込み
  7. 全ランで同じ補正を自動適用（JSON を変えない限り不変）
  8. Raw .delila ファイルは常に Recorder が保存（クラッシュ耐性）

[事後解析]
  9. event_builder build で ROOT files → 同一パイプラインでイベント構築
  10. オンラインと同一のイベント集合（同一入力・同一設定なら）
```

## 3. アーキテクチャ

### 3.1 統一パイプライン

```
[HitSource]     [Sorter]        [Workers×N]     [Writers×M]
               std::thread    std::thread×N    std::thread×M
    │              │              │              │
 ZMQ SUB or    accumulate     build_events    local buffer
 ROOT files    + time_calib      │           extend(batch)
    │          + sort            │           threshold→write
    │          + safe_cut        │
    ▼              ▼              ▼              ▼
 Vec<Hit> ──→ SortedChunk ──→ Vec<Built> ──→ ROOT files
(crossbeam)  (crossbeam)    (crossbeam MPMC) (file-per-batch)
```

Online と Offline の**唯一の違いは HitSource**。
Sorter 以降は完全に同一のコードパスを通る。

### 3.2 HitSource trait（新規）

```rust
/// ヒット供給元の抽象化
///
/// Online (ZMQ) と Offline (ROOT files) で差し替え可能。
/// Sorter スレッドがこの trait を通じてヒットを受信する。
enum HitBatch {
    Hits(Vec<Hit>),
    Eos,
}

/// タイムアウト付きヒット供給
///
/// Sorter は短い間隔（~100ms）で next_batch を呼び、
/// Timeout 時に flush 条件を評価する。
/// これにより、低レート時やビーム停止時にもデータが滞留しない。
trait HitSource: Send {
    /// 次のバッチを取得。timeout 経過で Err(Timeout) を返す。
    fn next_batch(&mut self, timeout: Duration)
        -> Result<HitBatch, crossbeam::RecvTimeoutError>;
}
```

#### ZmqHitSource（Online）

```rust
struct ZmqHitSource {
    hit_rx: crossbeam::Receiver<HitBatch>,
    // Receiver tokio task が ZMQ SUB → hit_rx に送信
}

impl HitSource for ZmqHitSource {
    fn next_batch(&mut self, timeout: Duration)
        -> Result<HitBatch, crossbeam::RecvTimeoutError>
    {
        self.hit_rx.recv_timeout(timeout)
    }
}
```

- Receiver は別途 tokio task として起動（既存パターン）
- ZMQ SUB → Message deserialization → `Hit::from_event_data()` → crossbeam 送信
- HWM=0（データドロップ禁止）
- EOS 検出 → `HitBatch::Eos` 送信
- 5s タイムアウト → EOS ロスト想定（既存ロジック維持）

#### RootFileHitSource（Offline）

```rust
struct RootFileHitSource {
    files: Vec<PathBuf>,
    current_file_index: usize,
    batch_size: usize,  // default: 500_000 hits
}

impl HitSource for RootFileHitSource {
    fn next_batch(&mut self, _timeout: Duration)
        -> Result<HitBatch, crossbeam::RecvTimeoutError>
    {
        // ファイルを順に読み、batch_size ごとに Vec<Hit> を返す
        // 全ファイル読了後に Ok(HitBatch::Eos) を返す
        // timeout は無視（ファイル読み込みは即座に完了）
        // ファイル間で EOS を送らない（単一連続ストリーム）
    }
}
```

- ROOT ファイルを逐次読み込み → batch_size ごとに Sorter に供給
- **メモリ使用量はデータサイズに依存しない**（常に batch_size 分のみ）
- batch_size のデフォルト = sorter_threshold と同じ 500K hits
- ファイル間の境界はヒットの連続ストリームとして扱う（Run 全体で1つのストリーム）

### 3.3 共通コア（変更最小限）

```
chunk_builder.rs:
  - SortedChunk          // ソート済みヒット + core_end
  - TriggerConfig        // トリガー/AC/優先度/coincidence window
  - build_events_from_chunk(&SortedChunk, &TriggerConfig) → Vec<BuiltEvent>
  - sort_and_split(Vec<Hit>, safe_horizon_ns) → (SortedChunk, Vec<Hit>)
```

既存の `sort_and_flush()` は `sort_and_split(buffer, 0.0)` と等価。
ただしコードの明確さのため残す（EOS 時の意図が明示的）。

### 3.4 TriggerConfig ヘルパー（新規）

`TriggerConfig` を `ChSettings` JSON から構築するヘルパーを `chunk_builder.rs` に追加:

```rust
impl TriggerConfig {
    /// ChSettings JSON (ChannelConfig) から TriggerConfig を構築
    /// ファイル I/O を含まない pure 関数
    pub fn from_channel_config(
        config: &ChannelConfig,
        coincidence_window_ns: f64,
    ) -> Self {
        let mut triggers = HashSet::new();
        let mut priorities = HashMap::new();
        let mut ac_pairs = HashMap::new();

        for module_channels in config {
            for ch in module_channels {
                let key = (ch.module, ch.channel);
                if ch.is_event_trigger {
                    triggers.insert(key);
                    priorities.insert(key, ch.id as u32);
                }
                if ch.has_ac && ch.ac_module != 128 {
                    ac_pairs.insert(key, (ch.ac_module, ch.ac_channel));
                }
            }
        }

        TriggerConfig { triggers, priorities, ac_pairs, coincidence_window_ns }
    }
}
```

### 3.5 EventBuilderPipeline（新規：パイプライン構造体）

```rust
/// 統一イベントビルドパイプライン
///
/// HitSource から受け取ったヒットを Sorter → Workers → Writers で処理。
/// Online / Offline 共通。
pub struct EventBuilderPipeline {
    config: PipelineConfig,
    trigger_config: Arc<TriggerConfig>,
    time_calibration: TimeCalibration,
}

pub struct PipelineConfig {
    // パイプラインパラメータ
    pub safe_horizon_ns: f64,
    pub n_workers: usize,
    pub n_writers: usize,
    pub events_per_file: usize,
    pub sorter_threshold: usize,
    pub sorter_timeout: Duration,

    // 出力設定
    pub output_dir: PathBuf,
    pub run_id: u32,
}

impl EventBuilderPipeline {
    /// パイプラインを実行
    ///
    /// HitSource を消費し、Sorter → Workers → Writers を起動。
    /// 全てのイベントが書き出されるまでブロック。
    pub fn run(self, source: impl HitSource) -> Result<PipelineStats> {
        // 1. チャンネル作成
        // 2. Writer スレッド起動
        // 3. Worker スレッド起動
        // 4. Sorter スレッド起動（source を消費）
        // 5. 全スレッド join
        // 6. 統計返却
    }
}
```

**Online**: `ZmqHitSource` を渡して tokio task 内から呼ぶ
**Offline**: `RootFileHitSource` を渡して main thread から呼ぶ

### 3.6 Sorter（リファクタリング）

```rust
/// Sorter: HitSource からヒットを受信 → time_calib → sort → split → Workers へ
fn sorter_thread(
    mut source: impl HitSource,      // ← 変更点: trait object で受信
    chunk_tx: crossbeam::Sender<SortedChunk>,
    safe_horizon_ns: f64,
    threshold: usize,
    flush_timeout: Duration,
    time_calibration: &TimeCalibration,
    stats: Arc<PipelineStats>,
) {
    let mut buffer: Vec<Hit> = Vec::with_capacity(threshold * 2);
    let mut last_flush = Instant::now();

    // Sorter は短い間隔でウェイクアップし、flush 条件を評価する
    // (Gemini レビュー #1: blocking recv で timeout flush が効かない問題の対策)
    let poll_interval = Duration::from_millis(100);

    loop {
        // HitSource からタイムアウト付きで受信
        let wait_time = if buffer.len() >= threshold {
            Duration::ZERO  // バッファ満杯 → 即座に flush チェック
        } else {
            let remaining = flush_timeout.saturating_sub(last_flush.elapsed());
            std::cmp::min(poll_interval, remaining)
        };

        match source.next_batch(wait_time) {
            Ok(HitBatch::Hits(mut hits)) => {
                // Time calibration 適用（Online/Offline 共通の場所）
                for hit in &mut hits {
                    hit.apply_offset(time_calibration.get_offset(hit.module, hit.channel));
                }
                buffer.extend(hits);
            }
            Ok(HitBatch::Eos) => {
                // EOS: flush and exit
                if let Some(chunk) = sort_and_flush(buffer) {
                    let _ = chunk_tx.send(chunk);
                }
                break;
            }
            Err(crossbeam::RecvTimeoutError::Timeout) => {
                // タイムアウト → 下の flush チェックに進む
            }
            Err(crossbeam::RecvTimeoutError::Disconnected) => {
                // チャンネル切断 = ソース終了（ZMQ abort 等）
                if let Some(chunk) = sort_and_flush(buffer) {
                    let _ = chunk_tx.send(chunk);
                }
                break;
            }
        }

        // threshold or timeout → sort_and_split
        if buffer.len() >= threshold || last_flush.elapsed() >= flush_timeout {
            match sort_and_split(buffer, safe_horizon_ns) {
                Ok((chunk, retained)) => {
                    let _ = chunk_tx.send(chunk);
                    buffer = retained;
                    last_flush = Instant::now();
                }
                Err(returned) => {
                    buffer = returned;
                    last_flush = Instant::now();
                }
            }
        }
    }
}
```

**重要**: Time calibration は Sorter 内で適用。Online/Offline 共に同一の場所。

### 3.7 Workers / Writers（既存ロジック維持）

Worker thread と Writer thread は現在の `online.rs` の実装をそのまま
`EventBuilderPipeline` に移動。ロジックの変更なし。

## 4. オフラインでの Safe Horizon

**Gemini の核心的指摘への対応:**

Offline でも Online と同一の `sort_and_split(safe_horizon=50ms)` を使う。

- ROOT ファイルのヒットはほぼソート済みだが、複数ファイル結合時にファイル境界で
  タイムスタンプの前後が発生しうる
- Safe Horizon でこれを吸収し、Online と全く同じチャンク境界処理を行う
- **結果**: Online で構築されたイベントと Offline で構築されたイベントが
  完全に同一の境界処理ロジックを通る

Offline 専用の `safe_horizon_ns` 設定は不要。同一の 50ms デフォルトを使用。
（Offline ではネットワーク到着順の問題はないが、統一性のために同じ値を使う。
 オーバーヘッドは retained ヒット分のメモリ ~600KB のみ。）

## 5. 決定性について

複数 Worker が並列処理するため、出力 ROOT ファイル内のイベント順序は
スレッドスケジューリングに依存する（非決定的）。

**許容する理由:**
- イベント集合（set）は決定的 — 同じイベントが同じ内容で構築される
- 物理解析では EventID やファイル内順序は無関係（TriggerTime でソートして使う）
- Writers がファイルに書く前に `sort_unstable_by(trigger_time)` している（既存）

**Offline での検証用:**
- `--workers 1 --writers 1` で完全決定的な出力を生成可能（遅いが検証用）
- Online/Offline 比較時はイベントを TriggerTime でソートして比較

## 6. CLI インターフェース

```bash
# Time calibration（変更なし）
event_builder time-calib -i run0042_*.root -o timeSettings.json \
    --ref-module 0 --ref-channel 0 --window 1000

# Event building（統一パイプライン）
event_builder build -i run0042_*.root -o ./data/events/ \
    --config chSettings.json \
    --time-calib timeSettings.json \
    --window 500 \
    --trigger 0:0 --trigger 1:0 \
    --run-id 42 \
    --workers 4 --writers 2
```

**変更点:**
- `-o` は出力ディレクトリ（ファイルローテーションのため単一ファイルではない）
- `--run-id` 追加（ファイル名に使用）
- `--workers`, `--writers` 追加（デフォルト: 4, 2）
- `--slice-duration` 削除（SliceBuilder 固有の概念）

**維持する CLI オプション:**
- `-i/--input`, `--config`, `--time-calib/-T`
- `--window`, `--trigger`, `--tree-name`, `--max-hits`

## 7. 削除対象

| ファイル | 理由 |
|---------|------|
| `src/event_builder/slice_builder.rs` | chunk_builder に統一 |
| `src/event_builder/time_slice.rs` | SliceBuilder 専用 |
| `src/event_builder/time_sort.rs` | 旧 online v1 の遺物（既に未使用） |
| `src/event_builder/l1_builder.rs` | 旧方式、未使用 |

**維持:**
- `chunk_builder.rs` — コアエンジン（TriggerConfig ヘルパー追加のみ）
- `online.rs` → `pipeline.rs` にリネーム（統一パイプラインとして）
- `config.rs` — 設定型（変更なし）
- `hit.rs` — Hit 型（変更なし）
- `built_event.rs` — BuiltEvent 型（変更なし）
- `root_io.rs` — ROOT I/O（変更なし）
- `time_calibrator.rs` — 時間キャリブレーション（変更なし）

**新規:**
- `source.rs` — `HitSource` trait + `ZmqHitSource` + `RootFileHitSource`
- `pipeline.rs` — `EventBuilderPipeline` + Sorter/Worker/Writer threads

## 8. TOML 設定（Online EB）

```toml
[event_builder]
enabled = true
subscribe_address = "tcp://localhost:5557"  # Merger の PUB アドレス
command_address = "tcp://*:5595"
output_dir = "./data/events"

# イベントビルドパラメータ
coincidence_window_ns = 100.0
safe_horizon_ns = 50_000_000.0  # 50ms

# 設定ファイル（Configure 時に毎回ディスクから再読み込み）
ch_settings_file = "config/chSettings.json"
time_calib_file = "config/timeSettings.json"

# パイプラインチューニング
n_workers = 4
n_writers = 2
events_per_file = 100_000
sorter_threshold = 500_000
sorter_timeout_ms = 500
```

**重要**: `ch_settings_file` と `time_calib_file` は **Configure 遷移のたびにディスクから再読み込み**。
メモリにキャッシュした古い設定を使い続けるリスクを排除する。

## 9. Operator 統合

Online EB は Operator のコンポーネントとして管理される。

### ステートマシン

```
Idle → Configure → Configured → Arm → Armed → Start → Running → Stop → Configured
```

- **Configure**: ch_settings_file と time_calib_file を**ディスクから再読み込み**し、
  TriggerConfig と TimeCalibration を構築。設定が無い/壊れている場合は Configure 失敗。
- **Start**: `EventBuilderPipeline::run(ZmqHitSource)` を tokio::spawn_blocking で起動。
  Run ID は Operator の現在の run_number を渡す。file_index は 0 にリセット。
- **Stop**: ZmqHitSource の Receiver task を abort → crossbeam channel 切断 →
  Sorter が残データ flush → Workers drain → Writers 最終 flush
- **Reset**: パイプライン破棄

### Operator REST API（新規エンドポイント）

| Method | Path | Description |
|--------|------|-------------|
| GET | /api/event_builder/status | EB の状態 + PipelineStats |
| GET | /api/event_builder/config | 現在の設定 |

EB 専用の /cmd エンドポイントは不要 — 既存の Operator ステートマシンで制御。

## 10. 出力フォーマット

### ROOT TTree (EventTree)

| Branch | Type | Description |
|--------|------|-------------|
| EventID | u64 | 連番 |
| TriggerTime | f64 | 絶対タイムスタンプ [ns] |
| TriggerMod | u8 | トリガーモジュール |
| TriggerCh | u8 | トリガーチャンネル |
| Multiplicity | u32 | ヒット数（トリガー含む） |
| Mod | Vec\<u8\> | 各ヒットのモジュール |
| Ch | Vec\<u8\> | 各ヒットのチャンネル |
| Energy | Vec\<u16\> | 各ヒットのエネルギー (long gate) |
| EnergyShort | Vec\<u16\> | 各ヒットのエネルギー (short gate) |
| RelTime | Vec\<f64\> | 各ヒットのトリガーからの相対時間 [ns] |
| WithAC | Vec\<u8\> | AC coincidence フラグ (0/1) |

### ファイルローテーション

- `eb_run{RUN_ID:04}_{INDEX:04}_events.root`
- 100,000 events ごとに新ファイル
- Writers が MPMC で分担 → file_index は AtomicU32 で一意
- file_index は Run 開始時に 0 リセット

## 11. 実装ステップ

### Phase 1: 抽象化 + コア整備

1. **`HitSource` trait + `HitBatch` enum 定義** — `source.rs`
2. **`RootFileHitSource` 実装** — ROOT ファイル逐次読み + バッチ分割
3. **`TriggerConfig::from_channel_config()` 追加** — `chunk_builder.rs`
4. **`EventBuilderPipeline` 構造体** — 既存 `online.rs` のパイプラインロジックを抽出
   - Sorter: `HitSource` trait から受信するように変更
   - Workers / Writers: ロジック変更なし、場所移動のみ
5. **ユニットテスト**:
   - `TriggerConfig::from_channel_config()` テスト
   - `RootFileHitSource` のバッチ分割テスト
   - `EventBuilderPipeline` の integration テスト（小さなヒット集合 → 結果検証）

### Phase 2: オフライン CLI 書き直し

1. **`bin/event_builder.rs` の `run_event_building()` 書き直し**
   - `RootFileHitSource` + `EventBuilderPipeline` を使用
   - 出力先はディレクトリ（ファイルローテーション）
   - `--workers`, `--writers`, `--run-id` オプション追加
   - `--slice-duration` 削除
2. **回帰テスト**: 既存テストデータで旧 SliceBuilder vs 新パイプラインの結果比較
3. **SliceBuilder + TimeSlice + L1Builder + TimeSortBuffer 削除**
4. **`mod.rs` の re-export 整理**

### Phase 3: Online EB 統合

1. **`ZmqHitSource` 実装** — 既存 `receiver_task()` ロジックを HitSource に適合
2. **`online.rs` → `EventBuilderPipeline` + `ZmqHitSource` に書き直し**
   - `OnlineEventBuilder::run()` が `ZmqHitSource` を構築して `EventBuilderPipeline` に渡す
3. **Writers デフォルトを 4 → 2 に変更**（oxyroot bench 結果）
4. **Run ID をパイプラインに伝播**（現在ハードコード `9999`）

### Phase 4: Operator 統合

1. **TOML `[event_builder]` セクション追加**
2. **Operator に EB コンポーネント登録**（Configure/Start/Stop 制御）
3. **Configure 時に ch_settings + time_calib をディスクから再読み込み**
4. **REST API: /api/event_builder/status**
5. **フロントエンド: EB ステータス表示**（status-panel に追加）

### Phase 5: E2E テスト + 検証

1. **Online**: eb_test_sender → Merger → Online EB → ROOT files → 検証
2. **Offline**: 同一データを offline CLI で処理 → 結果比較（TriggerTime ソート後に比較）
3. **本番データ**: 172.18.4.76 で実データ検証
4. **大規模テスト**: 10億+ hits のデータでメモリ使用量・処理速度確認

## 12. テスト戦略

### 既存テスト（維持）
- `chunk_builder::tests` — 48 テスト（unit + integration）
- `built_event::tests`, `hit::tests`, `config::tests`, `root_io::tests`

### 新規テスト
- `TriggerConfig::from_channel_config()` ユニットテスト
- `RootFileHitSource` バッチ分割テスト
- `EventBuilderPipeline` integration テスト（小規模ヒット → 結果検証）
- Online/Offline 同一結果テスト（同一入力 → TriggerTime ソートして比較）

### 回帰テスト
- `test_offline_root_data` を新パイプラインで実行（ELIFANT テストデータ）

## 13. データ保全

- **ZMQ HWM=0**: ZmqHitSource の SUB ソケット
- **crossbeam bounded**: HitSource → Sorter, Sorter → Workers, Workers → Writers
  - バックプレッシャーで制御、データドロップなし
- **Raw .delila 常時保存**: Recorder が並行保存。EB がクラッシュしてもデータ復元可能
- **オフライン再処理可能**: raw → delila2root → event_builder build で同一結果を再現

## 14. 性能見積もり

| 指標 | 値 | 根拠 |
|------|-----|------|
| Online EB スループット | > 1M hits/s | chunk_builder 単体で ~3M hits/s (bench), 4 workers |
| ROOT 出力 | 0.79 M events/s (1 writer) | oxyroot bench (2026-02-25) |
| 実運用ヒットレート | ~300k hits/s (6 デジタイザ) | Run 156 実測 3.81M events/10s |
| マージン | 3x 以上 | 実運用 vs 処理能力 |
| Stop 遅延 | < 200 ms | Sorter flush + Writer flush |
| Offline メモリ使用量 | ~50 MB (steady state) | sorter_threshold × Hit size + retained |

## 15. 将来拡張（MVP scope 外）

- **L2 Filter**: `build_events_from_chunk()` の出力に対して Counter/Flag/Accept フィルタ適用
- **Energy Calibration**: `ChSettings.p0-p3` で ADC → keV 変換
- **直接 .delila 読み込み**: `DelilaFileHitSource` を実装すれば delila2root 不要に
- **分散 EB**: 複数マシンで Workers を分散（10+ デジタイザ対応時）
- **mimalloc 導入**: グローバルアロケータ切り替えでパイプライン全体を高速化

---

## Appendix: Gemini レビュー指摘事項と対応

| # | 指摘 | 対応 |
|---|------|------|
| 1 | Offline の「全ヒットメモリロード」は NG | → `RootFileHitSource` でストリーム化、同一パイプライン |
| 2 | Time Calibration 適用場所が Online/Offline で異なる | → Sorter 内で統一適用 |
| 3 | 並列 Workers による非決定的順序 | → 許容。検証時は `--workers 1` or TriggerTime ソート比較 |
| 4 | Configure 時に JSON を毎回リロードすべき | → 明記。メモリキャッシュしない |
| 5 | Run ID 伝播 + file_index リセット | → PipelineConfig に run_id 追加、Start 時に 0 リセット |
| 6 | `sort_and_flush` は `sort_and_split(0)` で統合可能 | → コードの明確さのため残すが、内部で同一ロジック |

### Gemini レビュー 2回目 (v2)

| # | 指摘 | 重要度 | 対応 |
|---|------|--------|------|
| 1 | Sorter の blocking recv で timeout flush が効かない | **必須** | → `next_batch(timeout)` + poll_interval (100ms) で解決 |
| 2 | ファイル間で EOS を送らないこと | 確認 | → 設計通り（単一ストリーム） |
| 3 | `get_offset()` の HashMap が Sorter ボトルネックの可能性 | 確認 | → 実測後に必要なら Vec に最適化 |
| 4 | ZMQ shutdown 時に channel drop → flush 確実に | 確認 | → Disconnected ハンドリング追加済み |
