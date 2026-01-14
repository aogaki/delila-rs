//! Recorder component - writes event data to files
//!
//! Architecture (Lock-Free Task Separation):
//! - Receiver task: ZMQ SUB → mpsc channel (non-blocking)
//! - Sorter task: Sorts events by timestamp, manages margin buffer
//! - Writer task: mpsc channel → File I/O (handles fsync)
//! - Command task: ZMQ REP socket for control commands
//!
//! File naming: run{XXXX}_{YYYY}_{ExpName}.msgpack
//!   - XXXX: Run number (4 digits, zero-padded)
//!   - YYYY: File sequence within run (4 digits)
//!   - ExpName: Experiment name from RunConfig

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
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
    Message, MinimalEventData, MinimalEventDataBatch, RunConfig,
};

/// Recorder configuration
#[derive(Debug, Clone)]
pub struct RecorderConfig {
    /// ZMQ connect address (e.g., "tcp://localhost:5557")
    pub subscribe_address: String,
    /// ZMQ bind address for commands (e.g., "tcp://*:5580")
    pub command_address: String,
    /// Output directory
    pub output_dir: PathBuf,
    /// Maximum file size in bytes (default: 1GB)
    pub max_file_size: u64,
    /// Maximum file duration in seconds (default: 600 = 10min)
    pub max_file_duration_secs: u64,
    /// Internal channel capacity for receiver → sorter
    pub channel_capacity: usize,
    /// Sorting buffer margin ratio (0.0 - 1.0, default: 0.05 = 5%)
    pub sort_margin_ratio: f64,
    /// Minimum events before flush (default: 10000)
    pub min_events_before_flush: usize,
    /// fsync interval in batches (0 = only at file close, default: 0 for HDD)
    pub fsync_interval_batches: usize,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            subscribe_address: "tcp://localhost:5557".to_string(),
            command_address: "tcp://*:5580".to_string(),
            output_dir: PathBuf::from("./data"),
            max_file_size: 1024 * 1024 * 1024, // 1GB
            max_file_duration_secs: 600,       // 10 minutes
            channel_capacity: 1000,
            sort_margin_ratio: 0.05, // 5% margin
            min_events_before_flush: 10000,
            fsync_interval_batches: 0, // HDD-friendly default
        }
    }
}

