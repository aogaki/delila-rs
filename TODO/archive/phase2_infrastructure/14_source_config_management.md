# Source Configuration Management

**Created:** 2026-01-20
**Status: COMPLETED** (2026-01-20)

## Overview

Sourceコンポーネント（Emulator/Digitizer）の設定管理を改善する。

## Completed Tasks

### Phase 1: SourceType Enum ✅

- [x] `SourceType` enum を追加
  - Emulator (default)
  - Psd1, Psd2, Pha1, Zle
- [x] `source_type` フィールドを `SourceNetworkConfig` に追加
- [x] `is_digitizer()`, `is_emulator()` メソッド更新
- [x] テスト追加

### Phase 2: Emulator Runtime Config ✅

- [x] `EmulatorRuntimeConfig` struct 追加 (`command.rs`)
- [x] `UpdateEmulatorConfig` コマンド追加
- [x] `RuntimeSettings` struct (lock-free atomics)
- [x] `on_update_emulator_config` トレイトメソッド
- [x] Operator API経由でZMQコマンド送信

### Phase 3: config_file Field ✅

- [x] `config_file: Option<String>` を `SourceNetworkConfig` に追加
- [x] `DigitizerConfig::load()` メソッド追加
- [x] `DigitizerConfig::save()` メソッド追加
- [x] `DigitizerConfigError` エラー型追加
- [x] `load_digitizer_config()` ヘルパーメソッド
- [x] `load_digitizer_config_required()` ヘルパーメソッド
- [x] テスト追加

## Implementation Summary

### Files Modified

- `src/config/mod.rs`
  - `SourceType` enum (Emulator, Psd1, Psd2, Pha1, Zle)
  - `config_file` field in `SourceNetworkConfig`
  - `load_digitizer_config()`, `load_digitizer_config_required()`
  - `ConfigError::DigitizerConfigError` variant

- `src/config/digitizer.rs`
  - `DigitizerConfigError` enum
  - `DigitizerConfig::load()` (JSON file)
  - `DigitizerConfig::save()` (JSON file)

- `src/common/command.rs`
  - `EmulatorRuntimeConfig` struct
  - `Command::UpdateEmulatorConfig` variant

- `src/common/state.rs`
  - `on_update_emulator_config` trait method
  - `handle_command` に UpdateEmulatorConfig 処理追加

- `src/data_source_emulator/mod.rs`
  - `RuntimeSettings` struct (atomic variables)
  - `EmulatorCommandExt::on_update_emulator_config` 実装
  - `generate_batch()`, `generate_waveform()` がランタイム設定を使用

- `src/operator/routes.rs`
  - `update_emulator_settings` が ZMQ コマンドを送信

- `config.toml`
  - `type = "emulator"` フィールド追加

### Usage Example

```toml
# config.toml
[[network.sources]]
id = 0
name = "digitizer-0"
type = "psd2"                                    # SourceType
config_file = "config/digitizers/digitizer_0.json"  # DigitizerConfig path
bind = "tcp://*:5555"
command = "tcp://*:5560"
digitizer_url = "dig2://172.18.4.56"
pipeline_order = 1
```

```json
// config/digitizers/digitizer_0.json
{
  "digitizer_id": 0,
  "name": "LaBr3 Digitizer",
  "firmware": "PSD2",
  "num_channels": 32,
  "board": {
    "start_source": "SWcmd"
  },
  "channel_defaults": {
    "enabled": "True",
    "dc_offset": 20.0,
    "polarity": "Negative",
    "trigger_threshold": 500
  }
}
```

## Related Tasks

以下の項目は `TODO/15_digitizer_implementation.md` に移行:
- MongoDB-based configuration management → Phase 3
- Web UI for digitizer configuration editing → Phase 6
- Per-digitizer settings page → Phase 6
