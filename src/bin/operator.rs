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
use delila_rs::config::{Config, InfluxDbConfig, MongoConfig};
use delila_rs::operator::{
    ComponentConfig, DigitizerConfigRepository, EmulatorSettings, OperatorConfig, RouterBuilder,
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

/// Load component configuration, operator config, emulator settings, and config file paths
#[allow(clippy::type_complexity)]
fn load_config(
    config_file: &str,
) -> (
    Vec<ComponentConfig>,
    OperatorConfig,
    EmulatorSettings,
    Vec<(u32, PathBuf)>,
    Option<InfluxDbConfig>,
    Option<MongoConfig>,
    Option<delila_rs::config::BacklogAutostopConfig>,
) {
    // Try to load from config file
    if let Ok(config) = Config::load(config_file) {
        info!("Loaded configuration from {}", config_file);
        let components = build_components_from_config(&config);
        // Resolve web_ui_dir: explicit TOML value, or auto-detect
        let web_ui_dir = config.operator.web_ui_dir.map(PathBuf::from).or_else(|| {
            let default_path = PathBuf::from("web/operator-ui/dist/operator-ui/browser");
            if default_path.exists() {
                Some(default_path)
            } else {
                None
            }
        });
        let monitor_http_port = config.network.monitor.as_ref().map(|m| m.http_port);
        let defaults = OperatorConfig::default();
        let operator_config = OperatorConfig {
            port: config.operator.port,
            experiment_name: config.operator.experiment_name,
            web_ui_dir,
            monitor_http_port,
            configure_timeout_ms: config
                .operator
                .configure_timeout_ms
                .unwrap_or(defaults.configure_timeout_ms),
            arm_timeout_ms: config
                .operator
                .arm_timeout_ms
                .unwrap_or(defaults.arm_timeout_ms),
            start_timeout_ms: config
                .operator
                .start_timeout_ms
                .unwrap_or(defaults.start_timeout_ms),
            reset_timeout_ms: config
                .operator
                .reset_timeout_ms
                .unwrap_or(defaults.reset_timeout_ms),
            elog: config.operator.elog,
            drain_stop_timeout_secs: config.operator.drain_stop_timeout_secs,
        };
        // Load emulator settings from config
        let emulator_settings = if let Ok(settings) = config.settings.get_settings() {
            EmulatorSettings::from(&settings)
        } else {
            EmulatorSettings::default()
        };
        // Extract (source_id, config_file) pairs from digitizer sources
        // TOML source id is the single source of truth for digitizer identification
        let config_files: Vec<(u32, PathBuf)> = config
            .network
            .sources
            .iter()
            .filter(|s| s.is_digitizer())
            .filter_map(|s| s.config_file.as_ref().map(|f| (s.id, PathBuf::from(f))))
            .collect();
        let influxdb_config = config.operator.influxdb;
        let mongodb_config = config.operator.mongodb;
        let backlog_autostop_config = config.operator.backlog_autostop;
        return (
            components,
            operator_config,
            emulator_settings,
            config_files,
            influxdb_config,
            mongodb_config,
            backlog_autostop_config,
        );
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
            pipeline_order: 1,

            source_id: Some(0),
            is_digitizer: false,
            config_file: None,
            source_type: None,
        },
        ComponentConfig {
            name: "Emulator 1".to_string(),
            address: "tcp://localhost:5561".to_string(),
            pipeline_order: 1,

            source_id: Some(1),
            is_digitizer: false,
            config_file: None,
            source_type: None,
        },
        ComponentConfig {
            name: "Merger".to_string(),
            address: "tcp://localhost:5570".to_string(),
            pipeline_order: 2,

            source_id: None,
            is_digitizer: false,
            config_file: None,
            source_type: None,
        },
        ComponentConfig {
            name: "Recorder".to_string(),
            address: "tcp://localhost:5580".to_string(),
            pipeline_order: 3,

            source_id: None,
            is_digitizer: false,
            config_file: None,
            source_type: None,
        },
        ComponentConfig {
            name: "Monitor".to_string(),
            address: "tcp://localhost:5590".to_string(),
            pipeline_order: 3,

            source_id: None,
            is_digitizer: false,
            config_file: None,
            source_type: None,
        },
    ];
    (
        components,
        OperatorConfig::default(),
        EmulatorSettings::default(),
        Vec::new(),
        None,
        None,
        None,
    )
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
        // Resolve bind address to connect address using source's host
        let address = source.command_connect_address_with_base(config.network.port_base_command);
        components.push(ComponentConfig {
            name,
            address,
            pipeline_order: source.pipeline_order,
            source_id: Some(source.id),
            is_digitizer: source.is_digitizer(),
            config_file: source.config_file.as_ref().map(PathBuf::from),
            source_type: Some(source.source_type.clone()),
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

            source_id: None,
            is_digitizer: false,
            config_file: None,
            source_type: None,
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

            source_id: None,
            is_digitizer: false,
            config_file: None,
            source_type: None,
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

            source_id: None,
            is_digitizer: false,
            config_file: None,
            source_type: None,
        });
    }

    // Add event builder
    if let Some(ref eb) = config.network.event_builder {
        let address = eb
            .command
            .clone()
            .unwrap_or_else(|| "tcp://*:5595".to_string())
            .replace("tcp://*:", "tcp://localhost:");
        components.push(ComponentConfig {
            name: "EventBuilder".to_string(),
            address,
            pipeline_order: eb.pipeline_order,

            source_id: None,
            is_digitizer: false,
            config_file: None,
            source_type: None,
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

    // Load component, operator, and emulator configuration
    let (
        components,
        operator_config,
        emulator_settings,
        config_files,
        influxdb_config,
        mongodb_toml,
        backlog_autostop_config,
    ) = load_config(&args.operator.common.config_file);
    // Resolve MongoDB settings: CLI flags win, fall back to TOML `[operator.mongodb]`.
    let resolved_mongo_uri: Option<String> = args
        .mongodb_uri
        .clone()
        .or_else(|| mongodb_toml.as_ref().map(|m| m.uri.clone()));
    let resolved_mongo_db: String = if args.mongodb_uri.is_some() {
        // CLI wins for the uri ⇒ also use the CLI database (defaulted to "delila").
        args.mongodb_database.clone()
    } else if let Some(ref m) = mongodb_toml {
        m.database.clone()
    } else {
        args.mongodb_database.clone()
    };
    info!("Loaded {} component(s)", components.len());
    for comp in &components {
        info!("  {} -> {}", comp.name, comp.address);
    }
    info!("Experiment name: {}", operator_config.experiment_name);
    if let Some(ref ui_dir) = operator_config.web_ui_dir {
        info!("Web UI directory: {}", ui_dir.display());
    } else {
        info!("Web UI: not configured (use web_ui_dir in [operator] or build Angular app)");
    }
    info!(
        "Emulator settings: {} events/batch, {}ms interval",
        emulator_settings.events_per_batch, emulator_settings.batch_interval_ms
    );

    // Connect to MongoDB if URI is provided (CLI > TOML).
    let (run_repo, digitizer_repo) = if let Some(ref uri) = resolved_mongo_uri {
        use mongodb::options::ClientOptions;
        use mongodb::Client;

        // Try to connect to MongoDB
        let connect_result: Option<(RunRepository, DigitizerConfigRepository)> = async {
            let options = ClientOptions::parse(uri).await.ok()?;
            let client = Client::with_options(options).ok()?;

            // Test connection
            client
                .database("admin")
                .run_command(mongodb::bson::doc! { "ping": 1 })
                .await
                .ok()?;

            info!(
                "Connected to MongoDB at {} (database: {})",
                uri, resolved_mongo_db
            );

            let run_repo = RunRepository::new(&client, &resolved_mongo_db);
            let digitizer_repo = DigitizerConfigRepository::new(&client, &resolved_mongo_db);
            Some((run_repo, digitizer_repo))
        }
        .await;

        match connect_result {
            Some((run_repo, digitizer_repo)) => (Some(run_repo), Some(digitizer_repo)),
            None => {
                warn!("Failed to connect to MongoDB. Run history will not be available.");
                (None, None)
            }
        }
    } else {
        info!("MongoDB not configured. Run history will not be available.");
        (None, None)
    };

    // Resolve port (CLI --port overrides TOML config)
    let port = args.operator.port.unwrap_or(operator_config.port);

    // Create router with builder
    let (app, app_state) = RouterBuilder::new(components)
        .config(operator_config)
        .config_dir(PathBuf::from("./config/digitizers"))
        .config_files(config_files)
        .run_repo(run_repo)
        .digitizer_repo(digitizer_repo)
        .emulator_settings(emulator_settings)
        .build();

    // Start InfluxDB writer background task if configured
    if let Some(influxdb_config) = influxdb_config {
        let state = app_state.clone();
        tokio::spawn(async move {
            delila_rs::operator::influxdb::run_writer(influxdb_config, state).await;
        });
    }

    // Backlog autostop watcher (TODO 68) — opt-in via [operator.backlog_autostop]
    if let Some(backlog_config) = backlog_autostop_config {
        let state = app_state.clone();
        tokio::spawn(async move {
            delila_rs::operator::backlog_watch::run_watcher(backlog_config, state).await;
        });
    }

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Starting Operator server on http://{}", addr);
    info!("Swagger UI: http://localhost:{}/swagger-ui/", port);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