/// Recorder errors
#[derive(Error, Debug)]
pub enum RecorderError {
    #[error("ZMQ error: {0}")]
    Zmq(#[from] tmq::TmqError),

    #[error("Deserialization error: {0}")]
    Deserialization(#[from] rmp_serde::decode::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] rmp_serde::encode::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Channel send error")]
    ChannelSend,
}

/// Lock-free statistics for hot path
#[derive(Debug)]
struct AtomicStats {
    received_batches: AtomicU64,
    received_events: AtomicU64,
    written_events: AtomicU64,
    written_bytes: AtomicU64,
    files_written: AtomicU64,
    dropped_batches: AtomicU64,
}

impl AtomicStats {
    fn new() -> Self {
        Self {
            received_batches: AtomicU64::new(0),
            received_events: AtomicU64::new(0),
            written_events: AtomicU64::new(0),
            written_bytes: AtomicU64::new(0),
            files_written: AtomicU64::new(0),
            dropped_batches: AtomicU64::new(0),
        }
    }

    fn snapshot(&self) -> RecorderStats {
        RecorderStats {
            total_events: self.received_events.load(Ordering::Relaxed),
            total_batches: self.received_batches.load(Ordering::Relaxed),
            total_bytes_written: self.written_bytes.load(Ordering::Relaxed),
            files_written: self.files_written.load(Ordering::Relaxed) as u32,
            written_events: self.written_events.load(Ordering::Relaxed),
            dropped_batches: self.dropped_batches.load(Ordering::Relaxed),
        }
    }
}

/// Statistics for current recording session
#[derive(Debug, Default, Clone)]
pub struct RecorderStats {
    pub total_events: u64,
    pub total_batches: u64,
    pub total_bytes_written: u64,
    pub files_written: u32,
    pub written_events: u64,
    pub dropped_batches: u64,
}

/// Sorting buffer with margin strategy
///
/// Holds events and flushes them sorted, keeping a tail margin
/// to handle late-arriving events from different channels.
struct SortingBuffer {
    events: Vec<MinimalEventData>,
    margin_ratio: f64,
    min_buffer_size: usize,
    min_margin_count: usize,
}

impl SortingBuffer {
    fn new(margin_ratio: f64, min_buffer_size: usize) -> Self {
        Self {
            events: Vec::with_capacity(min_buffer_size * 2),
            margin_ratio,
            min_buffer_size,
            min_margin_count: 1000, // At least 1000 events as margin
        }
    }

    /// Add a batch of events to the buffer
    fn add_batch(&mut self, batch: &MinimalEventDataBatch) {
        self.events.reserve(batch.events.len());
        for event in &batch.events {
            self.events.push(*event);
        }
    }

    /// Flush events that are safe to write (keeping tail margin)
    fn flush(&mut self) -> Vec<MinimalEventData> {
        if self.events.len() < self.min_buffer_size {
            return Vec::new();
        }

        // Sort by timestamp (copy to avoid unaligned access on packed struct)
        self.events.sort_by(|a, b| {
            let ts_a = a.timestamp_ns;
            let ts_b = b.timestamp_ns;
            ts_a.partial_cmp(&ts_b).unwrap_or(std::cmp::Ordering::Equal)
        });

        // Calculate margin (tail events to keep)
        let margin_count = (self.events.len() as f64 * self.margin_ratio) as usize;
        let margin_count = margin_count.max(self.min_margin_count);

        // Write count (everything except margin)
        let write_count = self.events.len().saturating_sub(margin_count);
        if write_count == 0 {
            return Vec::new();
        }

        // Drain the write portion, keep the margin
        self.events.drain(..write_count).collect()
    }

    /// Flush all remaining events (for run end)
    fn flush_all(&mut self) -> Vec<MinimalEventData> {
        self.events.sort_by(|a, b| {
            let ts_a = a.timestamp_ns;
            let ts_b = b.timestamp_ns;
            ts_a.partial_cmp(&ts_b).unwrap_or(std::cmp::Ordering::Equal)
        });
        std::mem::take(&mut self.events)
    }

    /// Clear buffer (for reset)
    fn clear(&mut self) {
        self.events.clear();
    }

    /// Current buffer size
    fn len(&self) -> usize {
        self.events.len()
    }
}

/// Message from sorter to writer
enum WriterCommand {
    /// Write these sorted events
    WriteEvents(Vec<MinimalEventData>),
    /// Open a new file for a new run
    NewRun(RunConfig),
    /// Close current file (run stopped)
    CloseFile,
    /// Shutdown writer task
    Shutdown,
}

/// File writer (runs in dedicated task)
struct FileWriter {
    config: RecorderConfig,
    run_config: Option<RunConfig>,
    writer: Option<BufWriter<File>>,
    file_sequence: u32,
    current_file_size: u64,
    current_file_start: Option<Instant>,
    batches_since_fsync: usize,
    stats: Arc<AtomicStats>,
}

impl FileWriter {
    fn new(config: RecorderConfig, stats: Arc<AtomicStats>) -> Self {
        Self {
            config,
            run_config: None,
            writer: None,
            file_sequence: 0,
            current_file_size: 0,
            current_file_start: None,
            batches_since_fsync: 0,
            stats,
        }
    }

    fn generate_filename(&self) -> PathBuf {
        let run_config = self.run_config.as_ref().expect("RunConfig not set");
        let exp_name = if run_config.exp_name.is_empty() {
            "data".to_string()
        } else {
            run_config.exp_name.clone()
        };

        let filename = format!(
            "run{:04}_{:04}_{}.msgpack",
            run_config.run_number, self.file_sequence, exp_name
        );

        self.config.output_dir.join(filename)
    }

    fn open_new_file(&mut self) -> Result<(), RecorderError> {
        self.close_file()?;

        fs::create_dir_all(&self.config.output_dir)?;

        let path = self.generate_filename();
        let file = File::create(&path)?;
        self.writer = Some(BufWriter::with_capacity(64 * 1024, file));

        self.current_file_size = 0;
        self.current_file_start = Some(Instant::now());
        self.batches_since_fsync = 0;

        info!(
            path = %path.display(),
            sequence = self.file_sequence,
            "Opened new data file"
        );

        Ok(())
    }

    fn close_file(&mut self) -> Result<(), RecorderError> {
        if let Some(mut writer) = self.writer.take() {
            writer.flush()?;
            // Final fsync on close
            writer.get_ref().sync_data()?;
            self.stats.files_written.fetch_add(1, Ordering::Relaxed);
            self.file_sequence += 1;

            info!(
                size_mb = self.current_file_size as f64 / 1_000_000.0,
                "Closed data file"
            );
        }
        self.current_file_start = None;
        Ok(())
    }

    fn needs_rotation(&self) -> bool {
        if self.current_file_size >= self.config.max_file_size {
            return true;
        }

        if let Some(start) = self.current_file_start {
            if start.elapsed().as_secs() >= self.config.max_file_duration_secs {
                return true;
            }
        }

        false
    }

    fn write_events(&mut self, events: Vec<MinimalEventData>) -> Result<(), RecorderError> {
        if events.is_empty() {
            return Ok(());
        }

        // Open file if needed
        if self.writer.is_none() {
            self.open_new_file()?;
        }

        // Check for rotation
        if self.needs_rotation() {
            self.open_new_file()?;
        }

        // Create a batch for serialization
        let batch = MinimalEventDataBatch {
            source_id: 0, // Recorder-generated batch
            sequence_number: self.stats.received_batches.load(Ordering::Relaxed),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
            events,
        };

        let event_count = batch.events.len() as u64;
        let data = batch.to_msgpack()?;
        let len_bytes = (data.len() as u32).to_le_bytes();

        if let Some(ref mut writer) = self.writer {
            writer.write_all(&len_bytes)?;
            writer.write_all(&data)?;

            let bytes_written = 4 + data.len() as u64;
            self.current_file_size += bytes_written;
            self.stats
                .written_bytes
                .fetch_add(bytes_written, Ordering::Relaxed);
            self.stats
                .written_events
                .fetch_add(event_count, Ordering::Relaxed);

            // fsync if configured
            self.batches_since_fsync += 1;
            if self.config.fsync_interval_batches > 0
                && self.batches_since_fsync >= self.config.fsync_interval_batches
            {
                writer.flush()?;
                writer.get_ref().sync_data()?;
                self.batches_since_fsync = 0;
                debug!("fsync completed");
            }
        }

        debug!(
            events = event_count,
            file_size_mb = self.current_file_size as f64 / 1_000_000.0,
            "Wrote sorted events"
        );

        Ok(())
    }

    fn new_run(&mut self, run_config: RunConfig) {
        self.run_config = Some(run_config);
        self.file_sequence = 0;
    }

    #[allow(dead_code)] // Reserved for future crash recovery feature
    fn reset(&mut self) -> Result<(), RecorderError> {
        self.close_file()?;
        self.run_config = None;
        self.file_sequence = 0;
        Ok(())
    }
}

/// Command handler extension for Recorder
struct RecorderCommandExt {
    stats: Arc<AtomicStats>,
    writer_tx: mpsc::UnboundedSender<WriterCommand>,
}

impl CommandHandlerExt for RecorderCommandExt {
    fn component_name(&self) -> &'static str {
        "Recorder"
    }

    fn on_configure(&mut self, config: &RunConfig) -> Result<(), String> {
        // Send new run config to writer task (unbounded send is synchronous)
        self.writer_tx
            .send(WriterCommand::NewRun(config.clone()))
            .map_err(|e| format!("Failed to send config to writer: {}", e))
    }

    fn on_start(&mut self) -> Result<(), String> {
        // Writer will open file on first write
        Ok(())
    }

    fn on_stop(&mut self) -> Result<(), String> {
        self.writer_tx
            .send(WriterCommand::CloseFile)
            .map_err(|e| format!("Failed to send close to writer: {}", e))
    }

    fn on_reset(&mut self) -> Result<(), String> {
        self.writer_tx
            .send(WriterCommand::CloseFile)
            .map_err(|e| format!("Failed to send reset to writer: {}", e))
    }

    fn status_details(&self) -> Option<String> {
        let stats = self.stats.snapshot();
        Some(format!(
            "Received: {} events, Written: {} events, Files: {}, Dropped: {}",
            stats.total_events, stats.written_events, stats.files_written, stats.dropped_batches
        ))
    }
}

/// Recorder component
pub struct Recorder {
    config: RecorderConfig,
    shared_state: Arc<tokio::sync::Mutex<ComponentSharedState>>,
    stats: Arc<AtomicStats>,
    state_rx: watch::Receiver<ComponentState>,
    state_tx: watch::Sender<ComponentState>,
}

impl Recorder {
    /// Create a new recorder
    pub async fn new(config: RecorderConfig) -> Result<Self, RecorderError> {
        let (state_tx, state_rx) = watch::channel(ComponentState::Idle);
        let stats = Arc::new(AtomicStats::new());

        info!(
            subscribe = %config.subscribe_address,
            command = %config.command_address,
            output_dir = %config.output_dir.display(),
            max_file_size_mb = config.max_file_size / 1_000_000,
            max_duration_sec = config.max_file_duration_secs,
            fsync_interval = config.fsync_interval_batches,
            sort_margin = format!("{}%", config.sort_margin_ratio * 100.0),
            "Recorder created"
        );

        Ok(Self {
            config,
            shared_state: Arc::new(tokio::sync::Mutex::new(ComponentSharedState::new())),
            stats,
            state_rx,
            state_tx,
        })
    }

