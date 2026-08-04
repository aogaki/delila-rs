# root_sink 使用マニュアル

`root_sink` は DELILA パイプラインに**並列接続する ROOT シンク**である。
1 プロセスで 2 役をこなす:

1. **スカラー ROOT Recorder** — 全イベントを 5 フィールド
   (module / channel / energy / energy_short / timestamp_ns)のフラット TTree に記録。
   `.delila` → `delila2root` の 2 段変換が不要になる。
2. **簡易ライブモニタ** — コインシデンス Δt・エネルギー等のヒストグラムを
   THttpServer(JSROOT)でブラウザ配信。フロントエンド実装ゼロ。

既存の Recorder(`.delila` 主記録)には一切手を触れない。merger の PUB を
追加購読するだけなので、途中で kill してもデータ保全に影響はない。

```
Reader → Merger ─PUB(tcp://*:5557)─→ Recorder (.delila 主記録)
                          ├────────→ Monitor(既存 Web モニタ)
                          └────────→ root_sink ─→ run%04u_0000_<exp>.root
                                        └→ THttpServer :8090(JSROOT ライブ表示)
```

ビルド手順・開発者向け情報は [tools/root_sink/README.md](../tools/root_sink/README.md)
(英語)を参照。本マニュアルは運用に絞る。

---

## 前提条件

- **DAQ スタックが 2026-07-21 以降の master であること(重要)。**
  それ以前の merger は Stop 時に EndOfStream を破棄するバグがあり
  (commit `dc0669c` で修正)、root_sink がラン終了を検知できず
  ファイルが永久に `run_inprogress_*.root` のまま残る。
  症状が出たら真っ先に merger のビルド日を疑うこと。
- バイナリ配備済みホスト: **side3** (`~daq/.local/bin/root_sink`)、
  **gant** (`~/.local/bin/root_sink`)。
- `~/.local/bin` 作成前から開いているシェルは PATH に入っていない。
  見つからない場合は新しいシェルを開くか絶対パスで実行する。

---

## クイックスタート

### config TOML からの起動(推奨)

DAQ config に `[network.root_sink]` セクションを置くと、`scripts/start_daq.sh`
が他のコンポーネントと一緒に root_sink を自動起動する(online event builder と
同じ扱い)。

```toml
[network.root_sink]
subscribe  = "tcp://localhost:5557"
output_dir = "/home/daq/rootsink_data"
http_port  = 8090
hists      = "/home/daq/rootsink_data/histograms.json"
gamma_ch   = 3
thgem1_ch  = 7
thgem2_ch  = 11
window_ns  = 10000
```

- セクションがあれば `start_daq.sh` が自動起動、無ければ従来どおり(手動起動)。
- `--operator` は `[operator]` の `port` から自動導出される(明示不要)。
- キー名は CLI フラグと同じ(ハイフン→アンダースコア。例 `--out-dir` →
  `output_dir`)。指定したキーだけがフラグになり、残りは root_sink の既定値。
- バイナリ探索順: `ROOT_SINK_BIN`(環境変数) > PATH > `~/.local/bin/root_sink`
  > `tools/root_sink/root_sink`。見つからなければ黄色い警告を出してスキップ
  (致命ではない)。
- **side3 は反映済み**(2026-07-21、`~daq/delila-rs/config.toml` に上記セクション追記済み。
  `start_daq.sh` からの自動起動とファイル名一致を run 14 で実機検証済み)。
  外したい場合はセクションを削除またはコメントアウトするだけ。

### 手動起動(side3 の実例)

```bash
nohup ~/.local/bin/root_sink \
  --zmq tcp://localhost:5557 \
  --out-dir ~/rootsink_data \
  --operator http://localhost:9092 \
  --hists ~/rootsink_data/histograms.json \
  --gamma-ch 3 --thgem1-ch 7 --thgem2-ch 11 \
  --window-ns 10000 \
  --http-port 8090 \
  > ~/rootsink_data/sink.log 2>&1 &
```

- 起動はいつでもよい(ラン中でも可。次の Data から記録が始まる)。
- **常駐型**: 複数ランをまたいで走らせたままにする。ラン毎の再起動は不要。
- 停止は `kill <PID>`(SIGTERM/Ctrl-C とも安全にクローズする)。
- あとは通常どおり operator(Web UI or REST)で Configure → Start → Stop
  するだけ。root_sink 側の操作は何もいらない。

ブラウザで **http://<ホスト>:8090/** を開くとヒストグラムがライブで見える。

### DAQ 再起動との関係

root_sink は `start_daq.sh` / `stop_daq.sh` の管理対象になった
(いずれも `pkill -x root_sink` で正確なプロセス名一致でkillする)。

