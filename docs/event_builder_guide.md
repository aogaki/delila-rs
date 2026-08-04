# Offline Event Builder Guide

> **Status (2026-08-04): FROZEN.** The Rust event builder documented here still
> works as described and remains the reference implementation, but new event-builder
> development moves to C++ (root_sink lineage) — see
> [TODO/event-builder/66_cpp_event_builder.md](../TODO/event-builder/66_cpp_event_builder.md).
> Known limitation: oxyroot writes ROOT files **uncompressed** (write-side
> compression is unimplemented in the crate), so output files are ~1.5-3x larger
> than C++/ZSTD equivalents.

A tool to produce ROOT TTree event files from raw `.delila` data acquired by the DELILA DAQ system.
ELIFANT-Event compatible coincidence event builder.

## Overview

```
.delila files → [Time Calibration] → [Event Building] → ROOT files
```

Two-step workflow:

1. **Time Calibration** — Measure timestamp offsets between digitizers
2. **Event Building** — Group hits within a coincidence window into events

## Building

Requires the `root` feature (ROOT file output via oxyroot):

```bash
cargo build --release --features root --bin event_builder
```

The binary is generated at `target/release/event_builder`.

## Step 0: Preparing chSettings.json

The channel settings file defines triggers, AC pairs, energy thresholds, and calibration coefficients.
The format is an **array of arrays grouped by module**: `[[ch0, ch1, ...], [ch0, ch1, ...], ...]`:

```json
[
    [
        {
            "IsEventTrigger": true,
            "ID": 0,
            "Module": 0,
            "Channel": 0,
            "HasAC": true,
            "ACModule": 0,
            "ACChannel": 1,
            "Phi": 0.0,
            "Theta": 0.0,
            "Distance": 0.0,
            "ThresholdADC": 100,
            "x": 0.0,
            "y": 0.0,
            "z": 0.0,
            "p0": 0.0,
            "p1": 1.0,
            "p2": 0.0,
            "p3": 0.0,
            "DetectorType": "LaBr3",
            "Tags": []
        }
    ]
]
```

### Key Fields

| Field | Description |
|-------|-------------|
| `IsEventTrigger` | Set to `true` to use this channel as an event trigger |
| `Module` / `Channel` | Digitizer module number / channel number |
| `HasAC` | Whether this channel has an Anti-Compton pair |
| `ACModule` / `ACChannel` | AC pair module / channel (128 = disabled) |
| `ThresholdADC` | Energy threshold in ADC units (hits below this are discarded) |
| `p0` - `p3` | Energy calibration coefficients (cubic polynomial: E = p0 + p1*x + p2*x^2 + p3*x^3) |
| `DetectorType` | Detector type label (informational only, does not affect building) |

> **Tip:** Start with all channels set to `"IsEventTrigger": false`, then set only the desired trigger channels to `true`.
> Alternatively, you can skip chSettings.json entirely and specify triggers via the `--trigger` CLI option.

## Step 1: Time Calibration

Measures clock offsets between digitizers.
Determines the time difference of each channel relative to a reference trigger (typically a pulser or known synchronization signal).

```bash
./event_builder time-calib \
    -i data/run0001_*.delila \
    --ref-module 0 --ref-channel 0 \
    --window 1000 \
    -o timeSettings.json \
    --hist-output timeAlignment.root
```

### Options

| Option | Default | Description |
|--------|---------|-------------|
| `-i, --input` | (required) | Input `.delila` file(s) (multiple allowed, shell glob expansion) |
| `-o, --output` | `timeSettings.json` | Output JSON file for time offsets |
| `--ref-module` | `0` | Reference trigger module number |
| `--ref-channel` | `0` | Reference trigger channel number |
| `--window` | `1000` | Coincidence window [ns] |
| `--min-entries` | `1000` | Minimum entries required for a valid calibration |
| `--max-events` | `0` (all) | Maximum number of events to process |
| `--hist-output` | `timeAlignment.root` | ROOT output file for time histograms |
| `--ref-energy-min` | `0` | Reference trigger energy lower bound (ADC) |
| `--ref-energy-max` | `65535` | Reference trigger energy upper bound (ADC) |

### Output: timeSettings.json

```json
{
    "ref_module": 0,
    "ref_channel": 0,
    "offsets": {
        "00_00": 0.0,
        "00_01": -125.34,
        "01_00": 75.12
    }
}
```

