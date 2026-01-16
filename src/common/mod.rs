//! Common data types shared across components
//!
//! This module defines the core data structures for event data transfer
//! and control commands.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// Re-export command types
pub mod command;
pub use command::{Command, CommandResponse, ComponentState, RunConfig};

// Shared state and command handling infrastructure
pub mod state;
pub use state::{handle_command, handle_command_simple, CommandHandlerExt, ComponentSharedState};

// Generic command task for ZMQ REP socket handling
pub mod command_task;
pub use command_task::{run_command_task, run_command_task_with_state};

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

/// Waveform data from digitizer
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Waveform {
    /// Analog probe 1 samples (signed 14-bit values)
    pub analog_probe1: Vec<i16>,
    /// Analog probe 2 samples (signed 14-bit values)
    pub analog_probe2: Vec<i16>,
    /// Digital probe 1 samples (1-bit per sample, packed)
    pub digital_probe1: Vec<u8>,
    /// Digital probe 2 samples (1-bit per sample, packed)
    pub digital_probe2: Vec<u8>,
    /// Digital probe 3 samples (1-bit per sample, packed)
    pub digital_probe3: Vec<u8>,
    /// Digital probe 4 samples (1-bit per sample, packed)
    pub digital_probe4: Vec<u8>,
    /// Time resolution (0=1x, 1=2x, 2=4x, 3=8x)
    pub time_resolution: u8,
    /// Trigger threshold
    pub trigger_threshold: u16,
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
    /// Optional waveform data (skipped in serialization when None)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub waveform: Option<Waveform>,
}

impl EventData {
    /// Create a new EventData without waveform
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
            waveform: None,
        }
    }

    /// Create a new EventData with waveform
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
    EndOfStream { source_id: u32 },
    /// Heartbeat for liveness detection
    Heartbeat(Heartbeat),
}

impl Message {
    /// Create a data message
    pub fn data(batch: EventDataBatch) -> Self {
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
            digital_probe1: vec![0, 1, 0],
            digital_probe2: vec![1, 0, 1],
            digital_probe3: vec![],
            digital_probe4: vec![],
            time_resolution: 1,
            trigger_threshold: 500,
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
        let msg = Message::eos(99);

        assert!(msg.is_eos());
        assert_eq!(msg.source_id(), 99);

        let bytes = msg.to_msgpack().unwrap();
        let decoded = Message::from_msgpack(&bytes).unwrap();

        assert!(decoded.is_eos());
        assert_eq!(decoded.source_id(), 99);
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
        let msg = Message::eos(99);
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
