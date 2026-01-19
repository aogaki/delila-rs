//! DAQ Operator - REST API server for system control
//!
//! Provides HTTP endpoints to control all DAQ components and Swagger UI for documentation.
//!
//! Usage:
//!   cargo run --bin operator                       # Use config.toml
//!   cargo run --bin operator -- -f config.toml     # Explicit config file
//!   cargo run --bin operator -- --port 8080
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

use clap::Parser;
use delila_rs::common::OperatorArgs;
use delila_rs::config::Config;
use delila_rs::operator::{create_router, ComponentConfig};
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser, Debug)]
#[command(name = "operator", about = "DELILA operator - REST API server for DAQ system control")]
struct Args {
    #[command(flatten)]
    operator: OperatorArgs,
}

/// Load component configuration from config file or use defaults
fn load_components(config_file: &str) -> Vec<ComponentConfig> {
    // Try to load from config file
    if let Ok(config) = Config::load(config_file) {
        info!("Loaded configuration from {}", config_file);
        return build_components_from_config(&config);
    }

    warn!(
        "Config file {} not found or invalid, using default addresses",
        config_file
    );

    // Default component configuration
    // pipeline_order: 1 = upstream (data sources), higher = downstream
    vec![
        ComponentConfig {
            name: "Emulator 0".to_string(),
            address: "tcp://localhost:5560".to_string(),
            pipeline_order: 1, // upstream (data source)
        },
        ComponentConfig {
            name: "Emulator 1".to_string(),
            address: "tcp://localhost:5561".to_string(),
            pipeline_order: 1, // upstream (data source)
        },
        ComponentConfig {
            name: "Merger".to_string(),
            address: "tcp://localhost:5570".to_string(),
            pipeline_order: 2, // middle
        },
        ComponentConfig {
            name: "Recorder".to_string(),
            address: "tcp://localhost:5580".to_string(),
            pipeline_order: 3, // downstream (data sink)
        },
        ComponentConfig {
            name: "Monitor".to_string(),
            address: "tcp://localhost:5590".to_string(),
            pipeline_order: 3, // downstream (data sink)
        },
    ]
}

/// Build ComponentConfig list from parsed Config
fn build_components_from_config(config: &Config) -> Vec<ComponentConfig> {
    let mut components = Vec::new();

    // Add sources (emulators/readers)
    for source in &config.network.sources {
        let name = if source.name.is_empty() {
            format!("Source {}", source.id)
        } else {
            source.name.clone()
        };
        // Convert bind address (tcp://*:port) to connect address (tcp://localhost:port)
        let address = source
            .command_address()
            .replace("tcp://*:", "tcp://localhost:");
        components.push(ComponentConfig {
            name,
            address,
            pipeline_order: source.pipeline_order,
        });
    }

    // Add merger
    if let Some(ref merger) = config.network.merger {
        let address = merger
            .command
            .clone()
            .unwrap_or_else(|| "tcp://*:5570".to_string())
            .replace("tcp://*:", "tcp://localhost:");
        components.push(ComponentConfig {
            name: "Merger".to_string(),
            address,
            pipeline_order: merger.pipeline_order,
        });
    }

    // Add recorder
    if let Some(ref recorder) = config.network.recorder {
        let address = recorder
            .command
            .clone()
            .unwrap_or_else(|| "tcp://*:5580".to_string())
            .replace("tcp://*:", "tcp://localhost:");
        components.push(ComponentConfig {
            name: "Recorder".to_string(),
            address,
            pipeline_order: recorder.pipeline_order,
        });
    }

    // Add monitor
    if let Some(ref monitor) = config.network.monitor {
        let address = monitor
            .command
            .clone()
            .unwrap_or_else(|| "tcp://*:5590".to_string())
            .replace("tcp://*:", "tcp://localhost:");
        components.push(ComponentConfig {
            name: "Monitor".to_string(),
            address,
            pipeline_order: monitor.pipeline_order,
        });
    }

    components
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Initialize logging
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    // Load component configuration
    let components = load_components(&args.operator.common.config_file);
    info!("Loaded {} component(s)", components.len());
    for comp in &components {
        info!("  {} -> {}", comp.name, comp.address);
    }

    // Create router
    let app = create_router(components);

    // Start server
    let port = args.operator.port;
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Starting Operator server on http://{}", addr);
    info!("Swagger UI: http://localhost:{}/swagger-ui/", port);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
