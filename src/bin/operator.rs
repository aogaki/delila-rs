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
use std::path::PathBuf;

use clap::Parser;
use delila_rs::common::OperatorArgs;
use delila_rs::config::Config;
use delila_rs::operator::{
    create_router_with_config, create_router_with_mongodb, ComponentConfig, OperatorConfig,
    RunRepository,
};
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser, Debug)]
#[command(
    name = "operator",
    about = "DELILA operator - REST API server for DAQ system control"
)]
struct Args {
    #[command(flatten)]
    operator: OperatorArgs,

    /// MongoDB connection URI (optional)
    #[arg(long, env = "MONGODB_URI")]
    mongodb_uri: Option<String>,

    /// MongoDB database name
    #[arg(long, env = "MONGODB_DATABASE", default_value = "delila")]
    mongodb_database: String,
}

/// Load component configuration and operator config from config file or use defaults
fn load_config(config_file: &str) -> (Vec<ComponentConfig>, OperatorConfig) {
    // Try to load from config file
    if let Ok(config) = Config::load(config_file) {
        info!("Loaded configuration from {}", config_file);
        let components = build_components_from_config(&config);
        let operator_config = OperatorConfig {
            experiment_name: config.operator.experiment_name,
            ..OperatorConfig::default()
        };
        return (components, operator_config);
    }

    warn!(
        "Config file {} not found or invalid, using default addresses",
        config_file
    );

    // Default component configuration
    // pipeline_order: 1 = upstream (data sources), higher = downstream
    let components = vec![
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
    ];
    (components, OperatorConfig::default())
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

    // Load component and operator configuration
    let (components, operator_config) = load_config(&args.operator.common.config_file);
    info!("Loaded {} component(s)", components.len());
    for comp in &components {
        info!("  {} -> {}", comp.name, comp.address);
    }
    info!("Experiment name: {}", operator_config.experiment_name);

    // Connect to MongoDB if URI is provided
    let run_repo = if let Some(ref uri) = args.mongodb_uri {
        match RunRepository::connect(uri, &args.mongodb_database).await {
            Ok(repo) => {
                info!(
                    "Connected to MongoDB at {} (database: {})",
                    uri, args.mongodb_database
                );
                Some(repo)
            }
            Err(e) => {
                warn!("Failed to connect to MongoDB: {}. Run history will not be available.", e);
                None
            }
        }
    } else {
        info!("MongoDB not configured. Run history will not be available.");
        None
    };

    // Create router
    let app = if let Some(repo) = run_repo {
        create_router_with_mongodb(
            components,
            operator_config,
            PathBuf::from("./config/digitizers"),
            repo,
        )
    } else {
        // Without MongoDB, use with_config to pass experiment_name
        create_router_with_config(components, operator_config)
    };

    // Start server
    let port = args.operator.port;
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Starting Operator server on http://{}", addr);
    info!("Swagger UI: http://localhost:{}/swagger-ui/", port);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