- **手動起動した sink も `start_daq.sh` 実行時に殺される**点に注意。
- `[network.root_sink]` セクションが**無い** config で `start_daq.sh` を走らせた
  場合も、既存の root_sink は殺される(常駐させたい場合は `start_daq.sh` の後に
  再度手動起動する)。
- merger が再起動しても root_sink は ZMQ を自動再接続する。逆に root_sink だけを
  再起動しても DAQ 本体には影響しない。

---

## CLI リファレンス

| フラグ | 既定値 | 説明 |
|---|---|---|
| `--zmq ADDR` | `tcp://localhost:5557` | merger PUB エンドポイント |
| `--out-dir DIR` | `.` | `.root` 出力ディレクトリ |
| `--tree NAME` | `delila` | TTree 名(delila2root と同じ既定値 — マクロを共用可能) |
| `--exp-name NAME` | (なし) | ファイル名の実験名を明示指定(最優先) |
| `--operator URL` | (なし) | 例 `http://localhost:9092`。ラン開始時に `/api/status` の `experiment_name` を取得 |
| `--hists FILE` | (なし) | ヒストグラム定義 JSON(後述)。未指定ならビルトイン 4 種 |
| `--gamma-ch N` | −1 | ガンマ線検出器チャンネル |
| `--thgem1-ch N` | −1 | ThGEM1 チャンネル |
| `--thgem2-ch N` | −1 | ThGEM2 チャンネル(3 つ全て指定でマッチャ有効。省略時は recorder 専用) |
| `--xy1-ch T,XL,XR,YU,YD` | (なし) | ディレイライン XY モニタ 1: トリガー + 4 腕のチャンネル(相異なる 5 整数のカンマ区切り)。**`--hists` 必須**(pos1 スコープのヒストにのみ表示) |
| `--xy2-ch T,XL,XR,YU,YD` | (なし) | ディレイライン XY モニタ 2(pos2 スコープ) |
| `--window-ns X` | 1000 | コインシデンス半窓 ±W [ns](Δt/XY マッチャ共通) |
| `--margin-ns X` | 10000 | 到着順の乱れ許容(熟成遅延)[ns](Δt/XY マッチャ共通)。**ストリームはバッチ内ソートのみ**で、バッチ跨ぎの乱れは ms 級になり得る(side3 run0023 実測で最大 8.2 ms)→ 実運用では `20000000`(20 ms)推奨 |
| `--http-port N` | 8090 | THttpServer ポート。0 で無効 |
| `--dt-bins/--dt-min/--dt-max` | 2000 / −1000 / +1000 | ビルトイン Δt ヒストの軸(`--hists` 使用時は無視) |
| `--autosave-sec N` | 30 | TTree AutoSave 間隔(書きかけファイルも ROOT で開ける) |

---

## 出力ファイルと命名規則

| 状態 | ファイル名 |
|---|---|
| ラン中(暫定) | `run_inprogress_<unixtime>.root` |
| ラン終了(確定) | `run%04u_0000_<exp>.root` |
| ヒスト設定の控え | `run%04u_0000_<exp>_hists.json`(`--hists` 使用時) |

- 確定名は **Rust Recorder の `.delila` と拡張子以外完全一致**
  (例: `run0013_0000_X730_ThGEM_Test.delila` ↔ 同 `.root`)。
  root_sink 自身は分割しないので連番部は通常 `0000`。
  名前衝突時は Recorder と同じく `_<unix ナノ秒>` を付加する。
- **ROOT の自動分割への備え**: ROOT は TTree のファイルが `MaxTreeSize` を超えると
  勝手に次ファイルへ切り替える。root_sink はこの閾値を **2 TB** に設定しており
  (スカラーデータでは ~10^11 イベント相当、実質未到達)、万一超えた場合も
  WARNING を出した上で分割パートを `run%04u_0001_<exp>.root`, `_0002_`… と
  **Recorder と同じ連番規則**でリネームする(縮小閾値ビルドで E2E 検証済み。
  全パートのエントリ合計 = Recorder のイベント数を確認)。
- リネームは **EndOfStream 受信時**(= operator の Stop)に行われる。
  run 番号も EOS が運んでくるため operator への依存はない。
- 実験名 `<exp>` の解決順:
  1. `--exp-name`(明示指定、常に勝つ)
  2. `--operator` → `/api/status` の `experiment_name`
     (Web UI 運用ではこれが Recorder のファイル名と必ず一致する。**推奨**)
  3. どちらも無い/取得失敗 → `"data"`(stderr に警告が出る)
- ラン中に kill した場合、書きかけは `run_inprogress_*.root` のまま残る
  (未完了ランを完成品に見せないための仕様)。AutoSave 済みなので
  ROOT で開いて中身を確認できる。不要なら手で消してよい。

### TTree 構造

