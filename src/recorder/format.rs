//! File format structures for DELILA data files
//!
//! File structure:
//! ```text
//! ┌─────────────────────────────────────────┐
//! │  Header (length-prefixed MsgPack)        │
//! │  - Magic, Version, Metadata             │
//! ├─────────────────────────────────────────┤
//! │  Data Block 1                           │
//! │  - Length prefix (u32 LE)               │
//! │  - MsgPack serialized batch             │
//! ├─────────────────────────────────────────┤
//! │  ...                                    │
//! ├─────────────────────────────────────────┤
//! │  Footer (fixed 64 bytes)                │
//! │  - Magic, checksums, completion flag    │
//! └─────────────────────────────────────────┘
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use xxhash_rust::xxh64::xxh64;

/// Magic bytes for DELILA data files
pub const FILE_MAGIC: [u8; 8] = *b"DELILA02";

/// Current file format version
pub const FORMAT_VERSION: u32 = 2;

/// Footer magic bytes (different from header to detect truncation)
pub const FOOTER_MAGIC: [u8; 8] = *b"DLEND002";

/// Fixed footer size in bytes
pub const FOOTER_SIZE: usize = 64;

/// File header containing metadata about the run and file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHeader {
    /// Format version
    pub version: u32,

    /// Run number
    pub run_number: u32,

    /// Experiment name
    pub exp_name: String,

    /// File sequence number within run (0, 1, 2, ...)
    pub file_sequence: u32,

    /// File creation time (Unix timestamp in nanoseconds)
    pub file_start_time_ns: u64,

    /// Run comment
    pub comment: String,

    /// Sorting configuration
    pub sort_margin_ratio: f64,

    /// Whether data is timestamp-sorted
    pub is_sorted: bool,

    /// Source IDs that contributed to this file
    pub source_ids: Vec<u32>,

    /// Additional key-value metadata
    pub metadata: HashMap<String, String>,
}

