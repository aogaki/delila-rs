//! REST API routes for DAQ control

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post, put},
    Json, Router,
};
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use utoipa::{OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

use crate::common::{Command, ComponentMetrics, ComponentState, EmulatorRuntimeConfig, RunConfig};
use crate::config::{DigitizerConfig, Settings as ConfigSettings};

use super::{
    ApiResponse, CommandResult, ComponentClient, ComponentConfig, ComponentStatus,
    ConfigureRequest, CurrentRunInfo, DigitizerConfigDocument, DigitizerConfigRepository,
    LastRunInfo, OperatorConfig, RunDocument, RunNote, RunRepository, RunStats, RunStatus,
    StartRequest, SystemState, SystemStatus,
};

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
        get_status,
        configure,
        arm,
        start,
        stop,
        reset,
        run_start,
        list_digitizers,
        get_digitizer,
        update_digitizer,
        save_digitizer,
        save_all_digitizers,
        save_digitizer_to_mongodb,
        get_digitizer_history,
        restore_digitizer_version,
        get_run_config_snapshot,
        get_run_history,
        get_run,
        get_next_run_number,
        add_run_note,
        get_emulator_settings,
        update_emulator_settings,
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

/// Create the axum router with all routes
pub fn create_router(components: Vec<ComponentConfig>) -> Router {
    create_router_with_config(components, OperatorConfig::default())
}

/// Create the axum router with custom configuration
pub fn create_router_with_config(
    components: Vec<ComponentConfig>,
    config: OperatorConfig,
) -> Router {
    create_router_full(
        components,
        config,
        PathBuf::from("./config/digitizers"),
        None,
        None,
    )
}

/// Create the axum router with MongoDB support
pub fn create_router_with_mongodb(
    components: Vec<ComponentConfig>,
    config: OperatorConfig,
    config_dir: PathBuf,
    run_repo: RunRepository,
    digitizer_repo: Option<DigitizerConfigRepository>,
) -> Router {
    create_router_full(components, config, config_dir, Some(run_repo), digitizer_repo)
}

/// Create the axum router with full configuration including config directory
pub fn create_router_full(
    components: Vec<ComponentConfig>,
    config: OperatorConfig,
    config_dir: PathBuf,
    run_repo: Option<RunRepository>,
    digitizer_repo: Option<DigitizerConfigRepository>,
) -> Router {
    create_router_with_emulator_settings(
        components,
        config,
        config_dir,
        run_repo,
        digitizer_repo,
        EmulatorSettings::default(),
    )
}

/// Create the axum router with emulator settings
pub fn create_router_with_emulator_settings(
    components: Vec<ComponentConfig>,
    config: OperatorConfig,
    config_dir: PathBuf,
    run_repo: Option<RunRepository>,
    digitizer_repo: Option<DigitizerConfigRepository>,
    emulator_settings: EmulatorSettings,
) -> Router {
    // Load existing digitizer configs from disk
    let digitizer_configs = load_digitizer_configs(&config_dir).unwrap_or_default();

    let state = Arc::new(AppState {
        client: ComponentClient::new(),
        components,
        config,
        digitizer_configs: RwLock::new(digitizer_configs),
        config_dir,
        run_repo,
        digitizer_repo,
        current_run: RwLock::new(None),
        emulator_settings: RwLock::new(emulator_settings),
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
        .route("/api/digitizers/save-all", post(save_all_digitizers))
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

/// Get system and component status
#[utoipa::path(
    get,
    path = "/api/status",
    tag = "DAQ Control",
    responses(
        (status = 200, description = "System status", body = SystemStatus)
    )
)]
async fn get_status(State(state): State<Arc<AppState>>) -> Json<SystemStatus> {
    let components = state.client.get_all_status(&state.components).await;
    let system_state = SystemState::from_components(&components);

    // Get current run info and update real-time values
    let run_info = {
        let cached = state.current_run.read().await.clone();
        if let Some(mut info) = cached {
            if info.status == RunStatus::Running {
                // Update elapsed time
                info.elapsed_secs = chrono::Utc::now()
                    .signed_duration_since(info.start_time)
                    .num_seconds();

                // Update stats from Recorder metrics (authoritative source for recorded data)
                let recorder_metrics = components
                    .iter()
                    .find(|c| c.name == "Recorder")
                    .and_then(|c| c.metrics.as_ref());
                let (total_events, total_bytes) = recorder_metrics
                    .map(|m| (m.events_processed as i64, m.bytes_transferred as i64))
                    .unwrap_or((0, 0));
                let average_rate = if info.elapsed_secs > 0 {
                    total_events as f64 / info.elapsed_secs as f64
                } else {
                    0.0
                };

                info.stats = RunStats {
                    total_events,
                    total_bytes,
                    average_rate,
                };
            }
            Some(info)
        } else {
            None
        }
    };

    // Get next run number and last run info from MongoDB (for multi-client sync)
    let (next_run_number, last_run_info) = if let Some(ref repo) = state.run_repo {
        let next = repo
            .get_next_run_number_for_experiment(&state.config.experiment_name)
            .await
            .ok();
        let last = match repo
            .get_last_run_info_for_experiment(&state.config.experiment_name)
            .await
        {
            Ok(info) => info,
            Err(e) => {
                tracing::warn!("Failed to get last_run_info: {}", e);
                None
            }
        };
        (next, last)
    } else {
        (None, None)
    };

    Json(SystemStatus {
        components,
        system_state,
        run_info,
        experiment_name: state.config.experiment_name.clone(),
        next_run_number,
        last_run_info,
    })
}

/// Configure all components for a run
#[utoipa::path(
    post,
    path = "/api/configure",
    tag = "DAQ Control",
    request_body = ConfigureRequest,
    responses(
        (status = 200, description = "Configuration result", body = ApiResponse),
        (status = 400, description = "Invalid request", body = ApiResponse)
    )
)]
async fn configure(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ConfigureRequest>,
) -> (StatusCode, Json<ApiResponse>) {
    let run_config: RunConfig = request.into();
    let run_number = run_config.run_number;
    let results = state
        .client
        .configure_all(&state.components, run_config)
        .await;

    let response = ApiResponse::success(format!("Configure command sent for run {}", run_number))
        .with_results(results);

    let status = if response.success {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };

    (status, Json(response))
}

