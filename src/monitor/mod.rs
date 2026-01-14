//! Monitor component - receives event data and provides histogram visualization
//!
//! Architecture (Lock-Free):
//! - Receiver task: ZMQ SUB → mpsc channel (non-blocking)
//! - Histogram task: mpsc channel → histogram update (owns state, no locks in hot path)
//! - Command task: REP socket for control commands
//! - HTTP server: REST API + static files for web UI (reads histogram via channel query)
//!
//! This module provides real-time monitoring of DAQ data with browser-based
//! histogram display.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tmq::{subscribe, Context};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tracing::{debug, info, warn};

use crate::common::{
    handle_command, run_command_task, CommandHandlerExt, ComponentSharedState, ComponentState,
    Message, MinimalEventData, MinimalEventDataBatch,
};

/// Monitor configuration
#[derive(Debug, Clone)]
pub struct MonitorConfig {
    /// ZMQ connect address (e.g., "tcp://localhost:5557")
    pub subscribe_address: String,
    /// ZMQ bind address for commands (e.g., "tcp://*:5590")
    pub command_address: String,
    /// HTTP server port
    pub http_port: u16,
    /// Default histogram configuration
    pub histogram_config: HistogramConfig,
    /// Internal channel capacity
    pub channel_capacity: usize,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            subscribe_address: "tcp://localhost:5557".to_string(),
            command_address: "tcp://*:5590".to_string(),
            http_port: 8081,
            histogram_config: HistogramConfig::default(),
            channel_capacity: 1000,
        }
    }
}

/// Monitor errors
#[derive(Error, Debug)]
pub enum MonitorError {
    #[error("ZMQ error: {0}")]
    Zmq(#[from] tmq::TmqError),

    #[error("Deserialization error: {0}")]
    Deserialization(#[from] rmp_serde::decode::Error),

    #[error("HTTP server error: {0}")]
    Http(String),
}

/// Histogram configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistogramConfig {
    /// Number of bins
    pub num_bins: u32,
    /// Minimum value
    pub min_value: f32,
    /// Maximum value
    pub max_value: f32,
}

impl Default for HistogramConfig {
    fn default() -> Self {
        Self {
            num_bins: 4096,
            min_value: 0.0,
            max_value: 65535.0, // 16-bit ADC max
        }
    }
}

/// 1D Histogram for a single channel
#[derive(Debug, Clone, Serialize)]
pub struct Histogram1D {
    pub module_id: u32,
    pub channel_id: u32,
    pub config: HistogramConfig,
    pub bins: Vec<u64>,
    pub total_counts: u64,
    pub overflow: u64,
    pub underflow: u64,
}

impl Histogram1D {
    /// Create a new histogram with the given configuration
    pub fn new(module_id: u32, channel_id: u32, config: HistogramConfig) -> Self {
        let bins = vec![0u64; config.num_bins as usize];
        Self {
            module_id,
            channel_id,
            config,
            bins,
            total_counts: 0,
            overflow: 0,
            underflow: 0,
        }
    }

    /// Fill the histogram with a value
    pub fn fill(&mut self, value: f32) {
        self.total_counts += 1;

        if value < self.config.min_value {
            self.underflow += 1;
            return;
        }

        if value >= self.config.max_value {
            self.overflow += 1;
            return;
        }

        let range = self.config.max_value - self.config.min_value;
        let bin_width = range / self.config.num_bins as f32;
        let bin = ((value - self.config.min_value) / bin_width) as usize;

        if bin < self.bins.len() {
            self.bins[bin] += 1;
        } else {
            self.overflow += 1;
        }
    }

    /// Clear the histogram
    pub fn clear(&mut self) {
        self.bins.fill(0);
        self.total_counts = 0;
        self.overflow = 0;
        self.underflow = 0;
    }
}

/// Key for identifying a channel histogram
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChannelKey {
    pub module_id: u32,
    pub channel_id: u32,
}

impl ChannelKey {
    pub fn new(module_id: u32, channel_id: u32) -> Self {
        Self {
            module_id,
            channel_id,
        }
    }
}

/// Monitor state containing all histograms (owned by histogram task)
#[derive(Debug, Default)]
pub struct MonitorState {
    pub histograms: HashMap<ChannelKey, Histogram1D>,
    pub total_events: u64,
    pub start_time: Option<Instant>,
    pub histogram_config: HistogramConfig,
}

impl MonitorState {
    pub fn new(config: HistogramConfig) -> Self {
        Self {
            histograms: HashMap::new(),
            total_events: 0,
            start_time: None,
            histogram_config: config,
        }
    }

