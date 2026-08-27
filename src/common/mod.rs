//! Common data types shared across components
//!
//! This module defines the core data structures for event data transfer
//! and control commands.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// CLI argument parsing
pub mod cli;
pub use cli::{
    CommonArgs, ControllerArgs, DataSinkArgs, MergerArgs, MonitorArgs, OperatorArgs, PipelineArgs,
    RecorderArgs, SourceArgs,
};

// Re-export command types
pub mod command;
pub use command::{
    ChannelRegistration, Command, CommandResponse, ComponentState, EmulatorRuntimeConfig, RunConfig,
};

// Shared state and command handling infrastructure
pub mod state;
pub use state::{handle_command, handle_command_simple, CommandHandlerExt, ComponentSharedState};

// Generic command task for ZMQ REP socket handling
pub mod command_task;
pub use command_task::{run_command_task, run_command_task_with_state};

// Unified metrics framework
pub mod metrics;
pub use metrics::{AtomicCounters, CounterSnapshot, RateSnapshot};

// Common error types
pub mod error;
pub use error::{PipelineError, PipelineResult};

// Unified shutdown handling
pub mod shutdown;
pub use shutdown::{setup_shutdown, setup_shutdown_with_message, ShutdownReceiver, ShutdownSender};

// ZMQ socket initialization helpers (HWM=0 policy)
pub mod zmq_helper;

/// Byte/item accounting for unbounded inter-task channels (TODO 68).
pub mod queue_accounting;
pub use zmq_helper::{pub_no_hwm, sub_no_hwm};

pub mod delila_schema;

/// Heartbeat message for liveness detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    /// Source identifier
    pub source_id: u32,
    /// Unix timestamp in nanoseconds
    pub timestamp: u64,
    /// Monotonic counter
    pub counter: u64,
}

impl Heartbeat {
    /// Create a new heartbeat
    pub fn new(source_id: u32, counter: u64) -> Self {
        Self {
            source_id,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
            counter,
        }
    }
}

/// Component metrics for monitoring
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ComponentMetrics {
    /// Total events processed
    pub events_processed: u64,
    /// Total bytes transferred
    pub bytes_transferred: u64,
    /// Current queue size
    pub queue_size: u32,
    /// Maximum queue capacity
    pub queue_max: u32,
    /// Events per second
    pub event_rate: f64,
    /// Bytes per second
    pub data_rate: f64,
    /// Cumulative trigger loss count (DIG1: estimated from flags, DIG2: exact from counters)
    #[serde(default)]
    pub trigger_loss_count: u64,
    /// Trigger loss rate as percentage (0.0 - 100.0)
    #[serde(default)]
    pub trigger_loss_rate: f64,
    /// Per-channel cumulative event counts (index = channel number)
    /// Only populated by Reader components; None for others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_counts: Option<Vec<u64>>,
    /// Current backlog in the component's unbounded channel, in bytes
    /// (TODO 68). Non-zero only for Merger/Recorder today.
    #[serde(default)]
    pub queue_bytes: u64,
    /// High-water mark of `queue_bytes` since the current run started.
    #[serde(default)]
    pub queue_bytes_peak: u64,
    /// Backlog severity vs the configured watermarks:
    /// 0 = OK, 1 = soft limit exceeded, 2 = hard limit exceeded
    /// (drain-first stop material). See `queue_accounting::backlog_level`.
    #[serde(default)]
    pub backlog_level: u8,
}

/// Flag bit definitions for event status
pub mod flags {
    /// Pileup detected
    pub const FLAG_PILEUP: u64 = 0x01;
    /// Trigger lost
    pub const FLAG_TRIGGER_LOST: u64 = 0x02;
    /// Signal saturation (over range)
    pub const FLAG_OVER_RANGE: u64 = 0x04;
    /// 1024 trigger count
    pub const FLAG_1024_TRIGGER: u64 = 0x08;
    /// N lost triggers
    pub const FLAG_N_LOST_TRIGGER: u64 = 0x10;
}

