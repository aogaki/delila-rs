//! Run history handlers and response types

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::config::DigitizerConfig;

use super::super::{ApiResponse, RunDocument, RunNote, RunStats, RunStatus};
use super::AppState;

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

/// Next run number response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct NextRunNumberResponse {
    pub next_run_number: i32,
}

/// Request body for adding a note
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AddNoteRequest {
    /// Note text to add
    pub text: String,
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
pub(super) async fn get_run_config_snapshot(
    State(state): State<Arc<AppState>>,
    Path(run_number): Path<i32>,
) -> Result<Json<Vec<DigitizerConfig>>, (StatusCode, Json<ApiResponse>)> {
    let repo = state.digitizer_repo.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiResponse::error(
                "MongoDB not configured for digitizer configs",
            )),
        )
    })?;

    let snapshot = repo
        .get_run_snapshot(run_number, &state.config.experiment_name)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::error(format!(
                    "Failed to get snapshot: {}",
                    e
                ))),
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
pub(super) async fn get_run_history(
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
pub(super) async fn get_run(
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
pub(super) async fn get_next_run_number(
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
pub(super) async fn add_run_note(
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
