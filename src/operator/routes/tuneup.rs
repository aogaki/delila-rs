//! Tune Up mode handlers
//!
//! Tune Up allows adjusting digitizer parameters while viewing waveforms,
//! without recording data. Only one digitizer at a time.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::common::{Command, ComponentState, RunConfig};
use crate::config::{DigitizerConfig, FirmwareType};

use super::super::{ApiResponse, SystemState};
use super::AppState;

/// Request body for starting Tune Up mode
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct TuneUpStartRequest {
    /// Digitizer ID to tune
    pub digitizer_id: u32,
}

/// Start Tune Up mode for a specific digitizer
///
/// Starts only the target digitizer's Reader + Merger + Monitor (no Recorder).
/// System must be in Idle state.
#[utoipa::path(
    post,
    path = "/api/tuneup/start",
    tag = "DAQ Control",
    request_body = TuneUpStartRequest,
    responses(
        (status = 200, description = "Tune Up started", body = ApiResponse),
        (status = 409, description = "System not in Idle or already in Tune Up", body = ApiResponse)
    )
)]
pub(super) async fn tuneup_start(
    State(state): State<Arc<AppState>>,
    Json(request): Json<TuneUpStartRequest>,
) -> (StatusCode, Json<ApiResponse>) {
    let digitizer_id = request.digitizer_id;

    // Guard: must not already be in tune up mode
    if state.tuneup.is_active() {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error(
                "Tune Up mode is already active. Stop it first.",
            )),
        );
    }

    // Guard: system must be Idle
    let components = state.client.get_all_status(&state.components).await;
    let system_state = SystemState::from_components(&components);
    if system_state != SystemState::Idle {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error(format!(
                "System must be Idle to start Tune Up (current: {:?})",
                system_state
            ))),
        );
    }

    // Filter components: target Reader + Merger + Monitor (exclude Recorder + other Readers)
    let filtered: Vec<_> = state
        .components
        .iter()
        .filter(|c| {
            if c.is_digitizer {
                // Only the target digitizer's Reader
                c.source_id == Some(digitizer_id)
            } else if c.source_id.is_some() {
                // Emulator sources - include them (they also have source_id)
                // Actually for tune up we only want the target digitizer, skip emulators
                false
            } else {
                // Non-source components: include Merger + Monitor, exclude Recorder
                !c.name.contains("Recorder")
            }
        })
        .cloned()
        .collect();

    if filtered.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error(format!(
                "No components found for digitizer {}",
                digitizer_id
            ))),
        );
    }

    // Verify we have the target Reader
    let has_reader = filtered
        .iter()
        .any(|c| c.is_digitizer && c.source_id == Some(digitizer_id));
    if !has_reader {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error(format!(
                "No Reader found for digitizer {}",
                digitizer_id
            ))),
        );
    }

    // Set tune up mode before starting (so status reflects it immediately).
    // Atomic: a single store publishes both the active flag and the id —
    // no torn read possible from concurrent /api/status pollers.
    state.tuneup.enter(digitizer_id);

    // TuneUp RunConfig: run_number 0, exp_name "TuneUp" (not recorded in MongoDB)
    let run_config = RunConfig {
        run_number: 0,
        comment: String::new(),
        exp_name: "TuneUp".to_string(),
    };

    // Configure → Arm → Start (filtered components only)
    let configure_result = state
        .client
        .configure_all_sync(&filtered, run_config, state.config.configure_timeout_ms)
        .await;

    if let Err(e) = configure_result {
        // Rollback tune up state
        state.tuneup.exit();
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Tune Up configure failed: {}",
                e
            ))),
        );
    }

    // Apply digitizer config to Reader (same as Phase 1.5 in run_start)
    //
    // X743Std: skip this block. `configure_all_sync` above already applied the
    // exact same config via the Reader's Configure path, and `force_software_trigger()`
    // is a no-op for X743Std (see `src/config/digitizer.rs`). A second
    // `apply_config_standard()` call within ~2s (Reset + full reconfigure + ADC cal)
    // destabilizes libCAENDigitizer.so and triggers SIGSEGV at `SWStartAcquisition`.
    // See TODO/48_v1743_tuneup_double_apply_crash.md for the crash analysis.
    {
        // Clone the config out of the DashMap before any .await — DashMap
        // shard guards are not Send, so they can't be held across awaits.
        let config = state
            .digitizer_configs
            .get(&digitizer_id)
            .map(|r| r.value().clone());
        if let Some(config) = config {
            let reader_comp = filtered
                .iter()
                .find(|c| c.is_digitizer && c.source_id == Some(digitizer_id));
            if let Some(comp) = reader_comp {
                if config.firmware == FirmwareType::X743Std {
                    tracing::info!(
                        digitizer_id,
                        name = %comp.name,
                        "X743Std: skipping redundant Tune Up Apply (configure_all_sync already applied identical config)"
                    );
                } else {
                    tracing::info!(
                        digitizer_id,
                        name = %comp.name,
                        "Pushing digitizer config to Reader (Tune Up)"
                    );
                    // Force software trigger for Tune Up (original config in state is unchanged)
                    let mut tuneup_config = config.clone();
                    tuneup_config.force_software_trigger();
                    tracing::info!(digitizer_id, "Forcing software trigger for Tune Up mode");

                    match state
                        .client
                        .send_command(
                            &comp.address,
                            &Command::ApplyDigitizerConfig(Box::new(tuneup_config)),
                        )
                        .await
                    {
                        Ok(resp) if resp.success => {
                            tracing::info!(
                                digitizer_id,
                                params = resp.message,
                                "Digitizer config applied (Tune Up)"
                            );
                        }
                        Ok(resp) => {
                            let _ = state.client.reset_all(&filtered).await;
                            state.tuneup.exit();
                            return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(ApiResponse::error(format!(
                                    "Tune Up config apply failed: {}",
                                    resp.message
                                ))),
                            );
                        }
                        Err(e) => {
                            let _ = state.client.reset_all(&filtered).await;
                            state.tuneup.exit();
                            return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(ApiResponse::error(format!(
                                    "Failed to send config to Reader: {}",
                                    e
                                ))),
                            );
                        }
                    }
                }
            }
        } else {
            tracing::warn!(
                digitizer_id,
                "No digitizer config in memory — Reader will use its JSON file defaults"
            );
        }
    }

    // Register channels for the tuned digitizer with Monitor (best-effort)
    {
        let registrations = AppState::build_channel_registrations_from(
            &state.digitizer_configs,
            Some(digitizer_id),
        );
        state.send_register_channels(registrations).await;
    }

    if let Err(e) = state
        .client
        .arm_all_sync(&filtered, state.config.arm_timeout_ms)
        .await
    {
        let _ = state.client.reset_all(&filtered).await;
        state.tuneup.exit();
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Tune Up arm failed: {}", e))),
        );
    }

    match state
        .client
        .start_all_sync(&filtered, 0, state.config.start_timeout_ms)
        .await
    {
        Ok(results) => {
            let response =
                ApiResponse::success(format!("Tune Up started for digitizer {}", digitizer_id))
                    .with_results(results);
            (StatusCode::OK, Json(response))
        }
        Err(e) => {
            let _ = state.client.reset_all(&filtered).await;
            state.tuneup.exit();
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::error(format!("Tune Up start failed: {}", e))),
            )
        }
    }
}

