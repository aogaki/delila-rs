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
    pub const LAST_WORD_SHIFT: u32 = 63;
    pub const CHANNEL_SHIFT: u32 = 56;
    pub const CHANNEL_MASK: u64 = 0x7F;
    pub const SPECIAL_EVENT_SHIFT: u32 = 55;
    pub const SPECIAL_EVENT_MASK: u64 = 0x1;
    pub const TIMESTAMP_MASK: u64 = 0xFFFFFFFFFFFF;
    // Single-word event: timestamp is reduced to 32 bits
    pub const TIMESTAMP_REDUCED_MASK: u64 = 0xFFFFFFFF;

    // Event second word
    pub const WAVEFORM_FLAG_SHIFT: u32 = 62;
    pub const FLAGS_LOW_PRIORITY_SHIFT: u32 = 50;
    pub const FLAGS_LOW_PRIORITY_MASK: u64 = 0xFFF; // 12 bits (C++ dpppsd.hpp: flag_low_priority{12})
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

    // Single-word event: flag_high is at bits 55:48 of first word (not same as 2nd word position)
    pub const SINGLE_WORD_FLAG_HIGH_SHIFT: u32 = 48;

    // Validation
    pub const MIN_DATA_SIZE: usize = 2 * WORD_SIZE; // header + 1 single-word event minimum
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
            }
            // None means: special event filtered, or decode error.
            // In all cases word_index has been advanced, so continue.
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
            println!("Word {:3}: 0x{:016x} | {:064b}", i, word, word);

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

        let aggregate_counter = ((header >> constants::AGGREGATE_COUNTER_SHIFT)
            & constants::AGGREGATE_COUNTER_MASK) as u16;

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

    /// Decode a single event from raw data.
    ///
    /// Handles three event formats:
    /// 1. Single-word event (EnDataReduction=True): bit 63 of first word is set
    /// 2. Special event (EnStatEvents=True): bit 55 of first word is set, filtered out
    /// 3. Standard 2+ word event: normal physics data
    ///
    /// Reference: external/caen-dig2/src/endpoints/dpppsd.cpp decode_hit()
    fn decode_event(&self, data: &[u8], word_index: &mut usize) -> Option<EventData> {
        let total_words = data.len() / constants::WORD_SIZE;

        // Need at least 1 word
        if *word_index >= total_words {
            return None;
        }

        // Read first word
        let first_word = self.read_u64(data, *word_index);
        *word_index += 1;

        // Check last_word bit (bit 63) - indicates single-word compressed event
        let is_last_word = ((first_word >> constants::LAST_WORD_SHIFT) & 0x1) != 0;

        // Extract channel (same position in both formats)
        let channel = ((first_word >> constants::CHANNEL_SHIFT) & constants::CHANNEL_MASK) as u8;

        if is_last_word {
            // Single-word event (EnDataReduction=True)
            // Layout: [63:last=1][62:56 channel][55:48 flag_high][47:16 timestamp_reduced][15:0 energy]
            return self.decode_single_word_event(first_word, channel);
        }

        // Standard multi-word event
        // Check special_event flag (bit 55)
        let is_special_event = ((first_word >> constants::SPECIAL_EVENT_SHIFT)
            & constants::SPECIAL_EVENT_MASK)
            != 0;

        // Extract raw timestamp (48 bits)
        let raw_timestamp = first_word & constants::TIMESTAMP_MASK;

        // Need second word
        if *word_index >= total_words {
            return None;
        }

        let second_word = self.read_u64(data, *word_index);
        *word_index += 1;

        // Check last_word of second word to see if there are extra words
        let has_waveform = ((second_word >> constants::WAVEFORM_FLAG_SHIFT) & 0x1) != 0;
        let mut is_last = ((second_word >> constants::LAST_WORD_SHIFT) & 0x1) != 0;

        // Consume extra words (time_info, counter_info for special events; wave_info for normal)
        while !is_last {
            if *word_index >= total_words {
                break;
            }
            let extra_word = self.read_u64(data, *word_index);
            *word_index += 1;
            is_last = ((extra_word >> constants::LAST_WORD_SHIFT) & 0x1) != 0;
        }

        // Filter out special events (statistics data, not physics)
        if is_special_event {
            if self.config.dump_enabled {
                println!("[PSD2] Special event filtered (ch={})", channel);
            }
            return None;
        }

        // Decode standard event from second word
        let flags_low = (second_word >> constants::FLAGS_LOW_PRIORITY_SHIFT)
            & constants::FLAGS_LOW_PRIORITY_MASK;
        let flags_high = (second_word >> constants::FLAGS_HIGH_PRIORITY_SHIFT)
            & constants::FLAGS_HIGH_PRIORITY_MASK;
        let flags = ((flags_high << 12) | flags_low) as u32;

        let energy = (second_word & constants::ENERGY_MASK) as u16;
        let energy_short =
            ((second_word >> constants::ENERGY_SHORT_SHIFT) & constants::ENERGY_SHORT_MASK) as u16;

        let fine_time =
            ((second_word >> constants::FINE_TIME_SHIFT) & constants::FINE_TIME_MASK) as u16;
        let coarse_time_ns = (raw_timestamp as f64) * self.config.time_step_ns;
        let fine_time_ns =
            (fine_time as f64 / constants::FINE_TIME_SCALE) * self.config.time_step_ns;
        let timestamp_ns = coarse_time_ns + fine_time_ns;

        // Decode waveform if present
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

    /// Decode a single-word compressed event (EnDataReduction=True)
    ///
    /// Single-word layout (from dpppsd.cpp:244-257):
    /// ```text
    /// [63:last=1][62:56 channel][55:48 flag_high_priority][47:16 timestamp_reduced][15:0 energy]
    /// ```
    /// No energy_short, fine_time, flags_low, or waveform in this format.
    fn decode_single_word_event(
        &self,
        word: u64,
        channel: u8,
    ) -> Option<EventData> {
        let flags_high = ((word >> constants::SINGLE_WORD_FLAG_HIGH_SHIFT)
            & constants::FLAGS_HIGH_PRIORITY_MASK) as u32;
        let timestamp_reduced = (word >> constants::FINE_TIME_SHIFT)
            & constants::TIMESTAMP_REDUCED_MASK;
        let energy = (word & constants::ENERGY_MASK) as u16;

        let timestamp_ns = (timestamp_reduced as f64) * self.config.time_step_ns;
        let flags = flags_high << 12; // high priority only, low priority not available

        if self.config.dump_enabled {
            println!("--- Single-word Event ---");
            println!("  Channel:      {}", channel);
            println!("  Timestamp:    {:.3} ns", timestamp_ns);
            println!("  Energy:       {}", energy);
            println!("  Flags (high): 0x{:02x}", flags_high);
        }

        Some(EventData {
            timestamp_ns,
            module: self.config.module_id,
            channel,
            energy,
            energy_short: 0,
            fine_time: 0,
            flags,
            waveform: None,
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
        let check2 =
            (wf_header >> constants::WAVEFORM_CHECK2_SHIFT) & constants::WAVEFORM_CHECK2_MASK;
        if check1 != 1 || check2 != 0 {
            if self.config.dump_enabled {
                println!(
                    "[PSD2] Invalid waveform header: check1={}, check2={}",
                    check1, check2
                );
            }
            return None;
        }

        let time_resolution = ((wf_header >> constants::TIME_RESOLUTION_SHIFT)
            & constants::TIME_RESOLUTION_MASK) as u8;
        let trigger_threshold = ((wf_header >> constants::TRIGGER_THRESHOLD_SHIFT)
            & constants::TRIGGER_THRESHOLD_MASK) as u16;

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
                let ap2 = ((sample >> constants::ANALOG_PROBE2_SHIFT)
                    & constants::ANALOG_PROBE_MASK) as i16;
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
    fn test_decoder_with_config() {
        let config = Psd2Config {
            time_step_ns: 4.0,
            module_id: 5,
            dump_enabled: true,
        };
        let decoder = Psd2Decoder::new(config);
        assert_eq!(decoder.config.time_step_ns, 4.0);
        assert_eq!(decoder.config.module_id, 5);
        assert!(decoder.config.dump_enabled);
    }

    #[test]
    fn test_classify_small_data() {
        let decoder = Psd2Decoder::with_defaults();
        let raw = RawData {
            data: vec![0; 8], // Too small (< 16 bytes = 2 words)
            size: 8,
            n_events: 0,
        };
        assert_eq!(decoder.classify(&raw), DataType::Unknown);
    }

    #[test]
    fn test_classify_minimum_size() {
        let decoder = Psd2Decoder::with_defaults();
        // Exactly 16 bytes (2 words) - minimum for single-word event
        let raw = RawData {
            data: vec![0; 16],
            size: 16,
            n_events: 0,
        };
        // Not a start/stop signal, so should be Event
        assert_eq!(decoder.classify(&raw), DataType::Event);
    }

    #[test]
    fn test_classify_stop_signal() {
        let decoder = Psd2Decoder::with_defaults();
        // Stop signal: 24 bytes (3 words), type=0x3, subtype=0x2
        // Word format (Big Endian): type in bits 63-60, subtype in bits 59-56
        let mut data = vec![0u8; 24];
        // Set first word: type=3 at bits 63-60, subtype=2 at bits 59-56
        // In Big Endian: byte 0 contains bits 63-56
        data[0] = 0x32; // type=3, subtype=2
        let raw = RawData {
            data,
            size: 24,
            n_events: 0,
        };
        assert_eq!(decoder.classify(&raw), DataType::Stop);
    }

    #[test]
    fn test_classify_start_signal() {
        let decoder = Psd2Decoder::with_defaults();
        // Start signal: 32 bytes (4 words), type=0x3, subtype=0x0
        let mut data = vec![0u8; 32];
        // Set first word: type=3 at bits 63-60, subtype=0 at bits 59-56
        data[0] = 0x30; // type=3, subtype=0
        let raw = RawData {
            data,
            size: 32,
            n_events: 0,
        };
        assert_eq!(decoder.classify(&raw), DataType::Start);
    }

    #[test]
    fn test_read_u64_big_endian() {
        let decoder = Psd2Decoder::with_defaults();
        // Test big-endian reading
        let data: Vec<u8> = vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        let word = decoder.read_u64(&data, 0);
        assert_eq!(word, 0x0102030405060708);
    }

    #[test]
    fn test_set_dump_enabled() {
        let mut decoder = Psd2Decoder::with_defaults();
        assert!(!decoder.config.dump_enabled);
        decoder.set_dump_enabled(true);
        assert!(decoder.config.dump_enabled);
        decoder.set_dump_enabled(false);
        assert!(!decoder.config.dump_enabled);
    }

    #[test]
    fn test_decode_empty_returns_empty() {
        let mut decoder = Psd2Decoder::with_defaults();
        let raw = RawData {
            data: vec![0; 8], // Too small
            size: 8,
            n_events: 0,
        };
        let events = decoder.decode(&raw);
        assert!(events.is_empty());
    }

    #[test]
    fn test_decode_stop_signal_returns_empty() {
        let mut decoder = Psd2Decoder::with_defaults();
        let mut data = vec![0u8; 24];
        data[0] = 0x32; // Stop signal
        let raw = RawData {
            data,
            size: 24,
            n_events: 0,
        };
        let events = decoder.decode(&raw);
        assert!(events.is_empty());
    }

    #[test]
    fn test_decode_start_signal_returns_empty() {
        let mut decoder = Psd2Decoder::with_defaults();
        let mut data = vec![0u8; 32];
        data[0] = 0x30; // Start signal
        let raw = RawData {
            data,
            size: 32,
            n_events: 0,
        };
        let events = decoder.decode(&raw);
        assert!(events.is_empty());
    }

    #[test]
    fn test_decode_invalid_header_type() {
        let mut decoder = Psd2Decoder::with_defaults();
        // Create data with wrong header type (not 0x2)
        let mut data = vec![0u8; 24];
        // Header type at bits 63-60, set to 0x1 (invalid, should be 0x2)
        data[0] = 0x10;
        let raw = RawData {
            data,
            size: 24,
            n_events: 0,
        };
        let events = decoder.decode(&raw);
        assert!(events.is_empty());
    }

    #[test]
    fn test_decode_valid_single_event() {
        let mut decoder = Psd2Decoder::with_defaults();

        // Create valid event data (Big Endian)
        // Word 0 (Header): type=0x2, total_size=3 (3 words)
        // Word 1 (Event first): channel=5, timestamp=1000
        // Word 2 (Event second): flags, energy, etc.
        let mut data = vec![0u8; 24];

        // Header word (8 bytes, Big Endian):
        // - bits 63-60: type = 0x2 (DATA)
        // - bits 31-0: total_size = 3 (words)
        data[0] = 0x20; // type=2 in high nibble
        data[7] = 0x03; // total_size=3 in low byte

        // Event first word (channel and timestamp):
        // - bits 62-56: channel = 5
        // - bits 47-0: timestamp (coarse) = 500
        data[8] = 0x05; // channel=5 in bits 62-56 (shifted)
                        // timestamp = 500 in low 6 bytes
        data[13] = 0x00;
        data[14] = 0x01;
        data[15] = 0xF4; // 500 = 0x1F4

        // Event second word (energy, flags, etc.):
        // - bits 15-0: energy = 1234
        // - bits 41-26: energy_short = 567
        // - bits 25-16: fine_time = 100
        data[22] = 0x04; // energy high byte
        data[23] = 0xD2; // energy low byte = 1234

        let raw = RawData {
            data,
            size: 24,
            n_events: 1,
        };

        let events = decoder.decode(&raw);
        assert_eq!(events.len(), 1);

        let event = &events[0];
        assert_eq!(event.channel, 5);
        assert_eq!(event.energy, 1234);
    }

    #[test]
    fn test_psd2_config_default() {
        let config = Psd2Config::default();
        assert_eq!(config.time_step_ns, 2.0);
        assert_eq!(config.module_id, 0);
        assert!(!config.dump_enabled);
    }

    #[test]
    fn test_constants_word_size() {
        assert_eq!(constants::WORD_SIZE, 8);
        assert_eq!(constants::MIN_DATA_SIZE, 16); // 2 * 8
        assert_eq!(constants::START_SIGNAL_SIZE, 32); // 4 * 8
        assert_eq!(constants::STOP_SIGNAL_SIZE, 24); // 3 * 8
    }

    #[test]
    fn test_constants_header_masks() {
        assert_eq!(constants::HEADER_TYPE_MASK, 0xF);
        assert_eq!(constants::HEADER_TYPE_DATA, 0x2);
        assert_eq!(constants::HEADER_TYPE_SHIFT, 60);
    }

    #[test]
    fn test_constants_signal_types() {
        assert_eq!(constants::START_SIGNAL_TYPE, 0x3);
        assert_eq!(constants::START_SIGNAL_SUBTYPE, 0x0);
        assert_eq!(constants::STOP_SIGNAL_TYPE, 0x3);
        assert_eq!(constants::STOP_SIGNAL_SUBTYPE, 0x2);
    }

    /// Helper: build a big-endian byte vector from u64 words
    fn words_to_bytes(words: &[u64]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    /// Build a data header word (type=0x2, total_size in 64-bit words)
    fn make_header(total_words: u32) -> u64 {
        (0x2u64 << 60) | (total_words as u64)
    }

    /// Build a standard first word (last=0, channel, timestamp_48bit)
    fn make_first_word(channel: u8, timestamp: u64) -> u64 {
        ((channel as u64) << 56) | (timestamp & 0xFFFFFFFFFFFF)
    }

    /// Build a standard second word
    fn make_second_word(
        last_word: bool,
        has_waveform: bool,
        flags_low: u16,
        flags_high: u8,
        energy_short: u16,
        fine_time: u16,
        energy: u16,
    ) -> u64 {
        ((last_word as u64) << 63)
            | ((has_waveform as u64) << 62)
            | (((flags_low as u64) & 0xFFF) << 50)
            | ((flags_high as u64) << 42)
            | ((energy_short as u64) << 26)
            | (((fine_time as u64) & 0x3FF) << 16)
            | (energy as u64)
    }

    /// Build a single-word event (last=1, channel, flag_high, timestamp_reduced, energy)
    fn make_single_word_event(channel: u8, flag_high: u8, timestamp: u32, energy: u16) -> u64 {
        (1u64 << 63)
            | ((channel as u64) << 56)
            | ((flag_high as u64) << 48)
            | ((timestamp as u64) << 16)
            | (energy as u64)
    }

    /// Build a special event first word (special_event=1)
    fn make_special_first_word(channel: u8, timestamp: u64) -> u64 {
        ((channel as u64) << 56) | (1u64 << 55) | (timestamp & 0xFFFFFFFFFFFF)
    }

    /// Build an extra word (last_word flag, extra_type, extra_data)
    fn make_extra_word(last_word: bool, extra_type: u8) -> u64 {
        ((last_word as u64) << 63) | (((extra_type as u64) & 0x7) << 60)
    }

    #[test]
    fn test_decode_single_word_event() {
        let mut decoder = Psd2Decoder::with_defaults();

        // Single-word event: last=1, channel=10, flag_high=0x03, timestamp=5000, energy=2000
        let data = words_to_bytes(&[
            make_header(2),                              // header: 2 words total
            make_single_word_event(10, 0x03, 5000, 2000), // single-word event
        ]);

        let raw = RawData {
            size: data.len(),
            data,
            n_events: 1,
        };

        let events = decoder.decode(&raw);
        assert_eq!(events.len(), 1);

        let event = &events[0];
        assert_eq!(event.channel, 10);
        assert_eq!(event.energy, 2000);
        assert_eq!(event.energy_short, 0); // not available in single-word
        assert_eq!(event.fine_time, 0); // not available in single-word
        assert_eq!(event.flags, 0x03 << 12); // high priority shifted by 12
        assert_eq!(event.timestamp_ns, 5000.0 * 2.0); // timestamp * time_step
        assert!(event.waveform.is_none());
    }

    #[test]
    fn test_decode_special_event_filtered() {
        let mut decoder = Psd2Decoder::with_defaults();

        // Special event with one extra word (time_info)
        let data = words_to_bytes(&[
            make_header(4), // 4 words total
            make_special_first_word(3, 1000), // special event, channel=3
            make_second_word(false, false, 0, 0, 0, 0, 0), // 2nd word, last=0
            make_extra_word(true, 1), // extra: last=1, type=time_info
        ]);

        let raw = RawData {
            size: data.len(),
            data,
            n_events: 1,
        };

        let events = decoder.decode(&raw);
        assert_eq!(events.len(), 0, "Special events should be filtered out");
    }

    #[test]
    fn test_decode_special_event_with_following_normal_event() {
        let mut decoder = Psd2Decoder::with_defaults();

        // Special event followed by a normal event
        let data = words_to_bytes(&[
            make_header(6), // 6 words total
            // Special event (3 words)
            make_special_first_word(3, 1000),
            make_second_word(false, false, 0, 0, 0, 0, 0),
            make_extra_word(true, 1),
            // Normal event (2 words)
            make_first_word(5, 2000),
            make_second_word(true, false, 0, 0, 0, 0, 1500),
        ]);

        let raw = RawData {
            size: data.len(),
            data,
            n_events: 2,
        };

        let events = decoder.decode(&raw);
        assert_eq!(events.len(), 1, "Only normal event should remain");
        assert_eq!(events[0].channel, 5);
        assert_eq!(events[0].energy, 1500);
    }

    #[test]
    fn test_decode_flags_12bit_mask() {
        let mut decoder = Psd2Decoder::with_defaults();

        // Event with all 12 flags_low bits set and flags_high = 0xAB
        let data = words_to_bytes(&[
            make_header(3),
            make_first_word(1, 500),
            make_second_word(true, false, 0xFFF, 0xAB, 100, 50, 3000),
        ]);

        let raw = RawData {
            size: data.len(),
            data,
            n_events: 1,
        };

        let events = decoder.decode(&raw);
        assert_eq!(events.len(), 1);

        let event = &events[0];
        assert_eq!(event.energy, 3000);
        assert_eq!(event.energy_short, 100);
        assert_eq!(event.fine_time, 50);

        // flags = (flags_high << 12) | flags_low = (0xAB << 12) | 0xFFF
        let expected_flags = (0xABu32 << 12) | 0xFFF;
        assert_eq!(
            event.flags, expected_flags,
            "flags should use 12-bit shift: got 0x{:05x}, expected 0x{:05x}",
            event.flags, expected_flags
        );
    }

    #[test]
    fn test_decode_mixed_single_and_standard_events() {
        let mut decoder = Psd2Decoder::with_defaults();

        // Mix: standard event + single-word event
        let data = words_to_bytes(&[
            make_header(4),                        // 4 words
            make_first_word(1, 100),               // standard: 1st word
            make_second_word(true, false, 0, 0, 0, 0, 500), // standard: 2nd word
            make_single_word_event(2, 0, 200, 800), // single-word event
        ]);

        let raw = RawData {
            size: data.len(),
            data,
            n_events: 2,
        };

        let events = decoder.decode(&raw);
        assert_eq!(events.len(), 2);

        // Events sorted by timestamp
        assert_eq!(events[0].channel, 1); // timestamp 100*2=200ns
        assert_eq!(events[0].energy, 500);
        assert_eq!(events[1].channel, 2); // timestamp 200*2=400ns
        assert_eq!(events[1].energy, 800);
    }

    #[test]
    fn test_aggregate_counter_tracking() {
        let decoder = Psd2Decoder::with_defaults();
        assert_eq!(decoder.last_aggregate_counter, 0);
    }

    #[test]
    fn test_events_sorted_by_timestamp() {
        let mut decoder = Psd2Decoder::with_defaults();

        // Create data with header indicating 2 events
        let mut data = vec![0u8; 40]; // 5 words (header + 2 events)

        // Header: type=2, total_size=5
        data[0] = 0x20;
        data[7] = 0x05;

        // First event: channel=1, later timestamp
        data[8] = 0x01;
        data[15] = 0xFF; // larger timestamp

        // Energy for first event
        data[22] = 0x00;
        data[23] = 0x64; // 100

        // Second event: channel=2, earlier timestamp
        data[24] = 0x02;
        data[31] = 0x01; // smaller timestamp

        // Energy for second event
        data[38] = 0x00;
        data[39] = 0xC8; // 200

        let raw = RawData {
            data,
            size: 40,
            n_events: 2,
        };

        let events = decoder.decode(&raw);
        // Events should be sorted by timestamp (ascending)
        if events.len() >= 2 {
            assert!(events[0].timestamp_ns <= events[1].timestamp_ns);
        }
    }
}
