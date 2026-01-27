//! Digitizer configuration handlers and response types

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::common::Command;
use crate::config::DigitizerConfig;

use super::super::{ApiResponse, DigitizerConfigDocument};
use super::AppState;

/// Result of detecting a single digitizer via hardware probe
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DetectedDigitizer {
    /// Component name (Reader) that found this digitizer
    pub component_name: String,
    /// Source ID from component config
    pub source_id: u32,
    /// Device info from hardware (model, serial_number, firmware_type, etc.)
    #[schema(value_type = Object)]
    pub device_info: serde_json::Value,
    /// Whether a saved config was found in MongoDB for this serial number
    pub config_found: bool,
    /// Existing config from MongoDB (if found by serial number)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<DigitizerConfig>,
}

/// Response from digitizer detect endpoint
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DetectResponse {
    /// Whether all detect operations succeeded
    pub success: bool,
    /// Human-readable summary message
    pub message: String,
    /// Detected digitizers with their device info and configs
    pub digitizers: Vec<DetectedDigitizer>,
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

/// Request body for restoring a config version
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RestoreVersionRequest {
    /// Version number to restore
    pub version: u32,
}

/// List all digitizer configurations
#[utoipa::path(
    get,
    path = "/api/digitizers",
    tag = "Digitizer Config",
    responses(
        (status = 200, description = "List of digitizer configurations", body = Vec<DigitizerConfig>)
    )
)]
pub(super) async fn list_digitizers(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<DigitizerConfig>> {
    let configs = state.digitizer_configs.read().await;
    let mut list: Vec<DigitizerConfig> = configs.values().cloned().collect();
    list.sort_by_key(|c| c.digitizer_id);
    Json(list)
}

/// Detect connected digitizer hardware
///
/// Sends a Detect command to all digitizer Reader components (Idle state only).
/// For each detected digitizer, looks up its serial number in MongoDB to find
/// a previously saved configuration.
///
/// This is an independent step -- it does NOT change any component's state.
#[utoipa::path(
    post,
    path = "/api/digitizers/detect",
    tag = "Digitizer Config",
    responses(
        (status = 200, description = "Detection results", body = DetectResponse)
    )
)]
pub(super) async fn detect_digitizers(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<DetectResponse>) {
    // Filter for physical digitizer components
    let digitizer_components: Vec<_> = state.components.iter().filter(|c| c.is_digitizer).collect();

    if digitizer_components.is_empty() {
        return (
            StatusCode::OK,
            Json(DetectResponse {
                success: true,
                message: "No digitizer components configured".to_string(),
                digitizers: vec![],
            }),
        );
    }

    let mut detected = Vec::new();
    let mut errors = Vec::new();

    for comp in &digitizer_components {
        match state
            .client
            .send_command(&comp.address, &Command::Detect)
            .await
        {
            Ok(resp) if resp.success => {
                if let Some(data) = resp.data {
                    // Try to look up config by serial number in MongoDB
                    let serial = data
                        .get("serial_number")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    let (config_found, config) = if let (Some(ref repo), Some(ref serial)) =
                        (&state.digitizer_repo, &serial)
                    {
                        match repo.get_config_by_serial(serial).await {
                            Ok(Some(doc)) => (true, Some(doc.config)),
                            Ok(None) => (false, None),
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to lookup config by serial {}: {}",
                                    serial,
                                    e
                                );
                                (false, None)
                            }
                        }
                    } else {
                        (false, None)
                    };

                    detected.push(DetectedDigitizer {
                        component_name: comp.name.clone(),
                        source_id: comp.source_id.unwrap_or(0),
                        device_info: data,
                        config_found,
                        config,
                    });
                }
            }
            Ok(resp) => {
                errors.push(format!("{}: {}", comp.name, resp.message));
            }
            Err(e) => {
                errors.push(format!("{}: {}", comp.name, e));
            }
        }
    }

    let message = if errors.is_empty() {
        format!("Detected {} digitizer(s)", detected.len())
    } else {
        format!(
            "Detected {} digitizer(s), {} error(s): {}",
            detected.len(),
            errors.len(),
            errors.join("; ")
        )
    };

    (
        StatusCode::OK,
        Json(DetectResponse {
            success: errors.is_empty(),
            message,
            digitizers: detected,
        }),
    )
}

/// Get a digitizer configuration by hardware serial number
///
/// Looks up the current (active) configuration in MongoDB by serial number.
/// Used to restore settings for a previously-seen digitizer.
#[utoipa::path(
    get,
    path = "/api/digitizers/by-serial/{serial}",
    tag = "Digitizer Config",
    params(
        ("serial" = String, Path, description = "Hardware serial number")
    ),
    responses(
        (status = 200, description = "Digitizer configuration", body = DigitizerConfig),
        (status = 404, description = "No config found for serial", body = ApiResponse),
        (status = 503, description = "MongoDB not available", body = ApiResponse)
    )
)]
pub(super) async fn get_digitizer_by_serial(
    State(state): State<Arc<AppState>>,
    Path(serial): Path<String>,
) -> Result<Json<DigitizerConfig>, (StatusCode, Json<ApiResponse>)> {
    let repo = state.digitizer_repo.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::error(
                "MongoDB not configured for digitizer configs",
            )),
        )
    })?;

    let doc = repo.get_config_by_serial(&serial).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to query MongoDB: {}",
                e
            ))),
        )
    })?;

    doc.map(|d| Json(d.config)).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error(format!(
                "No config found for serial number: {}",
                serial
            ))),
        )
    })
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
pub(super) async fn get_digitizer(
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
pub(super) async fn update_digitizer(
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
pub(super) async fn save_digitizer(
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
pub(super) async fn save_all_digitizers(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<ApiResponse>) {
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
pub(super) async fn save_digitizer_to_mongodb(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> (StatusCode, Json<ApiResponse>) {
    let repo = match &state.digitizer_repo {
        Some(r) => r,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ApiResponse::error(
                    "MongoDB not configured for digitizer configs",
                )),
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
            Json(ApiResponse::error(format!(
                "Failed to save to MongoDB: {}",
                e
            ))),
        ),
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
pub(super) async fn get_digitizer_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<DigitizerConfigHistoryItem>>, (StatusCode, Json<ApiResponse>)> {
    let repo = state.digitizer_repo.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::error(
                "MongoDB not configured for digitizer configs",
            )),
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
pub(super) async fn restore_digitizer_version(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
    Json(request): Json<RestoreVersionRequest>,
) -> (StatusCode, Json<ApiResponse>) {
    let repo = match &state.digitizer_repo {
        Some(r) => r,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ApiResponse::error(
                    "MongoDB not configured for digitizer configs",
                )),
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
            Json(ApiResponse::error(format!(
                "Failed to restore version: {}",
                e
            ))),
        ),
    }
}
