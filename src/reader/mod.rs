//! Reader module for digitizer data acquisition
//!
//! This module provides:
//! - CAEN digitizer FFI bindings (caen)
//! - Data decoders (decoder)
//! - Reader integration with two-task architecture

pub mod caen;
#[cfg(feature = "x743")]
pub mod caen_legacy;
pub mod decoder;
mod parallel_decode;
mod read_loop_dig1;
mod read_loop_dig2;
mod state;

use state::effective_state_for;
#[cfg(feature = "x743")]
use state::{next_reconnect_cooldown, state_rank, RECONNECT_INITIAL};

/// Per-event decode parameters cached by the V1743 ReadLoop so
/// `x743_std_event_to_event_data` never touches `DigitizerConfig` on the hot path.
///
/// The CAEN lib in Standard mode only populates `TDC` and `DataChannel[]` on
/// `CAEN_DGTZ_X743_GROUP_t`; `Charge`/`Peak`/`Baseline`/`PosEdgeTimeStamp` stay
/// zero. We run a tiny Rust post-processor on each waveform to extract baseline,
/// amplitude and a software-CFD fine time — the latter is where the 5 ps RMS
/// time-resolution comes from.
#[cfg(feature = "x743")]
#[derive(Debug, Clone)]
struct X743DecodeParams {
    energy_scale: f32,
    energy_offset: f32,
    save_waveform: bool,
    ns_per_sample: f64,
    /// Sample count used for baseline averaging (from the start of the waveform).
    baseline_samples: usize,
    /// CFD delay in samples.
    cfd_delay_samples: usize,
    /// CFD fraction (typ. 0.2–0.5 for PMT-like pulses).
    cfd_fraction: f32,
    /// TTF moving-average tap count applied to the raw waveform before
    /// baseline / CFD computation. 0 or 1 = pass-through. Mirrors WaveDemo.
    ttf_smoothing_taps: usize,
    /// Per-channel polarity: `true` = negative pulse (pulse dips below baseline).
    channel_negative: [bool; caen_legacy::MAX_CHANNELS],
    /// `true` when `energy_source = "soft"`: `energy` comes from the gated charge
    /// integral instead of the pulse amplitude.
    use_soft_charge: bool,
    /// Long-gate opening offset (samples before the CFD anchor). Soft charge only.
    charge_gate_pre_samples: usize,
    /// Long-gate width in samples. Soft charge only.
    charge_gate_samples: usize,
}

/// Result of the Rust-side V1743 waveform post-processor. See `analyze()`.
#[cfg(feature = "x743")]
#[derive(Debug, Clone, Copy)]
struct X743WaveformStats {
    /// Mean of the pre-trigger samples (ADC units). Used as the pedestal for the
    /// soft-charge integral and for diagnostics/tests.
    baseline: f32,
    /// Signed peak extremum (min for negative pulses, max for positive).
    /// Kept for diagnostics/tests.
    #[allow(dead_code)]
    peak: f32,
    /// `|peak − baseline|` — pulse amplitude.
    amplitude: f32,
    /// Sub-sample leading-edge time in ns, measured from sample 0 of the waveform.
    /// Computed from the zero-crossing of the CFD signal `f·s[i] − s[i − delay]`
    /// between sample `floor(edge)` and `floor(edge)+1`.
    cfd_time_ns: f64,
    /// Index of the peak sample (for diagnostics, flags packing).
    peak_index: u16,
    /// `true` if the CFD zero-crossing search succeeded. `false` events fall back
    /// to sample-quantised timing so they are still usable but shouldn't be used
    /// for resolution measurement.
    cfd_valid: bool,
}

#[cfg(feature = "x743")]
impl X743WaveformStats {
    /// Run the Rust-side post-processor. Returns `None` only if `samples` is too
    /// short to contain a meaningful baseline + pulse.
    ///
    /// Parameters:
    /// - `samples`: ADC samples from `CAEN_DGTZ_X743_GROUP_t.DataChannel[ch]`
    ///   (float, already corrected by `correction_level="all"`).
    /// - `ns_per_sample`: 1 / sampling frequency (e.g. 0.3125 ns @ 3.2 GSa/s).
    /// - `negative_pulse`: polarity of the pulse. Flips peak direction and the
    ///   CFD zero-crossing slope sign but keeps all sums/amplitudes positive.
    /// - `baseline_n`: pre-trigger samples averaged for baseline.
    /// - `cfd_delay`: delay `d` used in the CFD `f·s[i] − s[i − d]` signal.
    /// - `cfd_fraction`: `f` used in the CFD signal.
    fn analyze(
        samples: &[f32],
        ns_per_sample: f64,
        negative_pulse: bool,
        baseline_n: usize,
        cfd_delay: usize,
        cfd_fraction: f32,
    ) -> Option<Self> {
        let n = samples.len();
        if n < baseline_n + cfd_delay + 4 {
            return None;
        }

        // Baseline = simple mean of the first `baseline_n` samples. SAMLONG
        // correction_level="all" already removes cell-by-cell Line Offset and
        // Individual Pedestal, so a scalar mean is sufficient.
        let n_bl = baseline_n.min(n / 2);
        let baseline: f32 = samples[..n_bl].iter().sum::<f32>() / n_bl as f32;

        // Signed extremum over the post-baseline region.
        let (peak_index, peak) = if negative_pulse {
            samples[n_bl..]
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, &v)| (i + n_bl, v))?
        } else {
            samples[n_bl..]
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, &v)| (i + n_bl, v))?
        };
        let amplitude = (peak - baseline).abs();

        // Software CFD on the baseline-subtracted waveform.
        //   d[i] = f · x[i] − x[i − delay]           (baseline-subtracted x, 0 < f < 1)
        //   Zero-crossing on the leading edge is our timing.
        //
        // Signs (walk through with baseline=0):
        //   Positive pulse rising from 0 to +A over `delay` samples:
        //     - Pre-pulse: x=0 everywhere → d = 0
        //     - Leading edge: x[i]=+a, x[i−delay]=0 → d = f·a − 0 = +f·a (positive)
        //     - At peak: x[i]=+A, x[i−delay]=+a → d = f·A − a; once a > f·A, d < 0
        //     - So d goes 0 → positive → NEGATIVE = **POS→NEG crossing** on leading edge
        //   Negative pulse falling from 0 to −A:
        //     - Leading edge: x[i]=−a, x[i−delay]=0 → d = f·(−a) = negative
        //     - At peak: x[i]=−A, x[i−delay]=−a → d = f·(−A) − (−a) = −f·A + a; flips positive
        //     - So d goes 0 → negative → POSITIVE = **NEG→POS crossing** on leading edge
        let crossing_is_pos_to_neg = !negative_pulse;
        let cfd = |i: usize| -> f32 {
            cfd_fraction * (samples[i] - baseline) - (samples[i - cfd_delay] - baseline)
        };

        // Search backwards from the peak so we find the crossing adjacent to the
        // real leading edge instead of a noise-driven crossing deep in the
        // pre-trigger baseline region. We require the crossing to lie within a
        // few rise-times of the peak so long pre-trigger windows don't pollute
        // the result.
        let min_start = n_bl.max(cfd_delay + 1);
        let end = peak_index.min(n - 1);
        // Look back at most 4× the CFD delay — that's enough to span the rising
        // edge for any reasonable pulse (< 4·delay samples rise time).
        let search_span = (cfd_delay * 4).max(16);
        let start = end.saturating_sub(search_span).max(min_start);

        let mut crossing: Option<(usize, f32, f32)> = None; // (i, prev_d, curr_d)
        let mut next_d = cfd(end);
        for i in (start + 1..end).rev() {
            let curr_d = cfd(i);
            let is_match = if crossing_is_pos_to_neg {
                // Scanning backwards: "POS→NEG on leading edge" means when moving
                // forward d transitions from positive (earlier) to negative (later).
                // Backward iteration sees: next_d (later) ≤ 0 and curr_d (earlier) > 0.
                curr_d > 0.0 && next_d <= 0.0
            } else {
                curr_d < 0.0 && next_d >= 0.0
            };
            if is_match {
                crossing = Some((i + 1, curr_d, next_d));
                break;
            }
            next_d = curr_d;
        }

        let (cfd_time_ns, cfd_valid) = if let Some((i, prev_d, curr_d)) = crossing {
            let denom = curr_d - prev_d;
            // Linear interpolation of the zero of the CFD signal between samples
            // (i-1, i). This is the sub-sample precision — every ~pulse amplitude
            // of noise here costs ~(noise/slope) ns of timing RMS.
            let frac = if denom.abs() < f32::EPSILON {
                0.0_f64
            } else {
                (-prev_d / denom) as f64
            };
            let idx = (i - 1) as f64 + frac;
            (idx * ns_per_sample, true)
        } else {
            // No zero-crossing found (very low amplitude, baseline-only, etc.).
            // Fall back to the peak position — sample-quantised timing at best.
            ((peak_index as f64) * ns_per_sample, false)
        };

        Some(Self {
            baseline,
            peak,
            amplitude,
            cfd_time_ns,
            peak_index: peak_index.min(u16::MAX as usize) as u16,
            cfd_valid,
        })
    }
}

/// Soft-charge long-gate integral for V1743 Standard mode.
///
/// Integrates the baseline-subtracted, sign-corrected waveform over a gate of
/// `gate_len` samples opening `gate_pre` samples before `anchor`. `anchor` is the
/// integer sample index of the software-CFD crossing (or the peak sample when the
/// CFD search failed) so the gate tracks the pulse rather than a fixed window.
///
/// The sum is sign-corrected (`baseline − s` for negative pulses, `s − baseline`
/// for positive) so the returned charge is positive for a real pulse, matching
/// the amplitude-path convention. Pedestal is removed by the baseline subtraction,
/// so out-of-pulse samples contribute only zero-mean noise. Units are ADC·samples;
/// caller applies `energy_scale`/`energy_offset`.
#[cfg(feature = "x743")]
fn x743_long_charge(
    samples: &[f32],
    baseline: f32,
    anchor: usize,
    gate_pre: usize,
    gate_len: usize,
    negative: bool,
) -> f32 {
    if gate_len == 0 || samples.is_empty() {
        return 0.0;
    }
    let start = anchor.saturating_sub(gate_pre).min(samples.len());
    let end = start.saturating_add(gate_len).min(samples.len());
    let mut acc = 0.0f32;
    for &s in &samples[start..end] {
        acc += if negative { baseline - s } else { s - baseline };
    }
    acc
}

#[cfg(feature = "x743")]
impl X743DecodeParams {
    /// Build from a loaded `DigitizerConfig`. Returns conservative defaults if
    /// no config / no `[x743]` section is present so the ReadLoop can still
    /// decode (TDC-only resolution, no fine-time correction).
    fn from_config(dig_config: Option<&crate::config::digitizer::DigitizerConfig>) -> Self {
        let mut p = Self {
            energy_scale: 1.0,
            energy_offset: 0.0,
            save_waveform: false,
            ns_per_sample: Self::ns_per_sample("3.2ghz"),
            baseline_samples: 32,
            cfd_delay_samples: 4,
            cfd_fraction: 0.3,
            ttf_smoothing_taps: 0,
            channel_negative: [true; caen_legacy::MAX_CHANNELS],
            use_soft_charge: false,
            charge_gate_pre_samples: 8,
            charge_gate_samples: 64,
        };
        let Some(dc) = dig_config else {
            return p;
        };

        // Per-channel polarity table, derived from channel_defaults +
        // channel_overrides so the decoder doesn't touch the config on the hot path.
        let default_is_neg = dc
            .channel_defaults
            .polarity
            .as_deref()
            .map(Self::polarity_is_negative)
            .unwrap_or(true);
        for ch in 0..caen_legacy::MAX_CHANNELS {
            let per_ch = dc
                .channel_overrides
                .get(&(ch as u8))
                .and_then(|c| c.polarity.as_deref())
                .map(Self::polarity_is_negative);
            p.channel_negative[ch] = per_ch.unwrap_or(default_is_neg);
        }

        let Some(x743) = dc.x743.as_ref() else {
            return p;
        };
        if x743.energy_source.eq_ignore_ascii_case("charge") {
            tracing::warn!(
                "x743 energy_source=\"charge\" selected but the CAEN lib does not populate \
                 Charge in Standard mode; energy will be 0. Use \"amplitude\" (default) instead."
            );
        }
        p.charge_gate_pre_samples = x743.charge_gate_pre_samples as usize;
        p.charge_gate_samples = x743.charge_gate_samples as usize;
        p.use_soft_charge = x743.energy_source.eq_ignore_ascii_case("soft");
        if p.use_soft_charge {
            tracing::info!(
                "x743 energy_source=\"soft\": gated charge integral active \
                 (gate anchored at CFD crossing, pre={} samples, width={} samples, \
                 scale={}, offset={})",
                p.charge_gate_pre_samples,
                p.charge_gate_samples,
                x743.energy_scale,
                x743.energy_offset,
            );
        }
        p.energy_scale = x743.energy_scale;
        p.energy_offset = x743.energy_offset;
        p.save_waveform = x743.save_waveform;
        p.ns_per_sample = Self::ns_per_sample(&x743.sampling_frequency);
        p.baseline_samples = x743.baseline_samples.max(4) as usize;
        p.cfd_delay_samples = x743.cfd_delay_samples.max(1) as usize;
        p.cfd_fraction = x743.cfd_fraction.clamp(0.01, 0.99);
        p.ttf_smoothing_taps = x743.ttf_smoothing.taps();
        p
    }

    fn polarity_is_negative(s: &str) -> bool {
        // Treat anything that isn't explicitly positive/rising as negative.
        // Matches the convention used by `apply_channel_config` in handle.rs.
        !matches!(
            s.to_lowercase().as_str(),
            "positive" | "pos" | "rising" | "risingedge"
        )
    }

    fn ns_per_sample(freq: &str) -> f64 {
        match freq.to_lowercase().as_str() {
            "3.2ghz" | "3200mhz" => 1.0 / 3.2,
            "1.6ghz" | "1600mhz" => 1.0 / 1.6,
            "800mhz" | "0.8ghz" => 1.0 / 0.8,
            "400mhz" | "0.4ghz" => 1.0 / 0.4,
            _ => 1.0 / 3.2,
        }
    }
}

/// Per-event scratch buffers reused across the V1743 decode hot path.
/// `raw` holds samples copied out of CAEN-lib-owned memory; `smoothed` holds
/// the moving-average output. Both are sized to `record_length` (≤ 1024).
/// Reusing them avoids one `Vec<f32>::with_capacity` per channel per event.
#[cfg(feature = "x743")]
#[derive(Default)]
struct X743Scratch {
    raw: Vec<f32>,
    smoothed: Vec<f32>,
}

#[cfg(feature = "x743")]
impl X743Scratch {
    fn new() -> Self {
        Self {
            raw: Vec::with_capacity(1024),
            smoothed: Vec::with_capacity(1024),
        }
    }

    /// Apply N-tap moving average to `self.raw` writing to `self.smoothed`.
    /// Returns a slice into the buffer that was actually used.
    /// `taps == 0 || taps == 1` → returns `&self.raw` directly (no copy).
    /// Edge handling: leading samples (i < taps-1) average over the available
    /// `i+1` samples (no zero padding); steady-state from i = taps-1 onwards.
    fn smoothed_view(&mut self, taps: usize) -> &[f32] {
        if taps <= 1 || self.raw.is_empty() {
            return &self.raw;
        }
        let n = self.raw.len();
        self.smoothed.clear();
        self.smoothed.reserve(n);
        let mut sum: f32 = 0.0;
        for i in 0..n {
            sum += self.raw[i];
            if i >= taps {
                sum -= self.raw[i - taps];
            }
            let denom = (i + 1).min(taps) as f32;
            self.smoothed.push(sum / denom);
        }
        &self.smoothed
    }
}

/// Diagnostic-only observer for the raw V1743 TDC stream (TODO 62).
///
/// The V1743 decoder uses the raw 40-bit TDC directly as coarse time — there is
/// **no** rollover extension, so a momentary garbage/out-of-order TDC corrupts
/// only its own event and self-heals on the next (no persistent state to
/// poison; the run30 failure mode is structurally impossible). Operationally
/// runs are kept < 90 min so the counter (wrap ~91.6 min) never wraps.
///
/// This struct exists purely to keep that decision *observable* (CLAUDE.md
/// "silent failure を作らない"): it logs the first `PER_RUN` events per group
/// after each run and any **backward** raw-TDC step (a non-monotonic value — the
/// glitch we still want to catch red-handed). It never influences the emitted
/// timestamp, so a glitch it sees cannot alter data.
#[cfg(feature = "x743")]
struct X743TdcDiag {
    /// Startup log budget per group (any-direction), decremented per event.
    budget: Vec<u32>,
    /// Separate, small budget for backward-step logs per group, so a glitch
    /// STORM cannot turn into per-event synchronous logging on the hot path.
    backward_budget: Vec<u32>,
    /// Total backward steps seen per group this run (counted even when the log
    /// budget is exhausted, so the running total stays visible).
    backward_count: Vec<u64>,
    /// Last masked raw TDC seen per group (for backward detection only).
    last_raw: Vec<Option<u64>>,
}

#[cfg(feature = "x743")]
impl X743TdcDiag {
    /// Startup events logged per group at the start of each run (any direction).
    const STARTUP_PER_RUN: u32 = 200;
    /// Cap on backward-step logs per group per run. Bounds hot-path logging in
    /// the glitch-storm case (the run30 failure mode) — the total is still
    /// tracked in `backward_count` and surfaced in every log line.
    const BACKWARD_PER_RUN: u32 = 50;

    fn new(groups: usize) -> Self {
        Self {
            budget: vec![Self::STARTUP_PER_RUN; groups],
            backward_budget: vec![Self::BACKWARD_PER_RUN; groups],
            backward_count: vec![0; groups],
            last_raw: vec![None; groups],
        }
    }

    /// Re-arm at run start (SWStartAcquisition): fresh budgets, forget last raw.
    fn rearm(&mut self) {
        for b in &mut self.budget {
            *b = Self::STARTUP_PER_RUN;
        }
        for b in &mut self.backward_budget {
            *b = Self::BACKWARD_PER_RUN;
        }
        for c in &mut self.backward_count {
            *c = 0;
        }
        for l in &mut self.last_raw {
            *l = None;
        }
    }

