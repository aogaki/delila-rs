//! Merger - receives from multiple upstream sources and forwards downstream
//!
//! Architecture:
//! - Receiver task: SUB socket → mpsc channel (with sequence tracking)
//! - Sender task: mpsc channel → PUB socket
//! - Command task: REP socket for control commands
//! - Decoupled to prevent send blocking from affecting receive
//!
//! Performance: Uses AtomicU64 for hot-path counters to avoid mutex contention

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use thiserror::Error;
use tmq::{publish, request_reply, subscribe, AsZmqSocket, Context};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

use crate::common::{Command, CommandResponse, ComponentMetrics, ComponentState, Message, RunConfig};

/// Merger configuration
#[derive(Debug, Clone)]
pub struct MergerConfig {
    /// ZMQ addresses to subscribe to (upstream sources)
    pub sub_addresses: Vec<String>,
    /// ZMQ address to publish to (downstream)
    pub pub_address: String,
    /// ZMQ bind address for commands (e.g., "tcp://*:5570")
    pub command_address: String,
    /// Internal channel capacity (bounded buffer)
    pub channel_capacity: usize,
}

impl Default for MergerConfig {
    fn default() -> Self {
        Self {
            sub_addresses: vec!["tcp://localhost:5555".to_string()],
            pub_address: "tcp://*:5556".to_string(),
            command_address: "tcp://*:5570".to_string(),
            channel_capacity: 1000,
        }
    }
}

