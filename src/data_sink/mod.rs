//! Data sink - receives and processes event data via ZeroMQ
//!
//! Architecture (Lock-Free):
//! - Receiver task: SUB socket → mpsc channel (non-blocking)
//! - Processor task: mpsc channel → stats update + console output
//! - Command task: REP socket for control commands
//!
//! This module provides a data consumer that subscribes to event data
//! and outputs statistics to the console.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use thiserror::Error;
use tmq::{subscribe, Context};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

use crate::common::{
    handle_command, run_command_task, CommandHandlerExt, ComponentSharedState, ComponentState,
    EventDataBatch, Message,
};

/// DataSink configuration
#[derive(Debug, Clone)]
pub struct DataSinkConfig {
    /// ZMQ connect address (e.g., "tcp://localhost:5555")
    pub address: String,
    /// ZMQ bind address for commands (e.g., "tcp://*:5580")
    pub command_address: String,
    /// Statistics output interval in seconds
    pub stats_interval_secs: u64,
    /// Internal channel capacity
    pub channel_capacity: usize,
}

impl Default for DataSinkConfig {
    fn default() -> Self {
        Self {
            address: "tcp://localhost:5555".to_string(),
            command_address: "tcp://*:5580".to_string(),
            stats_interval_secs: 1,
            channel_capacity: 1000,
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
    fn update(&mut self, batch: &EventDataBatch) {
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
    fn update(&mut self, batch: &EventDataBatch) {
        self.sources
            .entry(batch.source_id)
            .or_default()
            .update(batch);
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

/// Atomic counters for hot-path statistics (lock-free)
struct AtomicStats {
    received_batches: AtomicU64,
    processed_batches: AtomicU64,
    dropped_batches: AtomicU64,
    eos_received: AtomicU64,
}

impl AtomicStats {
    fn new() -> Self {
        Self {
            received_batches: AtomicU64::new(0),
            processed_batches: AtomicU64::new(0),
            dropped_batches: AtomicU64::new(0),
            eos_received: AtomicU64::new(0),
        }
    }

    #[inline]
    fn record_received(&self) {
        self.received_batches.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    fn record_processed(&self) {
        self.processed_batches.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    #[allow(dead_code)] // Reserved for future bounded channel debugging
    fn record_drop(&self) {
        self.dropped_batches.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    fn record_eos(&self) {
        self.eos_received.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> (u64, u64, u64, u64) {
        (
            self.received_batches.load(Ordering::Relaxed),
            self.processed_batches.load(Ordering::Relaxed),
            self.dropped_batches.load(Ordering::Relaxed),
            self.eos_received.load(Ordering::Relaxed),
        )
    }
}

/// Message type for internal channel
enum ProcessorMessage {
    Data(EventDataBatch),
    Eos { source_id: u32 },
}

/// Command handler extension for DataSink
struct DataSinkCommandExt {
    atomic_stats: Arc<AtomicStats>,
}

impl CommandHandlerExt for DataSinkCommandExt {
    fn component_name(&self) -> &'static str {
        "DataSink"
    }

    fn status_details(&self) -> Option<String> {
        let (recv, proc, drop, eos) = self.atomic_stats.snapshot();
        Some(format!(
            "Received: {}, Processed: {}, Dropped: {}, EOS: {}",
            recv, proc, drop, eos
        ))
    }
}

/// Data sink - subscribes to event data via ZeroMQ
///
/// # C++ Equivalent
/// Similar to a consumer component with a SUB socket.
/// Uses async/await instead of blocking recv().
pub struct DataSink {
    config: DataSinkConfig,
    shared_state: Arc<tokio::sync::Mutex<ComponentSharedState>>,
    atomic_stats: Arc<AtomicStats>,
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
            shared_state: Arc::new(tokio::sync::Mutex::new(ComponentSharedState::new())),
            atomic_stats: Arc::new(AtomicStats::new()),
            state_rx,
            state_tx,
        })
    }

    /// Get current state
    pub fn state(&self) -> ComponentState {
        *self.state_rx.borrow()
    }

    /// Run the data sink loop
    pub async fn run(
        &mut self,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), DataSinkError> {
        // Create channel for receiver → processor (unbounded - memory growth indicates bottleneck)
        let (proc_tx, proc_rx) = mpsc::unbounded_channel::<ProcessorMessage>();

        // Create SUB socket
        let context = Context::new();
        let socket = subscribe(&context)
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
        let atomic_stats_for_cmd = self.atomic_stats.clone();

        let cmd_handle = tokio::spawn(async move {
            run_command_task(
                command_address,
                shared_state,
                state_tx,
                shutdown_for_cmd,
                move |state, tx, cmd| {
                    let mut ext = DataSinkCommandExt {
                        atomic_stats: atomic_stats_for_cmd.clone(),
                    };
                    handle_command(state, tx, cmd, Some(&mut ext))
                },
                "DataSink",
            )
            .await;
        });

        // Spawn receiver task
        let shutdown_for_recv = shutdown.resubscribe();
        let atomic_stats_for_recv = self.atomic_stats.clone();
        let state_rx_for_recv = self.state_rx.clone();
        let recv_handle = tokio::spawn(async move {
            Self::receiver_task(
                socket,
                proc_tx,
                shutdown_for_recv,
                atomic_stats_for_recv,
                state_rx_for_recv,
            )
            .await
        });

        // Spawn processor task
        let atomic_stats_for_proc = self.atomic_stats.clone();
        let stats_interval_secs = self.config.stats_interval_secs;
        let proc_handle = tokio::spawn(async move {
            Self::processor_task(proc_rx, atomic_stats_for_proc, stats_interval_secs).await
        });

        // Wait for shutdown signal
        let _ = shutdown.recv().await;
        info!("DataSink received shutdown signal");

        // Wait for tasks to complete
        let _ = recv_handle.await;
        let _ = proc_handle.await;
        let _ = cmd_handle.await;

        // Log final stats
        let (recv, proc, drop, eos) = self.atomic_stats.snapshot();
        info!(
            received = recv,
            processed = proc,
            dropped = drop,
            eos = eos,
            "DataSink stopped"
        );

        Ok(())
    }

    /// Receiver task: SUB → channel (non-blocking)
    async fn receiver_task(
        mut socket: subscribe::Subscribe,
        tx: mpsc::UnboundedSender<ProcessorMessage>,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
        atomic_stats: Arc<AtomicStats>,
        mut state_rx: watch::Receiver<ComponentState>,
    ) {
        loop {
            let is_running = *state_rx.borrow() == ComponentState::Running;

            tokio::select! {
                biased;

                _ = shutdown.recv() => {
                    info!("DataSink receiver task shutting down");
                    break;
                }

                _ = state_rx.changed() => {
                    let current = *state_rx.borrow();
                    info!(state = %current, "DataSink receiver state changed");
                    continue;
                }

                msg = socket.next(), if is_running => {
                    match msg {
                        Some(Ok(multipart)) => {
                            if let Some(data) = multipart.into_iter().next() {
                                match Message::from_msgpack(&data) {
                                    Ok(Message::Data(batch)) => {
                                        atomic_stats.record_received();
                                        debug!(
                                            seq = batch.sequence_number,
                                            events = batch.len(),
                                            source = batch.source_id,
                                            "Received batch"
                                        );

                                        // Non-blocking send to processor (unbounded)
                                        if tx.send(ProcessorMessage::Data(batch)).is_err() {
                                            info!("Processor channel closed, exiting");
                                            break;
                                        }
                                    }
                                    Ok(Message::EndOfStream { source_id }) => {
                                        atomic_stats.record_eos();
                                        info!(source_id = source_id, "Received EOS from upstream");
                                        let _ = tx.send(ProcessorMessage::Eos { source_id });
                                    }
                                    Ok(Message::Heartbeat(hb)) => {
                                        debug!(source_id = hb.source_id, counter = hb.counter, "Received heartbeat");
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
    }

    /// Processor task: channel → stats + console output
    async fn processor_task(
        mut rx: mpsc::UnboundedReceiver<ProcessorMessage>,
        atomic_stats: Arc<AtomicStats>,
        stats_interval_secs: u64,
    ) {
        let mut stats = DataSinkStats::default();
        let start_time = Instant::now();
        let mut last_report_time = Instant::now();
        let stats_interval = Duration::from_secs(stats_interval_secs);

        while let Some(msg) = rx.recv().await {
            match msg {
                ProcessorMessage::Data(batch) => {
                    stats.update(&batch);
                    atomic_stats.record_processed();

                    // Check if should report
                    if last_report_time.elapsed() >= stats_interval {
                        let total_elapsed = start_time.elapsed().as_secs_f64();
                        let interval_elapsed = last_report_time.elapsed().as_secs_f64();
                        let report = stats.report(total_elapsed, interval_elapsed);
                        last_report_time = Instant::now();
                        println!("{}", report);
                    }
                }
                ProcessorMessage::Eos { source_id } => {
                    stats.record_eos();
                    info!(source_id = source_id, "Processed EOS");
                }
            }
        }

        // Final stats report
        let total_elapsed = start_time.elapsed().as_secs_f64();
        let event_rate = if total_elapsed > 0.0 {
            stats.total_events as f64 / total_elapsed
        } else {
            0.0
        };
        let batch_rate = if total_elapsed > 0.0 {
            stats.total_batches as f64 / total_elapsed
        } else {
            0.0
        };

        println!();
        println!("========== Final Statistics ==========");
        println!("Duration:     {:.2} s", total_elapsed);
        println!("Total Events: {}", stats.total_events);
        println!("Total Batches: {}", stats.total_batches);
        println!(
            "Event Rate:   {:.0} events/s ({:.2} MHz)",
            event_rate,
            event_rate / 1_000_000.0
        );
        println!("Batch Rate:   {:.0} batches/s", batch_rate);
        println!(
            "Gaps:         {} ({} missing sequences)",
            stats.total_gaps(),
            stats.total_missing()
        );
        println!("Sources:      {}", stats.sources.len());
        println!("=======================================");

        info!("Processor task completed");
    }

    /// Get current statistics snapshot
    pub fn stats_snapshot(&self) -> (u64, u64, u64, u64) {
        self.atomic_stats.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::EventData;

    #[test]
    fn default_config() {
        let config = DataSinkConfig::default();
        assert_eq!(config.address, "tcp://localhost:5555");
        assert_eq!(config.stats_interval_secs, 1);
    }

    #[test]
    fn source_stats_tracking() {
        let mut stats = SourceStats::default();

        let mut batch = EventDataBatch::new(0, 0);
        batch.push(EventData::zeroed());
        batch.push(EventData::zeroed());

        stats.update(&batch);

        assert_eq!(stats.total_batches, 1);
        assert_eq!(stats.total_events, 2);
        assert_eq!(stats.last_sequence, Some(0));
    }

    #[test]
    fn gap_detection() {
        let mut stats = DataSinkStats::default();

        // First batch with seq 0
        let batch0 = EventDataBatch::new(0, 0);
        stats.update(&batch0);

        // Skip to seq 5 (lost 1,2,3,4)
        let batch5 = EventDataBatch::new(0, 5);
        stats.update(&batch5);

        assert_eq!(stats.total_gaps(), 1);
        assert_eq!(stats.total_missing(), 4);
    }

    #[test]
    fn multi_source_tracking() {
        let mut stats = DataSinkStats::default();

        // Source 0: sequences 0, 1, 5 (gap of 3)
        stats.update(&EventDataBatch::new(0, 0));
        stats.update(&EventDataBatch::new(0, 1));
        stats.update(&EventDataBatch::new(0, 5));

        // Source 1: sequences 0, 10 (gap of 9)
        stats.update(&EventDataBatch::new(1, 0));
        stats.update(&EventDataBatch::new(1, 10));

        assert_eq!(stats.sources.len(), 2);
        assert_eq!(stats.total_gaps(), 2);
        assert_eq!(stats.total_missing(), 12); // 3 + 9
    }

    #[test]
    fn atomic_stats() {
        let stats = AtomicStats::new();
        stats.record_received();
        stats.record_received();
        stats.record_processed();
        stats.record_drop();

        let (recv, proc, drop, eos) = stats.snapshot();
        assert_eq!(recv, 2);
        assert_eq!(proc, 1);
        assert_eq!(drop, 1);
        assert_eq!(eos, 0);
    }
}
