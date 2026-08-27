//! Recorder component - writes raw event data to files
//!
//! Architecture (Lock-Free Task Separation):
//! - Receiver task: ZMQ SUB → mpsc channel (non-blocking)
//! - Writer task: mpsc channel → File I/O
//! - Command task: ZMQ REP socket for control commands
//!
//! Note: This is a Raw Data Recorder - data is written unsorted.
//! Sorting will be performed by the future Online Event Builder component.
//!
//! File naming: run{XXXX}_{YYYY}_{ExpName}.delila
//!   - XXXX: Run number (4 digits, zero-padded)
//!   - YYYY: File sequence within run (4 digits)
//!   - ExpName: Experiment name from RunConfig
//!
//! File format (v2):
//! - Header: Magic "DELILA02" + length (4 bytes) + MsgPack metadata
//! - Data blocks: length (4 bytes LE) + MsgPack batch (repeated)
//! - Footer: Fixed 64 bytes with magic "DLEND002", checksums, completion flag

mod format;

pub use format::{
    ChecksumCalculator, DataBlockIterator, DataFileReader, FileFooter, FileFormatError, FileHeader,
    FileValidationResult, FOOTER_SIZE, FORMAT_VERSION,
};

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use thiserror::Error;
use tmq::{subscribe, Context};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::common::{
    handle_command, run_command_task, sub_no_hwm, CommandHandlerExt, ComponentSharedState,
    ComponentState, EventDataBatch, Message, MessageHeader, RunConfig,
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
    /// Soft backlog watermark in bytes (0 = disabled). See TODO 68.
    pub backlog_soft_limit_bytes: u64,
    /// Hard backlog watermark in bytes (0 = disabled). See TODO 68.
    pub backlog_hard_limit_bytes: u64,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            subscribe_address: "tcp://localhost:5557".to_string(),
            command_address: "tcp://*:5580".to_string(),
            output_dir: PathBuf::from("./data"),
            max_file_size: 1024 * 1024 * 1024, // 1GB
            max_file_duration_secs: 600,       // 10 minutes
            backlog_soft_limit_bytes: 4096 * 1024 * 1024,
            backlog_hard_limit_bytes: 0,
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
    /// Latched by the writer task when a file write/close fails (disk full,
    /// I/O error). Surfaces as ComponentState::Error via effective_state so
    /// the operator SEES the failure instead of the run silently discarding
    /// the rest of its data (TODO 58 H7). Cleared on Configure/Reset.
    write_failed: AtomicBool,
    /// Backlog gauge for the receiver→writer channel (TODO 68). Lifetime
    /// counters — deliberately NOT touched by `reset()`: channel contents can
    /// straddle run boundaries, and clearing while items sit in the channel
    /// corrupts the derived depth. Only `queue.reset_peak()` runs per-run.
    queue: crate::common::queue_accounting::QueueAccounting,
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
            write_failed: AtomicBool::new(false),
            queue: crate::common::queue_accounting::QueueAccounting::new(),
        }
    }

    fn reset(&self) {
        self.received_batches.store(0, Ordering::Relaxed);
        self.received_events.store(0, Ordering::Relaxed);
        self.written_events.store(0, Ordering::Relaxed);
        self.written_bytes.store(0, Ordering::Relaxed);
        self.files_written.store(0, Ordering::Relaxed);
        self.dropped_batches.store(0, Ordering::Relaxed);
        // `queue` intentionally NOT reset — see the field doc. Per-run peak:
        self.queue.reset_peak();
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

/// Rate tracker for 1-second interval rate calculation
#[derive(Debug)]
struct RateTracker {
    prev_events: AtomicU64,
    prev_time: std::sync::Mutex<Option<Instant>>,
    current_rate: AtomicU64,
}

impl RateTracker {
    fn new() -> Self {
        Self {
            prev_events: AtomicU64::new(0),
            prev_time: std::sync::Mutex::new(None),
            current_rate: AtomicU64::new(0),
        }
    }

    /// Update rate calculation. Call this periodically (e.g., every second).
    fn update(&self, current_events: u64) {
        let now = Instant::now();
        let mut prev_time_guard = self.prev_time.lock().unwrap();

        if let Some(prev_time) = *prev_time_guard {
            let elapsed = now.duration_since(prev_time).as_secs_f64();
            if elapsed >= 1.0 {
                let prev_events = self.prev_events.load(Ordering::Relaxed);
                let delta = current_events.saturating_sub(prev_events);
                let rate = (delta as f64 / elapsed) as u64;
                self.current_rate.store(rate, Ordering::Relaxed);
                self.prev_events.store(current_events, Ordering::Relaxed);
                *prev_time_guard = Some(now);
            }
        } else {
            // First call - initialize
            self.prev_events.store(current_events, Ordering::Relaxed);
            *prev_time_guard = Some(now);
        }
    }

    fn get_rate(&self) -> f64 {
        self.current_rate.load(Ordering::Relaxed) as f64
    }

    fn reset(&self) {
        self.prev_events.store(0, Ordering::Relaxed);
        self.current_rate.store(0, Ordering::Relaxed);
        *self.prev_time.lock().unwrap() = None;
    }
}

/// Commands for writer task
///
/// Backlog accounting (TODO 68) covers `WriteRawBatch` only: the control
/// variants carry no payload bytes and are transient, so accounting them
/// would add touch points for zero information. The drain-wait condition is
/// about data bytes.
enum WriterCommand {
    /// Write a batch as pre-serialized MsgPack bytes (zero-copy hot path)
    WriteRawBatch {
        /// Raw MsgPack bytes of EventDataBatch (ready to write to file)
        data: Vec<u8>,
        /// Number of events in this batch (extracted from header)
        event_count: u64,
    },
    /// End of stream - close current file
    EndOfStream { source_id: u32, run_number: u32 },
    /// Configure for a new run
    NewRun(RunConfig),
    /// Drain pending batches and start recording with the run number
    /// This ensures no stale data from previous runs remains in the channel
    DrainAndStart { run_number: u32 },
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
    stats: Arc<AtomicStats>,
    /// Checksum calculator for current file
    checksum: ChecksumCalculator,
    /// Footer accumulating statistics for current file
    footer: FileFooter,
    /// Header size for current file (needed for data_bytes calculation)
    header_size: u64,
    /// Whether we have an active run (file can be opened)
    run_active: bool,
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
            stats,
            checksum: ChecksumCalculator::new(),
            footer: FileFooter::new(),
            header_size: 0,
            run_active: false,
        }
    }

    fn generate_filename(&self) -> PathBuf {
        let run_config = self.run_config.as_ref().expect("RunConfig not set");
        let exp_name = if run_config.exp_name.is_empty() {
            "data".to_string()
        } else {
            run_config.exp_name.clone()
        };

        // Generate base filename
        let base_filename = format!(
            "run{:04}_{:04}_{}.delila",
            run_config.run_number, self.file_sequence, exp_name
        );
        let base_path = self.config.output_dir.join(&base_filename);

        // If file doesn't exist, use base filename
        if !base_path.exists() {
            return base_path;
        }

        // File exists - append Unix timestamp (nanoseconds) to avoid overwriting
        let timestamp_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();

        let filename_with_ts = format!(
            "run{:04}_{:04}_{}_{}.delila",
            run_config.run_number, self.file_sequence, exp_name, timestamp_ns
        );

        warn!(
            existing = %base_path.display(),
            new = %filename_with_ts,
            "File already exists, using timestamped filename"
        );

        self.config.output_dir.join(filename_with_ts)
    }

    fn open_new_file(&mut self) -> Result<(), RecorderError> {
        self.close_file()?;

        fs::create_dir_all(&self.config.output_dir)?;

        let path = self.generate_filename();
        let file = File::create(&path)?;
        let mut writer = BufWriter::with_capacity(64 * 1024, file);

        // Reset checksum and footer for new file
        self.checksum.reset();
        self.footer = FileFooter::new();

        // Create and write header
        let run_config = self.run_config.as_ref().expect("RunConfig not set");
        let mut header = FileHeader::new(
            run_config.run_number,
            run_config.exp_name.clone(),
            self.file_sequence,
        );
        header.comment = run_config.comment.clone();
        // Self-describing event schema (format v3+) so external readers such as
        // the C++ `TDelila` learn the exact wire layout from the file itself.
        header.metadata.insert(
            "event_schema".to_string(),
            crate::common::delila_schema::schema_json(),
        );

        let header_bytes = header
            .to_bytes()
            .map_err(|e| RecorderError::Io(std::io::Error::other(e.to_string())))?;
        writer.write_all(&header_bytes)?;

        self.header_size = header_bytes.len() as u64;
        self.current_file_size = self.header_size;
        self.current_file_start = Some(Instant::now());

        self.writer = Some(writer);

        info!(
            path = %path.display(),
            sequence = self.file_sequence,
            header_size = self.header_size,
            "Opened new data file"
        );

        Ok(())
    }

    fn close_file(&mut self) -> Result<(), RecorderError> {
        if let Some(mut writer) = self.writer.take() {
            // Finalize and write footer
            self.footer.data_checksum = self.checksum.finalize();
            self.footer.data_bytes = self.checksum.bytes_processed();
            self.footer.finalize();

            let footer_bytes = self.footer.to_bytes();
            writer.write_all(&footer_bytes)?;

            writer.flush()?;
            // Final fsync on close
            writer.get_ref().sync_data()?;
            self.stats.files_written.fetch_add(1, Ordering::Relaxed);
            self.file_sequence += 1;

            info!(
                size_mb = (self.current_file_size + FOOTER_SIZE as u64) as f64 / 1_000_000.0,
                events = self.footer.total_events,
                checksum = format!("{:016x}", self.footer.data_checksum),
                "Closed data file"
            );
        }
        self.current_file_start = None;
        Ok(())
    }

    fn needs_rotation(&self) -> bool {
        // Account for footer size in rotation check
        if self.current_file_size + FOOTER_SIZE as u64 >= self.config.max_file_size {
            return true;
        }

        if let Some(start) = self.current_file_start {
            if start.elapsed().as_secs() >= self.config.max_file_duration_secs {
                return true;
            }
        }

        false
    }

    /// Write pre-serialized MsgPack bytes directly to file (zero-copy hot path).
    /// The `data` is already a valid EventDataBatch MsgPack — no re-serialization needed.
    fn write_raw_batch(&mut self, data: &[u8], event_count: u64) -> Result<(), RecorderError> {
        if event_count == 0 {
            return Ok(());
        }

        // Don't write if run is not active
        if !self.run_active {
            debug!("Ignoring write_raw_batch: run not active");
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

        let len_bytes = (data.len() as u32).to_le_bytes();

        if let Some(ref mut writer) = self.writer {
            writer.write_all(&len_bytes)?;
            writer.write_all(data)?;

            // Update checksum with data block (length prefix + data)
            self.checksum.update(&len_bytes);
            self.checksum.update(data);

            let bytes_written = 4 + data.len() as u64;
            self.current_file_size += bytes_written;
            self.footer.total_events += event_count;

            // Extract timestamp range from batch for footer (writer thread, cheap vs I/O)
            if let Ok(batch) = EventDataBatch::from_msgpack(data) {
                if !batch.events.is_empty() {
                    let mut min_ts = f64::MAX;
                    let mut max_ts = f64::MIN;
                    for ev in &batch.events {
                        if ev.timestamp_ns < min_ts {
                            min_ts = ev.timestamp_ns;
                        }
                        if ev.timestamp_ns > max_ts {
                            max_ts = ev.timestamp_ns;
                        }
                    }
                    self.footer.update_timestamp_range(min_ts, max_ts);
                }
            }

            self.stats
                .written_bytes
                .fetch_add(bytes_written, Ordering::Relaxed);
            self.stats
                .written_events
                .fetch_add(event_count, Ordering::Relaxed);
        }

        debug!(
            events = event_count,
            file_size_mb = self.current_file_size as f64 / 1_000_000.0,
            "Wrote batch"
        );

        Ok(())
    }

    fn new_run(&mut self, run_config: RunConfig) {
        self.run_config = Some(run_config);
        // Note: file state reset is done in start_run()
    }

    fn start_run(&mut self, run_number: u32) {
        // Close any leftover file from previous run
        if self.writer.is_some() {
            if let Err(e) = self.close_file() {
                warn!(error = %e, "Failed to close leftover file on start");
            }
        }

        // Update run_number in run_config (this is the key change for timer-based starts)
        if let Some(ref mut cfg) = self.run_config {
            cfg.run_number = run_number;
        }

        // Reset file state for new run
        self.file_sequence = 0;
        self.current_file_size = 0;
        self.current_file_start = None;
        self.checksum = ChecksumCalculator::new();
        self.footer = FileFooter::new();
        self.header_size = 0;

        self.run_active = true;
    }

    fn end_run(&mut self) -> Result<(), RecorderError> {
        self.run_active = false;
        self.close_file()
    }
}

