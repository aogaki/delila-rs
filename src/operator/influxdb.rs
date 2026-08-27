//! InfluxDB metrics writer for Grafana monitoring
//!
//! Polls Reader components via ZMQ GetStatus for per-channel event counts,
//! and writes cumulative counts to InfluxDB v3 Core using Line Protocol.
//! Rate calculation is done in Grafana using derivative().

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use super::routes::AppState;
use crate::config::InfluxDbConfig;

/// Run the InfluxDB writer loop. Never returns unless InfluxDB is unreachable.
pub async fn run_writer(config: InfluxDbConfig, state: Arc<AppState>) {
    let client = reqwest::Client::new();
    let write_url = format!(
        "{}/api/v3/write_lp?db={}",
        config.url.trim_end_matches('/'),
        config.database
    );
    let interval = Duration::from_secs(config.interval_secs);

    info!(
        url = %config.url,
        database = %config.database,
        interval_secs = config.interval_secs,
        "InfluxDB writer started"
    );

    loop {
        tokio::time::sleep(interval).await;

        // Poll all components for status
        let statuses = state.client.get_all_status(&state.components).await;

        // Build Line Protocol lines from Reader metrics
        let mut lines = Vec::new();

        for (comp, status) in state.components.iter().zip(statuses.iter()) {
            if !status.online {
                continue;
            }

            let metrics = match &status.metrics {
                Some(m) => m,
                None => continue,
            };

            // Non-source components (Merger/Recorder/...): export the backlog
            // gauge (TODO 68) — before this, their metrics never reached
            // Grafana at all.
            let source_id = match comp.source_id {
                Some(id) => id,
                None => {
                    lines.push(format!(
                        "backlog,component={} queue_bytes={}u,queue_bytes_peak={}u,queue_items={}u,backlog_level={}u,events_processed={}u",
                        comp.name.replace(' ', "\\ "),
                        metrics.queue_bytes,
                        metrics.queue_bytes_peak,
                        metrics.queue_size,
                        metrics.backlog_level,
                        metrics.events_processed,
                    ));
                    continue;
                }
            };

            // System-level metrics for this source
            lines.push(format!(
                "system_rate,module={} total_events={}u,event_rate={},bytes={}u",
                source_id, metrics.events_processed, metrics.event_rate, metrics.bytes_transferred,
            ));

            // Per-channel counts
            if let Some(ref counts) = metrics.channel_counts {
                for (ch, &count) in counts.iter().enumerate() {
                    if count > 0 {
                        lines.push(format!(
                            "channel_rate,module={},channel={} count={}u",
                            source_id, ch, count,
                        ));
                    }
                }
            }
        }

        if lines.is_empty() {
            continue;
        }

        let body = lines.join("\n");

        match client.post(&write_url).body(body).send().await {
            Ok(resp) if resp.status().is_success() => {
                // Success - no log needed at normal operation
            }
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                warn!(
                    status = %status,
                    body = %text,
                    "InfluxDB write failed"
                );
            }
            Err(e) => {
                warn!(error = %e, "InfluxDB connection error");
            }
        }
    }
}
