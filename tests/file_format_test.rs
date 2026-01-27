//! E2E tests for DELILA file format (write → read → verify)
//!
//! All scalar fields are generated from seeded random numbers.
//! A checksum over (module, channel, energy, energy_short, timestamp_ns)
//! is stored in `flags`. On read-back the checksum is recomputed and compared.

use std::io::{Cursor, Write};

use delila_rs::common::{EventData, EventDataBatch};
use delila_rs::recorder::{ChecksumCalculator, DataFileReader, FileFooter, FileHeader};
use rand::prelude::*;
use rand::rngs::StdRng;

/// Compute a checksum over all scalar fields except `flags`.
/// Bit-shifts prevent trivial XOR cancellation between narrow fields.
fn compute_checksum(ev: &EventData) -> u64 {
    let ts = ev.timestamp_ns.to_bits();
    (ev.module as u64)
        ^ ((ev.channel as u64) << 8)
        ^ ((ev.energy as u64) << 16)
        ^ ((ev.energy_short as u64) << 32)
        ^ ts
}

/// Verify that every event in a reader passes the checksum.
/// Returns the total number of events checked.
fn verify_all_checksums<R: std::io::Read + std::io::Seek>(
    reader: &mut DataFileReader<R>,
) -> u64 {
    let mut count = 0u64;
    for batch_result in reader.data_blocks() {
        let batch = batch_result.expect("read batch");
        for ev in &batch.events {
            let expected = compute_checksum(ev);
            assert_eq!(
                ev.flags, expected,
                "checksum mismatch at event {} (mod={}, ch={}, e={}, es={}, ts={})",
                count, ev.module, ev.channel, ev.energy, ev.energy_short, ev.timestamp_ns,
            );
            count += 1;
        }
    }
    count
}

/// Create a random test event with the checksum embedded in `flags`.
fn make_random_event(rng: &mut StdRng) -> EventData {
    let module: u8 = rng.gen();
    let channel: u8 = rng.gen();
    let energy: u16 = rng.gen();
    let energy_short: u16 = rng.gen();
    // Positive timestamps only (realistic range: 0 .. 1e18 ns ≈ 31 years)
    let timestamp_ns: f64 = rng.gen_range(0.0..1e15);

    let mut ev = EventData::new(module, channel, energy, energy_short, timestamp_ns, 0);
    ev.flags = compute_checksum(&ev);
    ev
}

/// Write a complete .delila file (header + batches + footer) into a Vec<u8>.
fn write_file(header: &FileHeader, batches: &[EventDataBatch]) -> Vec<u8> {
    let mut buf = Vec::new();

    // Header
    header.write_to(&mut buf).expect("write header");

    // Data blocks + checksum + footer stats
    let mut checksum = ChecksumCalculator::new();
    let mut footer = FileFooter::new();
    let mut total_events = 0u64;

    for batch in batches {
        let data = batch.to_msgpack().expect("serialize batch");
        let len_bytes = (data.len() as u32).to_le_bytes();

        buf.write_all(&len_bytes).unwrap();
        buf.write_all(&data).unwrap();

        checksum.update(&len_bytes);
        checksum.update(&data);

        for ev in &batch.events {
            footer.update_timestamp_range(ev.timestamp_ns, ev.timestamp_ns);
        }
        total_events += batch.events.len() as u64;
    }

    footer.total_events = total_events;
    footer.data_checksum = checksum.finalize();
    footer.data_bytes = checksum.bytes_processed();
    footer.finalize();

    footer.write_to(&mut buf).expect("write footer");
    buf
}

// ---------------------------------------------------------------------------
// Test 1: Single-batch roundtrip with random events
// ---------------------------------------------------------------------------

#[test]
fn test_write_read_roundtrip() {
    let header = FileHeader::new(1, "E2E_Test".to_string(), 0);
    let mut rng = StdRng::seed_from_u64(42);

    let mut batch = EventDataBatch::new(0, 0);
    for _ in 0..500 {
        batch.push(make_random_event(&mut rng));
    }

    let file_bytes = write_file(&header, &[batch]);
    let cursor = Cursor::new(file_bytes);
    let mut reader = DataFileReader::new(cursor).expect("open file");

    // Verify header
    let h = reader.header().expect("header present");
    assert_eq!(h.run_number, 1);
    assert_eq!(h.exp_name, "E2E_Test");

    // Verify all checksums
    let count = verify_all_checksums(&mut reader);
    assert_eq!(count, 500);
}

// ---------------------------------------------------------------------------
// Test 2: Corruption detection via file-level checksum
// ---------------------------------------------------------------------------