impl FileHeader {
    /// Create a new header with required fields
    pub fn new(run_number: u32, exp_name: String, file_sequence: u32) -> Self {
        Self {
            version: FORMAT_VERSION,
            run_number,
            exp_name,
            file_sequence,
            file_start_time_ns: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
            comment: String::new(),
            sort_margin_ratio: 0.0, // Raw data recorder: unsorted
            is_sorted: false,
            source_ids: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    /// Serialize header to bytes (with magic prefix)
    pub fn to_bytes(&self) -> Result<Vec<u8>, rmp_serde::encode::Error> {
        let mut buf = Vec::with_capacity(256);
        buf.extend_from_slice(&FILE_MAGIC);
        let header_bytes = rmp_serde::to_vec(self)?;
        let len = header_bytes.len() as u32;
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(&header_bytes);
        Ok(buf)
    }

    /// Deserialize header from bytes (expects magic prefix)
    pub fn from_bytes(data: &[u8]) -> Result<Self, FileFormatError> {
        if data.len() < 12 {
            return Err(FileFormatError::TooShort);
        }

        // Check magic
        if data[0..8] != FILE_MAGIC {
            return Err(FileFormatError::InvalidMagic);
        }

        // Read length
        let len = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;

        if data.len() < 12 + len {
            return Err(FileFormatError::TooShort);
        }

        rmp_serde::from_slice(&data[12..12 + len]).map_err(FileFormatError::Deserialization)
    }

    /// Write header to a writer
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<usize, FileFormatError> {
        let bytes = self.to_bytes()?;
        writer.write_all(&bytes)?;
        Ok(bytes.len())
    }

    /// Read header from a reader
    pub fn read_from<R: Read>(reader: &mut R) -> Result<Self, FileFormatError> {
        // Read magic
        let mut magic = [0u8; 8];
        reader.read_exact(&mut magic)?;
        if magic != FILE_MAGIC {
            return Err(FileFormatError::InvalidMagic);
        }

        // Read length
        let mut len_bytes = [0u8; 4];
        reader.read_exact(&mut len_bytes)?;
        let len = u32::from_le_bytes(len_bytes) as usize;

        // Read header data
        let mut data = vec![0u8; len];
        reader.read_exact(&mut data)?;

        rmp_serde::from_slice(&data).map_err(FileFormatError::Deserialization)
    }
}

/// File footer containing checksums and completion status
///
/// Fixed 64-byte structure for easy seeking to file end.
#[derive(Debug, Clone, Copy)]
pub struct FileFooter {
    /// Footer magic bytes (8 bytes)
    pub magic: [u8; 8],

    /// xxHash64 of all data blocks (excluding header and footer)
    pub data_checksum: u64,

    /// Total number of events written
    pub total_events: u64,

    /// Total bytes of data blocks (excluding header and footer)
    pub data_bytes: u64,

    /// First event timestamp (ns) in this file
    pub first_event_time_ns: f64,

    /// Last event timestamp (ns) in this file
    pub last_event_time_ns: f64,

    /// File end time (Unix timestamp in nanoseconds)
    pub file_end_time_ns: u64,

    /// Write completion flag (1 = complete, 0 = incomplete/crashed)
    pub write_complete: u8,

    /// Reserved for future use
    _reserved: [u8; 7],
}

impl Default for FileFooter {
    fn default() -> Self {
        Self::new()
    }
}

impl FileFooter {
    /// Create a new empty footer
    pub fn new() -> Self {
        Self {
            magic: FOOTER_MAGIC,
            data_checksum: 0,
            total_events: 0,
            data_bytes: 0,
            first_event_time_ns: f64::MAX,
            last_event_time_ns: f64::MIN,
            file_end_time_ns: 0,
            write_complete: 0,
            _reserved: [0u8; 7],
        }
    }

    /// Mark as complete and set end time
    pub fn finalize(&mut self) {
        self.write_complete = 1;
        self.file_end_time_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
    }

    /// Check if file was completely written
    pub fn is_complete(&self) -> bool {
        self.write_complete == 1
    }

    /// Update timestamp range with new event timestamps
    pub fn update_timestamp_range(&mut self, first: f64, last: f64) {
        if first < self.first_event_time_ns {
            self.first_event_time_ns = first;
        }
        if last > self.last_event_time_ns {
            self.last_event_time_ns = last;
        }
    }

    /// Serialize footer to fixed 64-byte array
    pub fn to_bytes(&self) -> [u8; FOOTER_SIZE] {
        let mut buf = [0u8; FOOTER_SIZE];

        // Magic (8 bytes)
        buf[0..8].copy_from_slice(&self.magic);

        // Data checksum (8 bytes)
        buf[8..16].copy_from_slice(&self.data_checksum.to_le_bytes());

        // Total events (8 bytes)
        buf[16..24].copy_from_slice(&self.total_events.to_le_bytes());

        // Data bytes (8 bytes)
        buf[24..32].copy_from_slice(&self.data_bytes.to_le_bytes());

        // First event time (8 bytes)
        buf[32..40].copy_from_slice(&self.first_event_time_ns.to_le_bytes());

        // Last event time (8 bytes)
        buf[40..48].copy_from_slice(&self.last_event_time_ns.to_le_bytes());

        // File end time (8 bytes)
        buf[48..56].copy_from_slice(&self.file_end_time_ns.to_le_bytes());

        // Write complete flag (1 byte)
        buf[56] = self.write_complete;

        // Reserved (7 bytes) - already zeroed

        buf
    }

    /// Deserialize footer from 64-byte array
    pub fn from_bytes(data: &[u8; FOOTER_SIZE]) -> Result<Self, FileFormatError> {
        // Check magic
        let mut magic = [0u8; 8];
        magic.copy_from_slice(&data[0..8]);
        if magic != FOOTER_MAGIC {
            return Err(FileFormatError::InvalidFooterMagic);
        }

        Ok(Self {
            magic,
            data_checksum: u64::from_le_bytes([
                data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
            ]),
            total_events: u64::from_le_bytes([
                data[16], data[17], data[18], data[19], data[20], data[21], data[22], data[23],
            ]),
            data_bytes: u64::from_le_bytes([
                data[24], data[25], data[26], data[27], data[28], data[29], data[30], data[31],
            ]),
            first_event_time_ns: f64::from_le_bytes([
                data[32], data[33], data[34], data[35], data[36], data[37], data[38], data[39],
            ]),
            last_event_time_ns: f64::from_le_bytes([
                data[40], data[41], data[42], data[43], data[44], data[45], data[46], data[47],
            ]),
            file_end_time_ns: u64::from_le_bytes([
                data[48], data[49], data[50], data[51], data[52], data[53], data[54], data[55],
            ]),
            write_complete: data[56],
            _reserved: [0u8; 7],
        })
    }

    /// Write footer to a writer
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<(), FileFormatError> {
        let bytes = self.to_bytes();
        writer.write_all(&bytes)?;
        Ok(())
    }

    /// Read footer from a reader
    pub fn read_from<R: Read>(reader: &mut R) -> Result<Self, FileFormatError> {
        let mut buf = [0u8; FOOTER_SIZE];
        reader.read_exact(&mut buf)?;
        Self::from_bytes(&buf)
    }
}

/// Incremental checksum calculator using xxHash64
#[derive(Debug, Clone)]
pub struct ChecksumCalculator {
    state: u64,
    bytes_processed: u64,
}

impl Default for ChecksumCalculator {
    fn default() -> Self {
        Self::new()
    }
}

impl ChecksumCalculator {
    /// Create a new checksum calculator
    pub fn new() -> Self {
        Self {
            state: 0,
            bytes_processed: 0,
        }
    }

    /// Update checksum with new data
    ///
    /// Note: xxHash64 doesn't support streaming natively, so we combine
    /// block hashes. For large files, consider using xxh3 streaming API.
    pub fn update(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        // Compute hash of this block
        let block_hash = xxh64(data, 0);

        // Combine with previous state using XOR and rotation
        // This is a simple combination; for stronger guarantees, use xxh3 streaming
        self.state = self.state.rotate_left(5) ^ block_hash;
        self.bytes_processed += data.len() as u64;
    }

    /// Get the final checksum
    pub fn finalize(&self) -> u64 {
        // Final mixing with total bytes for added entropy
        self.state ^ self.bytes_processed
    }

    /// Get bytes processed so far
    pub fn bytes_processed(&self) -> u64 {
        self.bytes_processed
    }

    /// Reset calculator
    pub fn reset(&mut self) {
        self.state = 0;
        self.bytes_processed = 0;
    }
}

/// File format errors
#[derive(Debug, thiserror::Error)]
pub enum FileFormatError {
    #[error("Data too short to contain valid structure")]
    TooShort,

    #[error("Invalid file magic bytes")]
    InvalidMagic,

    #[error("Invalid footer magic bytes")]
    InvalidFooterMagic,

    #[error("Deserialization error: {0}")]
    Deserialization(#[from] rmp_serde::decode::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] rmp_serde::encode::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Checksum mismatch: expected {expected:016x}, got {actual:016x}")]
    ChecksumMismatch { expected: u64, actual: u64 },

    #[error("Incomplete file (footer indicates crash during write)")]
    IncompleteFile,
}

/// Result of file validation
#[derive(Debug)]
pub struct FileValidationResult {
    /// Whether the file is valid
    pub is_valid: bool,
    /// Header information (if readable)
    pub header: Option<FileHeader>,
    /// Footer information (if readable)
    pub footer: Option<FileFooter>,
    /// Number of recoverable data blocks
    pub recoverable_blocks: usize,
    /// Total recoverable events
    pub recoverable_events: u64,
    /// Validation errors encountered
    pub errors: Vec<String>,
}

impl FileValidationResult {
    /// Check if file needs recovery (has data but incomplete)
    pub fn needs_recovery(&self) -> bool {
        !self.is_valid && self.recoverable_blocks > 0
    }
}

/// Reader for DELILA data files with recovery support
pub struct DataFileReader<R> {
    reader: R,
    header: Option<FileHeader>,
    footer: Option<FileFooter>,
    header_size: usize,
    file_size: u64,
}

impl<R: std::io::Read + std::io::Seek> DataFileReader<R> {
    /// Open a data file for reading
    pub fn new(mut reader: R) -> Result<Self, FileFormatError> {
        // Get file size
        let file_size = reader.seek(std::io::SeekFrom::End(0))?;
        reader.seek(std::io::SeekFrom::Start(0))?;

        let mut this = Self {
            reader,
            header: None,
            footer: None,
            header_size: 0,
            file_size,
        };

        // Try to read header
        this.read_header()?;

        Ok(this)
    }

    /// Read and validate the file header
    fn read_header(&mut self) -> Result<(), FileFormatError> {
        self.reader.seek(std::io::SeekFrom::Start(0))?;
        let header = FileHeader::read_from(&mut self.reader)?;

        // Calculate header size (magic + length prefix + msgpack data)
        let pos = self.reader.stream_position()?;
        self.header_size = pos as usize;
        self.header = Some(header);
        Ok(())
    }

    /// Try to read the footer (may fail for incomplete files)
    pub fn read_footer(&mut self) -> Result<FileFooter, FileFormatError> {
        if self.file_size < FOOTER_SIZE as u64 {
            return Err(FileFormatError::TooShort);
        }

        self.reader
            .seek(std::io::SeekFrom::End(-(FOOTER_SIZE as i64)))?;
        let footer = FileFooter::read_from(&mut self.reader)?;
        self.footer = Some(footer);
        Ok(footer)
    }

    /// Get the header
    pub fn header(&self) -> Option<&FileHeader> {
        self.header.as_ref()
    }

    /// Get the footer (if read)
    pub fn footer(&self) -> Option<&FileFooter> {
        self.footer.as_ref()
    }

    /// Validate file integrity
    pub fn validate(&mut self) -> FileValidationResult {
        let mut result = FileValidationResult {
            is_valid: false,
            header: self.header.clone(),
            footer: None,
            recoverable_blocks: 0,
            recoverable_events: 0,
            errors: Vec::new(),
        };

        // Check header
        if self.header.is_none() {
            result.errors.push("Missing or invalid header".to_string());
            return result;
        }

        // Try to read footer
        match self.read_footer() {
            Ok(footer) => {
                result.footer = Some(footer);
                if !footer.is_complete() {
                    result
                        .errors
                        .push("File incomplete (crash during write)".to_string());
                }
            }
            Err(e) => {
                result.errors.push(format!("Failed to read footer: {}", e));
            }
        }

        // Count recoverable blocks
        let (blocks, events) = self.count_recoverable_blocks();
        result.recoverable_blocks = blocks;
        result.recoverable_events = events;

        // File is valid if footer exists, is complete, and checksum matches
        if let Some(ref footer) = result.footer {
            if footer.is_complete() {
                // Verify checksum
                match self.verify_checksum() {
                    Ok(true) => {
                        result.is_valid = true;
                    }
                    Ok(false) => {
                        result.errors.push("Checksum mismatch".to_string());
                    }
                    Err(e) => {
                        result.errors.push(format!("Checksum verification error: {}", e));
                    }
                }
            }
        }

        result
    }

    /// Count recoverable data blocks
    fn count_recoverable_blocks(&mut self) -> (usize, u64) {
        let mut blocks = 0;
        let mut events = 0u64;

        // Position after header
        if self.reader.seek(std::io::SeekFrom::Start(self.header_size as u64)).is_err() {
            return (0, 0);
        }

        // Data region ends at file_size - FOOTER_SIZE (if footer might exist)
        let data_end = if self.file_size >= FOOTER_SIZE as u64 {
            self.file_size - FOOTER_SIZE as u64
        } else {
            self.file_size
        };

        loop {
            let pos = match self.reader.stream_position() {
                Ok(p) => p,
                Err(_) => break,
            };

            if pos >= data_end {
                break;
            }

            // Try to read length prefix
            let mut len_bytes = [0u8; 4];
            if self.reader.read_exact(&mut len_bytes).is_err() {
                break;
            }

            let len = u32::from_le_bytes(len_bytes) as usize;

            // Sanity check on length
            if len == 0 || len > 100_000_000 {
                // Max 100MB per block
                break;
            }

            // Check if we have enough data
            if pos + 4 + len as u64 > data_end {
                break;
            }

            // Try to read and parse the block
            let mut data = vec![0u8; len];
            if self.reader.read_exact(&mut data).is_err() {
                break;
            }

            // Try to deserialize to count events
            match crate::common::EventDataBatch::from_msgpack(&data) {
                Ok(batch) => {
                    events += batch.events.len() as u64;
                    blocks += 1;
                }
                Err(_) => {
                    // Block is corrupted, stop here
                    break;
                }
            }
        }

        (blocks, events)
    }

    /// Verify data checksum
    fn verify_checksum(&mut self) -> Result<bool, FileFormatError> {
        let footer = self.footer.as_ref().ok_or(FileFormatError::TooShort)?;

        // Position after header
        self.reader.seek(std::io::SeekFrom::Start(self.header_size as u64))?;

        let mut calc = ChecksumCalculator::new();

        // Read all data blocks and update checksum
        let data_end = self.file_size - FOOTER_SIZE as u64;

        loop {
            let pos = self.reader.stream_position()?;
            if pos >= data_end {
                break;
            }

            // Read length prefix
            let mut len_bytes = [0u8; 4];
            if self.reader.read_exact(&mut len_bytes).is_err() {
                break;
            }

            let len = u32::from_le_bytes(len_bytes) as usize;
            if len == 0 || len > 100_000_000 {
                break;
            }

            // Read data
            let mut data = vec![0u8; len];
            if self.reader.read_exact(&mut data).is_err() {
                break;
            }

            // Update checksum
            calc.update(&len_bytes);
            calc.update(&data);
        }

        let computed = calc.finalize();
        Ok(computed == footer.data_checksum)
    }

    /// Iterator over data blocks (for recovery)
    pub fn data_blocks(&mut self) -> DataBlockIterator<'_, R> {
        // Position after header
        let _ = self.reader.seek(std::io::SeekFrom::Start(self.header_size as u64));

        let data_end = if self.file_size >= FOOTER_SIZE as u64 {
            self.file_size - FOOTER_SIZE as u64
        } else {
            self.file_size
        };

        DataBlockIterator {
            reader: &mut self.reader,
            data_end,
            done: false,
        }
    }
}

/// Iterator over data blocks in a file
pub struct DataBlockIterator<'a, R> {
    reader: &'a mut R,
    data_end: u64,
    done: bool,
}

impl<'a, R: std::io::Read + std::io::Seek> Iterator for DataBlockIterator<'a, R> {
    type Item = Result<crate::common::EventDataBatch, FileFormatError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        let pos = match self.reader.stream_position() {
            Ok(p) => p,
            Err(e) => {
                self.done = true;
                return Some(Err(FileFormatError::Io(e)));
            }
        };