/// Sentinel value for "probe type unknown / not parsed". FW that doesn't
/// carry probe-type info on the wire (PSD1/PSD2/PHA1/AMax/V1743) emits
/// this so the UI falls back to a generic "A0" / "D0" label rather than
/// claiming a specific probe identity.
pub const UNKNOWN_PROBE_TYPE: u8 = 0xFF;

fn default_unknown_analog_probe_types() -> [u8; 3] {
    [UNKNOWN_PROBE_TYPE; 3]
}
fn default_unknown_digital_probe_types() -> [u8; 16] {
    [UNKNOWN_PROBE_TYPE; 16]
}

/// Waveform data from digitizer
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Waveform {
    /// Analog probe 1 samples
    pub analog_probe1: Vec<i16>,
    /// Analog probe 2 samples
    pub analog_probe2: Vec<i16>,
    /// Analog probe 3 samples (AMax debug FW: triangle filter wave). Empty
    /// Vec for FW that emits ≤ 2 analog probes — `#[serde(default)]` keeps
    /// older `.delila` readers happy.
    #[serde(default)]
    pub analog_probe3: Vec<i16>,
    /// Digital probe 1 samples (one u8 = 0 or 1 per sample; NOT bit-packed)
    pub digital_probe1: Vec<u8>,
    /// Digital probe 2 samples (one u8 = 0 or 1 per sample; NOT bit-packed)
    pub digital_probe2: Vec<u8>,
    /// Digital probe 3 samples (one u8 = 0 or 1 per sample; NOT bit-packed)
    pub digital_probe3: Vec<u8>,
    /// Digital probe 4 samples (one u8 = 0 or 1 per sample; NOT bit-packed)
    pub digital_probe4: Vec<u8>,
    /// Digital probe 5 samples (one u8 = 0 or 1 per sample; NOT bit-packed) — AMax debug FW
    /// shaping_track. Empty Vec for FW that emits ≤ 4 digital probes.
    #[serde(default)]
    pub digital_probe5: Vec<u8>,
    /// Digital probes 6..16 — reserved slots for future AMax debug FW
    /// digital lane bits 10..0 (currently constant 0 in hardware). Empty
    /// for every FW today.
    #[serde(default)]
    pub digital_probe6: Vec<u8>,
    #[serde(default)]
    pub digital_probe7: Vec<u8>,
    #[serde(default)]
    pub digital_probe8: Vec<u8>,
    #[serde(default)]
    pub digital_probe9: Vec<u8>,
    #[serde(default)]
    pub digital_probe10: Vec<u8>,
    #[serde(default)]
    pub digital_probe11: Vec<u8>,
    #[serde(default)]
    pub digital_probe12: Vec<u8>,
    #[serde(default)]
    pub digital_probe13: Vec<u8>,
    #[serde(default)]
    pub digital_probe14: Vec<u8>,
    #[serde(default)]
    pub digital_probe15: Vec<u8>,
    #[serde(default)]
    pub digital_probe16: Vec<u8>,
    /// Time resolution (0=1x, 1=2x, 2=4x, 3=8x)
    pub time_resolution: u8,
    /// Trigger threshold
    pub trigger_threshold: u16,
    /// Nanoseconds per waveform sample (0.0 = unknown)
    #[serde(default)]
    pub ns_per_sample: f64,
    /// True when `analog_probe1` is sign-extended 14-bit data (PHA1
    /// trapezoid / Delta probe etc., range `[-8192, 8191]`). False for
    /// raw-ADC unsigned probes (PSD1 / PSD2 / AMax input, range
    /// `[0, 16383]`). The frontend reads this to decide whether to apply
    /// the +8191 centering offset that keeps signed probes visible
    /// alongside unsigned ones.
    #[serde(default)]
    pub analog_probe1_is_signed: bool,
    /// Same as `analog_probe1_is_signed` for the second analog probe.
    #[serde(default)]
    pub analog_probe2_is_signed: bool,
    /// Same as `analog_probe1_is_signed` for the third analog probe
    /// (AMax debug FW triangle wave is signed 16-bit).
    #[serde(default)]
    pub analog_probe3_is_signed: bool,
    /// Probe-type identifier for analog probes 0..2 — PHA2 canonical
    /// encoding (CAEN doxygen `legacy/PHA2_Parameters/a00108.html`):
    /// 0=ADCInput, 1=TimeFilter, 2=EnergyFilter, 3=EnergyFilterBaseline,
    /// 4=EnergyFilterMinusBaseline, 0xFF=`UNKNOWN_PROBE_TYPE` (FW that
    /// doesn't carry probe-type info on the wire). AMax debug FW uses
    /// the 0x40+ range. The frontend maps these to display labels like
    /// "A0: TimeFilter". Old `.delila` files (pre-2026-05-05) without
    /// this field deserialize to all-`UNKNOWN_PROBE_TYPE` via the custom
    /// default fn.
    #[serde(default = "default_unknown_analog_probe_types")]
    pub analog_probe_type: [u8; 3],
    /// Probe-type identifier for digital probes 0..15 — PHA2 canonical
    /// encoding: 0=Trigger, 1=TimeFilterArmed, 2=ReTriggerGuard,
    /// 3=EnergyFilterBaselineFreeze, 4=EnergyFilterPeaking,
    /// 5=EnergyFilterPeakReady, 6=EnergyFilterPileUpGuard, 7=EventPileUp,
    /// 8=ADCSaturation, 9=ADCSaturationProtection, A=PostSaturationEvent,
    /// B=EnergyFilterSaturation, C=SignalInhibit, 0xFF=`UNKNOWN_PROBE_TYPE`.
    /// AMax debug FW uses the 0x40+ range. Slots 5..15 reserved for future
    /// digital lane bit assignments.
    #[serde(default = "default_unknown_digital_probe_types")]
    pub digital_probe_type: [u8; 16],
}

