//! Recorder binary - writes event data to files
//!
//! Usage:
//!   cargo run --bin recorder                       # Use config.toml
//!   cargo run --bin recorder -- -f config.toml     # Explicit config file
//!   cargo run --bin recorder -- -a tcp://localhost:5557 -o ./data

use std::path::PathBuf;

use clap::Parser;
use delila_rs::common::{setup_shutdown_with_message, RecorderArgs};
use delila_rs::config::Config;
use delila_rs::recorder::{Recorder, RecorderConfig};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "recorder",
    about = "DELILA recorder - writes event data to files"
)]
struct Args {
    #[command(flatten)]
    recorder: RecorderArgs,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing (logging)
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("delila_rs=info".parse()?))
        .init();

    let args = Args::parse();

    // Load configuration
    let config = Config::load(&args.recorder.common.config_file)?;
    info!(config_file = %args.recorder.common.config_file, "Loaded configuration");

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

    // CLI overrides config file
    let recorder_config = RecorderConfig {
        subscribe_address: args.recorder.address.unwrap_or(subscribe_addr),
        command_address: command_addr,
        output_dir: PathBuf::from(args.recorder.output_dir.unwrap_or(out_dir)),
        max_file_size: max_size_mb * 1024 * 1024,
        max_file_duration_secs: max_duration_sec,
    };

    // Setup shutdown handling
    let (_shutdown_tx, shutdown_rx) =
        setup_shutdown_with_message("Received Ctrl+C, shutting down...");

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
