//! CAEN Digitizer Info - Sample program to test FFI bindings
//!
//! Usage: cargo run --bin caen_info -- <url> [--decode]
//! Example: cargo run --bin caen_info -- dig2://172.18.4.56
//! Example: cargo run --bin caen_info -- dig2://172.18.4.56 --decode

use delila_rs::reader::decoder::{Psd2Config, Psd2Decoder, RawData as DecoderRawData};
use delila_rs::reader::CaenHandle;
use std::env;
use std::thread;
use std::time::Duration;

fn main() {
    // Get URL from command line arguments
    let args: Vec<String> = env::args().collect();
    let url = if args.len() > 1 && !args[1].starts_with("--") {
        &args[1]
    } else {
        "dig2://172.18.4.56" // Default URL from PSD2.conf
    };

    // Check for flags
    let decode_enabled = args.iter().any(|a| a == "--decode");
    let devtree_enabled = args.iter().any(|a| a == "--devtree");

    println!("===========================================");
    println!("CAEN Digitizer Info");
    println!("===========================================");
    println!("Connecting to: {}", url);
    if decode_enabled {
        println!("Decode mode: ENABLED");
    }
    if devtree_enabled {
        println!("DevTree mode: ENABLED (will save to devtree.json)");
    }
    println!();

    // Open connection to digitizer
    let handle = match CaenHandle::open(url) {
        Ok(h) => {
            println!("[OK] Connected successfully");
            h
        }
        Err(e) => {
            eprintln!("[ERROR] Failed to connect: {}", e);
            std::process::exit(1);
        }
    };

    // Get basic device info
    println!();
    println!("--- Device Information ---");

    let info_paths = [
        "/par/ModelName",
        "/par/SerialNum",
        "/par/FwType",
        "/par/FPGA_FwVer",
        "/par/NumCh",
        "/par/ADC_SamplRate",
        "/par/ADC_Nbit",
        "/par/FormFactor",
    ];

    for path in &info_paths {
        match handle.get_value(path) {
            Ok(value) => {
                let name = path.split('/').next_back().unwrap_or(path);
                println!("  {:<20}: {}", name, value);
            }
            Err(e) => {
                eprintln!("  {}: Error - {}", path, e);
            }
        }
    }

    // Get and save DevTree if requested
    if devtree_enabled {
        println!();
        println!("--- DevTree ---");
        match handle.get_device_tree() {
            Ok(tree) => {
                let filename = "devtree.json";
                match std::fs::write(filename, &tree) {
                    Ok(_) => println!("  [OK] DevTree saved to {}", filename),
                    Err(e) => eprintln!("  [ERROR] Failed to save DevTree: {}", e),
                }
            }
            Err(e) => {
                eprintln!("  [ERROR] Failed to get DevTree: {}", e);
            }
        }
        println!();
        println!("===========================================");
        println!("Done (DevTree mode).");
        println!("===========================================");
        return;
    }

    // Test parameter setting (read-modify-verify cycle)
    println!();
    println!("--- Parameter Set Test ---");
    test_parameter_setting(&handle);

    // Test command sending
    println!();
    println!("--- Command Test ---");
    test_commands(&handle);

    // Test data readout (with optional decoding)
    println!();
    println!("--- Data Readout Test ---");
    test_data_readout(&handle, decode_enabled);

    println!();
    println!("===========================================");
    println!("Done. Handle will be automatically closed.");
    println!("===========================================");

    // Handle is automatically closed when it goes out of scope (Drop trait)
}

