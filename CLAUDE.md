# CLAUDE.md - DELILA-Rust (Next Gen DAQ)

## User Profile & Communication Style
- **User:** Aogaki (Senior Physicist & Computer Engineer)
- **Background:** 27 years of C++ experience, PhD in Computer Engineering. Expert in ROOT, DAQ-Middleware, and experimental physics.
- **Role:** Claude acts as a "Junior Rust Partner" or "Vibe Coding Copilot"
- **Communication:**
    - Explain Rust concepts using **C++ analogies** (e.g., `Arc` = `std::shared_ptr`, `Box` = `std::unique_ptr`, `Drop` = RAII destructor)
    - Focus on **memory layout, ownership costs, and performance implications**
    - Do not lecture on basic algorithms; focus on Rust-specific syntax and borrow checker resolutions

## Project Overview

- **Goal:** Build a Minimum Viable Product (MVP) of a distributed DAQ system by **mid-March 2026**
- **Hardware:** CAEN Digitizers (Optical Link/USB)
- **Architecture:** ZeroMQ-based distributed pipeline (Reader -> Merger -> Recorder/Monitor)
- **Reference:** C++ implementation available in `DELILA2/` submodule

## Tech Stack (Strict Constraints)

| Category | Library | Notes |
|----------|---------|-------|
| Language | Rust 2021 edition | |
| Async Runtime | `tokio` | Network/File I/O |
| Messaging | `tmq` | ZeroMQ bindings for Tokio |
| Serialization | `serde` + `rmp-serde` | MessagePack format |
| Web Backend | `axum` | REST API |
| Frontend | Angular (TypeScript) | Clean JSON API interface |
| Plotting | `plotly` / `plotters` | Interactive histograms / Static waveforms |
| FFI | `bindgen` | CAEN C-libraries |

## Architecture

```
┌─────────┐    ZMQ     ┌─────────┐    ZMQ     ┌──────────┐
│ Reader  │ ────PUB──► │ Merger  │ ────PUB──► │ Recorder │
│ (CAEN)  │            │         │            │ (File)   │
└─────────┘            └─────────┘            └──────────┘
                            │
                            │ PUB
                            ▼
                       ┌─────────┐
                       │ Monitor │
                       │ (Web)   │
                       └─────────┘
```

## Project Structure

```
delila-rs/
├── Cargo.toml
├── CLAUDE.md
├── DELILA2/              # C++ reference (submodule)
├── src/
│   ├── lib.rs
│   ├── reader/           # Digitizer readout
│   │   ├── mod.rs
│   │   ├── emulator.rs   # Dummy data generator
│   │   └── caen.rs       # CAEN FFI wrapper
│   ├── merger/           # Event building
│   ├── recorder/         # File writing
│   ├── monitor/          # Web interface
│   └── common/           # Shared types (EventData, etc.)
└── tests/
```

## Development Phases

| Phase | Period | Goal |
|-------|--------|------|
| **1** | Jan 2026 | Pipeline with Emulators + ZMQ + Web Monitor |
| **2** | Feb 2026 | CAEN Driver (Safe Wrapper via bindgen) |
| **3** | Late Feb | Driver integration + File Writer (MsgPack) |
| **4** | Mar 2026 | Web UI + Run Control |

## Design Principles (Priority Order)

1. **KISS (Keep It Simple, Stupid)** - Highest Priority
   - Simplicity always comes first
   - Avoid over-abstraction
   - Implement working code via the shortest path

2. **TDD (Test-Driven Development)** - Foundation of All Implementation
   - Write tests first (Red → Green → Refactor)
   - Code without tests is non-existent code
   - Every feature must have unit tests

3. **Clean Architecture** - Subordinate to KISS
   - Dependencies point inward
   - When conflicting with KISS, KISS wins
   - Avoid "future-proofing" over-design

**Decision Criteria:** "Is this architecture really necessary? Can it be solved in a simpler way?"

## Coding Standards

### 1. Safety & FFI

```rust
// Unsafe ONLY in CAEN driver wrapper layer
pub struct Digitizer {
    handle: ffi::CAEN_DGTZ_Handle,  // C's void* equivalent
}

// Drop = RAII destructor (C++ analogy)
impl Drop for Digitizer {
    fn drop(&mut self) {
        unsafe {
            ffi::CAEN_DGTZ_CloseDigitizer(self.handle);
        }
    }
}

// Send impl requires thread-safety verification of C handle
// unsafe impl Send for Digitizer {}  // Only if C-lib is thread-safe
```

### 2. Error Handling

