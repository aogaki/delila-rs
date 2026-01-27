# Current Sprint - TODO Index

**Updated:** 2026-01-26

このファイルは現在のスプリントの概要を示すインデックスです。
Claudeセッション開始時に必ず読み込まれます。

---

## ~~最優先: PSD2 デコーダ バグフィックス~~ ✅ 完了 (2026-01-26)

**Linux移行後の実機検証で発見されたバグ** → `TODO/archive/phase3_psd_decoders/00_psd2_decoder_bugfix.md`

C++ リファレンス (`external/caen-dig2/src/endpoints/dpppsd.cpp`) との比較で
以下の重大バグを確認:

1. **[P0] Single-word event 未対応** - 高レート時にデコーダがデシンクしてデータ全損
2. **[P0] Special event 未フィルタ** - 統計イベントが物理データに混入
3. **[P1] STOP シグナル無視** - ハードウェア停止が検出されない
4. **[P2] FLAGS マスク誤り** - flag_low_priority が 11bit (正しくは 12bit)
5. **[P2] Waveform 欠落** - convert_event() が波形データを落としている

**実機確認済み:**
- ハードウェア接続: OK (VX2730, dig2://172.18.4.56)
- データ読み出し: OK (9000イベント/テスト)
- タイムスタンプ: 正常 (10μs間隔)
- energy=0: ゲートパラメータ未設定が原因 (psd2_test.json 適用で解消見込み)

---

## ~~PSD2 実機動作確認~~ ✅ 完了 (2026-01-26)

- DAQフルパイプライン動作確認: Reader → Merger → Recorder → Monitor (10kHz)
- Operator REST API 経由で Configure → Arm → Start → Running 遷移
- ch4 パルサー信号でヒストグラム表示確認 (energy ≈ 34, bin[2])
- Angular UI (port 4200) + Monitor API (port 8081) 動作確認

**ヒストグラム表示バグ修正** (2026-01-26):
- bin[2] (16.7Mカウント) がチャート上で欠落する問題を修正
- 原因: 4096バーをサブピクセル幅 (~0.2px) で描画 → ECharts large-mode で隣接バーに上書きされピーク消失
- 対策: max-value ダウンサンプリング (ROOT TH1::Draw() と同じアプローチ) + largeThreshold 引き上げ
- 修正ファイル: `web/operator-ui/src/app/components/histogram-chart/histogram-chart.component.ts`

**設定ファイル:**
- `config/config_psd2_test.toml` - 実機テスト用 (ChSelfTrigger)
- `config/digitizers/psd2_test.json` - デジタイザ設定 (ch4有効, threshold=1000)

---

## ~~PSD1 デコーダ実装~~ ✅ 全Phase完了 (2026-01-26)

**仕様書:** `docs/psd1_decoder_spec.md`
**実装計画:** `TODO/archive/phase3_psd_decoders/17_psd1_decoder_implementation.md`
**ハードウェア:** DT5730B (Serial: 990, DPP-PSD1, USB, 8ch, 14-bit, 500 MS/s)

### Phase 1: デコーダコア ✅ → Phase 2: Reader統合 ✅ → Phase 3: 実機検証 ✅

**Phase 1 完了 (2026-01-26):** 46 テスト pass, Board/Channel/Event の 3 層デコーダ実装
**Phase 2 完了 (2026-01-26):** DecoderKind enum dispatch, from_config() mapping, Arm=Start 対応
**Phase 3 完了 (2026-01-26):** 実機検証成功 — 14/14 パラメータ適用, ~10.4k evt/s, ヒストグラム表示確認

Phase 3 で修正した主な課題:
1. DIG1 endpoint: DATA+SIZE のみ (N_EVENTS 除外)
2. START_MODE_SW: Arm コマンドを Start フェーズで実行
3. Watch channel 状態スキップ: `(_, Running)` パターンで対応
4. PSD1 パラメータ値フォーマット: ポラリティ/extras/self_trg の値マッピング

---

## ~~データ出力検証 (Task B)~~ ✅ 完了 (2026-01-26)

→ `TODO/archive/phase4_data_verification/18_data_verification.md`

- E2E テスト: 4テスト全パス (flags に per-event XOR チェックサム, シード付き乱数)
- `recover validate`: emulator データ 59,560,000 イベント Valid
- `recover dump`: flat binary 変換成功 (22 bytes/event, サイズ整合確認)
- `macros/read_dump.C`: legacy Recorder 互換 TTree (DELILA_Tree)

## ~~Phase 6 — デジタイザ設定 UI~~ ✅ 実装完了 (2026-01-27)

→ `TODO/19_settings_ui.md`

**実装内容:**
1. Reader Detect コマンド (FELib一時接続 → DeviceInfo取得 → 切断)
2. MongoDB スキーマ拡張 (serial_number, model + serial検索)
3. REST API 拡張 (POST /api/digitizers/detect, GET /api/digitizers/by-serial/:serial)
4. Angular チャンネルテーブルコンポーネント (横スクロール, sticky列, override ハイライト)
5. digitizer-settings 3タブ化 (Board / Frequent / Advanced)
6. config expand/compress ロジック (defaults+overrides ↔ flat per-channel)

## 次のセッション

- **A:** Multi-digitizer 統合テスト (PSD1 + PSD2)
- **D:** Phase 10: Angular UI の rust-embed 統合

---

## Active Tasks

| Priority | File | Status | Summary |
|----------|------|--------|---------|
| 1 | [19_settings_ui.md](19_settings_ui.md) | **Implemented** | Phase 6: デジタイザ設定 UI |
| 2 | [15_digitizer_implementation.md](15_digitizer_implementation.md) | **In Progress** | VX2730 (PSD2) 実機デジタイザ実装 |
| 3 | [11_operator_web_ui.md](11_operator_web_ui.md) | **In Progress** | Operator Web UI (Angular + Material) |
| - | [16_linux_migration_checklist.md](16_linux_migration_checklist.md) | Reference | Linux移行チェックリスト |

---

## Digitizer Implementation (2026-01-23~)

**Spec:** `docs/digitizer_system_spec.md`
**Target:** VX2730 (DPP-PSD2) via Ethernet (`dig2://`)

### Phases

| Phase | Description | Status |
|-------|-------------|--------|
| 1 | FELib Connection Layer | ✅ Complete |
| 2 | DevTree Read/Write | ✅ Complete |
| 3 | Config Storage & Apply (MongoDB) | ✅ Complete |
| 4 | Data Acquisition | ✅ Complete |
| 5 | Reader + Master/Slave + PSD1 | ✅ Complete (Master/Slave ✅, PSD1 全Phase ✅) ← **MVP完了ライン** |
| 6 | Web UI Settings | ✅ Complete |
| 7 | Future (Templates, Monitoring) | Future |

### Principles
- **KISS:** 最小限の抽象化、動くコードを最短経路で
- **TDD:** テストファーストで実装
- **Clean Architecture:** 依存は内向き（KISSと競合時はKISS優先）

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
- PSD2 デコーダ バグフィックス (single-word event, special event, STOP signal, flags, waveform)
- PSD2 実機動作確認 (VX2730, ch4 パルサー 10kHz)
- ヒストグラム表示修正 (max-value downsampling for sub-pixel bar rendering)
- PSD1 デコーダ実装 + Reader統合 + DT5730B 実機検証 (10kHz パルサー, 全パラメータ適用成功)
- データ出力検証 (E2E テスト + recover validate/dump + ROOT マクロ)

---

## Archived

| Directory | Contents |
|-----------|----------|
| `archive/phase1_basic_pipeline/` | 基本パイプライン設計 |
| `archive/phase1_components/` | CLIリファクタリング、CAEN FFI、Monitor、Merger |
| `archive/phase1_control_system/` | コントロールシステム設計 |
| `archive/phase2_infrastructure/` | タイムスタンプソート、Metrics API、Source設定管理 |
| `archive/phase3_psd_decoders/` | PSD2 バグフィックス、PSD1 デコーダ実装+実機検証 |
| `archive/phase4_data_verification/` | データ出力検証 (E2E テスト、recover dump、ROOT マクロ) |

---

## Notes

- **MVP目標:** 2026年3月中旬
- **現在のフェーズ:** Phase 6 実装完了 / PSD2 実機動作確認済み / PSD1 全Phase完了 / データ出力検証完了
- **実機確認済み:** VX2730 (Serial: 52622, DPP_PSD2, 32ch), DT5730B (Serial: 990, DPP_PSD1, 8ch, USB)
- **動作環境:** Linux (Ubuntu, Rust 1.93.0) - Mac から移行済み

## Reference Documents

| Document | Location | Priority |
|----------|----------|----------|
| **x2730 DPP-PSD CUP Documentation** | `legacy/documentation_2024092000-2/` | ★★★ |
| FELib User Guide | `legacy/GD9764_FELib_User_Guide.pdf` | ★★ |
| Digitizer System Spec | `docs/digitizer_system_spec.md` | ★★★ |
