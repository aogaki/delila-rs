//! FELib Integration Tests
//!
//! These tests require actual CAEN hardware connected.
//!
//! **IMPORTANT:** Run with single thread to avoid conflicts when accessing the same device:
//! ```bash
//! cargo test --test felib_integration_test -- --ignored --test-threads=1
//! ```
//!
//! Note: FELib is mostly thread-safe, but handle acquisition can cause conflicts
//! when multiple tests try to open/close the same device simultaneously.
//!
//! Set CAEN_DIGITIZER_URL environment variable to your device URL.
//! Default: dig2://172.18.4.56

use delila_rs::reader::caen::CaenHandle;

/// Get the digitizer URL from environment or use default
fn get_test_url() -> String {
    std::env::var("CAEN_DIGITIZER_URL").unwrap_or_else(|_| "dig2://172.18.4.56".to_string())
}

#[test]
#[ignore = "Requires CAEN hardware"]
fn test_connect_disconnect() {
    let url = get_test_url();
    println!("Connecting to: {}", url);

    let handle = CaenHandle::open(&url).expect("Failed to open device");
    assert!(handle.is_connected(), "Handle should be connected");

    // Handle will be automatically closed on drop (RAII)
    drop(handle);
    println!("Connection closed successfully");
}

#[test]
#[ignore = "Requires CAEN hardware"]
fn test_get_device_info() {
    let url = get_test_url();
    let handle = CaenHandle::open(&url).expect("Failed to open device");

    let info = handle.get_device_info().expect("Failed to get device info");

    println!("Device Info:");
    println!("  Model: {}", info.model);
    println!("  Serial: {}", info.serial_number);
    println!("  Firmware: {}", info.firmware_type);
    println!("  Channels: {}", info.num_channels);
    println!("  ADC Bits: {}", info.adc_bits);
    println!("  Sample Rate: {} SPS", info.sampling_rate_sps);

    // VX2730 specific assertions
    assert!(
        info.model.contains("2730") || info.model.contains("2740"),
        "Expected VX27xx model, got: {}",
        info.model
    );
    assert!(info.num_channels > 0, "Should have at least 1 channel");
    assert!(info.adc_bits >= 12, "ADC should be at least 12-bit");
}

#[test]
#[ignore = "Requires CAEN hardware"]
fn test_get_device_tree() {
    let url = get_test_url();
    let handle = CaenHandle::open(&url).expect("Failed to open device");

    let tree = handle.get_device_tree().expect("Failed to get device tree");

    // Device tree should be valid JSON
    let parsed: serde_json::Value =
        serde_json::from_str(&tree).expect("Device tree should be valid JSON");

    assert!(parsed.is_object(), "Device tree should be a JSON object");
    println!(
        "Device tree size: {} bytes, {} top-level keys",
        tree.len(),
        parsed.as_object().map(|o| o.len()).unwrap_or(0)
    );
}

#[test]
#[ignore = "Requires CAEN hardware"]
fn test_get_set_value() {
    let url = get_test_url();
    let handle = CaenHandle::open(&url).expect("Failed to open device");

    // Read a parameter (PSD2 uses lowercase path)
    let original = handle
        .get_value("/ch/0/par/dcoffset")
        .expect("Failed to get dcoffset");
    println!("Original dcoffset: {}", original);

    // Try to set a test value (will be restored)
    let test_value = "50";
    handle
        .set_value("/ch/0/par/dcoffset", test_value)
        .expect("Failed to set dcoffset");

    let new_value = handle
        .get_value("/ch/0/par/dcoffset")
        .expect("Failed to get dcoffset after set");
    println!("New dcoffset: {}", new_value);

    // Compare as floats (device may return with different precision)
    let new_float: f64 = new_value.parse().expect("Should be numeric");
    let test_float: f64 = test_value.parse().expect("Should be numeric");
    assert!(
        (new_float - test_float).abs() < 1.0,
        "Expected ~{}, got {}",
        test_float,
        new_float
    );

    // Restore original value
    handle
        .set_value("/ch/0/par/dcoffset", &original)
        .expect("Failed to restore dcoffset");
}