/// Command handler extension for Recorder
struct RecorderCommandExt {
    stats: Arc<AtomicStats>,
    rate_tracker: Arc<RateTracker>,
    writer_tx: std::sync::mpsc::Sender<WriterCommand>,
    /// Backlog watermarks in bytes (0 = disabled), from config (TODO 68).
    backlog_soft_limit_bytes: u64,
    backlog_hard_limit_bytes: u64,
}

impl CommandHandlerExt for RecorderCommandExt {
    fn component_name(&self) -> &'static str {
        "Recorder"
    }

    fn on_configure(&mut self, config: &RunConfig) -> Result<(), String> {
        // Send new run config to writer task
        self.writer_tx
            .send(WriterCommand::NewRun(config.clone()))
            .map_err(|e| format!("Failed to send config to writer: {}", e))
    }

    fn on_start(&mut self, run_number: u32) -> Result<(), String> {
        // Reset statistics and rate tracker for the new run
        self.stats.reset();
        self.rate_tracker.reset();

        // Drain any stale data from previous run and start recording
        self.writer_tx
            .send(WriterCommand::DrainAndStart { run_number })
            .map_err(|e| format!("Failed to send start to writer: {}", e))
    }

    fn on_stop(&mut self) -> Result<(), String> {
        // File close is handled by EOS or state change in writer task
        Ok(())
    }

    fn on_reset(&mut self) -> Result<(), String> {
        self.writer_tx
            .send(WriterCommand::CloseFile)
            .map_err(|e| format!("Failed to send reset to writer: {}", e))
    }

    fn status_details(&self) -> Option<String> {
        let stats = self.stats.snapshot();
        let failed = self.stats.write_failed.load(Ordering::Relaxed);
        Some(format!(
            "Received: {} events, Written: {} events, Files: {}, Dropped: {}{}",
            stats.total_events,
            stats.written_events,
            stats.files_written,
            stats.dropped_batches,
            if failed { ", WRITE FAILED" } else { "" }
        ))
    }

    /// Surface a latched file-write failure as Error so the operator sees it
    /// (TODO 58 H7). Mirrors the Reader's hw_state pattern — the software
    /// state machine says Running, but the component knows better.
    fn effective_state(&self, software_state: ComponentState) -> ComponentState {
        if self.stats.write_failed.load(Ordering::Relaxed) {
            ComponentState::Error
        } else {
            software_state
        }
    }

    fn get_metrics(&self) -> Option<crate::common::ComponentMetrics> {
        let stats = self.stats.snapshot();
        // Update rate tracker with current event count
        self.rate_tracker.update(stats.written_events);
        Some(crate::common::ComponentMetrics {
            events_processed: stats.written_events,
            bytes_transferred: stats.total_bytes_written,
            queue_size: self.stats.queue.depth_items().min(u32::MAX as u64) as u32,
            queue_max: 0,
            event_rate: self.rate_tracker.get_rate(),
            data_rate: 0.0,
            trigger_loss_count: 0,
            trigger_loss_rate: 0.0,
            channel_counts: None,
            queue_bytes: self.stats.queue.depth_bytes(),
            queue_bytes_peak: self.stats.queue.peak_bytes(),
            backlog_level: crate::common::queue_accounting::backlog_level(
                self.stats.queue.depth_bytes(),
                self.backlog_soft_limit_bytes,
                self.backlog_hard_limit_bytes,
            ),
        })
    }
}

