# 03: Emulator (Data Source)

## Goal
Create a dummy data generator that publishes EventData via ZeroMQ.

## Architecture
```
┌──────────┐    ZMQ PUB
│ Emulator │ ──────────► tcp://*:5555
└──────────┘
```

## Tasks
- [x] Create `src/data_source_emulator/mod.rs`
- [x] Implement `Emulator` struct with configurable rate
- [x] Implement `EmulatorConfig` with default values
- [x] Generate random MinimalEventData with realistic values
- [x] Publish batches via tmq PUB socket (async)
- [x] Create `src/bin/emulator.rs` executable
- [x] Graceful shutdown on Ctrl+C via broadcast channel

## Implemented Components
```rust
pub struct EmulatorConfig {
    pub address: String,           // "tcp://*:5555"
    pub source_id: u32,
    pub events_per_batch: usize,   // 100
    pub batch_interval_ms: u64,    // 100ms
    pub num_modules: u8,
    pub channels_per_module: u8,
}

pub struct Emulator {
    config: EmulatorConfig,
    socket: tmq::publish::Publish,
    sequence_number: u64,
    timestamp_ns: f64,
}
```

## Key Features
- Async/await with tokio
- tmq (ZeroMQ) PUB socket
- MessagePack serialization via rmp-serde
- tracing for structured logging
- Graceful shutdown via tokio::sync::broadcast

## Usage
```bash
cargo run --bin emulator
# Set RUST_LOG=debug for verbose output
RUST_LOG=debug cargo run --bin emulator
```

## Status: COMPLETED
