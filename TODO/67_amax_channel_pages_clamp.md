# TODO 67 — AMax `CHANNEL_PAGES`: レジスタ書き込み範囲を FW に追従させる

**Status: ✅ 実装完了 + gant 配備済 (2026-08-25) / 残件はレベッカ(FW 側)へ移管**

**関連:** [60_amax_fw_selfconfig.md](archive/60_amax_fw_selfconfig.md)(codegen 自己設定化の前身) /
[64_amax_opendpp_params.md](64_amax_opendpp_params.md) / `docs/amax_fw_update_manual.md`

---

## 1. 発端 — gant のビルドが赤いまま動かない (2026-08-25)

`./scripts/update_amax_fw.sh FW/16June2026/RegisterFile.json` が codegen 段階の手前、
**lib のコンパイルで**落ちていた。

```
error[E0609]: no field `enable_acq` on type `&AMaxChannelConfig`
   --> src/reader/mod.rs:842
```

`13july2026` の RegisterFile を渡しても同じエラーが出たことで、入力依存ではなく
**gant のワーキングツリーそのものが壊れている**と確定した。

### 真因

gant 上で codegen 生成物 3 ファイルが、`PAGE_BASE = 0x0` の空 stub に置き換わっていた。

| ファイル | 状態 |
|---|---|
| `src/config/amax_generated.rs` | 空 stub(全フィールド消失) |
| `src/reader/caen/amax_registers_generated.rs` | `PAGE_BASE = 0x0`、`REG_*` ゼロ件 |
| `web/operator-ui/src/app/models/amax-generated.ts` | 空 |

いまは存在しない RegisterFile(おそらく `RegisterFile_newscicomp.json`)を食わせた
codegen 実行の残骸。レジスタが 1 個も解決できず、それでもエラーにならずに
「空の生成物」を書き出していた。

- commit `302b87e`(AMax 2D plot)は無関係。**UI 19 ファイルのみ**の変更で、しかも
  空 stub 化の約 1 時間 45 分**後**のコミット。
- 破損ファイルのバックアップ: `gant:/tmp/amax_codegen_broken_20260825_102314`
- 復旧 = HEAD から 3 ファイルを checkout。HEAD とバイト一致を確認済み。
- **旧メモ「生成 2 ファイルを checkout で戻すのは厳禁」は撤回**。あれは −509/+9 の
  空 stub を守っていただけで、正しい対処を妨げていた。

> ⚠️ **ブートストラップの罠**: `update_amax_fw.sh` は step 1 が `cargo run --bin amax_codegen`
> なので、**lib が壊れていると codegen にたどり着けない**。生成物が壊れる ⇒ lib が壊れる
> ⇒ 生成物を直せない、というデッドロックになる。生成物を手で最小限つくって
> ブートストラップするしかない(§4 でも同じ手順を踏んだ)。

---

## 2. 12august 2ch FW — アドレスマップが 16 倍動いた

復旧後、`FW/12august/RegisterFile_newamax_2ch.json` で codegen 成功。
しかし生成物の実質変更は **ベースアドレス 2 個だけ**だった。

```diff
-pub const PAGE_BASE: u32      = 0x800000;
+pub const PAGE_BASE: u32      = 0x80000;
-pub const BROADCAST_BASE: u32 = 0x800000;
+pub const BROADCAST_BASE: u32 = 0x80000;
```

`PAGE_STRIDE` は `0x40000` のまま、**レジスタ別オフセットの変更は 0 件**、
28 フィールドの名前も同一。`amax_generated.rs` / `amax-generated.ts` の差分は
出典コメント 1 行のみ。`dist/` の変化は 0 ファイル。

### ベースがチャンネル数に比例する

| RegisterFile | channels | PAGE_STRIDE | PAGE_BASE | |
|---|---|---|---|---|
| `FW/13july2026/RegisterFile13iulie.json` | 32 | 0x40000 | 0x800000 | = 32 × 0x40000 |
| `FW/12august/RegisterFile_newamax_2ch.json` | 2 | 0x40000 | 0x80000 | = 2 × 0x40000 |

チャンネルページ領域が「自分と同じサイズの領域の直後」に置かれる設計。
16 倍のずれはエクスポートの欠落ではなく **2ch ビルドの必然**と判断した。

なお RegisterFile の `Project` 名は `NEW_AMAX_firmware32_caenlist_triggrer_logic_32channels`
で「32」を名乗るが、実体は 57 レジスタ / 2 チャンネルページ(`page_amax_energy_1_0`,
`page_amax_energy_1_1`)。FW 側は当面 2ch 運用とのこと。

