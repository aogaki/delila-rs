# Event Builder 仕様書

**Version:** 0.8.0
**Date:** 2026-08-04
**Status:** Design — **実装言語を C++ (root_sink 系譜) へ再転換** ([TODO 66](66_cpp_event_builder.md))。本仕様の概念部(§1.4 責務境界 / L1/L2 named-ops / 2 層 threshold / timeSettings tree)は C++ 実装の要求仕様として引き続き有効。Rust 統一パイプライン(§11)は凍結・参照実装化

> v0.7 → v0.8 主要変更 (2026-08-04):
> - **実装言語を C++ へ再転換**(v0.5 の「Rust 側で完結」を撤回)。ただし v0.3 の
>   別リポジトリ + Event Bridge 案ではなく、**delila-rs 内 root_sink 系譜**
>   (merger PUB 直接購読、TDelila ベース、online=offline 同一コード)
> - 根拠: ①oxyroot の ROOT 書き込み圧縮が未実装と確定(常に非圧縮、実測
>   28.1 MB vs C++ ZSTD 18.1 MB)②root_sink/PositionMatcher の実証実績
>   ③EB 性能は上流(Rust)で律速済みでスカラー処理は C++ 単線で十分
> - `.delila` 正典・Rust 取得系(HWM=0/no-drop)は不変。経緯と条件は TODO 66 参照

> v0.4 → v0.5 主要変更:
> - C++ Event Builder (別リポジトリ) 経路を **撤回** — Rust 側で完結
> - Online/Offline で **Hit 構造を分離** (`OnlineHit` / `OfflineHit` + `HitLike` trait)
> - L1/L2 を **named-ops AST** で表現（ELIFANT-Event の L2 設計を L1 にも拡張）
> - **Threshold を 3 層に分離**（noise floor / trigger gate / event filter）
> - **Event Bridge プロセスを retire**、Online EB は Merger PUB に直接 subscribe
> - EB output は **MessagePack** `BuiltEventBatch`、EB Monitor が subscribe
>
> v0.5 → v0.5.1:
> - `timeSettings.json` を **tree モデル** に刷新（§ 4.3）— 多 root 許容 + 1 回適用 + ingress-time alignment

---

## 1. 概要

### 1.1 目的

核物理実験 (DELILA) の DAQ システム向けイベントビルダー。
複数デジタイザからの非同期ヒットデータを時間順に整列し、
コインシデンスウィンドウ内のヒットをイベントとして構築する。

### 1.2 動作モード

| モード | 入力 | Hit 型 | 出力 |
|--------|------|--------|------|
| **オンライン** | Merger PUB (MessagePack `EventDataBatch`) | `OnlineHit` (lean, 16 B) | ROOT (Rust) + ZMQ PUB → EB Monitor |
| **オフライン** | `.delila` ファイル / ROOT ファイル | `OfflineHit` (rich, 56 B) | ROOT (Rust) |

両モードとも **同じ chunk_builder コア**を使用し、`HitSource` trait で入力を切り替える（§ 11）。

### 1.3 要求仕様サマリ

| 項目 | 値 |
|------|-----|
| 処理レート（オンライン） | 2 MHz hits |
| 許容レイテンシ | 10 秒 〜 1 分 |
| メモリ使用量上限 | 10 GB |
| コインシデンスウィンドウ | ±500 ns（設定可能） |
| 最大到着遅延 | 1 秒未満 |

### 1.4 設計原則 — EB の責務境界

> **EB は「物理屋が解析しやすい時間整列済みデータ」を最大限の情報量で提供する。
> 最終的な物理判断は物理屋が下流で一つ一つ確認しながら行う。**

EB は **汎用のイベント構築エンジン**であり、特定実験の物理仮定を組み込まない。
判断が分かれる cut は **EB に入れず、必ず下流の解析側に置く**こと。

#### 1.4.1 EB に入れて良いもの

1. **時間整列 + コインシデンスウィンドウ**でのイベント構築 (§ 8)
2. **検出器固有のノイズフロア** (per-channel `ThresholdADC`、§ 5.1) ―
   *hardware-tied、再較正不要*
