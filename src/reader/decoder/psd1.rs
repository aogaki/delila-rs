//! PSD1 Decoder for DT5730 (DPP-PSD1) digitizers
//!
//! Decodes RAW data from x725/x730 series digitizers with DPP-PSD firmware.
//!
//! # Data Format
//!
//! PSD1 uses 32-bit Little-Endian words in a hierarchical structure:
//! Board Aggregate → Dual Channel Block → Events
//!
//! Key differences from PSD2:
//! - 32-bit LE (vs 64-bit BE)
//! - Hierarchical board → channel pair → event structure
//! - No Start/Stop signals in data
//! - Channel pairing: pair * 2 + channel_flag
//! - 47-bit timestamp: (extended_time << 31) | trigger_time_tag

use super::common::{DataType, EventData, RawData, Waveform};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

mod constants {
    pub const WORD_SIZE: usize = 4; // 32-bit

    pub mod board_header {
        pub const HEADER_SIZE_WORDS: usize = 4;
        pub const HEADER_SIZE_BYTES: usize = HEADER_SIZE_WORDS * super::WORD_SIZE;

        // Word 0
        pub const TYPE_SHIFT: u32 = 28;
        pub const TYPE_MASK: u32 = 0xF;
        pub const TYPE_DATA: u32 = 0xA;
        pub const AGGREGATE_SIZE_MASK: u32 = 0x0FFF_FFFF;

        // Word 1
        pub const BOARD_ID_SHIFT: u32 = 27;
        pub const BOARD_ID_MASK: u32 = 0x1F;
        pub const BOARD_FAIL_SHIFT: u32 = 26;
        pub const DUAL_CHANNEL_MASK: u32 = 0xFF;

        // Word 2
        pub const COUNTER_MASK: u32 = 0x7F_FFFF;
    }

    pub mod channel_header {
        pub const HEADER_SIZE_WORDS: usize = 2;

        // Word 0
        pub const DUAL_CHANNEL_SIZE_MASK: u32 = 0x3F_FFFF;

        // Word 1 - Configuration
        pub const NUM_SAMPLES_MASK: u32 = 0xFFFF;
        pub const DIGITAL_PROBE1_SHIFT: u32 = 16;
        pub const DIGITAL_PROBE1_MASK: u32 = 0x7;
        pub const DIGITAL_PROBE2_SHIFT: u32 = 19;
        pub const DIGITAL_PROBE2_MASK: u32 = 0x7;
        pub const ANALOG_PROBE_SHIFT: u32 = 22;
        pub const ANALOG_PROBE_MASK: u32 = 0x3;
        pub const EXTRA_OPTION_SHIFT: u32 = 24;
        pub const EXTRA_OPTION_MASK: u32 = 0x7;
        pub const SAMPLES_ENABLED_SHIFT: u32 = 27;
        pub const EXTRAS_ENABLED_SHIFT: u32 = 28;
        pub const TIME_ENABLED_SHIFT: u32 = 29;
        pub const CHARGE_ENABLED_SHIFT: u32 = 30;
        pub const DUAL_TRACE_SHIFT: u32 = 31;
    }

    pub mod event {
        // Trigger Time Tag word
        pub const TRIGGER_TIME_MASK: u32 = 0x7FFF_FFFF;
        pub const CHANNEL_FLAG_SHIFT: u32 = 31;

        // Extras word (option 0b010)
        pub const FINE_TIME_MASK: u32 = 0x3FF;
        pub const FLAGS_SHIFT: u32 = 10;
        pub const FLAGS_MASK: u32 = 0x3F;
        pub const EXTENDED_TIME_SHIFT: u32 = 16;
        pub const EXTENDED_TIME_MASK: u32 = 0xFFFF;

        // Charge word
        pub const CHARGE_SHORT_MASK: u32 = 0x7FFF;
        pub const PILEUP_SHIFT: u32 = 15;
        pub const CHARGE_LONG_SHIFT: u32 = 16;
        pub const CHARGE_LONG_MASK: u32 = 0xFFFF;

        // Timestamp
        pub const FINE_TIME_SCALE: f64 = 1024.0;
    }

    pub mod waveform {
        pub const ANALOG_SAMPLE_MASK: u32 = 0x3FFF;
        pub const DP1_SHIFT: u32 = 14;
        pub const DP2_SHIFT: u32 = 15;
        pub const SECOND_SAMPLE_SHIFT: u32 = 16;
        pub const SAMPLES_PER_GROUP: usize = 8;
    }
}

// ---------------------------------------------------------------------------
// Internal data structures
// ---------------------------------------------------------------------------

/// Board Aggregate Header (4 words)
#[derive(Debug, Clone)]
struct BoardHeader {
    aggregate_size: u32,
    #[allow(dead_code)]
    board_id: u8,
    board_fail: bool,
    dual_channel_mask: u8,
    aggregate_counter: u32,
    #[allow(dead_code)]
    board_time_tag: u32,
}

/// Dual Channel Header (2 words)
#[derive(Debug, Clone)]
struct DualChannelHeader {
    block_size: u32,
    num_samples_wave: u16,
    #[allow(dead_code)]
    digital_probe1: u8,
    #[allow(dead_code)]
    digital_probe2: u8,
    #[allow(dead_code)]
    analog_probe: u8,
    extra_option: u8,
    samples_enabled: bool,
    extras_enabled: bool,
    time_enabled: bool,
    charge_enabled: bool,
    dual_trace: bool,
}

impl DualChannelHeader {
    /// Calculate the number of words per event based on enable flags
    fn event_size_words(&self) -> usize {
        let mut size = 0;
        if self.time_enabled {
            size += 1;
        }
        if self.extras_enabled {
            size += 1;
        }
        if self.samples_enabled {
            size += self.num_samples_wave as usize * 2;
        }
        if self.charge_enabled {
            size += 1;
        }
        size
    }
}

// ---------------------------------------------------------------------------
// PSD1 Configuration & Decoder
// ---------------------------------------------------------------------------

/// PSD1 decoder configuration
#[derive(Debug, Clone)]
pub struct Psd1Config {
    /// Time step in nanoseconds (DT5730 = 2 ns at 500 MS/s)
    pub time_step_ns: f64,
    /// Module identifier for EventData output
    pub module_id: u8,
    /// Enable debug dump output
    pub dump_enabled: bool,
}

impl Default for Psd1Config {
    fn default() -> Self {
        Self {
            time_step_ns: 2.0, // DT5730: 500 MS/s
            module_id: 0,
            dump_enabled: false,
        }
    }
}