### FW 世代の Magic 値(実機照合用)

| FW | ページ命名 | PAGE_BASE | Magic |
|---|---|---|---|
| 16June2026 / 20260617 | `_4_N` | — | `204AB9EC` |
| 13july2026 | `_1_N` | 0x800000 | `169CC879` |
| 12august 2ch | `_1_N` | 0x80000 | `6000F6A7` |

---

## 3. 本題の欠陥 — `num_channels` だけが FW に追従しない

AMax のアドレスマップは、ほぼ全部 RegisterFile から自動導出されている。

| 値 | 出どころ |
|---|---|
| `PAGE_BASE` / `PAGE_STRIDE` / `BROADCAST_BASE` | codegen が導出 |
| レジスタ集合・各オフセット | codegen が導出 |
| **ループ回数 = `num_channels`** | **digitizer JSON に手書き** |

「アドレスの作り方」は FW に追従するのに「それを何回まわすか」は追従しない。
[`apply_amax_channel_config`](../src/reader/caen/handle.rs) は
`for ch in 0..config.num_channels` で回るだけだった。

### 実害の見積り

`config_amax_56_2Digitizer.toml` が参照する
`config/digitizers/amax_56_10G.json` と `amax_56_10G_slave.json` はどちらも
`"num_channels": 32`。2ch FW に対してそのまま Configure すると、
**28 レジスタ × ch2〜ch31 = 840 回**の書き込みが FW の定義しないアドレスへ飛ぶ。

しかも 2ch マップの `PAGE_BASE = 0x80000` から数えると:

| 論理 ch | word addr | |
|---|---|---|
| ch2 | `0x100000` | 未定義領域 |
| ch29 | `0x7C0000` | 未定義領域 |
| **ch30** | **`0x800000`** | **旧 32ch マップの ch0 ページ基点そのもの** |
| ch31 | `0x840000` | 旧 32ch マップの ch1 ページ内 |

単に「どこでもない場所」ではなく、**古い 32ch マップの実アドレスに重なる**。
「旧アドレス書込は FW 破壊」の条件に一致する。

`validate_num_channels` は救いにならない。照合先が `/par/NumCh`
(ベース FW が返す物理 32ch)なので、AMax カスタム FW のページ数とは無関係に通る。

---

## 4. 実装 (2026-08-25)

### 4.1 codegen が `CHANNEL_PAGES` を導出・出力

`derive_layout` は既にチャンネルページを全部 `min_by_idx: BTreeMap<u32, u32>` に
拾っていた(`PAGE_STRIDE` はここから計算されている)のに、導出後に捨てていた。

- `DerivedLayout` に `channel_pages: u32` を追加。値は **span (`max index + 1`)**、
  個数ではない。疎な index 集合(ch0 + ch4 のみ等)で実在するページを取り零さないため。
- 疎な集合を検出したら `main` が warn(CLAUDE.md「silent failure を作らない」)。
- レイアウト要約行に `CHANNEL_PAGES=N` を追加。

```
amax_codegen layout: PAGE_BASE=0x800000 (auto), PAGE_STRIDE=0x40000 (auto),
                     BROADCAST_BASE=0x800000 (auto), CHANNEL_PAGES=32, canonical=per-channel ch0
```

- `emit_rust_registers` が `pub const CHANNEL_PAGES: u32` を出力。
  `PAGE_BASE` と同じ RegisterFile から出るので、両者は原理的にズレない。

### 4.2 書き込みループをクランプ

`src/reader/caen/handle.rs`:

```rust
let n_ch = r::amax_channel_span(config.num_channels, r::CHANNEL_PAGES);
if n_ch < config.num_channels {
    warn!(config_num_channels = ..., fw_channel_pages = ..., first_skipped = n_ch,
          "[AMax] config num_channels exceeds the channel pages this firmware defines ...");
}
for ch in 0..n_ch { ... }
```

`amax_channel_span` は純関数として `src/reader/caen/amax_registers.rs`(生成物の
手書きラッパ)に置いた。FFI だらけの handle.rs ではなくここに置くことで、Operator 側
(§4.3)からも同じ判定を参照できる。
**`CHANNEL_PAGES == 0` は「書き込むな」ではなく「per-channel ページを持たない
broadcast-only FW ⇒ クランプ対象なし」** と解釈する。ここを取り違えると
2026-04 世代の FW で設定が一切効かなくなる。

### 4.3 skipped を操作画面に出す