    /// Observe the masked 40-bit raw TDC for group `g`. Logs the first
    /// `STARTUP_PER_RUN` events of the run and up to `BACKWARD_PER_RUN` backward
    /// steps; always updates `last_raw` and the backward counter. Purely
    /// observational — never affects the emitted timestamp.
    fn observe(&mut self, g: usize, raw: u64) {
        let prev = self.last_raw.get(g).copied().flatten();
        if let Some(l) = self.last_raw.get_mut(g) {
            *l = Some(raw);
        }
        let backward = prev.is_some_and(|p| raw < p);
        if backward {
            if let Some(c) = self.backward_count.get_mut(g) {
                *c += 1;
            }
        }

        // Two independent, BOUNDED reasons to log — never per-event-forever:
        //  * startup: first STARTUP_PER_RUN events of the run (any direction)
        //  * glitch:  a backward raw-TDC step, capped at BACKWARD_PER_RUN so a
        //    storm (exactly when the hot path is most stressed) cannot become
        //    synchronous per-event logging + allocation. `backward` is unexpected
        //    under the <90 min run rule: it is either the run30-class glitch or a
        //    run that exceeded the 91 min wrap — both worth seeing.
        let startup = self.budget.get(g).is_some_and(|&b| b > 0);
        let glitch = !startup && backward && self.backward_budget.get(g).is_some_and(|&b| b > 0);
        if !(startup || glitch) {
            return;
        }
        if startup {
            if let Some(b) = self.budget.get_mut(g) {
                *b = b.saturating_sub(1);
            }
        } else if let Some(b) = self.backward_budget.get_mut(g) {
            *b = b.saturating_sub(1);
        }
        let raw_hex = format!("0x{raw:010X}");
        let prev_hex = prev.map(|p| format!("0x{p:010X}"));
        info!(
            group = g,
            raw = %raw_hex,
            prev = ?prev_hex,
            backward = backward,
            backward_total = self.backward_count.get(g).copied().unwrap_or(0),
            ts_ns = raw as f64 * 5.0,
            "V1743 TDC diag"
        );
    }
}

// Re-exports
pub use crate::config::FirmwareType;

use crate::config::devtree_paths as devtree;
pub use caen::{CaenError, CaenHandle, EndpointHandle, OpenDppEvent};
pub use decoder::{
    AMaxConfig, AMaxDecoder, DataType, DecodeResult, EventData, Pha1Config, Pha1Decoder,
    Pha2Config, Pha2Decoder, Psd1Config, Psd1Decoder, Psd2Config, Psd2Decoder, Waveform,
};

use crate::common::{
    handle_command, pub_no_hwm, run_command_task, CommandHandlerExt, ComponentSharedState,
    ComponentState, EventData as CommonEventData, Message, Waveform as CommonWaveform,
};
use futures::SinkExt;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tmq::publish;
use tmq::Context;
use tokio::sync::{mpsc, watch, Mutex};
use tokio::time::interval;
use tracing::{debug, error, info, warn};

