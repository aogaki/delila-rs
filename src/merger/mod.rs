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

use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use thiserror::Error;
use tmq::{publish, subscribe, AsZmqSocket, Context};
use tokio::sync::{mpsc, watch};
use tracing::{info, trace, warn};

use crate::common::{
    handle_command, pub_no_hwm, run_command_task, sub_no_hwm, CommandHandlerExt,
    ComponentSharedState, ComponentState, MessageHeader,
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
    /// Soft backlog watermark in bytes (0 = disabled). See TODO 68.
    pub backlog_soft_limit_bytes: u64,
    /// Hard backlog watermark in bytes (0 = disabled). See TODO 68.
    pub backlog_hard_limit_bytes: u64,
}

impl Default for MergerConfig {
    fn default() -> Self {
        Self {
            sub_addresses: vec!["tcp://localhost:5555".to_string()],
            pub_address: "tcp://*:5556".to_string(),
            command_address: "tcp://*:5570".to_string(),
            backlog_soft_limit_bytes: 4096 * 1024 * 1024,
            backlog_hard_limit_bytes: 0,
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

/// What `SourceStats::update` observed about a batch's sequence number
/// (TODO 58 M18 — the call site must surface `Gap`/`Restart`, they mean
/// batches were lost or a source restarted mid-run).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqEvent {
    /// Sequence advanced normally (or first batch from this source).
    Ok,
    /// `gap` batches are missing between the last and this sequence number.
    Gap(u64),
    /// Sequence number jumped far backwards — source restart.
    Restart,
}

impl SourceStats {
    fn update(&mut self, seq: u64) -> SeqEvent {
        let event = if let Some(last) = self.last_sequence {
            if seq < last.saturating_sub(100) {
                self.restart_count += 1;
                SeqEvent::Restart
            } else {
                let expected = last + 1;
                if seq > expected {
                    let gap = seq - expected;
                    self.gaps_detected += 1;
                    self.total_gap_size += gap;
                    SeqEvent::Gap(gap)
                } else {
                    SeqEvent::Ok
                }
            }
        } else {
            SeqEvent::Ok
        };

        self.last_sequence = Some(seq);
        self.total_batches += 1;
        event
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

    fn clear(&self) {
        self.received_batches.store(0, Ordering::Relaxed);
        self.sent_batches.store(0, Ordering::Relaxed);
        self.dropped_batches.store(0, Ordering::Relaxed);
        self.eos_received.store(0, Ordering::Relaxed);
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
    // Backlog gauge for the receiver→sender unbounded channel (TODO 68).
    // Lifetime counters: NOT cleared by `clear()` — channel contents straddle
    // run boundaries; only the peak is re-based per run (`reset_peak`).
    queue: crate::common::queue_accounting::QueueAccounting,
    /// Soft backlog watermark in bytes (0 = disabled), from config.
    backlog_soft_limit_bytes: u64,
    /// Hard backlog watermark in bytes (0 = disabled), from config.
    backlog_hard_limit_bytes: u64,
    /// Last `backlog_level` logged by the periodic tick (spam control).
    last_logged_level: std::sync::atomic::AtomicU8,
}

impl MergerExtState {
    fn new(backlog_soft_limit_bytes: u64, backlog_hard_limit_bytes: u64) -> Self {
        Self {
            source_stats: DashMap::new(),
            atomic_stats: AtomicStats::new(),
            queue: crate::common::queue_accounting::QueueAccounting::new(),
            backlog_soft_limit_bytes,
            backlog_hard_limit_bytes,
            last_logged_level: std::sync::atomic::AtomicU8::new(0),
        }
    }

    /// Current backlog severity (0/1/2) for the channel gauge.
    fn backlog_level(&self) -> u8 {
        crate::common::queue_accounting::backlog_level(
            self.queue.depth_bytes(),
            self.backlog_soft_limit_bytes,
            self.backlog_hard_limit_bytes,
        )
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

    fn on_start(&mut self, _run_number: u32) -> Result<(), String> {
        self.ext_state.clear();
        self.ext_state.atomic_stats.clear();
        // Per-run peak window; the cumulative gauge itself must survive.
        self.ext_state.queue.reset_peak();
        Ok(())
    }

    fn on_reset(&mut self) -> Result<(), String> {
        self.ext_state.clear();
        self.ext_state.atomic_stats.clear();
        self.ext_state.queue.reset_peak();
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

    fn get_metrics(&self) -> Option<crate::common::ComponentMetrics> {
        let stats = self.ext_state.get_stats();
        Some(crate::common::ComponentMetrics {
            // Merger forwards batches, so we report batch counts
            events_processed: stats.sent_batches,
            bytes_transferred: 0, // Merger doesn't track bytes
            queue_size: self.ext_state.queue.depth_items().min(u32::MAX as u64) as u32,
            queue_max: 0,
            event_rate: 0.0, // Will be calculated in Phase 2
            data_rate: 0.0,
            trigger_loss_count: 0,
            trigger_loss_rate: 0.0,
            channel_counts: None,
            queue_bytes: self.ext_state.queue.depth_bytes(),
            queue_bytes_peak: self.ext_state.queue.peak_bytes(),
            backlog_level: self.ext_state.backlog_level(),
        })
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
        let ext_state = Arc::new(MergerExtState::new(
            config.backlog_soft_limit_bytes,
            config.backlog_hard_limit_bytes,
        ));
        Self {
            config,
            shared_state: Arc::new(tokio::sync::Mutex::new(ComponentSharedState::new())),
            ext_state,
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
        // Pass tmq::Multipart directly to avoid copy in receiver + sender (zero-copy)
        let (tx, rx) = mpsc::unbounded_channel::<tmq::Multipart>();

        let context = Context::new();

        let first_addr = self
            .config
            .sub_addresses
            .first()
            .ok_or(MergerError::NoUpstreamAddresses)?;

        let sub_socket = subscribe(&context).connect(first_addr)?.subscribe(b"")?;
        // Never drop messages — buffer in memory instead (DAQ: no data loss)
        sub_no_hwm(&sub_socket)?;

        info!(address = %first_addr, "Merger subscribed to upstream (RCVHWM=0)");

        for addr in self.config.sub_addresses.iter().skip(1) {
            sub_socket.get_socket().connect(addr)?;
            info!(address = %addr, "Merger subscribed to upstream");
        }

        let pub_socket = publish(&context).bind(&self.config.pub_address)?;
        // Never drop messages — buffer in memory instead (DAQ: no data loss)
        pub_no_hwm(&pub_socket)?;
        info!(address = %self.config.pub_address, "Merger publishing to downstream (SNDHWM=0)");

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

        // Spawn sender task (zero-copy: forwards raw bytes, state-aware)
        let ext_state_for_send = self.ext_state.clone();
        let state_rx_for_send = self.state_rx.clone();
        let shutdown_for_send = shutdown.resubscribe();
        let sender_handle = tokio::spawn(async move {
            Self::sender_task(
                rx,
                pub_socket,
                ext_state_for_send,
                state_rx_for_send,
                shutdown_for_send,
            )
            .await
        });

        // Wait for shutdown, evaluating the backlog watermark every 10 s
        // (TODO 68). Log only on level transitions: rise = warn, fall to 0 =
        // info — at most one line per tick by construction.
        let mut backlog_tick = tokio::time::interval(std::time::Duration::from_secs(10));
        backlog_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = shutdown.recv() => break,
                _ = backlog_tick.tick() => {
                    let ext = &self.ext_state;
                    let level = ext.backlog_level();
                    let last = ext.last_logged_level.swap(level, Ordering::Relaxed);
                    if level > last {
                        warn!(
                            backlog_level = level,
                            queue_bytes = ext.queue.depth_bytes(),
                            queue_items = ext.queue.depth_items(),
                            soft_limit_bytes = ext.backlog_soft_limit_bytes,
                            hard_limit_bytes = ext.backlog_hard_limit_bytes,
                            "Merger backlog watermark exceeded"
                        );
                    } else if level < last && level == 0 {
                        info!(
                            queue_bytes = ext.queue.depth_bytes(),
                            "Merger backlog back below watermarks"
                        );
                    }
                }
            }
        }
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
    ///
    /// IMPORTANT: Always drains ZMQ socket to prevent internal buffer growth.
    /// When not Running, data is discarded immediately.
    async fn receiver_task(
        mut socket: subscribe::Subscribe,
        tx: mpsc::UnboundedSender<tmq::Multipart>,
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

                // Always receive from ZMQ to drain the socket buffer
                // Data is only forwarded when Running, otherwise discarded.
                // EndOfStream is exempt from that discard — see below.
                msg = socket.next() => {
                    match msg {
                        Some(Ok(multipart)) => {
                            // EndOfStream must be forwarded regardless of state.
                            // A real Reader acks Stop BEFORE its decode pipeline
                            // flushes the EOS, so the operator's ordered stop
                            // moves the merger out of Running ~ms before the EOS
                            // arrives; the Stop-tail discard below would then
                            // silently eat the run boundary and every PUB
                            // consumer that finalizes on EOS (root_sink) hangs
                            // in "writing" forever (observed on side3, run 12,
                            // 2026-07-21). EOS is a control marker, not data —
                            // the TODO 58 C3 accepted-loss exception does not
                            // cover it. Stale EOS downstream is safe: Recorder
                            // filters by run_number (C1), root_sink ignores
                            // EOS while idle.
                            let is_eos = multipart
                                .0
                                .front()
                                .is_some_and(|d| Self::frame_is_eos(d));

                            // Not running — discard to prevent ZMQ buffer growth.
                            // Tail batches after Stop land here by design
                            // (accepted loss — see CLAUDE.md データ保全 exception).
                            // Count + log so the loss is observable, never silent
                            // (TODO 58 C3): dropped growing while Running is a bug.
                            if !is_running && !is_eos {
                                let n = ext_state
                                    .atomic_stats
                                    .dropped_batches
                                    .fetch_add(1, Ordering::Relaxed)
                                    + 1;
                                if n == 1 || n.is_multiple_of(1000) {
                                    info!(
                                        discarded_total = n,
                                        "Discarding batch while not Running (expected Stop tail)"
                                    );
                                }
                                continue;
                            }

                            // Lightweight header parsing from the first frame (no copy)
                            if let Some(data) = multipart.0.front() {
                                match MessageHeader::parse(data) {
                                    Some(MessageHeader::Data { source_id, sequence_number }) => {
                                        ext_state.atomic_stats.record_received();
                                        // TODO 58 M18: gaps/restarts were counted but
                                        // never logged — data loss must be visible.
                                        match ext_state.source_stats
                                            .entry(source_id)
                                            .or_default()
                                            .update(sequence_number)
                                        {
                                            SeqEvent::Ok => {}
                                            SeqEvent::Gap(gap) => warn!(
                                                source = source_id,
                                                seq = sequence_number,
                                                missing_batches = gap,
                                                "Sequence gap detected — upstream batches lost"
                                            ),
                                            SeqEvent::Restart => warn!(
                                                source = source_id,
                                                seq = sequence_number,
                                                "Sequence jumped backwards — source restart detected"
                                            ),
                                        }
                                        trace!(source = source_id, seq = sequence_number, "Received data");
                                    }
                                    Some(MessageHeader::EndOfStream { source_id }) => {
                                        ext_state.atomic_stats.record_eos();
                                        info!(
                                            source = source_id,
                                            running = is_running,
                                            "Received EOS — forwarding (run-boundary marker)"
                                        );
                                    }
                                    Some(MessageHeader::Heartbeat { source_id }) => {
                                        trace!(source = source_id, "Received heartbeat");
                                    }
                                    None => {
                                        warn!("Failed to parse message header");
                                        continue;
                                    }
                                }
                            } else {
                                continue;
                            }

                            // Pass original Multipart directly — zero copy.
                            // Gauge tracks physical channel occupancy (data,
                            // EOS and heartbeats alike) — sum before the move,
                            // undo if the send fails (TODO 68).
                            let frame_bytes: u64 =
                                multipart.0.iter().map(|m| m.len() as u64).sum();
                            ext_state.queue.on_enqueue(frame_bytes);
                            if tx.send(multipart).is_err() {
                                ext_state.queue.on_dequeue(frame_bytes);
                                info!("Channel closed, receiver exiting");
                                break;
                            }
                            trace!("Receiver forwarded message");
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

    /// True when a raw frame is an EndOfStream marker. EOS is exempt from the
    /// not-Running Stop-tail discard in BOTH merger tasks (receiver + sender):
    /// silently dropping it orphans every EOS-driven consumer downstream
    /// (root_sink never finalizes its run file — side3 run 12, 2026-07-21).
    fn frame_is_eos(data: &[u8]) -> bool {
        matches!(
            MessageHeader::parse(data),
            Some(MessageHeader::EndOfStream { .. })
        )
    }

    /// Sender task: channel → PUB (zero-copy: direct byte forwarding)
    ///
    /// State-aware: only forwards data when Running.
    /// When not Running, receives from channel but discards (drains stale data).
    /// On transition to Running, explicitly drains any remaining stale data.
    async fn sender_task(
        mut rx: mpsc::UnboundedReceiver<tmq::Multipart>,
        mut socket: publish::Publish,
        ext_state: Arc<MergerExtState>,
        mut state_rx: watch::Receiver<ComponentState>,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) {
        loop {
            let is_running = *state_rx.borrow() == ComponentState::Running;

            tokio::select! {
                biased;

                _ = shutdown.recv() => {
                    info!("Sender task shutting down");
                    break;
                }

                _ = state_rx.changed() => {
                    let current = *state_rx.borrow();
                    info!(state = %current, "Sender state changed");
                    // On transition to Running, drain any stale data from channel
                    if current == ComponentState::Running {
                        let mut drained = 0u64;
                        while let Ok(stale) = rx.try_recv() {
                            // Keep the backlog gauge honest: these items leave
                            // the channel here, outside the normal recv path.
                            ext_state
                                .queue
                                .on_dequeue(stale.0.iter().map(|m| m.len() as u64).sum());
                            drained += 1;
                        }
                        if drained > 0 {
                            info!(drained, "Drained stale data from channel on start");
                        }
                    }
                    continue;
                }

                data = rx.recv() => {
                    match data {
                        Some(multipart) => {
                            // Item left the channel — account regardless of
                            // whether it is forwarded or Stop-tail discarded.
                            ext_state
                                .queue
                                .on_dequeue(multipart.0.iter().map(|m| m.len() as u64).sum());
                            if !is_running {
                                // EndOfStream is exempt from the Stop-tail
                                // discard here too (same reasoning as the
                                // receiver): the run-boundary marker must reach
                                // PUB subscribers even though the merger left
                                // Running before the Reader's EOS flushed.
                                let is_eos = multipart
                                    .0
                                    .front()
                                    .is_some_and(|d| Self::frame_is_eos(d));
                                if !is_eos {
                                    // Accepted Stop-tail loss — count + log, never
                                    // silent (TODO 58 C3; CLAUDE.md exception).
                                    let n = ext_state
                                        .atomic_stats
                                        .dropped_batches
                                        .fetch_add(1, Ordering::Relaxed)
                                        + 1;
                                    if n == 1 || n.is_multiple_of(1000) {
                                        info!(
                                            discarded_total = n,
                                            "Sender discarding batch while not Running (expected Stop tail)"
                                        );
                                    }
                                    continue;
                                }
                                info!("Publishing EOS while not Running (run-boundary marker)");
                            }
                            // True zero-copy: forward original ZMQ Multipart directly
                            match socket.send(multipart).await {
                                Ok(()) => {
                                    ext_state.atomic_stats.record_sent();
                                    trace!("Sender forwarded message");
                                }
                                Err(e) => {
                                    ext_state.atomic_stats.record_drop();
                                    warn!(error = %e, "Failed to send message");
                                }
                            }
                        }
                        None => {
                            info!("Channel closed, sender exiting");
                            break;
                        }
                    }
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

    // EOS must survive the not-Running Stop-tail discard (receiver + sender):
    // a real Reader acks Stop before its pipeline flushes the EOS, so the
    // merger has already left Running when the marker arrives. Data and
    // Heartbeat frames must NOT be exempt — the C3 accepted-loss exception
    // stays in force for them.
    #[test]
    fn eos_frame_exempt_from_stop_tail_discard() {
        use crate::common::{EventDataBatch, Message};

        let eos = Message::eos(3, 42).to_msgpack().unwrap();
        assert!(Merger::frame_is_eos(&eos));

        let hb = Message::heartbeat(3, 7).to_msgpack().unwrap();
        assert!(!Merger::frame_is_eos(&hb));

        let data = Message::Data(EventDataBatch::new(3, 1))
            .to_msgpack()
            .unwrap();
        assert!(!Merger::frame_is_eos(&data));

        assert!(!Merger::frame_is_eos(&[]));
        assert!(!Merger::frame_is_eos(&[0xC0]));
    }

    #[test]
    fn default_config() {
        let config = MergerConfig::default();
        assert_eq!(config.pub_address, "tcp://*:5556");
        assert_eq!(config.command_address, "tcp://*:5570");
    }

    #[test]
    fn custom_config() {
        let config = MergerConfig {
            sub_addresses: vec!["tcp://localhost:7000".to_string()],
            pub_address: "tcp://*:7001".to_string(),
            command_address: "tcp://*:7002".to_string(),
            backlog_soft_limit_bytes: 0,
            backlog_hard_limit_bytes: 0,
        };
        assert_eq!(config.sub_addresses.len(), 1);
    }

    #[test]
    fn source_stats_update() {
        let mut stats = SourceStats::default();

        assert_eq!(stats.update(0), SeqEvent::Ok);
        assert_eq!(stats.total_batches, 1);

        assert_eq!(stats.update(1), SeqEvent::Ok);
        assert_eq!(stats.total_batches, 2);

        // Gap detection
        assert_eq!(stats.update(200), SeqEvent::Gap(198));
        assert_eq!(stats.total_batches, 3);
        assert_eq!(stats.gaps_detected, 1);
        assert_eq!(stats.total_gap_size, 198);

        // Restart detection
        assert_eq!(stats.update(0), SeqEvent::Restart);
        assert_eq!(stats.restart_count, 1);
    }

    #[test]
    fn source_stats_no_gap_sequential() {
        let mut stats = SourceStats::default();
        for i in 0..100 {
            stats.update(i);
        }
        assert_eq!(stats.total_batches, 100);
        assert_eq!(stats.gaps_detected, 0);
        assert_eq!(stats.total_gap_size, 0);
        assert_eq!(stats.restart_count, 0);
    }

    #[test]
    fn source_stats_multiple_gaps() {
        let mut stats = SourceStats::default();
        stats.update(0);
        stats.update(5); // gap of 4
        stats.update(10); // gap of 4
        stats.update(100); // gap of 89
        assert_eq!(stats.gaps_detected, 3);
        assert_eq!(stats.total_gap_size, 4 + 4 + 89);
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

    #[test]
    fn atomic_stats_eos() {
        let stats = AtomicStats::new();
        stats.record_eos();
        stats.record_eos();
        stats.record_eos();

        let (_, _, _, eos) = stats.snapshot();
        assert_eq!(eos, 3);
    }

    #[test]
    fn merger_stats_total_gaps() {
        let mut stats = MergerStats::default();
        stats.sources.insert(
            0,
            SourceStats {
                gaps_detected: 5,
                total_gap_size: 10,
                ..Default::default()
            },
        );
        stats.sources.insert(
            1,
            SourceStats {
                gaps_detected: 3,
                total_gap_size: 7,
                ..Default::default()
            },
        );

        assert_eq!(stats.total_gaps(), 8);
        assert_eq!(stats.total_missing(), 17);
    }

    #[test]
    fn merger_ext_state_new() {
        let state = MergerExtState::new(0, 0);
        let stats = state.get_stats();
        assert_eq!(stats.received_batches, 0);
        assert_eq!(stats.sent_batches, 0);
        assert!(stats.sources.is_empty());
    }

    #[test]
    fn merger_ext_state_clear() {
        let state = MergerExtState::new(0, 0);
        state.source_stats.insert(0, SourceStats::default());
        state.source_stats.insert(1, SourceStats::default());
        assert_eq!(state.source_stats.len(), 2);

        state.clear();
        assert_eq!(state.source_stats.len(), 0);
    }

    #[test]
    fn merger_creation() {
        let config = MergerConfig::default();
        let merger = Merger::new(config);
        assert_eq!(merger.state(), ComponentState::Idle);
    }

    #[test]
    fn merger_stats_empty() {
        let config = MergerConfig::default();
        let merger = Merger::new(config);
        let stats = merger.stats();
        assert_eq!(stats.received_batches, 0);
        assert_eq!(stats.sent_batches, 0);
    }

    #[test]
    fn merger_error_display() {
        let err = MergerError::NoUpstreamAddresses;
        let msg = format!("{}", err);
        assert!(msg.contains("No upstream"));

        let err = MergerError::ChannelSend;
        let msg = format!("{}", err);
        assert!(msg.contains("Channel"));
    }

    #[test]
    fn merger_command_ext_component_name() {
        let ext_state = Arc::new(MergerExtState::new(0, 0));
        let ext = MergerCommandExt { ext_state };
        assert_eq!(ext.component_name(), "Merger");
    }

    #[test]
    fn merger_command_ext_on_reset() {
        let ext_state = Arc::new(MergerExtState::new(0, 0));
        ext_state.source_stats.insert(0, SourceStats::default());

        let mut ext = MergerCommandExt {
            ext_state: ext_state.clone(),
        };
        assert!(ext.on_reset().is_ok());
        assert_eq!(ext_state.source_stats.len(), 0);
    }

    #[test]
    fn merger_command_ext_status_details() {
        let ext_state = Arc::new(MergerExtState::new(0, 0));
        ext_state.atomic_stats.record_received();
        ext_state.atomic_stats.record_sent();

        let ext = MergerCommandExt { ext_state };
        let details = ext.status_details();
        assert!(details.is_some());
        let s = details.unwrap();
        assert!(s.contains("Received: 1"));
        assert!(s.contains("Sent: 1"));
    }
}
