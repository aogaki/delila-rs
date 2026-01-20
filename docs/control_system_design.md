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

## 8. Pipeline Ordering for Start/Stop

### Problem

Data pipelines require careful ordering during Start and Stop:
- **Start**: Downstream components (Recorder, Monitor) must be ready before upstream (Sources) begin sending data
- **Stop**: Upstream components (Sources) must stop first to ensure all data flows through before downstream closes

### Solution: `pipeline_order` Field

Each component has a `pipeline_order` field indicating its position in the data flow:

| pipeline_order | Component Type | Position |
|----------------|---------------|----------|
| 1 | Source (Reader/Emulator) | Upstream |
| 2 | Merger | Middle |
| 3 | Recorder, Monitor | Downstream |

### Ordering Behavior

```
Start Order: DESCENDING (downstream first)
  3 → 2 → 1
  Recorder → Merger → Sources

Stop Order: ASCENDING (upstream first)
  1 → 2 → 3
  Sources → Merger → Recorder
```

### Implementation

```rust
/// Start all components in pipeline order (descending: downstream first)
/// IMPORTANT: Sequential start - wait for each component to reach Running
/// before starting the next. This prevents ZMQ buffer overflow.
pub async fn start_all_sequential(
    &self,
    configs: &[ComponentConfig],
    run_number: u32,
    per_component_timeout_ms: u64,
) -> Result<Vec<CommandResult>, String> {
    let mut sorted: Vec<_> = configs.iter().collect();
    sorted.sort_by(|a, b| b.pipeline_order.cmp(&a.pipeline_order));

    for config in sorted {
        // Send start command
        let result = self.start(config, run_number).await;
        if !result.success { return Err(...); }

        // Wait for this component to reach Running state
        self.wait_for_state(&[config], ComponentState::Running, per_component_timeout_ms).await?;
    }
    Ok(results)
}

/// Stop all components in pipeline order (ascending: upstream first)
pub async fn stop_all(&self, configs: &[ComponentConfig]) -> Vec<CommandResult> {
    let mut sorted: Vec<_> = configs.iter().collect();
    sorted.sort_by(|a, b| a.pipeline_order.cmp(&b.pipeline_order));
    // Send Stop command to each in order
}
```

### Sequential Start Importance

Without sequential start:
```
❌ Problem: All Start commands sent quickly
   Recorder receives Start → begins initialization (100ms)
   Emulator receives Start → immediately Running → generates data
   → ZMQ buffer fills up → Memory explosion (10GB+)

✅ Solution: Wait for Running before next
   Recorder receives Start → waits for Running (initialized)
   Monitor receives Start → waits for Running
   Merger receives Start → waits for Running
   Emulator receives Start → now all downstream ready
   → No buffer overflow
```

### Why This Matters

Without proper ordering:
- **Wrong Start order**: Recorder might miss initial data
- **Wrong Stop order**: Data in transit when upstream stops may be lost

Example of data loss with wrong Stop order:
```
❌ Wrong: Stop Recorder first
   Source sends data → Merger forwards → Recorder already stopped = DATA LOST

✅ Correct: Stop Source first
   Source stops → Merger flushes → Recorder receives all → Recorder stops
```

## 9. Configuration Example

### TOML Configuration

```toml
[operator]
operator_id = "operator_ctrl01_0"
configure_timeout_ms = 5000
arm_timeout_ms = 10000
start_timeout_ms = 5000
stop_timeout_ms = 30000
command_retry_count = 3

[[network.sources]]
id = 0
name = "emulator-0"
bind = "tcp://*:5555"
command = "tcp://*:5560"
pipeline_order = 1        # Upstream (data source)

[[network.sources]]
id = 1
name = "emulator-1"
bind = "tcp://*:5556"
command = "tcp://*:5561"
pipeline_order = 1        # Upstream (data source)

[network.merger]
subscribe = ["tcp://localhost:5555", "tcp://localhost:5556"]
publish = "tcp://*:5557"
command = "tcp://*:5570"
pipeline_order = 2        # Middle layer

[network.recorder]
subscribe = "tcp://localhost:5557"
command = "tcp://*:5580"
output_dir = "./data"
pipeline_order = 3        # Downstream (data sink)

[network.monitor]
subscribe = "tcp://localhost:5557"
command = "tcp://*:5590"
http_port = 8080
pipeline_order = 3        # Downstream (data sink)
```