/// Arm all components
#[utoipa::path(
    post,
    path = "/api/arm",
    tag = "DAQ Control",
    responses(
        (status = 200, description = "Arm result", body = ApiResponse),
        (status = 400, description = "Invalid state transition", body = ApiResponse)
    )
)]
async fn arm(State(state): State<Arc<AppState>>) -> (StatusCode, Json<ApiResponse>) {
    let results = state.client.arm_all(&state.components).await;

    let response = ApiResponse::success("Arm command sent").with_results(results);

    let status = if response.success {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };

    (status, Json(response))
}

/// Start data acquisition
///
/// If the system is in Configured state, this will automatically arm first,
/// then start. If already Armed, it will just start.
/// The run_number is passed at start time to allow changing it without re-configuring hardware.
#[utoipa::path(
    post,
    path = "/api/start",
    tag = "DAQ Control",
    request_body = StartRequest,
    responses(
        (status = 200, description = "Start result", body = ApiResponse),
        (status = 400, description = "Invalid state transition", body = ApiResponse)
    )
)]
async fn start(
    State(state): State<Arc<AppState>>,
    Json(request): Json<StartRequest>,
) -> (StatusCode, Json<ApiResponse>) {
    let run_number = request.run_number;
    let comment = request.comment;

    // Check current state
    let components = state.client.get_all_status(&state.components).await;
    let system_state = SystemState::from_components(&components);

    // If Configured, arm first
    if system_state == SystemState::Configured {
        match state
            .client
            .arm_all_sync(&state.components, state.config.arm_timeout_ms)
            .await
        {
            Ok(arm_results) => {
                let arm_response =
                    ApiResponse::success("Arm command sent").with_results(arm_results);
                if !arm_response.success {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ApiResponse::error("Auto-arm failed before start")),
                    );
                }
            }
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse::error(format!("Auto-arm failed: {}", e))),
                );
            }
        }
    }

    // Now start with the run number (sequential: wait for each component to reach Running)
    let start_result = state
        .client
        .start_all_sync(&state.components, run_number, state.config.start_timeout_ms)
        .await;

    let response = match start_result {
        Ok(results) => ApiResponse::success(format!("Start command sent for run {}", run_number))
            .with_results(results),
        Err(e) => {
            return (
                StatusCode::REQUEST_TIMEOUT,
                Json(ApiResponse::error(format!("Start failed: {}", e))),
            );
        }
    };

    let status = if response.success {
        let exp_name = &state.config.experiment_name;

        // Record run start in MongoDB and update current_run
        if let Some(ref repo) = state.run_repo {
            let mongo_start = std::time::Instant::now();
            match repo
                .start_run(run_number as i32, exp_name, &comment, None)
                .await
            {
                Ok(doc) => {
                    tracing::info!("MongoDB start_run took {:?}", mongo_start.elapsed());
                    let info = CurrentRunInfo::from_document(&doc);
                    *state.current_run.write().await = Some(info);
                }
                Err(e) => {
                    tracing::warn!("Failed to record run start in MongoDB: {}", e);
                    // Still set current_run for in-memory tracking
                    *state.current_run.write().await = Some(CurrentRunInfo {
                        run_number: run_number as i32,
                        exp_name: exp_name.clone(),
                        comment: comment.clone(),
                        start_time: chrono::Utc::now(),
                        elapsed_secs: 0,
                        status: RunStatus::Running,
                        stats: RunStats::default(),
                        notes: Vec::new(),
                    });
                }
            }
        }

        // Create digitizer config snapshot for this run
        if let Some(ref digitizer_repo) = state.digitizer_repo {
            let configs: Vec<_> = state
                .digitizer_configs
                .read()
                .await
                .values()
                .cloned()
                .collect();
            if !configs.is_empty() {
                if let Err(e) = digitizer_repo
                    .create_run_snapshot(run_number as i32, exp_name, configs)
                    .await
                {
                    tracing::warn!("Failed to create config snapshot: {}", e);
                }
            }
        } else {
            // No MongoDB, just track in memory
            *state.current_run.write().await = Some(CurrentRunInfo {
                run_number: run_number as i32,
                exp_name: exp_name.clone(),
                comment,
                start_time: chrono::Utc::now(),
                elapsed_secs: 0,
                status: RunStatus::Running,
                stats: RunStats::default(),
                notes: Vec::new(),
            });
        }
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };

    (status, Json(response))
}