/// PSD1 Decoder for DT5730 (DPP-PSD) digitizers
pub struct Psd1Decoder {
    config: Psd1Config,
    last_aggregate_counter: u32,
}

impl Psd1Decoder {
    /// Create a new PSD1 decoder with given configuration
    pub fn new(config: Psd1Config) -> Self {
        Self {
            config,
            last_aggregate_counter: 0,
        }
    }

    /// Create a decoder with default configuration
    pub fn with_defaults() -> Self {
        Self::new(Psd1Config::default())
    }

    /// Enable or disable dump output
    pub fn set_dump_enabled(&mut self, enabled: bool) {
        self.config.dump_enabled = enabled;
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Classify the data type
    ///
    /// PSD1 has no Start/Stop signals in the data stream.
    /// Returns Event if valid board header (type=0xA), Unknown otherwise.
    pub fn classify(&self, raw: &RawData) -> DataType {
        if raw.size < constants::board_header::HEADER_SIZE_BYTES {
            return DataType::Unknown;
        }
        if !raw.size.is_multiple_of(constants::WORD_SIZE) {
            return DataType::Unknown;
        }

        let word0 = read_u32(&raw.data, 0);
        let header_type =
            (word0 >> constants::board_header::TYPE_SHIFT) & constants::board_header::TYPE_MASK;

        if header_type == constants::board_header::TYPE_DATA {
            DataType::Event
        } else {
            DataType::Unknown
        }
    }

    /// Decode raw data into events
    pub fn decode(&mut self, raw: &RawData) -> Vec<EventData> {
        let data_type = self.classify(raw);
        if data_type != DataType::Event {
            if self.config.dump_enabled {
                println!("[PSD1] Non-event data, size={}", raw.size);
            }
            return vec![];
        }

        let total_bytes = raw.size;
        let mut offset: usize = 0;
        let mut all_events = Vec::new();

        // Process multiple board aggregate blocks
        while offset + constants::board_header::HEADER_SIZE_BYTES <= total_bytes {
            match self.decode_board_aggregate(&raw.data, &mut offset) {
                Ok(mut events) => all_events.append(&mut events),
                Err(msg) => {
                    if self.config.dump_enabled {
                        println!("[PSD1] Board aggregate error: {}", msg);
                    }
                    break;
                }
            }
        }

        // Sort by timestamp
        all_events.sort_by(|a, b| {
            a.timestamp_ns
                .partial_cmp(&b.timestamp_ns)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if self.config.dump_enabled {
            println!("[PSD1] Decoded {} events", all_events.len());
        }

        all_events
    }

    // -----------------------------------------------------------------------
    // Board level
    // -----------------------------------------------------------------------

    fn decode_board_aggregate(
        &mut self,
        data: &[u8],
        offset: &mut usize,
    ) -> Result<Vec<EventData>, String> {
        let header = self.decode_board_header(data, *offset)?;

        let block_end = *offset + (header.aggregate_size as usize) * constants::WORD_SIZE;
        if block_end > data.len() {
            return Err(format!(
                "Board aggregate size {} exceeds data length {}",
                block_end,
                data.len()
            ));
        }

        // Track aggregate counter
        if self.last_aggregate_counter != 0
            && header.aggregate_counter != self.last_aggregate_counter.wrapping_add(1)
            && self.config.dump_enabled
        {
            println!(
                "[PSD1] Aggregate counter discontinuity: {} -> {}",
                self.last_aggregate_counter, header.aggregate_counter
            );
        }
        self.last_aggregate_counter = header.aggregate_counter;

        if header.board_fail && self.config.dump_enabled {
            println!("[PSD1] Board fail bit set!");
        }

        *offset += constants::board_header::HEADER_SIZE_WORDS * constants::WORD_SIZE;

        let mut events = Vec::new();
        let mask = header.dual_channel_mask;

        for pair_index in 0u8..8 {
            if mask & (1 << pair_index) == 0 {
                continue;
            }
            if *offset >= block_end {
                break;
            }

            match self.decode_dual_channel_block(data, offset, pair_index, block_end) {
                Ok(mut ch_events) => events.append(&mut ch_events),
                Err(msg) => {
                    if self.config.dump_enabled {
                        println!("[PSD1] Dual channel pair {} error: {}", pair_index, msg);
                    }
                    // Skip to block end
                    *offset = block_end;
                    break;
                }
            }
        }

        // Ensure offset is at block end
        *offset = block_end;
        Ok(events)
    }

    fn decode_board_header(&self, data: &[u8], offset: usize) -> Result<BoardHeader, String> {
        if offset + constants::board_header::HEADER_SIZE_BYTES > data.len() {
            return Err("Insufficient data for board header".to_string());
        }

        let w0 = read_u32(data, offset);
        let w1 = read_u32(data, offset + 4);
        let w2 = read_u32(data, offset + 8);
        let w3 = read_u32(data, offset + 12);

        let header_type =
            (w0 >> constants::board_header::TYPE_SHIFT) & constants::board_header::TYPE_MASK;
        if header_type != constants::board_header::TYPE_DATA {
            return Err(format!(
                "Invalid header type: 0x{:x} (expected 0xA)",
                header_type
            ));
        }

        Ok(BoardHeader {
            aggregate_size: w0 & constants::board_header::AGGREGATE_SIZE_MASK,
            board_id: ((w1 >> constants::board_header::BOARD_ID_SHIFT)
                & constants::board_header::BOARD_ID_MASK) as u8,
            board_fail: ((w1 >> constants::board_header::BOARD_FAIL_SHIFT) & 1) != 0,
            dual_channel_mask: (w1 & constants::board_header::DUAL_CHANNEL_MASK) as u8,
            aggregate_counter: w2 & constants::board_header::COUNTER_MASK,
            board_time_tag: w3,
        })
    }

    // -----------------------------------------------------------------------
    // Channel level
    // -----------------------------------------------------------------------

    fn decode_dual_channel_block(
        &self,
        data: &[u8],
        offset: &mut usize,
        pair_index: u8,
        block_end: usize,
    ) -> Result<Vec<EventData>, String> {
        let ch_header = self.decode_dual_channel_header(data, *offset)?;

        let ch_block_end = *offset + (ch_header.block_size as usize) * constants::WORD_SIZE;
        let ch_block_end = ch_block_end.min(block_end);

        *offset += constants::channel_header::HEADER_SIZE_WORDS * constants::WORD_SIZE;

        let event_size = ch_header.event_size_words();
        if event_size == 0 {
            return Ok(vec![]);
        }

        let mut events = Vec::new();

        while *offset + event_size * constants::WORD_SIZE <= ch_block_end {
            match self.decode_event(data, offset, &ch_header, pair_index) {
                Ok(event) => events.push(event),
                Err(msg) => {
                    if self.config.dump_enabled {
                        println!("[PSD1] Event decode error: {}", msg);
                    }
                    break;
                }
            }
        }

        *offset = ch_block_end;
        Ok(events)
    }

    fn decode_dual_channel_header(
        &self,
        data: &[u8],
        offset: usize,
    ) -> Result<DualChannelHeader, String> {
        let needed = constants::channel_header::HEADER_SIZE_WORDS * constants::WORD_SIZE;
        if offset + needed > data.len() {
            return Err("Insufficient data for channel header".to_string());
        }

        let w0 = read_u32(data, offset);
        let w1 = read_u32(data, offset + 4);

        Ok(DualChannelHeader {
            block_size: w0 & constants::channel_header::DUAL_CHANNEL_SIZE_MASK,
            num_samples_wave: (w1 & constants::channel_header::NUM_SAMPLES_MASK) as u16,
            digital_probe1: ((w1 >> constants::channel_header::DIGITAL_PROBE1_SHIFT)
                & constants::channel_header::DIGITAL_PROBE1_MASK) as u8,
            digital_probe2: ((w1 >> constants::channel_header::DIGITAL_PROBE2_SHIFT)
                & constants::channel_header::DIGITAL_PROBE2_MASK) as u8,
            analog_probe: ((w1 >> constants::channel_header::ANALOG_PROBE_SHIFT)
                & constants::channel_header::ANALOG_PROBE_MASK) as u8,
            extra_option: ((w1 >> constants::channel_header::EXTRA_OPTION_SHIFT)
                & constants::channel_header::EXTRA_OPTION_MASK) as u8,
            samples_enabled: ((w1 >> constants::channel_header::SAMPLES_ENABLED_SHIFT) & 1) != 0,
            extras_enabled: ((w1 >> constants::channel_header::EXTRAS_ENABLED_SHIFT) & 1) != 0,
            time_enabled: ((w1 >> constants::channel_header::TIME_ENABLED_SHIFT) & 1) != 0,
            charge_enabled: ((w1 >> constants::channel_header::CHARGE_ENABLED_SHIFT) & 1) != 0,
            dual_trace: ((w1 >> constants::channel_header::DUAL_TRACE_SHIFT) & 1) != 0,
        })
    }

    // -----------------------------------------------------------------------
    // Event level
    // -----------------------------------------------------------------------

    fn decode_event(
        &self,
        data: &[u8],
        offset: &mut usize,
        ch_header: &DualChannelHeader,
        pair_index: u8,
    ) -> Result<EventData, String> {
        // Time tag
        let mut trigger_time_tag: u32 = 0;
        let mut channel_flag: u8 = 0;
        if ch_header.time_enabled {
            let w = read_u32(data, *offset);
            *offset += constants::WORD_SIZE;
            channel_flag = ((w >> constants::event::CHANNEL_FLAG_SHIFT) & 1) as u8;
            trigger_time_tag = w & constants::event::TRIGGER_TIME_MASK;
        }

        // Extras
        let mut extended_time: u16 = 0;
        let mut fine_time: u16 = 0;
        let mut flags: u32 = 0;
        if ch_header.extras_enabled {
            let w = read_u32(data, *offset);
            *offset += constants::WORD_SIZE;
            let (ext, ft, fl) = decode_extras_word(w, ch_header.extra_option);
            extended_time = ext;
            fine_time = ft;
            flags = fl;
        }

        // Waveform
        let waveform = if ch_header.samples_enabled {
            Some(self.decode_waveform(data, offset, ch_header))
        } else {
            None
        };

        // Charge
        let mut charge_long: u16 = 0;
        let mut charge_short: u16 = 0;
        if ch_header.charge_enabled {
            let w = read_u32(data, *offset);
            *offset += constants::WORD_SIZE;
            let (cl, cs, pileup) = decode_charge_word(w);
            charge_long = cl;
            charge_short = cs;
            if pileup {
                flags |= 1 << 15; // Pileup flag at bit 15
            }
        }

        let channel = pair_index * 2 + channel_flag;
        let timestamp_ns =
            calculate_timestamp(&self.config, trigger_time_tag, extended_time, fine_time);

        if self.config.dump_enabled {
            println!("--- PSD1 Event ---");
            println!("  Channel:      {}", channel);
            println!("  Timestamp:    {:.3} ns", timestamp_ns);
            println!("  Charge Long:  {}", charge_long);
            println!("  Charge Short: {}", charge_short);
            println!("  Fine Time:    {}", fine_time);
            println!("  Flags:        0x{:08x}", flags);
        }

        Ok(EventData {
            timestamp_ns,
            module: self.config.module_id,
            channel,
            energy: charge_long,
            energy_short: charge_short,
            fine_time,
            flags,
            waveform,
        })
    }

    // -----------------------------------------------------------------------
    // Waveform
    // -----------------------------------------------------------------------

    fn decode_waveform(
        &self,
        data: &[u8],
        offset: &mut usize,
        ch_header: &DualChannelHeader,
    ) -> Waveform {
        let total_words = ch_header.num_samples_wave as usize * 2;
        let total_samples =
            ch_header.num_samples_wave as usize * constants::waveform::SAMPLES_PER_GROUP;

        let capacity = if ch_header.dual_trace {
            total_samples / 2
        } else {
            total_samples
        };

        let mut analog_probe1 = Vec::with_capacity(capacity);
        let mut analog_probe2 = Vec::with_capacity(if ch_header.dual_trace { capacity } else { 0 });
        let mut digital_probe1 = Vec::with_capacity(total_samples);
        let mut digital_probe2 = Vec::with_capacity(total_samples);

        for _ in 0..total_words {
            let w = read_u32(data, *offset);
            *offset += constants::WORD_SIZE;

            // Lower half: sample 2N
            let s1_analog = (w & constants::waveform::ANALOG_SAMPLE_MASK) as i16;
            let s1_dp1 = ((w >> constants::waveform::DP1_SHIFT) & 1) as u8;
            let s1_dp2 = ((w >> constants::waveform::DP2_SHIFT) & 1) as u8;

            // Upper half: sample 2N+1
            let upper = w >> constants::waveform::SECOND_SAMPLE_SHIFT;
            let s2_analog = (upper & constants::waveform::ANALOG_SAMPLE_MASK) as i16;
            let s2_dp1 = ((upper >> constants::waveform::DP1_SHIFT) & 1) as u8;
            let s2_dp2 = ((upper >> constants::waveform::DP2_SHIFT) & 1) as u8;

            if ch_header.dual_trace {
                // Even samples = probe1, odd samples = probe2
                analog_probe1.push(s1_analog);
                analog_probe2.push(s2_analog);
            } else {
                analog_probe1.push(s1_analog);
                analog_probe1.push(s2_analog);
            }

            digital_probe1.push(s1_dp1);
            digital_probe1.push(s2_dp1);
            digital_probe2.push(s1_dp2);
            digital_probe2.push(s2_dp2);
        }

        Waveform {
            analog_probe1,
            analog_probe2,
            digital_probe1,
            digital_probe2,
            digital_probe3: vec![],
            digital_probe4: vec![],
            time_resolution: 0,
            trigger_threshold: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Free functions (pure, easy to test)
// ---------------------------------------------------------------------------

/// Read a u32 from data at given byte offset (Little-Endian)
#[inline]
fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

/// Decode extras word based on extra_option
///
/// Returns (extended_time, fine_time, flags)
fn decode_extras_word(word: u32, extra_option: u8) -> (u16, u16, u32) {
    let extended_time = ((word >> constants::event::EXTENDED_TIME_SHIFT)
        & constants::event::EXTENDED_TIME_MASK) as u16;

    match extra_option {
        // 0b010: Extended time + flags + fine time
        2 => {
            let flags = (word >> constants::event::FLAGS_SHIFT) & constants::event::FLAGS_MASK;
            let fine_time = (word & constants::event::FINE_TIME_MASK) as u16;
            (extended_time, fine_time, flags)
        }
        // 0b001: Extended time + flags (16-bit)
        1 => {
            let flags = word & 0xFFFF;
            (extended_time, 0, flags)
        }
        // 0b000: Extended time + baseline×4
        0 => (extended_time, 0, 0),
        // Others: just extract extended time
        _ => (extended_time, 0, 0),
    }
}

/// Decode charge word
///
/// Returns (charge_long, charge_short, pileup)
fn decode_charge_word(word: u32) -> (u16, u16, bool) {
    let charge_long =
        ((word >> constants::event::CHARGE_LONG_SHIFT) & constants::event::CHARGE_LONG_MASK) as u16;
    let charge_short = (word & constants::event::CHARGE_SHORT_MASK) as u16;
    let pileup = ((word >> constants::event::PILEUP_SHIFT) & 1) != 0;
    (charge_long, charge_short, pileup)
}

/// Calculate timestamp in nanoseconds
fn calculate_timestamp(
    config: &Psd1Config,
    trigger_time_tag: u32,
    extended_time: u16,
    fine_time: u16,
) -> f64 {
    let combined = ((extended_time as u64) << 31) | (trigger_time_tag as u64);
    let coarse_ns = (combined as f64) * config.time_step_ns;
    let fine_ns = (fine_time as f64) * (config.time_step_ns / constants::event::FINE_TIME_SCALE);
    coarse_ns + fine_ns
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Write a u32 in Little-Endian to a byte vector
    fn push_u32(buf: &mut Vec<u8>, value: u32) {
        buf.extend_from_slice(&value.to_le_bytes());
    }

    /// Build board header (4 words)
    fn make_board_header(aggregate_size: u32, mask: u8, board_id: u8, counter: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        // Word 0: type=0xA + size
        push_u32(&mut buf, (0xA << 28) | (aggregate_size & 0x0FFF_FFFF));
        // Word 1: board_id + mask
        push_u32(&mut buf, ((board_id as u32) << 27) | (mask as u32));
        // Word 2: counter
        push_u32(&mut buf, counter & 0x7F_FFFF);
        // Word 3: time tag
        push_u32(&mut buf, 0x1234_5678);
        buf
    }

    /// Dual channel config flags packed into word 1
    struct DualChFlags {
        dt: bool,
        eq: bool,
        et: bool,
        ee: bool,
        es: bool,
        extra_option: u8,
        num_samples: u16,
    }

    impl Default for DualChFlags {
        fn default() -> Self {
            Self {
                dt: false,
                eq: true,
                et: true,
                ee: true,
                es: false,
                extra_option: 2, // Extended + flags + fine time
                num_samples: 0,
            }
        }
    }

    fn make_dual_channel_header(size: u32, flags: &DualChFlags) -> Vec<u8> {
        let mut buf = Vec::new();
        // Word 0: bit[31]=1, size
        push_u32(&mut buf, (1 << 31) | (size & 0x3F_FFFF));
        // Word 1: config flags
        let mut w1: u32 = flags.num_samples as u32;
        w1 |= (flags.extra_option as u32 & 0x7) << 24;
        if flags.es {
            w1 |= 1 << 27;
        }
        if flags.ee {
            w1 |= 1 << 28;
        }
        if flags.et {
            w1 |= 1 << 29;
        }
        if flags.eq {
            w1 |= 1 << 30;
        }
        if flags.dt {
            w1 |= 1 << 31;
        }
        push_u32(&mut buf, w1);
        buf
    }

    /// Make trigger time tag word
    fn make_time_word(trigger_time: u32, odd_channel: bool) -> u32 {
        let mut w = trigger_time & 0x7FFF_FFFF;
        if odd_channel {
            w |= 1 << 31;
        }
        w
    }

    /// Make extras word (option 0b010: extended_time + flags + fine_time)
    fn make_extras_word(extended_time: u16, flags: u8, fine_time: u16) -> u32 {
        ((extended_time as u32) << 16)
            | (((flags as u32) & 0x3F) << 10)
            | ((fine_time as u32) & 0x3FF)
    }

    /// Make charge word
    fn make_charge_word(charge_long: u16, charge_short: u16, pileup: bool) -> u32 {
        let mut w = ((charge_long as u32) << 16) | ((charge_short as u32) & 0x7FFF);
        if pileup {
            w |= 1 << 15;
        }
        w
    }

    /// Build a minimal event (ET+EE+EQ, 3 words)
    fn make_event(
        trigger_time: u32,
        odd: bool,
        ext_time: u16,
        flags: u8,
        fine_time: u16,
        charge_long: u16,
        charge_short: u16,
    ) -> Vec<u8> {
        let mut buf = Vec::new();
        push_u32(&mut buf, make_time_word(trigger_time, odd));
        push_u32(&mut buf, make_extras_word(ext_time, flags, fine_time));
        push_u32(&mut buf, make_charge_word(charge_long, charge_short, false));
        buf
    }

    fn default_decoder() -> Psd1Decoder {
        Psd1Decoder::new(Psd1Config {
            time_step_ns: 2.0,
            module_id: 0,
            dump_enabled: false,
        })
    }

    // -----------------------------------------------------------------------
    // Basic tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_decoder_creation() {
        let dec = Psd1Decoder::with_defaults();
        assert_eq!(dec.config.time_step_ns, 2.0);
        assert_eq!(dec.config.module_id, 0);
        assert!(!dec.config.dump_enabled);
    }

    #[test]
    fn test_decoder_with_config() {
        let dec = Psd1Decoder::new(Psd1Config {
            time_step_ns: 4.0,
            module_id: 5,
            dump_enabled: true,
        });
        assert_eq!(dec.config.time_step_ns, 4.0);
        assert_eq!(dec.config.module_id, 5);
        assert!(dec.config.dump_enabled);
    }

    #[test]
    fn test_set_dump_enabled() {
        let mut dec = Psd1Decoder::with_defaults();
        assert!(!dec.config.dump_enabled);
        dec.set_dump_enabled(true);
        assert!(dec.config.dump_enabled);
    }

    // -----------------------------------------------------------------------
    // classify tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_classify_too_small() {
        let dec = default_decoder();
        let raw = RawData::new(vec![0; 12]); // < 16 bytes
        assert_eq!(dec.classify(&raw), DataType::Unknown);
    }

    #[test]
    fn test_classify_not_aligned() {
        let dec = default_decoder();
        let raw = RawData::new(vec![0; 17]); // not multiple of 4
        assert_eq!(dec.classify(&raw), DataType::Unknown);
    }

    #[test]
    fn test_classify_valid_board_header() {
        let dec = default_decoder();
        let data = make_board_header(4, 0x01, 0, 1);
        let raw = RawData::new(data);
        assert_eq!(dec.classify(&raw), DataType::Event);
    }

    #[test]
    fn test_classify_invalid_header_type() {
        let dec = default_decoder();
        let mut data = vec![0u8; 16];
        // Set type to 0xB instead of 0xA
        let word0: u32 = 0xB000_0004;
        data[..4].copy_from_slice(&word0.to_le_bytes());
        let raw = RawData::new(data);
        assert_eq!(dec.classify(&raw), DataType::Unknown);
    }

    #[test]
    fn test_classify_always_event_no_start_stop() {
        // PSD1 never returns Start or Stop
        let dec = default_decoder();
        let data = make_board_header(4, 0x01, 0, 1);
        let raw = RawData::new(data);
        let dt = dec.classify(&raw);
        assert_ne!(dt, DataType::Start);
        assert_ne!(dt, DataType::Stop);
    }

    // -----------------------------------------------------------------------
    // read_u32 tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_read_u32_little_endian() {
        let data = [0x78, 0x56, 0x34, 0x12];
        assert_eq!(read_u32(&data, 0), 0x1234_5678);
    }

    #[test]
    fn test_read_u32_offset() {
        let data = [0x00, 0x00, 0x00, 0x00, 0xEF, 0xBE, 0xAD, 0xDE];
        assert_eq!(read_u32(&data, 4), 0xDEAD_BEEF);
    }

    // -----------------------------------------------------------------------
    // Board header tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_decode_board_header() {
        let dec = default_decoder();
        let data = make_board_header(100, 0x03, 5, 42);
        let header = dec.decode_board_header(&data, 0).unwrap();
        assert_eq!(header.aggregate_size, 100);
        assert_eq!(header.dual_channel_mask, 0x03);
        assert_eq!(header.board_id, 5);
        assert_eq!(header.aggregate_counter, 42);
        assert!(!header.board_fail);
    }

    #[test]
    fn test_decode_board_header_fail_bit() {
        let dec = default_decoder();
        let mut data = make_board_header(4, 0x01, 0, 1);
        // Set board fail bit (bit 26 of word 1)
        let w1 = read_u32(&data, 4) | (1 << 26);
        data[4..8].copy_from_slice(&w1.to_le_bytes());
        let header = dec.decode_board_header(&data, 0).unwrap();
        assert!(header.board_fail);
    }

    #[test]
    fn test_decode_board_header_insufficient_data() {
        let dec = default_decoder();
        let data = vec![0u8; 12]; // Only 3 words, need 4
        assert!(dec.decode_board_header(&data, 0).is_err());
    }

    // -----------------------------------------------------------------------
    // Dual channel header tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_decode_dual_channel_header() {
        let dec = default_decoder();
        let flags = DualChFlags::default(); // ET+EE+EQ, extra_option=2
        let data = make_dual_channel_header(50, &flags);
        let header = dec.decode_dual_channel_header(&data, 0).unwrap();

        assert_eq!(header.block_size, 50);
        assert!(header.time_enabled);
        assert!(header.extras_enabled);
        assert!(header.charge_enabled);
        assert!(!header.samples_enabled);
        assert!(!header.dual_trace);
        assert_eq!(header.extra_option, 2);
        assert_eq!(header.num_samples_wave, 0);
    }

    #[test]
    fn test_dual_channel_enable_flags_all() {
        let dec = default_decoder();
        let flags = DualChFlags {
            dt: true,
            eq: true,
            et: true,
            ee: true,
            es: true,
            extra_option: 2,
            num_samples: 16,
        };
        let data = make_dual_channel_header(100, &flags);
        let header = dec.decode_dual_channel_header(&data, 0).unwrap();

        assert!(header.dual_trace);
        assert!(header.charge_enabled);
        assert!(header.time_enabled);
        assert!(header.extras_enabled);
        assert!(header.samples_enabled);
        assert_eq!(header.num_samples_wave, 16);
    }

    #[test]
    fn test_dual_channel_event_size_minimal() {
        let header = DualChannelHeader {
            block_size: 0,
            num_samples_wave: 0,
            digital_probe1: 0,
            digital_probe2: 0,
            analog_probe: 0,
            extra_option: 2,
            samples_enabled: false,
            extras_enabled: true,
            time_enabled: true,
            charge_enabled: true,
            dual_trace: false,
        };
        assert_eq!(header.event_size_words(), 3); // time + extras + charge
    }

    #[test]
    fn test_dual_channel_event_size_with_waveform() {
        let header = DualChannelHeader {
            block_size: 0,
            num_samples_wave: 4, // 4 * 8 = 32 samples, 4 * 2 = 8 words
            digital_probe1: 0,
            digital_probe2: 0,
            analog_probe: 0,
            extra_option: 2,
            samples_enabled: true,
            extras_enabled: true,
            time_enabled: true,
            charge_enabled: true,
            dual_trace: false,
        };
        assert_eq!(header.event_size_words(), 3 + 8); // 3 + 8 waveform words
    }

    // -----------------------------------------------------------------------
    // Extras word tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_decode_extras_word_option2() {
        // Option 0b010: extended_time + flags + fine_time
        let ext_time: u16 = 0x1234;
        let flags: u8 = 0x2A; // 0b101010
        let fine_time: u16 = 500;
        let word = make_extras_word(ext_time, flags, fine_time);

        let (ext, ft, fl) = decode_extras_word(word, 2);
        assert_eq!(ext, ext_time);
        assert_eq!(ft, fine_time);
        assert_eq!(fl, flags as u32);
    }

