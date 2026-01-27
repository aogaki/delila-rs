//! REST API routes for DAQ control

mod digitizer;
mod emulator;
mod run;
mod status;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    routing::{get, post, put},
    Router,
};
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use utoipa::{OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

use crate::common::{ComponentMetrics, ComponentState};
use crate::config::{DigitizerConfig, Settings as ConfigSettings};

use super::{
    ApiResponse, CommandResult, ComponentClient, ComponentConfig, ComponentStatus,
    ConfigureRequest, CurrentRunInfo, DigitizerConfigRepository,
    LastRunInfo, OperatorConfig, RunNote, RunRepository, RunStats, RunStatus,
    StartRequest, SystemState, SystemStatus,
};

// Re-export public types from sub-modules (used in OpenAPI schemas)
pub use digitizer::{DetectResponse, DetectedDigitizer, DigitizerConfigHistoryItem, RestoreVersionRequest};
pub use run::{AddNoteRequest, NextRunNumberResponse};

// Import handler functions from sub-modules (used in router and ApiDoc)
use digitizer::{
    detect_digitizers, get_digitizer, get_digitizer_by_serial, get_digitizer_history,
    list_digitizers, restore_digitizer_version, save_all_digitizers, save_digitizer,
    save_digitizer_to_mongodb, update_digitizer,
};
use emulator::{get_emulator_settings, update_emulator_settings};
use run::{add_run_note, get_next_run_number, get_run, get_run_config_snapshot, get_run_history};
use status::{arm, configure, get_status, reset, run_start, start, stop};

/// Application state shared across handlers
pub struct AppState {
    pub client: ComponentClient,
    pub components: Vec<ComponentConfig>,
    pub config: OperatorConfig,
    /// Digitizer configurations (keyed by digitizer_id)
    pub digitizer_configs: RwLock<HashMap<u32, DigitizerConfig>>,
    /// Directory for storing digitizer config files
    pub config_dir: PathBuf,
    /// Run repository for MongoDB storage (optional)
    pub run_repo: Option<RunRepository>,
    /// Digitizer config repository for MongoDB (optional)
    pub digitizer_repo: Option<DigitizerConfigRepository>,
    /// Current run info (cached in memory for fast access)
    pub current_run: RwLock<Option<CurrentRunInfo>>,
    /// Emulator settings (runtime-configurable)
    pub emulator_settings: RwLock<EmulatorSettings>,
}

/// Emulator runtime settings (API model)
///
/// These settings can be changed via the API and will be applied
/// when the emulator is next started.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EmulatorSettings {
    /// Events per batch
    pub events_per_batch: u32,
    /// Batch interval in milliseconds (0 = maximum speed)
    pub batch_interval_ms: u64,
    /// Number of simulated modules
    pub num_modules: u32,
    /// Channels per module
    pub channels_per_module: u32,
    /// Enable waveform generation
    pub enable_waveform: bool,
    /// Waveform probe bitmask (1=analog1, 2=analog2, 3=both analog, 63=all)
    pub waveform_probes: u8,
    /// Number of waveform samples
    pub waveform_samples: u32,
}

impl Default for EmulatorSettings {
    fn default() -> Self {
        Self {
            events_per_batch: 5000,
            batch_interval_ms: 0,
            num_modules: 2,
            channels_per_module: 16,
            enable_waveform: false,
            waveform_probes: 3, // Both analog probes
            waveform_samples: 512,
        }
    }
}

impl From<&ConfigSettings> for EmulatorSettings {
    fn from(settings: &ConfigSettings) -> Self {
        Self {
            events_per_batch: settings.events_per_batch,
            batch_interval_ms: settings.batch_interval_ms,
            num_modules: settings.num_modules,
            channels_per_module: settings.channels_per_module,
            enable_waveform: settings.enable_waveform,
            waveform_probes: settings.waveform_probes,
            waveform_samples: settings.waveform_samples as u32,
        }
    }
}

/// OpenAPI documentation
#[derive(OpenApi)]
#[openapi(
    paths(
        status::get_status,
        status::configure,
        status::arm,
        status::start,
        status::stop,
        status::reset,
        status::run_start,
        digitizer::list_digitizers,
        digitizer::detect_digitizers,
        digitizer::get_digitizer_by_serial,
        digitizer::get_digitizer,
        digitizer::update_digitizer,
        digitizer::save_digitizer,
        digitizer::save_all_digitizers,
        digitizer::save_digitizer_to_mongodb,
        digitizer::get_digitizer_history,
        digitizer::restore_digitizer_version,
        run::get_run_config_snapshot,
        run::get_run_history,
        run::get_run,
        run::get_next_run_number,
        run::add_run_note,
        emulator::get_emulator_settings,
        emulator::update_emulator_settings,
    ),
    components(schemas(
        SystemStatus,
        ComponentStatus,
        SystemState,
        ComponentState,
        ComponentMetrics,
        ConfigureRequest,
        StartRequest,
        ApiResponse,
        CommandResult,
        DigitizerConfig,
        DetectedDigitizer,
        DetectResponse,
        CurrentRunInfo,
        RunStats,
        RunStatus,
        NextRunNumberResponse,
        AddNoteRequest,
        RunNote,
        LastRunInfo,
        EmulatorSettings,
        DigitizerConfigHistoryItem,
        RestoreVersionRequest,
    )),
    tags(
        (name = "DAQ Control", description = "DAQ system control endpoints"),
        (name = "Digitizer Config", description = "Digitizer configuration endpoints"),
        (name = "Run History", description = "Run history and statistics"),
        (name = "Emulator Settings", description = "Emulator runtime configuration")
    ),
    info(
        title = "DELILA DAQ Operator API",
        version = "1.0.0",
        description = "REST API for controlling the DELILA DAQ system"
    )
)]
struct ApiDoc;

