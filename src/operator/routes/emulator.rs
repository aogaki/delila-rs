//! Emulator settings handlers

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    Json,
};

use crate::common::{Command, EmulatorRuntimeConfig};

use super::super::ApiResponse;
use super::{AppState, EmulatorSettings};

/// Get current emulator settings
#[utoipa::path(
    get,
    path = "/api/emulator",
    tag = "Emulator Settings",
    responses(
        (status = 200, description = "Current emulator settings", body = EmulatorSettings)
    )
)]
pub(super) async fn get_emulator_settings(
    State(state): State<Arc<AppState>>,
) -> Json<EmulatorSettings> {
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
pub(super) async fn update_emulator_settings(
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
