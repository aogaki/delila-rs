//! REST API routes for DAQ control

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use tower_http::cors::{Any, CorsLayer};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::common::{ComponentMetrics, ComponentState, RunConfig};

use super::{
    ApiResponse, CommandResult, ComponentClient, ComponentConfig, ComponentStatus,
    ConfigureRequest, OperatorConfig, SystemState, SystemStatus,
};

/// Application state shared across handlers
pub struct AppState {
    pub client: ComponentClient,
    pub components: Vec<ComponentConfig>,
    pub config: OperatorConfig,
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
    )),
    tags(
        (name = "DAQ Control", description = "DAQ system control endpoints")
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
    let state = Arc::new(AppState {
        client: ComponentClient::new(),
        components,
        config,
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // API routes
        .route("/api/status", get(get_status))
        .route("/api/configure", post(configure))
        .route("/api/arm", post(arm))
        .route("/api/start", post(start))
        .route("/api/stop", post(stop))
        .route("/api/reset", post(reset))
        // Two-phase synchronized run control
        .route("/api/run/start", post(run_start))
        // Swagger UI
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .layer(cors)
        .with_state(state)
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
        Err(e) => {
            return (
                StatusCode::REQUEST_TIMEOUT,
                Json(ApiResponse::error(format!("Start phase failed: {}", e))),
            );
        }
        Ok(results) if results.iter().any(|r| !r.success) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error("Start phase failed").with_results(results)),
            );
        }
        Ok(results) => {
            return (
                StatusCode::OK,
                Json(
                    ApiResponse::success(format!(
                        "Run {} started successfully (all components synchronized)",
                        run_number
                    ))
                    .with_results(results),
                ),
            );
        }
    }
}
