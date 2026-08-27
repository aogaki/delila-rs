# Current Sprint - TODO Index

**Updated:** 2026-08-25 — **TODO 67 実装完了**: AMax の書き込みチャンネル範囲を FW に追従化(`CHANNEL_PAGES` を codegen が導出 → `apply_amax_channel_config` がクランプ)。発端は gant のビルド赤(codegen 生成物 3 本が空 stub 化)と、12august FW が **2ch・`PAGE_BASE` 0x800000→0x80000** に変わっていたこと。`num_channels` だけが手書きで FW 追従していなかったため、32ch config × 2ch FW で **ch30 の書き込みが旧 32ch マップの ch0 ページに重なる**危険があった。**gant 配備済(CHANNEL_PAGES=2 を実 RegisterFile から自動導出、DAQ 無停止)。残件(FW 世代の Magic 照合・num_channels 整理)はレベッカへ移管**。
**前回 (2026-08-04)**: **TODO 66 Phase 1 実装完了**: root_sink を 5 スレッド構成(Receiver→Sorter→Workers×N→Writer + Display)に再構築し、統合 built-event tree(`--built-tree`)を追加。検証 V0-V8 全合格(run0023 replay で内容完全一致・workers 数非依存の決定的出力・**4-fold 15,644 がオフライン正解と完全一致**・hits tree 全体時刻ソート化)。**side3 配備は凍結中(ユーザー出張、2026-08-05〜。帰還後に実施)**。背景: EB の C++ 移行決定(SPEC v0.8、oxyroot 圧縮未実装 + root_sink 実証)、マルチスレッド設計は TODO 66 §5。
**前回 (2026-07-21)**: TODO 65 全面完了+アーカイブ、TODO 掃除(24/26/47/51/65 → archive、詳細記録退避 65KB→18KB)。

このファイルは現在のスプリントの概要を示すインデックスです。
Claudeセッション開始時に必ず読み込まれます。

---

## MVP Status: ✅ 達成 (2026-03-13)

全 Goal 達成。PSD2 + PSD1 + PHA1 全 FW DAQ 稼働中。Grafana モニタリング + ELOG 自動投稿も稼働中。

## Active Tasks

