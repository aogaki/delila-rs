# Monitor Component Implementation

**Created:** 2026-01-13
**Status:** COMPLETED (2026-01-14)
**Priority:** HIGH

## Overview

Monitorコンポーネントを実装し、データ取得の結果をリアルタイムで可視化できるようにする。

## Design Decisions

- **独立したコンポーネント** - Operatorとは別プロセス（負荷分散、独立性）
- **ヒストグラム設定可能** - ビン数、範囲をAPI/設定で変更可能
- **将来的に2Dヒストグラム＋グラフ必須** - PSD plot、時系列グラフ等

## Architecture

```
┌─────────┐    ZMQ SUB    ┌─────────────┐     HTTP      ┌──────────┐
│ Merger  │ ────────────► │   Monitor   │ ────────────► │ Browser  │
└─────────┘               │ (Histogram) │               │  (UI)    │
                          └─────────────┘               └──────────┘
                                │
                          Axum HTTP :8080
```

## Implementation Summary

### Files Created

| File | Purpose |
|------|---------|
| `src/monitor/mod.rs` | Monitor component with histogram logic |
| `src/monitor/monitor_ui.html` | Web UI with Plotly.js |
| `src/bin/monitor.rs` | Binary entry point |

### Implementation Tasks

#### Phase 1: Core Monitor - **COMPLETED**
- [x] Create `src/monitor/mod.rs`
- [x] Create `src/bin/monitor.rs`
- [x] ZMQ SUB to receive data from Merger
- [x] 1D Histogram accumulation (per channel)
- [x] Basic command interface (Configure/Start/Stop/Reset)

#### Phase 2: Web API - **COMPLETED**
- [x] Axum HTTP server (port 8080)
- [x] GET /api/histograms - return current histogram data
- [x] GET /api/histograms/:module/:channel - specific channel
- [x] POST /api/histograms/clear - reset histograms
- [x] GET /api/status - monitor status

#### Phase 3: Web UI - **COMPLETED**
- [x] HTML/JS frontend with Plotly.js
- [x] Channel list with counts
- [x] Interactive histogram display
- [x] Log scale toggle
- [x] Auto-refresh (500ms interval)
- [x] Clear histograms button

#### Phase 4: Real-time Updates (Future)
- [ ] WebSocket endpoint for live histogram updates
- [ ] Configurable update interval (e.g., 100ms)
- [ ] Efficient delta updates (not full histogram each time)

#### Phase 5: 2D Histograms & Graphs (Future)
- [ ] 2D histogram (PSD plot: Long vs Short gate)
- [ ] Time series graphs (count rate, etc.)
- [ ] Waveform display (if enabled)

## Data Structures

```rust
/// Histogram configuration (settable via API)
pub struct HistogramConfig {
    pub num_bins: u32,        // Default: 4096
    pub min_value: f32,       // Default: 0
    pub max_value: f32,       // Default: 65535 (16-bit ADC)
}

/// 1D Histogram data for a single channel
pub struct Histogram1D {
    pub module_id: u32,
    pub channel_id: u32,
    pub config: HistogramConfig,
    pub bins: Vec<u64>,       // Count per bin
    pub total_counts: u64,
    pub overflow: u64,
    pub underflow: u64,
}

/// Monitor state
pub struct MonitorState {
    pub histograms: HashMap<ChannelKey, Histogram1D>,
    pub total_events: u64,
    pub start_time: Option<Instant>,
    pub histogram_config: HistogramConfig,
}
```

## REST API

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/` | Web UI (HTML) |
| GET | `/api/status` | Monitor status (state, events, rate) |
| GET | `/api/histograms` | List all channels with counts |
| GET | `/api/histograms/:module/:channel` | Get specific histogram |
| POST | `/api/histograms/clear` | Clear all histograms |

## Usage

```bash
# Start monitor
cargo run --bin monitor -- --config config.toml

# Or with CLI options
cargo run --bin monitor -- --address tcp://localhost:5557 --port 8080

# Open browser
open http://localhost:8080/
```

## Test Results

All 52 tests passed including 5 new monitor tests:
- `test_histogram_config_default`
- `test_histogram_fill`
- `test_histogram_overflow_underflow`
- `test_histogram_clear`
- `test_monitor_state_process_event`

## End-to-End Test

Successfully tested with:
1. Emulator (source_id=0) generating 1000 events/batch at 100ms interval
2. Merger forwarding data
3. Monitor receiving and displaying histograms in browser

Event rate: ~120 kHz, 16 channels displayed with real-time updates.

## Frontend Technology (Future)

**Current:** Embedded HTML with Plotly.js (simple, no build step)

**Future options:**
- **Svelte + plotly.js** - 軽量、Plotly公式サポート、Tauri相性良し ← 推奨
- **Leptos + plotly.js** - Rust統一だがJS interop必要
- **React + react-plotly.js** - 定番だが重め

**方針:** まずバックエンドAPI完成 → フロントエンドは後から選択

## References

- `src/data_sink/mod.rs` - Similar ZMQ SUB pattern
- `config.toml` - `[network.monitor]` section
