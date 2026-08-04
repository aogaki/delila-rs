# DELILA-Rust Offline Event Builder マニュアル

> **Status (2026-08-04): 凍結版。** 本マニュアルの Rust `event_builder` は
> 記載どおり動作し続けます(参照実装として維持)が、EB の新規開発は
> C++(root_sink 系譜)へ移行しました —
> [TODO/event-builder/66_cpp_event_builder.md](../TODO/event-builder/66_cpp_event_builder.md) 参照。
> 既知の制約: oxyroot は書き込み圧縮が未実装のため、本ツールの ROOT 出力は
> **常に非圧縮**です(C++/ZSTD 比で 1.5〜3 倍のサイズ)。

`.delila` 生データから ROOT 形式の **built event** ファイルを作る一連の
オフライン処理を最初から最後まで通して説明します。**物理屋が自分で実行
することを想定**したマニュアルです。

> 関連 SPEC: [`TODO/event-builder/SPECIFICATION.md`](../TODO/event-builder/SPECIFICATION.md)
> (v0.8)。EB の責務境界は SPEC § 1.4 を参照してください。

---

## 1. 概要

### 1.1 入出力

```
.delila ファイル群 ──┬──→ time calibration → timeSettings.json
                      │
                      ↓
                  event_builder build ─┬──→ eb_runXXXX_NNNN_events.root (1〜複数)
                                       │
                                       └── (オプション) ZMQ PUB
                                            → EB Monitor へ
```

- **入力**: `.delila` 形式の生 hit データ（Reader/Recorder が書き出すバイナリ）
- **出力**:
  - ROOT TTree (デフォルト名 `EventTree`) を含む `.root` ファイル
  - `events_per_file` 件ごとにローテーション
- **設定ファイル** 3 種:
  - `chSettings.json` — チャンネルの記述（tag, calibration polynomial）
  - `eb_config.json` — L1 トリガー認識と L2 event filter
  - `timeSettings.json` — channel 毎の時刻オフセット（次節で生成）

### 1.2 全体フロー

```
[1] バイナリを準備               cargo build --release --features root --bins
[2] chSettings.json を書く        手作業 or ELIFANT 互換 JSON 流用
[3] eb_config.json を書く         L1/L2 を named-ops で定義
[4] 時間較正                      event_builder time-calib → timeSettings.json
[5] (オプション) 較正結果を確認   eb_offsets <timeSettings.json>
[6] イベント構築                   event_builder build → ROOT 出力
[7] 解析                          ROOT macro / Python (uproot) / 自前ツール
```

---

## 2. 必要なバイナリの準備

リポジトリのルートで:

```bash
cargo build --release --features root --bin event_builder
cargo build --release --bin eb_offsets
```

それぞれ `./target/release/event_builder`, `./target/release/eb_offsets`
として置かれます。以下では PATH に通しているか、フルパスで呼ぶ前提です。

確認:

```bash
./target/release/event_builder --help
./target/release/event_builder init --help          # § 4.0
./target/release/event_builder build --help         # § 6
./target/release/event_builder time-calib --help    # § 5
./target/release/event_builder validate-config --help  # § 4.4
./target/release/eb_offsets --help
```

`--features root` が無いと event_builder のメイン処理（ROOT 出力）が無効化
されるので必ず指定してください。

---

## 3. データ準備

### 3.1 入力ファイルの場所と命名

DELILA Reader/Recorder は `output_dir/runXXXX_NNNN_data.delila`
（XXXX = run number, NNNN = file index）として書き出します。
`event_builder build` には複数ファイルを並べて渡せます:

```bash
event_builder build -i ./data/run0042_*.delila ...
```

シェルの glob 展開で OS の引数長制限 (macOS は通常 256 KB) に収まる範囲なら
そのまま渡せます。

### 3.2 中身を覗いておく

`event_dump` で軽くチェックできます:

```bash
./target/release/event_dump ./data/run0042_0000_data.delila --summary
```

最初の数 event の Mod / Ch / TimeStamp が想定通り出ているか、
タイムスタンプ範囲が run の長さに整合するか確認しておくと
後の時間較正のデバッグが楽になります。

---

## 4. 設定ファイル

設定ファイルは **JSON Schema 付き** です。冒頭に `"$schema"` 行を追加すれば
VSCode / IntelliJ / Neovim+LSP が:

- フィールド名を autocomplete
- L1/L2 op type を dropdown 表示
- 必須 missing で赤線
- 値の範囲外を警告
- hover で field 説明を inline 表示

