//! DAQ Operator - REST API server for system control
//!
//! Provides HTTP endpoints to control all DAQ components and Swagger UI for documentation.
//!
//! Usage:
//!   cargo run --bin operator
//!   cargo run --bin operator -- --port 8080
//!   cargo run --bin operator -- --config config.toml
//!
//! Endpoints:
//!   GET  /api/status    - Get system status
//!   POST /api/configure - Configure all components
//!   POST /api/arm       - Arm all components
//!   POST /api/start     - Start data acquisition
//!   POST /api/stop      - Stop data acquisition
//!   POST /api/reset     - Reset all components
//!
//! Swagger UI: http://localhost:8080/swagger-ui/

use std::net::SocketAddr;

use delila_rs::operator::{create_router, ComponentConfig};
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

fn print_usage() {
    println!("DAQ Operator - REST API server for system control");
    println!();
    println!("Usage: operator [options]");
    println!();
    println!("Options:");
    println!("  --port <port>     HTTP server port (default: 8080)");
    println!("  --config <file>   Configuration file (default: config.toml)");
    println!("  -h, --help        Show this help");
    println!();
    println!("Endpoints:");
    println!("  GET  /api/status    - Get system and component status");
    println!("  POST /api/configure - Configure all components (body: {{\"run_number\": N}})");
    println!("  POST /api/arm       - Arm all components");
    println!("  POST /api/start     - Start data acquisition");
    println!("  POST /api/stop      - Stop data acquisition");
    println!("  POST /api/reset     - Reset all components to Idle");
    println!();
    println!("Swagger UI: http://localhost:<port>/swagger-ui/");
}

/// Parse command line arguments
fn parse_args() -> Option<(u16, String)> {
    let args: Vec<String> = std::env::args().collect();

    let mut port: u16 = 8080;
    let mut config_file = "config.toml".to_string();
    let mut i = 1;

    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                if i + 1 < args.len() {
                    port = args[i + 1].parse().unwrap_or_else(|_| {
                        eprintln!("Invalid port number: {}", args[i + 1]);
                        std::process::exit(1);
                    });
                    i += 2;
                } else {
                    eprintln!("--port requires a number");
                    std::process::exit(1);
                }
            }
            "--config" => {
                if i + 1 < args.len() {
                    config_file = args[i + 1].clone();
                    i += 2;
                } else {
                    eprintln!("--config requires a file path");
                    std::process::exit(1);
                }
            }
            "-h" | "--help" => {
                print_usage();
                return None;
            }
            _ => {
                eprintln!("Unknown option: {}", args[i]);
                print_usage();
                std::process::exit(1);
            }
        }
    }

    Some((port, config_file))
}

/// Load component configuration from config file or use defaults
fn load_components(config_file: &str) -> Vec<ComponentConfig> {
    // Try to read config file for component addresses
    // For now, use default addresses matching config.example.toml
    if std::path::Path::new(config_file).exists() {
        info!("Config file {} exists, using default component addresses", config_file);
    } else {
        info!("Config file {} not found, using default addresses", config_file);
    }

    // Default component configuration
    vec![
        ComponentConfig {
            name: "Emulator 0".to_string(),
            address: "tcp://localhost:5560".to_string(),
        },
        ComponentConfig {
            name: "Emulator 1".to_string(),
            address: "tcp://localhost:5561".to_string(),
        },
        ComponentConfig {
            name: "Merger".to_string(),
            address: "tcp://localhost:5570".to_string(),
        },
        ComponentConfig {
            name: "DataSink".to_string(),
            address: "tcp://localhost:5580".to_string(),
        },
    ]
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse arguments
    let (port, config_file) = match parse_args() {
        Some(args) => args,
        None => return Ok(()),
    };

    // Initialize logging
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    // Load component configuration
    let components = load_components(&config_file);
    info!("Loaded {} component(s)", components.len());
    for comp in &components {
        info!("  {} -> {}", comp.name, comp.address);
    }

    // Create router
    let app = create_router(components);

    // Start server
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Starting Operator server on http://{}", addr);
    info!("Swagger UI: http://localhost:{}/swagger-ui/", port);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
