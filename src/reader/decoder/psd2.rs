//! PSD2 Decoder for CAEN x27xx series digitizers
//!
//! Decodes 64-bit word format data from DPP-PSD firmware.

use super::common::{DataType, EventData, RawData, Waveform};

/// PSD2 constants (64-bit words, Little Endian)
mod constants {
    pub const WORD_SIZE: usize = 8;

    // Header
    pub const HEADER_TYPE_SHIFT: u32 = 60;
    pub const HEADER_TYPE_MASK: u64 = 0xF;
    pub const HEADER_TYPE_DATA: u64 = 0x2;
    pub const HEADER_FAIL_CHECK_SHIFT: u32 = 56;
    pub const HEADER_FAIL_CHECK_MASK: u64 = 0x1;
    pub const AGGREGATE_COUNTER_SHIFT: u32 = 32;
    pub const AGGREGATE_COUNTER_MASK: u64 = 0xFFFF;
    pub const TOTAL_SIZE_MASK: u64 = 0xFFFFFFFF;

    // Event first word
    pub const CHANNEL_SHIFT: u32 = 56;
    pub const CHANNEL_MASK: u64 = 0x7F;
    pub const TIMESTAMP_MASK: u64 = 0xFFFFFFFFFFFF;

    // Event second word
    pub const WAVEFORM_FLAG_SHIFT: u32 = 62;
    pub const FLAGS_LOW_PRIORITY_SHIFT: u32 = 50;
    pub const FLAGS_LOW_PRIORITY_MASK: u64 = 0x7FF;
    pub const FLAGS_HIGH_PRIORITY_SHIFT: u32 = 42;
    pub const FLAGS_HIGH_PRIORITY_MASK: u64 = 0xFF;
    pub const ENERGY_SHORT_SHIFT: u32 = 26;
    pub const ENERGY_SHORT_MASK: u64 = 0xFFFF;
    pub const FINE_TIME_SHIFT: u32 = 16;
    pub const FINE_TIME_MASK: u64 = 0x3FF;
    pub const FINE_TIME_SCALE: f64 = 1024.0;
    pub const ENERGY_MASK: u64 = 0xFFFF;

    // Waveform header
    pub const WAVEFORM_CHECK1_SHIFT: u32 = 63;
    pub const WAVEFORM_CHECK2_SHIFT: u32 = 60;
    pub const WAVEFORM_CHECK2_MASK: u64 = 0x7;
    pub const TIME_RESOLUTION_SHIFT: u32 = 44;
    pub const TIME_RESOLUTION_MASK: u64 = 0x3;
    pub const TRIGGER_THRESHOLD_SHIFT: u32 = 28;
    pub const TRIGGER_THRESHOLD_MASK: u64 = 0xFFFF;

    // Waveform size
    pub const WAVEFORM_WORDS_MASK: u64 = 0xFFF;

    // Waveform data decoding
    pub const ANALOG_PROBE_MASK: u32 = 0x3FFF;
    pub const ANALOG_PROBE2_SHIFT: u32 = 16;
    pub const DIGITAL_PROBE1_SHIFT: u32 = 14;
    pub const DIGITAL_PROBE2_SHIFT: u32 = 15;
    pub const DIGITAL_PROBE3_SHIFT: u32 = 30;
    pub const DIGITAL_PROBE4_SHIFT: u32 = 31;

    // Start/Stop signals
    pub const SIGNAL_TYPE_SHIFT: u32 = 60;
    pub const SIGNAL_SUBTYPE_SHIFT: u32 = 56;
    pub const SIGNAL_TYPE_MASK: u64 = 0xF;
    pub const START_SIGNAL_TYPE: u64 = 0x3;
    pub const START_SIGNAL_SUBTYPE: u64 = 0x0;
    pub const STOP_SIGNAL_TYPE: u64 = 0x3;
    pub const STOP_SIGNAL_SUBTYPE: u64 = 0x2;