3. **L1 トリガー認識** (§ 6) ― 「どの hit を event の起点にするか」の汎用判定
4. **L2 named-ops filter** (§ 7) で**汎用的な**条件:
   - tag ベースの hit カウント (`counter` / `flag` / `accept`)
   - event hit 数下限 (`min_hits`)
   - 時刻窓ベースの veto (`ac_veto` ― 物理 generic な意味で)
   - 単一 (mod, ch) のエネルギー範囲 (`energy_gate`)

#### 1.4.2 EB に入れてはならないもの

| Cut | なぜダメか |
|---|---|
| **検出器配置に依存するペアリング** (例: `mod0_ch X ↔ mod4_ch (15−X)`) | geometry が変われば全データ silent 汚染 |
| **2 body / N body kinematic cut** | 反応仮定が必要、新仮説で再解析する自由を奪う |
| **較正後エネルギーに依存する cut** (Bethe-Bloch banana 上の event だけ採用 等) | 較正パラメータの変更で結果が変わる、再現性低い |
| **粒子種仮定が必要な選択** (proton/α 識別 等) | 物理判断、解析者の責務 |
| **Multiplicity-conditional pairing** (1 sector のみ採用 等) | 反応 topology 依存 |

#### 1.4.3 違反した場合のリスク

1. **データ消失が永続化** ― EB がフィルタで落とした event は ROOT に書かれず、二度と戻らない。CLAUDE.md の zero-loss 原則に直接違反
2. **デバッグ不能** ― 落とされた event は見えず、不審な構造の正体（accidental か real か）が診断できない
3. **再解析の自由を奪う** ― 新しい物理仮定で見直したくなった時、EB を再走させる必要が出る (時間 + ディスク + データ整合性のコスト)
4. **EB が一実験専用ツールに退化** ― 汎用 EB の意味が失われる

#### 1.4.4 物理 cut の正しい置き場所

| 層 | ツール | 例 |
|---|---|---|
| 解析 (人間が確認しながら) | ROOT macro / Python (uproot) / Rust 自前 binary | anti-diagonal pairing, kinematic cut, PID banana, calibration-dependent thresholds |

EB は **追加 cut なしで全 coincidence event を吐き出す**。物理屋は ROOT 出力に対して macro で一段ずつ cut を入れ、各段で histogram を見て妥当性を確認する。

我々の責務は **その解析がやりやすくなるように、event に可能な限り情報を付ける** ことに尽きる (`UserInfo[]`、波形ポインタ、L2 で評価した中間 flag の保存、等の将来拡張)。

---

## 2. 入力データ

### 2.1 OnlineHit (lean, 24 B)

オンライン処理に必要な最小フィールドのみ。波形・user_info・flags は **drop**（raw `.delila` がオフライン解析時の safety net）。

```rust
pub struct OnlineHit {
    pub module: u8,
    pub channel: u8,
    pub energy: u16,
    pub energy_short: u16,
    pub timestamp_ns: f64,
    pub with_ac: bool,    // L2 で AC veto 判定時に set
}
```

メモリレイアウト: 24 B (align 8、padding 込み)。

### 2.2 OfflineHit (rich, 64 B)

オフライン解析では `user_info[4]` と `flags` を保持。波形は **持たない**（raw `.delila` を別途 join するか、必要なら EB 出力に sample range pointer を付ける拡張）。

```rust
pub struct OfflineHit {
    pub module: u8,
    pub channel: u8,
    pub energy: u16,
    pub energy_short: u16,
    pub timestamp_ns: f64,
    pub with_ac: bool,
    pub flags: u64,
    pub user_info: [u64; 4],
}
```

メモリレイアウト: 64 B。

### 2.3 共通 trait `HitLike`

`chunk_builder`, `time_sort`, `slice_builder` 等の pipeline 中核は **trait しか触らない**。output stage だけ具体型に分岐する。

```rust
pub trait HitLike {
    fn module(&self) -> u8;
    fn channel(&self) -> u8;
    fn timestamp_ns(&self) -> f64;
    fn energy(&self) -> u16;
    fn energy_short(&self) -> u16;
    fn with_ac(&self) -> bool;
    fn channel_key(&self) -> u16 {
        ((self.module() as u16) << 8) | (self.channel() as u16)
    }
}
```