    /// Process an event and update histograms
    pub fn process_event(&mut self, event: &MinimalEventData) {
        self.total_events += 1;

        let key = ChannelKey::new(event.module as u32, event.channel as u32);

        let histogram = self.histograms.entry(key).or_insert_with(|| {
            Histogram1D::new(
                event.module as u32,
                event.channel as u32,
                self.histogram_config.clone(),
            )
        });

        // Fill with energy (long gate)
        histogram.fill(event.energy as f32);
    }

    /// Process a batch of events
    pub fn process_batch(&mut self, batch: &MinimalEventDataBatch) {
        for event in &batch.events {
            self.process_event(event);
        }
    }

    /// Clear all histograms
    pub fn clear(&mut self) {
        for histogram in self.histograms.values_mut() {
            histogram.clear();
        }
        self.total_events = 0;
    }

    /// Create a snapshot for HTTP responses
    fn snapshot(&self) -> MonitorStateSnapshot {
        let elapsed_secs = self
            .start_time
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0);

        let event_rate = if elapsed_secs > 0.0 {
            self.total_events as f64 / elapsed_secs
        } else {
            0.0
        };

        MonitorStateSnapshot {
            total_events: self.total_events,
            elapsed_secs,
            event_rate,
            histograms: self.histograms.clone(),
        }
    }
}

/// Snapshot of monitor state for HTTP responses
#[derive(Debug, Clone)]
struct MonitorStateSnapshot {
    total_events: u64,
    elapsed_secs: f64,
    event_rate: f64,
    histograms: HashMap<ChannelKey, Histogram1D>,
}

/// Atomic counters for hot-path statistics (lock-free)
struct AtomicStats {
    received_batches: AtomicU64,
    processed_batches: AtomicU64,
    dropped_batches: AtomicU64,
}

impl AtomicStats {
    fn new() -> Self {
        Self {
            received_batches: AtomicU64::new(0),
            processed_batches: AtomicU64::new(0),
            dropped_batches: AtomicU64::new(0),
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

    fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.received_batches.load(Ordering::Relaxed),
            self.processed_batches.load(Ordering::Relaxed),
            self.dropped_batches.load(Ordering::Relaxed),
        )
    }
}

/// Message type for histogram task (commands from HTTP handlers and control)
enum HistogramMessage {
    /// Clear all histograms
    Clear,
    /// Get current state snapshot
    GetSnapshot(oneshot::Sender<MonitorStateSnapshot>),
    /// Get specific histogram
    GetHistogram(ChannelKey, oneshot::Sender<Option<Histogram1D>>),
    /// Set start time
    SetStartTime,
}

/// Shared state for HTTP handlers
#[derive(Clone)]
pub struct AppState {
    /// Channel to send requests to histogram task
    histogram_tx: mpsc::UnboundedSender<HistogramMessage>,
    /// Component state for status
    pub component_state: Arc<tokio::sync::Mutex<ComponentSharedState>>,
}

// =============================================================================
// HTTP API Handlers
// =============================================================================

/// Response for histogram list
#[derive(Serialize)]
struct HistogramListResponse {
    total_events: u64,
    elapsed_secs: f64,
    event_rate: f64,
    channels: Vec<ChannelSummary>,
}

#[derive(Serialize)]
struct ChannelSummary {
    module_id: u32,
    channel_id: u32,
    total_counts: u64,
}

/// GET /api/histograms - List all histograms
async fn list_histograms(State(state): State<AppState>) -> Json<HistogramListResponse> {
    let (tx, rx) = oneshot::channel();
    let _ = state.histogram_tx.send(HistogramMessage::GetSnapshot(tx));

    match rx.await {
        Ok(snapshot) => {
            let mut channels: Vec<ChannelSummary> = snapshot
                .histograms
                .iter()
                .map(|(key, hist)| ChannelSummary {
                    module_id: key.module_id,
                    channel_id: key.channel_id,
                    total_counts: hist.total_counts,
                })
                .collect();

            // Sort by module_id, then channel_id
            channels.sort_by(|a, b| {
                a.module_id
                    .cmp(&b.module_id)
                    .then(a.channel_id.cmp(&b.channel_id))
            });

            Json(HistogramListResponse {
                total_events: snapshot.total_events,
                elapsed_secs: snapshot.elapsed_secs,
                event_rate: snapshot.event_rate,
                channels,
            })
        }
        Err(_) => Json(HistogramListResponse {
            total_events: 0,
            elapsed_secs: 0.0,
            event_rate: 0.0,
            channels: vec![],
        }),
    }
}