    // Validation
    pub const MIN_DATA_SIZE: usize = 3 * WORD_SIZE;
    pub const START_SIGNAL_SIZE: usize = 4 * WORD_SIZE;
    pub const STOP_SIGNAL_SIZE: usize = 3 * WORD_SIZE;
}

/// PSD2 Decoder configuration
#[derive(Debug, Clone)]
pub struct Psd2Config {
    /// Time step in nanoseconds (typically 2ns for 500 MS/s)
    pub time_step_ns: f64,
    /// Module ID for identification
    pub module_id: u8,
    /// Enable dump output for debugging
    pub dump_enabled: bool,
}

impl Default for Psd2Config {
    fn default() -> Self {
        Self {
            time_step_ns: 2.0, // 500 MS/s -> 2ns per sample
            module_id: 0,
            dump_enabled: false,
        }
    }
}

/// PSD2 Decoder for x27xx series digitizers
#[derive(Debug, Clone)]
pub struct Psd2Decoder {
    config: Psd2Config,
    last_aggregate_counter: u16,
}

impl Psd2Decoder {
    /// Create a new PSD2 decoder with given configuration
    pub fn new(config: Psd2Config) -> Self {
        Self {
            config,
            last_aggregate_counter: 0,
        }
    }

    /// Create a decoder with default configuration
    pub fn with_defaults() -> Self {
        Self::new(Psd2Config::default())
    }

    /// Enable or disable dump output
    pub fn set_dump_enabled(&mut self, enabled: bool) {
        self.config.dump_enabled = enabled;
    }

    /// Classify the data type (Start/Stop/Event/Unknown)
    pub fn classify(&self, raw: &RawData) -> DataType {
        if raw.size < constants::MIN_DATA_SIZE {
            return DataType::Unknown;
        }

        // Check for stop signal (3 words)
        if raw.size == constants::STOP_SIGNAL_SIZE && self.is_stop_signal(&raw.data) {
            return DataType::Stop;
        }

        // Check for start signal (4 words)
        if raw.size == constants::START_SIGNAL_SIZE && self.is_start_signal(&raw.data) {
            return DataType::Start;
        }

        DataType::Event
    }

    /// Decode raw data into events
    pub fn decode(&mut self, raw: &RawData) -> Vec<EventData> {
        if self.config.dump_enabled {
            self.dump_raw_data(raw);
        }

        // Check data type first
        let data_type = self.classify(raw);
        match data_type {
            DataType::Start => {
                if self.config.dump_enabled {
                    println!("[PSD2] Start signal detected");
                }
                return vec![];
            }
            DataType::Stop => {
                if self.config.dump_enabled {
                    println!("[PSD2] Stop signal detected");
                }
                return vec![];
            }
            DataType::Unknown => {
                if self.config.dump_enabled {
                    println!("[PSD2] Unknown data type, size={}", raw.size);
                }
                return vec![];
            }
            DataType::Event => {}
        }

        // Read header
        let header = self.read_u64(&raw.data, 0);
        if !self.validate_header(header, raw.size) {
            return vec![];
        }

        let total_size = (header & constants::TOTAL_SIZE_MASK) as usize;
        let mut events = Vec::with_capacity(total_size / 2);
        let mut word_index = 1; // Skip header

        while word_index < total_size {
            if let Some(event) = self.decode_event(&raw.data, &mut word_index) {
                events.push(event);
            } else {
                // Failed to decode event, skip remaining
                break;
            }
        }

        // Sort by timestamp
        events.sort_by(|a, b| a.timestamp_ns.partial_cmp(&b.timestamp_ns).unwrap());

        if self.config.dump_enabled {
            println!("[PSD2] Decoded {} events", events.len());
        }

        events
    }

    /// Dump raw data for debugging
    pub fn dump_raw_data(&self, raw: &RawData) {
        println!("=== PSD2 Raw Data Dump ===");
        println!("Size: {} bytes ({} words)", raw.size, raw.size / 8);
        println!("N_Events (from HW): {}", raw.n_events);
        println!();

        let num_words = raw.size / constants::WORD_SIZE;
        for i in 0..num_words.min(20) {
            // Limit to first 20 words
            let word = self.read_u64(&raw.data, i);
            println!(
                "Word {:3}: 0x{:016x} | {:064b}",
                i, word, word
            );

            // Decode header if first word
            if i == 0 {
                self.dump_header(word);
            }
        }

        if num_words > 20 {
            println!("... ({} more words)", num_words - 20);
        }
        println!("=== End Dump ===");
        println!();
    }