/// Stop data acquisition
#[utoipa::path(
    post,
    path = "/api/stop",
    tag = "DAQ Control",
    responses(
        (status = 200, description = "Stop result", body = ApiResponse),
        (status = 400, description = "Invalid state transition", body = ApiResponse)
    )
)]
async fn stop(State(state): State<Arc<AppState>>) -> (StatusCode, Json<ApiResponse>) {
    // Get current run info before stopping
    let current_run = state.current_run.read().await.clone();

    let results = state.client.stop_all(&state.components).await;

    let response = ApiResponse::success("Stop command sent").with_results(results);

    let status = if response.success {
        // Record run end in MongoDB
        if let (Some(ref repo), Some(run_info)) = (&state.run_repo, current_run) {
            // Get final stats from components
            let components = state.client.get_all_status(&state.components).await;
            let total_events: i64 = components
                .iter()
                .filter_map(|c| c.metrics.as_ref())
                .map(|m| m.events_processed as i64)
                .sum();
            let total_bytes: i64 = components
                .iter()
                .filter_map(|c| c.metrics.as_ref())
                .map(|m| m.bytes_transferred as i64)
                .sum();
            let average_rate = if run_info.elapsed_secs > 0 {
                total_events as f64 / run_info.elapsed_secs as f64
            } else {
                0.0
            };

            let stats = RunStats {
                total_events,
                total_bytes,
                average_rate,
            };

            if let Err(e) = repo
                .end_run(
                    run_info.run_number,
                    &run_info.exp_name,
                    RunStatus::Completed,
                    stats,
                )
                .await
            {
                tracing::warn!("Failed to record run end in MongoDB: {}", e);
            }
        }

        // Clear current run
        *state.current_run.write().await = None;
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };

    (status, Json(response))
}

/// Reset all components to Idle state
#[utoipa::path(
    post,
    path = "/api/reset",
    tag = "DAQ Control",
    responses(
        (status = 200, description = "Reset result", body = ApiResponse)
    )
)]
async fn reset(State(state): State<Arc<AppState>>) -> (StatusCode, Json<ApiResponse>) {
    let results = state.client.reset_all(&state.components).await;

    let response = ApiResponse::success("Reset command sent").with_results(results);

    let status = if response.success {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };

    (status, Json(response))
}

