//! Emulator binary - publishes dummy event data via ZeroMQ
//!
//! Usage:
//!   cargo run --bin emulator                           # Use defaults
//!   cargo run --bin emulator -- --config config.toml   # Use config file
//!   cargo run --bin emulator -- --batches 10           # Run for 10 batches
//!   cargo run --bin emulator -- --source-id 1          # Use specific source

use clap::Parser;
use delila_rs::common::SourceArgs;
use delila_rs::config::Config;
use delila_rs::data_source_emulator::{Emulator, EmulatorConfig};
use tokio::sync::broadcast;
use tracing::info;
use tracing_subscriber::EnvFilter;

/// Emulator - publishes dummy event data via ZeroMQ
#[derive(Parser, Debug)]
#[command(name = "emulator", about = "DELILA data source emulator")]
struct Args {
    #[command(flatten)]
    source: SourceArgs,

    /// Run for N batches then send EOS and exit
    #[arg(short, long)]
    batches: Option<u64>,

    /// Batch interval in milliseconds
    #[arg(short, long)]
    interval: Option<u64>,

    /// Events per batch
    #[arg(short, long)]
    events: Option<usize>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing (logging)
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("delila_rs=info".parse()?))
        .init();

    // Parse command line arguments
    let args = Args::parse();

    // Build configuration
    let config_path = &args.source.common.config_file;
    let emulator_config = if std::path::Path::new(config_path).exists() {
        // Load from config file
        let config = Config::load(config_path)?;
        let settings = config.settings.get_settings()?;

        // Find source config by ID
        let sid = args.source.source_id.unwrap_or(0);
        let source_net = config.network.sources.iter().find(|s| s.id == sid);

        let bind_address = if let Some(addr) = &args.source.address {
            addr.clone()
        } else if let Some(src) = source_net {
            src.bind.clone()
        } else {
            format!("tcp://*:{}", 5555 + sid as u16)
        };

        info!(
            config_file = %config_path,
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
            events_per_batch: args.events.unwrap_or(settings.events_per_batch as usize),
            batch_interval_ms: args.interval.unwrap_or(settings.batch_interval_ms),
            heartbeat_interval_ms: 1000, // 1Hz heartbeat
            num_modules: settings.num_modules as u8,
            channels_per_module: settings.channels_per_module as u8,
            enable_waveform: settings.enable_waveform,
            waveform_probes: settings.waveform_probes,
            waveform_samples: settings.waveform_samples,
        }
    } else {
        // Use defaults with CLI overrides
        let sid = args.source.source_id.unwrap_or(0);
        EmulatorConfig {
            address: args
                .source
                .address
                .unwrap_or_else(|| "tcp://*:5555".to_string()),
            command_address: format!("tcp://*:{}", 5560 + sid as u16),
            source_id: sid,
            events_per_batch: args.events.unwrap_or(100),
            batch_interval_ms: args.interval.unwrap_or(100),
            heartbeat_interval_ms: 1000, // 1Hz heartbeat
            num_modules: 2,
            channels_per_module: 16,
            ..Default::default()
        }
    };

    // Create emulator
    let mut emulator = Emulator::new(emulator_config.clone()).await?;

    println!(
        "Emulator running. source_id={}, publishing to {}",
        emulator_config.source_id, emulator_config.address
    );

    if let Some(count) = args.batches {
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
