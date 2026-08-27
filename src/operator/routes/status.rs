//! DAQ control handlers (status, configure, arm, start, stop, reset, run_start)

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, Json};

use crate::common::{Command, RunConfig};
use crate::config::FirmwareType;

use super::super::{
    ApiResponse, ConfigureRequest, CurrentRunInfo, RunStats, RunStatus, StartRequest, SystemState,
    SystemStatus,
};
use super::AppState;

/// Get system and component status
#[utoipa::path(
    get,
    path = "/api/status",
    tag = "DAQ Control",
    responses(
        (status = 200, description = "System status", body = SystemStatus)
    )
)]
pub(super) async fn get_status(State(state): State<Arc<AppState>>) -> Json<SystemStatus> {
    let components = state.client.get_all_status(&state.components).await;
    let system_state = SystemState::from_components(&components);

    // Get current run info and update real-time values
    let run_info = {
        let cached = state.current_run.read().await.clone();
        if let Some(mut info) = cached {
            if info.status == RunStatus::Running {
                // Update elapsed time
                let now_ms = chrono::Utc::now().timestamp_millis();
                info.elapsed_secs = (now_ms - info.start_time) / 1000;

                // Update stats from Recorder metrics (authoritative source for recorded data)
                let recorder_metrics = components
                    .iter()
                    .find(|c| c.name == "Recorder")
                    .and_then(|c| c.metrics.as_ref());
                let (total_events, total_bytes) = recorder_metrics
                    .map(|m| (m.events_processed as i64, m.bytes_transferred as i64))
                    .unwrap_or((0, 0));
                let trigger_loss_count: i64 = components
                    .iter()
                    .filter(|c| c.role == "source")
                    .filter_map(|c| c.metrics.as_ref())
                    .map(|m| m.trigger_loss_count as i64)
                    .sum();
                let average_rate = if info.elapsed_secs > 0 {
                    total_events as f64 / info.elapsed_secs as f64
                } else {
                    0.0
                };

                info.stats = RunStats {
                    total_events,
                    total_bytes,
                    average_rate,
                    trigger_loss_count,
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

    // Single atomic load — both fields agree by construction (no torn read).
    let (tuneup_mode, tuneup_digitizer_id) = state.tuneup.snapshot();

    Json(SystemStatus {
        components,
        system_state,
        run_info,
        experiment_name: state.config.experiment_name.clone(),
        next_run_number,
        last_run_info,
        tuneup_mode,
        tuneup_digitizer_id,
        monitor_http_port: state.config.monitor_http_port,
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
pub(super) async fn configure(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ConfigureRequest>,
) -> (StatusCode, Json<ApiResponse>) {
    // Serialize run control — all guards below are check-then-act (TODO 58 H14).
    let _run_guard = state.run_control_lock.lock().await;
    // Guard: reject if Tune Up mode is active (TODO 58 M15 — configuring under
    // an active tuneup re-resets the digitizer mid-stream and strands the
    // tuneup flag).
    if state.tuneup.is_active() {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error(
                "Cannot configure while Tune Up mode is active. Stop Tune Up first.",
            )),
        );
    }
    let run_config: RunConfig = request.into();
    let run_number = run_config.run_number;

    // Reset to Idle first (mirrors run_start Phase 0). Without this, a Configure
    // issued while already in the Configured state is rejected by the state machine
    // (Configured -> Configured is not a valid transition), which makes the aggregate
    // response.success false and silently skips the disk reload + ApplyDigitizerConfig
    // push below. Command::Reset is idempotent from any state.
    match state
        .client
        .reset_all_sync(&state.components, state.config.reset_timeout_ms)
        .await
    {
        Err(e) => {
            return (
                StatusCode::REQUEST_TIMEOUT,
                Json(ApiResponse::error(format!("Reset phase failed: {}", e))),
            );
        }
        Ok(results) if results.iter().any(|r| !r.success) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error("Reset phase failed").with_results(results)),
            );
        }
        Ok(_) => {}
    }

    let results = state
        .client
        .configure_all(&state.components, run_config)
        .await;

    let mut response =
        ApiResponse::success(format!("Configure command sent for run {}", run_number))
            .with_results(results);

    let status = if response.success {
        // Reload configs from disk so edits since Operator start take effect
        state.reload_digitizer_configs();
        // Push digitizer configs to remote Readers. TODO 58 M13: a Reader/FW
        // rejection here used to be warn-only + HTTP 200 — completely silent
        // in the UI while the digitizer ran with the wrong settings.
        let mut apply_failures: Vec<String> = Vec::new();
        // Non-fatal: the Reader clamps an AMax sweep wider than the firmware's
        // channel-page map. Configure is the automatic path, so without this the
        // skipped channels would only ever appear in the Reader's own log.
        let mut amax_notes: Vec<String> = Vec::new();
        for comp in &state.components {
            if comp.is_digitizer {
                if let Some(source_id) = comp.source_id {
                    let Some(config) = state.digitizer_configs.get(&source_id) else {
                        // TODO 58 M13: this skip was silent.
                        tracing::warn!(
                            source_id,
                            name = %comp.name,
                            "No digitizer config loaded for this source — Apply skipped"
                        );
                        continue;
                    };
                    let cfg = config.value().clone();
                    drop(config);
                    // X743Std: skip the redundant push — `configure_all` above already
                    // applied this config via the Reader's Configure path (the Reader
                    // keeps its `dig_config` in sync, so Configure re-applies the current
                    // config). A second apply within ~2s destabilizes libCAENDigitizer
                    // and can SIGSEGV after Start. Same guard as run_start Phase 1.5
                    // (see TODO/48). Use POST /api/digitizers/{id}/apply to push a
                    // direct on-disk edit to an X743 reader without a full reconfigure.
                    if cfg.firmware == FirmwareType::X743Std {
                        tracing::info!(
                            source_id,
                            name = %comp.name,
                            "X743Std: skipping redundant Apply (configure_all already applied identical config)"
                        );
                        continue;
                    }
                    if let Some(note) = crate::reader::caen::amax_registers::channel_clamp_note(
                        cfg.firmware,
                        cfg.num_channels,
                    ) {
                        tracing::warn!(source_id, name = %comp.name, note = %note, "AMax clamp");
                        amax_notes.push(format!("{}: {}", comp.name, note));
                    }
                    tracing::info!(
                        source_id,
                        name = %comp.name,
                        "Pushing digitizer config to Reader"
                    );
                    match state
                        .client
                        .send_command(&comp.address, &Command::ApplyDigitizerConfig(Box::new(cfg)))
                        .await
                    {
                        Ok(resp) if resp.success => {}
                        Ok(resp) => {
                            tracing::error!(
                                source_id,
                                error = %resp.message,
                                "Reader rejected digitizer config"
                            );
                            apply_failures.push(format!("{}: {}", comp.name, resp.message));
                        }
                        Err(e) => {
                            tracing::error!(
                                source_id,
                                error = %e,
                                "Failed to send ApplyDigitizerConfig command"
                            );
                            apply_failures.push(format!("{}: {}", comp.name, e));
                        }
                    }
                }
            }
        }
        if apply_failures.is_empty() {
            if !amax_notes.is_empty() {
                response.message = format!("{} — {}", response.message, amax_notes.join("; "));
            }
            StatusCode::OK
        } else {
            response.success = false;
            response.message = format!(
                "Configure succeeded but digitizer config apply FAILED: {}",
                apply_failures.join("; ")
            );
            StatusCode::BAD_GATEWAY
        }
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
pub(super) async fn arm(State(state): State<Arc<AppState>>) -> (StatusCode, Json<ApiResponse>) {
    // Serialize run control (TODO 58 H14).
    let _run_guard = state.run_control_lock.lock().await;
    // Guard: reject if Tune Up mode is active (TODO 58 M15).
    if state.tuneup.is_active() {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error(
                "Cannot arm while Tune Up mode is active. Stop Tune Up first.",
            )),
        );
    }
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
pub(super) async fn start(
    State(state): State<Arc<AppState>>,
    Json(request): Json<StartRequest>,
) -> (StatusCode, Json<ApiResponse>) {
    // Serialize run control (TODO 58 H14).
    let _run_guard = state.run_control_lock.lock().await;
    // Guard: reject if Tune Up mode is active
    if state.tuneup.is_active() {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error(
                "Cannot start a run while Tune Up mode is active. Stop Tune Up first.",
            )),
        );
    }

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

    // Start with the run number
    let start_result = state
        .client
        .start_all_sync(&state.components, run_number, state.config.start_timeout_ms)
        .await;

    let mut response = match start_result {
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
        // TODO 58 M16: MongoDB persistence failures were warn-only + HTTP 200 —
        // run-history gaps were invisible to the operator. The run itself still
        // starts (DAQ > bookkeeping), but the response must say so.
        let mut mongo_warnings: Vec<String> = Vec::new();

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
                    tracing::error!("Failed to record run start in MongoDB: {}", e);
                    mongo_warnings.push(format!("run start not persisted: {}", e));
                    // Still set current_run for in-memory tracking
                    *state.current_run.write().await = Some(CurrentRunInfo {
                        run_number: run_number as i32,
                        exp_name: exp_name.clone(),
                        comment: comment.clone(),
                        start_time: chrono::Utc::now().timestamp_millis(),
                        elapsed_secs: 0,
                        status: RunStatus::Running,
                        stats: RunStats::default(),
                        notes: Vec::new(),
                    });
                }
            }
        } else {
            // TODO 58 M17: without MongoDB the in-memory current_run tracking
            // used to hang off the digitizer_repo branch below — with run
            // history disabled but a digitizer repo present, current_run was
            // never set and the UI showed no active run. Mirror run_start.
            *state.current_run.write().await = Some(CurrentRunInfo {
                run_number: run_number as i32,
                exp_name: exp_name.clone(),
                comment: comment.clone(),
                start_time: chrono::Utc::now().timestamp_millis(),
                elapsed_secs: 0,
                status: RunStatus::Running,
                stats: RunStats::default(),
                notes: Vec::new(),
            });
        }

        // Create digitizer config snapshot for this run
        if let Some(ref digitizer_repo) = state.digitizer_repo {
            let configs: Vec<_> = state
                .digitizer_configs
                .iter()
                .map(|r| r.value().clone())
                .collect();
            if !configs.is_empty() {
                if let Err(e) = digitizer_repo
                    .create_run_snapshot(run_number as i32, exp_name, configs)
                    .await
                {
                    tracing::error!("Failed to create config snapshot: {}", e);
                    mongo_warnings.push(format!("config snapshot not persisted: {}", e));
                }
            } else {
                tracing::warn!(
                    run_number,
                    "digitizer_configs is empty — config snapshot skipped. \
                     Check that config_file paths in TOML are correct and \
                     operator is started from the project root."
                );
            }
        }
        // TODO 58 M16: surface persistence failures in the HTTP response.
        if !mongo_warnings.is_empty() {
            response.message = format!(
                "{} — WARNING, run history incomplete: {}",
                response.message,
                mongo_warnings.join("; ")
            );
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
pub(super) async fn stop(State(state): State<Arc<AppState>>) -> (StatusCode, Json<ApiResponse>) {
    // Serialize run control (TODO 58 H14).
    let _run_guard = state.run_control_lock.lock().await;
    // Guard: reject if Tune Up mode is active (TODO 58 M15 — /api/stop during a
    // tuneup used to strand the tuneup flag, deadlocking later starts on 409).
    if state.tuneup.is_active() {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error(
                "Tune Up mode is active — use the Tune Up stop endpoint instead of /api/stop.",
            )),
        );
    }
    let results = state.client.stop_all(&state.components).await;

    let mut response = ApiResponse::success("Stop command sent").with_results(results);
    finish_run_bookkeeping(&state, &mut response, None).await;

    let status = if response.success {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };

    (status, Json(response))
}