/// Test parameter read/write cycle
fn test_parameter_setting(handle: &CaenHandle) {
    // Test 1: Channel enable (boolean parameter)
    println!();
    println!("Test 1: Channel 0 Enable (Boolean)");
    test_param_cycle(handle, "/ch/0/par/ChEnable", &["True", "False", "True"]);

    // Test 2: DC Offset (numeric parameter)
    println!();
    println!("Test 2: Channel 0 DC Offset (Numeric)");
    test_param_cycle(handle, "/ch/0/par/DCOffset", &["20", "50", "20"]);

    // Test 3: Trigger threshold (numeric parameter)
    println!();
    println!("Test 3: Channel 0 Trigger Threshold (Numeric)");
    test_param_cycle(handle, "/ch/0/par/TriggerThr", &["500", "1000", "500"]);

    // Test 4: Pulse polarity (enum parameter)
    println!();
    println!("Test 4: Channel 0 Pulse Polarity (Enum)");
    test_param_cycle(
        handle,
        "/ch/0/par/PulsePolarity",
        &["Negative", "Positive", "Negative"],
    );

    // Test 5: Global parameter - StartSource
    println!();
    println!("Test 5: Start Source (Global Enum)");
    test_param_cycle(
        handle,
        "/par/StartSource",
        &["SWcmd", "EncodedClkIn", "SWcmd"],
    );

    // Test 6: Gate lengths
    println!();
    println!("Test 6: Gate Long Length (Numeric with unit)");
    test_param_cycle(handle, "/ch/0/par/GateLongLengthT", &["400", "800", "400"]);
}

/// Helper function to test read-modify-verify cycle
fn test_param_cycle(handle: &CaenHandle, path: &str, values: &[&str]) {
    let param_name = path.split('/').next_back().unwrap_or(path);

    // Read original value
    let original = match handle.get_value(path) {
        Ok(v) => {
            println!("  [READ]  {}: {}", param_name, v);
            v
        }
        Err(e) => {
            println!("  [ERROR] Failed to read {}: {}", param_name, e);
            return;
        }
    };

    // Test each value in sequence
    for &new_value in values {
        // Set new value
        match handle.set_value(path, new_value) {
            Ok(()) => {
                println!("  [SET]   {} = {}", param_name, new_value);
            }
            Err(e) => {
                println!(
                    "  [ERROR] Failed to set {} = {}: {}",
                    param_name, new_value, e
                );
                continue;
            }
        }

        // Verify the value was set correctly
        match handle.get_value(path) {
            Ok(v) => {
                if v == new_value {
                    println!("  [VERIFY] OK: {} == {}", param_name, v);
                } else {
                    println!("  [VERIFY] MISMATCH: expected {}, got {}", new_value, v);
                }
            }
            Err(e) => {
                println!("  [ERROR] Failed to verify {}: {}", param_name, e);
            }
        }
    }

    // Restore original value
    if let Err(e) = handle.set_value(path, &original) {
        println!("  [WARN]  Failed to restore original value: {}", e);
    } else {
        println!("  [RESTORE] {} = {}", param_name, original);
    }
}

/// Test command sending
fn test_commands(handle: &CaenHandle) {
    // Test Reset command
    println!("  Sending /cmd/Reset...");
    match handle.send_command("/cmd/Reset") {
        Ok(()) => println!("  [OK] Reset command successful"),
        Err(e) => println!("  [ERROR] Reset failed: {}", e),
    }

    // Test ClearData command
    println!("  Sending /cmd/ClearData...");
    match handle.send_command("/cmd/ClearData") {
        Ok(()) => println!("  [OK] ClearData command successful"),
        Err(e) => println!("  [ERROR] ClearData failed: {}", e),
    }
}

