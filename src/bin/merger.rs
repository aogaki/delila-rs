//! Merger binary - receives from multiple emulators and forwards downstream
//!
//! Usage:
//!   cargo run --bin merger                                    # Use defaults
//!   cargo run --bin merger -- --config config.toml            # Use config file
//!   cargo run --bin merger -- -s tcp://localhost:5555 -p tcp://*:5557

use anyhow::Result;
use delila_rs::config::Config;
use delila_rs::merger::{Merger, MergerConfig};
use tokio::signal;
use tokio::sync::broadcast;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("delila_rs=debug".parse()?)
                .add_directive("merger=debug".parse()?),
        )
        .init();

    // Parse command line arguments
    let args: Vec<String> = std::env::args().collect();
    let mut config_path: Option<String> = None;
    let mut sub_addresses: Vec<String> = Vec::new();
    let mut pub_address: Option<String> = None;

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
            "--sub" | "-s" => {
                if i + 1 < args.len() {
                    sub_addresses.push(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("Error: --sub requires an address");
                    std::process::exit(1);
                }
            }
            "--pub" | "-p" => {
                if i + 1 < args.len() {
                    pub_address = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("Error: --pub requires an address");
                    std::process::exit(1);
                }
            }
            "--help" | "-h" => {
                println!("Merger - receives from multiple upstream sources and forwards downstream");
                println!();
                println!("Usage: merger [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --config, -c <FILE>  Load configuration from TOML file");
                println!("  --sub, -s <ADDR>     Subscribe to upstream address (can specify multiple)");
                println!("  --pub, -p <ADDR>     Publish to downstream address");
                println!("  --help, -h           Show this help message");
                println!();
                println!("Examples:");
                println!("  merger --config config.toml");
                println!("  merger -s tcp://localhost:5555 -s tcp://localhost:5556 -p tcp://*:5557");
                return Ok(());
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    // Build configuration
    let merger_config = if let Some(path) = config_path {
        // Load from config file
        let config = Config::load(&path)?;
        let merger_net = config
            .network
            .merger
            .ok_or_else(|| anyhow::anyhow!("No [network.merger] section in config file"))?;

        info!(config_file = %path, "Loaded configuration");

        // CLI overrides config file
        MergerConfig {
            sub_addresses: if sub_addresses.is_empty() {
                merger_net.subscribe
            } else {
                sub_addresses
            },
            pub_address: pub_address.unwrap_or(merger_net.publish),
            command_address: merger_net.command.unwrap_or_else(|| "tcp://*:5570".to_string()),
            channel_capacity: merger_net.channel_capacity,
        }
    } else {
        // Use defaults with CLI overrides
        if sub_addresses.is_empty() {
            sub_addresses.push("tcp://localhost:5555".to_string());
        }

        MergerConfig {
            sub_addresses,
            pub_address: pub_address.unwrap_or_else(|| "tcp://*:5556".to_string()),
            command_address: "tcp://*:5570".to_string(),
            channel_capacity: 1000,
        }
    };

    info!(?merger_config, "Starting merger");

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    // Spawn shutdown signal handler
    tokio::spawn(async move {
        signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
        info!("Ctrl+C received, initiating shutdown");
        let _ = shutdown_tx.send(());
    });

    // Run merger
    let mut merger = Merger::new(merger_config);
    merger.run(shutdown_rx).await?;

    info!("Merger stopped");
    Ok(())
}