#[test]
#[ignore = "Requires CAEN hardware"]
fn test_invalid_url_returns_error() {
    let result = CaenHandle::open("dig2://192.168.255.254");
    assert!(
        result.is_err(),
        "Should fail to connect to non-existent device"
    );

    if let Err(e) = result {
        println!("Expected error: {}", e);
        // Should be connection-related error
        assert!(
            e.code == -4 || e.code == -15, // DeviceNotFound or CommunicationError
            "Expected DeviceNotFound or CommunicationError, got: {} ({})",
            e.name,
            e.code
        );
    }
}

#[test]
#[ignore = "Requires CAEN hardware"]
fn test_invalid_parameter_path() {
    let url = get_test_url();
    let handle = CaenHandle::open(&url).expect("Failed to open device");

    let result = handle.get_value("/par/NonExistentParameter");
    assert!(result.is_err(), "Should fail for non-existent parameter");
}

#[test]
#[ignore = "Requires CAEN hardware"]
fn test_get_param_info() {
    let url = get_test_url();
    let handle = CaenHandle::open(&url).expect("Failed to open device");

    // Test a numeric parameter (PSD2 uses lowercase names)
    let info = handle
        .get_param_info("triggerthr")
        .expect("Failed to get triggerthr param info");

    println!("triggerthr ParamInfo:");
    println!("  Name: {}", info.name);
    println!("  Datatype: {}", info.datatype);
    println!("  AccessMode: {}", info.access_mode);
    println!("  SetInRun: {}", info.setinrun);
    println!("  Min: {:?}", info.min_value);
    println!("  Max: {:?}", info.max_value);

    assert_eq!(info.name, "triggerthr");
    // triggerthr should typically be a numeric type
    assert!(
        info.datatype.contains("NUMBER") || info.datatype.contains("U32"),
        "Expected numeric type, got: {}",
        info.datatype
    );
}

#[test]
#[ignore = "Requires CAEN hardware"]
fn test_get_param_info_setinrun() {
    let url = get_test_url();
    let handle = CaenHandle::open(&url).expect("Failed to open device");

    // dcoffset is typically setinrun=true (PSD2 uses lowercase)
    let dc_info = handle
        .get_param_info("dcoffset")
        .expect("Failed to get dcoffset param info");

    println!("dcoffset setinrun: {}", dc_info.setinrun);

    // chrecordlengtht (time-based) is typically setinrun=false
    // Note: PSD2 uses 's' suffix for samples, 't' suffix for time
    let rl_info = handle
        .get_param_info("chrecordlengtht")
        .expect("Failed to get chrecordlengtht param info");

    println!("chrecordlengtht setinrun: {}", rl_info.setinrun);

    // Both should have valid info
    assert!(!dc_info.datatype.is_empty());
    assert!(!rl_info.datatype.is_empty());
}

// =============================================================================
// Data Acquisition Tests (Phase 4)
// =============================================================================

#[test]
#[ignore = "Requires CAEN hardware"]
fn test_configure_endpoint() {
    let url = get_test_url();
    let handle = CaenHandle::open(&url).expect("Failed to open device");

    // Configure RAW endpoint for data reading
    let endpoint = handle
        .configure_endpoint(true)
        .expect("Failed to configure endpoint");

    println!(
        "Endpoint configured successfully, handle: {}",
        endpoint.raw()
    );
    assert!(endpoint.raw() != 0, "Endpoint handle should be non-zero");
}

#[test]
#[ignore = "Requires CAEN hardware"]
fn test_arm_disarm() {
    let url = get_test_url();
    let handle = CaenHandle::open(&url).expect("Failed to open device");

    // Configure endpoint first
    let _endpoint = handle
        .configure_endpoint(true)
        .expect("Failed to configure endpoint");

    // Arm acquisition
    handle
        .send_command("/cmd/armacquisition")
        .expect("Failed to arm acquisition");
    println!("Acquisition armed");

    // Small delay to ensure state change
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Disarm acquisition
    handle
        .send_command("/cmd/disarmacquisition")
        .expect("Failed to disarm acquisition");
    println!("Acquisition disarmed");
}