/// Merger errors
#[derive(Error, Debug)]
pub enum MergerError {
    #[error("ZMQ error: {0}")]
    Zmq(#[from] tmq::TmqError),

    #[error("ZMQ socket error: {0}")]
    ZmqSocket(#[from] zmq::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] rmp_serde::encode::Error),

    #[error("Deserialization error: {0}")]
    Deserialization(#[from] rmp_serde::decode::Error),

    #[error("Channel send error")]
    ChannelSend,

    #[error("No upstream addresses configured")]
    NoUpstreamAddresses,
}

/// Per-source statistics with sequence tracking
#[derive(Debug, Default, Clone)]
pub struct SourceStats {
    pub last_sequence: Option<u64>,
    pub total_batches: u64,
    pub restart_count: u32,
    pub gaps_detected: u64,
    pub total_gap_size: u64,
}

impl SourceStats {
    fn update(&mut self, seq: u64) -> bool {
        let restarted = if let Some(last) = self.last_sequence {
            if seq < last.saturating_sub(100) {
                self.restart_count += 1;
                true
            } else {
                let expected = last + 1;
                if seq > expected {
                    let gap = seq - expected;
                    self.gaps_detected += 1;
                    self.total_gap_size += gap;
                }
                false
            }
        } else {
            false
        };

        self.last_sequence = Some(seq);
        self.total_batches += 1;
        restarted
    }
}

/// Atomic counters for hot-path statistics (lock-free)
struct AtomicStats {
    received_batches: AtomicU64,
    sent_batches: AtomicU64,
    dropped_batches: AtomicU64,
    eos_received: AtomicU64,
}

impl AtomicStats {
    fn new() -> Self {
        Self {
            received_batches: AtomicU64::new(0),
            sent_batches: AtomicU64::new(0),
            dropped_batches: AtomicU64::new(0),
            eos_received: AtomicU64::new(0),
        }
    }

    #[inline]
    fn record_received(&self) {
        self.received_batches.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    fn record_sent(&self) {
        self.sent_batches.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
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
            self.sent_batches.load(Ordering::Relaxed),
            self.dropped_batches.load(Ordering::Relaxed),
            self.eos_received.load(Ordering::Relaxed),
        )
    }
}

/// Merger statistics (for reporting)
#[derive(Debug, Default, Clone)]
pub struct MergerStats {
    pub received_batches: u64,
    pub sent_batches: u64,
    pub dropped_batches: u64,
    pub eos_received: u64,
    pub sources: HashMap<u32, SourceStats>,
}

impl MergerStats {
    /// Get summary of all gaps across sources
    pub fn total_gaps(&self) -> u64 {
        self.sources.values().map(|s| s.gaps_detected).sum()
    }

    /// Get total missing sequences across sources
    pub fn total_missing(&self) -> u64 {
        self.sources.values().map(|s| s.total_gap_size).sum()
    }
}

/// Shared state between tasks
struct SharedState {
    // Sequence tracking per source (lock-free concurrent map)
    source_stats: DashMap<u32, SourceStats>,
    // Hot-path counters (lock-free)
    atomic_stats: AtomicStats,
    // Run configuration (protected by parking_lot Mutex for infrequent access)
    run_config: parking_lot::Mutex<Option<RunConfig>>,
}

impl SharedState {
    fn new() -> Self {
        Self {
            source_stats: DashMap::new(),
            atomic_stats: AtomicStats::new(),
            run_config: parking_lot::Mutex::new(None),
        }
    }

    fn get_stats(&self) -> MergerStats {
        let (received, sent, dropped, eos) = self.atomic_stats.snapshot();
        // Clone entries from DashMap (brief per-entry locks, not global)
        let sources: HashMap<u32, SourceStats> = self
            .source_stats
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect();
        MergerStats {
            received_batches: received,
            sent_batches: sent,
            dropped_batches: dropped,
            eos_received: eos,
            sources,
        }
    }
}

/// Merger component
pub struct Merger {
    config: MergerConfig,
    shared_state: Arc<SharedState>,
    state_rx: watch::Receiver<ComponentState>,
    state_tx: watch::Sender<ComponentState>,
}

impl Merger {
    /// Create a new merger with the given configuration
    pub fn new(config: MergerConfig) -> Self {
        let (state_tx, state_rx) = watch::channel(ComponentState::Idle);
        Self {
            config,
            shared_state: Arc::new(SharedState::new()),
            state_rx,
            state_tx,
        }
    }

    /// Get current state
    pub fn state(&self) -> ComponentState {
        *self.state_rx.borrow()
    }

    /// Handle incoming command (5-state machine)
    fn handle_command(
        shared_state: &Arc<SharedState>,
        state_tx: &watch::Sender<ComponentState>,
        cmd: Command,
    ) -> CommandResponse {
        let current = *state_tx.borrow();

        match cmd {
            Command::Configure(run_config) => {
                if !current.can_transition_to(ComponentState::Configured) {
                    return CommandResponse::error(
                        current,
                        format!("Cannot configure from {} state", current),
                    );
                }
                let run_number = run_config.run_number;
                *shared_state.run_config.lock() = Some(run_config);
                let _ = state_tx.send(ComponentState::Configured);
                info!(run_number, "Merger configured");
                CommandResponse::success_with_run(ComponentState::Configured, "Configured", run_number)
            }

            Command::Arm => {
                if !current.can_transition_to(ComponentState::Armed) {
                    return CommandResponse::error(
                        current,
                        format!("Cannot arm from {} state", current),
                    );
                }
                let _ = state_tx.send(ComponentState::Armed);
                info!("Merger armed");
                let run_number = shared_state.run_config.lock().as_ref().map(|c| c.run_number).unwrap_or(0);
                CommandResponse::success_with_run(ComponentState::Armed, "Armed", run_number)
            }

            Command::Start => {
                if !current.can_transition_to(ComponentState::Running) {
                    return CommandResponse::error(
                        current,
                        format!("Cannot start from {} state", current),
                    );
                }
                let _ = state_tx.send(ComponentState::Running);
                info!("Merger started");
                let run_number = shared_state.run_config.lock().as_ref().map(|c| c.run_number).unwrap_or(0);
                CommandResponse::success_with_run(ComponentState::Running, "Started", run_number)
            }

            Command::Stop => {
                if current != ComponentState::Running {
                    return CommandResponse::error(current, "Not running");
                }
                let _ = state_tx.send(ComponentState::Configured);
                info!("Merger stopped");
                let run_number = shared_state.run_config.lock().as_ref().map(|c| c.run_number).unwrap_or(0);
                CommandResponse::success_with_run(ComponentState::Configured, "Stopped", run_number)
            }

            Command::Reset => {
                *shared_state.run_config.lock() = None;
                // Clear stats on reset
                shared_state.source_stats.clear();
                let _ = state_tx.send(ComponentState::Idle);
                info!("Merger reset");
                CommandResponse::success(ComponentState::Idle, "Reset to Idle")
            }

            Command::GetStatus => {
                let stats = shared_state.get_stats();
                let run_config = shared_state.run_config.lock();
                let run_number = run_config.as_ref().map(|c| c.run_number);
                let msg = format!(
                    "State: {}, Received: {}, Sent: {}, Dropped: {}, Gaps: {}, Missing: {}",
                    current,
                    stats.received_batches,
                    stats.sent_batches,
                    stats.dropped_batches,
                    stats.total_gaps(),
                    stats.total_missing()
                );

                let metrics = ComponentMetrics {
                    events_processed: stats.received_batches, // batches, not events
                    bytes_transferred: 0,
                    queue_size: 0,
                    queue_max: 0,
                    event_rate: 0.0, // Would need time tracking to calculate
                    data_rate: 0.0,
                };

                let mut resp = CommandResponse::success(current, msg);
                resp.run_number = run_number;
                resp.metrics = Some(metrics);
                resp
            }
        }
    }

    /// Command handler task using tmq REQ/REP pattern
    async fn command_task(
        command_address: String,
        shared_state: Arc<SharedState>,
        state_tx: watch::Sender<ComponentState>,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) {
        let context = Context::new();

        let receiver = match request_reply::reply(&context).bind(&command_address) {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "Failed to bind command socket");
                return;
            }
        };

        info!(address = %command_address, "Merger command task started");

        let mut current_receiver = receiver;

        loop {
            tokio::select! {
                biased;

                _ = shutdown.recv() => {
                    info!("Merger command task received shutdown signal");
                    break;
                }

                recv_result = current_receiver.recv() => {
                    match recv_result {
                        Ok((mut multipart, sender)) => {
                            let response = if let Some(frame) = multipart.pop_front() {
                                match Command::from_json(&frame) {
                                    Ok(cmd) => {
                                        info!(command = %cmd, "Merger received command");
                                        Self::handle_command(&shared_state, &state_tx, cmd)
                                    }
                                    Err(e) => {
                                        warn!(error = %e, "Invalid command");
                                        CommandResponse::error(ComponentState::Idle, format!("Invalid: {}", e))
                                    }
                                }
                            } else {
                                CommandResponse::error(ComponentState::Idle, "Empty message")
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

        info!("Merger command task stopped");
    }

    /// Run the merger
    pub async fn run(
        &mut self,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), MergerError> {
        let (tx, rx) = mpsc::channel::<Message>(self.config.channel_capacity);

        let context = Context::new();

        let first_addr = self
            .config
            .sub_addresses
            .first()
            .ok_or(MergerError::NoUpstreamAddresses)?;

        let sub_socket = subscribe(&context)
            .connect(first_addr)?
            .subscribe(b"")?;

        info!(address = %first_addr, "Merger subscribed to upstream");

        for addr in self.config.sub_addresses.iter().skip(1) {
            sub_socket.get_socket().connect(addr)?;
            info!(address = %addr, "Merger subscribed to upstream");
        }

        let pub_socket = publish(&context).bind(&self.config.pub_address)?;
        info!(address = %self.config.pub_address, "Merger publishing to downstream");

        info!(state = %self.state(), "Merger ready, waiting for commands");

        // Spawn command handler task
        let command_address = self.config.command_address.clone();
        let shared_state = self.shared_state.clone();
        let state_tx = self.state_tx.clone();
        let shutdown_for_cmd = shutdown.resubscribe();

        let cmd_handle = tokio::spawn(async move {
            Self::command_task(command_address, shared_state, state_tx, shutdown_for_cmd).await;
        });

        // Spawn receiver task
        let shutdown_rx = shutdown.resubscribe();
        let shared_state_for_recv = self.shared_state.clone();
        let state_rx_for_recv = self.state_rx.clone();
        let receiver_handle = tokio::spawn(async move {
            Self::receiver_task(sub_socket, tx, shutdown_rx, shared_state_for_recv, state_rx_for_recv).await
        });

        // Spawn sender task
        let shared_state_for_send = self.shared_state.clone();
        let sender_handle = tokio::spawn(async move {
            Self::sender_task(rx, pub_socket, shared_state_for_send).await
        });

        // Wait for shutdown signal
        let _ = shutdown.recv().await;
        info!("Merger received shutdown signal");

        // Wait for tasks to complete
        let _ = receiver_handle.await;
        let _ = sender_handle.await;
        let _ = cmd_handle.await;

        // Log final stats
        let stats = self.shared_state.get_stats();
        info!(
            received = stats.received_batches,
            sent = stats.sent_batches,
            dropped = stats.dropped_batches,
            eos = stats.eos_received,
            gaps = stats.total_gaps(),
            missing = stats.total_missing(),
            "Merger stopped"
        );

        Ok(())
    }

    /// Receiver task: SUB → channel (with sequence tracking)
    async fn receiver_task(
        mut socket: subscribe::Subscribe,
        tx: mpsc::Sender<Message>,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
        shared_state: Arc<SharedState>,
        mut state_rx: watch::Receiver<ComponentState>,
    ) {
        loop {
            let is_running = *state_rx.borrow() == ComponentState::Running;

            tokio::select! {
                biased;

                _ = shutdown.recv() => {
                    info!("Receiver task shutting down");
                    break;
                }

                _ = state_rx.changed() => {
                    let current = *state_rx.borrow();
                    info!(state = %current, "Receiver state changed");
                    continue;
                }

                msg = socket.next(), if is_running => {
                    match msg {
                        Some(Ok(multipart)) => {
                            if let Some(data) = multipart.into_iter().next() {
                                match Message::from_msgpack(&data) {
                                    Ok(message) => {
                                        // Update sequence tracking (needs lock, but less frequent)
                                        match &message {
                                            Message::Data(batch) => {
                                                shared_state.atomic_stats.record_received();
                                                // Update per-source sequence tracking (lock-free per entry)
                                                shared_state.source_stats
                                                    .entry(batch.source_id)
                                                    .or_default()
                                                    .update(batch.sequence_number);
                                            }
                                            Message::EndOfStream { .. } => {
                                                shared_state.atomic_stats.record_eos();
                                            }
                                            Message::Heartbeat(_) => {
                                                // Heartbeats are forwarded but not counted as data
                                                debug!("Received heartbeat");
                                            }
                                        }

                                        // Try to send without blocking
                                        match tx.try_send(message) {
                                            Ok(()) => {
                                                debug!("Receiver forwarded message");
                                            }
                                            Err(mpsc::error::TrySendError::Full(_)) => {
                                                shared_state.atomic_stats.record_drop();
                                                warn!("Channel full, dropped message");
                                            }
                                            Err(mpsc::error::TrySendError::Closed(_)) => {
                                                info!("Channel closed, receiver exiting");
                                                break;
                                            }
                                        }
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
                            info!("SUB socket closed");
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Sender task: channel → PUB
    async fn sender_task(
        mut rx: mpsc::Receiver<Message>,
        mut socket: publish::Publish,
        shared_state: Arc<SharedState>,
    ) {
        while let Some(message) = rx.recv().await {
            match message.to_msgpack() {
                Ok(bytes) => {
                    let msg: tmq::Multipart = vec![tmq::Message::from(bytes.as_slice())].into();
                    match socket.send(msg).await {
                        Ok(()) => {
                            shared_state.atomic_stats.record_sent();
                            debug!("Sender forwarded message");
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to send message");
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Failed to serialize message");
                }
            }
        }

        info!("Sender task completed");
    }

    /// Get current statistics
    pub fn stats(&self) -> MergerStats {
        self.shared_state.get_stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = MergerConfig::default();
        assert_eq!(config.pub_address, "tcp://*:5556");
        assert_eq!(config.channel_capacity, 1000);
    }

    #[test]
    fn source_stats_update() {
        let mut stats = SourceStats::default();

        assert!(!stats.update(0));
        assert_eq!(stats.total_batches, 1);

        assert!(!stats.update(1));
        assert_eq!(stats.total_batches, 2);

        // Gap detection
        assert!(!stats.update(200));
        assert_eq!(stats.total_batches, 3);
        assert_eq!(stats.gaps_detected, 1);
        assert_eq!(stats.total_gap_size, 198);

        // Restart detection
        assert!(stats.update(0));
        assert_eq!(stats.restart_count, 1);
    }

    #[test]
    fn atomic_stats() {
        let stats = AtomicStats::new();
        stats.record_received();
        stats.record_received();
        stats.record_sent();
        stats.record_drop();

        let (recv, sent, drop, eos) = stats.snapshot();
        assert_eq!(recv, 2);
        assert_eq!(sent, 1);
        assert_eq!(drop, 1);
        assert_eq!(eos, 0);
    }
}
