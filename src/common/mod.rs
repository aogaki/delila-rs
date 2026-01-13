//! Common data types shared across components
//!
//! This module defines the core data structures for event data transfer
//! and control commands.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// Re-export command types
pub mod command;
pub use command::{Command, CommandResponse, ComponentState, RunConfig};

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
}

/// Flag bit definitions (compatible with C++ EventData)
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

/// Minimal event data without waveforms (22 bytes in C++)
///
/// Memory layout matches C++ `MinimalEventData` for binary compatibility.
/// C++ uses `__attribute__((packed))`, Rust uses `#[repr(C, packed)]`.
///
/// # C++ Equivalent
/// ```cpp
/// class MinimalEventData {
///     uint8_t module;          // 1 byte
///     uint8_t channel;         // 1 byte
///     uint16_t energy;         // 2 bytes
///     uint16_t energyShort;    // 2 bytes
///     double timeStampNs;      // 8 bytes
///     uint64_t flags;          // 8 bytes
/// } __attribute__((packed));   // Total: 22 bytes
/// ```
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[repr(C, packed)]
pub struct MinimalEventData {
    /// Hardware module ID (0-255)
    pub module: u8,
    /// Channel within module (0-255)
    pub channel: u8,
    /// Primary energy measurement
    pub energy: u16,
    /// Short gate energy (for PSD)
    pub energy_short: u16,
    /// Timestamp in nanoseconds
    pub timestamp_ns: f64,
    /// Status/error flags
    pub flags: u64,
}

impl MinimalEventData {
    /// Create a new MinimalEventData with all fields
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
        }
    }

    /// Create a zero-initialized MinimalEventData
    pub fn zeroed() -> Self {
        Self {
            module: 0,
            channel: 0,
            energy: 0,
            energy_short: 0,
            timestamp_ns: 0.0,
            flags: 0,
        }
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

// Compile-time size check: MinimalEventData must be exactly 22 bytes
const _: () = assert!(
    std::mem::size_of::<MinimalEventData>() == 22,
    "MinimalEventData must be 22 bytes"
);

/// Batch of minimal event data for network transfer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinimalEventDataBatch {
    /// Source identifier (digitizer/emulator ID)
    pub source_id: u32,
    /// Sequence number for ordering and loss detection
    pub sequence_number: u64,
    /// Batch creation timestamp (Unix time in nanoseconds)
    pub timestamp: u64,
    /// Event data
    pub events: Vec<MinimalEventData>,
}

impl MinimalEventDataBatch {
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
    pub fn push(&mut self, event: MinimalEventData) {
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
    Data(MinimalEventDataBatch),
    /// End of stream signal - source is shutting down
    EndOfStream { source_id: u32 },
    /// Heartbeat for liveness detection
    Heartbeat(Heartbeat),
}

impl Message {
    /// Create a data message
    pub fn data(batch: MinimalEventDataBatch) -> Self {
        Self::Data(batch)
    }

    /// Create an EOS message
    pub fn eos(source_id: u32) -> Self {
        Self::EndOfStream { source_id }
    }

    /// Check if this is an EOS message
    pub fn is_eos(&self) -> bool {
        matches!(self, Self::EndOfStream { .. })
    }

    /// Get source_id from message
    pub fn source_id(&self) -> u32 {
        match self {
            Self::Data(batch) => batch.source_id,
            Self::EndOfStream { source_id } => *source_id,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_event_data_size() {
        // Verify struct is exactly 22 bytes (same as C++)
        assert_eq!(std::mem::size_of::<MinimalEventData>(), 22);
    }

    #[test]
    fn minimal_event_data_roundtrip() {
        let event = MinimalEventData::new(
            1,      // module
            2,      // channel
            1000,   // energy
            800,    // energy_short
            123456789.0, // timestamp_ns
            flags::FLAG_PILEUP | flags::FLAG_OVER_RANGE, // flags
        );

        // Serialize and deserialize
        let bytes = rmp_serde::to_vec(&event).unwrap();
        let decoded: MinimalEventData = rmp_serde::from_slice(&bytes).unwrap();

        assert_eq!(event, decoded);
    }

    #[test]
    fn batch_roundtrip() {
        let mut batch = MinimalEventDataBatch::new(42, 1);
        batch.push(MinimalEventData::new(0, 0, 100, 80, 1000.0, 0));
        batch.push(MinimalEventData::new(0, 1, 200, 160, 2000.0, flags::FLAG_PILEUP));

        let bytes = batch.to_msgpack().unwrap();
        let decoded = MinimalEventDataBatch::from_msgpack(&bytes).unwrap();

        assert_eq!(batch.source_id, decoded.source_id);
        assert_eq!(batch.sequence_number, decoded.sequence_number);
        assert_eq!(batch.events.len(), decoded.events.len());
        assert_eq!(batch.events[0], decoded.events[0]);
        assert_eq!(batch.events[1], decoded.events[1]);
    }

    #[test]
    fn flag_helpers() {
        let event = MinimalEventData::new(
            0, 0, 0, 0, 0.0,
            flags::FLAG_PILEUP | flags::FLAG_OVER_RANGE,
        );

        assert!(event.has_pileup());
        assert!(!event.has_trigger_lost());
        assert!(event.has_over_range());
    }

    #[test]
    fn message_data_roundtrip() {
        let batch = MinimalEventDataBatch::new(42, 1);
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
        let msg = Message::eos(99);

        assert!(msg.is_eos());
        assert_eq!(msg.source_id(), 99);

        let bytes = msg.to_msgpack().unwrap();
        let decoded = Message::from_msgpack(&bytes).unwrap();

        assert!(decoded.is_eos());
        assert_eq!(decoded.source_id(), 99);
    }
}