してくれます。**JSON 構文を覚える必要はほぼなくなります**。

```jsonc
{
  "$schema": "/path/to/delila-rs/schemas/eb_config.schema.json",
  "version": "1.0",
  ...
}
```

リポジトリの schema:

- [`schemas/eb_config.schema.json`](../schemas/eb_config.schema.json) — eb_config.json
- [`schemas/chSettings.schema.json`](../schemas/chSettings.schema.json) — chSettings.json
- [`schemas/timeSettings.schema.json`](../schemas/timeSettings.schema.json) — timeSettings.json (tree form)

### 4.0 テンプレートから始める

書き起こす前に近いテンプレを見るのが速いです:

```bash
ls templates/eb_config/
# hpge_with_ac_veto.json  multiplicity_2.json  README.md
# si_telescope.json       single_trigger.json
```

[`templates/eb_config/README.md`](../templates/eb_config/README.md) に各
テンプレの用途と選び方が書いてあります。`cp` してから channel 番号や
tag を実機構成に書き換える運用が想定。

`chSettings.json` と `timeSettings.json` は実機構成依存で
テンプレが効きにくいので、骨格を CLI で吐かせます:

```bash
# 11 module × 16 ch の chSettings 骨格を生成。
# モジュール 0 を HPGe+Trigger、モジュール 4 を Si dE_Sector に。
event_builder init chsettings \
    --modules 11 \
    --channels 16 \
    --module-type "0:HPGe:HPGe,Trigger" \
    --module-type "4:Si:dE_Sector" \
    -o chSettings.json

# chSettings.json をもとに timeSettings 骨格 (offsets=0) を生成。
# 後で `event_builder time-calib` で実測値に置き換える。
event_builder init timesettings \
    -c chSettings.json \
    --root 0:0 \
    -o timeSettings.json
```

`--module-type` を省略すると全 module が `DetectorType="Unknown"` /
`Tags=[]` で出力されます。calibration は常に identity (`p0=0, p1=1`)、
あとで書き換えてください。

`timesettings` の `--root M:C` は **時間基準として使う参照チャンネル**
（典型的にはトリガーチャンネル、例えば HPGe の trigger sector）を
指定します。tree 内では `parent = None` のノードになり、他の全チャンネル
の時間オフセットはこのチャンネルの時刻からの差として測定されます。
あとで `event_builder time-calib --ref-module M --ref-channel C` を
**同じ `(M, C)`** で走らせると、骨格の `offset_ns: 0.0` が実測値に
置き換わります。

### 4.1 `chSettings.json`（チャンネル記述）

純粋なハードウェア記述ファイルです。Phase J で trigger 関連フィールドは
削除されています ([SPEC § 4.2](../TODO/event-builder/SPECIFICATION.md)):

```jsonc
[
  [   // module 0 — 16 channels
    {
      "Module": 0,
      "Channel": 0,
      "DetectorType": "HPGe",
      "Tags": ["HPGe", "Trigger"],
      "p0": 0.0, "p1": 1.0, "p2": 0.0, "p3": 0.0
    },
    {
      "Module": 0,
      "Channel": 1,
      "DetectorType": "Si",
      "Tags": ["dE_Sector"],
      "p0": 4.44, "p1": 1.06, "p2": 0.0, "p3": 0.0
    }
  ],
  [   // module 1 ...
  ]
]
```

| フィールド | 用途 |
|---|---|
| `Module`, `Channel` | ハードウェア (mod, ch)（必須） — channel の identity はこの 2 つだけ |
| `DetectorType` | 検出器種別の自由文字列（`HPGe` / `Si` / `AC` / `PMT` 等、ドキュメント目的） |
| `Tags` | L2 `counter` op が hit を選別するためのタグ集合 |
| `p0..p3` | エネルギー較正多項式 `E = p0 + p1·ADC + p2·ADC² + p3·ADC³`（解析側で利用） |

> 旧 ELIFANT-Event 由来の `"ID"` キーは SPEC v0.7.1 で廃止しました。残っていても
> serde が無視するので既存の `chSettings.json` はそのまま読めますが、新規作成時は
> 書かないでください。

**ELIFANT-Event の chSettings.json から変換**したい場合:

```bash
python3 tools/p91zr_test/convert_elifant_config.py \
    --elifant-dir /path/to/ELIFANT-Event/all_run_X \
    --out-dir /path/to/work
```

ELIFANT の `IsEventTrigger` / `HasAC` / `ACModule` / `ACChannel` /
`ThresholdADC` などは無視され、tag と calibration だけが取り出されます。