## 10. Typical Run Sequence

```
1. Operator starts, loads configuration (including pipeline_order)
2. Operator registers all components
3. User requests "Start Run 123"
4. Operator: Configure all → wait for Configured
5. Operator: Arm all → wait for Armed (SYNC POINT)
6. Operator: Start all (descending pipeline_order: downstream first)
   - Recorder starts (order 3)
   - Monitor starts (order 3)
   - Merger starts (order 2)
   - Sources start (order 1)
7. Components stream data with heartbeats
8. User requests "Stop Run"
9. Operator: Stop all (ascending pipeline_order: upstream first)
   - Sources stop (order 1) → send EndOfStream
   - Merger stops (order 2) → forwards EndOfStream
   - Recorder stops (order 3) → flushes buffers
   - Monitor stops (order 3)
10. All data written, run complete
```

## 11. Run History (MongoDB Integration)

### Overview

Run history is persisted to MongoDB for:
- Multi-client synchronization (next_run_number)
- Run metadata storage (comment, notes, duration)
- Historical queries (by experiment name)

### Data Model

```rust
struct RunDocument {
    run_number: i32,
    exp_name: String,
    comment: String,
    start_time: DateTime<Utc>,
    end_time: Option<DateTime<Utc>>,
    status: String,        // "running", "completed", "error", "aborted"
    duration_secs: Option<i64>,
    notes: Vec<RunNote>,   // Append-only logbook entries
    stats: Option<RunStats>,
}

struct RunNote {
    time: i64,             // UNIX timestamp in milliseconds
    text: String,
}
```

### API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/runs/next` | GET | Get next available run number |
| `/api/runs/current/note` | POST | Add note to current run |
| `/api/start` | POST | Start run (includes comment) |
| `/api/stop` | POST | End run (updates status, duration) |

### Start Request with Comment

```typescript
// Frontend sends comment with start request
POST /api/start
{
  "run_number": 123,
  "comment": "Beam/target info for this run"
}
```

### Comment Auto-fill

Previous run's comment and notes are provided for the next run:

```
Run N: Comment = "Target A, Beam 10MeV"
       Notes = ["[10:15] Beam unstable", "[10:30] Recovered"]

Run N+1: Suggested Comment =
  "Target A, Beam 10MeV
   ---
   [10:15] Beam unstable
   [10:30] Recovered"
```

### MongoDB Connection

```bash
# Operator startup with MongoDB
./target/release/operator --config config.toml \
    --mongodb-uri "mongodb://user:pass@localhost:27017" \
    --mongodb-database "delila"
```

## 12. Summary

| Aspect | Design Choice | Rationale |
|--------|---------------|-----------|
| State Machine | Strict transitions | Prevent invalid operations |
| Data Channel | PUB/SUB | High throughput, non-blocking |
| Status Channel | PUB/SUB | Decoupled monitoring |
| Command Channel | REQ/REP | Synchronous control with feedback |
| Heartbeat | Sender + Monitor | Detect failures without polling |
| Start Sync | Two-phase commit | Hardware synchronization |
| Pipeline Ordering | `pipeline_order` field | Ensures data integrity at Start/Stop |
| Sequential Start | Wait for Running | Prevents memory explosion |
| Run History | MongoDB | Multi-client sync, persistence |
| Note Timestamp | UNIX ms (i64) | Simple, query-friendly |
| Error Recovery | Reset to Idle | Clean slate approach |
| Component ID | type_host_index | Unique, descriptive naming |

This architecture provides:
- **Reliability**: Heartbeat detection catches failures
- **Synchronization**: Two-phase start aligns hardware
- **Scalability**: PUB/SUB handles many subscribers
- **Recoverability**: Reset clears errors cleanly
- **Observability**: Status channel provides real-time metrics
- **Data Integrity**: Pipeline ordering prevents data loss during Start/Stop
