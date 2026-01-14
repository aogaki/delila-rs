//! Merger - receives from multiple upstream sources and forwards downstream
//!
//! Architecture (Zero-Copy):
//! - Receiver task: SUB socket → mpsc channel (raw bytes, header-only parsing)
//! - Sender task: mpsc channel → PUB socket (direct byte forwarding)
//! - Command task: REP socket for control commands
//! - NO serialization/deserialization on the hot path
//!
//! Performance: Uses AtomicU64 for hot-path counters to avoid mutex contention

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use thiserror::Error;
use tmq::{publish, subscribe, AsZmqSocket, Context};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

use crate::common::{
    handle_command, run_command_task, CommandHandlerExt, ComponentSharedState, ComponentState,
    MessageHeader,
};

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
            channel_capacity: 10000,
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

/// Extended state for Merger (statistics and sequence tracking)
struct MergerExtState {
    // Sequence tracking per source (lock-free concurrent map)
    source_stats: DashMap<u32, SourceStats>,
    // Hot-path counters (lock-free)
    atomic_stats: AtomicStats,
}

impl MergerExtState {
    fn new() -> Self {
        Self {
            source_stats: DashMap::new(),
            atomic_stats: AtomicStats::new(),
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

    fn clear(&self) {
        self.source_stats.clear();
    }
}

/// Command handler extension for Merger with custom GetStatus
struct MergerCommandExt {
    ext_state: Arc<MergerExtState>,
}

impl CommandHandlerExt for MergerCommandExt {
    fn component_name(&self) -> &'static str {
        "Merger"
    }

    fn on_reset(&mut self) -> Result<(), String> {
        self.ext_state.clear();
        Ok(())
    }

    fn status_details(&self) -> Option<String> {
        let stats = self.ext_state.get_stats();
        Some(format!(
            "Received: {}, Sent: {}, Dropped: {}, Gaps: {}, Missing: {}",
            stats.received_batches,
            stats.sent_batches,
            stats.dropped_batches,
            stats.total_gaps(),
            stats.total_missing()
        ))
    }
}

/// Merger component
pub struct Merger {
    config: MergerConfig,
    shared_state: Arc<tokio::sync::Mutex<ComponentSharedState>>,
    ext_state: Arc<MergerExtState>,
    state_rx: watch::Receiver<ComponentState>,
    state_tx: watch::Sender<ComponentState>,
}

impl Merger {
    /// Create a new merger with the given configuration
    pub fn new(config: MergerConfig) -> Self {
        let (state_tx, state_rx) = watch::channel(ComponentState::Idle);
        Self {
            config,
            shared_state: Arc::new(tokio::sync::Mutex::new(ComponentSharedState::new())),
            ext_state: Arc::new(MergerExtState::new()),
            state_rx,
            state_tx,
        }
    }

    /// Get current state
    pub fn state(&self) -> ComponentState {
        *self.state_rx.borrow()
    }