### 4.2 `eb_config.json`（ランタイム設定）

L1 トリガー認識と L2 event filter を named-ops 形式で書きます
([SPEC §§ 4.1, 6, 7](../TODO/event-builder/SPECIFICATION.md)):

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
    "definitions": [
      {"type": "channel", "name": "HPGe0", "module": 0, "channel": 0},
      {"type": "channel", "name": "HPGe1", "module": 0, "channel": 1},
      {"type": "or",      "name": "any_HPGe", "inputs": ["HPGe0", "HPGe1"]}
    ],
    "trigger": "any_HPGe"
  },

  "l2": [
    {"type": "counter", "name": "HPGe_count",  "tags": ["HPGe"]},
    {"type": "flag",    "name": "HPGe_fired",  "monitor": "HPGe_count",
     "operator": ">",   "value": 0},
    {"type": "min_hits","name": "atleast2",    "min": 2},
    {"type": "accept",  "name": "keep",
     "monitor": ["HPGe_fired", "atleast2"], "operator": "AND"}
  ],

  "output": {
    "events_per_file": 1000000,
    "directory": "./eb_output",
    "zmq_pub_endpoint": null
  }
}
```

#### L1 ブロックの構造

| キー | 役割 |
|---|---|
| `definitions` | 名前付き op の AST。`channel` / `or` / `and` / `multiplicity` を任意に組み合わせて、内部 op は別の op の `inputs` から参照できる |
| `trigger` | `definitions` の中から **エントリポイントとなる op の `name`** を指定する。EB はこの op を評価してイベント窓を開く。例: `"trigger": "any_HPGe"` → `any_HPGe` という名前で定義した `or` op が L1 の最終判定になる |

> **トップレベル op は 1 つだけ**。複数の独立トリガーを OR したい場合は、
> それらを `or` op の `inputs` にまとめて 1 本のツリーにして、その root を
> `trigger` に指定します。

#### L1 で使える op (SPEC § 6.2)

| Type | 内容 | 入力 |
|---|---|---|
| `channel` | この (module, channel) の hit を trigger anchor 候補にする | (module, channel) |
| `module` | module 全体（0..`channels`-1 ch）を trigger 候補に展開する syntactic sugar — 16/32/64 ch の digitizer で `channel` を 16 回書く手間を 1 行にまとめる | (module, channels) |
| `or` | 子 op のいずれかが true | `inputs: [name]` |
| `multiplicity` | window 内で `min` 個の distinct channel が発火。input は `channel` または `module` 名（`module` は ch 群に展開される → 「モジュールの任意 N ch coincidence」が 1 行で書ける） | `channels`, `min`, `window_ns` |
| `and` | window 内で全 channel が発火（= multiplicity の `min == |inputs|`）。input は `channel` / `module` どちらでも可 | `inputs`, `window_ns` |

> **L1 `energy_gate` は v0.7 で削除されました**。エネルギー閾値は解析マクロでかけてください (SPEC § 5)。

例: モジュール 0 が 16 ch HPGe アレイで「いずれかの ch が発火すればトリガー」:

```json
{"type": "module", "name": "any_HPGe", "module": 0, "channels": 16}
```

これと等価な旧書式:

```json
{"type": "channel", "name": "HPGe0",  "module": 0, "channel": 0},
{"type": "channel", "name": "HPGe1",  "module": 0, "channel": 1},
... 14 行省略 ...
{"type": "channel", "name": "HPGe15", "module": 0, "channel": 15},
{"type": "or", "name": "any_HPGe",
 "inputs": ["HPGe0","HPGe1", ... ,"HPGe15"]}
```

例: モジュール 0 (HPGe 16 ch) のうち任意 2 ch が 200 ns 以内に同時発火:

```json
{"type": "module",       "name": "any_HPGe", "module": 0, "channels": 16},
{"type": "multiplicity", "name": "pair",     "channels": ["any_HPGe"],
 "min": 2, "window_ns": 200.0}
