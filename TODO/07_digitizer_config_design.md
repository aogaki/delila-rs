# Digitizer Configuration Design

**Created:** 2026-01-13
**Status:** COMPLETED (2026-01-13)

## Overview

Design digitizer configuration system following approach B:
- Separate JSON files for digitizer settings
- REST API for configuration management
- Settings persistence: manual save + shutdown time

## Implementation Summary

### Files Created/Modified

1. **`src/config/digitizer.rs`** - New file with data structures:
   - `DigitizerConfig` - Main configuration struct
   - `FirmwareType` - Enum for PSD1/PSD2/PHA
   - `BoardConfig` - Board-level parameters
   - `ChannelConfig` - Channel parameters with defaults + overrides pattern
   - `CaenParameter` - Path-value pair for CAEN API
   - `to_caen_parameters()` - Generates CAEN parameter list

2. **`src/config/mod.rs`** - Updated to export digitizer module

3. **`src/operator/routes.rs`** - Extended with:
   - `AppState` extended with `digitizer_configs: RwLock<HashMap<u32, DigitizerConfig>>`
   - New REST API endpoints for digitizer configuration

### REST API Endpoints

```
GET  /api/digitizers                    # List all digitizer configs
GET  /api/digitizers/{id}               # Get specific digitizer config
PUT  /api/digitizers/{id}               # Update digitizer config (in memory)
POST /api/digitizers/{id}/save          # Save config to JSON file
```

### Key Design Decisions

1. **All parameter values are String** - Not enum
   - CAEN FELib validates at SetValue time
   - DevTree provides valid choices dynamically
   - Different firmware versions have different valid values

2. **Defaults + Overrides pattern**
   - `channel_defaults` applies to all channels
   - `channel_overrides` contains only channels that differ

3. **Firmware-aware parameter names**
   - PSD1: `ch_enabled`, `ch_dcoffset`, `POLARITY_NEGATIVE`
   - PSD2: `ChEnable`, `DCOffset`, `Negative`

## Implementation Tasks

- [x] Create `src/config/digitizer.rs` with data structures
- [x] Add JSON serialization tests
- [x] Add `to_caen_parameters()` method
- [x] Extend `AppState` with digitizer config storage
- [x] Add REST API routes for digitizer config
- [x] Implement config file load/save
- [ ] Add "apply to hardware" logic via Reader component (future)
- [ ] Test with real DevTree parameters (future)

## File Structure

```
config/
├── config.toml           # Network topology (existing)
└── digitizers/
    ├── digitizer_0.json  # Digitizer 0 settings
    ├── digitizer_1.json  # Digitizer 1 settings
    └── ...
```

## Example JSON Configuration

```json
{
    "digitizer_id": 0,
    "name": "LaBr3 Digitizer",
    "firmware": "PSD2",
    "num_channels": 32,
    "board": {
        "start_source": "SWcmd",
        "gpio_mode": "Run",
        "test_pulse_period": 10000,
        "global_trigger_source": "TestPulse"
    },
    "channel_defaults": {
        "enabled": "True",
        "dc_offset": 20.0,
        "polarity": "Negative",
        "trigger_threshold": 500,
        "gate_long_ns": 400,
        "gate_short_ns": 100
    },
    "channel_overrides": {
        "0": { "trigger_threshold": 300 },
        "1": { "enabled": "False" }
    }
}
```

## Test Results

All 47 tests passed including 7 new digitizer config tests:
- `test_new_digitizer_config`
- `test_psd1_has_8_channels`
- `test_serialize_deserialize`
- `test_get_channel_config_with_override`
- `test_to_caen_parameters_psd2`
- `test_to_caen_parameters_psd1`
- `test_json_example_config`

## References

- DELILA2/PSD2.conf - Example configuration
- DELILA2/lib/digitizer/DevTree/PSD2.json - Full parameter definitions
- docs/architecture/config_and_deployment.md - Architecture design