    /// Dump header details
    fn dump_header(&self, header: u64) {
        let header_type = (header >> constants::HEADER_TYPE_SHIFT) & constants::HEADER_TYPE_MASK;
        let fail_check =
            (header >> constants::HEADER_FAIL_CHECK_SHIFT) & constants::HEADER_FAIL_CHECK_MASK;
        let aggregate_counter =
            (header >> constants::AGGREGATE_COUNTER_SHIFT) & constants::AGGREGATE_COUNTER_MASK;
        let total_size = header & constants::TOTAL_SIZE_MASK;

        println!("  Header type:        0x{:x}", header_type);
        println!("  Fail check:         {}", fail_check);
        println!("  Aggregate counter:  {}", aggregate_counter);
        println!("  Total size (words): {}", total_size);
    }

    /// Validate data header
    fn validate_header(&mut self, header: u64, data_size: usize) -> bool {
        let header_type = (header >> constants::HEADER_TYPE_SHIFT) & constants::HEADER_TYPE_MASK;
        if header_type != constants::HEADER_TYPE_DATA {
            if self.config.dump_enabled {
                println!(
                    "[PSD2] Invalid header type: 0x{:x} (expected 0x{:x})",
                    header_type,
                    constants::HEADER_TYPE_DATA
                );
            }
            return false;
        }

        let fail_check =
            (header >> constants::HEADER_FAIL_CHECK_SHIFT) & constants::HEADER_FAIL_CHECK_MASK;
        if fail_check != 0 && self.config.dump_enabled {
            println!("[PSD2] Board fail bit set!");
        }

        let aggregate_counter =
            ((header >> constants::AGGREGATE_COUNTER_SHIFT) & constants::AGGREGATE_COUNTER_MASK)
                as u16;

        // Check counter continuity (only warn, don't fail)
        if aggregate_counter != 0
            && aggregate_counter != self.last_aggregate_counter.wrapping_add(1)
            && self.config.dump_enabled
        {
            println!(
                "[PSD2] Aggregate counter discontinuity: {} -> {}",
                self.last_aggregate_counter, aggregate_counter
            );
        }
        self.last_aggregate_counter = aggregate_counter;

        let total_size = (header & constants::TOTAL_SIZE_MASK) as usize;
        if total_size * constants::WORD_SIZE != data_size && self.config.dump_enabled {
            println!(
                "[PSD2] Size mismatch: header={} bytes, actual={} bytes",
                total_size * constants::WORD_SIZE,
                data_size
            );
            // Continue anyway, use actual data size
        }

        true
    }