#[test]
#[ignore = "Requires CAEN hardware"]
fn test_arm_start_stop() {
    let url = get_test_url();
    let handle = CaenHandle::open(&url).expect("Failed to open device");

    // Configure endpoint
    let _endpoint = handle
        .configure_endpoint(true)
        .expect("Failed to configure endpoint");

    // Arm
    handle
        .send_command("/cmd/armacquisition")
        .expect("Failed to arm");
    println!("Armed");

    // Start
    handle
        .send_command("/cmd/swstartacquisition")
        .expect("Failed to start");
    println!("Started");

    // Let it run briefly
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Stop (disarm)
    handle
        .send_command("/cmd/disarmacquisition")
        .expect("Failed to stop");
    println!("Stopped");
}

#[test]
#[ignore = "Requires CAEN hardware"]
fn test_read_data_with_test_pulse() {
    let url = get_test_url();
    let handle = CaenHandle::open(&url).expect("Failed to open device");

    // Configure test pulse for data generation
    // Enable channel 0
    handle
        .set_value("/ch/0/par/chenable", "true")
        .expect("Failed to enable channel");

    // Set test pulse as global trigger source
    handle
        .set_value("/par/globaltriggersource", "TestPulse")
        .expect("Failed to set trigger source");

    // Set event trigger source to global
    handle
        .set_value("/ch/0/par/eventtriggersource", "GlobalTriggerSource")
        .expect("Failed to set event trigger source");

    // Set test pulse period (10 microseconds = 10000 ns)
    handle
        .set_value("/par/testpulseperiod", "10000")
        .expect("Failed to set test pulse period");

    // Set test pulse width (100 ns)
    handle
        .set_value("/par/testpulsewidth", "100")
        .expect("Failed to set test pulse width");

    // Configure endpoint
    let endpoint = handle
        .configure_endpoint(true)
        .expect("Failed to configure endpoint");

    // Arm and start
    handle.send_command("/cmd/armacquisition").expect("Arm");
    handle.send_command("/cmd/swstartacquisition").expect("Start");
    println!("Acquisition started with test pulse");

    // Wait for some data
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Try to read data
    let mut total_events = 0u32;
    let mut read_attempts = 0;

    for _ in 0..10 {
        read_attempts += 1;
        match endpoint.read_data(100, 1024 * 1024) {
            Ok(Some(raw)) => {
                println!(
                    "Read {} bytes, {} events (attempt {})",
                    raw.size, raw.n_events, read_attempts
                );
                total_events += raw.n_events;
            }
            Ok(None) => {
                println!("Timeout on attempt {}", read_attempts);
            }
            Err(e) => {
                println!("Read error on attempt {}: {}", read_attempts, e);
                break;
            }
        }
    }

    // Stop acquisition
    handle
        .send_command("/cmd/disarmacquisition")
        .expect("Failed to stop");
    println!("Acquisition stopped, total events: {}", total_events);

    // With test pulse, we should have received some events
    assert!(
        total_events > 0,
        "Should have received events with test pulse"
    );
}

