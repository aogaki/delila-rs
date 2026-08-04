# TODO 66: Event Builder の C++ 移行 (root_sink 系譜)

**Created:** 2026-08-04
**Status:** 🚧 IN PROGRESS — **Phase 1 実装完了 (2026-08-04、Mac ローカル検証済み)**。
side3 配備は**ユーザー出張のため凍結中**(帰還後に scp+md5 手順で実施)
**Priority:** 2
**関連:** [SPECIFICATION.md](SPECIFICATION.md) (要求仕様として引き続き有効) /
`tools/root_sink/` / [`docs/root_sink_manual.md`](../../docs/root_sink_manual.md)

---

## 1. 決定

**イベントビルダーの実装言語を C++(root_sink 系譜)に移す。**
Rust 統一パイプライン(`src/event_builder/`)は凍結し、参照実装として維持する。

これは SPEC 変更履歴上 3 度目の言語判断である:
- v0.3 (2026-01): Event Bridge + 別リポジトリ C++ EB 案
- v0.5 (2026-05): C++ 経路を撤回、Rust 側で完結(Bridge retire)
- **v0.8 (2026-08): C++ へ再転換 — ただし別リポジトリ/Bridge ではなく、
  delila-rs 内 `tools/` の root_sink 系譜(merger PUB 直接購読、TDelila ベース)**

## 2. 根拠 (2026-08-04 時点のエビデンス)

1. **oxyroot は ROOT 書き込み圧縮が未実装**(調査 2026-08-04 確定):
   `rcompress.rs::compress()` が `assert_eq!(compression, 1)` + `unimplemented!()`
   で常に非圧縮。実測: 同一 2M イベントで oxyroot 28.1 MB vs C++ ZSTD-5 18.1 MB。
   最新 0.1.25(2024-10)以降休眠、master も同一。**現行 Rust EB の ROOT 出力も
   非圧縮で書いている**。既知の wbasket 1000-entry パニック(delila2root C++ 化の
   原因)と合わせ、oxyroot の書き込み経路は本番非適格
2. **root_sink の実証**: side3 ThGEM で PositionMatcher(watermark 方式)を
   実装→テスト→配備→物理結果(位置分解能 0.25 mm)まで 1 日で完走。
   run0023 実データ 909,812 ヒットで offline 解析と 4-fold 15,644 完全一致
3. **online = offline 同一コード**: TDelila は ZMQ ワイヤも `.delila` ファイルも
   読める → 同じビルダーをライブとリプレイの両方で実行可能(Rust 側は
   replay staging stack が別途必要だった)
4. **EB は実験固有の物理コード**: コインシデンス定義・built-event スキーマは
   物理屋が高速に反復する場所であり、C++/ROOT が適切な住処
5. **性能は律速しない**: 重い処理(decode/serialize)は上流 Reader/Merger(Rust)
   で完了済み。EB の仕事は窓マッチング + Fill のみで、スカラー 14 B/ev なら
   1 M ev/s でも 14 MB/s — C++ シングルスレッドで十分

## 3. 変えないこと (絶対条件)

1. **`.delila`(Recorder)が正典のまま**。C++ EB の出力は導出データであり、
   いつでも再構築できる。データ保全(HWM=0 / no-drop)は Rust 取得系の責務のまま
2. **フレームワーク化しない**(KISS)。実験ごとに builder 1 本
   (`tools/<exp>_builder/` or root_sink 拡張)+ 共有ヘッダ
   (`TDelila.hpp` + `sink_core.hpp` 系)
3. SPEC の概念(§1.4 責務境界、L1/L2 named-ops、2 層 threshold、
   timeSettings tree)は**要求仕様として C++ 実装に引き継ぐ**

## 4. フェーズ計画

### Phase 1: マルチスレッド骨格 + ThGEM built tree — ✅ **完了 (2026-08-04)**
§5 の設計どおり 3 段で実装した(コミット列 `974b3bb`→`0d7f4b1`→`aaa11dc`→
`7381ad8`→`7ec50f0`+docs)。**side3 未配備**(出張凍結、Mac 検証のみ):

1. ✅ **`eb_core.hpp` + `test_eb_core.cpp`**(158 checks、素の g++ -pthread):
   Channel / SeqReorder / SortedChunk+split(H10 移植)/ Sorter / 統合ビルダー。
   追加の設計決定 = sort_and_split は**進捗ゼロ分割を拒否**(core_end <=
   core_start で false — Rust 版は呼び出し側の span 条件が偶然守っていた
   不変条件をプリミティブに内蔵)