/// Start a run with two-phase synchronization
///
/// This endpoint performs the complete run startup sequence:
/// 1. Configure all components (with sync)
/// 2. Arm all components (with sync - waits for all to be Armed)
/// 3. Start all components (with sync)
///
/// Each phase waits for all components to reach the expected state
/// before proceeding, with configurable timeouts.
#[utoipa::path(
    post,
    path = "/api/run/start",
    tag = "DAQ Control",
    request_body = ConfigureRequest,
    responses(
        (status = 200, description = "Run started successfully", body = ApiResponse),
        (status = 400, description = "Failed to start run", body = ApiResponse),
        (status = 408, description = "Timeout during synchronization", body = ApiResponse)
    )
)]
async fn run_start(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ConfigureRequest>,
) -> (StatusCode, Json<ApiResponse>) {
    let run_config: RunConfig = request.into();
    let run_number = run_config.run_number;

    // Phase 1: Configure
    let configure_result = state
        .client
        .configure_all_sync(
            &state.components,
            run_config,
            state.config.configure_timeout_ms,
        )
        .await;

    match configure_result {
        Err(e) => {
            return (
                StatusCode::REQUEST_TIMEOUT,
                Json(ApiResponse::error(format!("Configure phase failed: {}", e))),
            );
        }
        Ok(results) if results.iter().any(|r| !r.success) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error("Configure phase failed").with_results(results)),
            );
        }
        Ok(_) => {}
    }

    // Phase 2: Arm (sync point)
    let arm_result = state
        .client
        .arm_all_sync(&state.components, state.config.arm_timeout_ms)
        .await;

    match arm_result {
        Err(e) => {
            return (
                StatusCode::REQUEST_TIMEOUT,
                Json(ApiResponse::error(format!("Arm phase failed: {}", e))),
            );
        }
        Ok(results) if results.iter().any(|r| !r.success) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error("Arm phase failed").with_results(results)),
            );
        }
        Ok(_) => {}
    }

    // Phase 3: Start (with run_number)
    let start_result = state
        .client
        .start_all_sync(&state.components, run_number, state.config.start_timeout_ms)
        .await;

    match start_result {
        Err(e) => (
            StatusCode::REQUEST_TIMEOUT,
            Json(ApiResponse::error(format!("Start phase failed: {}", e))),
        ),
        Ok(results) if results.iter().any(|r| !r.success) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("Start phase failed").with_results(results)),
        ),
        Ok(results) => {
            // Create digitizer config snapshot for this run
            if let Some(ref digitizer_repo) = state.digitizer_repo {
                let exp_name = &state.config.experiment_name;
                let configs: Vec<_> = state
                    .digitizer_configs
                    .read()
                    .await
                    .values()
                    .cloned()
                    .collect();
                if !configs.is_empty() {
                    if let Err(e) = digitizer_repo
                        .create_run_snapshot(run_number as i32, exp_name, configs)
                        .await
                    {
                        tracing::warn!("Failed to create config snapshot: {}", e);
                    }
                }
            }

            (
                StatusCode::OK,
                Json(
                    ApiResponse::success(format!(
                        "Run {} started successfully (all components synchronized)",
                        run_number
                    ))
                    .with_results(results),
                ),
            )
        }
    }
}

// =============================================================================
// Digitizer Configuration Endpoints
// =============================================================================

/// List all digitizer configurations
#[utoipa::path(
    get,
    path = "/api/digitizers",
    tag = "Digitizer Config",
    responses(
        (status = 200, description = "List of digitizer configurations", body = Vec<DigitizerConfig>)
    )
)]
async fn list_digitizers(State(state): State<Arc<AppState>>) -> Json<Vec<DigitizerConfig>> {
    let configs = state.digitizer_configs.read().await;
    let mut list: Vec<DigitizerConfig> = configs.values().cloned().collect();
    list.sort_by_key(|c| c.digitizer_id);
    Json(list)
}

/// Get a specific digitizer configuration
#[utoipa::path(
    get,
    path = "/api/digitizers/{id}",
    tag = "Digitizer Config",
    params(
        ("id" = u32, Path, description = "Digitizer ID")
    ),
    responses(
        (status = 200, description = "Digitizer configuration", body = DigitizerConfig),
        (status = 404, description = "Digitizer not found", body = ApiResponse)
    )
)]
async fn get_digitizer(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
) -> Result<Json<DigitizerConfig>, (StatusCode, Json<ApiResponse>)> {
    let configs = state.digitizer_configs.read().await;

    configs.get(&id).cloned().map(Json).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error(format!("Digitizer {} not found", id))),
        )
    })
}

