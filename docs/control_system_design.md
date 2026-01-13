# DELILA Control System Design

This document describes the control and orchestration architecture for the DELILA DAQ system, based on the C++ reference implementation in DELILA2/.

## Overview

The DELILA control system manages distributed DAQ components through:
1. **State Machine** - Enforces valid component lifecycle transitions
2. **Three-Channel Communication** - Separates data, status, and commands
3. **Heartbeat System** - Detects component failures
4. **Two-Phase Start** - Synchronizes hardware across nodes

## 1. Component State Machine

### States

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
│       │                          │ Error                        │
│       │                          ▼                              │
│       │                    ┌──────────┐                         │
│       └─────────────────── │  Error   │                         │
│                            └──────────┘                         │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

### State Descriptions

| State | Description |
|-------|-------------|
| **Idle** | Initial state, no configuration loaded |
| **Configured** | Configuration loaded, ready to arm |
| **Armed** | Hardware prepared, waiting for synchronized start |
| **Running** | Actively acquiring/processing data |
| **Error** | Recoverable error state, requires Reset |

### Transitional States (Optional)

For async operations, intermediate states can be used:
- `Configuring` - Configuration in progress
- `Arming` - Hardware preparation in progress
- `Starting` - Acquisition starting
- `Stopping` - Graceful shutdown in progress

### Valid Transitions

```rust
fn is_valid_transition(from: State, to: State) -> bool {
    match (from, to) {
        // Normal flow
        (Idle, Configured) => true,       // Configure
        (Configured, Armed) => true,      // Arm
        (Armed, Running) => true,         // Start
        (Running, Configured) => true,    // Stop

        // Reset (from any state)
        (_, Idle) => true,

        // Error (from any state)
        (_, Error) => true,

        _ => false,
    }
}
```

## 2. Three-Channel Communication

### Channel Overview

```
┌─────────────┐                              ┌─────────────┐
│  Component  │                              │  Operator   │
│             │                              │             │
│  ┌───────┐  │  ══════ Data Channel ══════► │             │
│  │ Data  │  │     PUB/SUB, High throughput │             │
│  │ (PUB) │  │                              │             │
│  └───────┘  │                              │             │
│             │                              │             │
│  ┌───────┐  │  ══════ Status Channel ════► │  ┌───────┐  │
│  │Status │  │     PUB/SUB, Periodic        │  │Monitor│  │
│  │ (PUB) │  │                              │  │ (SUB) │  │
│  └───────┘  │                              │  └───────┘  │
│             │                              │             │
│  ┌───────┐  │  ◄═════ Command Channel ═══► │  ┌───────┐  │
│  │Command│  │     REQ/REP, On-demand       │  │Control│  │
│  │ (REP) │  │                              │  │ (REQ) │  │
│  └───────┘  │                              │  └───────┘  │
└─────────────┘                              └─────────────┘
```

### Data Channel

- **Pattern**: PUB/SUB (or PUSH/PULL for guaranteed delivery)
- **Direction**: Component → Downstream
- **Frequency**: High throughput (millions of events/sec)
- **Content**: Event data, Heartbeat, EndOfStream messages
- **Serialization**: MessagePack + optional LZ4 compression

```rust
enum DataMessage {
    Data(EventBatch),
    Heartbeat { source_id: u32, timestamp: u64 },
    EndOfStream { source_id: u32 },
}
```

### Status Channel

- **Pattern**: PUB/SUB
- **Direction**: Component → Operator
- **Frequency**: Periodic (1-10 Hz typical)
- **Content**: Component health and metrics
- **Serialization**: JSON

```rust
struct ComponentStatus {
    component_id: String,      // "emulator_host1_0"
    state: ComponentState,
    timestamp: u64,
    run_number: Option<u32>,
    metrics: ComponentMetrics,
    error_message: Option<String>,
    heartbeat_counter: u64,
}

struct ComponentMetrics {
    events_processed: u64,
    bytes_transferred: u64,
    queue_size: u32,
    queue_max: u32,
    event_rate: f64,       // events/sec
    data_rate: f64,        // MB/sec
}
```

### Command Channel

- **Pattern**: REQ/REP
- **Direction**: Bidirectional (Operator ↔ Component)
- **Frequency**: On-demand (rare, only for control)
- **Content**: Commands and responses
- **Serialization**: JSON

```rust
enum CommandType {
    Configure,    // Load configuration
    Arm,          // Prepare hardware (two-phase start phase 1)
    Start,        // Begin acquisition (two-phase start phase 2)
    Stop,         // End acquisition
    Reset,        // Return to Idle state
    GetStatus,    // Query current status
    Ping,         // Check if alive
}

struct Command {
    command_type: CommandType,
    request_id: u32,           // Correlation ID
    config_path: Option<String>,
    run_number: Option<u32>,
    graceful: bool,            // For Stop: flush data first
    payload: Option<String>,   // Additional JSON
}

struct CommandResponse {
    request_id: u32,
    success: bool,
    error_code: ErrorCode,
    current_state: ComponentState,
    message: String,
    payload: Option<String>,
}
```

