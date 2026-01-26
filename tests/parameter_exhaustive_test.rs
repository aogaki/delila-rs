//! Exhaustive Parameter Test
//!
//! Tests reading and writing all parameters from the DevTree.
//! This test discovers parameters dynamically from the device.
//!
//! Run with: `cargo test --test parameter_exhaustive_test -- --ignored --nocapture`
//!
//! WARNING: This test modifies device parameters. Original values are restored,
//! but use with caution on production systems.

use delila_rs::reader::caen::CaenHandle;
use serde_json::Value;
use std::collections::HashMap;

/// Get the digitizer URL from environment or use default
fn get_test_url() -> String {
    std::env::var("CAEN_DIGITIZER_URL").unwrap_or_else(|_| "dig2://172.18.4.56".to_string())
}

/// Parameter info extracted from DevTree
#[derive(Debug, Clone)]
struct ParamSpec {
    name: String,
    path: String,
    level: String,       // DIG, CH, LVDS, etc.
    datatype: String,    // NUMBER, STRING, etc.
    access_mode: String, // READ_ONLY, READ_WRITE
    setinrun: bool,
    min_value: Option<String>,
    max_value: Option<String>,
    allowed_values: Vec<String>,
}

/// Extract all parameters from DevTree JSON
fn extract_all_parameters(tree: &Value, current_path: &str, params: &mut Vec<ParamSpec>) {
    if let Some(obj) = tree.as_object() {
        for (key, value) in obj {
            let new_path = if current_path.is_empty() {
                format!("/{}", key)
            } else {
                format!("{}/{}", current_path, key)
            };

            // Check if this is a parameter (has datatype attribute)
            if let Some(datatype_obj) = value.get("datatype") {
                if let Some(datatype_val) = datatype_obj.get("value").and_then(|v| v.as_str()) {
                    // This is a parameter node
                    let access_mode = value
                        .get("accessmode")
                        .and_then(|v| v.get("value"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("READ_ONLY")
                        .to_string();

                    let setinrun = value
                        .get("setinrun")
                        .and_then(|v| v.get("value"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_lowercase() == "true")
                        .unwrap_or(false);

                    let min_value = value
                        .get("minvalue")
                        .and_then(|v| v.get("value"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    let max_value = value
                        .get("maxvalue")
                        .and_then(|v| v.get("value"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    // Extract allowed values
                    let mut allowed_values = Vec::new();
                    if let Some(av) = value.get("allowedvalues") {
                        if let Some(av_obj) = av.as_object() {
                            for (av_key, av_val) in av_obj {
                                if av_key.parse::<u32>().is_ok() {
                                    if let Some(v) = av_val.get("value").and_then(|v| v.as_str()) {
                                        allowed_values.push(v.to_string());
                                    }
                                }
                            }
                        }
                    }

                    // Determine level from path
                    let level = if new_path.contains("/ch/") {
                        "CH"
                    } else if new_path.contains("/lvds/") {
                        "LVDS"
                    } else if new_path.contains("/vga/") {
                        "VGA"
                    } else if new_path.contains("/endpoint/") {
                        "ENDPOINT"
                    } else {
                        "DIG"
                    }
                    .to_string();

                    params.push(ParamSpec {
                        name: key.clone(),
                        path: new_path.clone(),
                        level,
                        datatype: datatype_val.to_string(),
                        access_mode,
                        setinrun,
                        min_value,
                        max_value,
                        allowed_values,
                    });
                }
            }

            // Recursively search in child nodes
            if value.is_object() {
                extract_all_parameters(value, &new_path, params);
            }
        }
    }
}

/// Get unique board-level parameters (deduplicated)
fn get_board_params(params: &[ParamSpec]) -> Vec<ParamSpec> {
    let mut seen = HashMap::new();
    params
        .iter()
        .filter(|p| p.level == "DIG" && !p.path.contains("/ch/"))
        .filter(|p| {
            // Deduplicate by name
            if seen.contains_key(&p.name) {
                false
            } else {
                seen.insert(p.name.clone(), true);
                true
            }
        })
        .cloned()
        .collect()
}

/// Get channel-level parameters (only from channel 0 as template)
fn get_channel_params(params: &[ParamSpec]) -> Vec<ParamSpec> {
    let mut seen = HashMap::new();
    params
        .iter()
        .filter(|p| p.path.contains("/ch/0/"))
        .filter(|p| {
            if seen.contains_key(&p.name) {
                false
            } else {
                seen.insert(p.name.clone(), true);
                true
            }
        })
        .cloned()
        .collect()
}

#[test]
#[ignore = "Requires CAEN hardware - exhaustive test"]
fn test_read_all_board_parameters() {
    let url = get_test_url();
    println!("Connecting to: {}", url);

    let handle = CaenHandle::open(&url).expect("Failed to open device");

    // Get DevTree
    let tree_json = handle.get_device_tree().expect("Failed to get device tree");
    let tree: Value = serde_json::from_str(&tree_json).expect("Failed to parse DevTree");

    // Extract all parameters
    let mut all_params = Vec::new();
    extract_all_parameters(&tree, "", &mut all_params);

    println!("\n=== Total parameters found: {} ===\n", all_params.len());

    // Get board-level parameters
    let board_params = get_board_params(&all_params);
    println!("Board-level parameters: {}\n", board_params.len());

    let mut success_count = 0;
    let mut fail_count = 0;
    let mut failed_params = Vec::new();

    for param in &board_params {
        // Build the read path
        let read_path = format!("/par/{}", param.name);

        match handle.get_value(&read_path) {
            Ok(value) => {
                println!(
                    "[OK] {} = {} ({}{})",
                    param.name,
                    value,
                    param.datatype,
                    if param.access_mode == "READ_WRITE" {
                        ", RW"
                    } else {
                        ""
                    }
                );
                success_count += 1;
            }
            Err(e) => {
                println!("[FAIL] {} - Error: {}", param.name, e);
                failed_params.push((param.name.clone(), e.to_string()));
                fail_count += 1;
            }
        }
    }

    println!("\n=== Board Parameter Read Summary ===");
    println!("Success: {}", success_count);
    println!("Failed: {}", fail_count);

    if !failed_params.is_empty() {
        println!("\nFailed parameters:");
        for (name, err) in &failed_params {
            println!("  - {}: {}", name, err);
        }
    }

    // Allow some failures (some params may require specific conditions)
    let failure_rate = fail_count as f64 / (success_count + fail_count) as f64;
    assert!(
        failure_rate < 0.2,
        "Too many failures: {:.1}%",
        failure_rate * 100.0
    );
}

#[test]
#[ignore = "Requires CAEN hardware - exhaustive test"]
fn test_read_all_channel_parameters() {
    let url = get_test_url();
    println!("Connecting to: {}", url);

    let handle = CaenHandle::open(&url).expect("Failed to open device");

    // Get DevTree
    let tree_json = handle.get_device_tree().expect("Failed to get device tree");
    let tree: Value = serde_json::from_str(&tree_json).expect("Failed to parse DevTree");

    // Extract all parameters
    let mut all_params = Vec::new();
    extract_all_parameters(&tree, "", &mut all_params);

    // Get channel-level parameters (from ch/0)
    let channel_params = get_channel_params(&all_params);
    println!("Channel-level parameters: {}\n", channel_params.len());

    let mut success_count = 0;
    let mut fail_count = 0;
    let mut failed_params = Vec::new();

    // Test only channel 0
    for param in &channel_params {
        let read_path = format!("/ch/0/par/{}", param.name);

        match handle.get_value(&read_path) {
            Ok(value) => {
                println!(
                    "[OK] {} = {} ({}{}{})",
                    param.name,
                    value,
                    param.datatype,
                    if param.access_mode == "READ_WRITE" {
                        ", RW"
                    } else {
                        ""
                    },
                    if param.setinrun { ", setinrun" } else { "" }
                );
                success_count += 1;
            }
            Err(e) => {
                println!("[FAIL] {} - Error: {}", param.name, e);
                failed_params.push((param.name.clone(), e.to_string()));
                fail_count += 1;
            }
        }
    }

    println!("\n=== Channel Parameter Read Summary ===");
    println!("Success: {}", success_count);
    println!("Failed: {}", fail_count);

    if !failed_params.is_empty() {
        println!("\nFailed parameters:");
        for (name, err) in &failed_params {
            println!("  - {}: {}", name, err);
        }
    }

    let failure_rate = fail_count as f64 / (success_count + fail_count) as f64;
    assert!(
        failure_rate < 0.2,
        "Too many failures: {:.1}%",
        failure_rate * 100.0
    );
}

#[test]
#[ignore = "Requires CAEN hardware - exhaustive test"]
fn test_write_safe_parameters() {
    let url = get_test_url();
    println!("Connecting to: {}", url);

    let handle = CaenHandle::open(&url).expect("Failed to open device");

    // Get DevTree
    let tree_json = handle.get_device_tree().expect("Failed to get device tree");
    let tree: Value = serde_json::from_str(&tree_json).expect("Failed to parse DevTree");

    // Extract all parameters
    let mut all_params = Vec::new();
    extract_all_parameters(&tree, "", &mut all_params);

    // Filter for safe, writable parameters
    // Safe = has allowed_values OR has reasonable min/max range
    let safe_write_params: Vec<_> = all_params
        .iter()
        .filter(|p| p.access_mode == "READ_WRITE")
        .filter(|p| {
            // Skip dangerous parameters
            let dangerous = [
                "clocksource",
                "ipaddress",
                "netmask",
                "gateway",
                "permanentclockoutdelay",
                "license",
            ];
            !dangerous.contains(&p.name.to_lowercase().as_str())
        })
        .filter(|p| {
            // Only test if we have allowed_values or both min/max
            !p.allowed_values.is_empty() || (p.min_value.is_some() && p.max_value.is_some())
        })
        .collect();

    println!(
        "Safe writable parameters to test: {}\n",
        safe_write_params.len()
    );

    let mut success_count = 0;
    let mut fail_count = 0;
    let mut skipped_count = 0;

    for param in safe_write_params.iter().take(20) {
        // Limit to first 20 for safety
        // Determine read/write path
        let path = if param.path.contains("/ch/0/") {
            format!("/ch/0/par/{}", param.name)
        } else if param.path.contains("/par/") {
            format!("/par/{}", param.name)
        } else {
            println!("[SKIP] {} - unusual path: {}", param.name, param.path);
            skipped_count += 1;
            continue;
        };

        // Read original value
        let original = match handle.get_value(&path) {
            Ok(v) => v,
            Err(e) => {
                println!("[SKIP] {} - cannot read: {}", param.name, e);
                skipped_count += 1;
                continue;
            }
        };

        // Determine test value
        let test_value = if !param.allowed_values.is_empty() {
            // Pick a different allowed value if possible
            param
                .allowed_values
                .iter()
                .find(|v| *v != &original)
                .cloned()
                .unwrap_or_else(|| param.allowed_values[0].clone())
        } else if let (Some(min), Some(max)) = (&param.min_value, &param.max_value) {
            // For numeric, try a value in the middle
            if let (Ok(min_f), Ok(max_f)) = (min.parse::<f64>(), max.parse::<f64>()) {
                let mid = (min_f + max_f) / 2.0;
                // Round to integer if original looks like integer
                if !original.contains('.') {
                    (mid as i64).to_string()
                } else {
                    format!("{:.2}", mid)
                }
            } else {
                println!(
                    "[SKIP] {} - cannot parse min/max: {}/{}",
                    param.name, min, max
                );
                skipped_count += 1;
                continue;
            }
        } else {
            println!("[SKIP] {} - no safe test value available", param.name);
            skipped_count += 1;
            continue;
        };

        // Skip if test value equals original
        if test_value == original {
            println!("[SKIP] {} - test value equals original", param.name);
            skipped_count += 1;
            continue;
        }

        // Write test value
        print!("[TEST] {} : {} -> {} ... ", param.name, original, test_value);

        match handle.set_value(&path, &test_value) {
            Ok(()) => {
                // Verify write
                match handle.get_value(&path) {
                    Ok(new_val) => {
                        // Restore original
                        let _ = handle.set_value(&path, &original);

                        // Check if write was effective (allow some tolerance for floats)
                        let write_ok = if let (Ok(test_f), Ok(new_f)) =
                            (test_value.parse::<f64>(), new_val.parse::<f64>())
                        {
                            (test_f - new_f).abs() < 1.0
                        } else {
                            test_value == new_val
                        };

                        if write_ok {
                            println!("OK (restored)");
                            success_count += 1;
                        } else {
                            println!("MISMATCH (got {}, restored)", new_val);
                            fail_count += 1;
                        }
                    }
                    Err(e) => {
                        let _ = handle.set_value(&path, &original);
                        println!("VERIFY FAILED: {}", e);
                        fail_count += 1;
                    }
                }
            }
            Err(e) => {
                println!("WRITE FAILED: {}", e);
                fail_count += 1;
            }
        }
    }

    println!("\n=== Write Test Summary ===");
    println!("Success: {}", success_count);
    println!("Failed: {}", fail_count);
    println!("Skipped: {}", skipped_count);

    // Allow some failures
    if success_count + fail_count > 0 {
        let failure_rate = fail_count as f64 / (success_count + fail_count) as f64;
        assert!(
            failure_rate < 0.3,
            "Too many write failures: {:.1}%",
            failure_rate * 100.0
        );
    }
}

#[test]
#[ignore = "Requires CAEN hardware - exhaustive test"]
fn test_generate_parameter_report() {
    let url = get_test_url();
    println!("Connecting to: {}", url);

    let handle = CaenHandle::open(&url).expect("Failed to open device");

    // Get device info
    let info = handle.get_device_info().expect("Failed to get device info");
    println!("\n=== Device Info ===");
    println!("Model: {}", info.model);
    println!("Serial: {}", info.serial_number);
    println!("Firmware: {}", info.firmware_type);
    println!("Channels: {}", info.num_channels);

    // Get DevTree
    let tree_json = handle.get_device_tree().expect("Failed to get device tree");
    let tree: Value = serde_json::from_str(&tree_json).expect("Failed to parse DevTree");

    // Extract all parameters
    let mut all_params = Vec::new();
    extract_all_parameters(&tree, "", &mut all_params);

    println!("\n=== Parameter Summary ===");
    println!("Total parameters in DevTree: {}", all_params.len());

    // Count by level
    let dig_count = all_params.iter().filter(|p| p.level == "DIG").count();
    let ch_count = all_params.iter().filter(|p| p.level == "CH").count();
    let lvds_count = all_params.iter().filter(|p| p.level == "LVDS").count();
    let endpoint_count = all_params.iter().filter(|p| p.level == "ENDPOINT").count();

    println!("  - Board level (DIG): {}", dig_count);
    println!("  - Channel level (CH): {}", ch_count);
    println!("  - LVDS level: {}", lvds_count);
    println!("  - Endpoint level: {}", endpoint_count);

    // Count by access mode
    let rw_count = all_params
        .iter()
        .filter(|p| p.access_mode == "READ_WRITE")
        .count();
    let ro_count = all_params
        .iter()
        .filter(|p| p.access_mode == "READ_ONLY")
        .count();

    println!("\nAccess modes:");
    println!("  - READ_WRITE: {}", rw_count);
    println!("  - READ_ONLY: {}", ro_count);

    // Count setinrun
    let setinrun_count = all_params.iter().filter(|p| p.setinrun).count();
    println!("\nSetInRun=true: {}", setinrun_count);

    // List unique board parameters
    let board_params = get_board_params(&all_params);
    println!("\n=== Board Parameters ({}) ===", board_params.len());
    for p in &board_params {
        println!(
            "  {} ({}, {}{})",
            p.name,
            p.datatype,
            p.access_mode,
            if p.setinrun { ", setinrun" } else { "" }
        );
    }

    // List unique channel parameters
    let channel_params = get_channel_params(&all_params);
    println!("\n=== Channel Parameters ({}) ===", channel_params.len());
    for p in &channel_params {
        println!(
            "  {} ({}, {}{})",
            p.name,
            p.datatype,
            p.access_mode,
            if p.setinrun { ", setinrun" } else { "" }
        );
    }
}