```

> ⚠️ **`channels` の過剰指定は黙って無視されます**。
> 例えば 16 ch しか持たない digitizer に対して `"channels": 32` と書いても
> エラーにはならず、存在しない `(M, 16)..(M, 31)` のエントリが trigger
> 集合に積まれるだけで、ヒットが来ないので一切マッチしません（データ・性能
> どちらにも影響なし）。
>
> 逆に **過小指定** には注意が必要です。32 ch digitizer に
> `"channels": 16` を書くと **後半 16 ch のヒットが trigger 候補から外れる**
> ため、本来取れるはずのイベントを取りこぼします。実機の channel 数とは
> 必ず一致させてください（[`chSettings.json`](#41-chsettingsjsonチャンネル記述)
> の module 配列長と同じ値）。

#### L2 で使える op (SPEC § 7.2)

| Type | 内容 |
|---|---|
| `counter` | event 内の hit のうち、チャンネル tag が一致するものを count |
| `flag` | counter 値 vs `value` を `operator` で比較 → bool |
| `accept` | flag 群を AND/OR で結合。**いずれかの `accept` が true なら event を keep** |
| `energy_gate` | event 内に該当 (mod, ch) で `min_adc ≤ E ≤ max_adc` の hit があれば true |
| `min_hits` | event hit 数が `min` 以上か |
| `ac_veto` | trigger_channels と veto_channels が window_ns 内で同時発火していれば true（**veto = drop したい場合は Flag(== 0) で反転**） |

`accept` op が 1 つも `true` にならなかった event は drop されます。

> ⚠️ **L2 を空 (`"l2": []`) にしたり `accept` op を 1 つも書かないと、
> 暗黙のフォールバック (all-accept / all-drop) ではなく
> `validate-config` と `event_builder build` の両方で load エラーになります**
> (`"L2 chain must contain at least one accept op to gate event output"`)。
>
> 「全部 keep したい」場合は明示的に `min_hits: 1` + `accept` を書きます:
>
> ```json
> "l2": [
>   {"type": "min_hits", "name": "any_hit", "min": 1},
>   {"type": "accept",   "name": "keep_all",
>    "monitor": ["any_hit"], "operator": "AND"}
> ]
> ```
>
> （[`templates/eb_config/single_trigger.json`](../templates/eb_config/single_trigger.json) と同じパターン）

#### timing パラメータ

| パラメータ | 意味 | 標準値 |
|---|---|---|
| `coincidence_window_ns` | trigger 周辺 ±window 内の hit を 1 event に集める | 100〜500 ns |
| `buffer_delay_ns` | 到着遅延吸収のための time-sort バッファ深さ | 1e9 (1 s) |
| `slice_duration_ns` | 並列処理用の time slice 長 | 1e7 (10 ms) |

### 4.4 検証する

設定ファイルを書いた / 編集した後は、いきなり `event_builder build` を
走らせて発覚するより、軽量に validate するのがおすすめです:

```bash
event_builder validate-config ./work/eb_config.json
event_builder validate-config ./work/chSettings.json
event_builder validate-config ./work/timeSettings.json
```

成功時:

```
[OK] eb_config.json — OK (81 L1 ops, root='trigger', 0 multiplicity ops, 5 L2 ops, 80 static triggers)
```

失敗時は L1 cross-reference の typo、cycle、L2 accept op の欠落、
timeSettings tree の dangling parent などを行番号付きで指摘します
（JSON Schema (editor 側) と相補的: schema は構文・列挙、validate-config
は意味論）。

自動判別が誤動作する場合は `--kind eb-config | ch-settings | time-offsets`
で明示できます。

### 4.3 `timeSettings.json`（次節で生成）

時間較正の出力。形式は 2 種類:

| 形式 | 説明 | event_builder の対応 |
|---|---|---|
| Tree (SPEC § 4.3) | 推奨。`{version, entries: [{module, channel, ref, offset_ns}]}` | 読める ✓ |
| Legacy (1 ref) | `{ref_module, ref_channel, offsets: {"MM_CC": offset}}` | 読める ✓ (フォールバック) |

`event_builder time-calib` は legacy 形式で書きます。読む側は両形式対応な
ので気にせず使えます。

---

## 5. 時間較正 (time-calib)

### 5.1 何をするか

各チャンネル毎に **reference channel に対する時刻オフセット**を計測します。
ハードウェアの伝送遅延 / 配線長 / カウンタ起動タイミングのずれを吸収する
ためで、これをやらないと coincidence window から外れて event が組まれない
hit が大量発生します（p91Zr 実証では 95% の coincidence が失われていました）。

アルゴリズム:

1. reference channel (`ref_module, ref_channel`) の hit を anchor として
   ±`window` 内に発火している他チャンネルの hit との時刻差を集計
2. 各 channel ごとに分布の peak を fit → offset とする
3. `min_entries` 件以上集まったチャンネルを valid と判定

### 5.2 走らせる

```bash
./target/release/event_builder time-calib \
    --input ./data/run0042_*.delila \
    --output ./work/timeSettings.json \
    --ref-module 0 --ref-channel 0 \
    --window 1000 \
    --min-entries 1000 \
    --hist-output ./work/timeAlignment.root
