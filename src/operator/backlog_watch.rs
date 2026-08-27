//! Backlog autostop watcher (TODO 68, opt-in).
//!
//! The Operator has no background status poll of its own — status is pulled
//! on demand by the UI and the InfluxDB exporter. When the
//! `[operator.backlog_autostop]` TOML section is present, this task polls
//! component status and answers a `backlog_level == 2` report (hard
//! watermark exceeded) with a drain-first stop: sources stopped first, the
//! downstream backlog written to disk while Merger/Recorder are still
//! Running, then the rest of the pipeline stopped. That converts an
//! OOM-killer lottery into a short-but-complete run.
//!
//! Double opt-in by design: this watcher AND a component-side
//! `backlog_hard_limit_mb > 0` are both required before anything fires.

use std::sync::Arc;

use tracing::{error, info, warn};

use crate::config::BacklogAutostopConfig;
use crate::operator::routes::AppState;

pub async fn run_watcher(config: BacklogAutostopConfig, state: Arc<AppState>) {
    let interval = std::time::Duration::from_secs(config.poll_interval_secs.max(1));
    info!(
        poll_interval_secs = config.poll_interval_secs,
        "Backlog autostop watcher started"
    );

    loop {
        tokio::time::sleep(interval).await;

        // Only act while a run is active and Tune Up is not (Tune Up owns
        // its own stop paths; firing mid-tuneup would strand its state).
        if state.current_run.read().await.is_none() || state.tuneup.is_active() {
            continue;
        }

        let statuses = state.client.get_all_status(&state.components).await;
        let critical: Vec<(String, u64)> = statuses
            .iter()
            .filter(|s| s.online)
            .filter_map(|s| {
                let m = s.metrics.as_ref()?;
                (m.backlog_level >= 2).then(|| (s.name.clone(), m.queue_bytes))
            })
            .collect();
        if critical.is_empty() {
            continue;
        }

        let trigger = critical
            .iter()
            .map(|(name, bytes)| format!("{} ({} bytes queued)", name, bytes))
            .collect::<Vec<_>>()
            .join(", ");
        error!(
            trigger = %trigger,
            "Backlog hard watermark exceeded — initiating drain-first stop"
        );

        // Serialize against run-control HTTP calls; re-check the run under
        // the lock (check-then-act, TODO 58 H14 discipline). Holding the
        // lock through the drain (up to the timeout) is deliberate: a
        // concurrent /api/stop afterwards finds the pipeline stopped and
        // current_run cleared, and degrades to a harmless no-op.
        let _run_guard = state.run_control_lock.lock().await;
        if state.current_run.read().await.is_none() {
            warn!("Run already ended before autostop could act — skipping");
            continue;
        }

        let reason = format!("backlog_hard_limit exceeded by {}", trigger);
        let timeout_ms = state.config.drain_stop_timeout_secs * 1000;
        let (status, response) =
            crate::operator::routes::status::drain_first_stop(&state, &reason, timeout_ms).await;
        info!(
            http_status = %status,
            message = %response.0.message,
            "Backlog autostop drain-first stop finished"
        );
    }
}
