//! DataSink binary - subscribes to event data via ZeroMQ
//!
//! Usage:
//!   cargo run --bin data_sink                            # Use defaults
//!   cargo run --bin data_sink -- --config config.toml    # Use config file
//!   cargo run --bin data_sink -- --address tcp://localhost:5557

use delila_rs::config::Config;
use delila_rs::data_sink::{DataSink, DataSinkConfig};
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
            "--help" | "-h" => {
                println!("DataSink - subscribes to event data via ZeroMQ");
                println!();
                println!("Usage: data_sink [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --config, -c <FILE>   Load configuration from TOML file");
                println!("  --address, -a <ADDR>  ZMQ address to connect to");
                println!("  --help, -h            Show this help message");
                println!();
                println!("Examples:");
                println!("  data_sink --config config.toml");
                println!("  data_sink --address tcp://localhost:5557");
                return Ok(());
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    // Build configuration
    let sink_config = if let Some(path) = config_path {
        // Load from config file
        let config = Config::load(&path)?;

        // Try recorder config first (for file writing), then monitor config
        let (subscribe_addr, command_addr) = if let Some(ref recorder) = config.network.recorder {
            (
                recorder.subscribe.clone(),
                recorder
                    .command
                    .clone()
                    .unwrap_or_else(|| "tcp://*:5580".to_string()),
            )
        } else if let Some(ref monitor) = config.network.monitor {
            (monitor.subscribe.clone(), "tcp://*:5580".to_string())
        } else {
            (
                "tcp://localhost:5557".to_string(),
                "tcp://*:5580".to_string(),
            )
        };

        info!(config_file = %path, "Loaded configuration");

        DataSinkConfig {
            address: address.unwrap_or(subscribe_addr),
            command_address: command_addr,
            stats_interval_secs: 1,
            channel_capacity: 1000,
        }
    } else {
        // Use defaults with CLI overrides
        DataSinkConfig {
            address: address.unwrap_or_else(|| "tcp://localhost:5555".to_string()),
            command_address: "tcp://*:5580".to_string(),
            stats_interval_secs: 1,
            channel_capacity: 1000,
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

    // Create and run data sink
    let mut sink = DataSink::new(sink_config.clone()).await?;

    println!("DataSink running. Connecting to {}", sink_config.address);
    println!("Press Ctrl+C to stop.");

    sink.run(shutdown_rx).await?;

    println!("DataSink stopped.");
    Ok(())
}