/// Reader error type
#[derive(Debug, Error)]
pub enum ReaderError {
    #[error("CAEN error: {0}")]
    Caen(#[from] CaenError),

    #[error("ZMQ error: {0}")]
    Zmq(#[from] tmq::TmqError),

    #[error("MessagePack serialization error: {0}")]
    MsgPack(#[from] rmp_serde::encode::Error),

    #[error("Decode error: {0}")]
    Decode(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Channel send error")]
    ChannelSend,
}

/// Internal message type from ReadLoop to DecodeLoop
///
/// Supports both RAW data (requiring decoding) and pre-decoded events (from OpenDPP).
/// `Decoded` boxes its payload because `EventData` carries the (now 3-analog /
/// 5-digital) `Waveform` inline; without the box clippy flags the variant size
/// difference vs `Raw(decoder::RawData)`.
pub(crate) enum ReadLoopOutput {
    /// Raw data that needs decoding (PSD1/PSD2/PHA1/PHA2)
    Raw(decoder::RawData),
    /// Already decoded event (x743 — CAENDigitizer's DecodeEvent needs the
    /// handle and is not thread-safe, so x743 converts inside its ReadLoop).
    /// Only the feature-gated x743 read loop constructs this variant.
    #[cfg_attr(not(feature = "x743"), allow(dead_code))]
    Decoded(Box<decoder::EventData>),
    /// Untranslated OpenDPP event (AMax). The 4-lane debug unpack +
    /// EventData conversion is CPU-heavy (2000 samples → 8 probe Vecs), so
    /// it runs on the decode workers — the ReadLoop stays read-only.
    /// `enable_acq` snapshots the debug-FW flag at read time (it can change
    /// mid-session via Tune Up hot-swap).
    OpenDpp {
        event: Box<caen::OpenDppEvent>,
        enable_acq: bool,
    },
    /// Start signal — triggers decoder state reset (RolloverTracker etc.)
    Start,
    /// Stop signal — triggers EOS publication in DecodeLoop
    Stop,
}

/// AMax timebase: 1 LSB of `OpenDppEvent.timestamp` = 8 ns.
const AMAX_TIME_STEP_NS: f64 = 8.0;
/// AMax fine-timestamp scale: 10-bit field, divide by 1024 to get fractional ns.
const AMAX_FINE_TIME_SCALE: f64 = 1024.0;
/// AMax wire format: `flags_a` is the 8-bit high half, `flags_b` is the 12-bit
/// low half. The combined u32 sits in `EventData.flags` lower 20 bits.
const AMAX_FLAGS_A_SHIFT: u32 = 12;

/// Convert an `OpenDppEvent` from the FELib AMax endpoint into the unified
/// `decoder::EventData` used by the rest of the pipeline.
///
/// AMax is the only firmware where the FELib gives us *pre-decoded* events
/// (raw aggregate parsing happens inside `libCAEN_FELib`), so this function
/// is just a field-by-field translation rather than the bit-twiddling decoder
/// we use for PSD1/PSD2/PHA1/PHA2.
///
/// # Wire-format notes
///
/// * `flags = (flags_a << 12) | flags_b` — the 20-bit AMax flags field is
///   split across two FFI fields. Width is firmware-defined (CAEN AMax
///   `documentation_2026030952/`).
/// * `user_info` — AMax FW emits `[peak, baseline, fw_specific0, fw_specific1]`
///   as `Vec<u64>` over FFI. We copy the first 4 slots into a fixed-size
///   array; if the FW ever emits more we log once at `info!` instead of
///   silently truncating (per CLAUDE.md "Silent failure を作らない").
/// * `(v & 0x3FFF) as i16` — AMax raw ADC samples are *unsigned* 14-bit
///   ([0, 16383]). The cast to `i16` matches `Waveform::analog_probe1`'s
///   storage type; `analog_probe1_is_signed = false` flags this for the
///   frontend so it doesn't apply the +8191 centering offset that signed
///   PHA1 trapezoid probes need.
/// * `analog_probe_type` / `digital_probe_type` are `UNKNOWN_PROBE_TYPE`
///   because OpenDPP doesn't carry probe-type metadata on the wire (added
///   in Phase 4.5 for PHA2 only).
///
/// # AMax debug FW (2026-05-25)
///
/// When `ENABLE_ACQ = 1` on the `AMAX_firmware32_channel_4input_caenlist`
/// 13may FW, events arrive with the WAVE payload packed as **4 interleaved
/// 16-bit lanes** per source sample (raw / trap / triangle / digital).
/// `ENABLE_ACQ` lives on the per-channel page (`page_amax_energy_4_<N>`)
/// but the operator UI flips it via the broadcast page (word 0x200), which
/// fans out to all 32 channels — so when debug mode is on, *every* channel
/// emits 4-lane payload. The lane structure is recovered by the
/// `enable_acq_debug` branch in this fn (see [`unpack_amax_debug_waveform`]).
///
/// Caller (`read_loop_dig2`) tracks `enable_acq_debug` on
/// `DeviceConnection::amax_enable_acq`, refreshed from
/// `channel_defaults.amax.enable_acq` after every successful Apply /
/// auto-load. (Earlier builds gated on `event.channel == 0` — that was
/// only correct for the very first test FW shipped by Rebeca, which had
/// the debug MUX wired to ch0 only. The 13may caenlist FW replicates the
/// MUX per channel, so the gate has been removed.)
///
/// Offline `.delila` replays (captured with `caen_simple_test`) take a
/// different path through `AMaxDecoder::decode_event`, which reads the
/// raw u64 packed words directly and drives the 4-lane path off the
/// per-event SE bit — same behavior, just a different signal carrier.
pub(crate) fn opendpp_to_event_data(
    event: &OpenDppEvent,
    module_id: u8,
    enable_acq_debug: bool,
) -> decoder::EventData {
    let coarse_time_ns = (event.timestamp as f64) * AMAX_TIME_STEP_NS;
    let fine_time_ns = (event.fine_timestamp as f64 / AMAX_FINE_TIME_SCALE) * AMAX_TIME_STEP_NS;
    let timestamp_ns = coarse_time_ns + fine_time_ns;

    let flags = ((event.flags_a as u32) << AMAX_FLAGS_A_SHIFT) | (event.flags_b as u32);

    // Copy first 4 user_info slots into the fixed-size array. AMax FW emits
    // [0]=peak, [1]=baseline, [2..=3]=FW-specific. Missing slots stay 0.
    let mut user_info = [0u64; 4];
    for (i, slot) in event.user_info.iter().take(4).enumerate() {
        user_info[i] = *slot;
    }
    if event.user_info.len() > 4 {
        // Truncation is intentional (`EventData.user_info` is fixed-size to
        // keep the hot path zero-alloc), but FW that emits >4 slots is a
        // signal something has changed upstream. Log once-per-process so we
        // notice without flooding at MHz rates.
        static OVERFLOW_LOGGED: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if !OVERFLOW_LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            info!(
                slots = event.user_info.len(),
                module = module_id,
                "[AMax] OpenDPP user_info has >4 slots — truncating to 4 (one-shot)"
            );
        }
    }

    // Two waveform-format paths:
    //   * `enable_acq_debug` (and the sample buffer is a multiple of 4)
    //     → AMax debug FW 4-lane interleaved unpack (raw / trap /
    //     triangle / digital). Applies to *all* channels because the
    //     broadcast ENABLE_ACQ write fans out across the 32-channel page
    //     array — confirmed live on 2026-05-25: with ch0's ENABLE_ACQ on
    //     via broadcast, ch4 also delivered 4-lane encoded payload.
    //   * everything else → legacy single-lane raw waveform.
    //
    // `samples.len() % 4 != 0` would indicate FW desync (single-lane
    // payload arriving while we think we're in debug mode). Fall back
    // to single-lane in that case to avoid panicking on
    // `chunks_exact(4)` truncation — better to show partial data than
    // crash the read loop.
    let waveform = event.waveform.as_ref().map(|samples| {
        if enable_acq_debug && samples.len() % 4 == 0 {
            unpack_amax_debug_waveform(samples)
        } else {
            let analog_probe1 = samples.iter().map(|&v| (v & 0x3FFF) as i16).collect();
            decoder::common::Waveform {
                analog_probe1,
                analog_probe2: Vec::new(),
                analog_probe3: Vec::new(),
                digital_probe1: Vec::new(),
                digital_probe2: Vec::new(),
                digital_probe3: Vec::new(),
                digital_probe4: Vec::new(),
                digital_probe5: Vec::new(),
                digital_probe6: Vec::new(),
                digital_probe7: Vec::new(),
                digital_probe8: Vec::new(),
                digital_probe9: Vec::new(),
                digital_probe10: Vec::new(),
                digital_probe11: Vec::new(),
                digital_probe12: Vec::new(),
                digital_probe13: Vec::new(),
                digital_probe14: Vec::new(),
                digital_probe15: Vec::new(),
                digital_probe16: Vec::new(),
                time_resolution: 0,
                trigger_threshold: 0,
                ns_per_sample: AMAX_TIME_STEP_NS,
                analog_probe1_is_signed: false,
                analog_probe2_is_signed: false,
                analog_probe3_is_signed: false,
                analog_probe_type: [decoder::common::UNKNOWN_PROBE_TYPE; 3],
                digital_probe_type: [decoder::common::UNKNOWN_PROBE_TYPE; 16],
            }
        }
    });

    decoder::EventData {
        timestamp_ns,
        module: module_id,
        channel: event.channel,
        energy: event.energy,
        energy_short: event.psd, // PSD value stored in energy_short
        fine_time: event.fine_timestamp,
        flags,
        user_info,
        waveform,
    }
}

/// Unpack the AMax debug FW's 4-lane interleaved waveform payload.
///
/// FELib's OpenDPP endpoint passes the FW's u64-packed wire data through
/// to the `WAVEFORM` u16 array without recognising the lane structure.
/// Each source sample occupies 4 consecutive u16 slots:
///
/// | offset (mod 4) | lane | content                                | slot              |
/// |----------------|------|----------------------------------------|-------------------|
/// | 0              | 0    | raw waveform (signed 16-bit ADC)       | `analog_probe1`   |
/// | 1              | 1    | trapezoidal filter (signed)            | `analog_probe2`   |
/// | 2              | 2    | triangle filter (signed)               | `analog_probe3`   |
/// | 3              | 3    | digital lane (16 wires; bits 11..15)   | `digital_probe1..5` |
///
/// Digital lane bit map (from FW SCH `OutputOrder = IN_0 LEFT (MSBs)`,
/// `FW/debug/AMAX_firmware32_channel_4input_caenlist.scf` U75):
///   * bit 15 = Trigger_out → `digital_probe1`
///   * bit 14 = BL_Hold     → `digital_probe2`
///   * bit 13 = Energy_Dv   → `digital_probe3`
///   * bit 12 = shaping_dv  → `digital_probe4`
///   * bit 11 = shaping_track → `digital_probe5`
///   * bits 10..0 = constant 0 (padding for future wires)
///
/// Mirrors `AMaxDecoder::decode_debug_waveform` in
/// [src/reader/decoder/amax.rs] (offline raw-bytes path) — keep them in
/// sync if Rebeca's FW changes the lane layout.
fn unpack_amax_debug_waveform(samples: &[u16]) -> decoder::common::Waveform {
    use decoder::amax::amax_probe_types;
    use decoder::common::{Waveform, UNKNOWN_PROBE_TYPE};

    let n = samples.len() / 4;
    let mut analog_probe1 = Vec::with_capacity(n);
    let mut analog_probe2 = Vec::with_capacity(n);
    let mut analog_probe3 = Vec::with_capacity(n);
    let mut digital_probe1 = Vec::with_capacity(n);
    let mut digital_probe2 = Vec::with_capacity(n);
    let mut digital_probe3 = Vec::with_capacity(n);
    let mut digital_probe4 = Vec::with_capacity(n);
    let mut digital_probe5 = Vec::with_capacity(n);

    for chunk in samples.chunks_exact(4) {
        analog_probe1.push(chunk[0] as i16);
        analog_probe2.push(chunk[1] as i16);
        analog_probe3.push(chunk[2] as i16);
        let dig = chunk[3];
        digital_probe1.push(((dig >> 15) & 0x1) as u8);
        digital_probe2.push(((dig >> 14) & 0x1) as u8);
        digital_probe3.push(((dig >> 13) & 0x1) as u8);
        digital_probe4.push(((dig >> 12) & 0x1) as u8);
        digital_probe5.push(((dig >> 11) & 0x1) as u8);
    }

    Waveform {
        analog_probe1,
        analog_probe2,
        analog_probe3,
        digital_probe1,
        digital_probe2,
        digital_probe3,
        digital_probe4,
        digital_probe5,
        // Slots 6..16 reserved for future digital-lane bit assignments
        // (bits 10..0 are constant 0 in hardware today; see
        // `decoder::common::amax_probe_types` for the reserved range).
        digital_probe6: Vec::new(),
        digital_probe7: Vec::new(),
        digital_probe8: Vec::new(),
        digital_probe9: Vec::new(),
        digital_probe10: Vec::new(),
        digital_probe11: Vec::new(),
        digital_probe12: Vec::new(),
        digital_probe13: Vec::new(),
        digital_probe14: Vec::new(),
        digital_probe15: Vec::new(),
        digital_probe16: Vec::new(),
        time_resolution: 0,
        trigger_threshold: 0,
        ns_per_sample: AMAX_TIME_STEP_NS,
        // Debug-FW analog lanes are signed 16-bit per Rebeca's spec
        // (raw / trap / triangle all swing around 0 after the TM packer).
        analog_probe1_is_signed: true,
        analog_probe2_is_signed: true,
        analog_probe3_is_signed: true,
        analog_probe_type: [
            amax_probe_types::ANA_RAW,
            amax_probe_types::ANA_TRAP,
            amax_probe_types::ANA_TRIANGLE,
        ],
        digital_probe_type: [
            amax_probe_types::DIG_TRIGGER_OUT,
            amax_probe_types::DIG_BL_HOLD,
            amax_probe_types::DIG_ENERGY_DV,
            amax_probe_types::DIG_SHAPING_DV,
            amax_probe_types::DIG_SHAPING_TRACK,
            UNKNOWN_PROBE_TYPE,
            UNKNOWN_PROBE_TYPE,
            UNKNOWN_PROBE_TYPE,
            UNKNOWN_PROBE_TYPE,
            UNKNOWN_PROBE_TYPE,
            UNKNOWN_PROBE_TYPE,
            UNKNOWN_PROBE_TYPE,
            UNKNOWN_PROBE_TYPE,
            UNKNOWN_PROBE_TYPE,
            UNKNOWN_PROBE_TYPE,
            UNKNOWN_PROBE_TYPE,
        ],
    }
}

/// True when the AMax debug FW's `ENABLE_ACQ` register is set to 1 in the
/// applied config. Used by the OpenDPP read loop to decide whether to
/// unpack the 4-lane debug waveform on ch0.
///
/// Non-AMax firmwares always return false (the `amax` channel section is
/// `None`).
pub(crate) fn amax_enable_acq_from_config(
    config: &crate::config::digitizer::DigitizerConfig,
) -> bool {
    config
        .channel_defaults
        .amax
        .as_ref()
        .and_then(|a| a.enable_acq)
        .map(|v| v == 1)
        .unwrap_or(false)
}

/// Enum-based decoder dispatch (KISS: PSD1/PSD2/PHA1/PHA2/AMax, no trait object needed)
enum DecoderKind {
    Psd2(Psd2Decoder),
    Psd1(Psd1Decoder),
    Pha1(Pha1Decoder),
    Pha2(Pha2Decoder),
    AMax(AMaxDecoder),
}

impl DecoderKind {
    /// Build the decoder matching the reader's firmware type. Each
    /// dispatcher/worker thread owns an independent instance.
    fn for_config(config: &ReaderConfig) -> Self {
        match config.firmware {
            FirmwareType::PSD2 => {
                let psd2_config = Psd2Config {
                    time_step_ns: config.time_step_ns,
                    module_id: config.module_id,
                    dump_enabled: false,
                    num_channels: 32,
                };
                DecoderKind::Psd2(Psd2Decoder::new(psd2_config))
            }
            FirmwareType::PSD1 => {
                let psd1_config = Psd1Config {
                    time_step_ns: config.time_step_ns,
                    module_id: config.module_id,
                    dump_enabled: false,
                };
                DecoderKind::Psd1(Psd1Decoder::new(psd1_config))
            }
            FirmwareType::PHA1 => {
                let pha1_config = Pha1Config {
                    time_step_ns: config.time_step_ns,
                    module_id: config.module_id,
                    dump_enabled: false,
                };
                DecoderKind::Pha1(Pha1Decoder::new(pha1_config))
            }
            FirmwareType::PHA2 => {
                let pha2_config = Pha2Config {
                    time_step_ns: config.time_step_ns,
                    module_id: config.module_id,
                    dump_enabled: false,
                    num_channels: 32,
                };
                DecoderKind::Pha2(Pha2Decoder::new(pha2_config))
            }
            FirmwareType::AMax => {
                let amax_config = AMaxConfig {
                    module_id: config.module_id,
                    dump_enabled: false,
                    num_channels: 1, // AMax typically uses only ch0
                };
                DecoderKind::AMax(AMaxDecoder::new(amax_config))
            }
            FirmwareType::X743CI | FirmwareType::X743Std => {
                // x743 only uses the Decoded path (no Raw data to decode).
                // Create a dummy decoder that won't be called.
                let psd2_config = Psd2Config {
                    time_step_ns: 0.3125, // x743 TDC is 5ns but unused here
                    module_id: config.module_id,
                    dump_enabled: false,
                    num_channels: 16,
                };
                DecoderKind::Psd2(Psd2Decoder::new(psd2_config))
            }
        }
    }

    fn classify(&self, raw: &decoder::RawData) -> DataType {
        match self {
            Self::Psd2(d) => d.classify(raw),
            Self::Psd1(d) => d.classify(raw),
            Self::Pha1(d) => d.classify(raw),
            Self::Pha2(d) => d.classify(raw),
            Self::AMax(d) => d.classify(raw),
        }
    }

    fn decode_into(&mut self, raw: &decoder::RawData, events: &mut Vec<decoder::EventData>) {
        match self {
            Self::Psd2(d) => d.decode_into(raw, events),
            Self::Psd1(d) => d.decode_into(raw, events),
            Self::Pha1(d) => d.decode_into(raw, events),
            Self::Pha2(d) => d.decode_into(raw, events),
            Self::AMax(d) => {
                // AMax decoder returns AMaxEventData, extract base EventData
                let mut amax_events = Vec::new();
                d.decode_into(raw, &mut amax_events);
                events.extend(amax_events.into_iter().map(|e| e.base));
            }
        }
    }

    /// Reset decoder state for a new run (SW Fine TS rollover tracking)
    fn reset_for_new_run(&mut self) {
        match self {
            Self::Psd1(d) => d.reset_for_new_run(),
            Self::Pha1(d) => d.reset_for_new_run(),
            Self::Pha2(d) => d.reset_for_new_run(),
            Self::Psd2(_) | Self::AMax(_) => {} // No run-level state to reset
        }
    }

    /// DIG1 only: sequentially extend each board aggregate's 32-bit time tag
    /// (rollover tracking) without decoding events. One entry per aggregate.
    /// No-op for other firmware (their timestamps are self-contained).
    fn scan_extended_btts(&mut self, raw: &decoder::RawData, out: &mut Vec<u64>) {
        match self {
            Self::Psd1(d) => d.scan_extended_btts(raw, out),
            Self::Pha1(d) => d.scan_extended_btts(raw, out),
            Self::Psd2(_) | Self::Pha2(_) | Self::AMax(_) => out.clear(),
        }
    }

    /// Decode with pre-computed extended BTTs (DIG1 parallel path). For
    /// firmware without sequential timestamp state, or when `btts` is
    /// `None`, falls back to the plain stateful decode.
    fn decode_into_with_btts(
        &mut self,
        raw: &decoder::RawData,
        btts: Option<&[u64]>,
        events: &mut Vec<decoder::EventData>,
    ) {
        match (&mut *self, btts) {
            (Self::Psd1(d), Some(b)) => d.decode_into_with_btts(raw, b, events),
            (Self::Pha1(d), Some(b)) => d.decode_into_with_btts(raw, b, events),
            _ => self.decode_into(raw, events),
        }
    }
}

/// Reader configuration
#[derive(Debug, Clone)]
pub struct ReaderConfig {
    /// Device URL (e.g., "dig2://172.18.4.56")
    pub url: String,
    /// ZMQ data publish address
    pub data_address: String,
    /// ZMQ command address (REP socket)
    pub command_address: String,
    /// Source ID for this reader
    pub source_id: u32,
    /// Firmware type (determines decoder)
    pub firmware: FirmwareType,
    /// Module ID for decoded events
    pub module_id: u8,
    /// Read timeout in milliseconds
    pub read_timeout_ms: i32,
    /// Buffer size for raw data reads
    pub buffer_size: usize,
    /// Heartbeat interval in milliseconds (0 = disabled)
    pub heartbeat_interval_ms: u64,
    /// Time step in nanoseconds (for timestamp calculation)
    pub time_step_ns: f64,
    /// Path to digitizer configuration JSON file (optional)
    pub config_file: Option<String>,
    /// Minimum ADC value filter. Events with energy < adc_min are discarded.
    /// 0 = no filtering (default).
    pub adc_min: u16,
    /// Number of parallel decode worker threads. 0 = auto (half the logical
    /// CPUs minus one, clamped to [1, 8]).
    pub decode_workers: usize,
}

impl Default for ReaderConfig {
    fn default() -> Self {
        Self {
            url: "dig2://localhost".to_string(),
            data_address: "tcp://*:5555".to_string(),
            command_address: "tcp://*:5556".to_string(),
            source_id: 0,
            firmware: FirmwareType::PSD2,
            module_id: 0,
            read_timeout_ms: 100,
            buffer_size: 64 * 1024 * 1024, // 64MB - CAEN FELib has no bounds check
            heartbeat_interval_ms: 1000,
            time_step_ns: 2.0, // 500 MHz ADC = 2ns per sample
            config_file: None,
            adc_min: 0,
            decode_workers: 0,
        }
    }
}

impl ReaderConfig {
    /// Create ReaderConfig from Config and source ID
    ///
    /// Returns None if source_id is not found or source has no digitizer_url
    pub fn from_config(config: &crate::config::Config, source_id: u32) -> Option<Self> {
        let source = config.get_source(source_id)?;

        let firmware = match source.source_type {
            crate::config::SourceType::Psd2 => FirmwareType::PSD2,
            crate::config::SourceType::Psd1 => FirmwareType::PSD1,
            crate::config::SourceType::Pha1 => FirmwareType::PHA1,
            crate::config::SourceType::Pha2 => FirmwareType::PHA2,
            crate::config::SourceType::AMax => FirmwareType::AMax,
            crate::config::SourceType::X743CI => {
                tracing::warn!(
                    "SourceType::X743CI (DPP-CI Charge Mode) is deprecated — no TDC available. \
                     Falling back to Standard mode. Update TOML to source_type = \"x743_std\"."
                );
                FirmwareType::X743Std
            }
            crate::config::SourceType::X743Std => FirmwareType::X743Std,
            // Emulator/Zle sources shouldn't create a Reader — caller should handle
            _ => return None,
        };

        // x743 doesn't use FELib URL — connection is via X743Config (link_type/link_num/conet_node)
        let url = if firmware.is_legacy_api() {
            source.digitizer_url.clone().unwrap_or_default()
        } else {
            source.digitizer_url.as_ref()?.clone()
        };

        Some(Self {
            url,
            data_address: source.data_address(config.network.port_base_data),
            command_address: source.command_address_with_base(config.network.port_base_command),
            source_id,
            firmware,
            module_id: source.module_id.unwrap_or(source_id as u8),
            read_timeout_ms: 100,
            buffer_size: 64 * 1024 * 1024, // 64MB - CAEN FELib has no bounds check
            heartbeat_interval_ms: 1000,
            time_step_ns: source.time_step_ns.unwrap_or(2.0),
            config_file: source.config_file.clone(),
            adc_min: source.adc_min,
            decode_workers: source.decode_workers,
        })
    }
}

/// Metrics for monitoring
/// Maximum channels per digitizer (DT5725S = 32ch, DT5730B = 16ch)
pub const MAX_CHANNELS: usize = 32;

#[derive(Debug)]
pub struct ReaderMetrics {
    /// Total events decoded
    pub events_decoded: AtomicU64,
    /// Total bytes read from digitizer
    pub bytes_read: AtomicU64,
    /// Total batches published
    pub batches_published: AtomicU64,
    /// Current decode queue length (approximate)
    pub queue_length: AtomicU64,
    /// Cumulative trigger loss count (DIG1: flag-based estimate, DIG2: counter-based exact)
    pub trigger_loss_count: AtomicU64,
    /// Events with trigger_lost flag set (DIG1 only)
    pub trigger_lost_flag_events: AtomicU64,
    /// Events with n_lost_trigger flag set (DIG1 only)
    pub n_lost_trigger_flag_events: AtomicU64,
    /// Per-channel cumulative event counts (index = channel number)
    pub per_channel_counts: [AtomicU64; MAX_CHANNELS],
    /// Events filtered out by adc_min threshold
    pub filtered_events: AtomicU64,
    /// Run number of the current run (set on Start). Threaded into EndOfStream
    /// messages so the Recorder's stale-EOS filter matches instead of
    /// discarding every real EOS (TODO 58 C1 — run_number was hardcoded 0).
    pub current_run: AtomicU32,
}

impl Default for ReaderMetrics {
    fn default() -> Self {
        Self {
            events_decoded: AtomicU64::new(0),
            bytes_read: AtomicU64::new(0),
            batches_published: AtomicU64::new(0),
            queue_length: AtomicU64::new(0),
            trigger_loss_count: AtomicU64::new(0),
            trigger_lost_flag_events: AtomicU64::new(0),
            n_lost_trigger_flag_events: AtomicU64::new(0),
            per_channel_counts: std::array::from_fn(|_| AtomicU64::new(0)),
            filtered_events: AtomicU64::new(0),
            current_run: AtomicU32::new(0),
        }
    }
}

/// Rate tracker for 1-second interval rate calculation
#[derive(Debug)]
struct RateTracker {
    prev_events: AtomicU64,
    prev_time: std::sync::Mutex<Option<Instant>>,
    current_rate: AtomicU64,
}

impl RateTracker {
    fn new() -> Self {
        Self {
            prev_events: AtomicU64::new(0),
            prev_time: std::sync::Mutex::new(None),
            current_rate: AtomicU64::new(0),
        }
    }

    fn update(&self, current_events: u64) {
        let now = Instant::now();
        let mut prev_time_guard = self.prev_time.lock().unwrap();

        if let Some(prev_time) = *prev_time_guard {
            let elapsed = now.duration_since(prev_time).as_secs_f64();
            if elapsed >= 1.0 {
                let prev_events = self.prev_events.load(Ordering::Relaxed);
                let delta = current_events.saturating_sub(prev_events);
                let rate = (delta as f64 / elapsed) as u64;
                self.current_rate.store(rate, Ordering::Relaxed);
                self.prev_events.store(current_events, Ordering::Relaxed);
                *prev_time_guard = Some(now);
            }
        } else {
            self.prev_events.store(current_events, Ordering::Relaxed);
            *prev_time_guard = Some(now);
        }
    }

    fn get_rate(&self) -> f64 {
        self.current_rate.load(Ordering::Relaxed) as f64
    }

    fn reset(&self) {
        self.prev_events.store(0, Ordering::Relaxed);
        self.current_rate.store(0, Ordering::Relaxed);
        *self.prev_time.lock().unwrap() = None;
    }
}

/// Request from command handler to read_loop.
/// Delegates hardware operations to the read_loop's existing CaenHandle
/// to avoid opening multiple FELib connections.
pub(crate) enum ReadLoopRequest {
    /// Detect: read device info from hardware
    Detect {
        response_tx: std::sync::mpsc::SyncSender<Result<serde_json::Value, String>>,
    },
    /// Apply digitizer configuration to hardware
    ApplyConfig {
        config: Box<crate::config::digitizer::DigitizerConfig>,
        response_tx: std::sync::mpsc::SyncSender<Result<usize, String>>,
    },
    /// Apply only SetInRun parameters while running
    ApplyConfigRunning {
        config: Box<crate::config::digitizer::DigitizerConfig>,
        response_tx: std::sync::mpsc::SyncSender<Result<usize, String>>,
    },
    /// Read back AMax board-level user registers from live hardware.
    /// Used by the operator UI's Tune Up debug view to show what
    /// ENABLE_ACQ (and any future board-level register) is actually
    /// set to on the board, vs what's stored in the config file.
    ReadAmaxBoardRegisters {
        response_tx: std::sync::mpsc::SyncSender<Result<Vec<(String, u32)>, String>>,
    },
}

/// Command handler extension for Reader
struct ReaderCommandExt {
    metrics: Arc<ReaderMetrics>,
    rate_tracker: Arc<RateTracker>,
    /// Channel to delegate hardware requests to the read_loop's existing CaenHandle
    request_tx: std::sync::mpsc::Sender<ReadLoopRequest>,
    /// Hardware-confirmed state (updated by ReadLoop after actual HW transitions).
    /// GetStatus reports the minimum of software state and this value so that
    /// the Operator doesn't proceed until hardware is truly ready.
    hw_state: Arc<std::sync::Mutex<ComponentState>>,
}

impl CommandHandlerExt for ReaderCommandExt {
    fn component_name(&self) -> &'static str {
        "Reader"
    }

    fn status_details(&self) -> Option<String> {
        let events = self.metrics.events_decoded.load(Ordering::Relaxed);
        let batches = self.metrics.batches_published.load(Ordering::Relaxed);
        let bytes = self.metrics.bytes_read.load(Ordering::Relaxed);
        Some(format!(
            "Events: {}, Batches: {}, Bytes: {}",
            events, batches, bytes
        ))
    }

    fn get_metrics(&self) -> Option<crate::common::ComponentMetrics> {
        let events = self.metrics.events_decoded.load(Ordering::Relaxed);
        let bytes = self.metrics.bytes_read.load(Ordering::Relaxed);
        let queue = self.metrics.queue_length.load(Ordering::Relaxed);
        let trigger_loss = self.metrics.trigger_loss_count.load(Ordering::Relaxed);
        self.rate_tracker.update(events);
        let loss_rate = if events > 0 {
            (trigger_loss as f64 / (events as f64 + trigger_loss as f64)) * 100.0
        } else {
            0.0
        };
        Some(crate::common::ComponentMetrics {
            events_processed: events,
            bytes_transferred: bytes,
            queue_size: queue as u32,
            queue_max: 0,
            queue_bytes: 0,
            queue_bytes_peak: 0,
            backlog_level: 0,
            event_rate: self.rate_tracker.get_rate(),
            data_rate: 0.0,
            trigger_loss_count: trigger_loss,
            trigger_loss_rate: loss_rate,
            channel_counts: Some(
                self.metrics
                    .per_channel_counts
                    .iter()
                    .map(|c| c.load(Ordering::Relaxed))
                    .collect(),
            ),
        })
    }

    fn effective_state(&self, software_state: ComponentState) -> ComponentState {
        effective_state_for(software_state, *self.hw_state.lock().unwrap())
    }

    fn on_start(&mut self, run_number: u32) -> Result<(), String> {
        self.rate_tracker.reset();
        // Record the run number so EOS messages carry it (TODO 58 C1) —
        // the Recorder discards EOS whose run_number mismatches the run.
        self.metrics
            .current_run
            .store(run_number, Ordering::Relaxed);
        // Reset all metrics for new run
        self.metrics.events_decoded.store(0, Ordering::Relaxed);
        self.metrics.bytes_read.store(0, Ordering::Relaxed);
        self.metrics.batches_published.store(0, Ordering::Relaxed);
        self.metrics.trigger_loss_count.store(0, Ordering::Relaxed);
        self.metrics
            .trigger_lost_flag_events
            .store(0, Ordering::Relaxed);
        self.metrics
            .n_lost_trigger_flag_events
            .store(0, Ordering::Relaxed);
        self.metrics.filtered_events.store(0, Ordering::Relaxed);
        for ch in &self.metrics.per_channel_counts {
            ch.store(0, Ordering::Relaxed);
        }
        Ok(())
    }

    fn on_detect(&mut self) -> Result<serde_json::Value, String> {
        // Delegate to read_loop which owns the CaenHandle.
        // This avoids opening a second FELib connection that would
        // interfere with the existing one.
        let (resp_tx, resp_rx) = std::sync::mpsc::sync_channel(1);
        self.request_tx
            .send(ReadLoopRequest::Detect {
                response_tx: resp_tx,
            })
            .map_err(|_| "ReadLoop not running".to_string())?;
        resp_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| "Detect timeout: ReadLoop did not respond".to_string())?
    }

    fn on_apply_digitizer_config(
        &mut self,
        config: &crate::config::digitizer::DigitizerConfig,
    ) -> Result<usize, String> {
        let (resp_tx, resp_rx) = std::sync::mpsc::sync_channel(1);
        self.request_tx
            .send(ReadLoopRequest::ApplyConfig {
                config: Box::new(config.clone()),
                response_tx: resp_tx,
            })
            .map_err(|_| "ReadLoop not running".to_string())?;
        // 10s timeout: USB digitizers (DT5730B) can be slow
        resp_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| "ApplyConfig timeout: ReadLoop did not respond within 10s".to_string())?
    }

    fn on_apply_digitizer_config_running(
        &mut self,
        config: &crate::config::digitizer::DigitizerConfig,
    ) -> Result<usize, String> {
        let (resp_tx, resp_rx) = std::sync::mpsc::sync_channel(1);
        self.request_tx
            .send(ReadLoopRequest::ApplyConfigRunning {
                config: Box::new(config.clone()),
                response_tx: resp_tx,
            })
            .map_err(|_| "ReadLoop not running".to_string())?;
        resp_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| {
                "ApplyConfigRunning timeout: ReadLoop did not respond within 10s".to_string()
            })?
    }

    fn on_read_amax_board_registers(&mut self) -> Result<Vec<(String, u32)>, String> {
        let (resp_tx, resp_rx) = std::sync::mpsc::sync_channel(1);
        self.request_tx
            .send(ReadLoopRequest::ReadAmaxBoardRegisters {
                response_tx: resp_tx,
            })
            .map_err(|_| "ReadLoop not running".to_string())?;
        resp_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| {
                "ReadAmaxBoardRegisters timeout: ReadLoop did not respond within 2s".to_string()
            })?
    }
}