```

| フラグ | 意味 | 既定 |
|---|---|---|
| `-i, --input` | 入力 .delila 群 | 必須 |
| `-o, --output` | 出力 timeSettings.json | `timeSettings.json` |
| `--ref-module / --ref-channel` | reference channel | `0 / 0` |
| `--window` | 探索ウィンドウ [ns]（広め推奨：オフセット未知のため） | `1000` |
| `--min-entries` | この件数未満の ch は offset = 0 のまま | `1000` |
| `--max-events` | 0 で全 event 使う、>0 で打ち切り（デバッグ用） | `0` |
| `--hist-output` | 視覚確認用 ROOT ヒストグラム | `timeAlignment.root` |
| `--ref-energy-min/max` | reference hit をエネルギーで絞り込む | 全範囲 |

**reference channel 選定のコツ:**
- ノイズが少なく、レートが十分高い channel を選ぶ
- 望ましくは「全体の trigger 源」になっている channel（HPGe / Trigger detector など）
- ref-energy-min/max で reference を物理ピーク領域に絞ると分布が clean になる

### 5.3 較正結果の確認

#### (a) フラットな表で見る

```bash
./target/release/eb_offsets ./work/timeSettings.json
```

出力例:

```
 mod   ch   abs_offset_ns  depth  rmod  rch
--------------------------------------------------
   0    0           0.000      0     0    0
   0    1          12.300      1     0    0
   1    0         100.500      1     0    0
   ...
```

オプション:
- `--sort abs` — オフセット値順
- `--sort depth` — tree 深度順
- `--csv` — CSV 出力

**確認すべきこと:**
- すべてのチャンネルが期待する範囲（数 ns 〜 数百 ns）にあるか
- 異常に大きな値（〜数 µs）は ch 故障や mis-cabling の疑い
- 0.0 が並ぶ ch は `min-entries` 不足の可能性

#### (b) ヒストグラムで確認

`--hist-output timeAlignment.root` で書き出した ROOT には `TimeAlignment` TTree が入っており、
1 ch = 1 entry で `BinCenters` + `Counts` を vector で保持しています。

全 ch のピークが Δt = 0 に揃っているかを 2D heatmap + per-ch 1D で
ひと目確認するには付属マクロを使います:

```bash
root -l 'macros/plot_time_alignment.C("./work/timeAlignment.root")'
```

→ 2D heatmap（横軸 Δt [ns]、縦軸 channel ID、色 = count）と上位 12 ch
の 1D 投影、サマリー表が表示されます。ピークが揃っていなければそのまま
見える。

peak が綺麗に出ていない ch は信頼性が低いので、後の解析で除外するか
再計算してください。

### 5.4 timeSettings.json を tree 形式に変換したい場合

`event_builder time-calib` は legacy 形式で書きます。tree 形式のほうが
SPEC v0.5.1 以降の正式形式ですが、event_builder build はどちらも読める
ので必須ではありません。tree が必要なら手作業で:

```json
{
  "version": "1.0",
  "entries": [
    {"module": 0, "channel": 0, "ref": null,   "offset_ns": 0.0},
    {"module": 0, "channel": 1, "ref": [0, 0], "offset_ns": 12.3}
  ]
}
```

（root = ref-module/ref-channel、それ以外は ref: [ref_mod, ref_ch]）。

---

## 6. イベントビルド (build)

### 6.1 走らせる

```bash
./target/release/event_builder build \
    --input ./data/run0042_*.delila \
    --output ./work/eb_output \
    --config ./work/chSettings.json \
    --eb-config ./work/eb_config.json \
    --time-calib ./work/timeSettings.json \
    --run-id 42 \
    --workers 4 \
    --writers 2 \
    --events-per-file 1000000