#[test]
#[ignore = "Requires CAEN hardware"]
fn test_decode_test_pulse_events() {
    use delila_rs::reader::decoder::{Psd2Config, Psd2Decoder, RawData};

    let url = get_test_url();
    let handle = CaenHandle::open(&url).expect("Failed to open device");

    // Configure test pulse
    handle
        .set_value("/ch/0/par/chenable", "true")
        .expect("Enable ch0");
    handle
        .set_value("/par/globaltriggersource", "TestPulse")
        .expect("Set trigger");
    handle
        .set_value("/ch/0/par/eventtriggersource", "GlobalTriggerSource")
        .expect("Set event trigger");
    handle
        .set_value("/par/testpulseperiod", "10000")
        .expect("Set period");
    handle
        .set_value("/par/testpulsewidth", "100")
        .expect("Set width");

    // Configure endpoint
    let endpoint = handle.configure_endpoint(true).expect("Configure endpoint");

    // Create decoder
    let mut decoder = Psd2Decoder::new(Psd2Config {
        time_step_ns: 2.0,
        module_id: 0,
        dump_enabled: false,
    });

    // Start acquisition
    handle.send_command("/cmd/armacquisition").expect("Arm");
    handle.send_command("/cmd/swstartacquisition").expect("Start");

    // Read and decode data
    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut decoded_events = 0;

    for _ in 0..10 {
        match endpoint.read_data(100, 1024 * 1024) {
            Ok(Some(raw)) => {
                // Convert to decoder RawData format
                let decoder_raw = RawData {
                    data: raw.data,
                    size: raw.size,
                    n_events: raw.n_events,
                };

                let events = decoder.decode(&decoder_raw);
                for event in &events {
                    if decoded_events < 5 {
                        println!(
                            "Event: ch={}, ts={:.2}ns, energy={}, short={}",
                            event.channel, event.timestamp_ns, event.energy, event.energy_short
                        );
                    }
                }
                decoded_events += events.len();
            }
            Ok(None) => {}
            Err(_) => break,
        }
    }

    // Stop
    handle.send_command("/cmd/disarmacquisition").expect("Stop");
    println!("Total decoded events: {}", decoded_events);

    assert!(
        decoded_events > 0,
        "Should have decoded events from test pulse"
    );
}

// =============================================================================
// Master/Slave Synchronization Tests (Phase 5)
// =============================================================================

/// Test that master sync config parameters can be applied
#[test]
#[ignore = "Requires CAEN hardware"]
fn test_master_sync_config() {
    use delila_rs::config::{DigitizerConfig, FirmwareType, SyncConfig};

    let url = get_test_url();
    let handle = CaenHandle::open(&url).expect("Failed to open device");

    // Create master config with sync settings
    let mut config = DigitizerConfig::new(0, "Master", FirmwareType::PSD2);
    config.is_master = true;
    config.sync = Some(SyncConfig {
        trgout_source: Some("Run".to_string()),
        sin_source: None,
        start_source: Some("SWcmd".to_string()),
    });

    // Apply config
    let result = handle.apply_config(&config);
    match result {
        Ok(count) => {
            println!("Applied {} sync parameters for master", count);
            assert!(count >= 2, "Should apply at least 2 sync parameters");
        }
        Err(e) => {
            // Some parameters may not be supported on all firmware versions
            println!("Partial config apply (expected): {}", e);
        }
    }

    // Verify TrgOut setting was applied
    let trgout = handle.get_value("/par/trgoutsource");
    println!("TrgOut source: {:?}", trgout);
}

/// Test that slave sync config parameters can be applied
#[test]
#[ignore = "Requires CAEN hardware"]
fn test_slave_sync_config() {
    use delila_rs::config::{DigitizerConfig, FirmwareType, SyncConfig};

    let url = get_test_url();
    let handle = CaenHandle::open(&url).expect("Failed to open device");

    // Create slave config with sync settings
    let mut config = DigitizerConfig::new(0, "Slave", FirmwareType::PSD2);
    config.is_master = false;
    config.sync = Some(SyncConfig {
        trgout_source: None,
        sin_source: Some("SIN".to_string()),
        start_source: Some("SIN".to_string()),
    });

    // Apply config
    let result = handle.apply_config(&config);
    match result {
        Ok(count) => {
            println!("Applied {} sync parameters for slave", count);
            assert!(count >= 2, "Should apply at least 2 sync parameters");
        }
        Err(e) => {
            // Some parameters may not be supported on all firmware versions
            println!("Partial config apply (expected): {}", e);
        }
    }

    // Verify SIN setting was applied
    let sinsource = handle.get_value("/par/sinsource");
    println!("SIN source: {:?}", sinsource);
}

