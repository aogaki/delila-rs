//! Emulator binary - publishes dummy event data via ZeroMQ
//!
//! Usage:
//!   cargo run --bin emulator                           # Use defaults
//!   cargo run --bin emulator -- --config config.toml   # Use config file
//!   cargo run --bin emulator -- --batches 10           # Run for 10 batches
//!   cargo run --bin emulator -- --source-id 1          # Use specific source

use delila_rs::config::Config;
use delila_rs::data_source_emulator::{Emulator, EmulatorConfig};
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
    let mut batches: Option<u64> = None;
    let mut source_id: Option<u32> = None;
    let mut address: Option<String> = None;
    let mut interval_ms: Option<u64> = None;
    let mut events_per_batch: Option<usize> = None;

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
            "--batches" | "-b" => {
                if i + 1 < args.len() {
                    batches = Some(args[i + 1].parse().expect("batches must be a number"));
                    i += 2;
                } else {
                    eprintln!("Error: --batches requires a number");
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
            "--source-id" | "-s" => {
                if i + 1 < args.len() {
                    source_id = Some(args[i + 1].parse().expect("source-id must be a number"));
                    i += 2;
                } else {
                    eprintln!("Error: --source-id requires a number");
                    std::process::exit(1);
                }
            }
            "--interval" | "-i" => {
                if i + 1 < args.len() {
                    interval_ms = Some(args[i + 1].parse().expect("interval must be a number"));
                    i += 2;
                } else {
                    eprintln!("Error: --interval requires a number");
                    std::process::exit(1);
                }
            }
            "--events" | "-e" => {
                if i + 1 < args.len() {
                    events_per_batch = Some(args[i + 1].parse().expect("events must be a number"));
                    i += 2;
                } else {
                    eprintln!("Error: --events requires a number");
                    std::process::exit(1);
                }
            }
            "--help" | "-h" => {
                println!("Emulator - publishes dummy event data via ZeroMQ");
                println!();
                println!("Usage: emulator [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --config, -c <FILE>   Load configuration from TOML file");
                println!("  --source-id, -s <ID>  Source ID (selects config from file) [default: 0]");
                println!("  --batches, -b <N>     Run for N batches then send EOS and exit");
                println!("  --address, -a <ADDR>  Override ZMQ bind address");
                println!("  --interval, -i <MS>   Batch interval in milliseconds [default: 100]");
                println!("  --events, -e <N>      Events per batch [default: 100]");
                println!("  --help, -h            Show this help message");
                println!();
                println!("Examples:");
                println!("  emulator --config config.toml --source-id 1");
                println!("  emulator --batches 100");
                println!("  emulator --interval 10 --events 1000  # High rate: 100kHz");
                return Ok(());
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    // Build configuration
    let emulator_config = if let Some(path) = config_path {
        // Load from config file
        let config = Config::load(&path)?;
        let settings = config.settings.get_settings()?;

        // Find source config by ID
        let sid = source_id.unwrap_or(0);
        let source_net = config
            .network
            .sources
            .iter()
            .find(|s| s.id == sid);

        let bind_address = if let Some(addr) = address {
            addr
        } else if let Some(src) = source_net {
            src.bind.clone()
        } else {
            format!("tcp://*:{}", 5555 + sid as u16)
        };

        info!(
            config_file = %path,
            source_id = sid,
            "Loaded configuration"
        );

        let command_addr = source_net
            .and_then(|s| s.command.clone())
            .unwrap_or_else(|| format!("tcp://*:{}", 5560 + sid as u16));

        EmulatorConfig {
            address: bind_address,
            command_address: command_addr,
            source_id: sid,
            events_per_batch: settings.events_per_batch as usize,
            batch_interval_ms: settings.batch_interval_ms,
            heartbeat_interval_ms: 1000, // 1Hz heartbeat
            num_modules: settings.num_modules as u8,
            channels_per_module: settings.channels_per_module as u8,
        }
    } else {
        // Use defaults with CLI overrides
        let sid = source_id.unwrap_or(0);
        EmulatorConfig {
            address: address.unwrap_or_else(|| "tcp://*:5555".to_string()),
            command_address: format!("tcp://*:{}", 5560 + sid as u16),
            source_id: sid,
            events_per_batch: events_per_batch.unwrap_or(100),
            batch_interval_ms: interval_ms.unwrap_or(100),
            heartbeat_interval_ms: 1000, // 1Hz heartbeat
            num_modules: 2,
            channels_per_module: 16,
        }
    };

    // Create emulator
    let mut emulator = Emulator::new(emulator_config.clone()).await?;

    println!(
        "Emulator running. source_id={}, publishing to {}",
        emulator_config.source_id, emulator_config.address
    );

    if let Some(count) = batches {
        // Run for fixed number of batches
        println!("Will send {} batches then EOS.", count);
        emulator.run_batches(count).await?;
    } else {
        // Run until Ctrl+C
        println!("Press Ctrl+C to stop.");

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

        emulator.run(shutdown_rx).await?;
    }

    println!("Emulator stopped.");
    Ok(())
}