/// Update a digitizer configuration (in memory)
///
/// Updates the configuration in memory. Use POST /api/digitizers/{id}/save to persist to disk.
#[utoipa::path(
    put,
    path = "/api/digitizers/{id}",
    tag = "Digitizer Config",
    params(
        ("id" = u32, Path, description = "Digitizer ID")
    ),
    request_body = DigitizerConfig,
    responses(
        (status = 200, description = "Configuration updated", body = ApiResponse),
        (status = 400, description = "Invalid configuration", body = ApiResponse)
    )
)]
async fn update_digitizer(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
    Json(config): Json<DigitizerConfig>,
) -> (StatusCode, Json<ApiResponse>) {
    // Validate that the path ID matches the config ID
    if config.digitizer_id != id {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error(format!(
                "Path ID {} does not match config digitizer_id {}",
                id, config.digitizer_id
            ))),
        );
    }

    let mut configs = state.digitizer_configs.write().await;
    configs.insert(id, config);

    (
        StatusCode::OK,
        Json(ApiResponse::success(format!(
            "Digitizer {} configuration updated (not yet saved to disk)",
            id
        ))),
    )
}

/// Save a digitizer configuration to disk
#[utoipa::path(
    post,
    path = "/api/digitizers/{id}/save",
    tag = "Digitizer Config",
    params(
        ("id" = u32, Path, description = "Digitizer ID")
    ),
    responses(
        (status = 200, description = "Configuration saved", body = ApiResponse),
        (status = 404, description = "Digitizer not found", body = ApiResponse),
        (status = 500, description = "Failed to save", body = ApiResponse)
    )
)]
async fn save_digitizer(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
) -> (StatusCode, Json<ApiResponse>) {
    let configs = state.digitizer_configs.read().await;

    let config = match configs.get(&id) {
        Some(c) => c.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::error(format!("Digitizer {} not found", id))),
            );
        }
    };
    drop(configs); // Release read lock

    // Ensure config directory exists
    if let Err(e) = std::fs::create_dir_all(&state.config_dir) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to create config directory: {}",
                e
            ))),
        );
    }

    // Save to file
    let file_path = state.config_dir.join(format!("digitizer_{}.json", id));
    let json = match serde_json::to_string_pretty(&config) {
        Ok(j) => j,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::error(format!(
                    "Failed to serialize config: {}",
                    e
                ))),
            );
        }
    };

    if let Err(e) = std::fs::write(&file_path, json) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to write config file: {}",
                e
            ))),
        );
    }

    (
        StatusCode::OK,
        Json(ApiResponse::success(format!(
            "Digitizer {} configuration saved to {}",
            id,
            file_path.display()
        ))),
    )
}

/// Save all digitizer configurations to disk
///
/// Saves all in-memory digitizer configurations to disk files.
/// Call this before Configure to ensure all configs are persisted.
#[utoipa::path(
    post,
    path = "/api/digitizers/save-all",
    tag = "Digitizer Config",
    responses(
        (status = 200, description = "All configurations saved", body = ApiResponse),
        (status = 500, description = "Failed to save some configurations", body = ApiResponse)
    )
)]
async fn save_all_digitizers(State(state): State<Arc<AppState>>) -> (StatusCode, Json<ApiResponse>) {
    // Ensure config directory exists
    if let Err(e) = std::fs::create_dir_all(&state.config_dir) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to create config directory: {}",
                e
            ))),
        );
    }

    let configs = state.digitizer_configs.read().await;
    let mut saved = 0;
    let mut errors = Vec::new();

    for (id, config) in configs.iter() {
        let file_path = state.config_dir.join(format!("digitizer_{}.json", id));
        match serde_json::to_string_pretty(config) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&file_path, json) {
                    errors.push(format!("digitizer_{}: {}", id, e));
                } else {
                    saved += 1;
                }
            }
            Err(e) => {
                errors.push(format!("digitizer_{}: {}", id, e));
            }
        }
    }

    if errors.is_empty() {
        (
            StatusCode::OK,
            Json(ApiResponse::success(format!(
                "Saved {} digitizer configuration(s) to {}",
                saved,
                state.config_dir.display()
            ))),
        )
    } else {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Saved {} config(s), {} failed: {}",
                saved,
                errors.len(),
                errors.join(", ")
            ))),
        )
    }
}

/// Save a digitizer configuration to MongoDB (with version history)
#[utoipa::path(
    post,
    path = "/api/digitizers/{id}/save-to-db",
    tag = "Digitizer Config",
    params(
        ("id" = u32, Path, description = "Digitizer ID"),
        ("description" = Option<String>, Query, description = "Optional description of changes")
    ),
    responses(
        (status = 200, description = "Configuration saved to MongoDB", body = ApiResponse),
        (status = 404, description = "Digitizer not found", body = ApiResponse),
        (status = 503, description = "MongoDB not available", body = ApiResponse)
    )
)]
async fn save_digitizer_to_mongodb(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> (StatusCode, Json<ApiResponse>) {
    let repo = match &state.digitizer_repo {
        Some(r) => r,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ApiResponse::error("MongoDB not configured for digitizer configs")),
            );
        }
    };

    let configs = state.digitizer_configs.read().await;
    let config = match configs.get(&id) {
        Some(c) => c.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::error(format!("Digitizer {} not found", id))),
            );
        }
    };
    drop(configs);

    let description = params.get("description").cloned();

    match repo.save_config(config, "api", description).await {
        Ok(doc) => (
            StatusCode::OK,
            Json(ApiResponse::success(format!(
                "Digitizer {} config saved to MongoDB (version {})",
                id, doc.version
            ))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to save to MongoDB: {}", e))),
        ),
    }
}