### 2.4 データソース

#### オンライン: Merger PUB に直接 subscribe

- **プロトコル:** ZeroMQ SUB
- **データ形式:** MessagePack (`Message::Data(EventDataBatch)`)
- **デフォルトアドレス:** `tcp://localhost:5556` (Merger PUB)
- `EventData` → `OnlineHit` 変換は EB 入口で実施（waveform/user_info/flags を drop）

**Event Bridge プロセスは廃止** (v0.3 で導入された 14 B/hit 固定バイナリ + 別プロセス架構は撤回)。

#### オフライン: `.delila` / ROOT ファイル

- `DelilaFileHitSource`: `.delila` を `DataFileReader` 経由でストリーミング、`EventData` → `OfflineHit` 変換
- `RootFileHitSource` (feature="root"): oxyroot で ROOT を 1 ファイルずつロード

---

## 3. システム構成

```
Reader → Merger ┬─→ Recorder
                ├─→ Monitor (hit-level)
                └─→ EB ┬─→ ROOT writer (内蔵)
                       └─→ ZMQ PUB ─→ EB Monitor (event-level, 別プロセス)
```

- **Monitor:** 既存。hit-level (Mod, Ch ごとの E スペクトル / レート / 波形)、ポート 8081
- **EB Monitor:** 新規プロセス、event-level (multiplicity, coincidence matrix, AC veto 効率, gated E スペ等)、ポート 8082 想定
- 各 pipeline stage が独立プロセス + 自前 REST、という既存パターンを踏襲

---

## 4. 設定ファイル

### 4.1 `eb_config.json` (新規、ランタイム設定)

```jsonc
{
  "version": "1.0",

  "timing": {
    "coincidence_window_ns": 500.0,
    "buffer_delay_ns": 1.0e9,
    "slice_duration_ns": 1.0e7
  },

  "channels_file": "chSettings.json",
  "time_offsets_file": "timeSettings.json",

  "l1": {
    "definitions": [ /* named-ops; § 6.1 */ ],
    "trigger": "<name of root op>"
  },

  "l2": [ /* named-ops; § 7.1 */ ],

  "output": {
    "events_per_file": 1000000,
    "directory": "./eb_output",
    "zmq_pub_endpoint": "tcp://*:5610"
  }
}
```

### 4.2 `chSettings.json` (slim、ハードウェア記述)

```jsonc
[
  [
    {
      "ID": 0,
      "Module": 0,
      "Channel": 0,
      "DetectorType": "HPGe",
      "Tags": ["HPGe", "Trigger"],
      "ThresholdADC": 100,   // hit-level noise floor (§ 5.1)
      "p0": 0.0, "p1": 1.0, "p2": 0.0, "p3": 0.0
    }
  ]
]
```

**削除フィールド** (v0.4 以前との非互換):

| 旧フィールド | 移動先 |
|---|---|
| `IsEventTrigger` | `eb_config.json` の `l1.definitions` (named channel op + trigger 参照) |
| `HasAC`, `ACModule`, `ACChannel` | `eb_config.json` の `l2` (ac_veto op) |

### 4.3 `timeSettings.json` (時刻オフセット較正、tree モデル)

各 ch は **親 ch への参照と offset** を持つ。tree の root は `ref: null`。多 root 許容（disconnected timing domain を表現）。

```jsonc
{
  "version": "1.0",
  "entries": [
    {"module": 9, "channel": 0, "ref": null,   "offset_ns": 0.0},      // root
    {"module": 9, "channel": 1, "ref": [9, 0], "offset_ns": 0.05},
    {"module": 0, "channel": 0, "ref": [9, 0], "offset_ns": 46.75},
    {"module": 5, "channel": 0, "ref": [0, 0], "offset_ns": 12.3}      // 経由参照
  ]
}
```

#### 4.3.1 セマンティクス

