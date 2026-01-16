//! Recorder binary - writes event data to files
//!
//! Usage:
//!   cargo run --bin recorder                            # Use defaults
//!   cargo run --bin recorder -- --config config.toml    # Use config file
//!   cargo run --bin recorder -- --address tcp://localhost:5557 --output ./data

use std::path::PathBuf;

use delila_rs::config::Config;
use delila_rs::recorder::{Recorder, RecorderConfig};
use tokio::sync::broadcast;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing (logging)
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("delila_rs=info".parse()?))
        .init();

    // Parse command line arguments
    let args: Vec<String> = std::env::args().collect();
    let mut config_path: Option<String> = None;
    let mut address: Option<String> = None;
    let mut output_dir: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-c" => {
                if i + 1 < args.len() {
                    config_path = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("Error: --config requires a file path");
                    std::process::exit(1);
                }
            }
            "--address" | "-a" => {
                if i + 1 < args.len() {
                    address = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("Error: --address requires an address");
                    std::process::exit(1);
                }
            }
            "--output" | "-o" => {
                if i + 1 < args.len() {
                    output_dir = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("Error: --output requires a directory path");
                    std::process::exit(1);
                }
            }
            "--help" | "-h" => {
                println!("Recorder - writes event data to DELILA files");
                println!();
                println!("Usage: recorder [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --config, -c <FILE>   Load configuration from TOML file");
                println!("  --address, -a <ADDR>  ZMQ address to connect to (default: tcp://localhost:5557)");
                println!("  --output, -o <DIR>    Output directory (default: ./data)");
                println!("  --help, -h            Show this help message");
                println!();
                println!("File naming: run{{XXXX}}_{{YYYY}}_{{ExpName}}.delila");
                println!("  XXXX: Run number (4 digits)");
                println!("  YYYY: File sequence (4 digits)");
                println!("  ExpName: From Configure command");
                println!();
                println!("Examples:");
                println!("  recorder --config config.toml");
                println!("  recorder --address tcp://localhost:5557 --output ./data");
                return Ok(());
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    // Build configuration
    let recorder_config = if let Some(path) = config_path {
        // Load from config file
        let config = Config::load(&path)?;

        let (subscribe_addr, command_addr, out_dir, max_size_mb, max_duration_sec) =
            if let Some(ref recorder) = config.network.recorder {
                (
                    recorder.subscribe.clone(),
                    recorder
                        .command
                        .clone()
                        .unwrap_or_else(|| "tcp://*:5580".to_string()),
                    recorder.output_dir.clone(),
                    recorder.max_file_size_mb,
                    recorder.max_file_duration_sec,
                )
            } else {
                (
                    "tcp://localhost:5557".to_string(),
                    "tcp://*:5580".to_string(),
                    "./data".to_string(),
                    1024,
                    600,
                )
            };

        info!(config_file = %path, "Loaded configuration");

        RecorderConfig {
            subscribe_address: address.unwrap_or(subscribe_addr),
            command_address: command_addr,
            output_dir: PathBuf::from(output_dir.unwrap_or(out_dir)),
            max_file_size: max_size_mb * 1024 * 1024,
            max_file_duration_secs: max_duration_sec,
        }
    } else {
        // Use defaults with CLI overrides
        RecorderConfig {
            subscribe_address: address.unwrap_or_else(|| "tcp://localhost:5557".to_string()),
            command_address: "tcp://*:5580".to_string(),
            output_dir: PathBuf::from(output_dir.unwrap_or_else(|| "./data".to_string())),
            max_file_size: 1024 * 1024 * 1024, // 1GB
            max_file_duration_secs: 600,       // 10 minutes
        }
    };

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);

    // Handle Ctrl+C
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for Ctrl+C");
        println!("\nReceived Ctrl+C, shutting down...");
        let _ = shutdown_tx_clone.send(());
    });

    // Create and run recorder
    let mut recorder = Recorder::new(recorder_config.clone()).await?;

    println!("========================================");
    println!("    DELILA Raw Data Recorder Started");
    println!("========================================");
    println!();
    println!("  Subscribing to: {}", recorder_config.subscribe_address);
    println!("  Output dir:     {}", recorder_config.output_dir.display());
    println!(
        "  Max file size:  {} MB",
        recorder_config.max_file_size / 1_000_000
    );
    println!(
        "  Max duration:   {} sec",
        recorder_config.max_file_duration_secs
    );
    println!("  Mode:           Raw (unsorted)");
    println!();
    println!("  Press Ctrl+C to stop.");
    println!("========================================");

    recorder.run(shutdown_rx).await?;

    println!("Recorder stopped.");
    Ok(())
}