```rust
// Use Result<T, E> everywhere, propagate with ?
pub fn configure(&mut self, config: &Config) -> Result<(), DigitizerError> {
    self.set_record_length(config.record_length)?;
    self.set_trigger(config.trigger)?;
    Ok(())
}

// Avoid .unwrap() in production - use anyhow/thiserror
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DigitizerError {
    #[error("Failed to open digitizer: {0}")]
    OpenFailed(i32),

    #[error("Configuration error: {0}")]
    ConfigError(String),
}
```

### 3. Concurrency & Performance

```rust
// Arc<Mutex<T>> = std::shared_ptr<std::mutex<T>> in C++
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

// Shared state between Web and DAQ threads
struct AppState {
    status: Arc<Mutex<DaqStatus>>,
    event_tx: mpsc::Sender<EventData>,
}

// Hot loop: minimize allocations, reuse buffers
fn read_loop(buffer: &mut Vec<u8>) {
    buffer.clear();  // Reuse allocation
    // ... fill buffer from digitizer
}
```

### 4. Code Quality

```bash
# Must pass before commit
cargo fmt
cargo clippy -- -D warnings
cargo test
```

## Build Commands

```bash
# Development
cargo check              # Fast type check
cargo build              # Debug build
cargo test               # Run tests
cargo clippy             # Lint

# Release
cargo build --release    # Optimized build

# Documentation
cargo doc --open         # Generate & view docs
```

## C++ to Rust Quick Reference

| C++ | Rust | Notes |
|-----|------|-------|
| `std::unique_ptr<T>` | `Box<T>` | Heap allocation, single owner |
| `std::shared_ptr<T>` | `Arc<T>` | Reference counted |
| `std::mutex` | `Mutex<T>` | Data inside mutex |
| RAII destructor | `Drop` trait | Automatic cleanup |
| `const T&` | `&T` | Immutable borrow |
| `T&` | `&mut T` | Mutable borrow |
| `std::optional<T>` | `Option<T>` | Nullable type |
| `throw`/`catch` | `Result<T, E>` | Error handling |
| `std::vector<T>` | `Vec<T>` | Dynamic array |
| `std::string` | `String` | Owned string |
| `const char*` | `&str` | String slice |

## Workflow (Vibe Coding)

1. **Generate:** Create skeleton code based on physical requirements
2. **Verify:** Run `cargo check` and `cargo clippy`
3. **Explain:** Briefly explain memory safety vs C++ equivalent

## TODO Management

Short-term goals are tracked in the `TODO/` directory.

```
TODO/
├── 01_task.md           # Current tasks (numbered for priority)
├── 02_another_task.md
└── archive/             # Completed tasks
    ├── phase1_emulator/ # Archived by milestone
    │   ├── 01_xxx.md
    │   └── 02_yyy.md
    └── phase2_driver/
```

### Workflow
1. Create TODO files in `TODO/` for current sprint goals
2. **When implementation is complete:**
   - Update the TODO file with `**Status: COMPLETED** (date)`
   - Mark all tasks as `[x]` (completed)
   - Add "Implementation Summary" section with files modified and key decisions
   - Add "Test Results" section if applicable
3. When a milestone is complete, create appropriately named directory in `TODO/archive/`
4. Move completed TODO files to the archive directory
5. Create new TODO files for the next sprint

**IMPORTANT:** Always update TODO files immediately after completing implementation. Do not leave stale unchecked items.

## Dependencies (Cargo.toml)

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
tmq = "0.4"
serde = { version = "1", features = ["derive"] }
rmp-serde = "1"
axum = "0.7"
thiserror = "1"
tracing = "0.1"
tracing-subscriber = "0.3"

[build-dependencies]
bindgen = "0.69"

[dev-dependencies]
tokio-test = "0.4"
```

## Important Files to Read

When starting a new session, read these files to understand current context:

### TODO Directory
Current tasks and implementation progress:
- `TODO/*.md` - Active tasks (numbered for priority)
- `TODO/archive/` - Completed tasks for reference

### Documentation
Architecture and design decisions:
- `docs/architecture/config_and_deployment.md` - 設定管理とデプロイメント設計
- `docs/control_system_design.md` - コントロールシステム設計

### Key Implementation Files
- `src/reader/caen/` - CAEN FFI bindings (handle.rs, error.rs, wrapper.c)
- `src/reader/decoder/` - Data decoders (psd2.rs, common.rs)
- `src/config/mod.rs` - Configuration management

## Notes

- Reference C++ implementation in `DELILA2/` for algorithm details
- Prioritize correctness and safety; optimize only where measured
- Use Rust's type system to catch errors at compile time
- Keep public API minimal and focused