    /// Get current state
    pub fn state(&self) -> ComponentState {
        *self.state_rx.borrow()
    }

    /// Get current statistics
    pub fn stats(&self) -> RecorderStats {
        self.stats.snapshot()
    }

    /// Run the recorder
    pub async fn run(
        &mut self,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), RecorderError> {
        // Create channels for task communication (unbounded - memory growth indicates bottleneck)
        // Receiver → Sorter
        let (recv_tx, recv_rx) = mpsc::unbounded_channel::<MinimalEventDataBatch>();
        // Sorter → Writer
        let (writer_tx, writer_rx) = mpsc::unbounded_channel::<WriterCommand>();

        // Create ZMQ SUB socket
        let context = Context::new();
        let socket = subscribe(&context)
            .connect(&self.config.subscribe_address)?
            .subscribe(b"")?;

        info!(
            address = %self.config.subscribe_address,
            "Recorder connected to upstream"
        );

        // === Spawn Writer Task ===
        let writer_config = self.config.clone();
        let writer_stats = self.stats.clone();
        let writer_handle =
            tokio::spawn(
                async move { Self::writer_task(writer_rx, writer_config, writer_stats).await },
            );

        // === Spawn Sorter Task ===
        let sorter_config = self.config.clone();
        let sorter_stats = self.stats.clone();
        let sorter_state_rx = self.state_rx.clone();
        let sorter_writer_tx = writer_tx.clone();
        let sorter_handle = tokio::spawn(async move {
            Self::sorter_task(
                recv_rx,
                sorter_writer_tx,
                sorter_config,
                sorter_stats,
                sorter_state_rx,
            )
            .await
        });

        // === Spawn Receiver Task ===
        // Note: recv_tx is moved into receiver task; sorter will see channel close when receiver exits
        let receiver_stats = self.stats.clone();
        let receiver_state_rx = self.state_rx.clone();
        let receiver_shutdown = shutdown.resubscribe();
        let receiver_handle = tokio::spawn(async move {
            Self::receiver_task(
                socket,
                recv_tx,
                receiver_shutdown,
                receiver_stats,
                receiver_state_rx,
            )
            .await
        });

        // === Spawn Command Task ===
        let command_address = self.config.command_address.clone();
        let shared_state = self.shared_state.clone();
        let state_tx = self.state_tx.clone();
        let shutdown_for_cmd = shutdown.resubscribe();
        let cmd_stats = self.stats.clone();
        let cmd_writer_tx = writer_tx.clone();

        let cmd_handle = tokio::spawn(async move {
            run_command_task(
                command_address,
                shared_state,
                state_tx,
                shutdown_for_cmd,
                move |state, tx, cmd| {
                    let mut ext = RecorderCommandExt {
                        stats: cmd_stats.clone(),
                        writer_tx: cmd_writer_tx.clone(),
                    };
                    handle_command(state, tx, cmd, Some(&mut ext))
                },
                "Recorder",
            )
            .await;
        });

        info!(state = %self.state(), "Recorder ready, waiting for commands");

        // === Stats reporting loop ===
        let mut stats_interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            tokio::select! {
                biased;

                _ = shutdown.recv() => {
                    info!("Recorder received shutdown signal");
                    break;
                }

                _ = stats_interval.tick() => {
                    if *self.state_rx.borrow() == ComponentState::Running {
                        let stats = self.stats.snapshot();
                        info!(
                            received_events = stats.total_events,
                            written_events = stats.written_events,
                            bytes_mb = stats.total_bytes_written as f64 / 1_000_000.0,
                            files = stats.files_written,
                            dropped = stats.dropped_batches,
                            "Recording progress"
                        );
                    }
                }
            }
        }

