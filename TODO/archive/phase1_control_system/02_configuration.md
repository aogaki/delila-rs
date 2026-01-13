# 02: Configuration System

## Goal
Create a flexible configuration system that supports multiple sources:
1. TOML file (network topology, infrastructure) - DONE
2. MongoDB (operational settings, per-run parameters) - future

## Design

### Configuration Hierarchy
```
┌─────────────────────────────────────────────────┐
│                 ConfigLoader                     │
│  ┌─────────────┐    ┌─────────────┐             │
│  │  TOML File  │    │   MongoDB   │  (future)   │
│  │  (network)  │    │ (settings)  │             │
│  └──────┬──────┘    └──────┬──────┘             │
│         │                  │                     │
│         └────────┬─────────┘                     │
│                  ▼                               │
│           Merged Config                          │
└─────────────────────────────────────────────────┘
```

### TOML Structure
See `config.example.toml` for full example.

## Tasks
- [x] Add `toml` dependency to Cargo.toml
- [x] Create `src/config/mod.rs` module
- [x] Define NetworkConfig structs
- [x] Define SettingsConfig structs
- [x] Implement TOML loader
- [x] Update emulator binary to use config
- [x] Update merger binary to use config
- [x] Update data_sink binary to use config
- [ ] Implement MongoDB loader (future)

## Usage

### Command Line
```bash
# Use config file
emulator --config config.toml --source-id 1
merger --config config.toml
data_sink --config config.toml

# Override with CLI args
emulator --config config.toml --source-id 1 --batches 100
merger --config config.toml --pub tcp://*:9999
```

### Config File Priority
1. CLI arguments (highest priority)
2. TOML config file
3. Default values (lowest priority)

## Files Created
- `src/config/mod.rs` - Configuration module
- `config.example.toml` - Example configuration
- `config.toml` - Working configuration (git-ignored)

## Test Results (2026-01-12)
```
Emulator 1: Loaded config.toml, source_id=1, tcp://*:5555
Emulator 2: Loaded config.toml, source_id=2, tcp://*:5556
Merger: Loaded config.toml, subscribe=[5555,5556], publish=5557
DataSink: Loaded config.toml, subscribe=5557
```

All components successfully loading network topology from shared config file.