/// GET /api/histograms/:module/:channel - Get specific histogram
async fn get_histogram(
    State(state): State<AppState>,
    axum::extract::Path((module_id, channel_id)): axum::extract::Path<(u32, u32)>,
) -> Result<Json<Histogram1D>, StatusCode> {
    let (tx, rx) = oneshot::channel();
    let key = ChannelKey::new(module_id, channel_id);
    let _ = state.histogram_tx.send(HistogramMessage::GetHistogram(key, tx));

    match rx.await {
        Ok(Some(hist)) => Ok(Json(hist)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/histograms/clear - Clear all histograms
async fn clear_histograms(State(state): State<AppState>) -> StatusCode {
    let _ = state.histogram_tx.send(HistogramMessage::Clear);
    info!("Histograms cleared");
    StatusCode::OK
}

/// GET /api/status - Get monitor status
#[derive(Serialize)]
struct StatusResponse {
    state: String,
    total_events: u64,
    num_channels: usize,
    elapsed_secs: f64,
    event_rate: f64,
}

async fn get_status(State(state): State<AppState>) -> Json<StatusResponse> {
    let component = state.component_state.lock().await;
    let component_state = component.state.to_string();
    drop(component);

    let (tx, rx) = oneshot::channel();
    let _ = state.histogram_tx.send(HistogramMessage::GetSnapshot(tx));

    match rx.await {
        Ok(snapshot) => Json(StatusResponse {
            state: component_state,
            total_events: snapshot.total_events,
            num_channels: snapshot.histograms.len(),
            elapsed_secs: snapshot.elapsed_secs,
            event_rate: snapshot.event_rate,
        }),
        Err(_) => Json(StatusResponse {
            state: component_state,
            total_events: 0,
            num_channels: 0,
            elapsed_secs: 0.0,
            event_rate: 0.0,
        }),
    }
}

/// GET / - Serve the web UI
async fn serve_ui() -> impl IntoResponse {
    Html(include_str!("monitor_ui.html"))
}

/// Create the Axum router
pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(serve_ui))
        .route("/api/status", get(get_status))
        .route("/api/histograms", get(list_histograms))
        .route(
            "/api/histograms/:module_id/:channel_id",
            get(get_histogram),
        )
        .route("/api/histograms/clear", axum::routing::post(clear_histograms))
        .with_state(state)
}

// =============================================================================
// Monitor Component
// =============================================================================

/// Command handler extension for Monitor
struct MonitorCommandExt {
    histogram_tx: mpsc::UnboundedSender<HistogramMessage>,
    atomic_stats: Arc<AtomicStats>,
}

impl CommandHandlerExt for MonitorCommandExt {
    fn component_name(&self) -> &'static str {
        "Monitor"
    }

    fn on_start(&mut self) -> Result<(), String> {
        // Set start time when Running begins (unbounded send is synchronous)
        let _ = self.histogram_tx.send(HistogramMessage::SetStartTime);
        Ok(())
    }

    fn on_reset(&mut self) -> Result<(), String> {
        let _ = self.histogram_tx.send(HistogramMessage::Clear);
        Ok(())
    }

    fn status_details(&self) -> Option<String> {
        let (recv, proc, drop) = self.atomic_stats.snapshot();
        Some(format!(
            "Received: {}, Processed: {}, Dropped: {}",
            recv, proc, drop
        ))
    }
}

/// Monitor component
pub struct Monitor {
    config: MonitorConfig,
    shared_state: Arc<tokio::sync::Mutex<ComponentSharedState>>,
    atomic_stats: Arc<AtomicStats>,
    state_rx: watch::Receiver<ComponentState>,
    state_tx: watch::Sender<ComponentState>,
}