2. ✅ **root_sink スレッド化**(`7381ad8`): 挙動保存を V0 ベースライン比較で実証
   - V2: run0023 replay 909,812 hits、ch 別カウント/Σenergy/ts XOR 完全一致
     + **hits tree が全体時刻ソート済み化**(compare_hits.C をリポジトリに追加)
   - V3: workers 1 vs 4 で順序込み FNV hash 完全一致(決定的出力)
   - V5: 2 ラン再生両方フル(R11 kill)/ stale EOS 無視 / mid-run SIGTERM で
     provisional 維持+テール全量到達
   - V6: rollover 23 パート、合計 909,812 / V7: 負荷中 /Reset・/ReloadHists
     正常、**/Files 非公開確認**(R1 = TRootSniffer::SetScanGlobalDir(kFALSE))
   - V8: RSS 276 MB 一定、271k ev/s でキュー深さ 0
   - E2E 基盤として **test_publisher.cxx を恒久コミット**(--repeat/--no-eos/--rate)
3. ✅ **built tree**(`7ec50f0`): `--built-tree NAME`。
   **V4: n_arms1==4 が 15,644 — オフライン正解(tmp/test.cpp)と完全一致**。
   トリガーアンカー全 40,045 行(partial 保持=腕効率)、rollover 全パートに
   両 tree(R6 実証)。V3/V6 built 込み再合格

当初案の「watermark matcher の N デック汎用化」は**やらない** —
Sorter 導入で EB 経路は純関数ビルダーに置き換わるため(§5.1)。
既存 watermark matcher(CoincidenceMatcher / PositionMatcher)は
軽量モニタ用として sink_core.hpp に残し、移行完了後に整理を判断。

### Phase 2: 汎用化
- コインシデンス設定を hists と同じ**リロード可能な JSON** に
  (Mongo/Web UI 依存を落とす)
- `.delila` リプレイモード(TDelila ファイル入力で同一ビルダーを実行)
- **H10 相当の境界テストを必ず移植**: チャンク/watermark 境界を跨ぐ
  コインシデンスの取り逃し・二重発火(Rust EB で 2026-07-09 に踏んだ罠。
  watermark 方式は原理的に同じ罠を持ち得る)

### Phase 3: ELIADE 適用判断
- clover addback / multiplicity / L2 相当の要件を C++ 側で満たせるか評価
- 満たせたら Rust EB 退役。**唯一の再考条件 = 波形レベルの情報を
  イベントビルドで使う要件が出た場合**(そのときは Rust 並列パイプラインを再評価)

## 5. マルチスレッド設計 (2026-08-04 ブレインストーム確定)

### 5.0 スレッドトポロジー

```
[Receiver]      [Sorter]                [Worker Pool ×N]         [Writer]
 ZMQ recv   →   accumulate + sort   →   event build (純関数)  →   TFile/TTree 専有
 + decode       + safe-horizon cut      chunk 単位・並列          seq で順序復元
 (馬鹿に保つ)   (watermark はここだけ)          │
                                               └──(tee, bounded)──→ [Display]
                                                                     TH1 専有 + THttpServer
```

Rust EB v2(Receiver→Sorter→Workers→Writer)および parallel_decode
(Dispatcher→workers→Collector/ReorderBuffer)で二度検証済みの形の C++ 移植。

### 5.1 Sorter 導入で watermark matcher が消える(中心決定)

deque + ripen + prune の複雑さは「未ソートストリームを単一スレッドで舐める」
ための対価だった。Sorter が sort + safe-horizon cut を一手に引き受けると
下流は**完全ソート済み世界**になり、イベントビルドは `lower_bound` 窓探索の
**純関数**になる — `macros/grid_resolution.C` / オフライン解析と同一アルゴリズム。
「online = offline 同一コード」が入力層(TDelila)だけでなく
ビルドアルゴリズム層でも成立する。

- Sorter の emit 判断は**データ内時刻**(watermark 前進)であって件数や
  wall clock ではない: `core_end = max_ts − safe_horizon` が `chunk_span` 分
  進んだら sort → split → emit、tail は buffer に残す
- チャンク境界(H10 の罠): chunk は tail を含んで渡し、trigger 発火は
  `ts < core_end` のみ emit。safe_horizon ≫ coincidence window で取り逃しなし
- デフォルト: **chunk_span = 100 ms / safe_horizon = 50 ms**
  (side3 実測乱れ 8.2 ms に対し余裕)

### 5.2 BuiltEvent: completeness は cut であって build 条件ではない

emit 条件は「トリガー ch にヒットがある」のみ。腕は全てオプショナル
(欠損 = NaN + has_* フラグ)。4-fold は tree に書く条件ではなく
オフラインの cut(`!isnan(x) && !isnan(y)`)。