/// Stop Tune Up mode
///
/// Stops all components started during Tune Up and resets to Idle.
#[utoipa::path(
    post,
    path = "/api/tuneup/stop",
    tag = "DAQ Control",
    responses(
        (status = 200, description = "Tune Up stopped", body = ApiResponse),
        (status = 409, description = "Not in Tune Up mode", body = ApiResponse)
    )
)]
pub(super) async fn tuneup_stop(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<ApiResponse>) {
    if !state.tuneup.is_active() {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("Not in Tune Up mode")),
        );
    }

    // Stop all components (the ones that are actually running)
    let results = state.client.stop_all(&state.components).await;
    let _reset_results = state.client.reset_all(&state.components).await;

    // Clear tune up state
    state.tuneup.exit();

    let response = ApiResponse::success("Tune Up stopped").with_results(results);
    (StatusCode::OK, Json(response))
}

/// Apply digitizer configuration during Tune Up
///
/// Performs automatic Stop → Apply → Arm → Start cycle on the entire pipeline.
/// Stops Reader + Merger + Monitor to flush all buffered data, applies new config,
/// then restarts everything. Monitor's on_start() auto-clears histograms.
#[utoipa::path(
    post,
    path = "/api/tuneup/apply/{id}",
    tag = "DAQ Control",
    params(
        ("id" = u32, Path, description = "Digitizer ID")
    ),
    request_body = DigitizerConfig,
    responses(
        (status = 200, description = "Configuration applied", body = ApiResponse),
        (status = 409, description = "Not in Tune Up mode", body = ApiResponse),
        (status = 404, description = "Reader not found", body = ApiResponse),
        (status = 500, description = "Apply failed", body = ApiResponse)
    )
)]
pub(super) async fn tuneup_apply(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
    Json(config): Json<DigitizerConfig>,
) -> (StatusCode, Json<ApiResponse>) {
    // Guard: must be in tune up mode, and the path id must match the digitizer
    // the tuneup was started for. TODO 58 M14: without the id check, an apply
    // to a DIFFERENT id overwrote that idle digitizer's config/JSON on disk and
    // stopped the live tuneup's Merger/Monitor mid-stream.
    let (active, active_id) = state.tuneup.snapshot();
    if !active {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error("Not in Tune Up mode")),
        );
    }
    if active_id != Some(id) {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error(format!(
                "Tune Up is active for digitizer {}, not {} — apply refused",
                active_id.map_or_else(|| "?".to_string(), |v| v.to_string()),
                id
            ))),
        );
    }

    // Serialize Apply calls to prevent concurrent Stop→Apply→Start races
    let _apply_guard = state.tuneup_apply_lock.lock().await;

    // Build pipeline component lists: Reader + Merger + Monitor (same filter as tuneup_start)
    let pipeline_components: Vec<_> = state
        .components
        .iter()
        .filter(|c| {
            if c.is_digitizer {
                c.source_id == Some(id)
            } else if c.source_id.is_some() {
                false // Skip emulators
            } else {
                !c.name.contains("Recorder")
            }
        })
        .cloned()
        .collect();

    let reader_comp = match pipeline_components
        .iter()
        .find(|c| c.is_digitizer && c.source_id == Some(id))
    {
        Some(c) => c.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::error(format!(
                    "No Reader found for digitizer {}",
                    id
                ))),
            );
        }
    };

    // 1. Update in-memory config
    // Keep what the AMax clamp note needs before `config` is consumed below.
    let (note_firmware, note_num_channels) = (config.firmware, config.num_channels);
    state.digitizer_configs.insert(id, config.clone());

    // 2. Save to disk (best-effort, sanitized)
    let file_path = reader_comp
        .config_file
        .clone()
        .unwrap_or_else(|| state.config_dir.join(format!("digitizer_{}.json", id)));
    if let Some(parent) = file_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut config_for_disk = config.clone();
    config_for_disk.sanitize_for_firmware();
    if let Ok(json) = serde_json::to_string_pretty(&config_for_disk) {
        if let Err(e) = std::fs::write(&file_path, &json) {
            tracing::warn!("Failed to save config to disk: {}", e);
        }
    }

    // 3. Stop entire pipeline (Reader + Merger + Monitor) to flush all buffers
    let stop_results = state.client.stop_all(&pipeline_components).await;
    if stop_results.iter().any(|r| !r.success) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(
                "Failed to stop pipeline components for apply",
            )),
        );
    }

    // Wait for all pipeline components to reach Configured state
    if let Err(e) = state
        .client
        .wait_for_state(&pipeline_components, ComponentState::Configured, 5000)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Pipeline did not reach Configured state: {}",
                e
            ))),
        );
    }

    // 4. Apply config via ZMQ (Reader is in Configured state)
    // Force software trigger for Tune Up (user's original config already saved at step 1)
    let mut tuneup_config = config;
    tuneup_config.force_software_trigger();
    match state
        .client
        .send_command(
            &reader_comp.address,
            &Command::ApplyDigitizerConfig(Box::new(tuneup_config)),
        )
        .await
    {
        Ok(resp) if !resp.success => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::error(format!(
                    "Reader rejected config: {}",
                    resp.message
                ))),
            );
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::error(format!(
                    "Failed to send config to Reader: {}",
                    e
                ))),
            );
        }
        _ => {}
    }

    // 5. Arm entire pipeline
    if let Err(e) = state
        .client
        .arm_all_sync(&pipeline_components, state.config.arm_timeout_ms)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to arm pipeline after apply: {}",
                e
            ))),
        );
    }

    // 6. Start entire pipeline (downstream first: Monitor → Merger → Reader)
    match state
        .client
        .start_all_sync(&pipeline_components, 0, state.config.start_timeout_ms)
        .await
    {
        Ok(_) => {
            // Give ReadLoop time to complete hardware arm+start.
            // ReadLoop processes state changes asynchronously (loop interval ~100ms).
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            // Same AMax clamp note as the plain Apply route — Tune Up is the
            // path an operator actually watches while changing settings.
            let mut message = format!(
                "Configuration applied to digitizer {} (full pipeline Stop→Apply→Start)",
                id
            );
            if let Some(note) = crate::reader::caen::amax_registers::channel_clamp_note(
                note_firmware,
                note_num_channels,
            ) {
                message.push_str(" — ");
                message.push_str(&note);
            }
            (StatusCode::OK, Json(ApiResponse::success(message)))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to restart pipeline after apply: {}",
                e
            ))),
        ),
    }
}
