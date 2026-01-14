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

/// Lightweight header info extracted from raw MessagePack bytes
/// Used for zero-copy forwarding where only metadata is needed
#[derive(Debug, Clone, Copy)]
pub enum MessageHeader {
    /// Data batch with source_id and sequence_number
    Data { source_id: u32, sequence_number: u64 },
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
        // MinimalEventDataBatch is [source_id, sequence_number, timestamp, events]
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

    /// Skip a MessagePack value and return new position
    fn skip_value(bytes: &[u8], pos: usize) -> Option<usize> {
        if pos >= bytes.len() {
            return None;
        }

        let b = bytes[pos];
        match b {
            // positive fixint
            0x00..=0x7f => Some(pos + 1),
            // fixmap
            0x80..=0x8f => {
                let count = (b & 0x0f) as usize;
                let mut p = pos + 1;
                for _ in 0..count * 2 {
                    p = Self::skip_value(bytes, p)?;
                }
                Some(p)
            }
            // fixarray
            0x90..=0x9f => {
                let count = (b & 0x0f) as usize;
                let mut p = pos + 1;
                for _ in 0..count {
                    p = Self::skip_value(bytes, p)?;
                }
                Some(p)
            }
            // fixstr
            0xa0..=0xbf => {
                let len = (b & 0x1f) as usize;
                Some(pos + 1 + len)
            }
            // nil
            0xc0 => Some(pos + 1),
            // false, true
            0xc2 | 0xc3 => Some(pos + 1),
            // bin8
            0xc4 if pos + 1 < bytes.len() => Some(pos + 2 + bytes[pos + 1] as usize),
            // bin16
            0xc5 if pos + 2 < bytes.len() => {
                let len = u16::from_be_bytes([bytes[pos + 1], bytes[pos + 2]]) as usize;
                Some(pos + 3 + len)
            }
            // float32
            0xca => Some(pos + 5),
            // float64
            0xcb => Some(pos + 9),
            // uint8, int8
            0xcc | 0xd0 => Some(pos + 2),
            // uint16, int16
            0xcd | 0xd1 => Some(pos + 3),
            // uint32, int32
            0xce | 0xd2 => Some(pos + 5),
            // uint64, int64
            0xcf | 0xd3 => Some(pos + 9),
            // str8
            0xd9 if pos + 1 < bytes.len() => Some(pos + 2 + bytes[pos + 1] as usize),
            // str16
            0xda if pos + 2 < bytes.len() => {
                let len = u16::from_be_bytes([bytes[pos + 1], bytes[pos + 2]]) as usize;
                Some(pos + 3 + len)
            }
            // str32
            0xdb if pos + 4 < bytes.len() => {
                let len = u32::from_be_bytes([
                    bytes[pos + 1],
                    bytes[pos + 2],
                    bytes[pos + 3],
                    bytes[pos + 4],
                ]) as usize;
                Some(pos + 5 + len)
            }
            // array16
            0xdc if pos + 2 < bytes.len() => {
                let count = u16::from_be_bytes([bytes[pos + 1], bytes[pos + 2]]) as usize;
                let mut p = pos + 3;
                for _ in 0..count {
                    p = Self::skip_value(bytes, p)?;
                }
                Some(p)
            }
            // array32
            0xdd if pos + 4 < bytes.len() => {
                let count = u32::from_be_bytes([
                    bytes[pos + 1],
                    bytes[pos + 2],
                    bytes[pos + 3],
                    bytes[pos + 4],
                ]) as usize;
                let mut p = pos + 5;
                for _ in 0..count {
                    p = Self::skip_value(bytes, p)?;
                }
                Some(p)
            }
            // map16
            0xde if pos + 2 < bytes.len() => {
                let count = u16::from_be_bytes([bytes[pos + 1], bytes[pos + 2]]) as usize;
                let mut p = pos + 3;
                for _ in 0..count * 2 {
                    p = Self::skip_value(bytes, p)?;
                }
                Some(p)
            }
            // negative fixint
            0xe0..=0xff => Some(pos + 1),
            _ => None,
        }
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

    #[test]
    fn message_header_parse_data() {
        let mut batch = MinimalEventDataBatch::new(42, 1);
        batch.sequence_number = 12345;
        batch.push(MinimalEventData::new(0, 0, 100, 80, 1000.0, 0));

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