## 3. Heartbeat System

### Purpose

Detect component failures or network issues without blocking data flow.

### Sender Side (HeartbeatManager)

Components send heartbeat messages when no data is available:

```rust
struct HeartbeatManager {
    interval: Duration,        // 100ms typical
    last_sent: Instant,
}

impl HeartbeatManager {
    fn is_due(&self) -> bool {
        self.last_sent.elapsed() >= self.interval
    }

    fn mark_sent(&mut self) {
        self.last_sent = Instant::now();
    }
}

// Usage in data loop:
loop {
    if let Some(data) = try_get_data() {
        send_data(data);
        heartbeat.mark_sent();  // Reset timer on any send
    } else if heartbeat.is_due() {
        send_heartbeat();
        heartbeat.mark_sent();
    }
}
```

### Receiver Side (HeartbeatMonitor)

Monitors track last message time per source:

```rust
struct HeartbeatMonitor {
    timeout: Duration,         // 6 seconds typical
    sources: HashMap<String, Instant>,
}

impl HeartbeatMonitor {
    fn update(&mut self, source_id: &str) {
        self.sources.insert(source_id.to_string(), Instant::now());
    }

    fn is_timed_out(&self, source_id: &str) -> bool {
        self.sources.get(source_id)
            .map(|t| t.elapsed() > self.timeout)
            .unwrap_or(true)
    }

    fn get_timed_out_sources(&self) -> Vec<String> {
        self.sources.iter()
            .filter(|(_, t)| t.elapsed() > self.timeout)
            .map(|(id, _)| id.clone())
            .collect()
    }
}
```

### Timeout Handling

When a component times out:
1. Log warning with component ID
2. Mark component as potentially failed
3. Operator can decide: retry, skip, or abort run

## 4. Two-Phase Start

### Problem

Multiple digitizers must start acquisition simultaneously. Network latency makes simple "send start to all" unreliable.

### Solution

Two-phase commit pattern:

```
┌──────────┐     ┌────────────┐     ┌────────────┐
│ Operator │     │ Component1 │     │ Component2 │
└────┬─────┘     └─────┬──────┘     └─────┬──────┘
     │                 │                  │
     │  Configure()    │                  │
     │────────────────►│                  │
     │  Configure()    │                  │
     │─────────────────────────────────►│
     │                 │                  │
     │  Configured OK  │                  │
     │◄────────────────│                  │
     │  Configured OK  │                  │
     │◄─────────────────────────────────│
     │                 │                  │
     │  ═══════════════════════════════  │
     │  Phase 1: Arm (prepare hardware)  │
     │  ═══════════════════════════════  │
     │                 │                  │
     │  Arm()          │                  │
     │────────────────►│ (prepare HW)    │
     │  Arm()          │                  │
     │─────────────────────────────────►│ (prepare HW)
     │                 │                  │
     │  Armed OK       │                  │
     │◄────────────────│                  │
     │  Armed OK       │                  │
     │◄─────────────────────────────────│
     │                 │                  │
     │  ══════════════════════════════════════════  │
     │  SYNC POINT: All components armed             │
     │  ══════════════════════════════════════════  │
     │                 │                  │
     │  ═══════════════════════════════  │
     │  Phase 2: Start (begin together)  │
     │  ═══════════════════════════════  │
     │                 │                  │
     │  Start()        │                  │
     │────────────────►│ (start acq)     │
     │  Start()        │                  │
     │─────────────────────────────────►│ (start acq)
     │                 │                  │
     │  Running OK     │                  │
     │◄────────────────│                  │
     │  Running OK     │                  │
     │◄─────────────────────────────────│
     │                 │                  │
```

### Implementation

```rust
// Operator side
async fn start_run(&self, run_number: u32) -> Result<()> {
    // Phase 1: Configure all
    self.send_command_to_all(Command::Configure).await?;
    self.wait_all_in_state(State::Configured).await?;

    // Phase 2: Arm all (prepare hardware)
    self.send_command_to_all(Command::Arm).await?;
    self.wait_all_in_state(State::Armed).await?;

    // SYNC POINT: All armed, ready to start

    // Phase 3: Start all (begin acquisition)
    self.send_command_to_all(Command::Start { run_number }).await?;
    self.wait_all_in_state(State::Running).await?;

    Ok(())
}
```

## 5. Error Handling

### Error Codes