/// Test data readout using the EndpointHandle
fn test_data_readout(handle: &CaenHandle, decode_enabled: bool) {
    // Step 1: Configure endpoint for RAW data
    println!("  Configuring RAW endpoint...");
    let endpoint = match handle.configure_endpoint() {
        Ok(ep) => {
            println!("  [OK] Endpoint configured (handle: {})", ep.raw());
            ep
        }
        Err(e) => {
            println!("  [ERROR] Failed to configure endpoint: {}", e);
            return;
        }
    };

    // Step 2: Arm acquisition
    println!("  Arming acquisition...");
    if let Err(e) = handle.send_command("/cmd/ArmAcquisition") {
        println!("  [ERROR] ArmAcquisition failed: {}", e);
        return;
    }
    println!("  [OK] Acquisition armed");

    // Step 3: Start acquisition (software start)
    println!("  Starting acquisition (SW command)...");
    if let Err(e) = handle.send_command("/cmd/SwStartAcquisition") {
        println!("  [ERROR] SwStartAcquisition failed: {}", e);
        let _ = handle.send_command("/cmd/DisarmAcquisition");
        return;
    }
    println!("  [OK] Acquisition started");

    // Step 4: Wait a bit for data to accumulate
    println!("  Waiting 500ms for data...");
    thread::sleep(Duration::from_millis(500));

    // Step 4.5: Test has_data first
    println!("  Checking if data is available (HasData)...");
    match endpoint.has_data(100) {
        Ok(true) => println!("  [OK] Data is available"),
        Ok(false) => println!("  [INFO] No data available yet (timeout)"),
        Err(e) => println!("  [ERROR] HasData failed: {}", e),
    }

    // Create decoder if enabled
    let mut decoder = if decode_enabled {
        let config = Psd2Config {
            time_step_ns: 2.0, // 500 MS/s -> 2ns
            module_id: 0,
            dump_enabled: true, // Enable dump for debugging
        };
        Some(Psd2Decoder::new(config))
    } else {
        None
    };

    // Step 5: Read data (with timeout)
    const BUFFER_SIZE: usize = 1024 * 1024; // 1MB buffer
    const TIMEOUT_MS: i32 = 1000; // 1 second timeout
    const MAX_READS: usize = 5; // Read up to 5 times

    println!(
        "  Reading data (buffer: {} bytes, timeout: {}ms)...",
        BUFFER_SIZE, TIMEOUT_MS
    );

    let mut total_bytes = 0usize;
    let mut total_events = 0u32;
    let mut read_count = 0usize;
    let mut decoded_events_count = 0usize;

    for i in 0..MAX_READS {
        match endpoint.read_data(TIMEOUT_MS, BUFFER_SIZE) {
            Ok(Some(raw_data)) => {
                println!(
                    "  [READ {}] size: {} bytes, n_events: {}",
                    i + 1,
                    raw_data.size,
                    raw_data.n_events
                );
                total_bytes += raw_data.size;
                total_events += raw_data.n_events;
                read_count += 1;

                // Show first few bytes as hex
                if raw_data.size > 0 && !decode_enabled {
                    let preview_len = std::cmp::min(32, raw_data.size);
                    let hex_preview: String = raw_data.data[..preview_len]
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<Vec<_>>()
                        .join(" ");
                    println!("           first {} bytes: {}", preview_len, hex_preview);
                }

                // Decode if enabled
                if let Some(ref mut dec) = decoder {
                    println!();
                    let decoder_raw = DecoderRawData {
                        data: raw_data.data,
                        size: raw_data.size,
                        n_events: raw_data.n_events,
                    };
                    let events = dec.decode(&decoder_raw);

                    if !events.is_empty() {
                        println!("  --- Decoded Events ({}) ---", events.len());
                        for (j, event) in events.iter().enumerate().take(10) {
                            println!("    Event {:3}: {}", j, event);
                        }
                        if events.len() > 10 {
                            println!("    ... and {} more events", events.len() - 10);
                        }
                        decoded_events_count += events.len();
                    }
                    println!();
                }
            }
            Ok(None) => {
                println!("  [READ {}] Timeout - no data available", i + 1);
                break;
            }
            Err(e) => {
                println!("  [ERROR] Read failed: {}", e);
                break;
            }
        }
    }

    // Step 6: Stop acquisition
    println!("  Stopping acquisition...");
    if let Err(e) = handle.send_command("/cmd/SwStopAcquisition") {
        println!("  [WARN] SwStopAcquisition failed: {}", e);
    }

    // Step 7: Disarm acquisition
    println!("  Disarming acquisition...");
    if let Err(e) = handle.send_command("/cmd/DisarmAcquisition") {
        println!("  [WARN] DisarmAcquisition failed: {}", e);
    }

    // Summary
    println!();
    println!("  --- Summary ---");
    println!("  Total reads:       {}", read_count);
    println!("  Total bytes:       {}", total_bytes);
    println!("  Total events (HW): {}", total_events);
    if decode_enabled {
        println!("  Decoded events:    {}", decoded_events_count);
    }

    if total_events > 0 {
        println!("  [OK] Data readout successful!");
    } else {
        println!("  [WARN] No events received (check trigger settings or signal input)");
    }
}