/// Bundles CaenHandle + EndpointHandle + hardware state tracking.
///
/// When dropped, endpoint is dropped before handle (Rust struct field drop order),
/// ensuring the endpoint is released before the connection is closed.
pub(crate) struct DeviceConnection {
    pub(crate) handle: CaenHandle,
    pub(crate) endpoint: EndpointHandle,
    /// Whether digitizer config has been applied since connection
    pub(crate) hw_configured: bool,
    /// Whether digitizer has been armed
    pub(crate) hw_armed: bool,
    /// Whether acquisition is running
    pub(crate) hw_running: bool,
    /// Auto-configure from JSON file failed — block Arm until Operator sends valid config
    pub(crate) auto_config_failed: bool,
    /// Backoff for failed Arm/Start hardware commands (TODO 58 M2). On failure
    /// the state-sync loop must NOT claim `hw_armed`/`hw_running` (that would
    /// show the operator a data-less "Armed"/"Running"), but the loop iterates
    /// every ~10 ms, so an unconditional retry would flood the log. Attempts
    /// are skipped until this instant.
    pub(crate) hw_cmd_retry_after: Option<std::time::Instant>,
    /// Cached DevTree parameter metadata for validation (None if fetch failed)
    pub(crate) param_cache: Option<std::collections::HashMap<String, caen::handle::ParamInfo>>,
    /// Enabled channel indices (for DIG2 counter polling)
    pub(crate) enabled_channels: Vec<u8>,
    /// Whether the (re)configured OpenDPP endpoint includes WAVEFORM
    /// fields. Selects between `read_opendpp_event` and
    /// `read_opendpp_event_with_waveform` on the read hot path.
    pub(crate) include_waveform: bool,
    /// Whether the most recently applied AMax config has `ENABLE_ACQ = 1`.
    /// Drives the 4-lane debug-waveform unpack in
    /// `opendpp_to_event_data`. Refreshed via
    /// `amax_enable_acq_from_config` after every successful Apply /
    /// auto-load. Always `false` for non-AMax firmwares.
    pub(crate) amax_enable_acq: bool,
}

/// Verify the digitizer's reported firmware matches the config-declared
/// firmware. Returns `Ok(())` if they agree, `Err(detailed_message)` on
/// mismatch or if `get_device_info()` itself fails.
///
/// Used by both DIG1 and DIG2 ApplyConfig handlers to hard-fail before
/// `apply_config_validated` sends 30+ params that the wrong firmware
/// would FELib-reject (silent miswire mode discovered 2026-05-07: PHA2
/// config sent to AMax HW caused 31/43 params to be silently skipped
/// while the operator UI reported "Configured" success).
///
/// X743 family is intentionally **not** routed through this check — it
/// uses `read_loop_x743_std` with `CaenLegacyHandle` (CAENDigitizer
/// Library), not FELib, so no `/par/FwType` round-trip exists.
pub(crate) fn check_firmware_match(
    conn: &DeviceConnection,
    url: &str,
    declared: crate::config::digitizer::FirmwareType,
) -> Result<(), String> {
    use crate::config::digitizer::FirmwareType;
    let info = conn.handle.get_device_info().map_err(|e| {
        format!(
            "Failed to read device info from digitizer at {} (cannot verify firmware): {}",
            url, e
        )
    })?;
    let detected = FirmwareType::from_caen_device(&info.firmware_type, &info.model);
    if detected == Some(declared) {
        return Ok(());
    }
    let detected_label = match detected {
        Some(fw) => format!("{:?}", fw),
        None => "<unrecognized>".to_string(),
    };
    Err(format!(
        "Firmware mismatch: digitizer at {} reports firmware \"{}\" model \"{}\" SN \"{}\" \
         (mapped to {}), but config declares firmware {:?}. Refusing to Apply — reload the \
         correct config or update the source's `type` field, then re-Configure.",
        url, info.firmware_type, info.model, info.serial_number, detected_label, declared
    ))
}

/// Try to connect to a digitizer and configure the RAW endpoint.
/// Returns None on failure (non-fatal — ReadLoop stays alive).
pub(crate) fn try_connect_raw(url: &str, include_n_events: bool) -> Option<DeviceConnection> {
    match CaenHandle::open(url) {
        Ok(h) => match h.configure_endpoint(include_n_events) {
            Ok(ep) => {
                info!("Connected to digitizer (RAW endpoint)");
                // Build param cache from DevTree (best-effort)
                let param_cache = match h.build_param_cache() {
                    Ok(cache) => {
                        info!(params = cache.len(), "Parameter cache built");
                        Some(cache)
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to build param cache, validation disabled");
                        None
                    }
                };
                Some(DeviceConnection {
                    handle: h,
                    endpoint: ep,
                    hw_configured: false,
                    hw_armed: false,
                    hw_running: false,
                    auto_config_failed: false,
                    hw_cmd_retry_after: None,
                    param_cache,
                    enabled_channels: Vec::new(),
                    include_waveform: false, // RAW path doesn't use OpenDPP waveforms
                    amax_enable_acq: false,
                })
            }
            Err(e) => {
                error!(error = %e, "Connected but endpoint configuration failed");
                None // h drops here → CAEN_FELib_Close
            }
        },
        Err(e) => {
            warn!(error = %e, "Failed to connect to digitizer");
            None
        }
    }
}

/// Try to connect to a digitizer and configure the **RawUDP** endpoint
/// (AMax / DPP_OPEN 10G firmware bulk raw readout). Returns None on failure
/// (non-fatal — ReadLoop stays alive). Mirrors [`try_connect_raw`] but uses
/// the `rawudp` endpoint that the open-DPP firmware exposes (the generic
/// `RAW` endpoint does not exist on this firmware — see
/// `configure_rawudp_endpoint`).
pub(crate) fn try_connect_rawudp(url: &str) -> Option<DeviceConnection> {
    match CaenHandle::open(url) {
        Ok(h) => match h.configure_rawudp_endpoint() {
            Ok(ep) => {
                info!("Connected to digitizer (RawUDP endpoint)");
                let param_cache = match h.build_param_cache() {
                    Ok(cache) => {
                        info!(params = cache.len(), "Parameter cache built");
                        Some(cache)
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to build param cache, validation disabled");
                        None
                    }
                };
                Some(DeviceConnection {
                    handle: h,
                    endpoint: ep,
                    hw_configured: false,
                    hw_armed: false,
                    hw_running: false,
                    auto_config_failed: false,
                    hw_cmd_retry_after: None,
                    param_cache,
                    enabled_channels: Vec::new(),
                    include_waveform: false, // RawUDP carries waveforms in the raw bytes
                    amax_enable_acq: false,
                })
            }
            Err(e) => {
                error!(error = %e, "Connected but RawUDP endpoint configuration failed");
                None // h drops here → CAEN_FELib_Close
            }
        },
        Err(e) => {
            warn!(error = %e, "Failed to connect to digitizer");
            None
        }
    }
}

/// Try to connect to a digitizer and configure the OpenDPP endpoint.
/// Returns None on failure (non-fatal — ReadLoop stays alive).
///
/// `include_waveform` mirrors `BoardConfig.waveforms_enabled`. AMax callers
/// pass `true` whenever the loaded config asks for waveforms; we still
/// fall back to `false` if no config has been loaded yet at connect time
/// (the endpoint gets re-configured later in the Configure path).
pub(crate) fn try_connect_opendpp(url: &str, include_waveform: bool) -> Option<DeviceConnection> {
    match CaenHandle::open(url) {
        Ok(h) => match h.configure_opendpp_endpoint(include_waveform) {
            Ok(ep) => {
                info!("Connected to digitizer (OpenDPP endpoint)");
                // Build param cache from DevTree (best-effort)
                let param_cache = match h.build_param_cache() {
                    Ok(cache) => {
                        info!(params = cache.len(), "Parameter cache built");
                        Some(cache)
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to build param cache, validation disabled");
                        None
                    }
                };
                Some(DeviceConnection {
                    handle: h,
                    endpoint: ep,
                    hw_configured: false,
                    hw_armed: false,
                    hw_running: false,
                    auto_config_failed: false,
                    hw_cmd_retry_after: None,
                    param_cache,
                    enabled_channels: Vec::new(),
                    include_waveform,
                    amax_enable_acq: false,
                })
            }
            Err(e) => {
                error!(error = %e, "Connected but OpenDPP endpoint configuration failed");
                None
            }
        },
        Err(e) => {
            warn!(error = %e, "Failed to connect to digitizer");
            None
        }
    }
}

/// Extract enabled channel indices from a DigitizerConfig.
pub(crate) fn get_enabled_channels_from_config(
    config: &crate::config::digitizer::DigitizerConfig,
) -> Vec<u8> {
    let default_enabled = config
        .channel_defaults
        .enabled
        .as_deref()
        .is_some_and(|v| v.eq_ignore_ascii_case("true"));
    let mut enabled = Vec::new();
    for ch in 0..config.num_channels {
        let ch_enabled = config
            .channel_overrides
            .get(&ch)
            .and_then(|c| c.enabled.as_deref())
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(default_enabled);
        if ch_enabled {
            enabled.push(ch);
        }
    }
    enabled
}

/// 24-bit counter wraparound-aware difference (for DIG2 FPGA counters).
fn wrapping_diff_24bit(current: u64, prev: u64) -> u64 {
    if current >= prev {
        current - prev
    } else {
        current + 0x100_0000 - prev
    }
}

/// DIG2 trigger counter polling state (tracks across poll intervals for wraparound handling).
pub(crate) struct Dig2PollState {
    prev_trigger: Vec<u64>,
    prev_saved: Vec<u64>,
    accumulated_lost: u64,
    accumulated_trigger: u64,
    initialized: bool,
}

impl Dig2PollState {
    pub(crate) fn new() -> Self {
        Self {
            prev_trigger: Vec::new(),
            prev_saved: Vec::new(),
            accumulated_lost: 0,
            accumulated_trigger: 0,
            initialized: false,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.prev_trigger.clear();
        self.prev_saved.clear();
        self.accumulated_lost = 0;
        self.accumulated_trigger = 0;
        self.initialized = false;
    }
}

/// Poll DIG2 trigger counters and update metrics.
/// Must only be called for DIG2 firmware during Running state.
pub(crate) fn poll_dig2_counters(
    conn: &DeviceConnection,
    poll: &mut Dig2PollState,
    metrics: &ReaderMetrics,
    last_warn: &mut Instant,
) {
    if conn.enabled_channels.is_empty() {
        return;
    }

    // Initialize prev vectors if needed
    if !poll.initialized {
        let n = conn.enabled_channels.len();
        poll.prev_trigger = vec![0; n];
        poll.prev_saved = vec![0; n];
        // Read initial baseline values
        for (i, &ch) in conn.enabled_channels.iter().enumerate() {
            // ChRealtimeMonitor must be read first to latch FPGA counters
            let _ = conn
                .handle
                .get_value(&format!("/ch/{}/par/ChRealtimeMonitor", ch));
            poll.prev_trigger[i] = conn
                .handle
                .get_value(&format!("/ch/{}/par/ChTriggerCnt", ch))
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            poll.prev_saved[i] = conn
                .handle
                .get_value(&format!("/ch/{}/par/ChSavedEventCnt", ch))
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
        }
        poll.initialized = true;
        return;
    }

    for (i, &ch) in conn.enabled_channels.iter().enumerate() {
        // ChRealtimeMonitor must be read first to latch FPGA counters
        let _ = conn
            .handle
            .get_value(&format!("/ch/{}/par/ChRealtimeMonitor", ch));
        let trigger = conn
            .handle
            .get_value(&format!("/ch/{}/par/ChTriggerCnt", ch))
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let saved = conn
            .handle
            .get_value(&format!("/ch/{}/par/ChSavedEventCnt", ch))
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);

        let delta_trigger = wrapping_diff_24bit(trigger, poll.prev_trigger[i]);
        let delta_saved = wrapping_diff_24bit(saved, poll.prev_saved[i]);
        poll.accumulated_trigger += delta_trigger;
        poll.accumulated_lost += delta_trigger.saturating_sub(delta_saved);

        poll.prev_trigger[i] = trigger;
        poll.prev_saved[i] = saved;
    }

    metrics
        .trigger_loss_count
        .store(poll.accumulated_lost, Ordering::Relaxed);

    if poll.accumulated_lost > 0 && last_warn.elapsed() >= Duration::from_secs(10) {
        let rate = if poll.accumulated_trigger > 0 {
            poll.accumulated_lost as f64 / poll.accumulated_trigger as f64 * 100.0
        } else {
            0.0
        };
        warn!(
            total_trigger = poll.accumulated_trigger,
            total_lost = poll.accumulated_lost,
            loss_rate_pct = format!("{:.2}", rate),
            "Trigger loss detected (DIG2 counters)"
        );
        *last_warn = Instant::now();
    }
}

/// Send firmware-specific arm command to the digitizer.
///
/// For DIG1 (PSD1/PHA) with START_MODE_SW, the actual arm is deferred to start phase.
/// For DIG2 (PSD2), always sends armacquisition immediately.
pub(crate) fn send_arm_command(
    handle: &CaenHandle,
    firmware: FirmwareType,
) -> Result<(), caen::CaenError> {
    if firmware.is_dig1() {
        let startmode = handle
            .get_value(devtree::par::START_MODE)
            .unwrap_or_default();
        if startmode == "START_MODE_SW" {
            info!("START_MODE_SW detected - deferring arm to Start");
        } else {
            info!("Arming digitizer (DIG1, mode={})", startmode);
            handle.send_command(devtree::cmd::ARM_ACQUISITION)?;
        }
    } else {
        info!("Arming digitizer (PSD2)");
        handle.send_command(devtree::cmd::ARM_ACQUISITION)?;
    }
    Ok(())
}

/// Send firmware-specific start command to the digitizer.
///
/// For DIG2 (PSD2), sends swstartacquisition.
/// For DIG1 (PSD1/PHA) with START_MODE_SW, sends armacquisition (arm=start).
///
/// AMax acquisition gate history: the 11june2026 FW gated acquisition behind
/// a global `START_DAQ` register (word `0x8000`) that had to be written 1 to
/// emit events. The **16June2026 FW dropped `START_DAQ` entirely** (register
/// removed from RegisterFile.json) — the only run-control registers left are
/// the per-channel `RUN_CFG` (programmed at Configure) and `ENABLE_ACQ`, so
/// `swstartacquisition` alone now drives the run. The explicit START_DAQ
/// write was therefore removed (would hit a non-existent register → CAEN
/// error). Pending live re-verification on gant with the 16June FW.
pub(crate) fn send_start_command(
    handle: &CaenHandle,
    firmware: FirmwareType,
) -> Result<(), caen::CaenError> {
    if firmware.is_dig1() {
        let startmode = handle
            .get_value(devtree::par::START_MODE)
            .unwrap_or_default();
        if startmode == "START_MODE_SW" {
            info!("Starting acquisition (DIG1, START_MODE_SW)");
            handle.send_command(devtree::cmd::ARM_ACQUISITION)?;
        } else {
            info!("DIG1 acquisition already started on Arm");
        }
    } else {
        info!("Starting digitizer acquisition (PSD2)");
        handle.send_command(devtree::cmd::SW_START_ACQUISITION)?;
    }
    Ok(())
}

/// Reader for CAEN digitizer data acquisition
///
/// Uses two-task architecture:
/// - ReadLoop: Blocking reads from CAEN hardware (spawn_blocking)
/// - DecodeLoop: Async decoding and ZMQ publishing
pub struct Reader {
    config: ReaderConfig,
    data_socket: publish::Publish,
    shared_state: Arc<Mutex<ComponentSharedState>>,
    state_rx: watch::Receiver<ComponentState>,
    state_tx: watch::Sender<ComponentState>,
    metrics: Arc<ReaderMetrics>,
    rate_tracker: Arc<RateTracker>,
}

impl Reader {
    /// Create a new Reader with the given configuration
    pub async fn new(config: ReaderConfig) -> Result<Self, ReaderError> {
        let context = Context::new();
        let data_socket = publish(&context).bind(&config.data_address)?;
        // Never drop messages — buffer in memory instead (DAQ: no data loss)
        pub_no_hwm(&data_socket).map_err(|e| ReaderError::Zmq(e.into()))?;

        info!(
            data_address = %config.data_address,
            command_address = %config.command_address,
            url = %config.url,
            "Reader bound to data address (SNDHWM=0)"
        );

        let (state_tx, state_rx) = watch::channel(ComponentState::Idle);

        Ok(Self {
            config,
            data_socket,
            shared_state: Arc::new(Mutex::new(ComponentSharedState::new())),
            state_rx,
            state_tx,
            metrics: Arc::new(ReaderMetrics::default()),
            rate_tracker: Arc::new(RateTracker::new()),
        })
    }

    /// Get current state
    pub fn state(&self) -> ComponentState {
        *self.state_rx.borrow()
    }

    /// Get metrics
    pub fn metrics(&self) -> &Arc<ReaderMetrics> {
        &self.metrics
    }

    /// Publish a message via ZMQ
    async fn publish_message(&mut self, message: &Message) -> Result<(), ReaderError> {
        let bytes = message.to_msgpack()?;
        let msg: tmq::Multipart = vec![tmq::Message::from(bytes.as_slice())].into();
        self.data_socket.send(msg).await?;

        match message {
            Message::Data(batch) => {
                debug!(
                    seq = batch.sequence_number,
                    events = batch.len(),
                    "Published batch"
                );
                self.metrics
                    .batches_published
                    .fetch_add(1, Ordering::Relaxed);
            }
            Message::EndOfStream { source_id, .. } => {
                info!(source_id = source_id, "Published EOS");
            }
            Message::Heartbeat(hb) => {
                debug!(
                    source_id = hb.source_id,
                    counter = hb.counter,
                    "Published heartbeat"
                );
            }
        }

        Ok(())
    }
}

/// Remap DIG1 (PSD1/PHA1) raw hardware flags to common flag constants.
///
/// Raw decoder flags come from EXTRAS word bits[15:10] shifted to bits[5:0],
/// plus pileup at bit[15] from the charge/energy word.
fn remap_dig1_flags(raw: u32) -> u64 {
    use crate::common::flags::*;
    let mut out: u64 = 0;
    if raw & (1 << 15) != 0 {
        out |= FLAG_PILEUP;
    } // Pileup from charge word
    if raw & (1 << 5) != 0 {
        out |= FLAG_TRIGGER_LOST;
    } // EXTRAS bit[15]
    if raw & (1 << 4) != 0 {
        out |= FLAG_OVER_RANGE;
    } // EXTRAS bit[14]
    if raw & (1 << 3) != 0 {
        out |= FLAG_1024_TRIGGER;
    } // EXTRAS bit[13]
    if raw & (1 << 2) != 0 {
        out |= FLAG_N_LOST_TRIGGER;
    } // EXTRAS bit[12]
    out
}