```

| フラグ | 意味 | 既定 |
|---|---|---|
| `-i, --input` | 入力 .delila（複数可、--root-input で ROOT も可） | 必須 |
| `-o, --output` | 出力ディレクトリ | `.` |
| `-c, --config` | chSettings.json | optional だが指定推奨（tag map） |
| `--eb-config` | eb_config.json（L1+L2） | **指定すると --trigger / --window は無視** |
| `-T, --time-calib` | timeSettings.json | optional（無いと offset=0） |
| `--window` | coincidence window [ns] (--eb-config 不使用時のみ) | `500` |
| `--output-tree` | 出力 TTree 名 | `EventTree` |
| `--run-id` | ファイル名の `runXXXX` 部分 | `0` |
| `--workers` | worker thread 数（event 構築） | `4` |
| `--writers` | writer thread 数（ROOT I/O） | `2` |
| `--events-per-file` | ROOT ファイルローテーション粒度 | `100000` |
| `--trigger` | CLI 簡易トリガー `module:channel`（複数可） | `--eb-config` がない場合の fallback |
| `--root-input` | 入力を `.delila` ではなく ROOT (ELIADE_Tree) と解釈 | off |
| `--root-tree` | --root-input 時の TTree 名 | `ELIADE_Tree` |

### 6.2 推奨ワークフロー (現在の事実上標準)

```
--eb-config ./eb_config.json       # L1+L2 すべて JSON で記述
--config    ./chSettings.json      # tag map (L2 counter ops で必要)
--time-calib ./timeSettings.json   # 時間較正
```

`--trigger` の CLI flag は **`--eb-config` が指定されている時は無視されます**。
過去のアドホック動作用に残してあります。

### 6.3 出力ファイル

```
work/eb_output/eb_run0042_0000_events.root
work/eb_output/eb_run0042_0001_events.root
...
```

- ファイル名: `eb_run<RUN_ID:04d>_<INDEX:04d>_events.root`
- `events_per_file` 件で次のファイルへ
- `n_writers` の writer が並列で書く → INDEX は worker 別に飛ぶことがある（ファイル間で event_id は時刻順、ファイル内も時刻順）

### 6.4 TTree スキーマ（`EventTree`）

| Branch | 型 | 説明 |
|---|---|---|
| `EventID` | `ULong64_t` | run 内で一意の連番 |
| `TriggerTime` | `Double_t` | trigger hit の絶対時刻 [ns]（time calibration 適用後） |
| `TriggerMod` | `UChar_t` | trigger hit の module |
| `TriggerCh` | `UChar_t` | trigger hit の channel |
| `Multiplicity` | `UInt_t` | event 内の hit 数 |
| `Mod` | `vector<unsigned char>` | hit ごとの module |
| `Ch` | `vector<unsigned char>` | hit ごとの channel |
| `Energy` | `vector<unsigned short>` | hit ごとの長ゲートエネルギー (ADC) |
| `EnergyShort` | `vector<unsigned short>` | 短ゲート (PSD 用) |
| `RelTime` | `vector<double>` | trigger からの相対時刻 [ns] |
| `WithAC` | `vector<unsigned char>` | L2 `ac_veto` で立てる flag (使ってなければ 0) |
| `<counter op の name>` | `Long64_t` | **L2 counter op ごとに 1 本ずつ自動追加**。値は per-event の counter 評価結果（例: `HPGe_count = 3`、`E_Sector_Counter = 1`）。L2 を使わない config では出力なし |

> **calibrated energy は入っていません**。`Energy` は ADC 値。calibration は
> `chSettings.p0..p3` を解析時に適用してください (SPEC § 1.4 — calibration は
> 解析側の責務)。

> ⚠️ **L2 counter の `name` は ROOT TTree branch 名にそのまま使われます**。
> `-` / `.` / 空白 / 先頭数字を含む名前は ROOT の演算子と衝突するので、
> JSON Schema と `validate-config` の両方で reject します
> （正規表現 `^[A-Za-z_][A-Za-z0-9_]*$`）。
> 例えば `HPGe-count` はエラー、`HPGe_count` は OK。
> 解析側で `chain.SetBranchAddress("HPGe_count", &cnt)` で読めます。
>
> **flag / accept の値は ROOT に書きません**。`accept` が true だったかは
> 「そのイベントが ROOT に存在するか」と等価（drop されたら書かれない）、
> `flag` は `counter + value` 比較で解析側で再現可能なので冗長。

### 6.5 ログを読む

`event_builder build` の末尾に下のようなサマリが出ます:

```
Event building complete  hits=8425960061 events_built=2034586877 events_kept=12392064 files=14
```

| key | 意味 |
|---|---|
| `hits` | EB に入力された total hit 数 |
| `events_built` | L1 で trigger anchor として認識された数 (= L2 通過前) |
| `events_kept` | L2 を通過して ROOT に書かれた event 数 |
| `files` | ROOT ファイル数 |

`events_built / hits` が極端に低い → L1 trigger 設定 / 時間較正の問題
`events_kept / events_built` が極端に低い → L2 が厳しすぎ

---

## 7. 解析

### 7.1 macro テンプレート

`tools/p91zr_test/analyse_si_e_de.C` を参考に。基本パターン:

```cpp
// my_analysis.C
{
  TChain ch("EventTree");
  ch.Add("./work/eb_output/eb_run0042_*.root");

  ULong64_t event_id;
  Double_t  trigger_time;
  UChar_t   trigger_mod, trigger_ch;
  UInt_t    mult;
  std::vector<unsigned char>*  mods    = nullptr;
  std::vector<unsigned char>*  chs     = nullptr;
  std::vector<unsigned short>* energy  = nullptr;
  std::vector<double>*         rel_t   = nullptr;

  ch.SetBranchAddress("EventID",      &event_id);
  ch.SetBranchAddress("TriggerTime",  &trigger_time);
  ch.SetBranchAddress("TriggerMod",   &trigger_mod);
  ch.SetBranchAddress("TriggerCh",    &trigger_ch);
  ch.SetBranchAddress("Multiplicity", &mult);
  ch.SetBranchAddress("Mod",          &mods);
  ch.SetBranchAddress("Ch",           &chs);
  ch.SetBranchAddress("Energy",       &energy);
  ch.SetBranchAddress("RelTime",      &rel_t);

  // L2 counter ops の値も branch として出力されている（eb_config.json の
  // counter `name` がそのまま branch 名）。例えば `HPGe_count` をイベント
  // フィルタに使う:
  Long64_t hpge_count = 0;
  ch.SetBranchAddress("HPGe_count",   &hpge_count);

  TH1F* h_mult = new TH1F("h_mult", ";Multiplicity;count", 32, -0.5, 31.5);
  // ...

  Long64_t n = ch.GetEntries();
  for (Long64_t i = 0; i < n; ++i) {
    ch.GetEntry(i);
    if (hpge_count < 2) continue;  // 解析側で counter を使った cut
    h_mult->Fill(mult);
    // 解析側でかける物理 cut（calibration / energy threshold / PID 等）
    // ...
  }

  TFile out("analysis.root", "RECREATE");
  h_mult->Write();
  out.Close();
}
```

実行:

```bash
root -l -b -q my_analysis.C
```

### 7.2 Python (uproot) を使う場合

```python
import uproot
import numpy as np

