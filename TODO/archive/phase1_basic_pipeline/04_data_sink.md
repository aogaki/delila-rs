# 04: DataSink (Data Consumer)

## Goal
Create a ZeroMQ subscriber that receives EventData and outputs to console.

## Architecture
```
              ZMQ SUB      ┌──────────┐
tcp://localhost:5555 ────► │ DataSink │ ──► Console
                           └──────────┘
```

## Tasks
- [x] Create `src/data_sink/mod.rs`
- [x] Implement `DataSink` struct with SUB socket
- [x] Implement `DataSinkConfig` with default values
- [x] Deserialize received MessagePack to MinimalEventDataBatch
- [x] Track statistics (events/sec, total count, lost batches)
- [x] Create `src/bin/data_sink.rs` executable
- [x] Unit tests for Stats tracking

## Implemented Components
```rust
pub struct DataSinkConfig {
    pub address: String,           // "tcp://localhost:5555"
    pub stats_interval_secs: u64,  // 1
}

pub struct DataSink {
    config: DataSinkConfig,
    socket: tmq::subscribe::Subscribe,
    stats: Stats,
}

struct Stats {
    total_batches: u64,
    total_events: u64,
    last_sequence: Option<u64>,
    lost_batches: u64,
    // ... timing fields
}
```

## Console Output Format
```
Events: 1000 total (1000/s avg, 1000/s current) | Batches: 10 | Lost: 0
```

## Usage
```bash
# Terminal 1: Start emulator
cargo run --bin emulator

# Terminal 2: Start data sink
cargo run --bin data_sink

# With debug logging
RUST_LOG=debug cargo run --bin data_sink
```

## Test Results
```
test data_sink::tests::default_config ... ok
test data_sink::tests::stats_tracking ... ok
test data_sink::tests::stats_lost_detection ... ok
```

## Status: COMPLETED