クランプはハードウェアを守るが、それだけだと Operator の表示は
`Applied 924 parameters to hardware` → `Applied 84 parameters to hardware` と
**数字が黙って 1/10 以下になるだけ**で、理由は reader のログにしか出ない。
操作者の席から見れば silent failure のままなので、HTTP 応答に載せた。

`amax_registers::channel_clamp_note(firmware, num_channels) -> Option<String>`
（純関数。AMax 以外・範囲内なら `None`）を 3 経路に配線:

| 経路 | ファイル |
|---|---|
| Apply ボタン | `src/operator/routes/digitizer.rs` |
| **Tune Up Apply**（操作中に一番見る経路） | `src/operator/routes/tuneup.rs` |
| Configure（自動経路。複数 reader 分を `;` 連結） | `src/operator/routes/status.rs` |

フロントは既に `notify.success(result.message)` で成功トーストに出しているので
（[digitizer-settings.component.ts:1332](../web/operator-ui/src/app/components/digitizer-settings/digitizer-settings.component.ts#L1332)）、
**UI 側の変更・`dist/` の再ビルドは不要**。

実際の表示（32ch config × 2ch FW）:

```
Applied 84 parameters to hardware — AMax: config num_channels=32 exceeds the 2
channel page(s) this firmware defines; channels 2-31 skipped (fix num_channels
in the digitizer JSON)
```

Configure は失敗扱いにしない（`success` は true のまま、message に追記）。
書き込み自体は正しくクランプされて成功しているため。

### 4.5 gant 配備 (2026-08-25 完了)

コミットせず rsync でソース 6 本 + TODO 2 本を送り、gant 上で codegen から回した。

```
$ ssh gant@172.18.6.114
$ ./scripts/update_amax_fw.sh FW/12august/RegisterFile_newamax_2ch.json --no-ui
amax_codegen layout: PAGE_BASE=0x80000 (auto), PAGE_STRIDE=0x40000 (auto),
                     BROADCAST_BASE=0x80000 (auto), CHANNEL_PAGES=2, canonical=per-channel ch0
    Finished `release` profile [optimized] target(s) in 1m 15s
```

**`CHANNEL_PAGES=2` が実 RegisterFile から自動導出された** — 仕組みが実データで効くことの確認。
gant 側 `cargo test` も全パス(lib 689 / codegen 27、exit 0)。

- §1 のブートストラップの罠を踏むので、rsync 後に生成ファイルへ
  `pub const CHANNEL_PAGES: u32 = 2;` を手で 1 行入れてから codegen を回した。
  codegen が正しい値で上書きするため stub は残っていない(確認済み)。
- 生成物 3 本のバックアップ: `gant:/tmp/amax_gen_backup_20260825_111908`
- `--no-ui`: 今回 TS 出力は出典コメントすら変わらず `dist/` も無変更のため。
- **DAQ は触っていない**。5 コンポーネント全て Idle / events 0 のまま。

⚠️ **クランプは「何チャンネル書くか」しか守らない。ベースアドレスが合っているかは別問題。**
ボードに 13july(32ch, `PAGE_BASE=0x800000`)が焼かれたまま 12august の生成物で動かすと、
書き込みは全部 `0x80000` 起点に飛ぶ。**Magic 照合は依然として必須**。

### 4.4 テスト

| テスト | 何を守るか |
|---|---|
| `derive_layout_channel_pages_tracks_the_fw_channel_count` | 2ch / 32ch で 2 / 32 |
| `derive_layout_channel_pages_reports_the_span_not_the_count` | 疎な集合で span (5) を返す |
| `derive_layout_broadcast_only_era` | per-channel ページ無し ⇒ 0 |
| `amax_channel_span_clamps_config_wider_than_firmware` | 32ch config × 2ch FW ⇒ 2 |
| `amax_channel_span_keeps_config_narrower_than_firmware` | 意図的に狭い config は尊重 |
| `amax_channel_span_trusts_config_when_firmware_has_no_channel_pages` | 0 を「書くな」と読まない |
| `amax_channel_span_saturates_absurd_firmware_page_counts` | u32→u8 の切り捨て事故防止 |
| `clamp_note_names_the_skipped_channels` | note が実際の `CHANNEL_PAGES` を引用する |
| `clamp_note_is_silent_when_the_config_fits` | 範囲内なら黙る |
| `clamp_note_ignores_non_amax_firmware` | 他 FW は FELib 経由なので対象外 |

`cargo test` 全パス(lib **689** / amax_codegen 27)。clippy は本変更で新規指摘ゼロ
(既存の 4 件はローカル clippy が新しいことによる HEAD 由来)。

`fmt` 済み。生成物は `FW/13july2026/RegisterFile13iulie.json` で再生成し、
`CHANNEL_PAGES = 32` の追加以外に差分が出ないことを確認済み。

---

## 5. 残作業 — **レベッカ(FW 側)へ移管 (2026-08-25)**

DELILA 側でやることは残っていない。以下は FW 世代が確定しないと判断できないため、
FW 作成者側の回答待ち。**回答が来るまで gant の DAQ は再起動しないこと**。

### レベッカへの確認事項

| # | 質問 | なぜ必要か |
|---|---|---|
| Q1 | gant のボードに焼かれている FW はどれか(Magic 値) | `6000F6A7`=12august 2ch / `169CC879`=13july 32ch。生成物と世代が違えば全書き込みが誤アドレスへ |
| Q2 | `RegisterFile_newamax_2ch.json` は本当に 2ch ビルドか、32ch FW の部分エクスポートか | `Project` 名は `..._32channels` なのに 2 ページ / 57 レジスタしかない |
| Q3 | `PAGE_BASE` は本当に `0x800000` → `0x80000` に動いたか | 他は 1 バイトも変わっていないのにベースだけ 16 倍ずれている |
| Q4 | `RegisterFile_newscicomp.json` は何だったか | gant の生成物を空 stub 化した犯人(§1)。ファイルは消えている |
| Q5 | 今後 2ch 継続か 32ch に戻るか | `num_channels` を 2 に落とすか別 config を新設するかの分岐 |

### 回答後に DELILA 側でやること

1. Q1 で世代が確定したら、その RegisterFile で `update_amax_fw.sh` を回して gant を確定させる
2. Q5 に応じて `num_channels` を整理(2 に落とす / `amax_56_2ch.json` を新設)
3. DAQ 再起動

---

## 5-old. (参考) 移管前の残作業リスト

1. **実機 Magic 照合(最優先)** — gant のボードに焼かれている FW 世代を Magic で確認する。
   `6000F6A7` なら 12august 2ch、`169CC879` なら 13july 32ch。
   **照合前に DAQ を再起動しないこと**(§6)。
2. **`num_channels` の整理** — クランプが入ったので FW 破壊のリスクは消えたが、
   `amax_56_10G.json` / `amax_56_10G_slave.json` が 32 のままだと
   Apply / Configure のたびに画面に skipped 通知が出続ける。
   2ch 恒久なら 2 に落とす。32ch FW に戻る可能性があるなら
   `amax_56_2ch.json` 系を新設して toml をそちらへ向ける。
   ※ `num_channels` は `get_enabled_channels_from_config` と `to_caen_parameters`
   も駆動するので、2 に落とすと標準 DIG2 パラメータと有効チャンネル列挙も ch0/ch1 のみになる
   (2ch FW ならそれが正しい挙動)。
3. ~~**gant への配備**~~ — **完了(§4.5)**。
4. FW 作成者への確認: `RegisterFile_newamax_2ch.json` は 32ch FW の部分エクスポートか、
   本当に 2ch ビルドか。消えた `RegisterFile_newscicomp.json` は何だったか。

---

## 6. gant の状態 (2026-08-25 時点)

- reader ×2 / merger / recorder / monitor / operator が **2026-08-14 15:55:47 から稼働**、
  state = **Idle**、`config_amax_56_2Digitizer.toml`、operator port 9090。
- **稼働中プロセスのメモリ上のイメージは古い `22f024a` ビルド(13july マップ)**。
  ボードには 12august マップの書き込みは一度も行っていない。
- ディスク上の `target/release/*` は 12august マップでビルド済み。
  **再起動するとレジスタマップが切り替わる**。

---

## 7. 教訓

- **自動導出したのに捨てた値は、いつか手書きの定数に化けて事故になる。**
  `PAGE_STRIDE` を計算するために全チャンネルページを列挙していながら、
  その個数を捨てていたのが今回の穴。
- **codegen が「空の結果」を正常終了で書き出せてはいけない。** 今回の gant 破損の
  引き金はこれ。§5 とは別に、`amax_codegen` が解決レジスタ 0 件で
  エラー終了するガードを入れる価値がある(未着手)。
- **生成物とビルドの循環依存**(§1 の罠)は `update_amax_fw.sh` の構造的な弱点。
  README/マニュアルにブートストラップ手順を書くか、codegen をワークスペース分離した
  別クレートにするかは要検討(未着手)。