with uproot.open("./work/eb_output/eb_run0042_0000_events.root") as f:
    tree = f["EventTree"]
    # Lazy iteration over chunks
    for batch in tree.iterate(
        ["EventID", "TriggerMod", "TriggerCh", "Mod", "Ch", "Energy", "RelTime"],
        step_size=100_000,
    ):
        ...
```

### 7.3 calibrate された energy が欲しい時

chSettings.json から `p0..p3` を読み込み、解析側で:

```cpp
double e_kev = p0 + p1 * adc + p2 * adc * adc + p3 * adc * adc * adc;
```

`tools/p91zr_test/analyse_si_e_de.C` の `Calib` 構造体と `load_calib`
関数が JSON loader として使えます (nlohmann/json 必要)。

---

## 8. トラブルシューティング

### 8.1 `events_kept = 0`

考えられる原因（チェック順）:

1. **L1 trigger channel が hit していない** —
   `event_dump <file> --summary` で hit が trigger channel に来ているか確認
2. **L2 が厳しすぎる** —
   L2 ops を一時的に `[{"type": "accept", "name": "all", "monitor": [], "operator": "OR"}]` のような
   "何もしない" 構成にして events_built > 0 か確認
3. **時間較正のズレ** — `--time-calib` を外して再走、改善するなら timeSettings.json が悪い

### 8.2 `events_kept` が小さすぎる (< 期待値の 1%)

- **時間較正未適用**: 経験的に最も多いケース。p91Zr では未較正で 643k → 較正で 12.4M (×19) に増えた
- **coincidence_window_ns が狭すぎる**: 100 ns → 500 ns に広げて様子を見る
- **L2 の `min_hits` や `ac_veto`** が想定より厳しく作動している

### 8.3 EB が遅い

- `--workers`, `--writers` を CPU コア数に合わせて増やす
- `--events-per-file` を増やすと ROOT 出力のオーバーヘッドが減る
- ROOT 入力 (`--root-input`) は .delila より遅い

### 8.4 「L1 op `X` references unknown name `Y`」

eb_config.json の name 参照が無効。typo か、参照先 op が定義より後に
書かれている。typo を直すか、依存関係順に並べ替える。

### 8.5 「L1 op `X`: nested ... not yet implemented」

`multiplicity` / `and` の `channels` / `inputs` には **leaf `channel` op の名前のみ**
指定可能。`or` ネストや multiplicity ネストは未サポート（SPEC § 6.2）。

---

## 9. クイックリファレンス

### よく使うコマンド

```bash
# ビルド
cargo build --release --features root --bin event_builder
cargo build --release --bin eb_offsets

