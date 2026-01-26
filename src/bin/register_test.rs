//! Register read/write test - Software reset via 0xEF24
//!
//! Usage: cargo run --bin register_test [-- <url>]
//! Default URL: dig1://caen.internal/usb?link_num=0

use delila_rs::reader::CaenHandle;
use std::env;
use std::thread;
use std::time::Duration;

fn main() {
    let args: Vec<String> = env::args().collect();
    let url = if args.len() > 1 {
        &args[1]
    } else {
        "dig1://caen.internal/usb?link_num=0"
    };

    println!("=== Register Test (Software Reset) ===");
    println!("URL: {}", url);
    println!();

    // Step 1: Connect and read license info
    println!("--- Before Reset ---");
    let handle = match CaenHandle::open(url) {
        Ok(h) => {
            println!("[OK] Connected");
            h
        }
        Err(e) => {
            eprintln!("[ERROR] Failed to connect: {}", e);
            std::process::exit(1);
        }
    };

    read_info(&handle);

    // Step 2: Software reset via register 0xEF24
    println!();
    println!("--- Software Reset (write 0xEF24 = 0) ---");
    match handle.set_user_register(0xEF24, 0) {
        Ok(()) => println!("[OK] Software reset issued"),
        Err(e) => {
            eprintln!("[ERROR] set_user_register failed: {}", e);
            std::process::exit(1);
        }
    }

    // Drop old handle
    drop(handle);

    // Step 3: Wait for reset to complete, then reconnect
    println!("Waiting 3 seconds for reset...");
    thread::sleep(Duration::from_secs(3));

    println!();
    println!("--- After Reset ---");
    let handle2 = match CaenHandle::open(url) {
        Ok(h) => {
            println!("[OK] Reconnected");
            h
        }
        Err(e) => {
            eprintln!("[ERROR] Failed to reconnect: {}", e);
            std::process::exit(1);
        }
    };

    read_info(&handle2);

    println!();
    println!("=== Done ===");
}

fn read_info(handle: &CaenHandle) {
    // DIG1 (DT5730B) uses lowercase parameter names
    // DIG2 (VX2730) uses CamelCase - try lowercase first
    let params = [
        "/par/modelname",
        "/par/serialnum",
        "/par/fwtype",
        "/par/licensestatus",
        "/par/timebombdowncounter",
    ];

    for path in &params {
        match handle.get_value(path) {
            Ok(value) => {
                let name = path.split('/').next_back().unwrap_or(path);
                println!("  {:<25}: {}", name, value);
            }
            Err(e) => {
                let name = path.split('/').next_back().unwrap_or(path);
                println!("  {:<25}: [ERROR] {}", name, e);
            }
        }
    }
}