impl Monitor {
    /// Create a new monitor
    pub async fn new(config: MonitorConfig) -> Result<Self, MonitorError> {
        let (state_tx, state_rx) = watch::channel(ComponentState::Idle);

        info!(
            subscribe = %config.subscribe_address,
            command = %config.command_address,
            http_port = config.http_port,
            "Monitor created"
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

    /// Run the monitor
    pub async fn run(
        &mut self,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<(), MonitorError> {
        // Create channels (unbounded - memory growth indicates bottleneck)
        let (hist_tx, hist_rx) = mpsc::unbounded_channel::<HistogramMessage>();
        let (data_tx, data_rx) = mpsc::unbounded_channel::<MinimalEventDataBatch>();

        // Create ZMQ SUB socket
        let context = Context::new();
        let socket = subscribe(&context)
            .connect(&self.config.subscribe_address)?
            .subscribe(b"")?;

        info!(
            address = %self.config.subscribe_address,
            "Monitor connected to upstream"
        );

        // Start HTTP server
        let app_state = AppState {
            histogram_tx: hist_tx.clone(),
            component_state: self.shared_state.clone(),
        };
        let router = create_router(app_state);

        let addr = format!("0.0.0.0:{}", self.config.http_port);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| MonitorError::Http(e.to_string()))?;

        info!(address = %addr, "HTTP server started");

        let http_shutdown = shutdown.resubscribe();
        let http_handle = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = http_shutdown.resubscribe().recv().await;
                })
                .await
                .ok();
        });

        // Start command handler
        let command_address = self.config.command_address.clone();
        let shared_state = self.shared_state.clone();
        let state_tx = self.state_tx.clone();
        let shutdown_for_cmd = shutdown.resubscribe();
        let hist_tx_for_cmd = hist_tx.clone();
        let atomic_stats_for_cmd = self.atomic_stats.clone();

        let cmd_handle = tokio::spawn(async move {
            run_command_task(
                command_address,
                shared_state,
                state_tx,
                shutdown_for_cmd,
                move |state, tx, cmd| {
                    let mut ext = MonitorCommandExt {
                        histogram_tx: hist_tx_for_cmd.clone(),
                        atomic_stats: atomic_stats_for_cmd.clone(),
                    };
                    handle_command(state, tx, cmd, Some(&mut ext))
                },
                "Monitor",
            )
            .await;
        });

        // Spawn receiver task
        let shutdown_for_recv = shutdown.resubscribe();
        let atomic_stats_for_recv = self.atomic_stats.clone();
        let state_rx_for_recv = self.state_rx.clone();
        let recv_handle = tokio::spawn(async move {
            Self::receiver_task(socket, data_tx, shutdown_for_recv, atomic_stats_for_recv, state_rx_for_recv).await
        });

        // Spawn histogram task
        let histogram_config = self.config.histogram_config.clone();
        let atomic_stats_for_hist = self.atomic_stats.clone();
        let hist_handle = tokio::spawn(async move {
            Self::histogram_task(hist_rx, data_rx, histogram_config, atomic_stats_for_hist).await
        });

        info!(state = %self.state(), "Monitor ready, waiting for commands");

        // Wait for shutdown signal
        let _ = shutdown.recv().await;
        info!("Monitor received shutdown signal");

        // Wait for tasks to complete
        let _ = recv_handle.await;
        let _ = hist_handle.await;
        let _ = cmd_handle.await;
        let _ = http_handle.await;

        let (recv, proc, drop) = self.atomic_stats.snapshot();
        info!(
            received = recv,
            processed = proc,
            dropped = drop,
            "Monitor stopped"
        );

        Ok(())
    }

    /// Receiver task: ZMQ SUB → channel (non-blocking)
    async fn receiver_task(
        mut socket: subscribe::Subscribe,
        tx: mpsc::UnboundedSender<MinimalEventDataBatch>,
        mut shutdown: broadcast::Receiver<()>,
        atomic_stats: Arc<AtomicStats>,
        mut state_rx: watch::Receiver<ComponentState>,
    ) {
        loop {
            let is_running = *state_rx.borrow() == ComponentState::Running;

            tokio::select! {
                biased;

                _ = shutdown.recv() => {
                    info!("Monitor receiver task shutting down");
                    break;
                }

                _ = state_rx.changed() => {
                    let current = *state_rx.borrow();
                    info!(state = %current, "Monitor receiver state changed");
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

                                        // Non-blocking send to histogram task (unbounded)
                                        if tx.send(batch).is_err() {
                                            info!("Histogram channel closed, exiting");
                                            break;
                                        }
                                    }
                                    Ok(Message::EndOfStream { source_id }) => {
                                        info!(source_id, "Received EOS");
                                    }
                                    Ok(Message::Heartbeat(hb)) => {
                                        debug!(source_id = hb.source_id, "Received heartbeat");
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

    /// Histogram task: owns MonitorState, processes batches and HTTP queries
    async fn histogram_task(
        mut cmd_rx: mpsc::UnboundedReceiver<HistogramMessage>,
        mut data_rx: mpsc::UnboundedReceiver<MinimalEventDataBatch>,
        histogram_config: HistogramConfig,
        atomic_stats: Arc<AtomicStats>,
    ) {
        let mut state = MonitorState::new(histogram_config);

        loop {
            tokio::select! {
                biased;

                // Command messages have priority (for responsiveness)
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(HistogramMessage::Clear) => {
                            state.clear();
                            state.start_time = None;
                            info!("Histograms cleared by command");
                        }
                        Some(HistogramMessage::GetSnapshot(tx)) => {
                            let _ = tx.send(state.snapshot());
                        }
                        Some(HistogramMessage::GetHistogram(key, tx)) => {
                            let _ = tx.send(state.histograms.get(&key).cloned());
                        }
                        Some(HistogramMessage::SetStartTime) => {
                            state.start_time = Some(Instant::now());
                        }
                        None => {
                            info!("Command channel closed");
                            break;
                        }
                    }
                }

                // Data batches
                batch = data_rx.recv() => {
                    match batch {
                        Some(batch) => {
                            state.process_batch(&batch);
                            atomic_stats.record_processed();
                        }
                        None => {
                            info!("Data channel closed");
                            break;
                        }
                    }
                }
            }
        }

        info!(
            total_events = state.total_events,
            num_channels = state.histograms.len(),
            "Histogram task completed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_histogram_config_default() {
        let config = HistogramConfig::default();
        assert_eq!(config.num_bins, 4096);
        assert_eq!(config.min_value, 0.0);
        assert_eq!(config.max_value, 65535.0);
    }

    #[test]
    fn test_histogram_fill() {
        let config = HistogramConfig {
            num_bins: 100,
            min_value: 0.0,
            max_value: 100.0,
        };
        let mut hist = Histogram1D::new(0, 0, config);

        // Fill with values
        hist.fill(50.0); // bin 50
        hist.fill(0.0); // bin 0
        hist.fill(99.9); // bin 99

        assert_eq!(hist.total_counts, 3);
        assert_eq!(hist.bins[50], 1);
        assert_eq!(hist.bins[0], 1);
        assert_eq!(hist.bins[99], 1);
    }

    #[test]
    fn test_histogram_overflow_underflow() {
        let config = HistogramConfig {
            num_bins: 100,
            min_value: 0.0,
            max_value: 100.0,
        };
        let mut hist = Histogram1D::new(0, 0, config);

        hist.fill(-10.0); // underflow
        hist.fill(100.0); // overflow (>= max)
        hist.fill(150.0); // overflow

        assert_eq!(hist.total_counts, 3);
        assert_eq!(hist.underflow, 1);
        assert_eq!(hist.overflow, 2);
    }

    #[test]
    fn test_histogram_clear() {
        let config = HistogramConfig {
            num_bins: 100,
            min_value: 0.0,
            max_value: 100.0,
        };
        let mut hist = Histogram1D::new(0, 0, config);

        hist.fill(50.0);
        hist.fill(60.0);
        assert_eq!(hist.total_counts, 2);

        hist.clear();
        assert_eq!(hist.total_counts, 0);
        assert_eq!(hist.bins[50], 0);
        assert_eq!(hist.bins[60], 0);
    }

    #[test]
    fn test_monitor_state_process_event() {
        let mut state = MonitorState::new(HistogramConfig::default());

        let event = MinimalEventData {
            module: 0,
            channel: 5,
            energy: 1000,
            energy_short: 500,
            timestamp_ns: 0.0,
            flags: 0,
        };

        state.process_event(&event);

        assert_eq!(state.total_events, 1);
        assert_eq!(state.histograms.len(), 1);

        let key = ChannelKey::new(0, 5);
        let hist = state.histograms.get(&key).unwrap();
        assert_eq!(hist.total_counts, 1);
    }

    #[test]
    fn test_atomic_stats() {
        let stats = AtomicStats::new();
        stats.record_received();
        stats.record_received();
        stats.record_processed();
        stats.record_drop();

        let (recv, proc, drop) = stats.snapshot();
        assert_eq!(recv, 2);
        assert_eq!(proc, 1);
        assert_eq!(drop, 1);
    }
}