/// Digitizer config history item (simplified for API response)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DigitizerConfigHistoryItem {
    pub version: u32,
    #[schema(value_type = String, format = "date-time")]
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub is_current: bool,
}

impl From<DigitizerConfigDocument> for DigitizerConfigHistoryItem {
    fn from(doc: DigitizerConfigDocument) -> Self {
        Self {
            version: doc.version,
            created_at: doc.created_at,
            created_by: doc.created_by,
            description: doc.description,
            is_current: doc.is_current,
        }
    }
}

/// Get version history for a digitizer configuration
#[utoipa::path(
    get,
    path = "/api/digitizers/{id}/history",
    tag = "Digitizer Config",
    params(
        ("id" = u32, Path, description = "Digitizer ID"),
        ("limit" = Option<i64>, Query, description = "Maximum versions to return (default: 20)")
    ),
    responses(
        (status = 200, description = "Configuration version history", body = Vec<DigitizerConfigHistoryItem>),
        (status = 503, description = "MongoDB not available", body = ApiResponse)
    )
)]
async fn get_digitizer_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<DigitizerConfigHistoryItem>>, (StatusCode, Json<ApiResponse>)> {
    let repo = state.digitizer_repo.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::error("MongoDB not configured for digitizer configs")),
        )
    })?;

    let limit = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    let history = repo.get_config_history(id, limit).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to get history: {}", e))),
        )
    })?;

    Ok(Json(history.into_iter().map(Into::into).collect()))
}

/// Request body for restoring a config version
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RestoreVersionRequest {
    /// Version number to restore
    pub version: u32,
}

/// Restore a specific version of digitizer configuration
#[utoipa::path(
    post,
    path = "/api/digitizers/{id}/restore",
    tag = "Digitizer Config",
    params(
        ("id" = u32, Path, description = "Digitizer ID")
    ),
    request_body = RestoreVersionRequest,
    responses(
        (status = 200, description = "Configuration restored", body = ApiResponse),
        (status = 404, description = "Version not found", body = ApiResponse),
        (status = 503, description = "MongoDB not available", body = ApiResponse)
    )
)]
async fn restore_digitizer_version(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
    Json(request): Json<RestoreVersionRequest>,
) -> (StatusCode, Json<ApiResponse>) {
    let repo = match &state.digitizer_repo {
        Some(r) => r,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ApiResponse::error("MongoDB not configured for digitizer configs")),
            );
        }
    };

    match repo.restore_version(id, request.version).await {
        Ok(doc) => {
            // Also update in-memory config
            let mut configs = state.digitizer_configs.write().await;
            configs.insert(id, doc.config);

            (
                StatusCode::OK,
                Json(ApiResponse::success(format!(
                    "Digitizer {} config restored from version {} (now version {})",
                    id, request.version, doc.version
                ))),
            )
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error(format!("Failed to restore version: {}", e))),
        ),
    }
}

/// Get the configuration snapshot for a specific run
#[utoipa::path(
    get,
    path = "/api/runs/{run_number}/config",
    tag = "Run History",
    params(
        ("run_number" = i32, Path, description = "Run number")
    ),
    responses(
        (status = 200, description = "Run configuration snapshot", body = Vec<DigitizerConfig>),
        (status = 404, description = "Snapshot not found", body = ApiResponse),
        (status = 503, description = "MongoDB not available", body = ApiResponse)
    )
)]
async fn get_run_config_snapshot(
    State(state): State<Arc<AppState>>,
    Path(run_number): Path<i32>,
) -> Result<Json<Vec<DigitizerConfig>>, (StatusCode, Json<ApiResponse>)> {
    let repo = state.digitizer_repo.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::error("MongoDB not configured for digitizer configs")),
        )
    })?;

    let snapshot = repo
        .get_run_snapshot(run_number, &state.config.experiment_name)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::error(format!("Failed to get snapshot: {}", e))),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::error(format!(
                    "No config snapshot found for run {}",
                    run_number
                ))),
            )
        })?;

    Ok(Json(snapshot.digitizer_configs))
}