| Priority | File | Status | Summary |
|----------|------|--------|---------|
| **2** | [68_backlog_watermark.md](68_backlog_watermark.md) | **✅ 実装完了 (2026-08-25)、E2E 3 モード合格** | バックログ水位計測 + OOM 前の秩序ある停止(ポスター想定問答が発端)。`queue_bytes`/`backlog_level` を metrics 化、soft/hard 水位(TOML)、`/api/stop_drain` + opt-in autostop watcher で **drain-first stop**(source 停止→Running のまま drain→残り停止。静穏 3 poll 判定)。E2E: manual 254k/autostop 286k イベント**全数書き込み・損失ゼロ**、timeout 経路は残余計数報告。InfluxDB の非 source 欠落も修正。副産物: `with_results()` の success 上書きバグ発見・修正 |
| **1** | [67_amax_channel_pages_clamp.md](67_amax_channel_pages_clamp.md) | **✅ 実装完了 + gant 配備済 (2026-08-25) / 残件はレベッカ(FW 側)へ移管** | AMax レジスタ書き込み範囲の FW 追従化。codegen が RegisterFile から `CHANNEL_PAGES` を導出 → `apply_amax_channel_config` がループをクランプ + warn。テスト 8 本追加(lib 687 / codegen 27 全パス)。gant 破損復旧・12august 2ch FW のアドレス 16 倍シフト・FW 世代別 Magic 一覧も同ファイル |
| **1** | [64_amax_opendpp_params.md](64_amax_opendpp_params.md) | **🚧 Phase 0+A ✅済 (2026-07-16) / 残 = FWHM 実測(FW 修正版待ち)** | AMax UI に OpenDPP 標準 DevTree パラメータ追加（FW 開発者要望）。**Phase 0 実装+実機検証済**: `dc_offset` + `vga_gain` を AMax Input タブへ splice、FW OFFSET → "Offset (Trapezoid)" リネーム。gant SN52622（13july FW）で Tune Up Apply → DevTree 直読みで反映確認。**Phase A 済**: `docs/devtree_examples/vx2730_dppopen_sn52622.json`、**DPP_OPEN の ch パラメータは 10 個のみ**（channelstriggermask 無し）と確定。**残** = ChGain × trapezoid FWHM 実測（0/6/12 dB、13july FW の波形空バグ修正版待ち）、FW 開発者への確認事項（ChGain データパス位置/内部ビット幅）。Phase B/D は FW 開発者の要望次第 |
| **1** | [63_v1743_cfd_search_window.md](63_v1743_cfd_search_window.md) | **📋 OPEN (2026-07-09)** | 既存 x743 CFD テスト2件 fail (`cfd_valid=false`)。原因: `analyze()` の後方探索窓 `search_span=4·cfd_delay=16` が遅い立ち上がりパルスのゼロ交差を取り逃す (commit `e4ad305` から潜在)。修正前に実機パルスの rise/delay 比測定要。silent peak-fallback の可視化 (warn) も追加 |
| **2** | [52_refactor_sprint_2026-q2.md](52_refactor_sprint_2026-q2.md) | **📋 Phase 3 待機** | Phase 1+2 完了済 (23 項目、累計 -3716 行)。残 = Phase 3 Component Hardening: R-D3 (X743 read_loop split) / R-D5 (connection extract) / R-D11/D12 / R-P6 / R-P8 (ComponentRunner) / R-X3-post (ZMQ 境界 cost 再計測) |
| **2** | [event-builder/66_cpp_event_builder.md](event-builder/66_cpp_event_builder.md) | **🚧 Phase 1 完了 (2026-08-04)、side3 配備凍結中** | **EB の C++ 移行**(root_sink 系譜)。Phase 1 実装済み: eb_core.hpp(158 checks)+ 5 スレッド化 + `--built-tree`(統合イベント: 両面 + LaBr3、NaN=欠損、4-fold は cut)。検証 = run0023 replay 内容一致・決定性・R11/rollover/HTTP 負荷・**4-fold 15,644 完全一致**。恒久 E2E 基盤 test_publisher.cxx 追加。次 = side3 配備(出張明け)→ Phase 2(JSON 設定 + .delila リプレイ) |
| **2** | [59_eliade_trap_autotune.md](59_eliade_trap_autotune.md) ([JA](59_eliade_trap_autotune_ja.md)) | **📋 PLANNING (2026-06-16)** | ELIADE Ge trap 補正 auto-tune。2026 = Ge 分解能チューン、beam 2027-01。8× clover HPGe / 4×V1725 PHA + V1730 PSD |
| - | [event-builder/SPECIFICATION.md](event-builder/SPECIFICATION.md) | **参照 (v0.8)** | Event Builder 仕様 — 概念部は C++ 実装の要求仕様として有効。実装言語判断の経緯は変更履歴参照 |

**休眠(ハード/外部待ち、archive 済)**:
[47 V1743 Step 4-7](archive/47_v1743_standard_mode_redesign.md)(2台目+S1 分配待ち)/
[51 PHA2 校正源テスト](archive/51_pha2_integration.md)(校正源待ち)/
[26 スケーリング](archive/26_multi_digitizer_scaling.md)・[24 L2 Filter](archive/24_l2_filter_implementation.md)(計画のみ)/
[50 Mac USB EEM](archive/50_mac_usb_eem_driver.md)(週末プロジェクト)

---

## ポスト MVP — 次のセッション候補

