//! Data sink - receives and processes event data via ZeroMQ
//!
//! Architecture:
//! - Main task: SUB socket for receiving data (with sequence tracking)
//! - Command task: REP socket for control commands
//!
//! This module provides a data consumer that subscribes to event data
//! and outputs statistics to the console.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use thiserror::Error;
use tmq::{request_reply, subscribe, Context};
use tokio::sync::{watch, Mutex};
use tracing::{debug, info, warn};

use crate::common::{Command, CommandResponse, ComponentMetrics, ComponentState, Message, MinimalEventDataBatch, RunConfig};

/// DataSink configuration
#[derive(Debug, Clone)]
pub struct DataSinkConfig {
    /// ZMQ connect address (e.g., "tcp://localhost:5555")
    pub address: String,
    /// ZMQ bind address for commands (e.g., "tcp://*:5580")
    pub command_address: String,
    /// Statistics output interval in seconds
    pub stats_interval_secs: u64,
}

impl Default for DataSinkConfig {
    fn default() -> Self {
        Self {
            address: "tcp://localhost:5555".to_string(),
            command_address: "tcp://*:5580".to_string(),
            stats_interval_secs: 1,
        }
    }
}

/// DataSink errors
#[derive(Error, Debug)]
pub enum DataSinkError {
    #[error("ZMQ error: {0}")]
    Zmq(#[from] tmq::TmqError),

    #[error("Deserialization error: {0}")]
    Deserialization(#[from] rmp_serde::decode::Error),
}

/// Per-source statistics with sequence tracking
#[derive(Debug, Default, Clone)]
pub struct SourceStats {
    pub last_sequence: Option<u64>,
    pub total_batches: u64,
    pub total_events: u64,
    pub gaps_detected: u64,
    pub total_gap_size: u64,
    pub restart_count: u32,
}

impl SourceStats {
    fn update(&mut self, batch: &MinimalEventDataBatch) {
        let seq = batch.sequence_number;

        if let Some(last) = self.last_sequence {
            // Detect restart: sequence dropped significantly
            if seq < last.saturating_sub(100) {
                self.restart_count += 1;
                info!(
                    source_id = batch.source_id,
                    last_seq = last,
                    new_seq = seq,
                    restarts = self.restart_count,
                    "Source restart detected"
                );
            } else {
                // Detect gaps (missing sequences)
                let expected = last + 1;
                if seq > expected {
                    let gap = seq - expected;
                    self.gaps_detected += 1;
                    self.total_gap_size += gap;
                    warn!(
                        source_id = batch.source_id,
                        expected = expected,
                        received = seq,
                        gap = gap,
                        total_gaps = self.gaps_detected,
                        "Sequence gap detected"
                    );
                }
            }
        }

        self.last_sequence = Some(seq);
        self.total_batches += 1;
        self.total_events += batch.len() as u64;
    }
}

/// Statistics tracker
#[derive(Debug, Default, Clone)]
pub struct DataSinkStats {
    pub sources: HashMap<u32, SourceStats>,
    pub total_batches: u64,
    pub total_events: u64,
    pub eos_received: u64,
    pub start_time_secs: Option<f64>,
    events_since_last_report: u64,
    last_report_elapsed_secs: f64,
}

impl DataSinkStats {
    fn update(&mut self, batch: &MinimalEventDataBatch) {
        self.sources.entry(batch.source_id).or_default().update(batch);
        self.total_batches += 1;
        self.total_events += batch.len() as u64;
        self.events_since_last_report += batch.len() as u64;
    }

    fn record_eos(&mut self) {
        self.eos_received += 1;
    }

    /// Get summary of all gaps across sources
    pub fn total_gaps(&self) -> u64 {
        self.sources.values().map(|s| s.gaps_detected).sum()
    }

    /// Get total missing sequences across sources
    pub fn total_missing(&self) -> u64 {
        self.sources.values().map(|s| s.total_gap_size).sum()
    }

    fn report(&mut self, total_elapsed: f64, interval_elapsed: f64) -> String {
        let events_per_sec = if interval_elapsed > 0.0 {
            self.events_since_last_report as f64 / interval_elapsed
        } else {
            0.0
        };
        let total_rate = if total_elapsed > 0.0 {
            self.total_events as f64 / total_elapsed
        } else {
            0.0
        };

        let report = format!(
            "Events: {} total ({:.0}/s avg, {:.0}/s current) | Batches: {} | Gaps: {} | Missing: {}",
            self.total_events,
            total_rate,
            events_per_sec,
            self.total_batches,
            self.total_gaps(),
            self.total_missing()
        );

        self.events_since_last_report = 0;
        self.last_report_elapsed_secs = total_elapsed;

        report
    }
}

/// Shared state between main task and command task
struct SharedState {
    state: ComponentState,
    stats: DataSinkStats,
    start_time: Option<Instant>,
    last_report_time: Option<Instant>,
    run_config: Option<RunConfig>,
}

impl SharedState {
    fn new() -> Self {
        Self {
            state: ComponentState::Idle,
            stats: DataSinkStats::default(),
            start_time: None,
            last_report_time: None,
            run_config: None,
        }
    }
}

/// Data sink - subscribes to event data via ZeroMQ
///
/// # C++ Equivalent
/// Similar to a consumer component with a SUB socket.
/// Uses async/await instead of blocking recv().
pub struct DataSink {
    config: DataSinkConfig,
    shared_state: Arc<Mutex<SharedState>>,
    state_rx: watch::Receiver<ComponentState>,
    state_tx: watch::Sender<ComponentState>,
}

impl DataSink {
    /// Create a new data sink with the given configuration
    pub async fn new(config: DataSinkConfig) -> Result<Self, DataSinkError> {
        let (state_tx, state_rx) = watch::channel(ComponentState::Idle);

        info!(
            data_address = %config.address,
            command_address = %config.command_address,
            "DataSink created"
        );

        Ok(Self {
            config,
            shared_state: Arc::new(Mutex::new(SharedState::new())),
            state_rx,
            state_tx,
        })
    }

    /// Get current state
    pub fn state(&self) -> ComponentState {
        *self.state_rx.borrow()
    }

    /// Handle incoming command (5-state machine)
    fn handle_command(
        state: &mut SharedState,
        state_tx: &watch::Sender<ComponentState>,
        cmd: Command,
    ) -> CommandResponse {
        let current = state.state;

        match cmd {
            Command::Configure(run_config) => {
                if !current.can_transition_to(ComponentState::Configured) {
                    return CommandResponse::error(
                        current,
                        format!("Cannot configure from {} state", current),
                    );
                }
                let run_number = run_config.run_number;
                state.run_config = Some(run_config);
                state.state = ComponentState::Configured;
                let _ = state_tx.send(ComponentState::Configured);
                info!(run_number, "DataSink configured");
                CommandResponse::success_with_run(ComponentState::Configured, "Configured", run_number)
            }

            Command::Arm => {
                if !current.can_transition_to(ComponentState::Armed) {
                    return CommandResponse::error(
                        current,
                        format!("Cannot arm from {} state", current),
                    );
                }
                state.state = ComponentState::Armed;
                let _ = state_tx.send(ComponentState::Armed);
                info!("DataSink armed");
                let run_number = state.run_config.as_ref().map(|c| c.run_number).unwrap_or(0);
                CommandResponse::success_with_run(ComponentState::Armed, "Armed", run_number)
            }

            Command::Start => {
                if !current.can_transition_to(ComponentState::Running) {
                    return CommandResponse::error(
                        current,
                        format!("Cannot start from {} state", current),
                    );
                }
                state.state = ComponentState::Running;
                state.start_time = Some(Instant::now());
                state.last_report_time = Some(Instant::now());
                let _ = state_tx.send(ComponentState::Running);
                info!("DataSink started");
                let run_number = state.run_config.as_ref().map(|c| c.run_number).unwrap_or(0);
                CommandResponse::success_with_run(ComponentState::Running, "Started", run_number)
            }

            Command::Stop => {
                if current != ComponentState::Running {
                    return CommandResponse::error(current, "Not running");
                }
                // Print final statistics before stopping
                let total_elapsed = state.start_time
                    .map(|t| t.elapsed().as_secs_f64())
                    .unwrap_or(0.0);
                let event_rate = if total_elapsed > 0.0 {
                    state.stats.total_events as f64 / total_elapsed
                } else {
                    0.0
                };
                let batch_rate = if total_elapsed > 0.0 {
                    state.stats.total_batches as f64 / total_elapsed
                } else {
                    0.0
                };

                println!();
                println!("========== Final Statistics ==========");
                println!("Duration:     {:.2} s", total_elapsed);
                println!("Total Events: {}", state.stats.total_events);
                println!("Total Batches: {}", state.stats.total_batches);
                println!("Event Rate:   {:.0} events/s ({:.2} MHz)", event_rate, event_rate / 1_000_000.0);
                println!("Batch Rate:   {:.0} batches/s", batch_rate);
                println!("Gaps:         {} ({} missing sequences)", state.stats.total_gaps(), state.stats.total_missing());
                println!("Sources:      {}", state.stats.sources.len());
                println!("=======================================");

                // Stop returns to Configured for quick restart
                state.state = ComponentState::Configured;
                let _ = state_tx.send(ComponentState::Configured);
                info!(
                    total_events = state.stats.total_events,
                    event_rate = event_rate,
                    duration_secs = total_elapsed,
                    "DataSink stopped"
                );
                let run_number = state.run_config.as_ref().map(|c| c.run_number).unwrap_or(0);
                CommandResponse::success_with_run(
                    ComponentState::Configured,
                    format!(
                        "Stopped. Events: {}, Rate: {:.0}/s, Duration: {:.2}s",
                        state.stats.total_events, event_rate, total_elapsed
                    ),
                    run_number,
                )
            }

            Command::Reset => {
                state.state = ComponentState::Idle;
                state.run_config = None;
                state.stats = DataSinkStats::default();
                state.start_time = None;
                state.last_report_time = None;
                let _ = state_tx.send(ComponentState::Idle);
                info!("DataSink reset");
                CommandResponse::success(ComponentState::Idle, "Reset to Idle")
            }

            Command::GetStatus => {
                let stats = &state.stats;
                let run_number = state.run_config.as_ref().map(|c| c.run_number);

                // Calculate rates
                let elapsed = state.start_time
                    .map(|t| t.elapsed().as_secs_f64())
                    .unwrap_or(0.0);
                let event_rate = if elapsed > 0.0 {
                    stats.total_events as f64 / elapsed
                } else {
                    0.0
                };

                let msg = format!(
                    "State: {}, Events: {}, Batches: {}, Gaps: {}, Missing: {}",
                    state.state,
                    stats.total_events,
                    stats.total_batches,
                    stats.total_gaps(),
                    stats.total_missing()
                );

                let metrics = ComponentMetrics {
                    events_processed: stats.total_events,
                    bytes_transferred: 0, // Not tracked currently
                    queue_size: 0,
                    queue_max: 0,
                    event_rate,
                    data_rate: 0.0,
                };

                let mut resp = CommandResponse::success(state.state, msg);
                resp.run_number = run_number;
                resp.metrics = Some(metrics);
                resp
            }
        }
    }

    /// Command handler task using tmq REQ/REP pattern
    async fn command_task(
        command_address: String,
        shared_state: Arc<Mutex<SharedState>>,
        state_tx: watch::Sender<ComponentState>,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) {
        let context = Context::new();

        // Bind REP socket
        let receiver = match request_reply::reply(&context).bind(&command_address) {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "Failed to bind command socket");
                return;
            }
        };

        info!(address = %command_address, "DataSink command task started");

        let mut current_receiver = receiver;

        loop {
            tokio::select! {
                biased;

                _ = shutdown.recv() => {
                    info!("DataSink command task received shutdown signal");
                    break;
                }

                recv_result = current_receiver.recv() => {
                    match recv_result {
                        Ok((mut multipart, sender)) => {
                            let response = if let Some(frame) = multipart.pop_front() {
                                match Command::from_json(&frame) {
                                    Ok(cmd) => {
                                        info!(command = %cmd, "DataSink received command");
                                        let mut state = shared_state.lock().await;
                                        Self::handle_command(&mut state, &state_tx, cmd)
                                    }
                                    Err(e) => {
                                        warn!(error = %e, "Invalid command");
                                        let state = shared_state.lock().await;
                                        CommandResponse::error(state.state, format!("Invalid: {}", e))
                                    }
                                }
                            } else {
                                let state = shared_state.lock().await;
                                CommandResponse::error(state.state, "Empty message")
                            };

                            let resp_bytes = match response.to_json() {
                                Ok(b) => b,
                                Err(e) => {
                                    warn!(error = %e, "Failed to serialize response");
                                    break;
                                }
                            };

                            let resp_msg: tmq::Multipart =
                                vec![tmq::Message::from(resp_bytes.as_slice())].into();

                            match sender.send(resp_msg).await {
                                Ok(next_receiver) => {
                                    current_receiver = next_receiver;
                                }
                                Err(e) => {
                                    warn!(error = %e, "Failed to send response");
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "Command receive error");
                            break;
                        }
                    }
                }
            }
        }

        info!("DataSink command task stopped");
    }

    /// Run the data sink loop
    pub async fn run(
        &mut self,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), DataSinkError> {
        let stats_interval = Duration::from_secs(self.config.stats_interval_secs);

        // Create SUB socket
        let context = Context::new();
        let mut socket = subscribe(&context)
            .connect(&self.config.address)?
            .subscribe(b"")?;

        info!(address = %self.config.address, "DataSink connected to upstream");

        info!(
            state = %self.state(),
            "DataSink ready, waiting for commands"
        );

        // Spawn command handler task
        let command_address = self.config.command_address.clone();
        let shared_state = self.shared_state.clone();
        let state_tx = self.state_tx.clone();
        let shutdown_for_cmd = shutdown.resubscribe();

        let cmd_handle = tokio::spawn(async move {
            Self::command_task(command_address, shared_state, state_tx, shutdown_for_cmd).await;
        });

        // Main data loop
        let mut state_rx = self.state_rx.clone();

        loop {
            let is_running = *state_rx.borrow() == ComponentState::Running;

            tokio::select! {
                biased;

                _ = shutdown.recv() => {
                    info!("DataSink received shutdown signal");
                    break;
                }

                _ = state_rx.changed() => {
                    let current = *state_rx.borrow();
                    info!(state = %current, "DataSink state changed");
                    continue;
                }

                msg = socket.next(), if is_running => {
                    match msg {
                        Some(Ok(multipart)) => {
                            if let Some(data) = multipart.into_iter().next() {
                                match Message::from_msgpack(&data) {
                                    Ok(Message::Data(batch)) => {
                                        debug!(
                                            seq = batch.sequence_number,
                                            events = batch.len(),
                                            source = batch.source_id,
                                            "Received batch"
                                        );

                                        let should_report = {
                                            let mut state = self.shared_state.lock().await;
                                            state.stats.update(&batch);

                                            // Check if should report
                                            state.last_report_time
                                                .map(|t| t.elapsed() >= stats_interval)
                                                .unwrap_or(false)
                                        };

                                        if should_report {
                                            let mut state = self.shared_state.lock().await;
                                            let total_elapsed = state.start_time
                                                .map(|t| t.elapsed().as_secs_f64())
                                                .unwrap_or(1.0);
                                            let interval_elapsed = state.last_report_time
                                                .map(|t| t.elapsed().as_secs_f64())
                                                .unwrap_or(1.0);
                                            let report = state.stats.report(total_elapsed, interval_elapsed);
                                            state.last_report_time = Some(Instant::now());
                                            println!("{}", report);
                                        }
                                    }
                                    Ok(Message::EndOfStream { source_id }) => {
                                        info!(source_id = source_id, "Received EOS from upstream");
                                        let mut state = self.shared_state.lock().await;
                                        state.stats.record_eos();
                                        // Don't break - continue receiving from other sources
                                    }
                                    Ok(Message::Heartbeat(hb)) => {
                                        debug!(source_id = hb.source_id, counter = hb.counter, "Received heartbeat");
                                        // Heartbeats are received but not processed as data
                                    }
                                    Err(e) => {
                                        warn!(error = %e, "Failed to deserialize message");
                                    }
                                }
                            }
                        }
                        Some(Err(e)) => {
                            warn!(error = %e, "ZMQ receive error");
                        }
                        None => {
                            info!("Socket closed");
                            break;
                        }
                    }
                }
            }
        }

        // Wait for command task
        let _ = cmd_handle.await;

        // Final stats report
        let state = self.shared_state.lock().await;
        let total_elapsed = state.start_time
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0);

        let event_rate = if total_elapsed > 0.0 {
            state.stats.total_events as f64 / total_elapsed
        } else {
            0.0
        };
        let batch_rate = if total_elapsed > 0.0 {
            state.stats.total_batches as f64 / total_elapsed
        } else {
            0.0
        };

        println!();
        println!("========== Final Statistics ==========");
        println!("Duration:     {:.2} s", total_elapsed);
        println!("Total Events: {}", state.stats.total_events);
        println!("Total Batches: {}", state.stats.total_batches);
        println!("Event Rate:   {:.0} events/s ({:.2} MHz)", event_rate, event_rate / 1_000_000.0);
        println!("Batch Rate:   {:.0} batches/s", batch_rate);
        println!("Gaps:         {} ({} missing sequences)", state.stats.total_gaps(), state.stats.total_missing());
        println!("Sources:      {}", state.stats.sources.len());
        println!("=======================================");

        info!(
            total_batches = state.stats.total_batches,
            total_events = state.stats.total_events,
            event_rate = event_rate,
            gaps = state.stats.total_gaps(),
            missing = state.stats.total_missing(),
            eos = state.stats.eos_received,
            "DataSink stopped"
        );

        Ok(())
    }

