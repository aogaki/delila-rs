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

## Component Architecture Principles (MANDATORY)

**全コンポーネントは以下のアーキテクチャに従うこと。違反は許容しない。**

### Lock-Free Task Separation

各コンポーネントは独立したタスクに分離し、mpscチャンネルで接続する。
**タスク間で直接Mutexを共有してブロックしてはならない。**

```
┌─────────────────────────────────────────────────────────────────┐
│                        Component                                 │
│                                                                  │
│  ┌──────────┐   mpsc    ┌──────────┐   mpsc    ┌──────────┐    │
│  │ Receiver │ ────────► │  Main    │ ────────► │ Sender   │    │
│  │ (ZMQ)    │  channel  │  Logic   │  channel  │ (ZMQ/IO) │    │
│  └──────────┘           └──────────┘           └──────────┘    │
│       │                      │                      │           │
│       │ 高速                 │ 処理                 │ 遅い可能性│
│       │ ブロック禁止         │ ソート等             │ fsync等   │
│       ▼                      ▼                      ▼           │
│  ┌──────────┐           ┌──────────┐                           │
│  │ Command  │◄─────────►│ State    │ (watch channel)           │
│  │ (ZMQ REP)│           │ (shared) │                           │
│  └──────────┘           └──────────┘                           │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### 必須ルール

1. **Receiver Task**: ZMQソケットからの受信専用。処理をせずチャンネルに送るだけ。
   - `try_send()` を使用し、チャンネルがfullでもブロックしない
   - ブロック禁止（データ損失よりも受信継続を優先）

2. **Main Logic Task**: データ処理（ソート、集計等）
   - 重い処理はここで行う
   - 入力・出力ともにmpscチャンネル経由

3. **Sender/Writer Task**: ZMQ送信またはファイル書き込み
   - fsync等の遅い操作はここで吸収
   - 上流をブロックしない

4. **Command Task**: 既存の`run_command_task()`を使用
   - 状態変更は`watch::Sender`経由で通知
   - 統計取得は`Arc<AtomicU64>`等のlock-free構造を使用

### 禁止事項

```rust
// ❌ 禁止: 受信ループ内でMutexロック
msg = socket.next() => {
    let mut state = self.state.lock().unwrap();  // ブロック！
    state.process(msg);
}

// ❌ 禁止: 書き込み完了を待ってから次の受信
for msg in receiver {
    file.write_all(&msg)?;
    file.sync_data()?;  // fsyncが受信をブロック！
}
```

### 推奨パターン

```rust
// ✅ 推奨: タスク分離 + チャンネル
let (tx, rx) = mpsc::channel(1000);

// Receiver task
tokio::spawn(async move {
    while let Some(msg) = socket.next().await {
        let _ = tx.try_send(msg);  // Non-blocking
    }
});

// Writer task
tokio::spawn(async move {
    while let Some(msg) = rx.recv().await {
        file.write_all(&msg)?;
        // fsyncはここでブロックしてもReceiverに影響なし
    }
});
```

### 統計・状態共有

```rust
// ✅ Lock-free counters for hot path
struct Stats {
    received: AtomicU64,
    sent: AtomicU64,
}

// ✅ watch channel for state broadcast
let (state_tx, state_rx) = watch::channel(ComponentState::Idle);

// ✅ DashMap for per-key stats (lock per entry, not global)
let per_source: DashMap<u32, SourceStats> = DashMap::new();
```

### 参照実装

- **Merger** (`src/merger/mod.rs`): Receiver/Senderタスク分離の例
- **Recorder** (`src/recorder/mod.rs`): Receiver/Sorter/Writerの3タスク分離の例

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

## System Testing

Use Claude Code slash commands for DAQ operations:

| Command | Description |
|---------|-------------|
| `/test-daq` | Run complete integration test |
| `/start-daq` | Start all DAQ components |
| `/stop-daq` | Stop all DAQ components |
| `/daq-status` | Check component status |

**State Machine:** `Idle → Configure → Configured → Arm → Armed → Start → Running → Stop → Configured`

**Web UIs:** Swagger http://localhost:8080/swagger-ui/ | Monitor http://localhost:8081/

**重要:** 常に Operator REST API 経由でコントロールする。直接 ZMQ コマンドは使用しない。

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
├── CURRENT.md           # 現在のスプリント概要（必ず読む）
├── 01_task.md           # アクティブなタスク
├── 02_another_task.md
└── archive/             # 完了済み（通常は読まない）
    ├── phase1_emulator/
    └── phase2_driver/
```

### セッション開始時に必ず読むファイル

1. `TODO/CURRENT.md` - 現在のスプリント概要と優先度
2. `docs/` 内の設計ドキュメント

**注意:** `TODO/archive/` は参照が必要な場合のみ読む。

### CURRENT.md の形式

```markdown
# Current Sprint (更新日: YYYY-MM-DD)

## 優先度高
- [ ] タスク名 → `TODO/XX_filename.md`

## 優先度中
- [ ] タスク名 → `TODO/YY_filename.md`

## 最近完了
- [x] タスク名 (完了日) → `TODO/archive/...`
```

### Workflow
1. Create TODO files in `TODO/` for current sprint goals
2. **CURRENT.md を常に最新に保つ**
3. **When implementation is complete:**
   - Update the TODO file with `**Status: COMPLETED** (date)`
   - Mark all tasks as `[x]` (completed)
   - Add "Implementation Summary" section with files modified and key decisions
   - Add "Test Results" section if applicable
   - CURRENT.md の該当タスクを「最近完了」に移動
4. When a milestone is complete, create appropriately named directory in `TODO/archive/`
5. Move completed TODO files to the archive directory
6. Create new TODO files for the next sprint

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

## Benchmark & Design Decision Documentation

ベンチマークや性能測定を行った場合:
1. 測定結果を関連するTODOファイルまたは設計ドキュメントに記録する
2. 以下を含める:
   - 測定日
   - 測定条件（ハードウェア、パラメータ）
   - 結果のテーブル
   - 結論と設計への影響
3. 将来の論文執筆時に引用可能な形式で記録する

例: `TODO/09_timestamp_sorting_design.md` にストレージベンチマーク結果を記録

## Notes

- Reference C++ implementation in `DELILA2/` for algorithm details
- Prioritize correctness and safety; optimize only where measured
- Use Rust's type system to catch errors at compile time
- Keep public API minimal and focused