/// Run-end bookkeeping shared by `/api/stop` and the drain-first stop
/// (TODO 68): Mongo run record, ELOG post, clearing `current_run`. Always
/// runs to completion even after partial component failures — the UI must
/// not think the run is still active.
async fn finish_run_bookkeeping(
    state: &Arc<AppState>,
    response: &mut ApiResponse,
    stop_reason: Option<&str>,
) {
    let current_run = state.current_run.read().await.clone();

    if let (Some(ref repo), Some(mut run_info)) = (&state.run_repo, current_run) {
        // Calculate final elapsed time
        let now_ms = chrono::Utc::now().timestamp_millis();
        run_info.elapsed_secs = (now_ms - run_info.start_time) / 1000;

        // Get final stats from components. Recorder is the authoritative source
        // for recorded events/bytes (TODO 58 H12) — summing every component
        // counts the same events ~4x (Reader+Merger+Recorder+Monitor) and that
        // inflated number was persisted to MongoDB/ELOG. Mirrors get_status.
        let components = state.client.get_all_status(&state.components).await;
        let recorder_metrics = components
            .iter()
            .find(|c| c.name == "Recorder")
            .and_then(|c| c.metrics.as_ref());
        let (total_events, total_bytes) = recorder_metrics
            .map(|m| (m.events_processed as i64, m.bytes_transferred as i64))
            .unwrap_or((0, 0));
        let trigger_loss_count: i64 = components
            .iter()
            .filter(|c| c.role == "source")
            .filter_map(|c| c.metrics.as_ref())
            .map(|m| m.trigger_loss_count as i64)
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
            trigger_loss_count,
        };

        // TODO 58 L9: a partial stop failure used to be recorded as Completed —
        // record Error so the run history reflects the abnormal end.
        let final_status = if response.success {
            RunStatus::Completed
        } else {
            RunStatus::Error
        };

        if let Err(e) = repo
            .end_run(
                run_info.run_number,
                &run_info.exp_name,
                final_status,
                stats.clone(),
                stop_reason.map(str::to_owned),
            )
            .await
        {
            // TODO 58 M16: persistence failure must reach the operator, not
            // just the log.
            tracing::error!("Failed to record run end in MongoDB: {}", e);
            response.message = format!(
                "{} — WARNING, run history incomplete: run end not persisted: {}",
                response.message, e
            );
        }

        // Post to ELOG (fire-and-forget, must not block stop)
        if let Some(ref elog_config) = state.config.elog {
            let elog_config = elog_config.clone();
            let run_number = run_info.run_number;
            let exp_name = run_info.exp_name.clone();
            let comment = match stop_reason {
                Some(reason) => format!("{} [stop: {}]", run_info.comment, reason),
                None => run_info.comment.clone(),
            };
            let elapsed = run_info.elapsed_secs;
            let stats = stats.clone();
            tokio::spawn(async move {
                crate::operator::elog::post_run_summary(
                    &elog_config,
                    run_number,
                    &exp_name,
                    &comment,
                    elapsed,
                    &stats,
                )
                .await;
            });
        }
    }

    // Always clear current run
    *state.current_run.write().await = None;
}

