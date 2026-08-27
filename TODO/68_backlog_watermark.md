# TODO 68 — バックログ水位計測 + OOM 前の秩序ある停止

**Status: ✅ 実装完了 (2026-08-25) — Phase 1-4 全部。E2E 3 モード (manual / autostop / timeout) 合格**

**関連:** CLAUDE.md 絶対ルール(データ保全) / TODO 58 C3(Stop テール例外) /
[67_amax_channel_pages_clamp.md](67_amax_channel_pages_clamp.md)(silent failure を出す先例 =
`channel_clamp_note` の 3 経路配線)

---

## 1. 発端 — 「Never Drop a Hit」ポスターの一番痛い質問 (2026-08-25)

学会ポスター準備中に想定問答を洗い出した:

> **「無制限バッファなら OOM で全部飛ぶ。落とすより悪いのでは？」**

回答の骨子は立てられる(§2)が、コードを確認したところ**正直に認めるべき穴が 2 つ**あった:

1. **メモリ水位を見て自発的に止まる仕組みが存在しない。**
   今日の「止まる」は設計された停止ではなく **OOM killer による停止**。
   コード中の `watermark` は全て EB の時刻ウォーターマーク(safe horizon)でメモリとは無関係。
2. **一番膨らむ場所が観測できていない。**
   ディスクが詰まったとき実際に溜まるのは Merger と Recorder のチャンネルだが、
   両者は `queue_size: 0` を**固定値で**報告している
   ([merger/mod.rs:275](../src/merger/mod.rs#L275), [recorder/mod.rs:570](../src/recorder/mod.rs#L570))。
   Reader だけが実キュー長を報告している([reader/mod.rs:1247](../src/reader/mod.rs#L1247))。

皮肉なことに [merger/mod.rs:318](../src/merger/mod.rs#L318) のコメントは
`// Use unbounded channel - if memory grows, it indicates downstream bottleneck`
と**この事態を予見していながら、その growth を誰も測っていない**。

## 2. 設計の立ち位置(ポスター問答の答え、実装の動機)

- 無制限バッファは無謀ではなく**弾力性の予算**。実測 90.5 MB/s (run0156) で:

  | 空きRAM | 吸収できる中断 |
  |---|---|
  | 16 GB | ≈ 3 分 |
  | 64 GB | ≈ 12 分 |
  | 128 GB | ≈ 24 分 |

  bounded queue が稼ぐのはミリ秒、unbounded は**分単位**(RAID 再構築・NFS ハング等を無傷で跨げる)。
- 故障モードの非対称性: ドロップは**静かに・継続的に・どこでも**起きる。
  OOM は**一度だけ・大きな音で・切断点が特定できる形**で起きる。
  ディスク到達分はブロック構造で無傷(15 本の kill されたランを全数回収済み)。
- **ただし** OOM killer 任せでは「切断点の特定」も「滞留分の救出」も運任せ。
  水位を測り、溢れる前に**滞留分をディスクに吐き切ってから**止まるべき。それが本 TODO。

## 3. 対象バッファの棚卸し

| # | 場所 | 実体 | 現在の観測 |
|---|---|---|---|
| B1 | Reader decode 経路 | crossbeam + ReorderBuffer | ✅ `queue_size` 報告あり |
| B2 | **Merger** receiver→sender | `mpsc::unbounded_channel::<tmq::Multipart>` ([merger/mod.rs:320](../src/merger/mod.rs#L320)) | ❌ 0 固定 |
| B3 | **Recorder** receiver→writer | `std::sync::mpsc::channel::<WriterCommand>` ([recorder/mod.rs:633](../src/recorder/mod.rs#L633)) | ❌ 0 固定 |
| B4 | ZMQ 内部バッファ (HWM=0) | libzmq 内部 | ❌ 観測 API 無し |

B4 は占有量を取る API が無いが、**Merger/Recorder の receiver は状態に関わらず常に
ZMQ を drain する設計**([recorder/mod.rs:801](../src/recorder/mod.rs#L801) に明文)なので、
バックログは観測可能な B2/B3 に集中する。この不変条件は本 TODO の前提 —
**「ZMQ は常に飲み干し、自前の測れるチャンネルに溜める」を維持すること**。

## 4. 実装 (2026-08-25 完了)

計画 (§4-old) から実装で確定した差分:

- **`queue_max` 流用はやめた** — フィールドを足す以上 wire-compat トリックは不要で、
  「容量」の意味論を汚す。`ComponentMetrics` に `queue_bytes` / `queue_bytes_peak` /
  `backlog_level` (0/1/2) を一括追加 (`#[serde(default)]`、構築サイト 7 箇所更新)。
  `backlog_warning/critical` フラグも作らない (level と冗長)。
- **会計プリミティブ** = `src/common/queue_accounting.rs`。Monitor の累積 2 カウンタ
  差分パターン (負にならない)。**Reader の `queue_length` は減算が無く壊れている**
  (増加のみ、reader/mod.rs:1097) — 模倣禁止の教訓として記録。
  `reset_peak()` は 0 でなく**現在深度**に再基準化 (ラン境界を跨ぐバックログを隠さない)。
  累積カウンタは lifetime — `AtomicStats::clear()/reset()` に入れない。
- **Merger**: send site (計 1 箇所) で move 前にバイト集計、sender_task の recv と
  **Running 遷移時 stale drain** (未計上の罠だった) で減算。`run()` の shutdown 待ちを
  select + 10 s tick 化して水位遷移をログ (上昇=warn、0 復帰=info、遷移時のみ)。
- **Recorder**: `AtomicStats` に gauge (3 タスク共有済みで配管不要)。`WriteRawBatch` のみ
  計上 (制御 variant は 0 バイト、enum にコメント)。**減算は `write_raw_batch` が返った後**
  — drain 待ちの「最終バッチ 1 個」競合窓を閉じる。既存 10 s ループの Running ゲート外で
  水位評価。
- **設定**: TOML `backlog_soft_limit_mb` (default 4096) / `backlog_hard_limit_mb`
  (default 0=無効) を merger/recorder 両セクションに。operator 側は
  `drain_stop_timeout_secs` (default 60) + opt-in `[operator.backlog_autostop]`
  (`poll_interval_secs`)。**autostop は二重 opt-in** (component hard>0 + operator セクション)。
- **InfluxDB**: influxdb.rs の source-only フィルタを修正 — 非 source は
  `backlog,component={name} queue_bytes=..` 行を出力。**従来 Merger/Recorder の metrics は
  一切 Grafana に届いていなかった**。
- **drain-first stop** (`/api/stop_drain` + `src/operator/backlog_watch.rs`):
  1. Tune Up ガード → source/非 source 分割
  2. source を stop → Configured 待ち (10 s) — Reader は **Stop ack 後に EOS を flush**
     するのでこの待ちが drain 待ちに先行必須
  3. `wait_for_backlog_drained` (client.rs、200 ms poll): 成功 = 全対象
     **`queue_bytes==0` かつ `events_processed` 不変を 3 poll 連続** (~600 ms 静穏)。
     静穏条件は飾りではない — ①Reader の EOS 遅延 ②merger→recorder ZMQ 機内分、の
     2 つの偽 0 窓を閉じる
  4. 非 source を stop (この間ずっと Running だったので Stop テール破棄は発火しない)
  5. タイムアウト時: 残余 bytes/items をコンポーネント別に計数し
     応答 + ログ + ELOG + Mongo (`stop_reason` フィールド新設) に出して**それでも stop を強制**
  6. `/api/stop` は無変更 (実績あるビームタイム経路を守る)。記帳は
     `finish_run_bookkeeping` に抽出して両者で共有
- **テストフック**: `DELILA_TEST_WRITE_DELAY_MS` (writer thread、起動時に
  `TEST HOOK ACTIVE` warn)。SIGSTOP 不採用 — receiver ごと止まり「ZMQ を飲み干す」
  不変条件ごと殺すため試験として無効。

### 実装中に発見・修正したバグ

**`ApiResponse::with_results()` が `success` を再計算する** — drain タイムアウトで
`ApiResponse::error(...)` を作っても、直後の `.with_results()` がコンポーネント Stop の
成否だけから `success=true` に**上書き**していた。E2E の timeout モードが検出。
`with_results` の後で drain 失敗を再適用する形で修正。

### 検証結果

- ユニット: `queue_accounting` 7 本 + 既存全部 = **lib 696 / codegen 27 全パス**
- E2E `scripts/backlog_drain_test.sh` (エミュレータ 100k ev/s vs 200 ms/batch writer):
  - **manual**: backlog 6.3 MB / level 2 → `/api/stop_drain` → **254 000 イベント全数書き込み・
    queue_bytes 0・dropped 増加ゼロ**
  - **autostop**: ウォッチャー (1 s poll) が発火 → **286 000 イベント全数・損失ゼロ**
  - **timeout** (1 s): `success:false` + `DRAIN INCOMPLETE: Recorder: 7033988 bytes /
    141 batches` — **残余は数えられ、無言にならない**
- clippy: 新規指摘ゼロ (HEAD ベースライン 4 件のみ、行ズレ)

### 残 (別件)

- Grafana ダッシュボードに `backlog` measurement のパネル追加 (Influx 行は出ている)
- Operator UI での backlog_level 表示 (型は追加済み、バッジ表示は未実装)
- per-host 合算判定 (§6.5 — Operator 側で後付け可能な設計にしてある)

---

## 4-old. 当初の実装計画 (参照用)

### Phase 1 — 観測(まずここまで。単独で価値がある)

- B2/B3 に `Arc<(AtomicUsize /*items*/, AtomicU64 /*bytes*/)>` の会計を付ける
  (tokio/std の mpsc は `len()` を持たないので send +1 / recv −1 を自前で)。
  **bytes が主役**(Multipart サイズは可変)。items は補助。
- `ComponentMetrics.queue_size` に items、**遊んでいる `queue_max` を高水位マーク**として使う
  (フィールド追加不要、wire 互換)。bytes は新フィールド `queue_bytes` を検討
  (→ feedback メモ「Pre-allocate over incremental schema growth」: 足すなら一度で)。
- Operator status / InfluxDB(既存経路)に乗れば Grafana で見える。

### Phase 2 — 水位ポリシー

- TOML per-component: `backlog_soft_limit_mb`(warn) / `backlog_hard_limit_mb`(行動)。
- **soft**: `warn!` + status に `backlog_warning` フラグ。CLAUDE.md「silent failure を作らない」。
- **hard**: コンポーネントは status に `backlog_critical` を立てるだけ。
  **行動の主体は Operator**(全体視野と制御権を持つのは Operator だけ)。
- **デフォルトは soft=有効(例 4 GiB)、hard=無効(opt-in)** を提案。
  自動停止という新経路のバグでビームタイムを殺すのが最悪なので、実機検証まで opt-in。

### Phase 3 — 緊急停止シーケンス(本 TODO の核心、要注意)

⚠️ **既存の Stop をそのまま流用してはならない。**
Recorder は非 Running のバッチを Stop テール例外として破棄する
([recorder/mod.rs:836](../src/recorder/mod.rs#L836))。バックログ起因で普通に stop_all すると
**救いたい滞留分そのものが全部「テール」として捨てられる** — 目的と真逆。

正しい順序(drain-first stop):

```
1. Operator が backlog_critical を検知
2. Reader のみ Stop(HW disarm — 蛇口を閉める)
3. Merger/Recorder は Running のまま維持し、queue_bytes ≈ 0 まで drain を待つ
4. drain 完了後に Merger → Recorder を Stop(通常の EOS 経路)
5. 結果: ディスク上のファイルは「短いが完全」。切断点 = Reader を止めた時刻
```

run_stop の既存フェーズ構成(Reader 先行 Stop + EOS 伝播)に近いが、
「**下流の queue_bytes が捌けるまで待つ**」ステップが新規。タイムアウト
(ディスク完全死で drain が進まない場合)の扱いも決めること — その場合のみ
残余は失われるが、**何 bytes / 何 events 失ったかを数えて出す**(無言にしない)。

### Phase 4 — 検証

- 会計のユニットテスト(send/recv/EOS/Stop 経路での増減一致)。
- エミュレータ E2E: Recorder の writer を人工的に遅延(テストフック or SIGSTOP)→
  backlog 成長 → soft warn → hard → drain-first stop → **イベント数がディスク上で完全一致**、
  Running 中の dropped_batches 増加ゼロ、を確認。
- 高水位マークが Grafana で見えること。

## 5. スコープ外

- ZMQ 内部占有量の直接観測(API が無い。§3 の drain 不変条件で代替)
- Online EB(unbounded mpsc は同型だが、まず本流 Merger/Recorder で確立してから)
- RSS ベースの水位(プラットフォーム依存。バイト会計で十分直接的)

## 6. 決めどころ(実装前に要判断)

1. `queue_bytes` を ComponentMetrics に足すか、`queue_max` 流用で済ますか
2. soft/hard のデフォルト値(4 GiB / 無効、の提案でよいか)
3. drain 待ちタイムアウト(提案: 60 s、超過時は残余を計数して強制 Stop)
4. hard 発火の判定を Operator ポーリング(現行 status 周期)に乗せるか、専用通知にするか
5. **水位の単位: per-component か per-host か (2026-08-25 議論)** —
   測れるのは per-component(自分のチャンネル会計)だが、**OOM はホスト単位の現象**。
   gant 型の全部入り構成では各自 4 GiB × 4 プロセス = 16 GiB が「全員正常」のまま
   ホストを沈め得る。当面の落とし所: ①判定は per-component のまま(決定的・KISS)、
   ②デフォルト値の目安「ホスト RAM ÷ 同居コンポーネント数」を TOML コメントに明記、
   ③将来はOperator が queue_bytes とアドレス(=ホスト対応)を両方持つので
   **ホスト単位の合算判定を Operator 側で後付け可能**(Phase 2 の行動主体=Operator と整合)