impl Default for Waveform {
    fn default() -> Self {
        Self {
            analog_probe1: Vec::new(),
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
            ns_per_sample: 0.0,
            analog_probe1_is_signed: false,
            analog_probe2_is_signed: false,
            analog_probe3_is_signed: false,
            analog_probe_type: [UNKNOWN_PROBE_TYPE; 3],
            digital_probe_type: [UNKNOWN_PROBE_TYPE; 16],
        }
    }
}

/// Event data with optional waveform
///
/// This is the unified event type used throughout the pipeline.
/// When waveform is None, serialization skips the field for minimal overhead.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventData {
    /// Hardware module ID (0-255)
    pub module: u8,
    /// Channel within module (0-255)
    pub channel: u8,
    /// Primary energy measurement
    pub energy: u16,
    /// Short gate energy (for PSD)
    pub energy_short: u16,
    /// Timestamp in nanoseconds (includes fine time)
    pub timestamp_ns: f64,
    /// Status/error flags (u64 for future extensibility)
    pub flags: u64,
    /// Per-event user info slots (AMax: 4 × u64 from OpenDPP user words).
    /// Slot 0 = AMax peak value (typical), 1 = baseline, 2-3 = FW-specific.
    /// All slots are 0 for non-AMax firmware. `#[serde(default)]` keeps the
    /// wire format backward compatible with older `.delila` files / consumers
    /// that pre-date the AMax integration.
    #[serde(default)]
    pub user_info: [u64; 4],
    /// Optional waveform data. Always serialized (as MessagePack `nil` when
    /// absent) so every `EventData` record is a fixed 8-element array — this
    /// keeps the self-describing wire layout uniform for the C++ `TDelila`
    /// reader (format v3+). `#[serde(default)]` keeps legacy v2 files (which
    /// omitted the field entirely) readable: the missing trailing field
    /// deserializes to `None`.
    #[serde(default)]
    pub waveform: Option<Waveform>,
}

impl EventData {
    /// Create a new EventData without waveform (user_info defaults to [0;4])
    pub fn new(
        module: u8,
        channel: u8,
        energy: u16,
        energy_short: u16,
        timestamp_ns: f64,
        flags: u64,
    ) -> Self {
        Self {
            module,
            channel,
            energy,
            energy_short,
            timestamp_ns,
            flags,
            user_info: [0; 4],
            waveform: None,
        }
    }