        // Shutdown tasks
        // Note: recv_tx was moved to receiver task, channel will close when receiver exits
        let _ = writer_tx.send(WriterCommand::Shutdown);

        let _ = receiver_handle.await;
        let _ = sorter_handle.await;
        let _ = writer_handle.await;
        let _ = cmd_handle.await;

        let stats = self.stats.snapshot();
        info!(
            total_events = stats.total_events,
            written_events = stats.written_events,
            total_bytes_mb = stats.total_bytes_written as f64 / 1_000_000.0,
            files_written = stats.files_written,
            dropped = stats.dropped_batches,
            "Recorder stopped"
        );

        Ok(())
    }

    /// Receiver task: ZMQ SUB → channel (non-blocking)
    async fn receiver_task(
        mut socket: subscribe::Subscribe,
        tx: mpsc::UnboundedSender<MinimalEventDataBatch>,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
        stats: Arc<AtomicStats>,
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
                    debug!(state = %current, "Receiver state changed");
                    continue;
                }

                msg = socket.next(), if is_running => {
                    match msg {
                        Some(Ok(multipart)) => {
                            if let Some(data) = multipart.into_iter().next() {
                                match Message::from_msgpack(&data) {
                                    Ok(Message::Data(batch)) => {
                                        stats.received_batches.fetch_add(1, Ordering::Relaxed);
                                        stats.received_events.fetch_add(batch.events.len() as u64, Ordering::Relaxed);

                                        // Send to sorter (unbounded channel never blocks)
                                        if tx.send(batch).is_err() {
                                            info!("Channel closed, receiver exiting");
                                            break;
                                        }
                                        debug!("Forwarded batch to sorter");
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

    /// Sorter task: Sorts events and forwards to writer
    async fn sorter_task(
        mut rx: mpsc::UnboundedReceiver<MinimalEventDataBatch>,
        writer_tx: mpsc::UnboundedSender<WriterCommand>,
        config: RecorderConfig,
        _stats: Arc<AtomicStats>,
        mut state_rx: watch::Receiver<ComponentState>,
    ) {
        let mut buffer =
            SortingBuffer::new(config.sort_margin_ratio, config.min_events_before_flush);
        let mut flush_interval = tokio::time::interval(Duration::from_millis(500));

        loop {
            tokio::select! {
                biased;

                batch = rx.recv() => {
                    match batch {
                        Some(batch) => {
                            buffer.add_batch(&batch);

                            // Try to flush if buffer is large enough
                            let events = buffer.flush();
                            if !events.is_empty()
                                && writer_tx.send(WriterCommand::WriteEvents(events)).is_err()
                            {
                                warn!("Writer channel closed");
                                break;
                            }
                        }
                        None => {
                            // Channel closed, flush remaining and exit
                            info!("Sorter: input channel closed, flushing remaining");
                            let events = buffer.flush_all();
                            if !events.is_empty() {
                                let _ = writer_tx.send(WriterCommand::WriteEvents(events));
                            }
                            break;
                        }
                    }
                }

                _ = flush_interval.tick() => {
                    // Periodic flush check
                    if *state_rx.borrow() == ComponentState::Running && buffer.len() > 0 {
                        let events = buffer.flush();
                        if !events.is_empty()
                            && writer_tx.send(WriterCommand::WriteEvents(events)).is_err()
                        {
                            warn!("Writer channel closed during periodic flush");
                            break;
                        }
                    }
                }

                _ = state_rx.changed() => {
                    let current = *state_rx.borrow();
                    debug!(state = %current, "Sorter state changed");

                    // On stop, flush all remaining events
                    if current == ComponentState::Configured || current == ComponentState::Idle {
                        let events = buffer.flush_all();
                        if !events.is_empty() {
                            let _ = writer_tx.send(WriterCommand::WriteEvents(events));
                        }
                        buffer.clear();
                    }
                }
            }
        }

        info!("Sorter task completed");
    }

    /// Writer task: Handles file I/O
    async fn writer_task(
        mut rx: mpsc::UnboundedReceiver<WriterCommand>,
        config: RecorderConfig,
        stats: Arc<AtomicStats>,
    ) {
        let mut writer = FileWriter::new(config, stats);

        while let Some(cmd) = rx.recv().await {
            match cmd {
                WriterCommand::WriteEvents(events) => {
                    if let Err(e) = writer.write_events(events) {
                        warn!(error = %e, "Failed to write events");
                    }
                }
                WriterCommand::NewRun(run_config) => {
                    writer.new_run(run_config);
                    info!("Writer configured for new run");
                }
                WriterCommand::CloseFile => {
                    if let Err(e) = writer.close_file() {
                        warn!(error = %e, "Failed to close file");
                    }
                }
                WriterCommand::Shutdown => {
                    if let Err(e) = writer.close_file() {
                        warn!(error = %e, "Failed to close file on shutdown");
                    }
                    break;
                }
            }
        }

        info!("Writer task completed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RecorderConfig::default();
        assert_eq!(config.max_file_size, 1024 * 1024 * 1024);
        assert_eq!(config.max_file_duration_secs, 600);
        assert_eq!(config.fsync_interval_batches, 0); // HDD-friendly default
        assert!((config.sort_margin_ratio - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn test_sorting_buffer_basic() {
        let mut buffer = SortingBuffer::new(0.05, 100);

        // Add events out of order
        let mut batch = MinimalEventDataBatch::new(1, 1);
        batch.push(MinimalEventData::new(0, 0, 100, 80, 3000.0, 0));
        batch.push(MinimalEventData::new(0, 1, 200, 160, 1000.0, 0));
        batch.push(MinimalEventData::new(0, 2, 300, 240, 2000.0, 0));

        buffer.add_batch(&batch);
        assert_eq!(buffer.len(), 3);
    }

    #[test]
    fn test_sorting_buffer_flush_all() {
        let mut buffer = SortingBuffer::new(0.05, 100);

        let mut batch = MinimalEventDataBatch::new(1, 1);
        batch.push(MinimalEventData::new(0, 0, 100, 80, 3000.0, 0));
        batch.push(MinimalEventData::new(0, 1, 200, 160, 1000.0, 0));
        batch.push(MinimalEventData::new(0, 2, 300, 240, 2000.0, 0));

        buffer.add_batch(&batch);

        let events = buffer.flush_all();
        assert_eq!(events.len(), 3);

        // Verify sorted order
        assert!((events[0].timestamp_ns - 1000.0).abs() < f64::EPSILON);
        assert!((events[1].timestamp_ns - 2000.0).abs() < f64::EPSILON);
        assert!((events[2].timestamp_ns - 3000.0).abs() < f64::EPSILON);

        // Buffer should be empty
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn test_sorting_buffer_margin() {
        // Create buffer with small min_margin_count for testing
        let mut buffer = SortingBuffer {
            events: Vec::with_capacity(200),
            margin_ratio: 0.20, // 20% margin
            min_buffer_size: 10,
            min_margin_count: 10, // Small for test
        };

        // Add 100 events
        let mut batch = MinimalEventDataBatch::new(1, 1);
        for i in 0..100 {
            batch.push(MinimalEventData::new(0, 0, 100, 80, i as f64 * 10.0, 0));
        }
        buffer.add_batch(&batch);

        // Flush should keep ~20 events as margin (20% of 100)
        let events = buffer.flush();

        // Should write 80 events (100 - 20% margin)
        assert_eq!(events.len(), 80);

        // Buffer should still have 20 margin events
        assert_eq!(buffer.len(), 20);
    }

    #[test]
    fn test_filename_generation() {
        let config = RecorderConfig {
            output_dir: PathBuf::from("/data"),
            ..Default::default()
        };
        let stats = Arc::new(AtomicStats::new());
        let mut writer = FileWriter::new(config, stats);
        writer.new_run(RunConfig {
            run_number: 42,
            exp_name: "CRIB2026".to_string(),
            ..Default::default()
        });

        let path = writer.generate_filename();
        assert_eq!(
            path.to_str().unwrap(),
            "/data/run0042_0000_CRIB2026.msgpack"
        );

        writer.file_sequence = 5;
        let path = writer.generate_filename();
        assert_eq!(
            path.to_str().unwrap(),
            "/data/run0042_0005_CRIB2026.msgpack"
        );
    }
}