/// Drain-first stop (TODO 68): stop the sources, let the downstream
/// components drain their backlog to disk while still Running, then stop
/// them. A plain `/api/stop` would move Merger/Recorder out of Running with
/// the backlog still queued — and both discard non-Running data as an
/// accepted Stop tail, which is exactly the data this path exists to save.
///
/// Caller must hold `state.run_control_lock`.
pub(crate) async fn drain_first_stop(
    state: &Arc<AppState>,
    reason: &str,
    timeout_ms: u64,
) -> (StatusCode, Json<ApiResponse>) {
    // Same Tune Up guard as /api/stop (TODO 58 M15).
    if state.tuneup.is_active() {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error(
                "Tune Up mode is active — use the Tune Up stop endpoint instead.",
            )),
        );
    }

    tracing::warn!(reason, timeout_ms, "Drain-first stop initiated");

    let (sources, sinks): (Vec<_>, Vec<_>) = state
        .components
        .iter()
        .cloned()
        .partition(|c| c.source_id.is_some());

    // 1. Close the tap: stop the readers (HW disarm), wait until they are
    //    actually out of Running — a Reader acks Stop before its pipeline
    //    flushes the final EOS, so this wait must precede the drain wait.
    let mut results = state.client.stop_all(&sources).await;
    let sources_stopped = results.iter().all(|r| r.success);
    if let Err(e) = state
        .client
        .wait_for_state(&sources, crate::common::ComponentState::Configured, 10_000)
        .await
    {
        tracing::error!(error = %e, "Sources did not reach Configured after stop — draining anyway");
    }

    // 2. Drain: Merger/Recorder stay Running, so the backlog flows to disk
    //    through the normal path and the readers' EOS closes the file.
    let drain_result = state
        .client
        .wait_for_backlog_drained(&sinks, timeout_ms)
        .await;

    // 3. Stop the downstream (ascending pipeline order preserved).
    let sink_results = state.client.stop_all(&sinks).await;
    let sinks_stopped = sink_results.iter().all(|r| r.success);
    results.extend(sink_results);

    let mut response = match &drain_result {
        Ok(()) => ApiResponse::success(format!(
            "Drain-first stop complete ({reason}) — backlog fully written"
        )),
        Err(residual) => {
            tracing::error!(residual = %residual.summary(), "Drain-first stop lost residual backlog");
            ApiResponse::error(format!(
                "Drain-first stop ({reason}) — DRAIN INCOMPLETE: {}",
                residual.summary()
            ))
        }
    }
    .with_results(results);
    // with_results() recomputes `success` from the component stop results
    // alone — an incomplete drain must stay a failure even when every Stop
    // command itself succeeded.
    if drain_result.is_err() || !(sources_stopped && sinks_stopped) {
        response.success = false;
    }

    // 4. Bookkeeping with the reason (and residual, if any) — a timed-out
    //    drain records the run as Error via response.success = false.
    let stop_reason = match &drain_result {
        Ok(()) => reason.to_string(),
        Err(residual) => format!("{reason}; {}", residual.summary()),
    };
    finish_run_bookkeeping(state, &mut response, Some(&stop_reason)).await;

    let status = if response.success {
        StatusCode::OK
    } else {
        StatusCode::BAD_GATEWAY
    };
    (status, Json(response))
}