- **A:** Energy Calibration + PSD 表示 (GitHub #7) → **設計完了** (2026-02-19, [設計書](../docs/plans/energy_calibration_psd.md)) — Phase 1 から開始
- **B:** x743 統合 (GitHub #6) → **Active** ([TODO](archive/45_v1743_support.md), [設計書](../docs/plans/x743_integration.md))
- **C:** Online EB 統合 (Phase 4: ZmqHitSource + Pipeline) — **✅ 2026-05-25 完了** (Operator command shim + `delila_merger_replay` staging + `eb_monitor_cli` stub)。EB Monitor 本体 (SPEC § 11.4 item M) は未着手
- **G:** 設定自動生成スクリプト (3-4) + デプロイスクリプト改善 (3-5)
- **I:** FELib/Dig2 Rust 移植検討 — JSON-RPC プロトコル直接実装 (ポスト MVP)

---

## Recently Completed

2026-05 以前の詳細エントリは [archive/completed_2026H1_details.md](archive/completed_2026H1_details.md) へ退避(2026-07-21 掃除)。

| File | Completed | Summary |
|------|-----------|---------|
| [65_cpp_root_sink.md](archive/65_cpp_root_sink.md) | 2026-07-21 | **root_sink 完成+拡張一式**: C++ 並列 ROOT シンク(スカラー TTree recorder + Δt/JSROOT モニタ、merger PUB 追加購読)。拡張 = ①ファイル名 Recorder 完全一致 `run%04u_0000_<exp>.root`(exp は --exp-name > --operator(/api/status)> "data")②`--hists histograms.json` 宣言的ヒスト + `/ReloadHists` ライブ再構築 ③`[network.root_sink]` TOML セクションで start_daq.sh 自動起動(`2f65dad`)④ROOT 自動分割(MaxTreeSize)対策: 2TB 設定+発生時は Recorder 連番でパートリネーム(`efe43a4`、UAF 回避)。**副産物: merger EOS 転送バグ発見・修正(`dc0669c`)** — 実 reader は Stop ack 後に EOS 発行→旧 merger は not-Running discard で EOS を silent 破棄、root_sink がラン境界を検知不能だった(EOS を両タスクの discard から恒久除外)。検証 = Mac 単体 157 + gant エミュ E2E(2ラン+ロールオーバー強制)+ side3 パルサー run13/14 完全一致。マニュアル docs/root_sink_manual.md + operations_manual JA/EN 反映。side3 は TOML 起動で運用中 |
| [58_code_review_2026-06-10.md](archive/58_code_review_2026-06-10.md) | 2026-07-13 | **全コードベース精査 — CRITICAL 4 + HIGH 14 + MEDIUM 20 + LOW 11 全決着**。C/H は 2026-07-09 (`563ab77` `61229b9` `7221c7d`)。M/L 最終回 (2026-07-13, `2af404a` `0f5907b` `539a2c7`): **FIXED 15** = M1/M2 (reader 状態の正直化: config ロード失敗→Arm ブロック、Arm/Start 失敗で Armed/Running を主張しない+5s backoff)、M3 (transmute UB→範囲ガード)、M5 (subcounter run 開始 underflow)、M8 (SIGTERM handler)、M9 (command_task rebind — socket エラーで永久制御不能だった)、M12 (node_agent SIGTERM→5s→SIGKILL)、M13 (Apply 拒否で configure=502 / run_start=Arm 前中断)、M14 (tuneup_apply id 一致検証)、M15 (configure/arm/stop に Tune Up ガード)、M16 (Mongo 失敗を HTTP response に)、M17 (start の current_run fallback 構造)、M18 (Merger gap/restart warn)、L3/L4/L6/L7/L9。**文書化 4** = L5 (batch_id 非単調)、L8 (CaenHandle コメント訂正)、L10/L11 (CLAUDE.md 例外明文化)。**defer 5** = M4/M7 (64ch ボード導入時)、M6 (serde_ignored 小タスク)、M10/M11 (perf、実測後)。**受け入れ 2** = M20 (Stop テール例外の範疇)、L1 (R-X1 監査済み)。681 tests pass |
| [62_v1743_drop_rollover.md](archive/62_v1743_drop_rollover.md) | 2026-07-09 | **V1743 rollover 撤去・生 TDC 直接化** (`40e87bf` master push 済): run30 破損の真因 = 起動後 ~1ms の未初期化 DMA バッファ由来ゴミ TDC (~6件, `0x1B1B1B1B` バイト反復) を RolloverTracker 永続状態が恒久破損に増幅 → `timestamp_ns = (TDC&40bit)×5 + cfd` 直接化で自己回復 (run46 実機確認, dropped=0/1.19M)。運用ルール: ラン 90 分未満。診断 `X743TdcDiag` (後退ログ上限化)。ゴミ受け入れ方針を operations_manual §9 (JA/EN) に明記。敵対的レビュー 7 エージェント + 指摘 2 件反映 |
| [61_delila2root_cpp.md](archive/61_delila2root_cpp.md) | 2026-07-08 | **delila2root C++ 化 + `.delila` format v3** (`fdc0721`+`90cd4ea`): 自己記述スキーマ埋込 + 依存ゼロ単一ヘッダ `TDelila.hpp` + ROOT ネイティブ ZSTD (4.3x)。v2 後方互換。Side3 実機 end-to-end (V1743 波形付き 108k events) 検証済。TODO 56/57 を置換 |
| [60_amax_fw_selfconfig.md](archive/60_amax_fw_selfconfig.md) | 2026-06-26 | **AMax FW 自己設定化** (`9096c77`+`7656f0a`): codegen が page_base/stride/broadcast_base を RegisterFile から自動導出、`scripts/update_amax_fw.sh` 一発で codegen→build→UI→deploy。FW 更新毎の手動アドレス合わせを撤廃 |
| [55_amax_10g_fw_round2.md](archive/55_amax_10g_fw_round2.md) | 2026-05-08 (close 2026-07-09) | **AMax 10G FW Round 2** (`8786008`+`1984bd3`): 16ch + 16 digital probes + Tune Up sub-mode + register inspector。残っていた J2/J3 実機検証は FW が 6 月版に置換され陳腐化 (後継は TODO 60 経由で検証済) としてクローズ |

## Cancelled (archived)

| File | Reason |
|------|--------|
| [archive/22_amax_decoder_implementation.md](archive/22_amax_decoder_implementation.md) | FELib OpenDPP エンドポイントから直接イベント取得で十分。デコーダ不要 |
| [archive/41_start_stop_restructure.md](archive/41_start_stop_restructure.md) | DIG1 タイムスタンプリセット問題は別原因が判明。Start/Stop フロー再構成は不要 |

---

## Completed Features (Summary)

- Emulator + ZMQ pipeline
- Merger (zero-copy forwarding)
- Recorder (sorting + file format v2)
- Monitor (Web UI + REST API + ECharts histogram/waveform)
- Operator (control system + pipeline ordering)
- MongoDB統合 (Run履歴、Comment永続化、Notes logbook)
- Source Config Management (SourceType enum, config_file, RuntimeConfig)
- Metrics API + RateTracker
- PSD2 デコーダ + 実機動作確認 (VX2730, 10kHz)
- PSD1 デコーダ + DT5730B 実機検証
- PHA1 デコーダ + マルチデジタイザ統合テスト (PSD2 + PHA1, マルチマシン ZMQ)
- データ出力検証 (E2E テスト + recover validate/dump + ROOT マクロ)
- データ完全性 + パフォーマンス + 波形堅牢性修正
- HV Gain Matcher Python ツール (SY5527, Phase 1 完了)
- Rust Event Builder (L1 完了, 67 tests, 206M hits in ~5s)
- Gemini レビュー改善
- Settings UI v2 (6カテゴリ + SetInRun + Apply via ZMQ)
- Apply Digitizer Config via ZMQ (Idle/Configured/Running 全対応)
- Tune Up Mode (Waveform + ヒストグラム + パラメータ 3-panel UI)
- Channel Registration (Monitor チャンネル事前登録 + 個別チャンネル名)
- AMax Viewer (スタンドアロン GUI ツール: 2D Histogram + Waveform + パラメータ調整 + ROOT出力)
- Monitor Quick Create (デジタイザ選択で全チャンネルビュー自動生成)
- Monitor レイアウト永続化 (Operator REST API + ファイル保存, 全ブラウザ共有)
- PHA1 Settings UI修正 (FirmwareType PHA→PHA1統一 + Virtual Probe データ駆動型 + 単位修正)
- Stop コマンドタイムアウト修正 (decode_loop yield + Recorder writer std::thread 分離)
- Tune Up Apply スペクトラム混在修正 (Merger sender_task ステート対応 + 全パイプライン Stop→Start)
- パラメータバリデーション (DevTree snap_to_step + UI step属性)
- delila2root Phase 1 (POD Event + two-pointer merge, 10.4億events 2m31s, 6.9M/s)
- PHA1 Waveform Decoder 修正 (sign_extend_14bit 符号拡張 + digital probe D0/D1 マッピング修正)
- ROOT マクロ ns_per_sample 対応 (Waveform X軸 ns 表示)
- Operator デフォルトポート 9090 移行 + docker mongo-express 8083
- Tune Up ソフトウェアトリガー強制 (clone-and-modify, SyncConfig 両方上書き)
- Run Start Waveform Recording 警告 (MatDialog, DigitizerService チェック)
- Tune Up Waveform 積算表示 (FIFO バッファ + timestamp 重複検出 + ECharts replaceMerge)
- Run History ビューワー (Runs タブ, アコーディオン展開, config snapshot 表示)
- BSON config snapshot 修正 (HashMap<u8/u32> キーの string 変換 serde モジュール)
- Config snapshot 診断ログ追加 (起動時ロード数 + 空時 warn)
- Cross-Run EOS 汚染修正 (run_number タグ + stale EOS フィルタ)
- DecodeLoop 並列化 (4 Workers crossbeam, PSD2 3.2x改善, 全FW統一パス)
- A3818 ドライバパッチ v1.6.12-delila1 (バッファオーバーフロー+off-by-one+セマフォ+PCIe障害, 76デプロイ済)
- ReadLoop transient error retry (30s timeout, 10ms interval, 5MHz×6modules 10min検証, 20回リカバリ全成功)
- DIG1 ch_extras_opt コード強制 (PSD1/PHA1 の 48-bit timestamp を JSON 設定に依存せず handle.rs で保証)
- oxyroot ROOT 出力ベンチマーク (Vec events 0.79M/s, file-per-batch確定, Stop <130ms)
- EB 統一パイプライン Phase 0-3 (HitSource trait, pipeline.rs, source.rs, .delila直接入力CLI, time alignment histogram)
- Grafana モニタリング (InfluxDB v3 Core + Grafana 2ダッシュボード: DAQ Overview + Channel Rate 48ch Stat)
- `is_master` 削除 (TOML/Operator config → Reader `startmode` に一元化、3MV Start タイムアウト修正)
- PHA1 実機稼働 (VX1730B 光リンク, 全FW DAQ完成)
- ELOG 統合 (Docker + Rust クライアント + Run Stop 自動投稿)
- A3818 scheduling-while-atomic 修正 (spin_lock→mutex, 76デプロイ済)
- V1743 Standard mode 単体構成 (VX1743 SN:25, optical link, CFD soft fine time, 95 min long-run で 40-bit TDC rollover 通過確認)
- RolloverTracker 統一実装 (PSD1/PHA1 SW Fine TS + V1743 TDC を modulo 演算で共通処理, 旧 Instant-ベース TimestampTracker 完全削除)
- UI Audit 2026-Q2 (32 項目 punch-list audit + 11 commits、 Settings header 整理 / CAEN enum 文化人化 / Monitor を chips 化 / Runs detail 別 page + ISO 8601 / Apply 失敗 inline alert / NotificationService 全統一 / error snackbar Copy ボタン)

---

## Archived

| Directory/File | Contents |
|----------------|----------|
| `archive/phase1_basic_pipeline/` | 基本パイプライン設計 |
| `archive/phase1_components/` | CLIリファクタリング、CAEN FFI、Monitor、Merger |
| `archive/phase1_control_system/` | コントロールシステム設計 |
| `archive/phase2_infrastructure/` | タイムスタンプソート、Metrics API、Source設定管理 |
| `archive/phase3_psd_decoders/` | PSD2 バグフィックス、PSD1 デコーダ実装+実機検証 |
| `archive/phase4_data_verification/` | データ出力検証 (E2E テスト、recover dump、ROOT マクロ) |
| `archive/phase5_decoders_and_testing/` | PHA1 デコーダ、マルチデジタイザ統合テスト |
| `archive/phase5_audit_and_review/` | データ完全性監査、Gemini レビュー改善 |
| `archive/11_operator_web_ui.md` | Operator Web UI (Phase 6 に統合) |
| `archive/15_digitizer_implementation.md` | デジタイザ実装 Phase 1-5 |
| `archive/16_linux_migration_checklist.md` | Linux移行チェックリスト |
| `archive/23_event_builder_implementation.md` | Event Builder L1 実装 (Time Slice 方式) |
| `archive/19_settings_ui.md` | デジタイザ設定 UI Phase 6 |
| `archive/22_amax_decoder_implementation.md` | AMax デコーダ (Cancelled) |
| `archive/25_apply_digitizer_via_zmq.md` | Apply Digitizer Config via ZMQ |
| `archive/27_settings_ui_v2.md` | Settings UI v2 (Phase 1-6) |
| `archive/28_tuneup_mode.md` | Tune Up Mode 実装 |
| `archive/28_psd1_pha1_parameter_overhaul.md` | PSD1/PHA1 パラメータ整理 |
| `archive/29_channel_registration.md` | Channel Registration + チャンネル名 |
| `archive/29_psd1_waveform_debug.md` | PSD1 Waveform デバッグ |
| `archive/31_parameter_validation.md` | パラメータバリデーション (Phase 1-3) |
| `archive/32_stop_command_timeout.md` | Stop タイムアウト修正 |
| `archive/34_tuneup_software_trigger.md` | Tune Up ソフトウェアトリガー |
| `archive/35_waveform_recording_warning.md` | Waveform Recording 警告 |
| `archive/36_accumulated_waveform.md` | 積算 Waveform 表示 |
| `archive/39_cross_run_eos_fix.md` | Cross-Run EOS 汚染修正 |
| `archive/40_decode_loop_parallelization.md` | DecodeLoop 並列化 |
| `archive/41_start_stop_restructure.md` | Start/Stop フロー再構成 (Cancelled) |
| `archive/42_a3818_driver_patch.md` | A3818 ドライバパッチ v1.6.12-delila1 |
| [44_a3818_open_fix.md](archive/44_a3818_open_fix.md) | A3818 scheduling-while-atomic 修正 |
| `archive/24_l2_filter_implementation.md` | L2 Filter 計画 (休眠、2026-07-21 移動) |
| `archive/26_multi_digitizer_scaling.md` | 10+ デジタイザスケーリング計画 (休眠、2026-07-21 移動) |
| `archive/47_v1743_standard_mode_redesign.md` | V1743 Standard mode (Step 1-3 完了、4-7 ハード待ち) |
| `archive/51_pha2_integration.md` | PHA2 統合 (校正源テストのみ残) |
| `archive/65_cpp_root_sink.md` | root_sink (完了、2026-07-21) |
| `archive/completed_2026H1_details.md` | CURRENT.md から退避した 2026 前半の詳細完了記録 |

---

## Known Issues / Future Work

- **TIME_STEP_NS ハードコード:** `src/config/digitizer.rs` の `TIME_STEP_NS = 2` は DT5730 (500 MS/s) 固定。DT5725 (250 MS/s → 4 ns/sample) など別サンプリングレートのデジタイザに対応する場合、`DeviceInfo.sampling_rate_sps` から動的計算するか、`DigitizerConfig` にサンプリングレートを持たせる必要あり。

## Notes

- **MVP: ✅ 達成** (2026-03-13)
- **現在のフェーズ:** ポスト MVP — 安定運用中、改善・拡張フェーズ
- **実機確認済み:**
  - VX2730 (Serial: 52621, DPP_PSD2, 32ch, Ethernet, 172.18.4.57)
  - DT5730B (Serial: 990, DPP_PSD1/PHA1, 16ch, USB) — ライセンス 30 min 制限あり
  - 5x VX1730B (PSD1, 16ch each) + 1x VX2730 (PSD2, 32ch) on 172.18.4.76
  - VX1743 (Serial: 25, SAMLONG 12-bit, 8ch/4groups, Optical Link) on 172.18.4.147 — Standard mode + 95 min long-run 完了 (2026-04-23)
- **動作環境:** Linux (Ubuntu, Rust 1.93.0) + macOS (クロスマシン統合)

## Reference Documents

| Document | Location | Priority |
|----------|----------|----------|
| **x2730 DPP-PSD CUP Documentation** | `legacy/documentation_2024092000-2/` | ★★★ |
| FELib User Guide | `legacy/GD9764_FELib_User_Guide.pdf` | ★★ |
| Digitizer System Spec | `docs/digitizer_system_spec.md` | ★★★ |
| Event Bridge Wire Format | `docs/event_bridge_wire_format.md` | ★★ |
| Event Builder Spec | `TODO/event-builder/SPECIFICATION.md` | ★★★ |