- display は量ごとに判定: dt ヒストは partial からも fill、XY は完全時のみ
- partial / trigger-only イベントは**捨てない** — `has_x` の割合がそのまま
  腕の検出効率(ThGEM 効率マップがオフラインでタダで出る。捨てると測れない)
- NaN ブランチは ZSTD が実質タダで潰す

```cpp
struct BuiltEvent {
  double   trig_t;
  uint16_t e_trig;
  float    x, y;            // 欠損は NaN(XL&&XR / YU&&YD 揃った時のみ値)
  float    dt_labr;         // 同様に NaN 可
  uint16_t e_labr, e_sum;
  uint8_t  nhits;           // 窓内で拾った腕の数
};
```

### 5.3 順序と決定性

chunk に seq を振り Writer 側で順序復元(parallel_decode Collector と同パターン)。
→ **worker 数 N に依らず出力 TTree のエントリ列が完全一致**。
回帰テストは「同一入力 → 同一 tree」を N=1 と N=4 の両方で回して差分ゼロ確認。
副産物: **hits tree が全体時刻ソート済みになる**(現状の「バッチ内のみソート」
制約が解消し、オフライン解析が素直な線形スキャンで書ける)。

### 5.4 キューと no-drop ポリシー

- **record 経路**(Receiver→Sorter→Workers→Writer): unbounded。
  `std::mutex` + `condition_variable` + `std::deque` テンプレで十分
  (粒度が chunk なのでキュー操作は毎秒数十回、lock-free はオーバーキル)。
  `std::move` でコピーゼロ
- **display 経路**: bounded(1000) + ドロップ計上。
  **CLAUDE.md の Monitor 例外と同種として本決定で明文化**: 記録経路は
  built tree + `.delila` が独立に持ち、表示専用パスのドロップは
  カウント + ログ可視化を条件に許容する

### 5.5 ROOT スレッド安全性の配置

- main で `ROOT::EnableThreadSafety()`、以後は**専有で守る**(ロック不要):
  TFile/TTree = Writer のみ、TH1 群 + THttpServer(`ProcessEvents()` 駆動)=
  Display のみ
- Writer の圧縮が律速したら `ROOT::EnableImplicitMT()`(basket 圧縮の
  ROOT 内部並列化)— 自前圧縮スレッドは作らない
- 出力は現行どおり単一ファイル + `SetMaxTreeSize`(file-per-batch + hadd は
  不採用: 正典は `.delila` なのでクラッシュ耐性の理由が既にない)

### 5.6 EOS / Run 境界プロトコル

Receiver が EOS 検出(run_number 照合、stale EOS 無視 — Cross-Run EOS の教訓)
→ Sorter が `core_end = +inf` で全 flush → workers にポイズンピル ×N
→ 全 worker 完了バリア → Writer finalize(sidecar JSON)→ Display flush。
Run start は先頭から全ステージ reset。ここが唯一ステートフルなので
重点的にテストを書く。

### 5.7 ファイル構成(別バイナリを作らない)

EB を別バイナリにすると ZMQ 購読・run ライフサイクル・operator 連携・
start_daq.sh 配線・JSROOT ポートが二重になる。root_sink は既に
recorder + monitor の 1 プロセス 2 役(TODO 65)であり、builder を
3 役目として同居させる。

| ファイル | 中身 | テスト |
|---|---|---|
| `eb_core.hpp`(新規) | queue テンプレ / SortedChunk + split / 純関数ビルダー / seq 順序復元 | 素の g++、ROOT/ZMQ フリー厳守 |
| `sink_core.hpp` | 現状維持(watermark matcher は移行完了まで残す) | 既存 215 checks |
| `root_sink.cxx` | スレッド編成 + ROOT/ZMQ 境界だけ | E2E |

## 6. Rust EB の扱い

- **凍結**(削除しない)。テストは通し続ける(cargo test に含まれたまま)
- C++ EB が実キャンペーンを 1 つ完走するまで退役判断をしない
- `docs/event_builder_guide.md` / `docs/offline_event_builder_manual.md` の
  手順は凍結版として引き続き有効(oxyroot 出力が非圧縮である旨を注記済み)

## 変更履歴

| 日付 | 変更内容 |
|------|----------|
| 2026-08-04 | 初版作成(方針決定の記録) |
| 2026-08-04 | §5 マルチスレッド設計を確定(ブレインストーム)。Phase 1 を 3 段移行に書き換え、N デック汎用化を撤回(純関数ビルダーで置換) |
| 2026-08-04 | **Phase 1 実装完了**(検証ゲート V0-V8 全合格、4-fold 15,644 一致)。ユーザー決定: X/Y は ns 生値、**統合イベント方式**(1 tree、ch1 OR ch6 アンカー、面ペアリング、dt_trig=面間 TOF)。side3 配備は出張明けまで凍結 |