    /// Run the merger
    pub async fn run(
        &mut self,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), MergerError> {
        // Use unbounded channel - if memory grows, it indicates downstream bottleneck
        let (tx, rx) = mpsc::unbounded_channel::<Bytes>();

        let context = Context::new();

        let first_addr = self
            .config
            .sub_addresses
            .first()
            .ok_or(MergerError::NoUpstreamAddresses)?;

        let sub_socket = subscribe(&context).connect(first_addr)?.subscribe(b"")?;

        info!(address = %first_addr, "Merger subscribed to upstream");

        for addr in self.config.sub_addresses.iter().skip(1) {
            sub_socket.get_socket().connect(addr)?;
            info!(address = %addr, "Merger subscribed to upstream");
        }

        let pub_socket = publish(&context).bind(&self.config.pub_address)?;
        info!(address = %self.config.pub_address, "Merger publishing to downstream");

        info!(state = %self.state(), "Merger ready, waiting for commands");

        // Spawn command handler task using common infrastructure
        let command_address = self.config.command_address.clone();
        let shared_state = self.shared_state.clone();
        let state_tx = self.state_tx.clone();
        let shutdown_for_cmd = shutdown.resubscribe();
        let ext_state_for_cmd = self.ext_state.clone();

        let cmd_handle = tokio::spawn(async move {
            run_command_task(
                command_address,
                shared_state,
                state_tx,
                shutdown_for_cmd,
                move |state, tx, cmd| {
                    let mut ext = MergerCommandExt {
                        ext_state: ext_state_for_cmd.clone(),
                    };
                    handle_command(state, tx, cmd, Some(&mut ext))
                },
                "Merger",
            )
            .await;
        });

        // Spawn receiver task (zero-copy: passes raw bytes)
        let shutdown_rx = shutdown.resubscribe();
        let ext_state_for_recv = self.ext_state.clone();
        let state_rx_for_recv = self.state_rx.clone();
        let receiver_handle = tokio::spawn(async move {
            Self::receiver_task(
                sub_socket,
                tx,
                shutdown_rx,
                ext_state_for_recv,
                state_rx_for_recv,
            )
            .await
        });

        // Spawn sender task (zero-copy: forwards raw bytes)
        let ext_state_for_send = self.ext_state.clone();
        let sender_handle =
            tokio::spawn(
                async move { Self::sender_task(rx, pub_socket, ext_state_for_send).await },
            );

        // Wait for shutdown signal
        let _ = shutdown.recv().await;
        info!("Merger received shutdown signal");

        // Wait for tasks to complete
        let _ = receiver_handle.await;
        let _ = sender_handle.await;
        let _ = cmd_handle.await;

        // Log final stats
        let stats = self.ext_state.get_stats();
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

    /// Receiver task: SUB → channel (zero-copy with header-only parsing)
    async fn receiver_task(
        mut socket: subscribe::Subscribe,
        tx: mpsc::UnboundedSender<Bytes>,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
        ext_state: Arc<MergerExtState>,
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
                                // Zero-copy: convert to Bytes (reference counted)
                                let raw_bytes: Bytes = Bytes::copy_from_slice(&data);

                                // Lightweight header parsing (no full deserialization)
                                match MessageHeader::parse(&raw_bytes) {
                                    Some(MessageHeader::Data { source_id, sequence_number }) => {
                                        ext_state.atomic_stats.record_received();
                                        // Update per-source sequence tracking
                                        ext_state.source_stats
                                            .entry(source_id)
                                            .or_default()
                                            .update(sequence_number);
                                        debug!(source = source_id, seq = sequence_number, "Received data");
                                    }
                                    Some(MessageHeader::EndOfStream { source_id }) => {
                                        ext_state.atomic_stats.record_eos();
                                        info!(source = source_id, "Received EOS");
                                    }
                                    Some(MessageHeader::Heartbeat { source_id }) => {
                                        debug!(source = source_id, "Received heartbeat");
                                    }
                                    None => {
                                        warn!("Failed to parse message header");
                                        continue;
                                    }
                                }

                                // Send raw bytes (unbounded channel never blocks)
                                if tx.send(raw_bytes).is_err() {
                                    info!("Channel closed, receiver exiting");
                                    break;
                                }
                                debug!("Receiver forwarded message");
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

    /// Sender task: channel → PUB (zero-copy: direct byte forwarding)
    async fn sender_task(
        mut rx: mpsc::UnboundedReceiver<Bytes>,
        mut socket: publish::Publish,
        ext_state: Arc<MergerExtState>,
    ) {
        while let Some(raw_bytes) = rx.recv().await {
            // Zero-copy: directly send raw bytes to ZMQ
            let bytes_slice: &[u8] = raw_bytes.as_ref();
            let msg: tmq::Multipart = vec![tmq::Message::from(bytes_slice)].into();
            match socket.send(msg).await {
                Ok(()) => {
                    ext_state.atomic_stats.record_sent();
                    debug!("Sender forwarded message");
                }
                Err(e) => {
                    warn!(error = %e, "Failed to send message");
                }
            }
        }

        info!("Sender task completed");
    }

    /// Get current statistics
    pub fn stats(&self) -> MergerStats {
        self.ext_state.get_stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = MergerConfig::default();
        assert_eq!(config.pub_address, "tcp://*:5556");
        assert_eq!(config.channel_capacity, 10000);
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