#[test]
fn test_checksum_detects_corruption() {
    let header = FileHeader::new(2, "Corrupt".to_string(), 0);
    let mut rng = StdRng::seed_from_u64(99);

    let mut batch = EventDataBatch::new(0, 0);
    for _ in 0..10 {
        batch.push(make_random_event(&mut rng));
    }
    let mut file_bytes = write_file(&header, &[batch]);

    // Find data region start
    let cursor = Cursor::new(file_bytes.clone());
    let reader = DataFileReader::new(cursor).expect("open file");
    let header_ref = reader.header().unwrap();
    let header_bytes = header_ref.to_bytes().expect("header bytes");
    let data_start = header_bytes.len();

    // Flip a byte in the data region
    let corrupt_pos = data_start + 20;
    file_bytes[corrupt_pos] ^= 0xFF;

    // The file-level checksum should now fail
    let cursor2 = Cursor::new(file_bytes);
    let mut reader2 = DataFileReader::new(cursor2).expect("open corrupted file");
    let result = reader2.validate();
    assert!(
        !result.is_valid,
        "corrupted file should not validate as valid"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Multiple batches with different source_ids
// ---------------------------------------------------------------------------

#[test]
fn test_multiple_batches_roundtrip() {
    let header = FileHeader::new(3, "MultiBatch".to_string(), 0);
    let mut rng = StdRng::seed_from_u64(7);

    let batch_specs: Vec<(u32, u64, usize)> = vec![
        (0, 0, 50), // source 0, seq 0, 50 events
        (1, 0, 30), // source 1, seq 0, 30 events
        (0, 1, 20), // source 0, seq 1, 20 events
    ];

    let batches: Vec<EventDataBatch> = batch_specs
        .iter()
        .map(|&(src, seq, n)| {
            let mut b = EventDataBatch::new(src, seq);
            for _ in 0..n {
                b.push(make_random_event(&mut rng));
            }
            b
        })
        .collect();

    let file_bytes = write_file(&header, &batches);
    let cursor = Cursor::new(file_bytes);
    let mut reader = DataFileReader::new(cursor).expect("open file");

    let mut total_events = 0u64;
    let mut batch_count = 0usize;
    let mut source_ids = Vec::new();

    for batch_result in reader.data_blocks() {
        let batch = batch_result.expect("read batch");
        source_ids.push(batch.source_id);
        for ev in &batch.events {
            assert_eq!(ev.flags, compute_checksum(ev));
        }
        total_events += batch.events.len() as u64;
        batch_count += 1;
    }

    assert_eq!(batch_count, 3);
    assert_eq!(total_events, 100); // 50 + 30 + 20
    assert_eq!(source_ids, vec![0, 1, 0]);
}

// ---------------------------------------------------------------------------
// Test 4: Footer statistics accuracy
// ---------------------------------------------------------------------------

#[test]
fn test_footer_statistics() {
    let header = FileHeader::new(4, "FooterTest".to_string(), 0);
    let mut rng = StdRng::seed_from_u64(123);

    let timestamps = [100.5, 200.0, 300.75, 50.25, 999.9];
    let mut batch = EventDataBatch::new(0, 0);
    for &ts in &timestamps {
        // Random energy/energy_short, fixed timestamps for footer range check
        let mut ev = EventData::new(0, rng.gen(), rng.gen(), rng.gen(), ts, 0);
        ev.flags = compute_checksum(&ev);
        batch.push(ev);
    }

    let file_bytes = write_file(&header, &[batch]);

    // Read footer directly
    let cursor = Cursor::new(file_bytes.clone());
    let mut reader = DataFileReader::new(cursor).expect("open file");
    let footer = reader.read_footer().expect("read footer");

    // Verify total_events
    assert_eq!(footer.total_events, 5);

    // Verify timestamp range
    assert!(
        (footer.first_event_time_ns - 50.25).abs() < f64::EPSILON,
        "first_event_time_ns: expected 50.25, got {}",
        footer.first_event_time_ns,
    );
    assert!(
        (footer.last_event_time_ns - 999.9).abs() < f64::EPSILON,
        "last_event_time_ns: expected 999.9, got {}",
        footer.last_event_time_ns,
    );

    // Verify completion flag
    assert!(footer.is_complete());

    // Verify data_bytes > 0
    assert!(footer.data_bytes > 0);

    // Verify file-level validation passes
    let cursor2 = Cursor::new(file_bytes);
    let mut reader2 = DataFileReader::new(cursor2).expect("open file");
    let result = reader2.validate();
    assert!(result.is_valid, "file should validate: {:?}", result.errors);

    // Verify footer matches what validate found
    let vfooter = result.footer.expect("footer in validation result");
    assert_eq!(vfooter.total_events, 5);
    assert_eq!(result.recoverable_blocks, 1);
    assert_eq!(result.recoverable_events, 5);
}
