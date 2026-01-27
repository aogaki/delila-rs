//! Reader binary - reads data from CAEN digitizers via ZeroMQ
//!
//! Usage:
//!   cargo run --bin reader -- --url dig2://172.18.4.56
//!   cargo run --bin reader -- --url dig2://172.18.4.56 --source-id 0
//!   cargo run --bin reader -- --config config.toml --source-id 0

use delila_rs::config::Config;
use delila_rs::reader::{FirmwareType, Reader, ReaderConfig};
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
    let mut url: Option<String> = None;
    let mut source_id: u32 = 0;
    let mut data_address: Option<String> = None;
    let mut command_address: Option<String> = None;
    let mut module_id: Option<u8> = None;
    let mut time_step_ns: Option<f64> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-f" => {
                if i + 1 < args.len() {
                    config_path = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("Error: --config requires a file path");
                    std::process::exit(1);
                }
            }
            "--url" | "-u" => {
                if i + 1 < args.len() {
                    url = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("Error: --url requires a URL");
                    std::process::exit(1);
                }
            }
            "--source-id" | "-s" => {
                if i + 1 < args.len() {
                    source_id = args[i + 1].parse().expect("source-id must be a number");
                    i += 2;
                } else {
                    eprintln!("Error: --source-id requires a number");
                    std::process::exit(1);
                }
            }
            "--module-id" | "-m" => {
                if i + 1 < args.len() {
                    module_id = Some(args[i + 1].parse().expect("module-id must be a number"));
                    i += 2;
                } else {
                    eprintln!("Error: --module-id requires a number");
                    std::process::exit(1);
                }
            }
            "--data-address" | "-d" => {
                if i + 1 < args.len() {
                    data_address = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("Error: --data-address requires an address");
                    std::process::exit(1);
                }
            }
            "--command-address" | "-c" => {
                if i + 1 < args.len() {
                    command_address = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("Error: --command-address requires an address");
                    std::process::exit(1);
                }
            }
            "--time-step" | "-t" => {
                if i + 1 < args.len() {
                    time_step_ns = Some(args[i + 1].parse().expect("time-step must be a number"));
                    i += 2;
                } else {
                    eprintln!("Error: --time-step requires a number");
                    std::process::exit(1);
                }
            }
            "--help" | "-h" => {
                println!("Reader - reads data from CAEN digitizers");
                println!();
                println!("Usage: reader [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --config, -f <FILE>          Load configuration from TOML file");
                println!("  --url, -u <URL>              Digitizer URL (e.g., dig2://172.18.4.56)");
                println!("  --source-id, -s <ID>         Source ID (selects config from file) [default: 0]");
                println!("  --module-id, -m <ID>         Module ID for decoded events");
                println!("  --data-address, -d <ADDR>    ZMQ data publish address");
                println!("  --command-address, -c <ADDR> ZMQ command address");
                println!(
                    "  --time-step, -t <NS>         ADC time step in nanoseconds [default: 2.0]"
                );
                println!("  --help, -h                   Show this help message");
                println!();
                println!("Examples:");
                println!("  reader --config config.toml --source-id 0");
                println!("  reader --url dig2://172.18.4.56");
                println!("  reader --url dig2://172.18.4.56 --source-id 0 --module-id 0");
                return Ok(());
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    // Build configuration from file or CLI arguments
    let config = if let Some(path) = config_path {
        // Load from config file
        let file_config = Config::load(&path)?;

        // Get base config from file
        let mut reader_config =
            ReaderConfig::from_config(&file_config, source_id).unwrap_or_else(|| {
                eprintln!(
                    "Error: source {} not found in config file, or no digitizer_url specified",
                    source_id
                );
                eprintln!("Hint: Add digitizer_url to [[network.sources]] section");
                std::process::exit(1);
            });

        // Apply CLI overrides
        if let Some(u) = url {
            reader_config.url = u;
        }
        if let Some(d) = data_address {
            reader_config.data_address = d;
        }
        if let Some(c) = command_address {
            reader_config.command_address = c;
        }
        if let Some(m) = module_id {
            reader_config.module_id = m;
        }
        if let Some(t) = time_step_ns {
            reader_config.time_step_ns = t;
        }

        info!(config_file = %path, source_id, "Loaded configuration from file");
        reader_config
    } else {
        // No config file - require URL
        let url = url.unwrap_or_else(|| {
            eprintln!("Error: --url or --config is required");
            eprintln!("Usage: reader --url dig2://172.18.4.56");
            eprintln!("       reader --config config.toml --source-id 0");
            std::process::exit(1);
        });

        ReaderConfig {
            url: url.clone(),
            data_address: data_address
                .unwrap_or_else(|| format!("tcp://*:{}", 5555 + source_id as u16)),
            command_address: command_address
                .unwrap_or_else(|| format!("tcp://*:{}", 5560 + source_id as u16)),
            source_id,
            firmware: FirmwareType::PSD2,
            module_id: module_id.unwrap_or(source_id as u8),
            read_timeout_ms: 100,
            buffer_size: 1024 * 1024,
            heartbeat_interval_ms: 1000,
            time_step_ns: time_step_ns.unwrap_or(2.0),
            config_file: None, // No config file when using CLI directly
        }
    };

    info!(
        url = %config.url,
        source_id = config.source_id,
        data_address = %config.data_address,
        command_address = %config.command_address,
        "Reader configuration"
    );

    // Create reader
    let reader = Reader::new(config.clone()).await?;

    println!(
        "Reader running. source_id={}, url={}, publishing to {}",
        config.source_id, config.url, config.data_address
    );
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

    reader.run(shutdown_rx).await?;

    println!("Reader stopped.");
    Ok(())
}