/// Convert EventData to CommonEventData (consumes event, zero-copy for
/// waveforms). Free function so decode worker threads can call it without a
/// `Reader` instance.
pub(crate) fn convert_event_to_common(event: EventData, firmware: FirmwareType) -> CommonEventData {
    let flags = if firmware.is_dig1() {
        remap_dig1_flags(event.flags)
    } else {
        event.flags as u64
    };

    let mut common = if let Some(wf) = event.waveform {
        CommonEventData::with_waveform(
            event.module,
            event.channel,
            event.energy,
            event.energy_short,
            event.timestamp_ns,
            flags,
            CommonWaveform {
                analog_probe1: wf.analog_probe1,   // move, not clone
                analog_probe2: wf.analog_probe2,   // move
                analog_probe3: wf.analog_probe3,   // move (AMax debug FW)
                digital_probe1: wf.digital_probe1, // move
                digital_probe2: wf.digital_probe2, // move
                digital_probe3: wf.digital_probe3, // move
                digital_probe4: wf.digital_probe4, // move
                digital_probe5: wf.digital_probe5, // move (AMax debug FW)
                digital_probe6: wf.digital_probe6,
                digital_probe7: wf.digital_probe7,
                digital_probe8: wf.digital_probe8,
                digital_probe9: wf.digital_probe9,
                digital_probe10: wf.digital_probe10,
                digital_probe11: wf.digital_probe11,
                digital_probe12: wf.digital_probe12,
                digital_probe13: wf.digital_probe13,
                digital_probe14: wf.digital_probe14,
                digital_probe15: wf.digital_probe15,
                digital_probe16: wf.digital_probe16,
                time_resolution: wf.time_resolution,
                trigger_threshold: wf.trigger_threshold,
                ns_per_sample: wf.ns_per_sample,
                analog_probe1_is_signed: wf.analog_probe1_is_signed,
                analog_probe2_is_signed: wf.analog_probe2_is_signed,
                analog_probe3_is_signed: wf.analog_probe3_is_signed,
                analog_probe_type: wf.analog_probe_type,
                digital_probe_type: wf.digital_probe_type,
            },
        )
    } else {
        CommonEventData::new(
            event.module,
            event.channel,
            event.energy,
            event.energy_short,
            event.timestamp_ns,
            flags,
        )
    };
    // Carry AMax user_info through to the wire format. Non-AMax firmwares
    // leave [0;4] from the decoder, so this is effectively a no-op there.
    common.user_info = event.user_info;
    common
}

impl Reader {
    /// Send EOS (End Of Stream) signal
    async fn send_eos(&mut self) -> Result<(), ReaderError> {
        let eos = Message::eos(
            self.config.source_id,
            self.metrics.current_run.load(Ordering::Relaxed),
        );
        self.publish_message(&eos).await
    }