| branch | 型 | 内容 |
|---|---|---|
| `module` | `b` (UChar_t) | モジュール(digitizer)番号 |
| `channel` | `b` (UChar_t) | チャンネル |
| `energy` | `s` (UShort_t) | エネルギー |
| `energy_short` | `s` (UShort_t) | ショートゲート(PSD) |
| `timestamp_ns` | `D` (Double_t) | タイムスタンプ [ns] |

```cpp
// 解析例
TFile f("run0013_0000_X730_ThGEM_Test.root");
TTree* tr = (TTree*)f.Get("delila");
tr->Draw("energy", "channel==3");
```

---

## ライブモニタ(JSROOT)

- **http://<ホスト>:8090/** を開く。左のツリーからヒストグラムをクリック。
- ページ全体が **2 秒ごとに自動更新**される(`_monitoring` 既定値)。
- 2D は自動で `colz` 描画。
- **表示プリセットの URL ブックマーク**(4 分割+自動更新):

  ```
  http://192.168.144.102:8090/?items=[dt1,dt2,dt2_vs_dt1,channels]&layout=grid2x2&monitoring=2000
  ```

- **/Reset** ボタン(または `curl 'http://<ホスト>:8090/Reset/cmd.json'`)で
  全ヒストをゼロクリア。ヒストは**ラン境界で自動クリアされない**
  (ラン跨ぎで積算する)設計なので、新しい測定条件の前に手動で Reset する。
- コインシデンスの時刻状態だけはラン毎に自動リセットされる
  (クロック巻き戻り対策)。

---

## ヒストグラム定義ファイル(`--hists`)

再コンパイルなしでヒストグラムの種類・ビン・レンジ・カットを変えられる。
標準ファイル `tools/root_sink/histograms.json` はビルトインと同じ 4 ヒスト
(dt1 / dt2 / dt2_vs_dt1 / channels)を定義している。

### フォーマット

```json
{ "histograms": [
  { "name": "dt1", "type": "TH1D", "fill": "dt1",
    "bins": 2000, "min": -1000, "max": 1000 },
  { "name": "E_vs_dt1", "type": "TH2D", "x": "dt1", "y": "gamma_energy",
    "xbins": 400, "xmin": -1000, "xmax": 1000,
    "ybins": 512, "ymin": 0, "ymax": 16384 }
]}
```

- 共通キー: `name`(必須・一意)、`type`(`TH1D`|`TH2D`)、`title`(省略時 name、
  `"タイトル;X軸;Y軸"` 形式可)、`drawopt`(省略時 2D=colz)。
- 1D は `fill`(=`x` の別名)+ `bins/min/max`。2D は `x`,`y` +
  `xbins/xmin/xmax/ybins/ymin/ymax`。

### 変数語彙(固定。式エンジンはない)

| スコープ | 変数 | カット |
|---|---|---|
| **hit**(全イベント) | `energy` `energy_short` `channel` `module` | `"channel": N`、`"energy_range": [lo,hi]` |
| **coinc**(熟成したガンマ毎) | `dt1` `dt2` `gamma_energy` `thgem1_energy` `thgem2_energy` | `"gamma_energy_range"` / `"thgem1_energy_range"` / `"thgem2_energy_range": [lo,hi]` |
| **pos1**(XY モニタ 1 のトリガー毎) | `xy1_x` `xy1_y` | (なし) |
| **pos2**(XY モニタ 2 のトリガー毎) | `xy2_x` `xy2_y` | (なし) |

- 1 つのヒスト内で **異なるスコープの変数・カットを混ぜることはできない**
  (起動/リロード時に検証エラーになる。pos1 と pos2 の混在も不可)。
- `dt1`/`thgem1_energy` は ThGEM1 パートナーが見つかったガンマのみ、
  `dt2`/`thgem2_energy` は ThGEM2 側のみ Fill される。
- coinc スコープを使うには `--gamma-ch/--thgem1-ch/--thgem2-ch` が必要
  (無いと起動時に警告が出て、該当ヒストは空のまま)。
- `xy1_x = t(XR) − t(XL)` は **XL/XR 両方**がトリガーと ±window 内でマッチした
  ときのみ、`xy1_y = t(YU) − t(YD)` は YU/YD 両方のときのみ Fill される
  (2D の XY 像は実質 4 腕コインシデンス要求。1D 射影は自分の腕ペアだけで埋まる)。
- pos1/pos2 スコープを使うには `--xy1-ch`/`--xy2-ch` が必要(無いと起動時警告+空のまま)。
  XY モニタにはビルトインの代替ヒストが無いため、`--hists` なしでは動かない。

### 実用例

```json
{ "name": "E_gamma", "type": "TH1D", "x": "energy", "channel": 3,
  "xbins": 4096, "xmin": 0, "xmax": 16384 }
```
```json
{ "name": "dt1_gated", "type": "TH1D", "fill": "dt1",
  "bins": 400, "min": -200, "max": 200,
  "gamma_energy_range": [800, 1200] }
```

