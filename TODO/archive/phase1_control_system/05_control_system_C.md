# 05: Control System - Phase C (Full Implementation)

**Status: COMPLETED** (2026-01-12)

## Goal
Complete control system with Status Channel, Heartbeat, Two-Phase Start, and Operator.

## Prerequisites
- Phase A completed (03_control_system_A.md)
- Phase B completed (04_control_system_B.md)

## Architecture
```
┌─────────────┐                              ┌─────────────┐
│  Component  │                              │  Operator   │
│             │                              │             │
│  ┌───────┐  │  ══════ Data Channel ══════► │             │
│  │ Data  │  │     PUB/SUB, High throughput │             │
│  │ (PUB) │  │     + Heartbeat messages     │             │
│  └───────┘  │                              │             │
│             │                              │             │
│  ┌───────┐  │  ══════ Status Channel ════► │  ┌───────┐  │
│  │Status │  │     PUB/SUB, Periodic (1Hz)  │  │Monitor│  │
│  │ (PUB) │  │                              │  │ (SUB) │  │
│  └───────┘  │                              │  └───────┘  │
│             │                              │             │
│  ┌───────┐  │  ◄═════ Command Channel ═══► │  ┌───────┐  │
│  │Command│  │     REQ/REP, On-demand       │  │Control│  │
│  │ (REP) │  │                              │  │ (REQ) │  │
│  └───────┘  │                              │  └───────┘  │
└─────────────┘                              └─────────────┘
```

## Tasks

### Status Channel (PARTIALLY COMPLETED 2026-01-12)
- [ ] Add `status_address` to component configs (deferred - using command channel)
- [x] Define `ComponentStatus` struct
- [x] Define `ComponentMetrics` struct
- [ ] Implement periodic status publisher (1Hz) (deferred)
- [x] Add status to Operator API via command channel

### Heartbeat System (PARTIALLY COMPLETED 2026-01-12)
- [x] Define `Heartbeat` message type
- [x] Implement heartbeat sender in Emulator (1Hz configurable)
- [ ] Implement `HeartbeatMonitor` (receiver side) (deferred)
- [x] Integrate heartbeat into data channel (Message enum)
- [ ] Add timeout detection and logging (deferred)

### Two-Phase Start (COMPLETED 2026-01-12)
- [x] Implement sync point in Operator (wait all Armed)
- [ ] Add start_order to component config (deferred - simple sequential order used)
- [x] Implement ordered startup sequence (configure → arm → start with sync)
- [x] Add timeout handling for each phase (5s default per phase)

### Operator Service (COMPLETED 2026-01-12)
- [x] Create `src/operator/mod.rs` module
- [x] Implement component registry
- [x] Implement `configure_all()` with error handling
- [x] Implement `arm_all()` with sync point
- [x] Implement `start_all()` with ordering
- [x] Implement `stop_all()` with graceful option
- [x] Create `src/bin/operator.rs` service

### Web API + Swagger UI (COMPLETED 2026-01-12)
- [x] Add utoipa and utoipa-swagger-ui dependencies
- [x] Add axum REST endpoints with OpenAPI annotations
- [x] GET /api/status - all component status
- [ ] GET /api/components - list registered components (deferred)
- [ ] GET /api/components/{id} - single component detail (deferred)
- [x] POST /api/configure - configure run (run_number)
- [x] POST /api/arm - arm all components
- [x] POST /api/start - start run
- [x] POST /api/stop - stop run
- [x] POST /api/reset - reset all to Idle
- [x] Swagger UI at /swagger-ui/
- [x] OpenAPI JSON at /api-docs/openapi.json
- [ ] WebSocket /ws/status for real-time status updates (deferred)

## Data Structures
```rust
// Status Channel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentStatus {
    pub component_id: String,
    pub state: ComponentState,
    pub timestamp: u64,
    pub run_number: Option<u32>,
    pub metrics: ComponentMetrics,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentMetrics {
    pub events_processed: u64,
    pub bytes_transferred: u64,
    pub queue_size: u32,
    pub queue_max: u32,
    pub event_rate: f64,
    pub data_rate: f64,
}

// Heartbeat
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub source_id: u32,
    pub timestamp: u64,
    pub counter: u64,
}

// Extended Message enum
pub enum Message {
    Data(MinimalEventDataBatch),
    EndOfStream { source_id: u32 },
    Heartbeat(Heartbeat),
}

// Operator config
#[derive(Debug, Clone, Deserialize)]
pub struct OperatorConfig {
    pub operator_id: String,
    pub configure_timeout_ms: u64,
    pub arm_timeout_ms: u64,
    pub start_timeout_ms: u64,
    pub stop_timeout_ms: u64,
    pub heartbeat_timeout_ms: u64,
    pub components: Vec<ComponentAddress>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ComponentAddress {
    pub component_id: String,
    pub component_type: String,
    pub data_address: String,
    pub status_address: String,
    pub command_address: String,
    pub start_order: u32,
}
```

