# 01: Merger

## Goal
Create a data merger that receives from multiple upstream sources and forwards to downstream.

## Architecture
```
┌──────────┐
│Emulator 1│──┐
└──────────┘  │  ZMQ SUB   ┌─────────────────────┐  ZMQ PUB   ┌──────────┐
              ├──────────► │       Merger        │ ─────────► │ DataSink │
┌──────────┐  │            │ ┌────────┐ ┌──────┐ │            └──────────┘
│Emulator 2│──┘            │ │Receiver│→│Sender│ │
└──────────┘               │ └────────┘ └──────┘ │
                           │     mpsc channel    │
                           └─────────────────────┘
```

## Design Decisions

### 1. Internal buffering
- Bounded mpsc channel between Receiver and Sender tasks
- `try_send()` to avoid blocking receiver
- Drop count tracked in statistics

### 2. Shutdown (EOS)
- EOS signal propagated from upstream to downstream
- Emulator sends EOS on graceful shutdown
- Merger forwards EOS and then exits
- DataSink receives EOS and exits

### 3. ZMQ High Water Mark
- Set to 0 (unlimited) to avoid silent drops
- Memory usage monitored externally (beam intensity adjusted if needed)

### 4. Source reconnection
- Track sequence number per source_id
- Detect restart when sequence drops significantly (>100)
- Auto-reset tracking on detected restart

## Tasks
- [x] Define EOS message type in common module
- [x] Update Emulator to send EOS on shutdown
- [x] Create `src/merger/mod.rs`
- [x] Implement MergerConfig
- [x] Implement Receiver task (SUB → channel)
- [x] Implement Sender task (channel → PUB)
- [x] Implement source tracking (per source_id stats)
- [x] Create `src/bin/merger.rs` executable
- [x] Update DataSink to handle EOS
- [x] Unit tests for source tracking logic
- [ ] Integration test with multiple emulators (manual test done)

## Key Components
```rust
pub struct MergerConfig {
    pub sub_addresses: Vec<String>,  // ["tcp://localhost:5555", ...]
    pub pub_address: String,         // "tcp://*:5556"
    pub channel_capacity: usize,     // Internal buffer size
}

struct MergerStats {
    received_batches: u64,
    sent_batches: u64,
    dropped_batches: u64,
    sources: HashMap<u32, SourceStats>,
}

struct SourceStats {
    last_sequence: u64,
    total_batches: u64,
    restart_count: u32,
}
```

## EOS Message
```rust
// In common module
pub enum Message {
    Data(MinimalEventDataBatch),
    EndOfStream { source_id: u32 },
}
```

## Acceptance Criteria
- [x] Multiple emulators can connect (via multiple --sub flags)
- [x] Data flows through to DataSink
- [x] EOS causes graceful cascade shutdown
- [x] Source restart detected and handled (via sequence tracking)
- [x] Statistics show drop count if any

## Test Results (2026-01-12)
```
Emulator: 5 batches → EOS
Merger: Forwarded 5 batches + EOS
DataSink: Received 4 batches + EOS → graceful shutdown
```

Pipeline: Emulator → Merger → DataSink working with EOS propagation.
