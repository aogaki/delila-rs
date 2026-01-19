//! Merger binary - receives from multiple emulators and forwards downstream
//!
//! Usage:
//!   cargo run --bin merger                      # Use config.toml
//!   cargo run --bin merger -- -f config.toml    # Explicit config file
//!   cargo run --bin merger -- -s tcp://localhost:5555 -p tcp://*:5557

use anyhow::Result;
use clap::Parser;
use delila_rs::common::{setup_shutdown, MergerArgs};
use delila_rs::config::Config;
use delila_rs::merger::{Merger, MergerConfig};
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "merger", about = "DELILA merger - receives from multiple sources and forwards downstream")]
struct Args {
    #[command(flatten)]
    merger: MergerArgs,
}

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

    let args = Args::parse();

    // Load configuration
    let config = Config::load(&args.merger.common.config_file)?;
    let merger_net = config
        .network
        .merger
        .ok_or_else(|| anyhow::anyhow!("No [network.merger] section in config file"))?;

    info!(config_file = %args.merger.common.config_file, "Loaded configuration");

    // CLI overrides config file
    let merger_config = MergerConfig {
        sub_addresses: if args.merger.sub_addresses.is_empty() {
            merger_net.subscribe
        } else {
            args.merger.sub_addresses
        },
        pub_address: args.merger.pub_address.unwrap_or(merger_net.publish),
        command_address: merger_net
            .command
            .unwrap_or_else(|| "tcp://*:5570".to_string()),
    };

    info!(?merger_config, "Starting merger");

    // Setup shutdown handling
    let (_shutdown_tx, shutdown_rx) = setup_shutdown();

    // Run merger
    let mut merger = Merger::new(merger_config);
    merger.run(shutdown_rx).await?;

    info!("Merger stopped");
    Ok(())
}
