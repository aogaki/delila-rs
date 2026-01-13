# 04: Control System - Phase B (Full State Machine)

**Status: COMPLETED** (2026-01-12)

## Goal
Extend Phase A with 5-state machine and Configure/Arm commands for proper DAQ lifecycle.

## Prerequisites
- Phase A completed (03_control_system_A.md)

## Architecture
```
┌────────────┐  REQ   ┌─────────────────────────────────────────┐
│ Controller │ ──────►│              Component                   │
│            │◄────── │  [Idle]→[Configured]→[Armed]→[Running]  │
└────────────┘  REP   │              ↑          ↓               │
                      │              └──[Error]←┘               │
                      └─────────────────────────────────────────┘
```

## State Machine
```
┌──────────────────────────────────────────────────────────────────┐
│                                                                  │
│   ┌──────┐    Configure    ┌────────────┐                       │
│   │ Idle │ ──────────────► │ Configured │ ◄─────────┐           │
│   └──────┘                 └────────────┘           │           │
│       ▲                          │                  │           │
│       │                          │ Arm              │ Stop      │
│       │ Reset                    ▼                  │           │
│       │                    ┌──────────┐             │           │
│       │                    │  Armed   │             │           │
│       │                    └──────────┘             │           │
│       │                          │                  │           │
│       │                          │ Start            │           │
│       │                          ▼                  │           │
│       │                    ┌──────────┐             │           │
│       │                    │ Running  │ ────────────┘           │
│       │                    └──────────┘                         │
│       │                          │                              │
│       │                          │ (on error)                   │
│       │                          ▼                              │
│       │                    ┌──────────┐                         │
│       └─────────────────── │  Error   │                         │
│                            └──────────┘                         │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

## Tasks
- [x] Extend `ComponentState` enum (add Configured, Armed, Error)
- [x] Extend `Command` enum (add Configure, Arm, Reset)
- [x] Implement state transition validation
- [x] Add `RunConfig` struct (run_number, settings)
- [x] Implement Configure handler (load/validate settings)
- [x] Implement Arm handler (prepare for acquisition)
- [x] Implement Reset handler (return to Idle, clear errors)
- [x] Add error state handling
- [x] Update Controller CLI with new commands
- [x] Update all component binaries

## Implementation Summary

### Files Modified
- `src/common/command.rs` - Extended ComponentState, Command, CommandResponse with RunConfig
- `src/common/mod.rs` - Re-exported RunConfig
- `src/data_source_emulator/mod.rs` - 5-state command handler
- `src/merger/mod.rs` - 5-state command handler with run_config
- `src/data_sink/mod.rs` - 5-state command handler with run_config
- `src/bin/controller.rs` - New commands: configure, arm, reset
- `scripts/daq_ctl.sh` - Updated for 5-state workflow

### Key Design Decisions
1. **Stop returns to Configured** - Allows quick restart without re-configuring
2. **Reset clears everything** - Returns to Idle, clears run_config and stats
3. **State transition validation** - `ComponentState::can_transition_to()` method
4. **Run number tracking** - CommandResponse includes run_number field

## Controller CLI Usage
```bash
# Full run sequence
controller configure tcp://localhost:5560 --run 123
controller arm tcp://localhost:5560
controller start tcp://localhost:5560
# ... data acquisition ...
controller stop tcp://localhost:5560

# Quick restart (from Configured)
controller arm tcp://localhost:5560
controller start tcp://localhost:5560

# Error recovery / full reset
controller reset tcp://localhost:5560
```

## Script Usage
```bash
./scripts/daq_ctl.sh configure --run 123
./scripts/daq_ctl.sh arm
./scripts/daq_ctl.sh start
./scripts/daq_ctl.sh stop
./scripts/daq_ctl.sh reset
```

## State Descriptions

| State | Description | Valid Commands |
|-------|-------------|----------------|
| Idle | Initial state, no configuration | Configure, GetStatus |
| Configured | Config loaded, ready to arm | Arm, Reset, GetStatus |
| Armed | Hardware ready, waiting for start | Start, Reset, GetStatus |
| Running | Actively acquiring data | Stop, GetStatus |
| Error | Recoverable error occurred | Reset, GetStatus |

## Acceptance Criteria
- [x] Full state machine with valid transitions only
- [x] Invalid transitions return error with clear message
- [x] Configure accepts run_number parameter
- [x] Arm prepares component (placeholder for Phase 2 hardware)
- [x] Reset clears error state and returns to Idle
- [x] Controller orchestrates full Configure→Arm→Start→Stop cycle

## Test Results (2026-01-12)
```
Full lifecycle test:
1. configure --run 123: All → Configured (run=123)
2. arm: All → Armed (run=123)
3. start: All → Running (run=123)
4. [5 sec data acquisition]
5. stop: All → Configured (run=123)
6. Invalid start from Configured: Error "Cannot start from Configured state"
7. arm + start: Quick restart works
8. reset: All → Idle (stats cleared)

Performance: 30.3 MHz event rate maintained
```

## Notes
- Armed state is preparation for Two-Phase Start (Phase C)
- Error state allows recovery without full restart
- This phase prepares structure for CAEN driver integration