## Two-Phase Start Sequence
```
Operator                    Component1              Component2
   │                            │                       │
   │  Configure(run=123)        │                       │
   ├───────────────────────────►│                       │
   │  Configure(run=123)        │                       │
   ├────────────────────────────────────────────────────►
   │                            │                       │
   │  [Wait all Configured]     │                       │
   │                            │                       │
   │  Arm()                     │                       │
   ├───────────────────────────►│ (prepare)             │
   │  Arm()                     │                       │
   ├────────────────────────────────────────────────────►
   │                            │                       │
   │  ════ SYNC POINT ════      │                       │
   │  [Wait all Armed]          │                       │
   │                            │                       │
   │  Start()                   │                       │
   ├───────────────────────────►│ (begin)               │
   │  Start()                   │                       │
   ├────────────────────────────────────────────────────►
   │                            │                       │
```

## Dependencies (to add to Cargo.toml)
```toml
# Web API
axum = "0.7"
tower-http = { version = "0.5", features = ["cors"] }

# Swagger / OpenAPI
utoipa = { version = "4", features = ["axum_extras"] }
utoipa-swagger-ui = { version = "7", features = ["axum"] }
```

## Swagger UI Example
```rust
use utoipa::{OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

#[derive(ToSchema, Serialize, Deserialize)]
pub struct ComponentStatus {
    /// Unique component identifier
    pub component_id: String,
    /// Current state (Idle, Configured, Armed, Running, Error)
    pub state: ComponentState,
    /// Events processed per second
    pub event_rate: f64,
}

#[utoipa::path(
    get,
    path = "/api/status",
    tag = "monitoring",
    responses(
        (status = 200, description = "All component status", body = Vec<ComponentStatus>)
    )
)]
async fn get_status(State(state): State<AppState>) -> Json<Vec<ComponentStatus>> {
    // ...
}

#[derive(OpenApi)]
#[openapi(
    info(title = "DELILA DAQ API", version = "1.0.0"),
    paths(get_status, get_components, configure_run, arm_all, start_run, stop_run, reset_all),
    components(schemas(ComponentStatus, ComponentState, CommandResponse, StartRunRequest)),
    tags(
        (name = "monitoring", description = "Status and metrics"),
        (name = "control", description = "Run control commands")
    )
)]
struct ApiDoc;

let app = Router::new()
    .route("/api/status", get(get_status))
    // ... other routes
    .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()));
```

## API Endpoints Summary
| Method | Path | Description | Tag |
|--------|------|-------------|-----|
| GET | /api/status | All component status + metrics | monitoring |
| GET | /api/components | List registered components | monitoring |
| GET | /api/components/{id} | Single component detail | monitoring |
| POST | /api/run/configure | Configure run (run_number) | control |
| POST | /api/run/arm | Arm all components | control |
| POST | /api/run/start | Start acquisition | control |
| POST | /api/run/stop | Stop acquisition | control |
| POST | /api/reset | Reset all to Idle | control |
| WS | /ws/status | Real-time status stream | monitoring |

## Acceptance Criteria
- [x] Status channel reports metrics (via command channel GetStatus)
- [ ] Heartbeat detects component timeout (6s default) - deferred
- [x] Two-phase start synchronizes all components
- [x] Operator manages full run lifecycle
- [x] Swagger UI accessible at /swagger-ui/
- [x] API testable via Swagger "Try it out" button
- [ ] All components gracefully handle network failures - deferred

## Implementation Summary

### Files Created/Modified
- `src/operator/mod.rs` - Module with data types (ComponentStatus, SystemStatus, OperatorConfig, etc.)
- `src/operator/client.rs` - ZMQ client with two-phase sync methods (wait_for_state, *_all_sync)
- `src/operator/routes.rs` - Axum REST API routes with /api/run/start synchronized endpoint
- `src/bin/operator.rs` - Operator binary entry point
- `src/common/mod.rs` - Added Heartbeat, ComponentMetrics types
- `src/common/command.rs` - Added metrics field to CommandResponse
- `src/data_sink/mod.rs` - GetStatus returns ComponentMetrics
- `src/merger/mod.rs` - GetStatus returns ComponentMetrics
- `src/data_source_emulator/mod.rs` - Heartbeat sender (1Hz configurable)

### Test Results (2026-01-12)
```
# Two-Phase Synchronized Run Start:
POST /api/run/start {"run_number": 300}
→ Phase 1: Configure all (with sync wait)
→ Phase 2: Arm all (sync point - waits for all Armed)
→ Phase 3: Start all (with sync wait)
→ Result: "Run 300 started successfully (all components synchronized)"

# Status with Metrics:
GET /api/status → Returns ComponentMetrics for Merger/DataSink:
  - events_processed: 238650000
  - event_rate: 30688549/s (~30.7 MHz)

Swagger UI: http://localhost:8080/swagger-ui/ - Working
```

## Notes
- This completes the control system as per docs/control_system_design.md
- Swagger UI enables API exploration and curl command generation (like Oat++)
- Heartbeat timeout values tunable via config
- Status channel is independent of data flow (monitoring continues even when idle)
- Angular frontend can be developed using OpenAPI spec as contract