/// Builder for creating the Operator API router.
///
/// All fields have sensible defaults; only `components` is required.
///
/// ```ignore
/// let app = RouterBuilder::new(components)
///     .config(operator_config)
///     .emulator_settings(settings)
///     .build();
/// ```
pub struct RouterBuilder {
    components: Vec<ComponentConfig>,
    config: OperatorConfig,
    config_dir: PathBuf,
    run_repo: Option<RunRepository>,
    digitizer_repo: Option<DigitizerConfigRepository>,
    emulator_settings: EmulatorSettings,
}

impl RouterBuilder {
    pub fn new(components: Vec<ComponentConfig>) -> Self {
        Self {
            components,
            config: OperatorConfig::default(),
            config_dir: PathBuf::from("./config/digitizers"),
            run_repo: None,
            digitizer_repo: None,
            emulator_settings: EmulatorSettings::default(),
        }
    }

    pub fn config(mut self, config: OperatorConfig) -> Self {
        self.config = config;
        self
    }

    pub fn config_dir(mut self, path: PathBuf) -> Self {
        self.config_dir = path;
        self
    }

    pub fn run_repo(mut self, repo: Option<RunRepository>) -> Self {
        self.run_repo = repo;
        self
    }

    pub fn digitizer_repo(mut self, repo: Option<DigitizerConfigRepository>) -> Self {
        self.digitizer_repo = repo;
        self
    }

    pub fn emulator_settings(mut self, settings: EmulatorSettings) -> Self {
        self.emulator_settings = settings;
        self
    }

    pub fn build(self) -> Router {
        let digitizer_configs = load_digitizer_configs(&self.config_dir).unwrap_or_default();

        let state = Arc::new(AppState {
            client: ComponentClient::new(),
            components: self.components,
            config: self.config,
            digitizer_configs: RwLock::new(digitizer_configs),
            config_dir: self.config_dir,
            run_repo: self.run_repo,
            digitizer_repo: self.digitizer_repo,
            current_run: RwLock::new(None),
            emulator_settings: RwLock::new(self.emulator_settings),
        });

        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        Router::new()
            // DAQ Control API routes
            .route("/api/status", get(get_status))
            .route("/api/configure", post(configure))
            .route("/api/arm", post(arm))
            .route("/api/start", post(start))
            .route("/api/stop", post(stop))
            .route("/api/reset", post(reset))
            // Two-phase synchronized run control
            .route("/api/run/start", post(run_start))
            // Run history routes
            .route("/api/runs", get(get_run_history))
            .route("/api/runs/next", get(get_next_run_number))
            .route("/api/runs/current/note", post(add_run_note))
            .route("/api/runs/:run_number", get(get_run))
            // Digitizer configuration routes
            .route("/api/digitizers", get(list_digitizers))
            .route("/api/digitizers/detect", post(detect_digitizers))
            .route("/api/digitizers/save-all", post(save_all_digitizers))
            .route(
                "/api/digitizers/by-serial/:serial",
                get(get_digitizer_by_serial),
            )
            .route("/api/digitizers/:id", get(get_digitizer))
            .route("/api/digitizers/:id", put(update_digitizer))
            .route("/api/digitizers/:id/save", post(save_digitizer))
            .route("/api/digitizers/:id/save-to-db", post(save_digitizer_to_mongodb))
            .route("/api/digitizers/:id/history", get(get_digitizer_history))
            .route("/api/digitizers/:id/restore", post(restore_digitizer_version))
            // Run config snapshots
            .route("/api/runs/:run_number/config", get(get_run_config_snapshot))
            // Emulator settings routes
            .route("/api/emulator", get(get_emulator_settings))
            .route("/api/emulator", put(update_emulator_settings))
            // Swagger UI
            .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
            .layer(cors)
            .with_state(state)
    }
}

/// Load digitizer configurations from JSON files in the config directory
fn load_digitizer_configs(config_dir: &PathBuf) -> std::io::Result<HashMap<u32, DigitizerConfig>> {
    let mut configs = HashMap::new();

    if !config_dir.exists() {
        return Ok(configs);
    }

    for entry in std::fs::read_dir(config_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().is_some_and(|ext| ext == "json") {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(config) = serde_json::from_str::<DigitizerConfig>(&content) {
                    configs.insert(config.digitizer_id, config);
                }
            }
        }
    }

    Ok(configs)
}