Keys are zero-padded `"MM_CC"` (module_channel), values are offsets in nanoseconds.
Inspect `timeAlignment.root` with a viewer to verify that peaks are properly aligned.

## Step 2: Event Building

Builds coincidence events using the time calibration results.

```bash
./event_builder build \
    -i data/run0001_*.delila \
    -o ./events/ \
    -c chSettings.json \
    -T timeSettings.json \
    --window 500 \
    --run-id 1 \
    --trigger 0:0
```

### Options

| Option | Default | Description |
|--------|---------|-------------|
| `-i, --input` | (required) | Input `.delila` file(s) |
| `-o, --output` | `.` | Output directory |
| `-c, --config` | (none) | Channel settings JSON (chSettings.json) |
| `-T, --time-calib` | (none) | Time calibration JSON |
| `--window` | `500` | Coincidence window [ns] |
| `--run-id` | `0` | Run ID (used in output file naming) |
| `--trigger` | (none) | Trigger channel in `module:channel` format, can be repeated |
| `--output-tree` | `EventTree` | ROOT TTree name |
| `--workers` | `4` | Number of worker threads |
| `--writers` | `2` | Number of writer threads |
| `--events-per-file` | `100000` | Events per ROOT file before rotation |

> **Note:** `--trigger` and `-c` (`IsEventTrigger` in chSettings.json) can both be used.
> Channels specified via `--trigger` are added as additional triggers.

### Output ROOT Files

Filename pattern: `eb_run{RUN_ID:04}_{INDEX:04}_events.root`

Example: `eb_run0001_0000_events.root`, `eb_run0001_0001_events.root`, ...

### TTree Branches

| Branch | Type | Description |
|--------|------|-------------|
| `EventID` | `u64` | Sequential event ID |
| `TriggerTime` | `f64` | Absolute trigger timestamp [ns] |
| `TriggerMod` | `u8` | Trigger module number |
| `TriggerCh` | `u8` | Trigger channel number |
| `Multiplicity` | `u32` | Hit multiplicity (including trigger) |
| `Mod` | `Vec<u8>` | Module number per hit |
| `Ch` | `Vec<u8>` | Channel number per hit |
| `Energy` | `Vec<u16>` | Long-gate energy (ADC) |
| `EnergyShort` | `Vec<u16>` | Short-gate energy (ADC) |
| `RelTime` | `Vec<f64>` | Time relative to trigger [ns] |
| `WithAC` | `Vec<u8>` | AC coincidence flag (0/1) |

## Example: Minimal Setup (No Config Files)

The simplest usage. No chSettings.json or time calibration needed:

```bash
# Use Module 0, Channel 0 as trigger
./event_builder build \
    -i data/*.delila \
    -o ./events/ \
    --trigger 0:0 \
    --window 500 \
    --run-id 1
```

## Example: Full Setup

```bash
# 1. Time calibration
./event_builder time-calib \
    -i data/run0042_*.delila \
    --ref-module 0 --ref-channel 0 \
    --window 1000 \
    --ref-energy-min 500 \
    -o timeSettings.json

# 2. Inspect timeAlignment.root to verify offsets are reasonable

# 3. Event building
./event_builder build \
    -i data/run0042_*.delila \
    -o ./events/ \
    -c chSettings.json \
    -T timeSettings.json \
    --window 500 \
    --run-id 42 \
    --workers 4 \
    --writers 2 \
    --events-per-file 100000

# 4. Verify in ROOT
# root -l events/eb_run0042_0000_events.root
# EventTree->Draw("Energy")
```

## Performance

- Throughput: ~0.79 M events/s (single writer), scales with multi-threading
- Well above the typical production rate of ~300k events/s
- `--workers 4 --writers 2` is the default and optimal for most cases
- File rotation (100k events/file) keeps individual file sizes manageable

## Troubleshooting

| Symptom | Cause / Solution |
|---------|-----------------|
| `Error: event_builder requires the 'root' feature` | Rebuild with `--features root` |
| All offsets are 0 in time calibration | No data on the reference channel. Check `--ref-module` / `--ref-channel` |
| 0 events built | Wrong `--trigger` specification, or no data on the trigger channel |
| Output files too large | Reduce `--events-per-file` (e.g., 50000) |
