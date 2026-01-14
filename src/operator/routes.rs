//! REST API routes for DAQ control

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
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::common::{ComponentMetrics, ComponentState, RunConfig};
use crate::config::DigitizerConfig;

use super::{
    ApiResponse, CommandResult, ComponentClient, ComponentConfig, ComponentStatus,
    ConfigureRequest, OperatorConfig, SystemState, SystemStatus,
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
    ),
    components(schemas(
        SystemStatus,
        ComponentStatus,
        SystemState,
        ComponentState,
        ComponentMetrics,
        ConfigureRequest,
        ApiResponse,
        CommandResult,
        DigitizerConfig,
    )),
    tags(
        (name = "DAQ Control", description = "DAQ system control endpoints"),
        (name = "Digitizer Config", description = "Digitizer configuration endpoints")
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
    create_router_full(components, config, PathBuf::from("./config/digitizers"))
}

/// Create the axum router with full configuration including config directory
pub fn create_router_full(
    components: Vec<ComponentConfig>,
    config: OperatorConfig,
    config_dir: PathBuf,
) -> Router {
    // Load existing digitizer configs from disk
    let digitizer_configs = load_digitizer_configs(&config_dir).unwrap_or_default();

    let state = Arc::new(AppState {
        client: ComponentClient::new(),
        components,
        config,
        digitizer_configs: RwLock::new(digitizer_configs),
        config_dir,
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
        // Digitizer configuration routes
        .route("/api/digitizers", get(list_digitizers))
        .route("/api/digitizers/{id}", get(get_digitizer))
        .route("/api/digitizers/{id}", put(update_digitizer))
        .route("/api/digitizers/{id}/save", post(save_digitizer))
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

    Json(SystemStatus {
        components,
        system_state,
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

    let response =
        ApiResponse::success(format!("Configure command sent for run {}", run_number))
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
#[utoipa::path(
    post,
    path = "/api/start",
    tag = "DAQ Control",
    responses(
        (status = 200, description = "Start result", body = ApiResponse),
        (status = 400, description = "Invalid state transition", body = ApiResponse)
    )
)]
async fn start(State(state): State<Arc<AppState>>) -> (StatusCode, Json<ApiResponse>) {
    let results = state.client.start_all(&state.components).await;

    let response = ApiResponse::success("Start command sent").with_results(results);

    let status = if response.success {
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
    let results = state.client.stop_all(&state.components).await;

    let response = ApiResponse::success("Stop command sent").with_results(results);

    let status = if response.success {
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
                Json(
                    ApiResponse::error("Configure phase failed")
                        .with_results(results),
                ),
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

    // Phase 3: Start
    let start_result = state
        .client
        .start_all_sync(&state.components, state.config.start_timeout_ms)
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
        Ok(results) => (
            StatusCode::OK,
            Json(
                ApiResponse::success(format!(
                    "Run {} started successfully (all components synchronized)",
                    run_number
                ))
                .with_results(results),
            ),
        ),
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

    configs
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| {
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
                Json(ApiResponse::error(format!("Failed to serialize config: {}", e))),
            );
        }
    };

    if let Err(e) = std::fs::write(&file_path, json) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to write config file: {}", e))),
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
