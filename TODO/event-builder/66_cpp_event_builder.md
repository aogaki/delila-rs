# TODO 66: Event Builder の C++ 移行 (root_sink 系譜)

**Created:** 2026-08-04
**Status:** 📋 OPEN — 方針承認済み (2026-08-04)、実装未着手
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

### Phase 1: ThGEM built tree (最初の実戦)
- root_sink に built-event TTree 出力を追加: LiveEventBuilder.C 相当
  (trigger 4-fold + LaBr3 → X/Y/TOF/ETrigger/ESum/ELaBr)
- オフライン照合の答え(`macros/grid_resolution.C`、ユーザーのマクロ)が揃っている
- ここで watermark matcher を N デック汎用化(CoincidenceMatcher /
  PositionMatcher の 8 割重複を一度だけ括り出す)

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

## 5. Rust EB の扱い

- **凍結**(削除しない)。テストは通し続ける(cargo test に含まれたまま)
- C++ EB が実キャンペーンを 1 つ完走するまで退役判断をしない
- `docs/event_builder_guide.md` / `docs/offline_event_builder_manual.md` の
  手順は凍結版として引き続き有効(oxyroot 出力が非圧縮である旨を注記済み)

## 変更履歴

| 日付 | 変更内容 |
|------|----------|
| 2026-08-04 | 初版作成(方針決定の記録) |