// =============================================================================
// Run History Endpoints
// =============================================================================

/// Run history response item (simplified from RunDocument)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RunHistoryItem {
    pub run_number: i32,
    pub exp_name: String,
    pub comment: String,
    #[schema(value_type = String, format = "date-time")]
    pub start_time: chrono::DateTime<chrono::Utc>,
    #[schema(value_type = Option<String>, format = "date-time")]
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
    pub duration_secs: Option<i32>,
    pub status: RunStatus,
    pub stats: RunStats,
}

impl From<RunDocument> for RunHistoryItem {
    fn from(doc: RunDocument) -> Self {
        Self {
            run_number: doc.run_number,
            exp_name: doc.exp_name,
            comment: doc.comment,
            start_time: doc.start_time,
            end_time: doc.end_time,
            duration_secs: doc.duration_secs,
            status: doc.status,
            stats: doc.stats,
        }
    }
}

/// Get recent run history
#[utoipa::path(
    get,
    path = "/api/runs",
    tag = "Run History",
    params(
        ("limit" = Option<i64>, Query, description = "Maximum number of runs to return (default: 50)")
    ),
    responses(
        (status = 200, description = "Run history", body = Vec<RunHistoryItem>),
        (status = 503, description = "MongoDB not available", body = ApiResponse)
    )
)]
async fn get_run_history(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<RunHistoryItem>>, (StatusCode, Json<ApiResponse>)> {
    let repo = state.run_repo.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::error("MongoDB not configured")),
        )
    })?;

    let limit = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);

    let runs = repo.get_recent_runs(limit).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to get run history: {}",
                e
            ))),
        )
    })?;

    Ok(Json(runs.into_iter().map(Into::into).collect()))
}

/// Get a specific run by run number
#[utoipa::path(
    get,
    path = "/api/runs/{run_number}",
    tag = "Run History",
    params(
        ("run_number" = i32, Path, description = "Run number")
    ),
    responses(
        (status = 200, description = "Run details", body = RunHistoryItem),
        (status = 404, description = "Run not found", body = ApiResponse),
        (status = 503, description = "MongoDB not available", body = ApiResponse)
    )
)]
async fn get_run(
    State(state): State<Arc<AppState>>,
    Path(run_number): Path<i32>,
) -> Result<Json<RunHistoryItem>, (StatusCode, Json<ApiResponse>)> {
    let repo = state.run_repo.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::error("MongoDB not configured")),
        )
    })?;

    let run = repo.get_run(run_number).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to get run: {}", e))),
        )
    })?;

    run.map(|r| Json(r.into())).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error(format!("Run {} not found", run_number))),
        )
    })
}

/// Next run number response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct NextRunNumberResponse {
    pub next_run_number: i32,
}

/// Get the next available run number
#[utoipa::path(
    get,
    path = "/api/runs/next",
    tag = "Run History",
    responses(
        (status = 200, description = "Next run number", body = NextRunNumberResponse),
        (status = 503, description = "MongoDB not available", body = ApiResponse)
    )
)]
async fn get_next_run_number(
    State(state): State<Arc<AppState>>,
) -> Result<Json<NextRunNumberResponse>, (StatusCode, Json<ApiResponse>)> {
    let repo = state.run_repo.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::error("MongoDB not configured")),
        )
    })?;

    let next = repo.get_next_run_number().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to get next run number: {}",
                e
            ))),
        )
    })?;

    Ok(Json(NextRunNumberResponse {
        next_run_number: next,
    }))
}

/// Request body for adding a note
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AddNoteRequest {
    /// Note text to add
    pub text: String,
}

/// Add a note to the current running run
#[utoipa::path(
    post,
    path = "/api/runs/current/note",
    tag = "Run History",
    request_body = AddNoteRequest,
    responses(
        (status = 200, description = "Note added", body = RunNote),
        (status = 400, description = "No run is currently active", body = ApiResponse),
        (status = 503, description = "MongoDB not available", body = ApiResponse)
    )
)]
async fn add_run_note(
    State(state): State<Arc<AppState>>,
    Json(request): Json<AddNoteRequest>,
) -> Result<Json<RunNote>, (StatusCode, Json<ApiResponse>)> {
    // Check if there's a current run
    let current_run = state.current_run.read().await.clone();
    let run_info = current_run.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("No run is currently active")),
        )
    })?;

    if run_info.status != RunStatus::Running {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("Run is not in running state")),
        ));
    }

    let repo = state.run_repo.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::error("MongoDB not configured")),
        )
    })?;

    let note = repo
        .add_note(run_info.run_number, &request.text)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::error(format!("Failed to add note: {}", e))),
            )
        })?;

    // Update in-memory cache
    {
        let mut current = state.current_run.write().await;
        if let Some(ref mut info) = *current {
            info.notes.push(note.clone());
        }
    }

    Ok(Json(note))
}