    /// Get current statistics
    pub async fn stats(&self) -> DataSinkStats {
        self.shared_state.lock().await.stats.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::MinimalEventData;

    #[test]
    fn default_config() {
        let config = DataSinkConfig::default();
        assert_eq!(config.address, "tcp://localhost:5555");
        assert_eq!(config.stats_interval_secs, 1);
    }

    #[test]
    fn source_stats_tracking() {
        let mut stats = SourceStats::default();

        let mut batch = MinimalEventDataBatch::new(0, 0);
        batch.push(MinimalEventData::zeroed());
        batch.push(MinimalEventData::zeroed());

        stats.update(&batch);

        assert_eq!(stats.total_batches, 1);
        assert_eq!(stats.total_events, 2);
        assert_eq!(stats.last_sequence, Some(0));
    }

    #[test]
    fn gap_detection() {
        let mut stats = DataSinkStats::default();

        // First batch with seq 0
        let batch0 = MinimalEventDataBatch::new(0, 0);
        stats.update(&batch0);

        // Skip to seq 5 (lost 1,2,3,4)
        let batch5 = MinimalEventDataBatch::new(0, 5);
        stats.update(&batch5);

        assert_eq!(stats.total_gaps(), 1);
        assert_eq!(stats.total_missing(), 4);
    }

    #[test]
    fn multi_source_tracking() {
        let mut stats = DataSinkStats::default();

        // Source 0: sequences 0, 1, 5 (gap of 3)
        stats.update(&MinimalEventDataBatch::new(0, 0));
        stats.update(&MinimalEventDataBatch::new(0, 1));
        stats.update(&MinimalEventDataBatch::new(0, 5));

        // Source 1: sequences 0, 10 (gap of 9)
        stats.update(&MinimalEventDataBatch::new(1, 0));
        stats.update(&MinimalEventDataBatch::new(1, 10));

        assert_eq!(stats.sources.len(), 2);
        assert_eq!(stats.total_gaps(), 2);
        assert_eq!(stats.total_missing(), 12); // 3 + 9
    }
}
