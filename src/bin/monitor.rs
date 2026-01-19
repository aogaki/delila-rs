//! Monitor binary - real-time histogram display via web browser
//!
//! Usage:
//!   cargo run --bin monitor                       # Use config.toml
//!   cargo run --bin monitor -- -f config.toml     # Explicit config file
//!   cargo run --bin monitor -- -a tcp://localhost:5557 -p 8080

use clap::Parser;
use delila_rs::common::{setup_shutdown_with_message, MonitorArgs};
use delila_rs::config::Config;
use delila_rs::monitor::{HistogramConfig, Monitor, MonitorConfig};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "monitor", about = "DELILA monitor - real-time histogram display via web browser")]
struct Args {
    #[command(flatten)]
    monitor: MonitorArgs,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing (logging)
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("delila_rs=info".parse()?))
        .init();

    let args = Args::parse();

    // Load configuration
    let config = Config::load(&args.monitor.common.config_file)?;
    info!(config_file = %args.monitor.common.config_file, "Loaded configuration");

    let (subscribe_addr, http_port) = if let Some(ref monitor) = config.network.monitor {
        (monitor.subscribe.clone(), monitor.http_port)
    } else {
        ("tcp://localhost:5557".to_string(), 8081)
    };

    // CLI overrides config file
    let monitor_config = MonitorConfig {
        subscribe_address: args.monitor.address.unwrap_or(subscribe_addr),
        command_address: "tcp://*:5590".to_string(),
        http_port: args.monitor.port.unwrap_or(http_port),
        histogram_config: HistogramConfig::default(),
        channel_capacity: 1000,
    };

    // Setup shutdown handling
    let (_shutdown_tx, shutdown_rx) =
        setup_shutdown_with_message("Received Ctrl+C, shutting down...");

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