// =============================================================================
// Emulator Settings Endpoints
// =============================================================================

/// Get current emulator settings
#[utoipa::path(
    get,
    path = "/api/emulator",
    tag = "Emulator Settings",
    responses(
        (status = 200, description = "Current emulator settings", body = EmulatorSettings)
    )
)]
async fn get_emulator_settings(State(state): State<Arc<AppState>>) -> Json<EmulatorSettings> {
    let settings = state.emulator_settings.read().await;
    Json(settings.clone())
}

/// Update emulator settings
///
/// Updates the emulator runtime settings. Changes will be applied
/// when the emulator is next started (via Configure).
#[utoipa::path(
    put,
    path = "/api/emulator",
    tag = "Emulator Settings",
    request_body = EmulatorSettings,
    responses(
        (status = 200, description = "Settings updated", body = ApiResponse),
        (status = 400, description = "Invalid settings", body = ApiResponse)
    )
)]
async fn update_emulator_settings(
    State(state): State<Arc<AppState>>,
    Json(new_settings): Json<EmulatorSettings>,
) -> (StatusCode, Json<ApiResponse>) {
    // Validate settings
    if new_settings.events_per_batch == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("events_per_batch must be > 0")),
        );
    }
    if new_settings.num_modules == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("num_modules must be > 0")),
        );
    }
    if new_settings.channels_per_module == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("channels_per_module must be > 0")),
        );
    }
    if new_settings.enable_waveform && new_settings.waveform_samples == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error(
                "waveform_samples must be > 0 when waveforms are enabled",
            )),
        );
    }

    // Update settings
    let new_settings_clone = new_settings.clone();
    {
        let mut settings = state.emulator_settings.write().await;
        *settings = new_settings;
    }

    // Send UpdateEmulatorConfig command to all Emulator components
    // (components with "Emulator" or "Source" in their name)
    let runtime_config = EmulatorRuntimeConfig {
        events_per_batch: new_settings_clone.events_per_batch,
        batch_interval_ms: new_settings_clone.batch_interval_ms,
        enable_waveform: new_settings_clone.enable_waveform,
        waveform_probes: new_settings_clone.waveform_probes,
        waveform_samples: new_settings_clone.waveform_samples,
    };
    let cmd = Command::UpdateEmulatorConfig(runtime_config);

    // Filter for Emulator/Source components
    let emulator_components: Vec<_> = state
        .components
        .iter()
        .filter(|c| {
            c.name.to_lowercase().contains("emulator")
                || c.name.to_lowercase().contains("source")
                || c.name.to_lowercase().contains("digitizer")
                || c.pipeline_order == 1 // upstream data source
        })
        .cloned()
        .collect();

    if emulator_components.is_empty() {
        return (
            StatusCode::OK,
            Json(ApiResponse::success(
                "Settings saved. No emulator components found to update.",
            )),
        );
    }

    // Send command to each emulator
    let mut updated_count = 0;
    let mut errors = Vec::new();

    for comp in &emulator_components {
        match state.client.send_command(&comp.address, &cmd).await {
            Ok(resp) if resp.success => {
                tracing::info!(component = %comp.name, "Emulator config updated");
                updated_count += 1;
            }
            Ok(resp) => {
                tracing::warn!(
                    component = %comp.name,
                    error = %resp.message,
                    "Failed to update emulator config"
                );
                errors.push(format!("{}: {}", comp.name, resp.message));
            }
            Err(e) => {
                tracing::warn!(
                    component = %comp.name,
                    error = %e,
                    "Failed to send config update command"
                );
                errors.push(format!("{}: {}", comp.name, e));
            }
        }
    }

    if errors.is_empty() {
        (
            StatusCode::OK,
            Json(ApiResponse::success(format!(
                "Settings updated and applied to {} emulator(s)",
                updated_count
            ))),
        )
    } else {
        (
            StatusCode::OK,
            Json(ApiResponse::success(format!(
                "Settings saved. Updated {}/{} emulators. Errors: {}",
                updated_count,
                emulator_components.len(),
                errors.join("; ")
            ))),
        )
    }
}