/// Stop the run by draining the pipeline first (TODO 68)
#[utoipa::path(
    post,
    path = "/api/stop_drain",
    tag = "DAQ Control",
    responses(
        (status = 200, description = "Drain-first stop complete, backlog fully written", body = ApiResponse),
        (status = 409, description = "Tune Up active", body = ApiResponse),
        (status = 502, description = "Drain incomplete or component stop failed", body = ApiResponse)
    )
)]
pub(super) async fn stop_drain(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<ApiResponse>) {
    // Serialize run control (TODO 58 H14).
    let _run_guard = state.run_control_lock.lock().await;
    let timeout_ms = state.config.drain_stop_timeout_secs * 1000;
    drain_first_stop(&state, "operator drain-stop request", timeout_ms).await
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
pub(super) async fn reset(State(state): State<Arc<AppState>>) -> (StatusCode, Json<ApiResponse>) {
    // Serialize run control (TODO 58 H14). Reset itself stays unguarded by
    // state on purpose — it is the operator's escape hatch (UI "Force Reset").
    let _run_guard = state.run_control_lock.lock().await;

    // If a run is active, close its record as Aborted BEFORE resetting —
    // otherwise the MongoDB doc stays "running" forever and current_run
    // leaks into the next session (TODO 58 H13).
    let aborted_run = state.current_run.write().await.take();
    if let (Some(ref repo), Some(mut run_info)) = (&state.run_repo, aborted_run) {
        let now_ms = chrono::Utc::now().timestamp_millis();
        run_info.elapsed_secs = (now_ms - run_info.start_time) / 1000;
        if let Err(e) = repo
            .end_run(
                run_info.run_number,
                &run_info.exp_name,
                RunStatus::Aborted,
                run_info.stats.clone(),
                None,
            )
            .await
        {
            tracing::warn!(
                run_number = run_info.run_number,
                "Failed to record aborted run in MongoDB: {}",
                e
            );
        } else {
            tracing::info!(
                run_number = run_info.run_number,
                "Run aborted by reset — recorded as Aborted"
            );
        }
    }

    let results = state.client.reset_all(&state.components).await;

    let response = ApiResponse::success("Reset command sent").with_results(results);

    let status = if response.success {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };

    (status, Json(response))
}

/// Start a run with full auto-cycle synchronization
///
/// This endpoint performs the complete run startup sequence:
/// 0. Reset all components (ensures clean state)
/// 1. Configure all components (with sync)
/// 2. Arm all components (with sync - waits for all to be Armed)
/// 3. Start all components (with sync)
///
/// Each phase waits for all components to reach the expected state
/// before proceeding, with configurable timeouts.
/// Config is always freshly applied, guaranteeing parameter consistency.
#[utoipa::path(
    post,
    path = "/api/run/start",
    tag = "DAQ Control",
    request_body = ConfigureRequest,
    responses(
        (status = 200, description = "Run started successfully", body = ApiResponse),
        (status = 400, description = "Failed to start run", body = ApiResponse),
        (status = 408, description = "Timeout during synchronization", body = ApiResponse),
        (status = 409, description = "System is Running or in Tune Up mode", body = ApiResponse)
    )
)]
pub(super) async fn run_start(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ConfigureRequest>,
) -> (StatusCode, Json<ApiResponse>) {
    // Serialize run control (TODO 58 H14).
    let _run_guard = state.run_control_lock.lock().await;
    // Guard: reject if Tune Up mode is active
    if state.tuneup.is_active() {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error(
                "Cannot start a run while Tune Up mode is active. Stop Tune Up first.",
            )),
        );
    }

    // Guard: reject if system is Running
    let components = state.client.get_all_status(&state.components).await;
    let system_state = SystemState::from_components(&components);
    if system_state == SystemState::Running {
        return (
            StatusCode::CONFLICT,
            Json(ApiResponse::error(
                "System is currently Running. Stop the run first before starting a new one.",
            )),
        );
    }

    let run_config: RunConfig = request.clone().into();
    let run_number = run_config.run_number;
    let comment = request.comment.clone();

    // Phase 0: Reset (ensure clean state — idempotent if already Idle)
    let reset_result = state
        .client
        .reset_all_sync(&state.components, state.config.reset_timeout_ms)
        .await;

    match reset_result {
        Err(e) => {
            return (
                StatusCode::REQUEST_TIMEOUT,
                Json(ApiResponse::error(format!("Reset phase failed: {}", e))),
            );
        }
        Ok(results) if results.iter().any(|r| !r.success) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error("Reset phase failed").with_results(results)),
            );
        }
        Ok(_) => {}
    }

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

    // Reload digitizer configs from disk so Phase 1.5 uses fresh values.
    // Without this, edits to JSON files (e.g. fine_ts_mode) after Operator start
    // would be overwritten by stale in-memory configs.
    state.reload_digitizer_configs();

    // Phase 1.5: Apply digitizer configs to remote Readers
    // This pushes configs over ZMQ so remote Readers don't need local config files
    //
    // X743Std: skipped — same reason as tuneup_start. `configure_all_sync` above
    // already invoked apply_config_standard() via the Reader's Configure path
    // with identical config. A second apply within ~2s (Reset + full reconfigure
    // + ADC cal) destabilizes libCAENDigitizer.so and triggers SIGSEGV after
    // SWStartAcquisition. See TODO/48_v1743_tuneup_double_apply_crash.md.
    {
        for comp in &state.components {
            if comp.is_digitizer {
                if let Some(source_id) = comp.source_id {
                    // digitizer_id == source_id by convention.
                    // Clone the config so we don't hold a DashMap shard guard
                    // across the .send_command().await below.
                    let config = match state
                        .digitizer_configs
                        .get(&source_id)
                        .map(|r| r.value().clone())
                    {
                        Some(c) => c,
                        None => {
                            // TODO 58 M13: this skip was silent.
                            tracing::warn!(
                                source_id,
                                name = %comp.name,
                                "No digitizer config loaded for this source — Phase 1.5 Apply skipped"
                            );
                            continue;
                        }
                    };
                    if config.firmware == FirmwareType::X743Std {
                        tracing::info!(
                            source_id,
                            name = %comp.name,
                            "X743Std: skipping redundant Phase 1.5 Apply (configure_all_sync already applied identical config)"
                        );
                        continue;
                    }
                    tracing::info!(
                        source_id,
                        name = %comp.name,
                        "Pushing digitizer config to Reader"
                    );
                    match state
                        .client
                        .send_command(
                            &comp.address,
                            &Command::ApplyDigitizerConfig(Box::new(config)),
                        )
                        .await
                    {
                        Ok(resp) if resp.success => {
                            tracing::info!(
                                source_id,
                                params = resp.message,
                                "Digitizer config applied successfully"
                            );
                        }
                        // TODO 58 M13: an FW rejection here was warn-only and
                        // the sequence continued into Arm/Start — the run then
                        // took data with the wrong (post-reset) settings.
                        // Abort before Arm instead.
                        Ok(resp) => {
                            tracing::error!(
                                source_id,
                                error = %resp.message,
                                "Reader rejected digitizer config — aborting run start"
                            );
                            return (
                                StatusCode::BAD_GATEWAY,
                                Json(ApiResponse::error(format!(
                                    "{} rejected its digitizer config: {} — run start aborted \
                                     before Arm. Fix the config and retry.",
                                    comp.name, resp.message
                                ))),
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                source_id,
                                error = %e,
                                "Failed to send ApplyDigitizerConfig command — aborting run start"
                            );
                            return (
                                StatusCode::BAD_GATEWAY,
                                Json(ApiResponse::error(format!(
                                    "Failed to push digitizer config to {}: {} — run start \
                                     aborted before Arm.",
                                    comp.name, e
                                ))),
                            );
                        }
                    }
                }
            }
        }
    }

    // Register channels with Monitor (best-effort, after configure success)
    {
        let registrations =
            AppState::build_channel_registrations_from(&state.digitizer_configs, None);
        state.send_register_channels(registrations).await;
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
            let exp_name = &state.config.experiment_name;
            // TODO 58 M16: persistence failures must reach the response.
            let mut mongo_warnings: Vec<String> = Vec::new();

            // Record run start in MongoDB and update current_run
            if let Some(ref repo) = state.run_repo {
                match repo
                    .start_run(run_number as i32, exp_name, &comment, None)
                    .await
                {
                    Ok(doc) => {
                        let info = CurrentRunInfo::from_document(&doc);
                        *state.current_run.write().await = Some(info);
                    }
                    Err(e) => {
                        tracing::error!("Failed to record run start in MongoDB: {}", e);
                        mongo_warnings.push(format!("run start not persisted: {}", e));
                        *state.current_run.write().await = Some(CurrentRunInfo {
                            run_number: run_number as i32,
                            exp_name: exp_name.clone(),
                            comment: comment.clone(),
                            start_time: chrono::Utc::now().timestamp_millis(),
                            elapsed_secs: 0,
                            status: RunStatus::Running,
                            stats: RunStats::default(),
                            notes: Vec::new(),
                        });
                    }
                }
            } else {
                *state.current_run.write().await = Some(CurrentRunInfo {
                    run_number: run_number as i32,
                    exp_name: exp_name.clone(),
                    comment,
                    start_time: chrono::Utc::now().timestamp_millis(),
                    elapsed_secs: 0,
                    status: RunStatus::Running,
                    stats: RunStats::default(),
                    notes: Vec::new(),
                });
            }

            // Create digitizer config snapshot for this run
            if let Some(ref digitizer_repo) = state.digitizer_repo {
                let configs: Vec<_> = state
                    .digitizer_configs
                    .iter()
                    .map(|r| r.value().clone())
                    .collect();
                if !configs.is_empty() {
                    if let Err(e) = digitizer_repo
                        .create_run_snapshot(run_number as i32, exp_name, configs)
                        .await
                    {
                        tracing::error!("Failed to create config snapshot: {}", e);
                        mongo_warnings.push(format!("config snapshot not persisted: {}", e));
                    }
                } else {
                    tracing::warn!(
                        run_number,
                        "digitizer_configs is empty — config snapshot skipped. \
                         Check that config_file paths in TOML are correct and \
                         operator is started from the project root."
                    );
                }
            }

            let mut message = format!(
                "Run {} started successfully (all components synchronized)",
                run_number
            );
            // TODO 58 M16: surface persistence failures in the HTTP response.
            if !mongo_warnings.is_empty() {
                message = format!(
                    "{} — WARNING, run history incomplete: {}",
                    message,
                    mongo_warnings.join("; ")
                );
            }
            (
                StatusCode::OK,
                Json(ApiResponse::success(message).with_results(results)),
            )
        }
    }
}
