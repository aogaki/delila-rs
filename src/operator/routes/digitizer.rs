//! Digitizer configuration handlers and response types

use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::common::Command;
use crate::config::{BoardConfig, ChannelConfig, DigitizerConfig, FirmwareType};

use super::super::{ApiResponse, DigitizerConfigDocument, DigitizerConfigRepository};
use super::AppState;

// =============================================================================
// Handler helpers (R-P5 — extracted from the parallel CRUD handlers below)
// =============================================================================

/// Build the canonical 503 response for MongoDB-backed routes when no
/// repository is configured.
fn mongodb_unavailable() -> (StatusCode, Json<ApiResponse>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ApiResponse::error(
            "MongoDB not configured for digitizer configs",
        )),
    )
}

/// Borrow the digitizer config repository or short-circuit with 503.
///
/// Used by the four MongoDB routes in this file (`get_digitizer_by_serial`,
/// `save_digitizer_to_mongodb`, `get_digitizer_history`,
/// `restore_digitizer_version`) plus `run.rs::get_run_config_snapshot`,
/// all of which share the same precondition.
pub(super) fn require_digitizer_repo(
    state: &AppState,
) -> Result<&DigitizerConfigRepository, (StatusCode, Json<ApiResponse>)> {
    state
        .digitizer_repo
        .as_ref()
        .ok_or_else(mongodb_unavailable)
}

/// Look up an in-memory digitizer config by id, cloning it. Returns the
/// shared "Digitizer {id} not found" 404 on miss.
///
/// `DashMap::get` returns a `Ref` guard scoped to the `.value().clone()`
/// expression, so the caller never holds a shard lock across `.await`.
fn require_digitizer_config(
    state: &AppState,
    id: u32,
) -> Result<DigitizerConfig, (StatusCode, Json<ApiResponse>)> {
    state
        .digitizer_configs
        .get(&id)
        .map(|r| r.value().clone())
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::error(format!("Digitizer {} not found", id))),
            )
        })
}

/// Reject mutating requests where the URL path id and the request body's
/// `digitizer_id` disagree (used by PUT and `apply`).
fn reject_path_id_mismatch(
    id: u32,
    config: &DigitizerConfig,
) -> Result<(), (StatusCode, Json<ApiResponse>)> {
    if config.digitizer_id == id {
        Ok(())
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error(format!(
                "Path ID {} does not match config digitizer_id {}",
                id, config.digitizer_id
            ))),
        ))
    }
}

/// Resolve the on-disk path for a digitizer's saved JSON config. Prefers
/// the TOML-specified `config_file` (so the Reader and the operator agree
/// on which file Configure will read), falling back to
/// `<config_dir>/digitizer_<id>.json` when no component owns that id.
fn resolve_config_path(state: &AppState, id: u32) -> PathBuf {
    state
        .components
        .iter()
        .find(|c| c.is_digitizer && c.source_id == Some(id))
        .and_then(|c| c.config_file.clone())
        .unwrap_or_else(|| state.config_dir.join(format!("digitizer_{}.json", id)))
}

/// Sanitize the config for its firmware, ensure the destination directory
/// exists, and write the pretty-printed JSON. Returns the underlying
/// I/O / serialization error verbatim so callers can decide between
/// "fail the request" (POST `/save`, POST `/save-all`) and "log and
/// continue" (POST `/apply` best-effort).
fn write_digitizer_config(file_path: &StdPath, mut config: DigitizerConfig) -> std::io::Result<()> {
    config.sanitize_for_firmware();
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&config).map_err(std::io::Error::other)?;
    std::fs::write(file_path, json)
}

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
    let mut list: Vec<DigitizerConfig> = state
        .digitizer_configs
        .iter()
        .map(|r| r.value().clone())
        .collect();
    list.sort_by_key(|c| c.digitizer_id);
    Json(list)
}