    /// ReadLoop for V1743 digitizers (CAENDigitizer Library).
    ///
    /// Unlike the FELib read loops, this performs ReadData → DecodeEvent → EventData
    /// conversion entirely within the read loop because CAENDigitizer's DecodeEvent
    /// requires the handle (not thread-safe). x743 is low-rate (~7 kHz) so this is fine.
    #[cfg(feature = "x743")]
    #[allow(clippy::too_many_arguments)]
    fn read_loop_x743_std(
        config: ReaderConfig,
        tx: mpsc::Sender<ReadLoopOutput>,
        state_rx: watch::Receiver<ComponentState>,
        _state_tx: watch::Sender<ComponentState>,
        metrics: Arc<ReaderMetrics>,
        shutdown: Arc<std::sync::atomic::AtomicBool>,
        request_rx: std::sync::mpsc::Receiver<ReadLoopRequest>,
        hw_state: Arc<std::sync::Mutex<ComponentState>>,
    ) -> Result<(), ReaderError> {
        use crate::reader::caen_legacy::*;

        info!(source_id = config.source_id, "ReadLoop (x743) starting");

        // Load digitizer config to get X743Config (connection params).
        // Mutable: the ApplyConfig/ApplyConfigRunning handlers below refresh this
        // to the last-applied config so the state-machine Configure path re-applies
        // the current config instead of reverting to these startup values.
        let mut dig_config = config.config_file.as_ref().and_then(|path| {
            crate::config::digitizer::DigitizerConfig::load(path)
                .map_err(|e| warn!("Failed to load digitizer config: {}", e))
                .ok()
        });
        let mut decode_params = X743DecodeParams::from_config(dig_config.as_ref());
        info!(
            "V1743 decode params: save_waveform={} amp_scale={} amp_offset={} baseline_samples={} cfd_delay={} cfd_frac={} ns_per_sample={}",
            decode_params.save_waveform,
            decode_params.energy_scale,
            decode_params.energy_offset,
            decode_params.baseline_samples,
            decode_params.cfd_delay_samples,
            decode_params.cfd_fraction,
            decode_params.ns_per_sample,
        );

        // Raw-TDC diagnostic observer (TODO 62). The V1743 decoder uses the raw
        // 40-bit TDC directly (no rollover extension), which self-heals momentary
        // glitches; this only *observes* the stream to keep that visible. Re-armed
        // on every SWStartAcquisition.
        let mut tdc_diag = X743TdcDiag::new(caen_legacy::MAX_GROUPS);

        // Reusable per-event scratch buffers (raw + smoothed waveform).
        // Eliminates one Vec<f32> alloc per channel per event.
        let mut x743_scratch = X743Scratch::new();

        // Connection state
        let mut handle: Option<X743Handle> = None;
        let mut readout_buf: Option<ReadoutBuffer> = None;
        let mut event_buf: Option<EventBuffer> = None;
        let mut hw_configured = false;
        let mut hw_armed = false;
        let mut hw_running = false;
        let mut reconnect_cooldown = RECONNECT_INITIAL;
        let mut last_connect_attempt: Option<Instant> = None;

        // Try to connect
        let try_connect =
            |dig_config: &Option<crate::config::digitizer::DigitizerConfig>| -> Option<X743Handle> {
                let x743_cfg = dig_config.as_ref()?.x743.as_ref()?;
                let link_type = match x743_cfg.link_type.as_str() {
                    "usb" => ConnectionType::USB,
                    _ => ConnectionType::OpticalLink,
                };
                match X743Handle::open(
                    link_type,
                    x743_cfg.link_num,
                    x743_cfg.conet_node,
                    x743_cfg.vme_base_address,
                ) {
                    Ok(h) => Some(h),
                    Err(e) => {
                        warn!("Failed to open V1743: {}", e);
                        None
                    }
                }
            };

        loop {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }

            // Lazy connection with backoff
            if handle.is_none() {
                let should_try = match last_connect_attempt {
                    None => true,
                    Some(last) => last.elapsed() >= reconnect_cooldown,
                };
                if should_try {
                    last_connect_attempt = Some(Instant::now());
                    if let Some(h) = try_connect(&dig_config) {
                        // Buffers are NOT allocated here — `CAEN_DGTZ_MallocReadoutBuffer`
                        // sizes the buffer based on the digitizer's *current* state, so it
                        // must be called AFTER `apply_config_standard` (which sets
                        // record_length and max_num_events_blt). Pre-config alloc returns
                        // a ~35 KB buffer (default state), but the configured DAQ needs
                        // ~35 MB; the size mismatch causes DMA from the CAEN background
                        // thread to overflow user memory and SIGSEGV after some cycles.
                        // See plan: ~/.claude/plans/gemini-cli-peppy-turtle.md (T7).
                        info!("V1743 connected (buffers will be allocated after configure)");
                        handle = Some(h);
                        reconnect_cooldown = RECONNECT_INITIAL;
                    } else {
                        let (cooldown, next_base) = next_reconnect_cooldown(reconnect_cooldown);
                        reconnect_cooldown = next_base;
                        debug!("Next reconnect attempt in {}ms", cooldown.as_millis());
                    }
                }
                if handle.is_none() {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
            }

            let h = handle
                .as_ref()
                .expect("handle.is_none() branch above continues the loop");
            let target_state = *state_rx.borrow();
            let target_rank = state_rank(target_state);

            // === State synchronization ===

            // Configure
            if target_rank >= state_rank(ComponentState::Configured) && !hw_configured {
                if let Some(ref dc) = dig_config {
                    match h.apply_config_standard(dc) {
                        Ok(n) => {
                            info!("V1743 configured successfully ({} parameters)", n);
                            // Re-allocate readout buffer + event buffer NOW that the
                            // digitizer is configured. The buffer size CAEN returns is
                            // sized to the active record_length / max_num_events_blt,
                            // so we must allocate *after* apply_config_standard. Drop
                            // any prior buffers first (Rust drops the old `Some` value
                            // before the assignment writes the new one — this is safe).
                            // See plan: ~/.claude/plans/gemini-cli-peppy-turtle.md (T7).
                            readout_buf = None;
                            event_buf = None;
                            match (h.malloc_readout_buffer(), h.allocate_event()) {
                                (Ok(rb), Ok(eb)) => {
                                    info!(
                                        "V1743 buffers allocated post-configure (size={} bytes)",
                                        rb.allocated_size()
                                    );
                                    readout_buf = Some(rb);
                                    event_buf = Some(eb);
                                }
                                (Err(e), _) | (_, Err(e)) => {
                                    error!("V1743 post-configure buffer allocation failed: {}", e);
                                    // Drop handle to reconnect fresh
                                    handle = None;
                                    hw_configured = false;
                                    hw_armed = false;
                                    hw_running = false;
                                    continue;
                                }
                            }
                            hw_configured = true;
                            *hw_state.lock().unwrap() = ComponentState::Configured;
                        }
                        Err(e) => {
                            error!("V1743 configure failed: {}", e);
                            // Drop handle to reconnect fresh
                            handle = None;
                            readout_buf = None;
                            event_buf = None;
                            hw_configured = false;
                            hw_armed = false;
                            hw_running = false;
                            continue;
                        }
                    }
                } else {
                    warn!("No digitizer config loaded — cannot configure V1743");
                }
            }

            // Arm (V1743: nothing to do — acquisition starts with SWStartAcquisition)
            if target_rank >= state_rank(ComponentState::Armed) && hw_configured && !hw_armed {
                info!("V1743 armed (ready for start)");
                hw_armed = true;
                *hw_state.lock().unwrap() = ComponentState::Armed;
            }

            // Start
            if target_rank >= state_rank(ComponentState::Running) && hw_armed && !hw_running {
                // UM1935 p.21: explicit ClearData is unnecessary because
                // CAEN_DGTZ_SWStartAcquisition runs an automatic clear cycle.
                // WaveDemo x743 v1.2.1 also calls SWStartAcquisition directly
                // without a preceding ClearData, and the manual cautions that
                // an explicit ClearData immediately before Start can occasionally
                // cause an internal state-machine race that drops the first
                // trigger of the run.
                match h.sw_start_acquisition() {
                    Ok(()) => {
                        info!("V1743 acquisition started");
                        // Re-arm the raw-TDC diagnostic for this run. No rollover
                        // state to reset — the decoder uses raw TDC directly (TODO 62).
                        tdc_diag.rearm();
                        hw_running = true;
                        *hw_state.lock().unwrap() = ComponentState::Running;
                    }
                    Err(e) => {
                        error!("V1743 start failed: {}", e);
                    }
                }
            }

            // Stop
            if target_rank < state_rank(ComponentState::Running) && hw_running {
                info!("V1743 stopping acquisition");
                if let Err(e) = h.sw_stop_acquisition() {
                    warn!("V1743 stop acquisition error: {}", e);
                }

                // Drain remaining events from board buffer
                if let (Some(ref mut rb), Some(ref mut eb)) = (&mut readout_buf, &mut event_buf) {
                    let mut drained = 0u32;
                    loop {
                        match h.read_data(rb) {
                            Ok(0) => break,
                            Ok(data_size) => {
                                if let Ok(n) = h.get_num_events(rb, data_size) {
                                    for i in 0..n {
                                        if let Ok((info, ptr)) = h.get_event_info(rb, data_size, i)
                                        {
                                            if h.decode_event(ptr, eb).is_ok() {
                                                let events = Self::x743_std_event_to_event_data(
                                                    eb.event(),
                                                    &info,
                                                    config.module_id,
                                                    &decode_params,
                                                    &mut x743_scratch,
                                                    &mut tdc_diag,
                                                );
                                                for event in events {
                                                    let _ = tx.blocking_send(
                                                        ReadLoopOutput::Decoded(Box::new(event)),
                                                    );
                                                }
                                                drained += 1;
                                            }
                                        }
                                    }
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    if drained > 0 {
                        info!("Drained {} events after stop", drained);
                    }
                }

                // Send Stop signal → decode_loop publishes EOS
                let _ = tx.blocking_send(ReadLoopOutput::Stop);

                hw_running = false;
                hw_armed = false; // V1743: must re-arm after stop
                *hw_state.lock().unwrap() = ComponentState::Configured;
            }

            // Reset to Idle
            if target_state == ComponentState::Idle && (hw_armed || hw_configured) {
                info!("V1743 resetting to Idle");
                if let Err(e) = h.reset() {
                    warn!("V1743 reset error: {}", e);
                }
                hw_configured = false;
                hw_armed = false;
                hw_running = false;
                *hw_state.lock().unwrap() = ComponentState::Idle;
            }

            // === Handle requests (Detect, ApplyConfig) ===
            if let Ok(req) = request_rx.try_recv() {
                match req {
                    ReadLoopRequest::Detect { response_tx } => {
                        let result = h.get_device_info_json().map_err(|e| e.to_string());
                        let _ = response_tx.send(result);
                    }
                    ReadLoopRequest::ApplyConfig {
                        config: new_config,
                        response_tx,
                    } => {
                        let result = h
                            .apply_config_standard(&new_config)
                            .map_err(|e| e.to_string());
                        if result.is_ok() {
                            decode_params = X743DecodeParams::from_config(Some(&new_config));
                            // Refresh the cached config so the state-machine Configure
                            // path (which re-applies `dig_config`) uses the latest values
                            // instead of reverting to the startup config on the next
                            // Reset -> Configure cycle.
                            dig_config = Some((*new_config).clone());
                            hw_configured = true;
                            // Re-allocate buffers — config change may shift record_length
                            // / max_num_events_blt; buffer must match. See plan T7.
                            readout_buf = None;
                            event_buf = None;
                            match (h.malloc_readout_buffer(), h.allocate_event()) {
                                (Ok(rb), Ok(eb)) => {
                                    info!("V1743 buffers re-allocated post-ApplyConfig (size={} bytes)", rb.allocated_size());
                                    readout_buf = Some(rb);
                                    event_buf = Some(eb);
                                }
                                (Err(e), _) | (_, Err(e)) => {
                                    error!("V1743 post-ApplyConfig buffer realloc failed: {}", e);
                                }
                            }
                        }
                        let _ = response_tx.send(result);
                    }
                    ReadLoopRequest::ReadAmaxBoardRegisters { response_tx } => {
                        // V1743 (X743 family) doesn't expose AMax board
                        // registers — return empty so the UI can render
                        // "no AMax registers" for X743 digitizers.
                        let _ = response_tx.send(Ok(Vec::new()));
                    }
                    ReadLoopRequest::ApplyConfigRunning {
                        config: new_config,
                        response_tx,
                    } => {
                        // V1743 doesn't support parameter changes while running
                        // but we can try re-applying if needed
                        if let Some(x743) = new_config.x743.as_ref() {
                            if !x743.extra_registers.is_empty() {
                                warn!(
                                    "ApplyConfigRunning: extra_registers ({} entries) will be applied while acquisition is running — board state may be disrupted",
                                    x743.extra_registers.len()
                                );
                            }
                        }
                        let result = h
                            .apply_config_standard(&new_config)
                            .map_err(|e| e.to_string());
                        if result.is_ok() {
                            decode_params = X743DecodeParams::from_config(Some(&new_config));
                            // Keep the cached config in sync (see ApplyConfig above).
                            dig_config = Some((*new_config).clone());
                            // Re-allocate buffers (best-effort; running mode is rare)
                            readout_buf = None;
                            event_buf = None;
                            match (h.malloc_readout_buffer(), h.allocate_event()) {
                                (Ok(rb), Ok(eb)) => {
                                    readout_buf = Some(rb);
                                    event_buf = Some(eb);
                                }
                                (Err(e), _) | (_, Err(e)) => {
                                    error!(
                                        "V1743 post-ApplyConfigRunning buffer realloc failed: {}",
                                        e
                                    );
                                }
                            }
                        }
                        let _ = response_tx.send(result);
                    }
                }
            }

            // === Data readout (only while Running) ===
            if !hw_running {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }

            if let (Some(ref mut rb), Some(ref mut eb)) = (&mut readout_buf, &mut event_buf) {
                match h.read_data(rb) {
                    Ok(0) => {
                        // No data available, continue polling
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Ok(data_size) => {
                        let num_events = match h.get_num_events(rb, data_size) {
                            Ok(n) => n,
                            Err(e) => {
                                warn!("GetNumEvents error: {}", e);
                                continue;
                            }
                        };

                        metrics
                            .bytes_read
                            .fetch_add(data_size as u64, Ordering::Relaxed);

                        for evt_idx in 0..num_events {
                            let (info, ptr) = match h.get_event_info(rb, data_size, evt_idx) {
                                Ok(v) => v,
                                Err(e) => {
                                    warn!("GetEventInfo error: {}", e);
                                    continue;
                                }
                            };

                            if let Err(e) = h.decode_event(ptr, eb) {
                                warn!("DecodeEvent error: {}", e);
                                continue;
                            }

                            // One-shot raw-field log so we can confirm what the CAEN lib fills in
                            // Standard mode. `debug!` level to avoid spamming production logs.
                            {
                                static DEBUGGED: std::sync::atomic::AtomicBool =
                                    std::sync::atomic::AtomicBool::new(false);
                                if !DEBUGGED.swap(true, Ordering::Relaxed) {
                                    let ev = eb.event();
                                    for g in 0..caen_legacy::MAX_GROUPS {
                                        if ev.GrPresent[g] == 0 {
                                            continue;
                                        }
                                        let grp = &ev.DataGroup[g];
                                        let ch0_null = grp.DataChannel[0].is_null();
                                        let ch1_null = grp.DataChannel[1].is_null();
                                        info!(
                                            "V1743 first decoded event (Standard mode) group={} ChSize={} TDC={} Charge={} Peak={} Baseline={} PosEdge={} NegEdge={} PeakIdx={} TrgCnt=[{},{}] TimeCnt=[{},{}] StartCell={} ch0_null={} ch1_null={}",
                                            g,
                                            grp.ChSize,
                                            grp.TDC,
                                            grp.Charge,
                                            grp.Peak,
                                            grp.Baseline,
                                            grp.PosEdgeTimeStamp,
                                            grp.NegEdgeTimeStamp,
                                            grp.PeakIndex,
                                            grp.TriggerCount[0],
                                            grp.TriggerCount[1],
                                            grp.TimeCount[0],
                                            grp.TimeCount[1],
                                            grp.StartIndexCell,
                                            ch0_null,
                                            ch1_null,
                                        );
                                    }
                                }
                            }

                            let events = Self::x743_std_event_to_event_data(
                                eb.event(),
                                &info,
                                config.module_id,
                                &decode_params,
                                &mut x743_scratch,
                                &mut tdc_diag,
                            );

                            for event in events {
                                metrics.queue_length.fetch_add(1, Ordering::Relaxed);
                                if tx
                                    .blocking_send(ReadLoopOutput::Decoded(Box::new(event)))
                                    .is_err()
                                {
                                    warn!("ReadLoop channel closed");
                                    return Ok(());
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // CAENDigitizer ReadData error — check if it's timeout-like
                        debug!("ReadData error: {} (may be timeout)", e);
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            }
        }

        // Cleanup
        if let Some(ref h) = handle {
            if hw_running {
                let _ = h.sw_stop_acquisition();
            }
        }
        // Dropping handle, readout_buf, event_buf triggers RAII cleanup
        info!("ReadLoop (x743) stopped");
        Ok(())
    }

    /// Convert a `CAEN_DGTZ_X743_EVENT_t` into a Vec of `decoder::EventData`.
    ///
    /// Each present group produces up to 2 events (one per channel). The CAEN
    /// lib only fills `TDC` and `DataChannel[]` in Standard mode — baseline,
    /// amplitude and fine time are computed in Rust by `X743WaveformStats`.
    ///
    /// Time model:
    /// ```text
    ///   timestamp_ns = (raw 40-bit TDC) * 5  +  cfd_time_ns
    /// ```
    /// where `cfd_time_ns` is the sub-sample position of the CFD zero-crossing
    /// inside the waveform, in ns from sample 0. A constant offset (trigger
    /// position inside the window) drops out of cross-event Δt measurements.
    ///
    /// The TDC is used **raw** (masked to 40 bits), with no rollover extension
    /// (TODO 62). Runs are kept < 90 min so the counter (wrap ~91.6 min) never
    /// wraps; in exchange a momentary garbage/out-of-order TDC corrupts only its
    /// own event and self-heals on the next, rather than poisoning persistent
    /// per-group rollover state (the run30 failure mode).
    ///
    /// `flags` layout:
    /// - bits 0..16: `peak_index` (sample count)
    /// - bit 24: `cfd_valid`
    /// - bit 25: `waveform_decode_failed` (too few samples / null ptr)
    #[cfg(feature = "x743")]
    fn x743_std_event_to_event_data(
        event: &crate::reader::caen_legacy::ffi::CAEN_DGTZ_X743_EVENT_t,
        _info: &crate::reader::caen_legacy::ffi::CAEN_DGTZ_EventInfo_t,
        module_id: u8,
        params: &X743DecodeParams,
        scratch: &mut X743Scratch,
        tdc_diag: &mut X743TdcDiag,
    ) -> Vec<decoder::EventData> {
        const TDC_NS: f64 = 5.0;
        // 40-bit TDC mask. The counter is 40 bits @ 5 ns (wrap ~91.6 min);
        // masking defensively strips any garbage upper u64 bits the CAEN lib
        // might leave set.
        const TDC_MASK: u64 = 0xFF_FFFF_FFFF;
        const FLAG_CFD_VALID: u32 = 1 << 24;
        const FLAG_WF_DECODE_FAIL: u32 = 1 << 25;

        let mut events = Vec::new();

        for g in 0..caen_legacy::MAX_GROUPS {
            if event.GrPresent[g] == 0 {
                continue;
            }
            let group = &event.DataGroup[g];

            // Coarse time: raw 40-bit TDC @ 5 ns used DIRECTLY — no rollover
            // extension (TODO 62). Operational rule keeps runs < 90 min so the
            // counter never wraps. A momentary garbage/out-of-order TDC therefore
            // corrupts only its own event and self-heals on the next, instead of
            // poisoning persistent rollover state (the run30 failure mode).
            //
            // KNOWN: the V1743 emits ~6 garbage-TDC events in the first ~1 ms of a
            // run (CAEN reads a few uninitialised DMA slots — TDC shows byte-repeat
            // patterns like 0x1B1B1B1B). They are accepted (self-healed) and cut
            // offline; see docs/operations_manual.md §9. Count via "V1743 TDC diag
            // backward=true" in the reader log.
            let tdc_ticks = group.TDC & TDC_MASK;
            let tdc_ns = tdc_ticks as f64 * TDC_NS;

            // Observe the raw stream for glitch visibility only (never affects
            // the timestamp above).
            tdc_diag.observe(g, tdc_ticks);

            for ch_in_group in 0..caen_legacy::CHANNELS_PER_GROUP {
                let channel = (g * caen_legacy::CHANNELS_PER_GROUP + ch_in_group) as u8;
                let negative = params
                    .channel_negative
                    .get(channel as usize)
                    .copied()
                    .unwrap_or(true);

                Self::x743_waveform_samples_into(group, ch_in_group, &mut scratch.raw);
                // TTF smoothing (WaveDemo-compatible) is applied BEFORE baseline /
                // CFD computation so noisy pulses get a stable timing fix. taps≤1
                // returns the raw slice — zero-copy fast path.
                let analyze_input = scratch.smoothed_view(params.ttf_smoothing_taps);

                let stats = X743WaveformStats::analyze(
                    analyze_input,
                    params.ns_per_sample,
                    negative,
                    params.baseline_samples,
                    params.cfd_delay_samples,
                    params.cfd_fraction,
                );

                // `energy_metric` feeds `energy` (Charge Long); `short_metric`
                // feeds `energy_short` (Charge Short). In "soft" mode Long is the
                // CFD-anchored gated charge integral and Short is the pulse
                // amplitude — the pair the user needs for integral-vs-height
                // discrimination. In "amplitude" mode Long is the amplitude and
                // Short is unused (0), preserving the previous behaviour.
                let (timestamp_ns, energy_metric, short_metric, peak_index, cfd_valid, decode_ok) =
                    if let Some(s) = stats {
                        let (long, short) = if params.use_soft_charge {
                            // Anchor the gate at the CFD crossing sample; fall back
                            // to the peak sample when the CFD search failed.
                            let anchor = if s.cfd_valid {
                                (s.cfd_time_ns / params.ns_per_sample).max(0.0) as usize
                            } else {
                                s.peak_index as usize
                            };
                            let charge = x743_long_charge(
                                analyze_input,
                                s.baseline,
                                anchor,
                                params.charge_gate_pre_samples,
                                params.charge_gate_samples,
                                negative,
                            );
                            (charge, s.amplitude)
                        } else {
                            (s.amplitude, 0.0)
                        };
                        (
                            tdc_ns + s.cfd_time_ns,
                            long,
                            short,
                            s.peak_index,
                            s.cfd_valid,
                            true,
                        )
                    } else {
                        (tdc_ns, 0.0, 0.0, 0, false, false)
                    };

                // Charge Long: metric (amplitude or charge) → scale+offset → u16.
                let energy_f = energy_metric * params.energy_scale + params.energy_offset;
                let energy = if energy_f.is_finite() {
                    energy_f.clamp(0.0, u16::MAX as f32) as u16
                } else {
                    0
                };
                // Charge Short: raw pulse amplitude (ADC units, unscaled) in "soft"
                // mode, else 0. Kept unscaled so it stays a direct pulse height
                // independent of the Long charge's energy_scale.
                let energy_short = if short_metric.is_finite() {
                    short_metric.clamp(0.0, u16::MAX as f32) as u16
                } else {
                    0
                };

                // fine_time = fractional part of cfd_time_ns within a TDC tick,
                // encoded as 10-bit (0..=1023) per the other decoders' convention.
                let cfd_only_ns = timestamp_ns - tdc_ns;
                let frac = (cfd_only_ns / TDC_NS).rem_euclid(1.0);
                let fine_time = (frac * 1024.0).clamp(0.0, 1023.0) as u16;

                let mut flags = (peak_index as u32) & 0xFFFF;
                if cfd_valid {
                    flags |= FLAG_CFD_VALID;
                }
                if !decode_ok {
                    flags |= FLAG_WF_DECODE_FAIL;
                }

                // DEBUG (one-shot): confirm waveform emission path is taken.
                {
                    static WF_DEBUGGED: std::sync::atomic::AtomicBool =
                        std::sync::atomic::AtomicBool::new(false);
                    if !WF_DEBUGGED.swap(true, Ordering::Relaxed) {
                        tracing::info!(
                            "V1743 waveform emission: save_waveform={}, raw.len()={}",
                            params.save_waveform,
                            scratch.raw.len(),
                        );
                    }
                }
                // Save the *raw* waveform (pre-smoothing) so downstream analysis
                // sees what the digitizer actually produced; smoothing is purely
                // a software-side timing aid for our CFD.
                let waveform = if params.save_waveform && !scratch.raw.is_empty() {
                    let samples_i16: Vec<i16> = scratch
                        .raw
                        .iter()
                        .map(|&s| {
                            if s.is_finite() {
                                s.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
                            } else {
                                0
                            }
                        })
                        .collect();
                    Some(decoder::Waveform {
                        analog_probe1: samples_i16,
                        analog_probe2: Vec::new(),
                        analog_probe3: Vec::new(),
                        digital_probe1: Vec::new(),
                        digital_probe2: Vec::new(),
                        digital_probe3: Vec::new(),
                        digital_probe4: Vec::new(),
                        digital_probe5: Vec::new(),
                        digital_probe6: Vec::new(),
                        digital_probe7: Vec::new(),
                        digital_probe8: Vec::new(),
                        digital_probe9: Vec::new(),
                        digital_probe10: Vec::new(),
                        digital_probe11: Vec::new(),
                        digital_probe12: Vec::new(),
                        digital_probe13: Vec::new(),
                        digital_probe14: Vec::new(),
                        digital_probe15: Vec::new(),
                        digital_probe16: Vec::new(),
                        time_resolution: 0,
                        trigger_threshold: 0,
                        ns_per_sample: params.ns_per_sample,
                        // V1743 Standard mode samples are unsigned 12-bit
                        // ADC values masked into 14-bit range; no probe-
                        // type info on the wire.
                        analog_probe1_is_signed: false,
                        analog_probe2_is_signed: false,
                        analog_probe3_is_signed: false,
                        analog_probe_type: [decoder::common::UNKNOWN_PROBE_TYPE; 3],
                        digital_probe_type: [decoder::common::UNKNOWN_PROBE_TYPE; 16],
                    })
                } else {
                    None
                };

                events.push(decoder::EventData {
                    timestamp_ns,
                    module: module_id,
                    channel,
                    energy,
                    energy_short,
                    fine_time,
                    flags,
                    user_info: [0; 4],
                    waveform,
                });
            }
        }

        events
    }

    /// Copy the raw float waveform out of the CAEN-lib-owned buffer into the
    /// caller-provided scratch buffer. Avoids a per-event `Vec<f32>` alloc.
    /// `dst` is cleared first, then filled. Empty result if `ChSize == 0` or
    /// the `DataChannel[ch]` pointer is null.
    #[cfg(feature = "x743")]
    fn x743_waveform_samples_into(
        group: &crate::reader::caen_legacy::ffi::CAEN_DGTZ_X743_GROUP_t,
        ch_in_group: usize,
        dst: &mut Vec<f32>,
    ) {
        dst.clear();
        let ch_size = group.ChSize as usize;
        if ch_size == 0 {
            return;
        }
        let ptr = group.DataChannel[ch_in_group];
        if ptr.is_null() {
            return;
        }
        // Safety: CAEN lib guarantees `DataChannel[ch_in_group]` points to `ChSize` floats
        // for the duration of this event decode; we copy them out immediately.
        let raw = unsafe { std::slice::from_raw_parts(ptr, ch_size) };
        dst.reserve(raw.len());
        dst.extend_from_slice(raw);
    }

    // V1743 DPP-CI (Charge Mode) support was removed on 2026-04-20.
    // See TODO/47_v1743_standard_mode_redesign.md — UM2750 Rev.5 Fig 10.9 has no TDC field
    // in Charge Mode, so physical timestamps cannot be recovered. Standard mode is
    // now the only supported V1743 path.

    /// DecodeLoop task — collector side of the parallel decode pipeline.
    ///
    /// Decoding/serialization runs on dedicated worker threads (see
    /// `parallel_decode`); this task reorders their output back into
    /// dispatch order and owns the ZMQ PUB socket. Start/Stop markers flow
    /// through the same ordered stream, so EOS is published only after every
    /// preceding batch (Stop→EOS without backlog loss).
    async fn decode_loop(
        config: ReaderConfig,
        rx: mpsc::Receiver<ReadLoopOutput>,
        mut data_socket: publish::Publish,
        metrics: Arc<ReaderMetrics>,
        state_rx: watch::Receiver<ComponentState>,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), ReaderError> {
        info!("DecodeLoop starting");

        if config.adc_min > 0 {
            info!(
                adc_min = config.adc_min,
                "ADC minimum filter enabled: events with energy < {} will be discarded",
                config.adc_min
            );
        }

        let mut collector_rx = parallel_decode::spawn_pipeline(&config, rx, Arc::clone(&metrics));
        let mut reorder = parallel_decode::ReorderBuffer::new();
        let mut trigger_warner = parallel_decode::TriggerLossWarner::new();

        let mut heartbeat_counter: u64 = 0;
        let mut total_batches: u64 = 0;

        // Heartbeat ticker
        let use_heartbeat = config.heartbeat_interval_ms > 0;
        let mut heartbeat_ticker =
            interval(Duration::from_millis(config.heartbeat_interval_ms.max(100)));

        loop {
            tokio::select! {
                biased;

                _ = shutdown.recv() => {
                    info!("DecodeLoop received shutdown signal");
                    break;
                }

                // Heartbeat (only when Running)
                _ = heartbeat_ticker.tick(), if use_heartbeat && *state_rx.borrow() == ComponentState::Running => {
                    let hb = Message::heartbeat(config.source_id, heartbeat_counter);
                    heartbeat_counter += 1;
                    match hb.to_msgpack() {
                        Ok(bytes) => {
                            let msg: tmq::Multipart = vec![tmq::Message::from(bytes.as_slice())].into();
                            if let Err(e) = data_socket.send(msg).await {
                                warn!(error = %e, "Failed to send heartbeat");
                            } else {
                                debug!(counter = heartbeat_counter, "Published heartbeat");
                            }
                        }
                        Err(e) => warn!(error = %e, "Failed to serialize heartbeat"),
                    }
                }

                item = collector_rx.recv() => {
                    match item {
                        Some(item) => {
                            for ready in reorder.push(item) {
                                total_batches += parallel_decode::publish_ready(
                                    ready,
                                    &config,
                                    &mut data_socket,
                                    &metrics,
                                    &mut heartbeat_counter,
                                    &mut trigger_warner,
                                )
                                .await;
                            }
                        }
                        None => {
                            // Pipeline threads exited (data channel closed).
                            // Flush whatever is still ordered in the buffer.
                            for ready in reorder.drain_remaining() {
                                total_batches += parallel_decode::publish_ready(
                                    ready,
                                    &config,
                                    &mut data_socket,
                                    &metrics,
                                    &mut heartbeat_counter,
                                    &mut trigger_warner,
                                )
                                .await;
                            }
                            info!("Decode pipeline drained, stopping decode loop");
                            break;
                        }
                    }
                }
            }
        }

        info!(
            total_batches,
            total_events = metrics.events_decoded.load(Ordering::Relaxed),
            "DecodeLoop stopped"
        );
        Ok(())
    }

    /// Run the reader with command control
    ///
    /// Spawns three tasks:
    /// - Command task: handles control commands
    /// - ReadLoop task: reads from CAEN hardware (blocking)
    /// - DecodeLoop task: decodes and publishes data (async)
    pub async fn run(
        mut self,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), ReaderError> {
        info!(
            source_id = self.config.source_id,
            state = %self.state(),
            "Reader ready, waiting for commands"
        );

        // Create channels (using ReadLoopOutput to support both RAW and OpenDPP paths)
        let (data_tx, data_rx) = mpsc::channel::<ReadLoopOutput>(1000);

        // Shutdown flag for ReadLoop (it runs in spawn_blocking, can't use async channel)
        let read_shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let read_shutdown_clone = read_shutdown.clone();

        // Channel for delegating hardware requests (Detect/ApplyConfig) to the read_loop
        let (request_tx, request_rx) = std::sync::mpsc::channel::<ReadLoopRequest>();

        // Hardware-confirmed state: ReadLoop updates this after actual HW transitions.
        // GetStatus reports min(software_state, hw_state) so Operator waits until
        // hardware is truly ready before proceeding (e.g. Start after Arm).
        let hw_state = Arc::new(std::sync::Mutex::new(ComponentState::Idle));
        let hw_state_for_read = hw_state.clone();

        // Spawn command handler task using common infrastructure
        let command_address = self.config.command_address.clone();
        let shared_state = self.shared_state.clone();
        let state_tx = self.state_tx.clone();
        let shutdown_for_cmd = shutdown.resubscribe();
        let metrics_for_cmd = self.metrics.clone();
        let rate_tracker_for_cmd = self.rate_tracker.clone();

        let cmd_handle = tokio::spawn(async move {
            run_command_task(
                command_address,
                shared_state,
                state_tx,
                shutdown_for_cmd,
                move |state, tx, cmd| {
                    let mut ext = ReaderCommandExt {
                        metrics: metrics_for_cmd.clone(),
                        rate_tracker: rate_tracker_for_cmd.clone(),
                        request_tx: request_tx.clone(),
                        hw_state: hw_state.clone(),
                    };
                    handle_command(state, tx, cmd, Some(&mut ext))
                },
                "Reader",
            )
            .await;
        });

        // Spawn ReadLoop task (blocking)
        // Select read loop based on firmware type:
        // - AMax uses OpenDPP endpoint (pre-decoded events)
        // - Others use RAW endpoint (requires software decoding)
        let read_config = self.config.clone();
        let read_state_rx = self.state_rx.clone();
        let read_metrics = self.metrics.clone();
        let use_opendpp = self.config.firmware == FirmwareType::AMax;
        let use_x743 = self.config.firmware.is_legacy_api();

        let read_state_tx = self.state_tx.clone();
        let read_handle = tokio::task::spawn_blocking(move || {
            if use_x743 {
                #[cfg(feature = "x743")]
                {
                    info!("Using CAENDigitizer Library for V1743 Standard mode");
                    Self::read_loop_x743_std(
                        read_config,
                        data_tx,
                        read_state_rx,
                        read_state_tx,
                        read_metrics,
                        read_shutdown_clone,
                        request_rx,
                        hw_state_for_read,
                    )
                }
                #[cfg(not(feature = "x743"))]
                {
                    error!("x743 firmware selected but 'x743' feature not enabled");
                    Err(ReaderError::Config(
                        "x743 feature not enabled at compile time".to_string(),
                    ))
                }
            } else if use_opendpp {
                info!("Using OpenDPP endpoint for AMax firmware");
                read_loop_dig2::run(
                    read_config,
                    data_tx,
                    read_state_rx,
                    read_state_tx,
                    read_metrics,
                    read_shutdown_clone,
                    request_rx,
                    hw_state_for_read,
                )
            } else {
                info!("Using RAW endpoint for firmware {:?}", read_config.firmware);
                read_loop_dig1::run(
                    read_config,
                    data_tx,
                    read_state_rx,
                    read_state_tx,
                    read_metrics,
                    read_shutdown_clone,
                    request_rx,
                    hw_state_for_read,
                )
            }
        });

        // Take ownership of data_socket for decode loop
        let data_socket = std::mem::replace(
            &mut self.data_socket,
            // Dummy socket - will not be used after this; bind to port 0 cannot
            // fail in practice (kernel always assigns an ephemeral port).
            publish(&Context::new())
                .bind("tcp://127.0.0.1:0")
                .expect("ephemeral-port bind on 127.0.0.1 cannot fail"),
        );

        // Spawn DecodeLoop task
        let decode_config = self.config.clone();
        let decode_metrics = self.metrics.clone();
        let decode_state_rx = self.state_rx.clone();
        let shutdown_for_decode = shutdown.resubscribe();

        let decode_handle = tokio::spawn(async move {
            Self::decode_loop(
                decode_config,
                data_rx,
                data_socket,
                decode_metrics,
                decode_state_rx,
                shutdown_for_decode,
            )
            .await
        });

        // Wait for shutdown signal
        let _ = shutdown.recv().await;
        info!("Reader received shutdown signal");

        // Signal ReadLoop to stop
        read_shutdown.store(true, Ordering::Relaxed);

        // Wait for tasks to complete
        let _ = cmd_handle.await;
        match read_handle.await {
            Ok(Ok(())) => info!("ReadLoop completed normally"),
            Ok(Err(e)) => error!(error = %e, "ReadLoop exited with error"),
            Err(e) => error!(error = %e, "ReadLoop task panicked"),
        }
        match decode_handle.await {
            Ok(Ok(())) => info!("DecodeLoop completed normally"),
            Ok(Err(e)) => error!(error = %e, "DecodeLoop exited with error"),
            Err(e) => error!(error = %e, "DecodeLoop task panicked"),
        }

        // Send EOS if we were running
        if *self.state_rx.borrow() == ComponentState::Running {
            self.send_eos().await?;
        }

        info!(
            total_events = self.metrics.events_decoded.load(Ordering::Relaxed),
            total_bytes = self.metrics.bytes_read.load(Ordering::Relaxed),
            total_batches = self.metrics.batches_published.load(Ordering::Relaxed),
            "Reader stopped"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ReaderConfig::default();
        assert_eq!(config.source_id, 0);
        assert_eq!(config.firmware, FirmwareType::PSD2);
        assert_eq!(config.buffer_size, 64 * 1024 * 1024);
    }

    #[test]
    fn test_convert_event() {
        let event = EventData {
            timestamp_ns: 1234567.0,
            module: 1,
            channel: 5,
            energy: 1000,
            energy_short: 800,
            fine_time: 512,
            flags: 0x01,
            user_info: [0; 4],
            waveform: None,
        };

        let minimal = convert_event_to_common(event, FirmwareType::PSD2);
        // CommonEventData is packed, so we need to copy values before comparing
        let module = minimal.module;
        let channel = minimal.channel;
        let energy = { minimal.energy };
        let energy_short = { minimal.energy_short };
        let timestamp_ns = { minimal.timestamp_ns };
        let flags = { minimal.flags };

        assert_eq!(module, 1);
        assert_eq!(channel, 5);
        assert_eq!(energy, 1000);
        assert_eq!(energy_short, 800);
        assert_eq!(timestamp_ns, 1234567.0);
        assert_eq!(flags, 0x01);
        assert!(minimal.waveform.is_none());
    }

    #[test]
    fn test_from_config_psd2_maps_firmware() {
        let toml = r#"
            [[network.sources]]
            id = 0
            type = "psd2"
            bind = "tcp://*:5555"
            digitizer_url = "dig2://172.18.4.56"

            [network.merger]
            subscribe = ["tcp://localhost:5555"]
            publish = "tcp://*:5557"

            [network.recorder]
            subscribe = "tcp://localhost:5557"
        "#;
        let config = crate::config::Config::from_toml(toml).unwrap();
        let reader_config = ReaderConfig::from_config(&config, 0).unwrap();
        assert_eq!(reader_config.firmware, FirmwareType::PSD2);
    }

    #[test]
    fn test_from_config_psd1_maps_firmware() {
        let toml = r#"
            [[network.sources]]
            id = 0
            type = "psd1"
            bind = "tcp://*:5555"
            digitizer_url = "dig1://caen.internal/usb?link_num=0"

            [network.merger]
            subscribe = ["tcp://localhost:5555"]
            publish = "tcp://*:5557"

            [network.recorder]
            subscribe = "tcp://localhost:5557"
        "#;
        let config = crate::config::Config::from_toml(toml).unwrap();
        let reader_config = ReaderConfig::from_config(&config, 0).unwrap();
        assert_eq!(reader_config.firmware, FirmwareType::PSD1);
    }

    #[test]
    fn test_from_config_emulator_returns_none() {
        let toml = r#"
            [[network.sources]]
            id = 0
            type = "emulator"
            bind = "tcp://*:5555"
            digitizer_url = "dig2://172.18.4.56"

            [network.merger]
            subscribe = ["tcp://localhost:5555"]
            publish = "tcp://*:5557"

            [network.recorder]
            subscribe = "tcp://localhost:5557"
        "#;
        let config = crate::config::Config::from_toml(toml).unwrap();
        // Emulator sources should NOT create a ReaderConfig
        assert!(ReaderConfig::from_config(&config, 0).is_none());
    }

    #[test]
    fn test_convert_event_with_waveform() {
        let wf = Waveform {
            analog_probe1: vec![100, 200, -300],
            analog_probe2: vec![10, 20, -30],
            analog_probe3: vec![],
            digital_probe1: vec![1, 0, 1],
            digital_probe2: vec![0, 1, 0],
            digital_probe3: vec![1, 1, 0],
            digital_probe4: vec![0, 0, 1],
            digital_probe5: vec![],
            digital_probe6: vec![],
            digital_probe7: vec![],
            digital_probe8: vec![],
            digital_probe9: vec![],
            digital_probe10: vec![],
            digital_probe11: vec![],
            digital_probe12: vec![],
            digital_probe13: vec![],
            digital_probe14: vec![],
            digital_probe15: vec![],
            digital_probe16: vec![],
            time_resolution: 2,
            trigger_threshold: 500,
            ns_per_sample: 2.0,
            analog_probe1_is_signed: true,
            analog_probe2_is_signed: true,
            analog_probe3_is_signed: false,
            analog_probe_type: [decoder::common::UNKNOWN_PROBE_TYPE; 3],
            digital_probe_type: [decoder::common::UNKNOWN_PROBE_TYPE; 16],
        };

        let event = EventData {
            timestamp_ns: 999.0,
            module: 0,
            channel: 3,
            energy: 2000,
            energy_short: 1500,
            fine_time: 100,
            flags: 0x00,
            user_info: [0; 4],
            waveform: Some(wf),
        };

        let converted = convert_event_to_common(event, FirmwareType::PSD2);
        assert!(converted.waveform.is_some(), "Waveform should be preserved");
        let cwf = converted.waveform.unwrap();
        assert_eq!(cwf.analog_probe1, vec![100, 200, -300]);
        assert_eq!(cwf.analog_probe2, vec![10, 20, -30]);
        assert_eq!(cwf.digital_probe1, vec![1, 0, 1]);
        assert_eq!(cwf.time_resolution, 2);
        assert_eq!(cwf.trigger_threshold, 500);
        assert_eq!(cwf.ns_per_sample, 2.0);
    }

    fn make_opendpp_event(user_info: Vec<u64>) -> OpenDppEvent {
        OpenDppEvent {
            channel: 7,
            timestamp: 1_000,
            fine_timestamp: 256,
            energy: 4096,
            flags_b: 0x123,
            flags_a: 0xAB,
            psd: 222,
            user_info,
            waveform: None,
            event_size: 0,
        }
    }

    /// `opendpp_to_event_data` packs `flags_a` (8b) and `flags_b` (12b) into a
    /// single u32 with `flags_a << 12 | flags_b`. Regression: keep the layout
    /// stable so wire-format consumers (ROOT writer, Monitor) keep parsing.
    #[test]
    fn opendpp_to_event_data_combines_flags_a_and_b() {
        let evt = make_opendpp_event(Vec::new());
        let ed = opendpp_to_event_data(&evt, 1, false);
        assert_eq!(ed.flags, (0xAB_u32 << 12) | 0x123_u32);
        assert_eq!(ed.module, 1);
        assert_eq!(ed.channel, 7);
        assert_eq!(ed.energy, 4096);
        assert_eq!(ed.energy_short, 222);
    }

    /// AMax timestamp = coarse * 8ns + fine * (8/1024)ns. Regression keeps the
    /// scale constants from drifting; without it a renamed `AMAX_TIME_STEP_NS`
    /// would silently misreport sub-ns timing.
    #[test]
    fn opendpp_to_event_data_timestamp_in_ns() {
        let evt = make_opendpp_event(Vec::new());
        let ed = opendpp_to_event_data(&evt, 0, false);
        // 1000 * 8 + 256 * (8/1024) = 8000 + 2.0 = 8002.0
        assert!((ed.timestamp_ns - 8002.0).abs() < 1e-9);
    }

    /// FW emitting >4 user_info slots must populate the first 4 and silently
    /// drop the rest (with a one-shot `info!` log, not in the test). The hot
    /// path stays zero-alloc.
    #[test]
    fn opendpp_to_event_data_truncates_user_info_to_four_slots() {
        let evt = make_opendpp_event(vec![1, 2, 3, 4, 5, 6]);
        let ed = opendpp_to_event_data(&evt, 0, false);
        assert_eq!(ed.user_info, [1, 2, 3, 4]);
    }

    /// Fewer-than-4 slots are zero-padded.
    #[test]
    fn opendpp_to_event_data_pads_short_user_info_with_zeros() {
        let evt = make_opendpp_event(vec![42, 99]);
        let ed = opendpp_to_event_data(&evt, 0, false);
        assert_eq!(ed.user_info, [42, 99, 0, 0]);
    }

    /// `enable_acq_debug=false` keeps the legacy single-lane path: the
    /// whole u16 stream lands in `analog_probe1`, all other probes stay
    /// empty. Regression for the non-debug AMax data path.
    #[test]
    fn opendpp_to_event_data_single_lane_when_debug_off() {
        let mut evt = make_opendpp_event(Vec::new());
        evt.channel = 0;
        evt.waveform = Some(vec![1000, 2000, 3000, 4000, 5000, 6000, 7000, 8000]);
        let ed = opendpp_to_event_data(&evt, 0, false);
        let wf = ed.waveform.expect("waveform should be present");
        assert_eq!(
            wf.analog_probe1,
            vec![1000, 2000, 3000, 4000, 5000, 6000, 7000, 8000]
        );
        assert!(wf.analog_probe2.is_empty());
        assert!(wf.analog_probe3.is_empty());
        assert!(wf.digital_probe1.is_empty());
        assert!(!wf.analog_probe1_is_signed);
    }

    /// `enable_acq_debug=true && channel==0` triggers the 4-lane interleaved
    /// unpack: every 4 consecutive u16s become 1 sample of [raw, trap,
    /// triangle, digital]. Lane 0/1/2 land in analog_probe1/2/3 as signed
    /// 16-bit; lane 3 is bit-decomposed into digital_probe1..5.
    #[test]
    fn opendpp_to_event_data_unpacks_four_lanes_on_ch0_debug() {
        let mut evt = make_opendpp_event(Vec::new());
        evt.channel = 0;
        // Two samples × 4 lanes interleaved:
        //   sample 0: raw=10000, trap=-1, triangle=20000, digital=0xA800
        //   sample 1: raw=10001, trap= 0, triangle=20001, digital=0x5000
        // digital bit map: 15=trig_out, 14=bl_hold, 13=energy_dv,
        //                  12=shaping_dv, 11=shaping_track
        // 0xA800 = 0b1010_1000_0000_0000 → bits 15,13,11 = 1
        // 0x5000 = 0b0101_0000_0000_0000 → bits 14,12 = 1
        evt.waveform = Some(vec![
            10000u16,
            (-1i16) as u16,
            20000u16,
            0xA800u16,
            10001u16,
            0u16,
            20001u16,
            0x5000u16,
        ]);
        let ed = opendpp_to_event_data(&evt, 0, true);
        let wf = ed.waveform.expect("waveform should be present");
        assert_eq!(wf.analog_probe1, vec![10000i16, 10001i16]);
        assert_eq!(wf.analog_probe2, vec![-1i16, 0i16]);
        assert_eq!(wf.analog_probe3, vec![20000i16, 20001i16]);
        // Trigger_out (bit 15): 1, 0
        assert_eq!(wf.digital_probe1, vec![1u8, 0u8]);
        // BL_Hold (bit 14): 0, 1
        assert_eq!(wf.digital_probe2, vec![0u8, 1u8]);
        // Energy_Dv (bit 13): 1, 0
        assert_eq!(wf.digital_probe3, vec![1u8, 0u8]);
        // shaping_dv (bit 12): 0, 1
        assert_eq!(wf.digital_probe4, vec![0u8, 1u8]);
        // shaping_track (bit 11): 1, 0
        assert_eq!(wf.digital_probe5, vec![1u8, 0u8]);
        // Slots 6..16 stay empty in the current digital lane layout.
        assert!(wf.digital_probe6.is_empty());
        // Debug-FW analog lanes are signed.
        assert!(wf.analog_probe1_is_signed);
        assert!(wf.analog_probe2_is_signed);
        assert!(wf.analog_probe3_is_signed);
    }

    /// `enable_acq_debug=true && channel!=0` also takes the 4-lane path:
    /// the broadcast ENABLE_ACQ write fans out across the 32-channel page
    /// array, so every channel emits 4-lane payload (confirmed live
    /// 2026-05-25 — ch4 delivered 4-lane with only ch0's ENABLE_ACQ set;
    /// see commit 37ce069).
    #[test]
    fn opendpp_to_event_data_unpacks_four_lanes_on_non_zero_channel() {
        let mut evt = make_opendpp_event(Vec::new());
        evt.channel = 3;
        evt.waveform = Some(vec![1000u16, 2000u16, 3000u16, 0xA800u16]);
        let ed = opendpp_to_event_data(&evt, 0, true);
        let wf = ed.waveform.expect("waveform should be present");
        assert_eq!(wf.analog_probe1, vec![1000i16]);
        assert_eq!(wf.analog_probe2, vec![2000i16]);
        assert_eq!(wf.analog_probe3, vec![3000i16]);
        // 0xA800 → bits 15 (trig_out), 13 (energy_dv), 11 (shaping_track)
        assert_eq!(wf.digital_probe1, vec![1u8]);
        assert_eq!(wf.digital_probe2, vec![0u8]);
        assert_eq!(wf.digital_probe3, vec![1u8]);
    }

    /// `amax_enable_acq_from_config`: reads `channel_defaults.amax.enable_acq`
    /// and treats `Some(1)` as true, `Some(0)` / `None` / missing-amax-section
    /// as false.
    ///
    /// Built via serde so the test doesn't need to spell every field of
    /// `DigitizerConfig` — that struct grows over time and the literal
    /// form rots quickly.
    #[test]
    fn amax_enable_acq_from_config_reads_channel_default() {
        use crate::config::digitizer::DigitizerConfig;

        let load = |amax_json: &str| -> DigitizerConfig {
            let json = format!(
                r#"{{
                    "digitizer_id": 0,
                    "name": "test",
                    "firmware": "AMax",
                    "board": {{}},
                    "channel_defaults": {{ {amax} }}
                }}"#,
                amax = amax_json
            );
            serde_json::from_str(&json).expect("test JSON should deserialize")
        };

        // No amax section at all → false.
        let cfg = load("");
        assert!(!amax_enable_acq_from_config(&cfg));
        // amax present, enable_acq missing → false.
        let cfg = load(r#""amax": {}"#);
        assert!(!amax_enable_acq_from_config(&cfg));
        // enable_acq=0 → false.
        let cfg = load(r#""amax": { "enable_acq": 0 }"#);
        assert!(!amax_enable_acq_from_config(&cfg));
        // enable_acq=1 → true.
        let cfg = load(r#""amax": { "enable_acq": 1 }"#);
        assert!(amax_enable_acq_from_config(&cfg));
    }

    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_decode_params_ns_per_sample() {
        assert!((X743DecodeParams::ns_per_sample("3.2ghz") - 1.0 / 3.2).abs() < 1e-9);
        assert!((X743DecodeParams::ns_per_sample("1.6GHz") - 1.0 / 1.6).abs() < 1e-9);
        assert!((X743DecodeParams::ns_per_sample("800MHz") - 1.0 / 0.8).abs() < 1e-9);
        assert!((X743DecodeParams::ns_per_sample("unknown") - 1.0 / 3.2).abs() < 1e-9);
    }

    /// Build a test-only `X743DecodeParams`. Avoids the big channel-table literal
    /// from being spelled out in every test.
    #[cfg(feature = "x743")]
    fn x743_test_params(negative: bool) -> X743DecodeParams {
        X743DecodeParams {
            energy_scale: 1.0,
            energy_offset: 0.0,
            save_waveform: false,
            ns_per_sample: 1.0 / 3.2,
            baseline_samples: 16,
            cfd_delay_samples: 2,
            cfd_fraction: 0.5,
            ttf_smoothing_taps: 0,
            channel_negative: [negative; caen_legacy::MAX_CHANNELS],
        }
    }

    /// Synthesize a simple linear-ramp pulse for CFD tests:
    /// - `baseline` samples at 0.0
    /// - linear rise of `rise_len` samples from 0.0 to ±`amp`
    /// - `hold_len` samples at the peak
    /// - flat back to 0 afterwards (if any room)
    #[cfg(feature = "x743")]
    fn x743_synth_pulse(
        baseline: usize,
        rise_len: usize,
        hold_len: usize,
        total: usize,
        amp: f32,
        negative: bool,
    ) -> Vec<f32> {
        let sign = if negative { -1.0 } else { 1.0 };
        let mut v = vec![0.0f32; total];
        for i in 0..rise_len {
            let frac = (i + 1) as f32 / rise_len as f32;
            v[baseline + i] = sign * amp * frac;
        }
        for i in 0..hold_len {
            if baseline + rise_len + i < total {
                v[baseline + rise_len + i] = sign * amp;
            }
        }
        v
    }

    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_cfd_negative_pulse_sub_sample_timing() {
        // 10 ns rise at 0.3125 ns/sample = 32 samples; baseline 64; peak hold 32.
        let ns_per_sample = 1.0 / 3.2;
        let wf = x743_synth_pulse(64, 32, 32, 256, 1000.0, true);
        let stats = X743WaveformStats::analyze(&wf, ns_per_sample, true, 32, 4, 0.3)
            .expect("analyzer returned None");
        assert!(
            stats.cfd_valid,
            "CFD crossing should be found on a clean ramp"
        );
        assert!(stats.baseline.abs() < 1e-3, "baseline ≈ 0 for our synth");
        assert!((stats.amplitude - 1000.0).abs() < 1e-3);
        // For a linear ramp from 0 to −A over `rise_len` samples starting at sample 64,
        // the CFD signal d[i] = f·x[i] − x[i − delay] with f=0.3, delay=4 crosses zero
        // at x[i] = x[i−delay] / f → same-height point on a linear ramp happens when
        //   delta_sample · (1/f − 1) = delay  →  delta = delay / (1/f − 1) = 4 / (10/3 − 1)
        //   = 4 / (7/3) = 12/7 ≈ 1.714 samples past the start of the rise.
        // So the zero-crossing ≈ sample 64 + 1.714 ≈ 65.714 → time ≈ 20.54 ns.
        // Allow up to 1 sample (0.31 ns) of tolerance — the backward-search picks
        // the bracketing samples and linear-interpolates, which on a linear ramp
        // is accurate to well below that tolerance.
        let expected_ns = (64.0 + 12.0 / 7.0) * ns_per_sample;
        let diff = (stats.cfd_time_ns - expected_ns).abs();
        assert!(
            diff < 0.35,
            "cfd_time_ns={} expected≈{} (diff={})",
            stats.cfd_time_ns,
            expected_ns,
            diff
        );
    }

    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_cfd_positive_pulse_finds_edge() {
        let wf = x743_synth_pulse(48, 24, 16, 128, 800.0, false);
        let stats =
            X743WaveformStats::analyze(&wf, 1.0 / 3.2, false, 32, 4, 0.3).expect("analyzer None");
        assert!(stats.cfd_valid);
        assert!((stats.amplitude - 800.0).abs() < 1e-3);
        assert!(stats.peak_index >= 48 + 24 && (stats.peak_index as usize) < 128);
    }

    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_cfd_amplitude_walk_is_small() {
        // With CFD the crossing time should NOT move meaningfully with amplitude
        // as long as the pulse shape is identical. Compare amp=500 vs amp=2000 for
        // the same rise length.
        let ns_per_sample = 1.0 / 3.2;
        let wf_a = x743_synth_pulse(48, 32, 32, 256, 500.0, true);
        let wf_b = x743_synth_pulse(48, 32, 32, 256, 2000.0, true);
        let a = X743WaveformStats::analyze(&wf_a, ns_per_sample, true, 32, 4, 0.3).unwrap();
        let b = X743WaveformStats::analyze(&wf_b, ns_per_sample, true, 32, 4, 0.3).unwrap();
        let walk = (a.cfd_time_ns - b.cfd_time_ns).abs();
        assert!(
            walk < 0.01,
            "CFD amplitude walk too large: {:.3} ns for a 4x amplitude change",
            walk
        );
    }

    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_cfd_short_waveform_returns_none() {
        // `None` when there aren't enough samples for baseline + CFD delay window.
        assert!(X743WaveformStats::analyze(&[0.0f32; 3], 1.0 / 3.2, true, 32, 4, 0.3).is_none());
    }

    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_decode_params_polarity_lookup() {
        use crate::config::digitizer::{ChannelConfig, DigitizerConfig, FirmwareType, X743Config};
        use std::collections::HashMap;
        let mut overrides = HashMap::new();
        overrides.insert(
            3u8,
            ChannelConfig {
                polarity: Some("Positive".to_string()),
                ..Default::default()
            },
        );
        let dc = DigitizerConfig {
            digitizer_id: 0,
            name: "T".into(),
            firmware: FirmwareType::X743Std,
            serial_number: Some("0".into()),
            model: Some("VX1743".into()),
            num_channels: 16,
            is_master: false,
            board: Default::default(),
            sync: None,
            channel_defaults: ChannelConfig {
                polarity: Some("Negative".to_string()),
                ..Default::default()
            },
            channel_overrides: overrides,
            channel_names: None,
            x743: Some(X743Config {
                link_type: "optical".into(),
                link_num: 0,
                conet_node: 0,
                vme_base_address: 0,
                sampling_frequency: "3.2ghz".into(),
                correction_level: "all".into(),
                record_length: 256,
                post_trigger_size: 20,
                max_num_events_blt: 1000,
                io_level: "nim".into(),
                trigger_source: "self".into(),
                group_enable_mask: 1,
                pulse_gen_enabled: false,
                pulse_pattern: 0,
                pulse_source: "continuous".into(),
                fine_time_source: "cfd_soft".into(),
                energy_source: "amplitude".into(),
                energy_scale: 1.0,
                energy_offset: 0.0,
                save_waveform: false,
                baseline_samples: 32,
                cfd_delay_samples: 4,
                cfd_fraction: 0.3,
                charge_gate_pre_samples: 8,
                charge_gate_samples: 64,
                ttf_smoothing: Default::default(),
                extra_registers: Vec::new(),
            }),
            amax_board: None,
        };
        let p = X743DecodeParams::from_config(Some(&dc));
        assert!(p.channel_negative[0], "ch0 defaults to Negative");
        assert!(!p.channel_negative[3], "ch3 overridden to Positive");
        assert!(p.channel_negative[15], "ch15 defaults to Negative");
        // energy_source="amplitude" → soft charge disabled.
        assert!(!p.use_soft_charge);
    }

    /// The gated charge integral: exact value over a known synthetic pulse,
    /// polarity symmetry, and safe gate clamping at the buffer edge.
    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_long_charge_negative_pulse() {
        // baseline 64 @0, linear rise 32 to −1000, hold 32 @−1000, total 256.
        let wf = x743_synth_pulse(64, 32, 32, 256, 1000.0, true);
        // Gate exactly over rise+hold: anchor=64 (rise start), no pre, width 64.
        let q = x743_long_charge(&wf, 0.0, 64, 0, 64, true);
        // rise: Σ 1000·(i+1)/32 for i=0..31 = 1000·16.5 = 16500
        // hold: 32 × 1000 = 32000
        let expected = 16_500.0 + 32_000.0;
        assert!(
            (q - expected).abs() < 1e-1,
            "charge={} expected={}",
            q,
            expected
        );
    }

    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_long_charge_positive_matches_negative_magnitude() {
        // Same pulse shape, positive polarity → same positive charge magnitude.
        let wf = x743_synth_pulse(64, 32, 32, 256, 1000.0, false);
        let q = x743_long_charge(&wf, 0.0, 64, 0, 64, false);
        assert!((q - 48_500.0).abs() < 1e-1, "charge={}", q);
    }

    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_long_charge_gate_clamps_to_buffer() {
        let wf = x743_synth_pulse(64, 32, 32, 256, 1000.0, true);
        // Anchor near the end with an oversized gate → clamps to buffer, no panic.
        // The tail is all baseline, so the integral is ≈ 0.
        let q = x743_long_charge(&wf, 0.0, 250, 4, 10_000, true);
        assert!(
            q.is_finite() && q.abs() < 1.0,
            "tail charge should be ≈0: {}",
            q
        );
        // Zero-width gate integrates nothing.
        assert_eq!(x743_long_charge(&wf, 0.0, 64, 0, 0, true), 0.0);
        // Anchor past the buffer end also stays safe.
        assert_eq!(x743_long_charge(&wf, 0.0, 10_000, 0, 64, true), 0.0);
    }

    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_decode_params_soft_charge_enables() {
        use crate::config::digitizer::{ChannelConfig, DigitizerConfig, FirmwareType, X743Config};
        let dc = DigitizerConfig {
            digitizer_id: 0,
            name: "T".into(),
            firmware: FirmwareType::X743Std,
            serial_number: Some("0".into()),
            model: Some("VX1743".into()),
            num_channels: 16,
            is_master: false,
            board: Default::default(),
            sync: None,
            channel_defaults: ChannelConfig {
                polarity: Some("Negative".to_string()),
                ..Default::default()
            },
            channel_overrides: Default::default(),
            channel_names: None,
            x743: Some(X743Config {
                link_type: "optical".into(),
                link_num: 0,
                conet_node: 0,
                vme_base_address: 0,
                sampling_frequency: "3.2ghz".into(),
                correction_level: "all".into(),
                record_length: 256,
                post_trigger_size: 20,
                max_num_events_blt: 1000,
                io_level: "nim".into(),
                trigger_source: "self".into(),
                group_enable_mask: 1,
                pulse_gen_enabled: false,
                pulse_pattern: 0,
                pulse_source: "continuous".into(),
                fine_time_source: "cfd_soft".into(),
                energy_source: "soft".into(),
                energy_scale: 1.0,
                energy_offset: 0.0,
                save_waveform: false,
                baseline_samples: 32,
                cfd_delay_samples: 4,
                cfd_fraction: 0.3,
                charge_gate_pre_samples: 6,
                charge_gate_samples: 48,
                ttf_smoothing: Default::default(),
                extra_registers: Vec::new(),
            }),
            amax_board: None,
        };
        let p = X743DecodeParams::from_config(Some(&dc));
        assert!(p.use_soft_charge, "energy_source=\"soft\" enables charge");
        assert_eq!(p.charge_gate_pre_samples, 6);
        assert_eq!(p.charge_gate_samples, 48);
    }

    #[cfg(feature = "x743")]
    fn fresh_x743_diag() -> X743TdcDiag {
        X743TdcDiag::new(caen_legacy::MAX_GROUPS)
    }

    /// The diagnostic observer's own logic (backward detection, per-group
    /// isolation, rearm reset) — its sole job is to keep a run30-class glitch
    /// visible, so a regression here would silently defeat that guarantee.
    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_tdc_diag_backward_detection_and_rearm() {
        let mut d = X743TdcDiag::new(2);

        // Forward then backward on group 0.
        d.observe(0, 1000);
        d.observe(0, 5); // backward
        assert_eq!(d.last_raw[0], Some(5), "last_raw tracks the latest raw");
        assert_eq!(d.backward_count[0], 1, "one backward step detected");

        // Group 1 is independent — its first observe cannot be backward.
        d.observe(1, 500);
        assert_eq!(d.last_raw[1], Some(500));
        assert_eq!(d.backward_count[1], 0, "group 1 unaffected by group 0");

        // A forward step is not counted as backward.
        d.observe(0, 2000);
        assert_eq!(d.backward_count[0], 1, "forward step is not backward");

        // Backward counter keeps counting past the log cap (count != log budget):
        // drain the backward budget plus a few more and confirm every backward
        // step is still tallied.
        let over = X743TdcDiag::BACKWARD_PER_RUN + 5;
        for _ in 0..over {
            d.observe(0, 3000); // forward, resets baseline high
            d.observe(0, 1); // backward
        }
        assert_eq!(
            d.backward_count[0],
            1 + over as u64,
            "all backward steps counted even after the log budget is exhausted"
        );

        // rearm() forgets last_raw and clears counters/budgets.
        d.rearm();
        assert_eq!(d.last_raw[0], None);
        assert_eq!(d.last_raw[1], None);
        assert_eq!(d.backward_count[0], 0);
        assert_eq!(d.backward_budget[0], X743TdcDiag::BACKWARD_PER_RUN);

        // First observe after rearm is never treated as backward.
        d.observe(0, 10);
        assert_eq!(
            d.backward_count[0], 0,
            "first post-rearm observe is not backward"
        );
    }

    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_std_event_to_event_data_absent_event() {
        let event = crate::reader::caen_legacy::ffi::CAEN_DGTZ_X743_EVENT_t::default();
        let info = crate::reader::caen_legacy::ffi::CAEN_DGTZ_EventInfo_t::default();
        let params = x743_test_params(true);
        let mut scratch = X743Scratch::new();
        let mut diag = fresh_x743_diag();
        let events = Reader::x743_std_event_to_event_data(
            &event,
            &info,
            7,
            &params,
            &mut scratch,
            &mut diag,
        );
        assert!(events.is_empty());
    }

    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_std_event_to_event_data_no_waveform_fallback() {
        // GrPresent set but ChSize=0 / DataChannel null → analyzer returns None,
        // events still emitted with amplitude=0 and WF_DECODE_FAIL flag.
        let mut event = crate::reader::caen_legacy::ffi::CAEN_DGTZ_X743_EVENT_t::default();
        event.GrPresent[0] = 1;
        event.DataGroup[0].TDC = 100; // 500 ns coarse
        let info = crate::reader::caen_legacy::ffi::CAEN_DGTZ_EventInfo_t::default();
        let params = x743_test_params(true);
        let mut scratch = X743Scratch::new();
        let mut diag = fresh_x743_diag();
        let events = Reader::x743_std_event_to_event_data(
            &event,
            &info,
            0,
            &params,
            &mut scratch,
            &mut diag,
        );
        assert_eq!(events.len(), 2);
        for e in &events {
            assert!((e.timestamp_ns - 500.0).abs() < 1e-9);
            assert_eq!(e.energy, 0);
            assert_eq!(e.flags & (1 << 24), 0, "CFD_VALID must be clear");
            assert_ne!(e.flags & (1 << 25), 0, "WF_DECODE_FAIL must be set");
        }
    }

    /// TODO 62 core guarantee: the V1743 timestamp is the raw 40-bit TDC × 5 ns
    /// with NO rollover extension, so a single garbage/out-of-order TDC value
    /// corrupts only its own event and the very next event self-heals — there is
    /// no persistent state to poison (the run30 failure mode is impossible).
    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_std_event_to_event_data_raw_tdc_self_heals() {
        let mut event = crate::reader::caen_legacy::ffi::CAEN_DGTZ_X743_EVENT_t::default();
        let info = crate::reader::caen_legacy::ffi::CAEN_DGTZ_EventInfo_t::default();
        let params = x743_test_params(true);
        let mut scratch = X743Scratch::new();
        let mut diag = fresh_x743_diag();
        event.GrPresent[0] = 1;

        // Helper: decode one event with a given raw TDC and return group-0 ts_ns.
        let mut decode = |tdc: u64| {
            event.DataGroup[0].TDC = tdc;
            let ev = Reader::x743_std_event_to_event_data(
                &event,
                &info,
                0,
                &params,
                &mut scratch,
                &mut diag,
            );
            assert_eq!(ev.len(), 2);
            ev[0].timestamp_ns
        };

        // Timestamp == raw TDC × 5 ns, directly.
        assert!((decode(1000) - 5000.0).abs() < 1e-9);
        // Backward glitch: ts follows the raw value (goes back), does NOT jump by
        // an epoch — no FLAG_TDC_UNDERFLOW (bit 26) exists any more either.
        let glitch_ts = decode(5);
        assert!((glitch_ts - 25.0).abs() < 1e-9);
        // The NEXT normal event is fully correct — self-healed, no persistent
        // offset from the glitch above.
        assert!((decode(1005) - 5025.0).abs() < 1e-9);

        // Upper u64 garbage bits above bit 39 are masked off (defensive).
        let garbage = (1u64 << 50) | 2000;
        assert!((decode(garbage) - 10000.0).abs() < 1e-9);
    }

    /// `taps == 0` and `taps == 1` are pass-through (no copy, no smoothing).
    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_scratch_smoothing_passthrough() {
        let mut sc = X743Scratch::new();
        sc.raw.extend_from_slice(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        let v0 = sc.smoothed_view(0);
        assert_eq!(v0, &[1.0, 2.0, 3.0, 4.0, 5.0]);
        let v1 = sc.smoothed_view(1);
        assert_eq!(v1, &[1.0, 2.0, 3.0, 4.0, 5.0]);
    }

    /// 2-tap moving average on a step input. Edge handling: the very first
    /// sample averages over only itself (no zero-padding).
    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_scratch_smoothing_2tap_step() {
        let mut sc = X743Scratch::new();
        // Input: 0, 0, 10, 10, 10
        sc.raw.extend_from_slice(&[0.0, 0.0, 10.0, 10.0, 10.0]);
        let v = sc.smoothed_view(2).to_vec();
        // i=0: sum/1 = 0/1 = 0.0
        // i=1: (0+0)/2 = 0.0
        // i=2: (0+10)/2 = 5.0
        // i=3: (10+10)/2 = 10.0
        // i=4: (10+10)/2 = 10.0
        assert_eq!(v, vec![0.0, 0.0, 5.0, 10.0, 10.0]);
    }

    /// 4-tap on a unit impulse: response = [1/1, 1/2, 1/3, 1/4, 0, 0, ...].
    /// Reuses the buffer across calls — verifies `clear()` resets correctly.
    #[cfg(feature = "x743")]
    #[test]
    fn test_x743_scratch_smoothing_4tap_impulse() {
        let mut sc = X743Scratch::new();
        sc.raw.extend_from_slice(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let v = sc.smoothed_view(4).to_vec();
        // i=0: 1/1=1, i=1: 1/2=0.5, i=2: 1/3≈0.333, i=3: 1/4=0.25
        // i=4: drops the impulse → 0
        assert!((v[0] - 1.0).abs() < 1e-6);
        assert!((v[1] - 0.5).abs() < 1e-6);
        assert!((v[2] - 1.0 / 3.0).abs() < 1e-6);
        assert!((v[3] - 0.25).abs() < 1e-6);
        assert!((v[4]).abs() < 1e-6);
        assert!((v[5]).abs() < 1e-6);

        // Reuse: replace raw with a different signal; smoothed buffer must clear.
        sc.raw.clear();
        sc.raw.extend_from_slice(&[2.0; 8]);
        let v2 = sc.smoothed_view(4).to_vec();
        // After 4 samples it's at steady-state = 2.0.
        assert!((v2[3] - 2.0).abs() < 1e-6);
        assert!((v2[7] - 2.0).abs() < 1e-6);
    }
}