- **符号:** `aligned_ts = raw_ts - offset_ns`（親より遅延している量を負値で書く）
  - `offset_ns > 0` → この ch は親より早く届く（aligned 化で時間軸が後退）
  - 現行 `Hit::apply_offset` ([src/event_builder/hit.rs:67](src/event_builder/hit.rs#L67)) と一致
- **絶対 offset:** root から目的 ch までの path 上の `offset_ns` 合計
- **適用タイミング:** HitSource 入口で 1 回のみ。以降 pipeline は **aligned space** で動作（raw/aligned の二重管理なし）
- **多 root の扱い:** EB は root 群を区別せず、各 ch の絶対 offset だけ使って同一 timeline として coincidence 判定する（root 単位の reject なし）
- **freeze:** run 開始時に load + resolve、run 中は不変

#### 4.3.2 Load 時処理

1. `entries` を読み込み、`HashMap<(mod, ch), &Entry>` 構築
2. 各 ch から root まで DFS で walk、絶対 offset を累積 → `HashMap<(mod, ch), f64>` flat キャッシュ
3. 以降 hit 単位の lookup は O(1)

#### 4.3.3 Validation (load 時 check)

| 種別 | 条件 | 対処 |
|---|---|---|
| Error | Cycle あり | abort |
| Error | `ref` 先が `entries` に存在しない | abort |
| Error | 同一 (mod, ch) entry 重複 | abort |
| Warn | root が 2 個以上 | warn ログ（disconnected timing domain）|
| Warn | `chSettings` に存在するが `timeSettings` に欠損 | warn ログ + `offset_ns = 0` で続行 |
| Warn | tree depth > 5 | warn ログ（drift 蓄積懸念）|

`--strict-time-offsets` フラグで warn を error に昇格可。

#### 4.3.4 補助 CLI

```
delila2root eb-offsets <timeSettings.json>
```

→ tree を resolve し flat な `(module, channel, absolute_offset_ns, depth, root)` 表を stdout 出力。JSON 編集時の確認用。

#### 4.3.5 生成ツール

`time_calibrator.rs` が以下を自動実施:

1. 入力 `.delila` / ROOT から全 pair の coincidence histogram 計算
2. 統計十分なペアのみ keep して **graph 構築**
3. `--root M:C` または「最も connected な ch」を root に選択
4. BFS spanning tree → tree 形式の `timeSettings.json` を出力
5. graph が複数の連結成分に分かれる → multi-root として出力 + warn

#### 4.3.6 将来拡張

- ホットリロード via REST `/api/eb/reload-time-offsets`（slice 境界で apply）
- 複数 timeSettings を input file group 別に適用（version-tagged run の合併解析向け）
- Module-level shorthand (`channel: null`) — 現状不採用、必要なら追加

---

## 5. 閾値モデル（2 層）

| 層 | 場所 | 適用タイミング | 性質 | 復元可否 |
|---|---|---|---|---|
| **(1) Event filter** | L2 op | built event 後 | per-event の汎用フィルタ (`min_hits`, `ac_veto`, tag-based counters) | 可 — reject された event のみ捨てる |
| **(2) Analysis cut** | 解析マクロ / Python / 自前 binary | EB 出力を読んだ後 | 実験固有の cut (calibration、geometry、particle ID、energy threshold) | 可 — EB 出力は再解析可能 |

**指針:**

- EB は **エネルギー閾値を一切持たない**。すべての hit を coincidence window 内で event に組み入れる
- ノイズ削減・物理 cut は **すべて解析側** (層 2)
- L2 (層 1) は **物理仮定を含まない汎用フィルタ**のみ。例: 「最低 2 hit 以上」「AC ch が時刻窓内に発火 → reject」「特定タグ集合の hit 数で条件」

**履歴:**

- v0.5.1 から v0.6 までは「3 層モデル」(hit-level floor + L1 trigger gate + L2 event filter) で記述されていた
- v0.7 で `chSettings.ThresholdADC` (Phase J で既に削除) と L1 `energy_gate` (v0.7 で削除) が両方とも EB の責務外 (§ 1.4) であることが確定し、2 層に簡素化

---

## 6. L1: トリガー認識

### 6.1 Named-ops モデル

各 op に `name` を付け、下流は **名前で参照**。JSON ツリー = AST → **パーサ不要**。

```jsonc
"l1": {
  "definitions": [
    {"type": "channel", "name": "HPGe0", "module": 0, "channel": 0},
    {"type": "channel", "name": "HPGe1", "module": 0, "channel": 1},
    {"type": "or", "name": "HPGe_any", "inputs": ["HPGe0", "HPGe1"]},
    {"type": "multiplicity", "name": "HPGe_pair",
     "channels": ["HPGe0", "HPGe1"], "min": 2, "window_ns": 100}
  ],
  "trigger": "HPGe_any"
}
```

`trigger` フィールドはルート op の名前。

### 6.2 Op 種別

| Type | フィールド | 意味 | 状態 |
|---|---|---|---|
| `channel` | `module`, `channel` | 指定チャンネルの hit を trigger 候補に | ✓ |
| `or` | `inputs: [name]` | 複数候補の OR | ✓ |
| `multiplicity` | `channels: [name]`, `min`, `window_ns` | window 内で `min` 個以上の **distinct** channel が発火 | ✓ |
| `and` | `inputs: [name]`, `window_ns` | window 内で全 channel が発火（= multiplicity の `min == |inputs|`） | ✓ |

**Stateful ops (`multiplicity` / `and`):** `inputs` / `channels` は **leaf `channel` op 名のみ**。nest 内で `or` を参照することは未対応。内部的には [`chunk_builder::MultiplicityTrigger`](../../src/event_builder/chunk_builder.rs) の sliding-window scan として実装される。`and` は `min == |inputs|` の multiplicity に lower される。

> **削除されたバリアント:** `energy_gate` は v0.7 で削除されました。per-channel エネルギー閾値は SPEC § 1.4 が禁ずる「データ消失を伴う実験固有 cut」に該当するため、解析マクロ側で実施します。

### 6.3 評価アルゴリズム

```
for each hit at the EB input:
    for each named-op in definitions (topological order):
        evaluate(op) → bool (per hit) or accumulate (multiplicity)
    if op[trigger] is true for this hit:
        mark hit as trigger anchor → 後続の coincidence / overlap 判定へ
```

実装メモ: definitions は依存順 (DAG sort) で評価。`HashMap<String, bool>` に中間結果。

### 6.4 ダブルカウント防止

§ 8.5 のルールを適用: 後方検索で先行トリガーを発見した場合は現在のトリガーを skip（「時間優先」）。

---

## 7. L2: built-event filter

### 7.1 Named-ops モデル (ELIFANT 流)

3 段 chain: `Counter → Flag → Accept`。各 op は `Name` + `Type`、下流は名前参照。

```jsonc
"l2": [
  {"type": "counter", "name": "E_Sector_Counter", "tags": ["E_Sector"]},
  {"type": "counter", "name": "dE_Sector_Counter", "tags": ["dE_Sector"]},
  {"type": "flag", "name": "E_pos", "monitor": "E_Sector_Counter",
   "operator": ">", "value": 0},
  {"type": "flag", "name": "dE_pos", "monitor": "dE_Sector_Counter",
   "operator": ">", "value": 0},
  {"type": "accept", "name": "Si_Both",
   "monitor": ["E_pos", "dE_pos"], "operator": "AND"}
]
```

EB は **すべての `accept` op を OR で**評価し、いずれかが true なら event を出力（出力時に accept 名のリストを attach）。

### 7.2 Op 種別

| Type | フィールド | 意味 | MVP |
|---|---|---|---|
| `counter` | `tags: [string]` | event 内 hit のうち tag が一致する数を count | ✓ |
| `flag` | `monitor`, `operator` (`==`/`!=`/`<`/`<=`/`>`/`>=`), `value` | counter 値と value の比較 → bool | ✓ |
| `accept` | `monitor: [name]`, `operator` (`AND`/`OR`) | flag 群を結合 → event 採否 | ✓ |
| `energy_gate` | `module`, `channel`, `min_adc`, `max_adc` | 特定 ch hit の E が範囲内か | 後 |
| `ac_veto` | `trigger_channels: [{module,channel}]`, `veto_channels: [{module,channel}]`, `window_ns` | trigger 周辺に veto ch が発火 → reject | 後 |
| `min_hits` | `min: u32` | event 内 hit 数下限 | 後 |

### 7.3 評価アルゴリズム

```
for each built event:
    reset counter map
    for each op in l2 (順序):
        match op.type:
            Counter: for hit in event: if hit.tags ∩ op.tags: counter[op.name]++
            Flag: flag[op.name] = compare(counter[op.monitor], op.operator, op.value)
            Accept: result[op.name] = combine(flag[m] for m in op.monitor, op.operator)
            ...
    if any accept is true: keep event (attach accepted names)
    else: drop
```

---

## 8. イベント構築アルゴリズム

### 8.1 処理フロー

```
HitSource ─→ Time Sort ─→ Time Slice ─→ L1 → Coincidence → L2 → Output
            (buffer 1s)   (10ms slice +    detect    ±window     filter
                          500ns overlap)  trigger    coincidence  + accept
                                                     hits
```

### 8.2 時間ソート

非同期到着 hit を時間順に整列。

- データ構造: 時刻 key の sorted buffer (実装は `time_sort.rs` 参照)
- 取り出し: `current_time - buffer_delay_ns` より古い hit を pop（時間順保証）
- `buffer_delay_ns`: デフォルト 1 秒（到着遅延の最大値 + マージン）

### 8.3 タイムスライス（オーバーラップ方式）

```
時間軸 ─────────────────────────────▶
Slice N:   [===== Core =====][= Overlap =]
                              ↑
                         coincidence_window
Slice N+1:               [= Overlap =][===== Core =====]
```

**境界処理:**
- トリガーが Core 内 → 当該 slice で処理
- トリガーが Overlap 内 → **次 slice で処理**（重複防止）

`slice_duration_ns`: デフォルト 10 ms、`overlap` = `coincidence_window_ns`。

### 8.4 コインシデンス検出

トリガー周辺 ±`coincidence_window_ns` の hit を集める。

```
trigger anchor hit at t0:
    後方検索: t0 - coincidence_window_ns まで遡って hit を収集
    前方検索: t0 + coincidence_window_ns まで進んで hit を収集
    途中で他の trigger anchor を発見:
        後方 → 現在の trigger を破棄（時間優先、§ 8.5）
        前方 → 検索停止（次 event へ）
```

### 8.5 ダブルカウント防止（時間優先）

```
時間軸 ──────────────────────────▶
        T1        T2
        ●─────────●
        │◀─window─▶│
T1 で event 構築 ✓、T2 は skip
```

採用理由: 物理的に自然（先に起きた事象を採用）、実装シンプル、恣意性なし。

---

## 9. 出力データ

### 9.1 `BuiltEvent` 構造

```rust
pub struct BuiltEvent {
    pub event_id: u64,
    pub trigger_time_ns: f64,
    pub trigger_module: u8,
    pub trigger_channel: u8,
    pub hits: Vec<EventHit>,        // 型は OnlineHit/OfflineHit 派生
    pub accepted: Vec<String>,      // L2 で true になった accept op の名前
}

pub struct EventHit {
    pub module: u8,
    pub channel: u8,
    pub energy: u16,
    pub energy_short: u16,
    pub relative_time_ns: f64,      // trigger 基準
    pub with_ac: bool,
    // OfflineHit 由来時のみ:
    pub flags: Option<u64>,
    pub user_info: Option<[u64; 4]>,
}
```

### 9.2 ROOT 出力（Rust 内蔵）

`event_builder::root_io` (oxyroot) で直接書き込み。TTree `Events`、ファイルローテーション `events_per_file` 単位。

**Branch:**
- `EventID` (ULong64_t), `TriggerTime` (Double_t), `TriggerMod`/`TriggerCh` (UChar_t), `NHits` (UInt_t)
- 可変長配列: `HitMod[NHits]`, `HitCh[NHits]`, `HitEnergy[NHits]`, `HitEnergyShort[NHits]`, `HitTime[NHits]`, `HitWithAC[NHits]`
- オフラインのみ: `HitFlags[NHits]` (ULong64_t), `HitUserInfo0..3[NHits]` (ULong64_t)
- `Accepted` (vector<string>) — L2 で採用された accept op 名

### 9.3 ZMQ PUB → EB Monitor

```
形式:     MessagePack (rmp-serde)
型:       BuiltEventBatch { events: Vec<BuiltEvent> }
エンドポイント: tcp://*:5610 (デフォルト)
パターン: PUB / SUB
HWM:      0 (unlimited, データ落とさない原則)
EOS:      Message::EndOfStream { run_number }
```

C++ EB 用の 14 B 固定バイナリ wire format と `event_bridge` バイナリは Phase Q (2026-05-19) で削除済み。`docs/event_bridge_wire_format.md` も同時に削除。

---

## 10. 性能要件

### 10.1 処理レート

| 条件 | 要求 |
|------|------|
| オンライン入力 | 2 MHz hits |
| イベント構築レート | trigger レート依存（typical 10〜100 kHz） |

### 10.2 メモリ使用量

**波形は EB に渡さない前提**で見積もり:

| モード | 1 hit | 1 秒分 (2 MHz) | 安全マージン 10× |
|---|---|---|---|
| Online (16 B) | 32 MB | 320 MB | 10 GB 上限内 |
| Offline (56 B) | 112 MB | 1.12 GB | 通常オンラインレートで走らせない |

波形が必要な解析は raw `.delila` から別途読み出し（オフライン EB 出力に sample range pointer を attach する拡張は将来検討）。

---

## 11. 統一パイプライン (TODO 38)

### 11.1 `HitSource` trait

```rust
pub trait HitSource<H: HitLike> {
    fn next_batch(&mut self, timeout: Duration) -> Result<HitBatch<H>, SourceError>;
    fn name(&self) -> &str;
}

pub enum HitBatch<H> {
    Hits(Vec<H>),
    Eos,
}
```

### 11.2 実装一覧

| 実装 | モード | 入力 | Hit 型 | Phase |
|---|---|---|---|---|
| `DelilaFileHitSource` | Offline | `.delila` | `OfflineHit` | ✓ (Phase 1) |
| `RootFileHitSource` | Offline | ROOT | `OfflineHit` | ✓ (Phase 1) |
| `ZmqHitSource` | Online | Merger PUB | `OnlineHit` | **Phase 4 (未着手)** |

### 11.3 共通中核

- `chunk_builder.rs`: trigger 認識 + coincidence 検出 (`HitLike` generic)
- `time_sort.rs`, `slice_builder.rs`, `time_calibrator.rs`
- `pipeline.rs`: Sorter thread / Worker threads / Writer thread

### 11.4 進捗

- **Phase 4** (✓ 2026-05-19): `ZmqHitSource` 実装 + Online EB を統一 pipeline 上に乗せ替え
- **Phase 5** (✓ 2026-05-19): 旧 `online.rs` (独自 pipeline) 削除
- **Phase J** (✓ 2026-05-19): `chSettings.json` を tags + 較正 のみに slim 化、`L1Builder` 削除
- **Phase Q** (✓ 2026-05-19): `event_bridge` バイナリ + wire format doc 削除
- **L1 stateful** (✓ 2026-05-19): `multiplicity` + `and` を `MultiplicityTrigger` の sliding-window scan として実装
- **M**: EB Monitor プロセス本体（histograms + REST）

---

## 12. エラー処理

### 12.1 データ異常

| 異常 | 検出 | 対処 |
|------|------|------|
| タイムスタンプ逆転 | 前 hit より小さい | warn ログ + skip |
| 無効 mod/ch | chSettings 範囲外 | warn ログ + skip |
| 大きな時間ジャンプ | configurable threshold 超過 | info ログ + 継続 |

### 12.2 システム異常

| 異常 | 検出 | 対処 |
|------|------|------|
| メモリ不足 | sorter buffer サイズ監視 | backpressure (HitSource 側に伝播) |
| 入力停止 | timeout | flush 処理 |
| 出力エラー | I/O error | retry + ログ |
| EB Monitor 切断 | ZMQ HWM = 0 で堆積 | PUB は止めない（subscribe 復帰で再送される） |

---

## 13. 設定パラメータ一覧

| パラメータ | 型 | デフォルト | 場所 |
|---|---|---|---|
| `coincidence_window_ns` | f64 | 500.0 | `eb_config.json` `timing` |
| `buffer_delay_ns` | f64 | 1e9 | 〃 |
| `slice_duration_ns` | f64 | 1e7 | 〃 |
| `events_per_file` | u64 | 1_000_000 | `output` |
| `directory` | string | `./eb_output` | 〃 |
| `zmq_pub_endpoint` | string | `tcp://*:5610` | 〃 |
| `ThresholdADC` | u32 | 0 | `chSettings.json` per channel |

---

## 14. 用語集

| 用語 | 説明 |
|---|---|
| Hit | デジタイザからの 1 検出信号 (`OnlineHit` / `OfflineHit`) |
| Event | コインシデンスウィンドウ内の hit 集合 (`BuiltEvent`) |
| Trigger anchor | L1 で選ばれた event 構築の基準 hit |
| L1 | Trigger 認識（どの hit を event の起点にするか） |
| L2 | Built event のフィルタリング (Accept / Reject) |
| Timeslice | 独立処理可能な時間区間 (Core + Overlap) |
| Coincidence | 時間的に近接した複数 hit の同時検出 |
| AC | Active Shield（アクティブシールド、L2 veto op） |
| PSD | Pulse Shape Discrimination（波形弁別） |
| Tag | チャンネルに付ける free-form 識別子 (`chSettings.Tags`) |
| Named-ops | 各 op が `name` を持ち、下流が名前で参照する DSL パターン |

---

## 15. 変更履歴

| 日付 | バージョン | 変更内容 |
|------|-----------|----------|
| 2025-01-27 | 0.1.0 | 初版作成 |
| 2025-01-27 | 0.2.0 | energy_short 追加、ROOT/ヒストグラム方式決定 |
| 2026-01-27 | 0.3.0 | Event Bridge (Rust) → C++ EB 経路の決定 |
| 2026-02-02 | 0.4.0 | Phase 7: Time Slice 方式へ移行 |
| 2026-05-19 | 0.5.0 | **大規模改訂**: C++ EB 経路を撤回 / OnlineHit と OfflineHit 分離 / L1+L2 named-ops モデル / 3 層 threshold モデル / Event Bridge retire / EB Monitor 新プロセス / 統一パイプライン位置付け明確化 |
| 2026-05-19 | 0.5.1 | `timeSettings.json` を tree モデルに刷新（C 案）— 多 root 許容 / HitSource 入口でオフセット 1 回適用 / `time_reference` field 廃止 / 補助 CLI `eb-offsets` 追加 |
| 2026-05-20 | 0.6.0 | **§ 1.4 設計原則 — EB の責務境界**を追加。EB は汎用エンジン、実験固有の geometry / kinematic / particle-ID cut は物理屋が下流解析で行う、と明文化。ELIFANT2025 p91Zr の実データテストで anti-diagonal sector pairing を EB に押し込む誘惑が発生したのを契機に整理 |
| 2026-05-20 | 0.7.0 | **L1 `energy_gate` 廃止**。p91Zr v3 ラン (`min_adc=100`) で 0.06 % しか events 変動せず、§ 1.4 が禁ずる「永続的データ消失を伴う閾値 cut」に該当することが実証された。閾値モデルを 3 層 → 2 層 (L2 generic filter + 解析 cut) に簡素化。SPEC § 5 / § 6.2 / runtime_config.rs から `energy_gate` / `trigger_energy_gates` 関連を完全削除 |
| 2026-08-04 | 0.8.0 | **実装言語を C++ (root_sink 系譜) へ再転換**([TODO 66](66_cpp_event_builder.md))。根拠 = oxyroot 書き込み圧縮未実装の確定 + root_sink/watermark matcher の実証(side3 ThGEM)+ TDelila による online=offline 同一コード。Rust 統一パイプラインは凍結・参照実装化。概念仕様(§1.4/L1/L2/threshold/timeSettings)は C++ 実装に引き継ぎ |