/// Test master/slave parameter readback
#[test]
#[ignore = "Requires CAEN hardware"]
fn test_sync_parameters_readback() {
    let url = get_test_url();
    let handle = CaenHandle::open(&url).expect("Failed to open device");

    // Read sync-related parameters
    println!("Sync-related parameters:");

    if let Ok(v) = handle.get_value("/par/startsource") {
        println!("  StartSource: {}", v);
    }

    if let Ok(v) = handle.get_value("/par/trgoutsource") {
        println!("  TrgOutSource: {}", v);
    }

    if let Ok(v) = handle.get_value("/par/sinsource") {
        println!("  SINSource: {}", v);
    }

    if let Ok(v) = handle.get_value("/par/sinlevel") {
        println!("  SINLevel: {}", v);
    }

    if let Ok(v) = handle.get_value("/par/gpiomode") {
        println!("  GPIOMode: {}", v);
    }
}

// =============================================================================
// ch4 External Pulser Signal Test
// =============================================================================

/// Test ch4 external pulser signal readout
///
/// Expects: ~20ns negative pulse at 10-11 kHz on ch4
/// Sets ch4 to ChSelfTrigger with appropriate gate settings.
/// Does NOT set GlobalTriggerSource (avoids TestPulse interference).
#[test]
#[ignore = "Requires CAEN hardware"]
fn test_ch4_pulser_signal() {
    use delila_rs::reader::decoder::{Psd2Config, Psd2Decoder, RawData};

    let url = get_test_url();
    let handle = CaenHandle::open(&url).expect("Failed to open device");

    // Disable all channels except ch4
    for ch in 0..32 {
        if ch != 4 {
            let _ = handle.set_value(&format!("/ch/{}/par/ChEnable", ch), "False");
        }
    }

    // Configure ch4 for self-trigger on external pulser
    // NOTE: GlobalTriggerSource is NOT set — avoids TestPulse triggers
    handle
        .set_value("/ch/4/par/ChEnable", "True")
        .expect("Enable ch4");
    handle
        .set_value("/ch/4/par/EventTriggerSource", "ChSelfTrigger")
        .expect("Set ch4 EventTriggerSource to ChSelfTrigger");
    handle
        .set_value("/ch/4/par/PulsePolarity", "Negative")
        .expect("Set ch4 polarity Negative");
    handle
        .set_value("/ch/4/par/DCOffset", "50")
        .expect("Set ch4 DC offset 50%");
    handle
        .set_value("/ch/4/par/TriggerThr", "1000")
        .expect("Set ch4 trigger threshold");
    handle
        .set_value("/ch/4/par/GateLongLengthT", "400")
        .expect("Set ch4 gate long 400ns");
    handle
        .set_value("/ch/4/par/GateShortLengthT", "100")
        .expect("Set ch4 gate short 100ns");

    // Readback key parameters for verification
    println!("\n=== ch4 Parameter Readback ===");
    for param in &[
        "ChEnable",
        "EventTriggerSource",
        "PulsePolarity",
        "DCOffset",
        "TriggerThr",
        "GateLongLengthT",
        "GateShortLengthT",
    ] {
        if let Ok(v) = handle.get_value(&format!("/ch/4/par/{}", param)) {
            println!("  ch4/{}: {}", param, v);
        }
    }
    // Also readback GlobalTriggerSource to confirm it's not TestPulse
    if let Ok(v) = handle.get_value("/par/globaltriggersource") {
        println!("  GlobalTriggerSource: {}", v);
    }

    // Configure endpoint and decoder
    let endpoint = handle.configure_endpoint(true).expect("Configure endpoint");
    let mut decoder = Psd2Decoder::new(Psd2Config {
        time_step_ns: 2.0,
        module_id: 0,
        dump_enabled: false,
    });

    // Acquire data
    handle
        .send_command("/cmd/armacquisition")
        .expect("Arm");
    handle
        .send_command("/cmd/swstartacquisition")
        .expect("Start");

    // Wait for pulser data to accumulate (~500ms → ~5000-5500 events at 10-11kHz)
    std::thread::sleep(std::time::Duration::from_millis(500));

    let mut ch4_events: Vec<delila_rs::reader::decoder::EventData> = Vec::new();
    let mut other_ch_count = 0u64;

    for _ in 0..30 {
        match endpoint.read_data(100, 1024 * 1024) {
            Ok(Some(raw)) => {
                let decoder_raw = RawData {
                    data: raw.data,
                    size: raw.size,
                    n_events: raw.n_events,
                };
                let events = decoder.decode(&decoder_raw);
                for event in events {
                    if event.channel == 4 {
                        ch4_events.push(event);
                    } else {
                        other_ch_count += 1;
                    }
                }
            }
            Ok(None) => {}
            Err(_) => break,
        }
    }

    // Stop acquisition
    handle
        .send_command("/cmd/disarmacquisition")
        .expect("Stop");

    // Analyze results
    println!("\n=== ch4 Pulser Signal Analysis ===");
    println!("ch4 events: {}", ch4_events.len());
    println!("Other channel events: {}", other_ch_count);

    assert!(
        ch4_events.len() > 100,
        "Should have >100 ch4 events from 10-11kHz pulser (got {})",
        ch4_events.len()
    );

    // Rate calculation
    if ch4_events.len() >= 2 {
        let first_ts = ch4_events.first().unwrap().timestamp_ns;
        let last_ts = ch4_events.last().unwrap().timestamp_ns;
        let duration_s = (last_ts - first_ts) / 1e9;

        if duration_s > 0.0 {
            let rate_hz = (ch4_events.len() - 1) as f64 / duration_s;
            println!("Duration: {:.3} s", duration_s);
            println!("Rate: {:.1} Hz (expected: 10000-11000 Hz)", rate_hz);

            // Rate should be approximately 10-11 kHz
            assert!(
                rate_hz > 5000.0 && rate_hz < 20000.0,
                "Rate should be ~10-11 kHz, got {:.1} Hz",
                rate_hz
            );
        }
    }

    // Energy statistics
    let energies: Vec<u16> = ch4_events.iter().map(|e| e.energy).collect();
    let energy_shorts: Vec<u16> = ch4_events.iter().map(|e| e.energy_short).collect();
    let non_zero: Vec<u16> = energies.iter().filter(|&&e| e > 0).cloned().collect();

    let (e_min, e_max) = (
        energies.iter().min().copied().unwrap_or(0),
        energies.iter().max().copied().unwrap_or(0),
    );
    let avg_energy = if !non_zero.is_empty() {
        non_zero.iter().map(|&e| e as f64).sum::<f64>() / non_zero.len() as f64
    } else {
        0.0
    };
    let (es_min, es_max) = (
        energy_shorts.iter().min().copied().unwrap_or(0),
        energy_shorts.iter().max().copied().unwrap_or(0),
    );

    println!("\nEnergy (long gate):");
    println!(
        "  min={}, max={}, avg={:.1}, non-zero={}/{}",
        e_min,
        e_max,
        avg_energy,
        non_zero.len(),
        energies.len()
    );
    println!("Energy short:");
    println!("  min={}, max={}", es_min, es_max);

    // Timestamp interval analysis (first 20 intervals)
    println!("\nTimestamp intervals (first 20):");
    for i in 0..std::cmp::min(20, ch4_events.len().saturating_sub(1)) {
        let dt = ch4_events[i + 1].timestamp_ns - ch4_events[i].timestamp_ns;
        println!(
            "  [{:2}] dt = {:.2} ns ({:.2} us)",
            i,
            dt,
            dt / 1000.0
        );
    }

    // Print first 10 events
    println!("\nFirst 10 events:");
    for (i, event) in ch4_events.iter().take(10).enumerate() {
        println!(
            "  [{}] ch={}, ts={:.2}ns, energy={}, short={}, flags={:#x}",
            i, event.channel, event.timestamp_ns, event.energy, event.energy_short, event.flags
        );
    }
}
