//! Monitor binary - real-time histogram display via web browser
//!
//! Usage:
//!   cargo run --bin monitor                            # Use defaults
//!   cargo run --bin monitor -- --config config.toml    # Use config file
//!   cargo run --bin monitor -- --address tcp://localhost:5557 --port 8080

use delila_rs::config::Config;
use delila_rs::monitor::{HistogramConfig, Monitor, MonitorConfig};
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
    let mut port: Option<u16> = None;

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
            "--port" | "-p" => {
                if i + 1 < args.len() {
                    port = Some(args[i + 1].parse().expect("Invalid port number"));
                    i += 2;
                } else {
                    eprintln!("Error: --port requires a port number");
                    std::process::exit(1);
                }
            }
            "--help" | "-h" => {
                println!("Monitor - real-time histogram display via web browser");
                println!();
                println!("Usage: monitor [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --config, -c <FILE>   Load configuration from TOML file");
                println!("  --address, -a <ADDR>  ZMQ address to connect to (default: tcp://localhost:5557)");
                println!("  --port, -p <PORT>     HTTP server port (default: 8080)");
                println!("  --help, -h            Show this help message");
                println!();
                println!("Examples:");
                println!("  monitor --config config.toml");
                println!("  monitor --address tcp://localhost:5557 --port 8080");
                println!();
                println!("Web UI: http://localhost:<port>/");
                return Ok(());
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    // Build configuration
    let monitor_config = if let Some(path) = config_path {
        // Load from config file
        let config = Config::load(&path)?;

        let (subscribe_addr, http_port) = if let Some(ref monitor) = config.network.monitor {
            (monitor.subscribe.clone(), monitor.http_port)
        } else {
            ("tcp://localhost:5557".to_string(), 8081)
        };

        info!(config_file = %path, "Loaded configuration");

        MonitorConfig {
            subscribe_address: address.unwrap_or(subscribe_addr),
            command_address: "tcp://*:5590".to_string(),
            http_port: port.unwrap_or(http_port),
            histogram_config: HistogramConfig::default(),
            channel_capacity: 1000,
        }
    } else {
        // Use defaults with CLI overrides
        MonitorConfig {
            subscribe_address: address.unwrap_or_else(|| "tcp://localhost:5557".to_string()),
            command_address: "tcp://*:5590".to_string(),
            http_port: port.unwrap_or(8081),
            histogram_config: HistogramConfig::default(),
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

    // Create and run monitor
    let mut monitor = Monitor::new(monitor_config.clone()).await?;

    println!("========================================");
    println!("       DELILA Monitor Started");
    println!("========================================");
    println!();
    println!("  Subscribing to: {}", monitor_config.subscribe_address);
    println!(
        "  Web UI:         http://localhost:{}/",
        monitor_config.http_port
    );
    println!();
    println!("  Press Ctrl+C to stop.");
    println!("========================================");

    monitor.run(shutdown_rx).await?;

    println!("Monitor stopped.");
    Ok(())
}