        if pos >= self.data_end {
            self.done = true;
            return None;
        }

        // Read length prefix
        let mut len_bytes = [0u8; 4];
        if let Err(e) = self.reader.read_exact(&mut len_bytes) {
            self.done = true;
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return None;
            }
            return Some(Err(FileFormatError::Io(e)));
        }

        let len = u32::from_le_bytes(len_bytes) as usize;
        if len == 0 || len > 100_000_000 {
            self.done = true;
            return None;
        }

        // Check bounds
        if pos + 4 + len as u64 > self.data_end {
            self.done = true;
            return None;
        }

        // Read data
        let mut data = vec![0u8; len];
        if let Err(e) = self.reader.read_exact(&mut data) {
            self.done = true;
            return Some(Err(FileFormatError::Io(e)));
        }

        // Deserialize
        match crate::common::EventDataBatch::from_msgpack(&data) {
            Ok(batch) => Some(Ok(batch)),
            Err(e) => {
                self.done = true;
                Some(Err(FileFormatError::Deserialization(e)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_header_roundtrip() {
        let mut header = FileHeader::new(42, "CRIB2026".to_string(), 5);
        header.comment = "Test run".to_string();
        header.source_ids = vec![0, 1, 2];
        header.metadata.insert("operator".to_string(), "Aogaki".to_string());

        let bytes = header.to_bytes().unwrap();
        let restored = FileHeader::from_bytes(&bytes).unwrap();

        assert_eq!(restored.version, FORMAT_VERSION);
        assert_eq!(restored.run_number, 42);
        assert_eq!(restored.exp_name, "CRIB2026");
        assert_eq!(restored.file_sequence, 5);
        assert_eq!(restored.comment, "Test run");
        assert_eq!(restored.source_ids, vec![0, 1, 2]);
        assert_eq!(restored.metadata.get("operator"), Some(&"Aogaki".to_string()));
    }

    #[test]
    fn test_file_header_magic() {
        let header = FileHeader::new(1, "test".to_string(), 0);
        let bytes = header.to_bytes().unwrap();

        // First 8 bytes should be magic
        assert_eq!(&bytes[0..8], &FILE_MAGIC);
    }

    #[test]
    fn test_file_header_invalid_magic() {
        let mut data = vec![0u8; 100];
        data[0..8].copy_from_slice(b"INVALID!");

        let result = FileHeader::from_bytes(&data);
        assert!(matches!(result, Err(FileFormatError::InvalidMagic)));
    }

    #[test]
    fn test_file_footer_size() {
        let footer = FileFooter::new();
        let bytes = footer.to_bytes();
        assert_eq!(bytes.len(), FOOTER_SIZE);
        assert_eq!(bytes.len(), 64);
    }

    #[test]
    fn test_file_footer_roundtrip() {
        let mut footer = FileFooter::new();
        footer.data_checksum = 0x123456789ABCDEF0;
        footer.total_events = 1_000_000;
        footer.data_bytes = 22_000_000;
        footer.first_event_time_ns = 1000.5;
        footer.last_event_time_ns = 999999.5;
        footer.finalize();

        let bytes = footer.to_bytes();
        let restored = FileFooter::from_bytes(&bytes).unwrap();

        assert_eq!(restored.data_checksum, footer.data_checksum);
        assert_eq!(restored.total_events, footer.total_events);
        assert_eq!(restored.data_bytes, footer.data_bytes);
        assert!((restored.first_event_time_ns - footer.first_event_time_ns).abs() < f64::EPSILON);
        assert!((restored.last_event_time_ns - footer.last_event_time_ns).abs() < f64::EPSILON);
        assert!(restored.is_complete());
    }

    #[test]
    fn test_file_footer_magic() {
        let footer = FileFooter::new();
        let bytes = footer.to_bytes();
        assert_eq!(&bytes[0..8], &FOOTER_MAGIC);
    }

    #[test]
    fn test_file_footer_invalid_magic() {
        let mut data = [0u8; FOOTER_SIZE];
        data[0..8].copy_from_slice(b"BADMAGIC");

        let result = FileFooter::from_bytes(&data);
        assert!(matches!(result, Err(FileFormatError::InvalidFooterMagic)));
    }

    #[test]
    fn test_file_footer_incomplete() {
        let footer = FileFooter::new();
        assert!(!footer.is_complete());

        let mut footer2 = FileFooter::new();
        footer2.finalize();
        assert!(footer2.is_complete());
    }

    #[test]
    fn test_checksum_calculator() {
        let mut calc = ChecksumCalculator::new();

        calc.update(b"Hello, ");
        calc.update(b"World!");

        let checksum1 = calc.finalize();
        assert_ne!(checksum1, 0);
        assert_eq!(calc.bytes_processed(), 13);

        // Different data should produce different checksum
        let mut calc2 = ChecksumCalculator::new();
        calc2.update(b"Different data");
        let checksum2 = calc2.finalize();

        assert_ne!(checksum1, checksum2);
    }

    #[test]
    fn test_checksum_calculator_empty() {
        let calc = ChecksumCalculator::new();
        assert_eq!(calc.finalize(), 0);
        assert_eq!(calc.bytes_processed(), 0);
    }

    #[test]
    fn test_checksum_calculator_reset() {
        let mut calc = ChecksumCalculator::new();
        calc.update(b"some data");
        assert!(calc.bytes_processed() > 0);

        calc.reset();
        assert_eq!(calc.bytes_processed(), 0);
        assert_eq!(calc.finalize(), 0);
    }

    #[test]
    fn test_timestamp_range_update() {
        let mut footer = FileFooter::new();

        // Initial state
        assert_eq!(footer.first_event_time_ns, f64::MAX);
        assert_eq!(footer.last_event_time_ns, f64::MIN);

        // Update with first batch
        footer.update_timestamp_range(1000.0, 2000.0);
        assert!((footer.first_event_time_ns - 1000.0).abs() < f64::EPSILON);
        assert!((footer.last_event_time_ns - 2000.0).abs() < f64::EPSILON);

        // Update with later batch
        footer.update_timestamp_range(1500.0, 3000.0);
        assert!((footer.first_event_time_ns - 1000.0).abs() < f64::EPSILON); // unchanged
        assert!((footer.last_event_time_ns - 3000.0).abs() < f64::EPSILON); // updated

        // Update with earlier batch
        footer.update_timestamp_range(500.0, 2500.0);
        assert!((footer.first_event_time_ns - 500.0).abs() < f64::EPSILON); // updated
        assert!((footer.last_event_time_ns - 3000.0).abs() < f64::EPSILON); // unchanged
    }

    #[test]
    fn test_header_write_read() {
        let header = FileHeader::new(123, "TestExp".to_string(), 7);

        let mut buf = Vec::new();
        header.write_to(&mut buf).unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let restored = FileHeader::read_from(&mut cursor).unwrap();

        assert_eq!(restored.run_number, 123);
        assert_eq!(restored.exp_name, "TestExp");
        assert_eq!(restored.file_sequence, 7);
    }

    #[test]
    fn test_footer_write_read() {
        let mut footer = FileFooter::new();
        footer.total_events = 500;
        footer.data_bytes = 11000;
        footer.finalize();

        let mut buf = Vec::new();
        footer.write_to(&mut buf).unwrap();
        assert_eq!(buf.len(), FOOTER_SIZE);

        let mut cursor = std::io::Cursor::new(buf);
        let restored = FileFooter::read_from(&mut cursor).unwrap();

        assert_eq!(restored.total_events, 500);
        assert_eq!(restored.data_bytes, 11000);
        assert!(restored.is_complete());
    }
}