    /// Decode a single event (2 words + optional waveform)
    fn decode_event(&self, data: &[u8], word_index: &mut usize) -> Option<EventData> {
        // Check bounds for at least 2 words
        if *word_index + 2 > data.len() / constants::WORD_SIZE {
            return None;
        }

        // Read first word (channel and timestamp)
        let first_word = self.read_u64(data, *word_index);
        *word_index += 1;

        // Read second word (flags and energy)
        let second_word = self.read_u64(data, *word_index);
        *word_index += 1;

        // Extract channel
        let channel = ((first_word >> constants::CHANNEL_SHIFT) & constants::CHANNEL_MASK) as u8;

        // Extract raw timestamp
        let raw_timestamp = first_word & constants::TIMESTAMP_MASK;

        // Extract flags
        let flags_low =
            (second_word >> constants::FLAGS_LOW_PRIORITY_SHIFT) & constants::FLAGS_LOW_PRIORITY_MASK;
        let flags_high =
            (second_word >> constants::FLAGS_HIGH_PRIORITY_SHIFT) & constants::FLAGS_HIGH_PRIORITY_MASK;
        let flags = ((flags_high << 11) | flags_low) as u32;

        // Extract energies
        let energy = (second_word & constants::ENERGY_MASK) as u16;
        let energy_short =
            ((second_word >> constants::ENERGY_SHORT_SHIFT) & constants::ENERGY_SHORT_MASK) as u16;

        // Extract fine time and calculate precise timestamp
        let fine_time =
            ((second_word >> constants::FINE_TIME_SHIFT) & constants::FINE_TIME_MASK) as u16;
        let coarse_time_ns = (raw_timestamp as f64) * self.config.time_step_ns;
        let fine_time_ns =
            (fine_time as f64 / constants::FINE_TIME_SCALE) * self.config.time_step_ns;
        let timestamp_ns = coarse_time_ns + fine_time_ns;

        // Check for waveform
        let has_waveform = ((second_word >> constants::WAVEFORM_FLAG_SHIFT) & 0x1) != 0;
        let waveform = if has_waveform {
            self.decode_waveform(data, word_index)
        } else {
            None
        };

        if self.config.dump_enabled {
            println!("--- Event ---");
            println!("  Channel:      {}", channel);
            println!("  Timestamp:    {:.3} ns", timestamp_ns);
            println!("  Energy:       {}", energy);
            println!("  Energy Short: {}", energy_short);
            println!("  Fine Time:    {}", fine_time);
            println!("  Flags:        0x{:05x}", flags);
            println!("  Has Waveform: {}", has_waveform);
            if let Some(ref wf) = waveform {
                println!("  Waveform samples: {}", wf.analog_probe1.len());
            }
        }

        Some(EventData {
            timestamp_ns,
            module: self.config.module_id,
            channel,
            energy,
            energy_short,
            fine_time,
            flags,
            waveform,
        })
    }

    /// Decode waveform data
    fn decode_waveform(&self, data: &[u8], word_index: &mut usize) -> Option<Waveform> {
        // Need at least 2 words for waveform header + size
        if *word_index + 2 > data.len() / constants::WORD_SIZE {
            return None;
        }

        // Read waveform header
        let wf_header = self.read_u64(data, *word_index);
        *word_index += 1;

        // Validate waveform header
        let check1 = (wf_header >> constants::WAVEFORM_CHECK1_SHIFT) & 0x1;
        let check2 = (wf_header >> constants::WAVEFORM_CHECK2_SHIFT) & constants::WAVEFORM_CHECK2_MASK;
        if check1 != 1 || check2 != 0 {
            if self.config.dump_enabled {
                println!(
                    "[PSD2] Invalid waveform header: check1={}, check2={}",
                    check1, check2
                );
            }
            return None;
        }

        let time_resolution =
            ((wf_header >> constants::TIME_RESOLUTION_SHIFT) & constants::TIME_RESOLUTION_MASK) as u8;
        let trigger_threshold =
            ((wf_header >> constants::TRIGGER_THRESHOLD_SHIFT) & constants::TRIGGER_THRESHOLD_MASK) as u16;

        // Read waveform size word
        let size_word = self.read_u64(data, *word_index);
        *word_index += 1;

        let n_waveform_words = (size_word & constants::WAVEFORM_WORDS_MASK) as usize;
        let n_samples = n_waveform_words * 2; // 2 samples per word

        // Check bounds
        if *word_index + n_waveform_words > data.len() / constants::WORD_SIZE {
            if self.config.dump_enabled {
                println!(
                    "[PSD2] Not enough data for waveform: need {} words, have {}",
                    n_waveform_words,
                    data.len() / constants::WORD_SIZE - *word_index
                );
            }
            return None;
        }

        // Allocate waveform vectors
        let mut analog_probe1 = Vec::with_capacity(n_samples);
        let mut analog_probe2 = Vec::with_capacity(n_samples);
        let mut digital_probe1 = Vec::with_capacity(n_samples);
        let mut digital_probe2 = Vec::with_capacity(n_samples);
        let mut digital_probe3 = Vec::with_capacity(n_samples);
        let mut digital_probe4 = Vec::with_capacity(n_samples);

        // Decode waveform data
        for _ in 0..n_waveform_words {
            let word = self.read_u64(data, *word_index);
            *word_index += 1;

            // Each word contains 2 samples (low 32 bits, high 32 bits)
            for shift in [0u32, 32u32] {
                let sample = ((word >> shift) & 0xFFFFFFFF) as u32;

                let ap1 = (sample & constants::ANALOG_PROBE_MASK) as i16;
                let ap2 = ((sample >> constants::ANALOG_PROBE2_SHIFT) & constants::ANALOG_PROBE_MASK) as i16;
                let dp1 = ((sample >> constants::DIGITAL_PROBE1_SHIFT) & 0x1) as u8;
                let dp2 = ((sample >> constants::DIGITAL_PROBE2_SHIFT) & 0x1) as u8;
                let dp3 = ((sample >> constants::DIGITAL_PROBE3_SHIFT) & 0x1) as u8;
                let dp4 = ((sample >> constants::DIGITAL_PROBE4_SHIFT) & 0x1) as u8;

                analog_probe1.push(ap1);
                analog_probe2.push(ap2);
                digital_probe1.push(dp1);
                digital_probe2.push(dp2);
                digital_probe3.push(dp3);
                digital_probe4.push(dp4);
            }
        }

        Some(Waveform {
            analog_probe1,
            analog_probe2,
            digital_probe1,
            digital_probe2,
            digital_probe3,
            digital_probe4,
            time_resolution,
            trigger_threshold,
        })
    }