```rust
enum ErrorCode {
    Success = 0,

    // Configuration (100-199)
    InvalidConfiguration = 100,
    ConfigurationNotFound = 101,

    // State (200-299)
    InvalidStateTransition = 200,
    NotConfigured = 201,
    NotArmed = 202,
    AlreadyRunning = 203,

    // Hardware (300-399)
    HardwareNotFound = 300,
    HardwareConnectionFailed = 301,
    HardwareTimeout = 302,

    // Communication (400-499)
    CommunicationError = 400,
    Timeout = 401,
    ConnectionLost = 402,

    // Internal (500-599)
    InternalError = 500,
    OutOfMemory = 501,

    Unknown = 999,
}
```

### Error Recovery

1. Component enters `Error` state
2. Status channel reports error message
3. Operator sends `Reset` command
4. Component returns to `Idle` state
5. All configuration cleared, ready for fresh start

## 6. Component Identity

### Naming Convention

```
{component_type}_{hostname}_{index}
```

Examples:
- `emulator_daq01_0` - First emulator on host daq01
- `merger_merger01_0` - Merger on host merger01
- `recorder_storage01_0` - Recorder on storage server

### Address Configuration

```rust
struct ComponentAddress {
    component_id: String,
    data_address: String,      // "tcp://*:5555" (bind)
    status_address: String,    // "tcp://*:5556" (bind)
    command_address: String,   // "tcp://*:5557" (bind)
}
```

## 7. Operator Interface

### Responsibilities

- Register/unregister components
- Send commands to all or specific components
- Monitor component status and heartbeats
- Coordinate two-phase start
- Handle errors and recovery

### API

```rust
trait Operator {
    // Component management
    fn register_component(&mut self, address: ComponentAddress);
    fn unregister_component(&mut self, component_id: &str);
    fn get_component_ids(&self) -> Vec<String>;

    // Commands (async, returns job_id)
    async fn configure_all(&self) -> String;
    async fn arm_all(&self) -> String;
    async fn start_all(&self, run_number: u32) -> String;
    async fn stop_all(&self, graceful: bool) -> String;
    async fn reset_all(&self) -> String;

    // Job tracking
    fn get_job_status(&self, job_id: &str) -> JobStatus;

    // Monitoring
    fn get_all_status(&self) -> Vec<ComponentStatus>;
    fn is_all_in_state(&self, state: ComponentState) -> bool;
}

struct JobStatus {
    job_id: String,
    state: JobState,  // Pending, Running, Completed, Failed
    error_message: Option<String>,
    created_at: Instant,
    completed_at: Option<Instant>,
}
```

## 8. Configuration Example

### TOML Configuration

```toml
[operator]
operator_id = "operator_ctrl01_0"
configure_timeout_ms = 5000
arm_timeout_ms = 10000
start_timeout_ms = 5000
stop_timeout_ms = 30000
command_retry_count = 3

[[components]]
component_id = "emulator_daq01_0"
component_type = "emulator"
data_address = "tcp://daq01:5555"
status_address = "tcp://daq01:5556"
command_address = "tcp://daq01:5557"
start_order = 1

[[components]]
component_id = "emulator_daq02_0"
component_type = "emulator"
data_address = "tcp://daq02:5555"
status_address = "tcp://daq02:5556"
command_address = "tcp://daq02:5557"
start_order = 1

[[components]]
component_id = "merger_merger01_0"
component_type = "merger"
data_address = "tcp://merger01:5560"
status_address = "tcp://merger01:5561"
command_address = "tcp://merger01:5562"
start_order = 2

[[components]]
component_id = "recorder_storage01_0"
component_type = "recorder"
data_address = ""  # No output
status_address = "tcp://storage01:5566"
command_address = "tcp://storage01:5567"
start_order = 3
```

## 9. Typical Run Sequence

```
1. Operator starts, loads configuration
2. Operator registers all components
3. User requests "Start Run 123"
4. Operator: Configure all → wait for Configured
5. Operator: Arm all → wait for Armed (SYNC POINT)
6. Operator: Start all → wait for Running
7. Components stream data with heartbeats
8. User requests "Stop Run"
9. Operator: Stop all (graceful) → wait for Configured
10. Components send EndOfStream, flush buffers
11. Run complete
```

## 10. Summary

| Aspect | Design Choice | Rationale |
|--------|---------------|-----------|
| State Machine | Strict transitions | Prevent invalid operations |
| Data Channel | PUB/SUB | High throughput, non-blocking |
| Status Channel | PUB/SUB | Decoupled monitoring |
| Command Channel | REQ/REP | Synchronous control with feedback |
| Heartbeat | Sender + Monitor | Detect failures without polling |
| Start Sync | Two-phase commit | Hardware synchronization |
| Error Recovery | Reset to Idle | Clean slate approach |
| Component ID | type_host_index | Unique, descriptive naming |

This architecture provides:
- **Reliability**: Heartbeat detection catches failures
- **Synchronization**: Two-phase start aligns hardware
- **Scalability**: PUB/SUB handles many subscribers
- **Recoverability**: Reset clears errors cleanly
- **Observability**: Status channel provides real-time metrics