/// Recorder component
pub struct Recorder {
    config: RecorderConfig,
    shared_state: Arc<tokio::sync::Mutex<ComponentSharedState>>,
    stats: Arc<AtomicStats>,
    rate_tracker: Arc<RateTracker>,
    state_rx: watch::Receiver<ComponentState>,
    state_tx: watch::Sender<ComponentState>,
}

impl Recorder {
    /// Create a new recorder
    pub async fn new(config: RecorderConfig) -> Result<Self, RecorderError> {
        let (state_tx, state_rx) = watch::channel(ComponentState::Idle);
        let stats = Arc::new(AtomicStats::new());
        let rate_tracker = Arc::new(RateTracker::new());

        info!(
            subscribe = %config.subscribe_address,
            command = %config.command_address,
            output_dir = %config.output_dir.display(),
            max_file_size_mb = config.max_file_size / 1_000_000,
            max_duration_sec = config.max_file_duration_secs,
            "Recorder created (raw data mode)"
        );

        Ok(Self {
            config,
            shared_state: Arc::new(tokio::sync::Mutex::new(ComponentSharedState::new())),
            stats,
            rate_tracker,
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
        // Create channel: Receiver → Writer (std::sync::mpsc for dedicated writer thread)
        let (writer_tx, writer_rx) = std::sync::mpsc::channel::<WriterCommand>();

        // Create ZMQ SUB socket
        let context = Context::new();
        let socket = subscribe(&context)
            .connect(&self.config.subscribe_address)?
            .subscribe(b"")?;
        // Never drop messages — buffer in memory instead (DAQ: no data loss)
        sub_no_hwm(&socket).map_err(tmq::TmqError::from)?;

        info!(
            address = %self.config.subscribe_address,
            "Recorder connected to upstream (RCVHWM=0)"
        );

        // === Spawn Writer Thread (dedicated OS thread for blocking file I/O) ===
        let writer_config = self.config.clone();
        let writer_stats = self.stats.clone();
        let writer_state_rx = self.state_rx.clone();
        let writer_handle = std::thread::Builder::new()
            .name("recorder-writer".to_string())
            .spawn(move || {
                Self::writer_task_blocking(writer_rx, writer_config, writer_stats, writer_state_rx)
            })
            .expect("Failed to spawn recorder-writer thread");

        // === Spawn Receiver Task ===
        let receiver_stats = self.stats.clone();
        let receiver_state_rx = self.state_rx.clone();
        let receiver_shutdown = shutdown.resubscribe();
        let receiver_writer_tx = writer_tx.clone();
        let receiver_handle = tokio::spawn(async move {
            Self::receiver_task(
                socket,
                receiver_writer_tx,
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
        let cmd_rate_tracker = self.rate_tracker.clone();
        let cmd_writer_tx = writer_tx.clone();
        let cmd_backlog_soft = self.config.backlog_soft_limit_bytes;
        let cmd_backlog_hard = self.config.backlog_hard_limit_bytes;

        let cmd_handle = tokio::spawn(async move {
            run_command_task(
                command_address,
                shared_state,
                state_tx,
                shutdown_for_cmd,
                move |state, tx, cmd| {
                    let mut ext = RecorderCommandExt {
                        stats: cmd_stats.clone(),
                        rate_tracker: cmd_rate_tracker.clone(),
                        writer_tx: cmd_writer_tx.clone(),
                        backlog_soft_limit_bytes: cmd_backlog_soft,
                        backlog_hard_limit_bytes: cmd_backlog_hard,
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
        let mut last_logged_level: u8 = 0;
        loop {
            tokio::select! {
                biased;

                _ = shutdown.recv() => {
                    info!("Recorder received shutdown signal");
                    break;
                }

                _ = stats_interval.tick() => {
                    // Backlog watermark check runs regardless of state — a
                    // backlog persists across Stop (TODO 68). Log only on
                    // level transitions (rise = warn, fall to 0 = info).
                    let level = crate::common::queue_accounting::backlog_level(
                        self.stats.queue.depth_bytes(),
                        self.config.backlog_soft_limit_bytes,
                        self.config.backlog_hard_limit_bytes,
                    );
                    let last = last_logged_level;
                    last_logged_level = level;
                    if level > last {
                        warn!(
                            backlog_level = level,
                            queue_bytes = self.stats.queue.depth_bytes(),
                            queue_items = self.stats.queue.depth_items(),
                            soft_limit_bytes = self.config.backlog_soft_limit_bytes,
                            hard_limit_bytes = self.config.backlog_hard_limit_bytes,
                            "Recorder backlog watermark exceeded"
                        );
                    } else if level < last && level == 0 {
                        info!(
                            queue_bytes = self.stats.queue.depth_bytes(),
                            "Recorder backlog back below watermarks"
                        );
                    }
                    if *self.state_rx.borrow() == ComponentState::Running {
                        let stats = self.stats.snapshot();
                        info!(
                            received_events = stats.total_events,
                            written_events = stats.written_events,
                            bytes_mb = stats.total_bytes_written as f64 / 1_000_000.0,
                            files = stats.files_written,
                            dropped = stats.dropped_batches,
                            queue_bytes = self.stats.queue.depth_bytes(),
                            "Recording progress"
                        );
                    }
                }
            }
        }

        // Shutdown tasks
        let _ = writer_tx.send(WriterCommand::Shutdown);
        drop(writer_tx);

        let _ = receiver_handle.await;
        // Writer is a std::thread — use spawn_blocking to avoid blocking tokio
        let _ = tokio::task::spawn_blocking(move || writer_handle.join()).await;
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

    /// Message::Data MsgPack wrapper size: 0x81 + 0xa4 + "Data" = 6 bytes.
    /// Everything after this offset is the raw EventDataBatch bytes.
    const DATA_WRAPPER_SIZE: usize = 6;

    /// Extract event count from raw EventDataBatch MsgPack bytes.
    /// Format: fixarray(4) [source_id, seq, timestamp, events_array]
    /// We skip the first 3 uint fields and read the events array header.
    fn extract_event_count(batch_bytes: &[u8]) -> u64 {
        // Parse outer array header
        let mut pos: usize = match batch_bytes.first() {
            Some(b) if (0x90..=0x9f).contains(b) => 1,
            Some(0xdc) if batch_bytes.len() >= 3 => 3,
            Some(0xdd) if batch_bytes.len() >= 5 => 5,
            _ => return 0,
        };

        // Skip 3 uint fields (source_id, sequence_number, timestamp)
        for _ in 0..3 {
            let skip = match batch_bytes.get(pos) {
                Some(b) if *b <= 0x7f => 1, // positive fixint
                Some(0xcc) => 2,            // uint8
                Some(0xcd) => 3,            // uint16
                Some(0xce) => 5,            // uint32
                Some(0xcf) => 9,            // uint64
                _ => return 0,
            };
            pos += skip;
        }

        // Read events array header for count
        match batch_bytes.get(pos) {
            Some(b) if (0x90..=0x9f).contains(b) => (b & 0x0f) as u64,
            Some(0xdc) if batch_bytes.len() >= pos + 3 => {
                u16::from_be_bytes([batch_bytes[pos + 1], batch_bytes[pos + 2]]) as u64
            }
            Some(0xdd) if batch_bytes.len() >= pos + 5 => u32::from_be_bytes([
                batch_bytes[pos + 1],
                batch_bytes[pos + 2],
                batch_bytes[pos + 3],
                batch_bytes[pos + 4],
            ]) as u64,
            _ => 0,
        }
    }

    /// Receiver task: ZMQ SUB → Writer channel (non-blocking, zero-copy)
    ///
    /// IMPORTANT: Always drains ZMQ socket to prevent internal buffer growth.
    /// When not Running, data is discarded immediately.
    ///
    /// Hot path uses lightweight header parsing instead of full deserialization:
    /// - Data messages: extract batch bytes (skip 6-byte Message wrapper) + event count
    /// - EOS: full deserialize (rare, once per source per run)
    async fn receiver_task(
        mut socket: subscribe::Subscribe,
        tx: std::sync::mpsc::Sender<WriterCommand>,
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

                // Always receive from ZMQ to drain the socket buffer
                // Data is only forwarded when Running, otherwise discarded
                msg = socket.next() => {
                    match msg {
                        Some(Ok(multipart)) => {
                            // Not running — discard to prevent ZMQ buffer growth.
                            // Tail batches after Stop land here by design
                            // (accepted loss — see CLAUDE.md データ保全 exception).
                            // Counted in dropped_batches (visible in Recording
                            // progress logs) so the loss is observable, never
                            // silent (TODO 58 C3): dropped growing while Running
                            // is a bug.
                            if !is_running {
                                let n = stats.dropped_batches.fetch_add(1, Ordering::Relaxed) + 1;
                                if n == 1 || n.is_multiple_of(1000) {
                                    info!(
                                        discarded_total = n,
                                        "Discarding batch while not Running (expected Stop tail)"
                                    );
                                }
                                continue;
                            }

                            if let Some(data) = multipart.into_iter().next() {
                                // Use lightweight header parsing for message type detection
                                match MessageHeader::parse(&data) {
                                    Some(MessageHeader::Data { .. }) => {
                                        // Extract raw EventDataBatch bytes (skip Message wrapper)
                                        if data.len() <= Self::DATA_WRAPPER_SIZE {
                                            continue;
                                        }
                                        let batch_bytes = &data[Self::DATA_WRAPPER_SIZE..];
                                        let event_count = Self::extract_event_count(batch_bytes);

                                        stats.received_batches.fetch_add(1, Ordering::Relaxed);
                                        stats.received_events.fetch_add(event_count, Ordering::Relaxed);

                                        // Pass raw bytes to writer (no deserialize/reserialize).
                                        // Backlog gauge (TODO 68): account before
                                        // the send, undo if the channel is gone.
                                        let batch_len = batch_bytes.len() as u64;
                                        stats.queue.on_enqueue(batch_len);
                                        if tx.send(WriterCommand::WriteRawBatch {
                                            data: batch_bytes.to_vec(),
                                            event_count,
                                        }).is_err() {
                                            stats.queue.on_dequeue(batch_len);
                                            info!("Channel closed, receiver exiting");
                                            break;
                                        }
                                    }
                                    Some(MessageHeader::EndOfStream { source_id }) => {
                                        // EOS is rare — full deserialize to get run_number
                                        let run_number = match Message::from_msgpack(&data) {
                                            Ok(Message::EndOfStream { run_number, .. }) => run_number,
                                            _ => 0,
                                        };
                                        info!(source_id, run_number, "Received EOS");
                                        if tx.send(WriterCommand::EndOfStream { source_id, run_number }).is_err() {
                                            info!("Channel closed, receiver exiting");
                                            break;
                                        }
                                    }
                                    Some(MessageHeader::Heartbeat { source_id }) => {
                                        debug!(source_id, "Received heartbeat");
                                    }
                                    None => {
                                        warn!("Failed to parse message header");
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

    /// Writer task: Handles file I/O on a dedicated OS thread.
    /// Runs blocking file I/O without affecting the tokio runtime.
    fn writer_task_blocking(
        rx: std::sync::mpsc::Receiver<WriterCommand>,
        config: RecorderConfig,
        stats: Arc<AtomicStats>,
        state_rx: watch::Receiver<ComponentState>,
    ) {
        // Kept for the write-failure latch: FileWriter takes ownership of the
        // shared stats Arc below.
        let stats_flag = Arc::clone(&stats);
        // TODO 68 E2E test hook — never silent when active.
        let test_write_delay_ms: Option<u64> = std::env::var("DELILA_TEST_WRITE_DELAY_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&ms| ms > 0);
        if let Some(ms) = test_write_delay_ms {
            warn!(
                delay_ms = ms,
                "TEST HOOK ACTIVE: artificial write delay per batch"
            );
        }
        let mut writer = FileWriter::new(config, stats);
        let mut eos_received = false;
        let mut current_run_number: u32 = 0;
        let mut last_state = *state_rx.borrow();

        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(cmd) => match cmd {
                    WriterCommand::WriteRawBatch { data, event_count } => {
                        // Test hook (TODO 68 E2E): artificial write latency to
                        // grow a real backlog. Env-gated; loud at startup.
                        if let Some(delay_ms) = test_write_delay_ms {
                            std::thread::sleep(Duration::from_millis(delay_ms));
                        }
                        if let Err(e) = writer.write_raw_batch(&data, event_count) {
                            // Disk full / I/O error: latch Error so the operator
                            // sees it — otherwise the rest of the run is silently
                            // discarded batch by batch (TODO 58 H7).
                            error!(error = %e, "Failed to write batch — latching Error state");
                            stats_flag.write_failed.store(true, Ordering::Relaxed);
                        }
                        // Dequeue only after the write returned (success or
                        // error): queue_bytes == 0 then means "handed to the
                        // BufWriter", closing the one-batch race the drain
                        // wait would otherwise have (TODO 68).
                        stats_flag.queue.on_dequeue(data.len() as u64);
                    }
                    WriterCommand::EndOfStream {
                        source_id,
                        run_number,
                    } => {
                        if run_number != current_run_number {
                            warn!(
                                source_id,
                                eos_run = run_number,
                                current_run = current_run_number,
                                "IGNORING stale EOS from previous run"
                            );
                        } else {
                            info!(source_id, run_number, "Writer received EOS - closing file");
                            if let Err(e) = writer.end_run() {
                                // A failed close can lose the footer/tail — latch
                                // Error so it is visible (TODO 58 H7).
                                error!(error = %e, "Failed to close file on EOS — latching Error state");
                                stats_flag.write_failed.store(true, Ordering::Relaxed);
                            }
                            eos_received = true;
                        }
                    }
                    WriterCommand::NewRun(run_config) => {
                        writer.new_run(run_config);
                        eos_received = false;
                        // A fresh Configure clears a latched write failure —
                        // the operator has acknowledged/resolved it (H7).
                        stats_flag.write_failed.store(false, Ordering::Relaxed);
                        info!("Writer configured for new run");
                    }
                    WriterCommand::DrainAndStart { run_number } => {
                        // NO explicit drain (TODO 58 H8): stale batches queued
                        // BEFORE this command are no-ops anyway (write_raw_batch
                        // ignores them while run_active=false), while a try_recv
                        // drain here raced AHEAD in the queue and could eat the
                        // NEW run's first batches (receiver may already be
                        // forwarding them once the state watch flips to Running).
                        current_run_number = run_number;
                        eos_received = false;
                        writer.start_run(run_number);
                        info!(run_number, "Writer started - recording enabled");
                    }
                    WriterCommand::CloseFile => {
                        if let Err(e) = writer.end_run() {
                            warn!(error = %e, "Failed to close file");
                        }
                        // CloseFile is sent by on_reset — Reset clears the latch (H7).
                        stats_flag.write_failed.store(false, Ordering::Relaxed);
                    }
                    WriterCommand::Shutdown => {
                        if let Err(e) = writer.close_file() {
                            warn!(error = %e, "Failed to close file on shutdown");
                        }
                        break;
                    }
                },
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Check for state changes (replaces tokio state_rx.changed())
                    let current = *state_rx.borrow();
                    if current != last_state {
                        debug!(state = %current, "Writer state changed");

                        // Close file when stopping (if not already closed by EOS)
                        if (current == ComponentState::Configured
                            || current == ComponentState::Idle)
                            && !eos_received
                        {
                            info!("State changed to {} - closing file", current);
                            if let Err(e) = writer.end_run() {
                                error!(error = %e, "Failed to close file on state change — latching Error state");
                                stats_flag.write_failed.store(true, Ordering::Relaxed);
                            }
                        }

                        // Reset EOS flag when starting new run
                        if current == ComponentState::Running {
                            eos_received = false;
                        }

                        last_state = current;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    info!("Writer channel disconnected");
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
        assert_eq!(path.to_str().unwrap(), "/data/run0042_0000_CRIB2026.delila");

        writer.file_sequence = 5;
        let path = writer.generate_filename();
        assert_eq!(path.to_str().unwrap(), "/data/run0042_0005_CRIB2026.delila");
    }
}