/// Detect connected digitizer hardware
///
/// Sends a Detect command to all digitizer Reader components (Idle or Configured state).
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
                    let source_id = comp.source_id.unwrap_or(0);

                    // Try to look up config by serial number in MongoDB
                    let serial = data
                        .get("serial_number")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    let (config_found, mut config) = if let (Some(ref repo), Some(ref serial)) =
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

                    // If no config from MongoDB, check in-memory configs or create default
                    if config.is_none() {
                        if let Some(existing) = state.digitizer_configs.get(&source_id) {
                            // Use existing config from JSON file, update hardware info
                            let mut c = existing.value().clone();
                            c.serial_number = serial.clone();
                            c.model = data
                                .get("model")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            if let Some(n) = data.get("num_channels").and_then(|v| v.as_u64()) {
                                c.num_channels = n as u8;
                            }
                            config = Some(c);
                        } else {
                            // Create a default config from TOML source_type (authoritative)
                            // or fall back to hardware detection
                            let firmware = comp
                                .source_type
                                .as_ref()
                                .and_then(|st| st.to_firmware_type())
                                .unwrap_or_else(|| firmware_from_device_info(&data));
                            let num_ch = data
                                .get("num_channels")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(32) as u8;
                            let model_name = data
                                .get("model")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Unknown");

                            let mut c = DigitizerConfig::new(
                                source_id,
                                format!("{} ({})", model_name, comp.name),
                                firmware,
                            );
                            c.serial_number = serial.clone();
                            c.model = Some(model_name.to_string());
                            c.num_channels = num_ch;
                            c.board = default_board_config(firmware);
                            c.channel_defaults = default_channel_config(firmware);
                            config = Some(c);
                        }
                    }

                    // Update in-memory config with detected hardware info
                    if let Some(ref cfg) = config {
                        state.digitizer_configs.insert(source_id, cfg.clone());
                    }

                    // Auto-save corrected config to disk (best-effort)
                    // This persists hardware-detected num_channels, serial, model
                    if let Some(ref cfg) = config {
                        let file_path = comp.config_file.clone().unwrap_or_else(|| {
                            state
                                .config_dir
                                .join(format!("digitizer_{}.json", source_id))
                        });
                        if let Some(parent) = file_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if let Ok(json) = serde_json::to_string_pretty(cfg) {
                            if let Err(e) = std::fs::write(&file_path, &json) {
                                tracing::warn!("Failed to auto-save config after Detect: {}", e);
                            } else {
                                tracing::info!(
                                    source_id,
                                    path = %file_path.display(),
                                    num_channels = cfg.num_channels,
                                    "Auto-saved config after Detect"
                                );
                            }
                        }
                    }

                    detected.push(DetectedDigitizer {
                        component_name: comp.name.clone(),
                        source_id,
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

/// Map firmware_type + model from device_info to FirmwareType enum.
///
/// Thin shim around `FirmwareType::from_caen_device` (`src/config/digitizer.rs`)
/// that pulls the two strings out of a `serde_json::Value` payload (the
/// shape `detect_digitizers` already produces). Falls back to `PSD2` when
/// the helper returns `None` so the existing detect-path semantics
/// (display *something* for unknown hardware) are preserved.
///
/// Read-loop callers (`read_loop_dig1` / `read_loop_dig2`) use the helper
/// directly and **must not** fall back — see `check_firmware_match` in
/// `src/reader/mod.rs`.
fn firmware_from_device_info(device_info: &serde_json::Value) -> FirmwareType {
    let fw = device_info
        .get("firmware_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let model = device_info
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    FirmwareType::from_caen_device(fw, model).unwrap_or(FirmwareType::PSD2)
}

/// Create default board config for a firmware type
fn default_board_config(firmware: FirmwareType) -> BoardConfig {
    match firmware {
        FirmwareType::PSD2 => BoardConfig {
            start_source: Some("SWcmd".to_string()),
            ..BoardConfig::default()
        },
        FirmwareType::PSD1 => BoardConfig {
            record_length: Some(1024),
            ..BoardConfig::default()
        },
        FirmwareType::PHA1 => BoardConfig {
            start_source: Some("SWcmd".to_string()),
            ..BoardConfig::default()
        },
        FirmwareType::AMax => BoardConfig {
            start_source: Some("SWcmd".to_string()),
            ..BoardConfig::default()
        },
        FirmwareType::PHA2 => BoardConfig {
            start_source: Some("SWcmd".to_string()),
            ..BoardConfig::default()
        },
        FirmwareType::X743CI | FirmwareType::X743Std => BoardConfig::default(),
    }
}

/// Create default channel config for a firmware type
fn default_channel_config(firmware: FirmwareType) -> ChannelConfig {
    match firmware {
        FirmwareType::PSD2 => ChannelConfig {
            enabled: Some("False".to_string()),
            dc_offset: Some(50.0),
            polarity: Some("Negative".to_string()),
            trigger_threshold: Some(1000),
            gate_long_ns: Some(400),
            gate_short_ns: Some(100),
            event_trigger_source: Some("Disabled".to_string()),
            wave_trigger_source: Some("Disabled".to_string()),
            ..ChannelConfig::default()
        },
        FirmwareType::PSD1 => ChannelConfig {
            enabled: Some("false".to_string()),
            dc_offset: Some(50.0),
            polarity: Some("Negative".to_string()),
            trigger_threshold: Some(500),
            gate_long_ns: Some(200),
            gate_short_ns: Some(50),
            gate_pre_ns: Some(30),
            ..ChannelConfig::default()
        },
        FirmwareType::PHA1 => ChannelConfig {
            enabled: Some("false".to_string()),
            dc_offset: Some(50.0),
            polarity: Some("Negative".to_string()),
            trigger_threshold: Some(500),
            ..ChannelConfig::default()
        },
        FirmwareType::AMax => ChannelConfig {
            enabled: Some("False".to_string()),
            dc_offset: Some(50.0),
            ..ChannelConfig::default()
        },
        FirmwareType::PHA2 => ChannelConfig {
            enabled: Some("False".to_string()),
            dc_offset: Some(50.0),
            polarity: Some("Negative".to_string()),
            trigger_threshold: Some(100),
            // Trapezoidal-filter DevTree defaults (see PHA2 DevTree)
            energy_filter_rise_time_ns: Some(5000),
            energy_filter_flat_top_ns: Some(1000),
            energy_filter_pole_zero_ns: Some(50000),
            energy_filter_peaking_position: Some(50),
            energy_filter_peaking_avg: Some("LowAVG".to_string()),
            energy_filter_baseline_avg: Some("Medium".to_string()),
            time_filter_rise_time_ns: Some(296),
            event_trigger_source: Some("Disabled".to_string()),
            wave_trigger_source: Some("Disabled".to_string()),
            ..ChannelConfig::default()
        },
        FirmwareType::X743CI | FirmwareType::X743Std => ChannelConfig {
            dc_offset: Some(50.0),
            ..ChannelConfig::default()
        },
    }
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
    let repo = require_digitizer_repo(&state)?;

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
    require_digitizer_config(&state, id).map(Json)
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
    if let Err(resp) = reject_path_id_mismatch(id, &config) {
        return resp;
    }

    state.digitizer_configs.insert(id, config);

    (
        StatusCode::OK,
        Json(ApiResponse::success(format!(
            "Digitizer {} configuration updated (not yet saved to disk)",
            id
        ))),
    )
}

/// Apply digitizer configuration to hardware
///
/// Updates the in-memory config, saves to disk (best-effort), and sends
/// the configuration to the Reader via ZMQ for hardware application.
/// Only available when the system is in Idle or Configured state.
#[utoipa::path(
    post,
    path = "/api/digitizers/{id}/apply",
    tag = "Digitizer Config",
    params(
        ("id" = u32, Path, description = "Digitizer ID")
    ),
    request_body = DigitizerConfig,
    responses(
        (status = 200, description = "Configuration applied to hardware", body = ApiResponse),
        (status = 400, description = "Invalid configuration", body = ApiResponse),
        (status = 404, description = "No Reader found for digitizer", body = ApiResponse),
        (status = 500, description = "Failed to apply", body = ApiResponse)
    )
)]
pub(super) async fn apply_digitizer_config(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
    Json(config): Json<DigitizerConfig>,
) -> (StatusCode, Json<ApiResponse>) {
    if let Err(resp) = reject_path_id_mismatch(id, &config) {
        return resp;
    }

    // 1. Find the Reader component for this digitizer (is_digitizer && source_id matches)
    let reader_comp = match state
        .components
        .iter()
        .find(|c| c.is_digitizer && c.source_id == Some(id))
    {
        Some(c) => c,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::error(format!(
                    "No Reader component found for digitizer {}",
                    id
                ))),
            );
        }
    };

    // 2. Update in-memory config
    // Keep what the AMax clamp note needs; `config` itself is moved into the
    // command below.
    let (note_firmware, note_num_channels) = (config.firmware, config.num_channels);
    state.digitizer_configs.insert(id, config.clone());

    // 3. Save to disk (best-effort, sanitized).
    // Same file Reader loads on Configure (resolve_config_path falls back
    // to <config_dir>/digitizer_<id>.json when no TOML config_file).
    let file_path = resolve_config_path(&state, id);
    match write_digitizer_config(&file_path, config.clone()) {
        Ok(()) => tracing::info!("Config saved to {}", file_path.display()),
        Err(e) => tracing::warn!("Failed to save config to disk: {}", e),
    }

    // 4. Send ApplyDigitizerConfig command via ZMQ
    match state
        .client
        .send_command(
            &reader_comp.address,
            &Command::ApplyDigitizerConfig(Box::new(config)),
        )
        .await
    {
        Ok(resp) if resp.success => {
            let params_applied = resp
                .data
                .as_ref()
                .and_then(|d| d.get("params_applied"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            // The Reader clamps an AMax sweep that is wider than the firmware's
            // channel-page map. That protects the hardware, but on its own it
            // only shows up as a smaller `params_applied` — say so here instead
            // of leaving the operator to diff counts.
            let mut message = format!("Applied {} parameters to hardware", params_applied);
            if let Some(note) = crate::reader::caen::amax_registers::channel_clamp_note(
                note_firmware,
                note_num_channels,
            ) {
                message.push_str(" — ");
                message.push_str(&note);
            }
            (StatusCode::OK, Json(ApiResponse::success(message)))
        }
        Ok(resp) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Reader rejected config: {}",
                resp.message
            ))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to send command to Reader: {}",
                e
            ))),
        ),
    }
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
    let mut config = match require_digitizer_config(&state, id) {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    // Ensure digitizer_id matches the TOML source_id (the HashMap key);
    // sanitize_for_firmware() runs inside write_digitizer_config().
    config.digitizer_id = id;

    let file_path = resolve_config_path(&state, id);
    if let Err(e) = write_digitizer_config(&file_path, config) {
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

    let mut saved = 0;
    let mut errors = Vec::new();

    for entry in state.digitizer_configs.iter() {
        let id = *entry.key();
        let file_path = resolve_config_path(&state, id);
        let mut config_for_disk = entry.value().clone();
        // Enforce ID consistency; sanitize_for_firmware() runs inside helper.
        config_for_disk.digitizer_id = id;
        if let Err(e) = write_digitizer_config(&file_path, config_for_disk) {
            errors.push(format!("digitizer_{}: {}", id, e));
        } else {
            saved += 1;
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
    let repo = match require_digitizer_repo(&state) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let config = match require_digitizer_config(&state, id) {
        Ok(c) => c,
        Err(resp) => return resp,
    };

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
    let repo = require_digitizer_repo(&state)?;

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
    let repo = match require_digitizer_repo(&state) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    match repo.restore_version(id, request.version).await {
        Ok(doc) => {
            // Also update in-memory config
            state.digitizer_configs.insert(id, doc.config);

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

/// AMax board-level register live readback — returns `(name → value)`
/// pairs as a JSON object. Used by the operator UI's Tune Up debug
/// view to surface what ENABLE_ACQ (and any future board-level
/// register added via `fw_params.json` `board_params`) is actually
/// set to on the digitizer vs what's stored in the config file.
///
/// Non-AMax / DIG1 / X743 firmwares return an empty object instead of
/// erroring, so the UI can render "no AMax registers" cleanly for
/// mixed-FW setups.
#[utoipa::path(
    get,
    path = "/api/digitizers/{id}/amax-board-registers",
    tag = "Digitizer Config",
    params(
        ("id" = u32, Path, description = "Digitizer ID")
    ),
    responses(
        (status = 200, description = "Live register values", body = serde_json::Value),
        (status = 404, description = "Digitizer not found", body = ApiResponse),
        (status = 503, description = "Reader did not respond", body = ApiResponse),
    )
)]
pub(super) async fn read_amax_board_registers(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiResponse>)> {
    let comp = state
        .components
        .iter()
        .find(|c| c.is_digitizer && c.source_id == Some(id))
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::error(format!(
                    "No digitizer component for source_id={}",
                    id
                ))),
            )
        })?;

    match state
        .client
        .send_command(&comp.address, &Command::ReadAmaxBoardRegisters)
        .await
    {
        Ok(resp) if resp.success => Ok(Json(
            resp.data
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
        )),
        Ok(resp) => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::error(format!(
                "Reader rejected ReadAmaxBoardRegisters: {}",
                resp.message
            ))),
        )),
        Err(e) => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::error(format!(
                "ReadAmaxBoardRegisters request failed: {}",
                e
            ))),
        )),
    }
}
