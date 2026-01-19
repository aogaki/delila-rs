//! DataSink binary - subscribes to event data via ZeroMQ
//!
//! Usage:
//!   cargo run --bin data_sink                       # Use config.toml
//!   cargo run --bin data_sink -- -f config.toml     # Explicit config file
//!   cargo run --bin data_sink -- -a tcp://localhost:5557

use clap::Parser;
use delila_rs::common::{setup_shutdown_with_message, DataSinkArgs};
use delila_rs::config::Config;
use delila_rs::data_sink::{DataSink, DataSinkConfig};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "data_sink",
    about = "DELILA data sink - subscribes to event data via ZeroMQ"
)]
struct Args {
    #[command(flatten)]
    sink: DataSinkArgs,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing (logging)
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("delila_rs=info".parse()?))
        .init();

    let args = Args::parse();

    // Load configuration
    let config = Config::load(&args.sink.common.config_file)?;
    info!(config_file = %args.sink.common.config_file, "Loaded configuration");

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

    // CLI overrides config file
    let sink_config = DataSinkConfig {
        address: args.sink.address.unwrap_or(subscribe_addr),
        command_address: command_addr,
        stats_interval_secs: 1,
        channel_capacity: 1000,
    };

    // Setup shutdown handling
    let (_shutdown_tx, shutdown_rx) =
        setup_shutdown_with_message("Received Ctrl+C, shutting down...");

    // Create and run data sink
    let mut sink = DataSink::new(sink_config.clone()).await?;

    println!("DataSink running. Connecting to {}", sink_config.address);
    println!("Press Ctrl+C to stop.");

    sink.run(shutdown_rx).await?;

    println!("DataSink stopped.");
    Ok(())
}