    /// Create a new EventData with waveform (user_info defaults to [0;4])
    pub fn with_waveform(
        module: u8,
        channel: u8,
        energy: u16,
        energy_short: u16,
        timestamp_ns: f64,
        flags: u64,
        waveform: Waveform,
    ) -> Self {
        Self {
            module,
            channel,
            energy,
            energy_short,
            timestamp_ns,
            flags,
            user_info: [0; 4],
            waveform: Some(waveform),
        }
    }

    /// Create a zero-initialized EventData
    pub fn zeroed() -> Self {
        Self {
            module: 0,
            channel: 0,
            energy: 0,
            energy_short: 0,
            timestamp_ns: 0.0,
            flags: 0,
            user_info: [0; 4],
            waveform: None,
        }
    }

    /// Check if this event has waveform data
    #[inline]
    pub fn has_waveform(&self) -> bool {
        self.waveform.is_some()
    }

    /// Check if pileup was detected
    #[inline]
    pub fn has_pileup(&self) -> bool {
        (self.flags & flags::FLAG_PILEUP) != 0
    }

    /// Check if trigger was lost
    #[inline]
    pub fn has_trigger_lost(&self) -> bool {
        (self.flags & flags::FLAG_TRIGGER_LOST) != 0
    }

    /// Check if signal was saturated (over range)
    #[inline]
    pub fn has_over_range(&self) -> bool {
        (self.flags & flags::FLAG_OVER_RANGE) != 0
    }
}

impl Default for EventData {
    fn default() -> Self {
        Self::zeroed()
    }
}

/// Batch of event data for network transfer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventDataBatch {
    /// Source identifier (digitizer/emulator ID)
    pub source_id: u32,
    /// Sequence number for ordering and loss detection
    pub sequence_number: u64,
    /// Batch creation timestamp (Unix time in nanoseconds)
    pub timestamp: u64,
    /// Event data
    pub events: Vec<EventData>,
}

impl EventDataBatch {
    /// Create a new empty batch
    pub fn new(source_id: u32, sequence_number: u64) -> Self {
        Self {
            source_id,
            sequence_number,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
            events: Vec::new(),
        }
    }

    /// Create a batch with pre-allocated capacity
    pub fn with_capacity(source_id: u32, sequence_number: u64, capacity: usize) -> Self {
        Self {
            source_id,
            sequence_number,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
            events: Vec::with_capacity(capacity),
        }
    }

    /// Number of events in the batch
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Check if batch is empty
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Add an event to the batch
    pub fn push(&mut self, event: EventData) {
        self.events.push(event);
    }

    /// Serialize to MessagePack bytes
    pub fn to_msgpack(&self) -> Result<Vec<u8>, rmp_serde::encode::Error> {
        rmp_serde::to_vec(self)
    }

    /// Deserialize from MessagePack bytes
    pub fn from_msgpack(bytes: &[u8]) -> Result<Self, rmp_serde::decode::Error> {
        rmp_serde::from_slice(bytes)
    }
}

/// Message type for pipeline communication
///
/// Wraps either event data or control signals (like EOS/Heartbeat).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    /// Event data batch
    Data(EventDataBatch),
    /// End of stream signal - source is shutting down
    EndOfStream { source_id: u32, run_number: u32 },
    /// Heartbeat for liveness detection
    Heartbeat(Heartbeat),
}

impl Message {
    /// Create a data message
    pub fn data(batch: EventDataBatch) -> Self {
        Self::Data(batch)
    }

    /// Create an EOS message
    pub fn eos(source_id: u32, run_number: u32) -> Self {
        Self::EndOfStream {
            source_id,
            run_number,
        }
    }

    /// Check if this is an EOS message
    pub fn is_eos(&self) -> bool {
        matches!(self, Self::EndOfStream { .. })
    }

    /// Get source_id from message
    pub fn source_id(&self) -> u32 {
        match self {
            Self::Data(batch) => batch.source_id,
            Self::EndOfStream { source_id, .. } => *source_id,
            Self::Heartbeat(hb) => hb.source_id,
        }
    }

    /// Check if this is a heartbeat message
    pub fn is_heartbeat(&self) -> bool {
        matches!(self, Self::Heartbeat(_))
    }