    /// Check if data is a start signal
    fn is_start_signal(&self, data: &[u8]) -> bool {
        if data.len() < constants::START_SIGNAL_SIZE {
            return false;
        }

        let first_word = self.read_u64(data, 0);
        let signal_type =
            (first_word >> constants::SIGNAL_TYPE_SHIFT) & constants::SIGNAL_TYPE_MASK;
        let signal_subtype =
            (first_word >> constants::SIGNAL_SUBTYPE_SHIFT) & constants::SIGNAL_TYPE_MASK;

        signal_type == constants::START_SIGNAL_TYPE
            && signal_subtype == constants::START_SIGNAL_SUBTYPE
    }

    /// Check if data is a stop signal
    fn is_stop_signal(&self, data: &[u8]) -> bool {
        if data.len() < constants::STOP_SIGNAL_SIZE {
            return false;
        }

        let first_word = self.read_u64(data, 0);
        let signal_type =
            (first_word >> constants::SIGNAL_TYPE_SHIFT) & constants::SIGNAL_TYPE_MASK;
        let signal_subtype =
            (first_word >> constants::SIGNAL_SUBTYPE_SHIFT) & constants::SIGNAL_TYPE_MASK;

        signal_type == constants::STOP_SIGNAL_TYPE
            && signal_subtype == constants::STOP_SIGNAL_SUBTYPE
    }

    /// Read a u64 from data at given word index
    ///
    /// **Important**: VX2730 (x27xx series) RAW data is in Big Endian format.
    /// Each 64-bit word needs to be byte-swapped for correct interpretation.
    #[inline]
    fn read_u64(&self, data: &[u8], word_index: usize) -> u64 {
        let offset = word_index * constants::WORD_SIZE;
        // Data from VX2730 is Big Endian
        u64::from_be_bytes(
            data[offset..offset + constants::WORD_SIZE]
                .try_into()
                .unwrap(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decoder_creation() {
        let decoder = Psd2Decoder::with_defaults();
        assert_eq!(decoder.config.time_step_ns, 2.0);
        assert_eq!(decoder.config.module_id, 0);
    }

    #[test]
    fn test_classify_small_data() {
        let decoder = Psd2Decoder::with_defaults();
        let raw = RawData {
            data: vec![0; 16], // Too small
            size: 16,
            n_events: 0,
        };
        assert_eq!(decoder.classify(&raw), DataType::Unknown);
    }
}