ディレイライン XY 像(`--xy1-ch 1,2,3,4,5` = trig,XL,XR,YU,YD の場合。
軸レンジは検出器の物理的広がりに合わせる — ThGEM 実測は ±250 ns):

```json
{ "name": "xy1_pos", "type": "TH2D",
  "title": "ThGEM1 XY;x = t_{XR} - t_{XL} [ns];y = t_{YU} - t_{YD} [ns]",
  "x": "xy1_x", "y": "xy1_y",
  "xbins": 500, "xmin": -250, "xmax": 250,
  "ybins": 500, "ymin": -250, "ymax": 250 }
```

### ライブ再読込(/ReloadHists)

1. JSON ファイルを編集(ヒスト追加・ビン変更など何でも)。
2. ブラウザの **/ReloadHists** ボタン、または
   `curl 'http://<ホスト>:8090/ReloadHists/cmd.json'`。
3. 成功: 旧ヒストは破棄され新セットが即 live になる(**積算はリセットされる**)。
   失敗: 全エラーが sink.log に出て、**旧セットがそのまま生き残る**
   (ラン中に書き損じても表示が消えない)。

ラン確定時、その時点の JSON が `run%04u_0000_<exp>_hists.json` として
`.root` の隣にコピーされる(どの設定で取ったかの再現性)。

---

## トラブルシューティング

| 症状 | 原因と対処 |
|---|---|
| Stop してもファイルが `run_inprogress` のまま | EOS が届いていない。①merger が 2026-07-21 より古い(`dc0669c` 未適用)→ スタック更新 ②sink 起動が Stop より後だった → 次ランから正常 |
| ファイル名が `run%04u_0000_data.root` になる | exp_name 未解決。`--operator` を付ける(sink.log に `experiment_name = ...` と取得元が出る)|
| `.delila` と `.root` で実験名が違う | curl 手打ちで Configure に UI と違う `exp_name` を送った場合に起きる。UI 運用なら一致する。強制したいなら `--exp-name` |
| coinc ヒストが空 | `--gamma-ch/--thgem1-ch/--thgem2-ch` 未指定(起動ログに警告)、またはチャンネル割当違い |
| XY ヒスト(pos1/pos2)が空 | `--xy1-ch/--xy2-ch` 未指定 or `--hists` 無し(どちらも起動ログに警告)、チャンネル割当違い、または片腕チャンネルが欠けている(2D は 4 腕全てのマッチ必須) |
| Δt/XY のコインシデンス効率が低い | `--margin-ns` がストリームの乱れより小さい(遅れて届いた相方が熟成に間に合わない)。20 ms に上げる。**分布の形は正しいのに数だけ少ない**のが特徴 |
| `/ReloadHists` しても変わらない | JSON にエラーがある。`sink.log` に全エラーが列挙され旧セット維持になっている |
| イベント数が Recorder と合わない | ラン途中から購読した/途中で kill した場合は当然合わない。フルランで一致しないなら異常(gant/side3 検証では 3 ラン連続で完全一致) |
| ssh 切断で sink が死ぬ | 旧バイナリ。`gROOT->SetBatch(kTRUE)`(`6f65ef1`)以降は起きない。`nohup` 併用を推奨 |
| ブラウザで開けない | `--http-port 0` で起動していないか、FW でポートが塞がれていないか確認 |
| root_sink が起動しない | `start_daq.sh` の summary table で **DEAD**、または `binary not found` 警告が出る → バイナリ未配備か、ROOT 実行環境(`LD_LIBRARY_PATH`)が無い |

ログの正常パターン(sink.log):

```
root_sink: experiment_name = "X730_ThGEM_Test" (source: http://localhost:9092/api/status)
root_sink: run started (source 0) -> .../run_inprogress_1784618933.root
root_sink: WRITING | events=524136 | 30236 ev/s | matcher_fills=174711
root_sink: run 13 finalized -> .../run0013_0000_X730_ThGEM_Test.root (755250 events)
root_sink: histogram config copied -> .../run0013_0000_X730_ThGEM_Test_hists.json
```

---

## 制限事項(仕様)

- 自発的なファイル分割なし(連番は通常 `0000`)。Recorder が分割した場合でも
  `.root` は 1 ファイルに全量が入る。例外は ROOT 自身の `MaxTreeSize`(2 TB)
  超過時のみで、その場合は `_0001`… と Recorder 準拠の連番になる(上記参照)。
- ラン境界は EOS 頼み。EOS を出さない上流(古いスタック)とは組み合わせ不可。
- モニタヒストは表示専用。記録の正はあくまで TTree / `.delila`。
- 波形は扱わない(スカラー 5 フィールドのみ)。波形が要る場合は従来どおり
  `.delila` + `delila2root`。
