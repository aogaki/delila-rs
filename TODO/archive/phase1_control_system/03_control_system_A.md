# 03: Control System - Phase A (Minimal)

**Status: COMPLETED** (2026-01-12)

## Goal
Add basic command/response control to components with 2-state machine (Idle ↔ Running).

## Architecture
```
┌────────────┐  REQ   ┌─────────────────────────────┐
│ Controller │ ──────►│ Component (Emulator/Merger) │
│            │◄────── │   [Idle] ◄──► [Running]     │
└────────────┘  REP   └─────────────────────────────┘
```

## Design Decisions

### 1. Command/Response Protocol
- JSON serialization (human-readable, easy debugging)
- REQ/REP pattern (synchronous, guaranteed response)
- Each component listens on a dedicated command port

### 2. Minimal State Machine
```
┌──────┐  Start   ┌─────────┐
│ Idle │ ───────► │ Running │
└──────┘ ◄─────── └─────────┘
           Stop
```

### 3. Component Changes
- Add `command_address` to config
- Add REP socket for commands (separate task)
- Add `ComponentState` enum
- Use `watch` channel for state synchronization between tasks

## Tasks
- [x] Define `Command` enum in common module
- [x] Define `CommandResponse` struct in common module
- [x] Define `ComponentState` enum (Idle, Running)
- [x] Add `command_address` to EmulatorConfig
- [x] Add `command_address` to MergerConfig
- [x] Add `command_address` to DataSinkConfig
- [x] Implement command handler in Emulator
- [x] Implement command handler in Merger
- [x] Implement command handler in DataSink
- [x] Create `src/bin/controller.rs` CLI tool
- [x] Update config.example.toml with command ports
- [x] Integration test: Controller → Emulator → Merger → DataSink

## Implementation Summary

### Files Created/Modified
- `src/common/command.rs` - Command, CommandResponse, ComponentState types
- `src/common/mod.rs` - Added command module export
- `src/data_source_emulator/mod.rs` - Command task with watch channel
- `src/merger/mod.rs` - Command task with sequence tracking
- `src/data_sink/mod.rs` - Command task with sequence tracking
- `src/bin/controller.rs` - CLI tool for sending commands
- `config.example.toml` - Added command port documentation

### Key Design Patterns
1. **Separate command task**: Each component spawns a dedicated tokio task for command handling
2. **Watch channel**: State changes propagated via `tokio::sync::watch`
3. **tmq REQ/REP state machine**: `recv() -> (Multipart, Sender)`, `send() -> Receiver`
4. **Sequence tracking**: Merger and DataSink track per-source sequences for gap detection

### Port Allocation
- 5555-5559: Data ports (PUB/SUB)
- 5560-5569: Source command ports (REQ/REP)
- 5570-5579: Merger command ports (REQ/REP)
- 5580-5589: Recorder/Sink command ports (REQ/REP)

## Test Results
Full pipeline test (Emulator → Merger → DataSink):
- 3383 batches processed
- 338,300 events received
- 0 gaps, 0 missing sequences
- All commands (Start/Stop/GetStatus) work correctly

## Acceptance Criteria
- [x] Components start in Idle state (no data flow)
- [x] `Start` command transitions to Running (data flows)
- [x] `Stop` command transitions to Idle (data stops)
- [x] `GetStatus` returns current state and statistics
- [x] Controller can orchestrate all components

## Usage
```bash
# Check status
cargo run --bin controller -- status tcp://localhost:5560

# Start component
cargo run --bin controller -- start tcp://localhost:5560

# Stop component
cargo run --bin controller -- stop tcp://localhost:5560
```

## Notes
- This is the foundation for Phase B (full state machine)
- Keep command handling simple - no timeouts or retries yet
- EOS handling remains unchanged (graceful shutdown within Running state)