# 設定ファイル検証
./target/release/event_builder validate-config ./work/eb_config.json
./target/release/event_builder validate-config ./work/chSettings.json
./target/release/event_builder validate-config ./work/timeSettings.json

# 時間較正
./target/release/event_builder time-calib \
    -i ./data/run0042_*.delila \
    -o ./work/timeSettings.json \
    --ref-module 0 --ref-channel 0 \
    --hist-output ./work/timeAlignment.root

# オフセット確認
./target/release/eb_offsets ./work/timeSettings.json --sort abs

# イベントビルド (推奨形)
./target/release/event_builder build \
    -i ./data/run0042_*.delila \
    -o ./work/eb_output \
    -c ./work/chSettings.json \
    --eb-config ./work/eb_config.json \
    -T ./work/timeSettings.json \
    --run-id 42

# 解析 (ROOT macro)
root -l -b -q my_analysis.C
```

### 関連ドキュメント

- [`TODO/event-builder/SPECIFICATION.md`](../TODO/event-builder/SPECIFICATION.md) — EB 仕様 (v0.7)
- [`tools/p91zr_test/`](../tools/p91zr_test/) — p91Zr 実データでの E2E テスト
  - `convert_elifant_config.py` — ELIFANT-Event 形式 → 我々の形式 変換
  - `convert_elifant_timesettings.py` — ELIFANT の 4D timeSettings → tree 形式 変換
  - `analyse_si_e_de.C` — ROOT 解析マクロ例（calibration / 同 sector pairing / mult==1）
  - `README.md` — 3 段階のクリーンネスヒエラルキー（naive / anti-diag / mult==1）の説明
- `docs/event_builder_design.md` — 設計ドキュメント

---

## 付録 A. 最小構成サンプル

### A.1 ディレクトリ構成

```
work/
├── chSettings.json
├── eb_config.json
├── timeSettings.json      # time-calib で生成
├── timeAlignment.root     # time-calib の診断ヒストグラム
└── eb_output/
    └── eb_run0042_0000_events.root
```

### A.2 最小 chSettings.json（HPGe 1 ch + dE 1 ch）

```json
[
  [
    {"Module": 0, "Channel": 0, "DetectorType": "HPGe",
     "Tags": ["HPGe"], "p0": 0.0, "p1": 1.0, "p2": 0.0, "p3": 0.0},
    {"Module": 0, "Channel": 1, "DetectorType": "Si",
     "Tags": ["dE"], "p0": 0.0, "p1": 1.0, "p2": 0.0, "p3": 0.0}
  ]
]
```

### A.3 最小 eb_config.json（HPGe trigger、dE coincidence 要求）

```json
{
  "version": "1.0",
  "timing": {
    "coincidence_window_ns": 200.0,
    "buffer_delay_ns": 1.0e9,
    "slice_duration_ns": 1.0e7
  },
  "channels_file": "chSettings.json",
  "time_offsets_file": "timeSettings.json",
  "l1": {
    "definitions": [
      {"type": "channel", "name": "HPGe0", "module": 0, "channel": 0}
    ],
    "trigger": "HPGe0"
  },
  "l2": [
    {"type": "counter", "name": "dE_count",  "tags": ["dE"]},
    {"type": "flag",    "name": "has_dE",    "monitor": "dE_count",
     "operator": ">",   "value": 0},
    {"type": "accept",  "name": "keep",
     "monitor": ["has_dE"], "operator": "AND"}
  ],
  "output": {
    "events_per_file": 1000000,
    "directory": "./eb_output",
    "zmq_pub_endpoint": null
  }
}
```

---

## 付録 B. 設計原則の再確認 (SPEC § 1.4 抜粋)

EB は「物理屋が解析しやすい時間整列済みデータを最大情報量で提供する」
汎用エンジンです。**実験固有の物理 cut は EB に入れず解析側で行う**こと:

- ✓ EB OK: 時間整列、coincidence window、tag ベースの汎用 filter
- ✗ EB NG: 検出器配置依存のペアリング (anti-diagonal 等)、kinematic cut、
   calibration 依存閾値、粒子種仮定が必要な cut、multiplicity-conditional pairing

これらを EB に入れると **データ消失が永続化** し、新仮説で再解析する自由を
失います。詳細は SPEC § 1.4 を参照してください。