    #[test]
    fn test_decode_extras_word_option0() {
        // Option 0b000: extended_time + baseline
        let word: u32 = (0xABCD_u32 << 16) | 0x1234;
        let (ext, ft, fl) = decode_extras_word(word, 0);
        assert_eq!(ext, 0xABCD);
        assert_eq!(ft, 0);
        assert_eq!(fl, 0);
    }

    #[test]
    fn test_decode_extras_word_option1() {
        // Option 0b001: extended_time + flags (16-bit)
        let word: u32 = (0x5678_u32 << 16) | 0x00FF;
        let (ext, ft, fl) = decode_extras_word(word, 1);
        assert_eq!(ext, 0x5678);
        assert_eq!(ft, 0);
        assert_eq!(fl, 0x00FF);
    }

    // -----------------------------------------------------------------------
    // Charge word tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_decode_charge_word() {
        let word = make_charge_word(1000, 500, false);
        let (cl, cs, pu) = decode_charge_word(word);
        assert_eq!(cl, 1000);
        assert_eq!(cs, 500);
        assert!(!pu);
    }

    #[test]
    fn test_decode_charge_word_with_pileup() {
        let word = make_charge_word(2000, 800, true);
        let (cl, cs, pu) = decode_charge_word(word);
        assert_eq!(cl, 2000);
        assert_eq!(cs, 800);
        assert!(pu);
    }

    #[test]
    fn test_decode_charge_word_max_values() {
        let word = make_charge_word(0xFFFF, 0x7FFF, false);
        let (cl, cs, _) = decode_charge_word(word);
        assert_eq!(cl, 0xFFFF);
        assert_eq!(cs, 0x7FFF);
    }

    // -----------------------------------------------------------------------
    // Timestamp tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_calculate_timestamp_basic() {
        let config = Psd1Config::default(); // 2 ns
                                            // trigger_time = 1000, ext = 0, fine = 0
        let ts = calculate_timestamp(&config, 1000, 0, 0);
        assert!((ts - 2000.0).abs() < 0.001); // 1000 * 2 ns
    }

    #[test]
    fn test_calculate_timestamp_with_extended() {
        let config = Psd1Config::default();
        // ext=1, ttt=0 → combined = 1 << 31 = 2^31
        let ts = calculate_timestamp(&config, 0, 1, 0);
        let expected = (1u64 << 31) as f64 * 2.0;
        assert!((ts - expected).abs() < 1.0);
    }

    #[test]
    fn test_calculate_timestamp_with_fine() {
        let config = Psd1Config::default();
        // ttt=0, ext=0, fine=512 → fine_ns = 512 * 2/1024 = 1.0 ns
        let ts = calculate_timestamp(&config, 0, 0, 512);
        assert!((ts - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_calculate_timestamp_combined() {
        let config = Psd1Config::default();
        // ttt=500, ext=0, fine=512
        let ts = calculate_timestamp(&config, 500, 0, 512);
        let expected = 500.0 * 2.0 + 512.0 * (2.0 / 1024.0);
        assert!((ts - expected).abs() < 0.001);
    }

    // -----------------------------------------------------------------------
    // Single event decode test
    // -----------------------------------------------------------------------

    #[test]
    fn test_decode_single_event() {
        let mut dec = default_decoder();

        // Build: board header + channel header + 1 event (3 words)
        let ch_flags = DualChFlags::default(); // ET+EE+EQ, extra_option=2
        let event_words = 3;
        let ch_size = 2 + event_words; // channel header (2) + event (3)
        let total_size = 4 + ch_size; // board header (4) + channel block

        let mut data = make_board_header(total_size as u32, 0x01, 0, 1); // pair 0
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));
        data.extend(make_event(1000, false, 0, 0, 100, 5000, 2000));

        let raw = RawData::new(data);
        let events = dec.decode(&raw);
        assert_eq!(events.len(), 1);

        let e = &events[0];
        assert_eq!(e.channel, 0); // pair=0, flag=0 → ch=0
        assert_eq!(e.energy, 5000);
        assert_eq!(e.energy_short, 2000);
        assert_eq!(e.fine_time, 100);
        assert!(e.waveform.is_none());
    }

    #[test]
    fn test_decode_odd_channel() {
        let mut dec = default_decoder();

        let ch_flags = DualChFlags::default();
        let ch_size = 2 + 3;
        let total_size = 4 + ch_size;

        let mut data = make_board_header(total_size as u32, 0x01, 0, 1); // pair 0
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));
        data.extend(make_event(1000, true, 0, 0, 0, 100, 50)); // odd=true

        let raw = RawData::new(data);
        let events = dec.decode(&raw);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].channel, 1); // pair=0, flag=1 → ch=1
    }

    #[test]
    fn test_decode_channel_pair_offset() {
        let mut dec = default_decoder();

        // Pair 2 → channels 4, 5
        let ch_flags = DualChFlags::default();
        let ch_size = 2 + 3;
        let total_size = 4 + ch_size;

        let mut data = make_board_header(total_size as u32, 0x04, 0, 1); // mask=0x04 → pair 2
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));
        data.extend(make_event(1000, false, 0, 0, 0, 100, 50));

        let raw = RawData::new(data);
        let events = dec.decode(&raw);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].channel, 4); // pair=2, flag=0 → ch=4
    }

    #[test]
    fn test_decode_event_flags() {
        let mut dec = default_decoder();

        let ch_flags = DualChFlags::default();
        let ch_size = 2 + 3;
        let total_size = 4 + ch_size;

        let mut data = make_board_header(total_size as u32, 0x01, 0, 1);
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));
        // flags = 0x2A (0b101010): trigger_lost + 1024_triggers
        data.extend(make_event(1000, false, 0, 0x2A, 0, 100, 50));

        let raw = RawData::new(data);
        let events = dec.decode(&raw);
        assert_eq!(events[0].flags, 0x2A);
    }

    #[test]
    fn test_decode_pileup_flag() {
        let mut dec = default_decoder();

        let ch_flags = DualChFlags::default();
        let ch_size = 2 + 3;
        let total_size = 4 + ch_size;

        let mut data = make_board_header(total_size as u32, 0x01, 0, 1);
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));
        // Event with pileup: build manually
        push_u32(&mut data, make_time_word(1000, false));
        push_u32(&mut data, make_extras_word(0, 0, 0));
        push_u32(&mut data, make_charge_word(100, 50, true)); // pileup=true

        let raw = RawData::new(data);
        let events = dec.decode(&raw);
        assert_ne!(events[0].flags & (1 << 15), 0); // pileup at bit 15
    }

    // -----------------------------------------------------------------------
    // Multiple events
    // -----------------------------------------------------------------------

    #[test]
    fn test_decode_multiple_events_in_pair() {
        let mut dec = default_decoder();

        let ch_flags = DualChFlags::default();
        let events_count = 3;
        let ch_size = 2 + events_count * 3; // 2 header + 3 events * 3 words
        let total_size = 4 + ch_size;

        let mut data = make_board_header(total_size as u32, 0x01, 0, 1);
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));

        for i in 0..events_count {
            let odd = i % 2 == 1;
            data.extend(make_event(
                (i as u32 + 1) * 1000,
                odd,
                0,
                0,
                0,
                (i as u16 + 1) * 100,
                (i as u16 + 1) * 50,
            ));
        }

        let raw = RawData::new(data);
        let events = dec.decode(&raw);
        assert_eq!(events.len(), 3);

        // Check channels alternate: 0, 1, 0
        assert_eq!(events[0].channel, 0);
        assert_eq!(events[1].channel, 1);
        assert_eq!(events[2].channel, 0);
    }

    #[test]
    fn test_decode_multiple_channel_pairs() {
        let mut dec = default_decoder();

        let ch_flags = DualChFlags::default();
        let ch_size = 2 + 3; // 1 event per pair
        let total_size = 4 + ch_size * 2; // 2 pairs

        let mut data = make_board_header(total_size as u32, 0x03, 0, 1); // mask=0x03 → pairs 0,1

        // Pair 0
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));
        data.extend(make_event(2000, false, 0, 0, 0, 200, 100));

        // Pair 1
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));
        data.extend(make_event(1000, true, 0, 0, 0, 300, 150));

        let raw = RawData::new(data);
        let events = dec.decode(&raw);
        assert_eq!(events.len(), 2);

        // Events sorted by timestamp: pair1(1000*2=2000ns) < pair0(2000*2=4000ns)
        assert_eq!(events[0].channel, 3); // pair=1, flag=1
        assert_eq!(events[1].channel, 0); // pair=0, flag=0
    }

    #[test]
    fn test_decode_multiple_board_aggregates() {
        let mut dec = default_decoder();

        let ch_flags = DualChFlags::default();
        let ch_size = 2 + 3;
        let block_size = 4 + ch_size;

        // Two board aggregates concatenated
        let mut data = Vec::new();

        // Block 1
        data.extend(make_board_header(block_size as u32, 0x01, 0, 1));
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));
        data.extend(make_event(1000, false, 0, 0, 0, 100, 50));

        // Block 2
        data.extend(make_board_header(block_size as u32, 0x01, 0, 2));
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));
        data.extend(make_event(2000, false, 0, 0, 0, 200, 100));

        let raw = RawData::new(data);
        let events = dec.decode(&raw);
        assert_eq!(events.len(), 2);
        // Sorted by timestamp
        assert_eq!(events[0].energy, 100);
        assert_eq!(events[1].energy, 200);
    }

    #[test]
    fn test_events_sorted_by_timestamp() {
        let mut dec = default_decoder();

        let ch_flags = DualChFlags::default();
        let ch_size = 2 + 3 * 2; // 2 events
        let total_size = 4 + ch_size;

        let mut data = make_board_header(total_size as u32, 0x01, 0, 1);
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));
        // Event with later time first
        data.extend(make_event(5000, false, 0, 0, 0, 500, 250));
        data.extend(make_event(1000, false, 0, 0, 0, 100, 50));

        let raw = RawData::new(data);
        let events = dec.decode(&raw);
        assert_eq!(events.len(), 2);
        assert!(events[0].timestamp_ns < events[1].timestamp_ns);
        assert_eq!(events[0].energy, 100); // Earlier event first
    }

    // -----------------------------------------------------------------------
    // Timestamp extended tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_decode_timestamp_with_extended_time() {
        let mut dec = default_decoder();

        let ch_flags = DualChFlags::default();
        let ch_size = 2 + 3;
        let total_size = 4 + ch_size;

        let mut data = make_board_header(total_size as u32, 0x01, 0, 1);
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));
        // ext_time=1, ttt=0 → combined = 2^31
        data.extend(make_event(0, false, 1, 0, 0, 100, 50));

        let raw = RawData::new(data);
        let events = dec.decode(&raw);
        assert_eq!(events.len(), 1);

        let expected = (1u64 << 31) as f64 * 2.0;
        assert!((events[0].timestamp_ns - expected).abs() < 1.0);
    }

    // -----------------------------------------------------------------------
    // Waveform tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_decode_waveform_basic() {
        let mut dec = default_decoder();

        let num_samples_wave: u16 = 1; // 1 * 8 = 8 samples, 1 * 2 = 2 words
        let ch_flags = DualChFlags {
            es: true,
            num_samples: num_samples_wave,
            ..Default::default()
        };
        let waveform_words = num_samples_wave as usize * 2;
        let ch_size = 2 + 3 + waveform_words; // header + event(time+extras+charge) + waveform
        let total_size = 4 + ch_size;

        let mut data = make_board_header(total_size as u32, 0x01, 0, 1);
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));

        // Time + Extras (before waveform)
        push_u32(&mut data, make_time_word(100, false));
        push_u32(&mut data, make_extras_word(0, 0, 0));

        // Waveform: 2 words = 4 samples
        // Word 0: sample0=100, sample1=200
        let wf_word0: u32 = 100 | (200 << 16);
        push_u32(&mut data, wf_word0);
        // Word 1: sample2=300, sample3=400
        let wf_word1: u32 = 300 | (400 << 16);
        push_u32(&mut data, wf_word1);

        // Charge (after waveform)
        push_u32(&mut data, make_charge_word(500, 250, false));

        let raw = RawData::new(data);
        let events = dec.decode(&raw);
        assert_eq!(events.len(), 1);

        let wf = events[0].waveform.as_ref().unwrap();
        assert_eq!(wf.analog_probe1.len(), 4);
        assert_eq!(wf.analog_probe1[0], 100);
        assert_eq!(wf.analog_probe1[1], 200);
        assert_eq!(wf.analog_probe1[2], 300);
        assert_eq!(wf.analog_probe1[3], 400);
    }

    #[test]
    fn test_decode_waveform_digital_probes() {
        let mut dec = default_decoder();

        let num_samples_wave: u16 = 1;
        let ch_flags = DualChFlags {
            es: true,
            num_samples: num_samples_wave,
            ..Default::default()
        };
        let waveform_words = num_samples_wave as usize * 2;
        let ch_size = 2 + 3 + waveform_words;
        let total_size = 4 + ch_size;

        let mut data = make_board_header(total_size as u32, 0x01, 0, 1);
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));

        push_u32(&mut data, make_time_word(100, false));
        push_u32(&mut data, make_extras_word(0, 0, 0));

        // Waveform with digital probes set
        // Lower: analog=50, DP1=1 (bit14), DP2=0 (bit15)
        // Upper: analog=60, DP1=0 (bit30), DP2=1 (bit31)
        let wf_word: u32 = 50 | (1 << 14) | (60 << 16) | (1 << 31);
        push_u32(&mut data, wf_word);
        push_u32(&mut data, 0); // Second waveform word

        push_u32(&mut data, make_charge_word(100, 50, false));

        let raw = RawData::new(data);
        let events = dec.decode(&raw);
        let wf = events[0].waveform.as_ref().unwrap();

        assert_eq!(wf.digital_probe1[0], 1); // s1 dp1
        assert_eq!(wf.digital_probe2[0], 0); // s1 dp2
        assert_eq!(wf.digital_probe1[1], 0); // s2 dp1
        assert_eq!(wf.digital_probe2[1], 1); // s2 dp2
    }

    #[test]
    fn test_decode_waveform_dual_trace() {
        let mut dec = default_decoder();

        let num_samples_wave: u16 = 1;
        let ch_flags = DualChFlags {
            dt: true,
            es: true,
            num_samples: num_samples_wave,
            ..Default::default()
        };
        let waveform_words = num_samples_wave as usize * 2;
        let ch_size = 2 + 3 + waveform_words;
        let total_size = 4 + ch_size;

        let mut data = make_board_header(total_size as u32, 0x01, 0, 1);
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));

        push_u32(&mut data, make_time_word(100, false));
        push_u32(&mut data, make_extras_word(0, 0, 0));

        // Dual trace: even=probe1, odd=probe2
        // Word 0: lower(probe1)=100, upper(probe2)=200
        let wf_word0: u32 = 100 | (200 << 16);
        push_u32(&mut data, wf_word0);
        // Word 1: lower(probe1)=300, upper(probe2)=400
        let wf_word1: u32 = 300 | (400 << 16);
        push_u32(&mut data, wf_word1);

        push_u32(&mut data, make_charge_word(100, 50, false));

        let raw = RawData::new(data);
        let events = dec.decode(&raw);
        let wf = events[0].waveform.as_ref().unwrap();

        assert_eq!(wf.analog_probe1.len(), 2); // Even samples
        assert_eq!(wf.analog_probe2.len(), 2); // Odd samples
        assert_eq!(wf.analog_probe1[0], 100);
        assert_eq!(wf.analog_probe2[0], 200);
        assert_eq!(wf.analog_probe1[1], 300);
        assert_eq!(wf.analog_probe2[1], 400);
    }

    // -----------------------------------------------------------------------
    // Module ID test
    // -----------------------------------------------------------------------

    #[test]
    fn test_module_id_propagation() {
        let mut dec = Psd1Decoder::new(Psd1Config {
            time_step_ns: 2.0,
            module_id: 7,
            dump_enabled: false,
        });

        let ch_flags = DualChFlags::default();
        let ch_size = 2 + 3;
        let total_size = 4 + ch_size;

        let mut data = make_board_header(total_size as u32, 0x01, 0, 1);
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));
        data.extend(make_event(1000, false, 0, 0, 0, 100, 50));

        let raw = RawData::new(data);
        let events = dec.decode(&raw);
        assert_eq!(events[0].module, 7);
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_decode_empty_data() {
        let mut dec = default_decoder();
        let raw = RawData::new(vec![]);
        let events = dec.decode(&raw);
        assert!(events.is_empty());
    }

    #[test]
    fn test_decode_invalid_header() {
        let mut dec = default_decoder();
        let mut data = vec![0u8; 16];
        // Type = 0xB (invalid)
        data[..4].copy_from_slice(&0xB000_0004u32.to_le_bytes());
        let raw = RawData::new(data);
        let events = dec.decode(&raw);
        assert!(events.is_empty());
    }

    #[test]
    fn test_decode_charge_only_event() {
        // Only EQ enabled, no time/extras
        let mut dec = default_decoder();

        let ch_flags = DualChFlags {
            et: false,
            ee: false,
            eq: true,
            ..Default::default()
        };
        let ch_size = 2 + 1; // header + 1 word (charge only)
        let total_size = 4 + ch_size;

        let mut data = make_board_header(total_size as u32, 0x01, 0, 1);
        data.extend(make_dual_channel_header(ch_size as u32, &ch_flags));
        push_u32(&mut data, make_charge_word(999, 444, false));

        let raw = RawData::new(data);
        let events = dec.decode(&raw);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].energy, 999);
        assert_eq!(events[0].energy_short, 444);
        assert_eq!(events[0].channel, 0); // No time word → channel_flag=0
        assert!((events[0].timestamp_ns).abs() < 0.001); // No time → 0
    }

    // -----------------------------------------------------------------------
    // Constants tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_constants_word_size() {
        assert_eq!(constants::WORD_SIZE, 4);
    }

    #[test]
    fn test_constants_header_type() {
        assert_eq!(constants::board_header::TYPE_DATA, 0xA);
    }
}