    /// Create a heartbeat message
    pub fn heartbeat(source_id: u32, counter: u64) -> Self {
        Self::Heartbeat(Heartbeat::new(source_id, counter))
    }

    /// Serialize to MessagePack bytes
    pub fn to_msgpack(&self) -> Result<Vec<u8>, rmp_serde::encode::Error> {
        rmp_serde::to_vec(self)
    }

    /// Deserialize from MessagePack bytes
    pub fn from_msgpack(bytes: &[u8]) -> Result<Self, rmp_serde::decode::Error> {
        rmp_serde::from_slice(bytes)
    }
}

/// Lightweight header info extracted from raw MessagePack bytes
/// Used for zero-copy forwarding where only metadata is needed
#[derive(Debug, Clone, Copy)]
pub enum MessageHeader {
    /// Data batch with source_id and sequence_number
    Data {
        source_id: u32,
        sequence_number: u64,
    },
    /// End of stream
    EndOfStream { source_id: u32 },
    /// Heartbeat
    Heartbeat { source_id: u32 },
}

impl MessageHeader {
    /// Extract header info from raw MessagePack bytes without full deserialization
    ///
    /// MessagePack format for Message enum:
    /// - fixmap with 1 entry: 0x81 (map of 1)
    /// - key: fixstr "Data", "EndOfStream", or "Heartbeat"
    /// - value: the actual data
    ///
    /// For Data variant, we need source_id and sequence_number from MinimalEventDataBatch
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }

        // Quick check: MessagePack map header
        // 0x81 = fixmap with 1 entry
        if bytes[0] != 0x81 {
            return None;
        }

        if bytes.len() < 3 {
            return None;
        }

        // Next byte should be fixstr length (0xa0-0xbf for str 0-31)
        let key_len = match bytes[1] {
            b if (0xa0..=0xbf).contains(&b) => (b & 0x1f) as usize,
            _ => return None,
        };

        if bytes.len() < 2 + key_len {
            return None;
        }

        let key = &bytes[2..2 + key_len];
        let value_start = 2 + key_len;

        match key {
            b"Data" => {
                // Data variant: value is MinimalEventDataBatch
                // It's a map with source_id, sequence_number, timestamp, events
                Self::parse_data_header(&bytes[value_start..])
            }
            b"EndOfStream" => {
                // EndOfStream variant: value is a map with source_id
                Self::parse_eos_header(&bytes[value_start..])
            }
            b"Heartbeat" => {
                // Heartbeat variant: value is a Heartbeat struct
                Self::parse_heartbeat_header(&bytes[value_start..])
            }
            _ => None,
        }
    }

    /// Parse Data variant header to extract source_id and sequence_number
    fn parse_data_header(bytes: &[u8]) -> Option<Self> {
        // rmp_serde serializes structs as arrays by default:
        // EventDataBatch is [source_id, sequence_number, timestamp, events]
        // We only need source_id (index 0) and sequence_number (index 1)

        if bytes.is_empty() {
            return None;
        }

        // Should be an array (fixarray 0x90-0x9f or array16 0xdc or array32 0xdd)
        let (_array_size, mut pos) = match bytes[0] {
            b if (0x90..=0x9f).contains(&b) => ((b & 0x0f) as usize, 1),
            0xdc if bytes.len() >= 3 => {
                let size = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
                (size, 3)
            }
            0xdd if bytes.len() >= 5 => {
                let size = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
                (size, 5)
            }
            _ => return None,
        };

        // Parse source_id (first element)
        let source_id = Self::parse_u32(&bytes[pos..], &mut pos)?;

        // Parse sequence_number (second element)
        let sequence_number = Self::parse_u64(&bytes[pos..], &mut pos)?;

        Some(MessageHeader::Data {
            source_id,
            sequence_number,
        })
    }

    /// Parse EndOfStream header
    fn parse_eos_header(bytes: &[u8]) -> Option<Self> {
        // rmp_serde serializes structs as arrays:
        // EndOfStream { source_id: u32 } -> [source_id]
        if bytes.is_empty() {
            return None;
        }

        // Should be an array with 1 element (fixarray 0x91)
        let mut pos = match bytes[0] {
            b if (0x90..=0x9f).contains(&b) => 1,
            _ => return None,
        };

        // Parse source_id (first and only element)
        let source_id = Self::parse_u32(&bytes[pos..], &mut pos)?;
        Some(MessageHeader::EndOfStream { source_id })
    }

    /// Parse Heartbeat header
    fn parse_heartbeat_header(bytes: &[u8]) -> Option<Self> {
        // rmp_serde serializes structs as arrays:
        // Heartbeat { source_id, timestamp, counter } -> [source_id, timestamp, counter]
        if bytes.is_empty() {
            return None;
        }

        // Should be an array with 3 elements (fixarray 0x93)
        let mut pos = match bytes[0] {
            b if (0x90..=0x9f).contains(&b) => 1,
            _ => return None,
        };

        // Parse source_id (first element)
        let source_id = Self::parse_u32(&bytes[pos..], &mut pos)?;
        Some(MessageHeader::Heartbeat { source_id })
    }

    /// Parse u32 from MessagePack
    fn parse_u32(bytes: &[u8], pos: &mut usize) -> Option<u32> {
        if bytes.is_empty() {
            return None;
        }

        match bytes[0] {
            // positive fixint (0x00-0x7f)
            b if b <= 0x7f => {
                *pos += 1;
                Some(b as u32)
            }
            // uint8 (0xcc)
            0xcc if bytes.len() >= 2 => {
                *pos += 2;
                Some(bytes[1] as u32)
            }
            // uint16 (0xcd)
            0xcd if bytes.len() >= 3 => {
                *pos += 3;
                Some(u16::from_be_bytes([bytes[1], bytes[2]]) as u32)
            }
            // uint32 (0xce)
            0xce if bytes.len() >= 5 => {
                *pos += 5;
                Some(u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]))
            }
            _ => None,
        }
    }

    /// Parse u64 from MessagePack
    fn parse_u64(bytes: &[u8], pos: &mut usize) -> Option<u64> {
        if bytes.is_empty() {
            return None;
        }

        match bytes[0] {
            // positive fixint (0x00-0x7f)
            b if b <= 0x7f => {
                *pos += 1;
                Some(b as u64)
            }
            // uint8 (0xcc)
            0xcc if bytes.len() >= 2 => {
                *pos += 2;
                Some(bytes[1] as u64)
            }
            // uint16 (0xcd)
            0xcd if bytes.len() >= 3 => {
                *pos += 3;
                Some(u16::from_be_bytes([bytes[1], bytes[2]]) as u64)
            }
            // uint32 (0xce)
            0xce if bytes.len() >= 5 => {
                *pos += 5;
                Some(u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as u64)
            }
            // uint64 (0xcf)
            0xcf if bytes.len() >= 9 => {
                *pos += 9;
                Some(u64::from_be_bytes([
                    bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8],
                ]))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_data_roundtrip() {
        let event = EventData::new(
            1,                                           // module
            2,                                           // channel
            1000,                                        // energy
            800,                                         // energy_short
            123456789.0,                                 // timestamp_ns
            flags::FLAG_PILEUP | flags::FLAG_OVER_RANGE, // flags
        );

        // Serialize and deserialize
        let bytes = rmp_serde::to_vec(&event).unwrap();
        let decoded: EventData = rmp_serde::from_slice(&bytes).unwrap();

        assert_eq!(event, decoded);
        assert!(!decoded.has_waveform());
    }

    #[test]
    fn event_data_with_waveform_roundtrip() {
        let wf = Waveform {
            analog_probe1: vec![100, 200, 300],
            analog_probe2: vec![50, 100, 150],
            analog_probe3: vec![],
            digital_probe1: vec![0, 1, 0],
            digital_probe2: vec![1, 0, 1],
            digital_probe3: vec![],
            digital_probe4: vec![],
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
            time_resolution: 1,
            trigger_threshold: 500,
            ns_per_sample: 2.0,
            analog_probe1_is_signed: false,
            analog_probe2_is_signed: false,
            analog_probe3_is_signed: false,
            analog_probe_type: [UNKNOWN_PROBE_TYPE; 3],
            digital_probe_type: [UNKNOWN_PROBE_TYPE; 16],
        };

        let event = EventData::with_waveform(1, 2, 1000, 800, 123456789.0, 0, wf);

        let bytes = rmp_serde::to_vec(&event).unwrap();
        let decoded: EventData = rmp_serde::from_slice(&bytes).unwrap();

        assert_eq!(event, decoded);
        assert!(decoded.has_waveform());
        assert_eq!(decoded.waveform.as_ref().unwrap().analog_probe1.len(), 3);
    }

    #[test]
    fn batch_roundtrip() {
        let mut batch = EventDataBatch::new(42, 1);
        batch.push(EventData::new(0, 0, 100, 80, 1000.0, 0));
        batch.push(EventData::new(0, 1, 200, 160, 2000.0, flags::FLAG_PILEUP));

        let bytes = batch.to_msgpack().unwrap();
        let decoded = EventDataBatch::from_msgpack(&bytes).unwrap();

        assert_eq!(batch.source_id, decoded.source_id);
        assert_eq!(batch.sequence_number, decoded.sequence_number);
        assert_eq!(batch.events.len(), decoded.events.len());
        assert_eq!(batch.events[0], decoded.events[0]);
        assert_eq!(batch.events[1], decoded.events[1]);
    }

    #[test]
    fn flag_helpers() {
        let event = EventData::new(0, 0, 0, 0, 0.0, flags::FLAG_PILEUP | flags::FLAG_OVER_RANGE);

        assert!(event.has_pileup());
        assert!(!event.has_trigger_lost());
        assert!(event.has_over_range());
    }

    #[test]
    fn message_data_roundtrip() {
        let batch = EventDataBatch::new(42, 1);
        let msg = Message::data(batch);

        assert!(!msg.is_eos());
        assert_eq!(msg.source_id(), 42);

        let bytes = msg.to_msgpack().unwrap();
        let decoded = Message::from_msgpack(&bytes).unwrap();

        assert!(!decoded.is_eos());
        assert_eq!(decoded.source_id(), 42);
    }

    #[test]
    fn message_eos_roundtrip() {
        let msg = Message::eos(99, 42);

        assert!(msg.is_eos());
        assert_eq!(msg.source_id(), 99);

        let bytes = msg.to_msgpack().unwrap();
        let decoded = Message::from_msgpack(&bytes).unwrap();

        assert!(decoded.is_eos());
        assert_eq!(decoded.source_id(), 99);
        if let Message::EndOfStream { run_number, .. } = decoded {
            assert_eq!(run_number, 42);
        }
    }

    #[test]
    fn message_header_parse_data() {
        let mut batch = EventDataBatch::new(42, 1);
        batch.sequence_number = 12345;
        batch.push(EventData::new(0, 0, 100, 80, 1000.0, 0));

        let msg = Message::data(batch);
        let bytes = msg.to_msgpack().unwrap();

        // Debug: print first 50 bytes
        println!("Message bytes ({} total):", bytes.len());
        for (i, b) in bytes.iter().take(50).enumerate() {
            print!("{:02x} ", b);
            if (i + 1) % 16 == 0 {
                println!();
            }
        }
        println!();

        let header = MessageHeader::parse(&bytes);
        println!("Parsed header: {:?}", header);

        assert!(header.is_some(), "Failed to parse header");
        match header.unwrap() {
            MessageHeader::Data {
                source_id,
                sequence_number,
            } => {
                assert_eq!(source_id, 42);
                assert_eq!(sequence_number, 12345);
            }
            _ => panic!("Expected Data variant"),
        }
    }

    #[test]
    fn message_header_parse_eos() {
        let msg = Message::eos(99, 0);
        let bytes = msg.to_msgpack().unwrap();

        println!("EOS bytes: {:02x?}", &bytes);

        let header = MessageHeader::parse(&bytes);
        assert!(header.is_some(), "Failed to parse EOS header");
        match header.unwrap() {
            MessageHeader::EndOfStream { source_id } => {
                assert_eq!(source_id, 99);
            }
            _ => panic!("Expected EndOfStream variant"),
        }
    }
}
