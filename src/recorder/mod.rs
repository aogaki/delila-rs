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
    ChecksumCalculator, DataBlockIterator, DataFileReader, FileFooter, FileFormatError,
    FileHeader, FileValidationResult, FOOTER_SIZE, FORMAT_VERSION,
};

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
    EventDataBatch, Message, RunConfig,
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
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            subscribe_address: "tcp://localhost:5557".to_string(),
            command_address: "tcp://*:5580".to_string(),
            output_dir: PathBuf::from("./data"),
            max_file_size: 1024 * 1024 * 1024, // 1GB
            max_file_duration_secs: 600,       // 10 minutes
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

/// Commands for writer task
enum WriterCommand {
    /// Write a batch of raw data
    WriteBatch(EventDataBatch),
    /// End of stream - close current file
    EndOfStream { source_id: u32 },
    /// Configure for a new run
    NewRun(RunConfig),
    /// Start recording (enable writing) with the run number to use
    StartRun { run_number: u32 },
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

        // File exists - append Unix timestamp to avoid overwriting
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let filename_with_ts = format!(
            "run{:04}_{:04}_{}_{}.delila",
            run_config.run_number, self.file_sequence, exp_name, timestamp
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

    fn write_batch(&mut self, batch: EventDataBatch) -> Result<(), RecorderError> {
        if batch.events.is_empty() {
            return Ok(());
        }

        // Don't write if run is not active
        if !self.run_active {
            debug!("Ignoring write_batch: run not active");
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

        // Update timestamp range for footer
        if let (Some(first), Some(last)) = (batch.events.first(), batch.events.last()) {
            self.footer
                .update_timestamp_range(first.timestamp_ns, last.timestamp_ns);
        }

        let event_count = batch.events.len() as u64;
        let data = batch.to_msgpack()?;
        let len_bytes = (data.len() as u32).to_le_bytes();

        if let Some(ref mut writer) = self.writer {
            writer.write_all(&len_bytes)?;
            writer.write_all(&data)?;

            // Update checksum with data block (length prefix + data)
            self.checksum.update(&len_bytes);
            self.checksum.update(&data);

            let bytes_written = 4 + data.len() as u64;
            self.current_file_size += bytes_written;
            self.footer.total_events += event_count;

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
    writer_tx: mpsc::UnboundedSender<WriterCommand>,
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
        // Enable writing in writer task with the run number
        self.writer_tx
            .send(WriterCommand::StartRun { run_number })
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
            "Recorder created (raw data mode)"
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
        // Create channel: Receiver → Writer
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
        let writer_state_rx = self.state_rx.clone();
        let writer_handle = tokio::spawn(async move {
            Self::writer_task(writer_rx, writer_config, writer_stats, writer_state_rx).await
        });

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
        let _ = writer_tx.send(WriterCommand::Shutdown);

        let _ = receiver_handle.await;
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

    /// Receiver task: ZMQ SUB → Writer channel (non-blocking)
    async fn receiver_task(
        mut socket: subscribe::Subscribe,
        tx: mpsc::UnboundedSender<WriterCommand>,
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

                                        // Send directly to writer
                                        if tx.send(WriterCommand::WriteBatch(batch)).is_err() {
                                            info!("Channel closed, receiver exiting");
                                            break;
                                        }
                                    }
                                    Ok(Message::EndOfStream { source_id }) => {
                                        info!(source_id, "Received EOS - closing file");
                                        if tx.send(WriterCommand::EndOfStream { source_id }).is_err() {
                                            info!("Channel closed, receiver exiting");
                                            break;
                                        }
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

    /// Writer task: Handles file I/O
    async fn writer_task(
        mut rx: mpsc::UnboundedReceiver<WriterCommand>,
        config: RecorderConfig,
        stats: Arc<AtomicStats>,
        mut state_rx: watch::Receiver<ComponentState>,
    ) {
        let mut writer = FileWriter::new(config, stats);
        let mut eos_received = false;

        loop {
            tokio::select! {
                biased;

                cmd = rx.recv() => {
                    match cmd {
                        Some(WriterCommand::WriteBatch(batch)) => {
                            if let Err(e) = writer.write_batch(batch) {
                                warn!(error = %e, "Failed to write batch");
                            }
                        }
                        Some(WriterCommand::EndOfStream { source_id }) => {
                            info!(source_id, "Writer received EOS - closing file");
                            if let Err(e) = writer.end_run() {
                                warn!(error = %e, "Failed to close file on EOS");
                            }
                            eos_received = true;
                        }
                        Some(WriterCommand::NewRun(run_config)) => {
                            writer.new_run(run_config);
                            eos_received = false;
                            info!("Writer configured for new run");
                        }
                        Some(WriterCommand::StartRun { run_number }) => {
                            writer.start_run(run_number);
                            info!(run_number, "Writer started - recording enabled");
                        }
                        Some(WriterCommand::CloseFile) => {
                            if let Err(e) = writer.end_run() {
                                warn!(error = %e, "Failed to close file");
                            }
                        }
                        Some(WriterCommand::Shutdown) => {
                            if let Err(e) = writer.close_file() {
                                warn!(error = %e, "Failed to close file on shutdown");
                            }
                            break;
                        }
                        None => {
                            info!("Writer channel closed");
                            break;
                        }
                    }
                }

                _ = state_rx.changed() => {
                    let current = *state_rx.borrow();
                    debug!(state = %current, "Writer state changed");

                    // Close file when stopping (if not already closed by EOS)
                    if (current == ComponentState::Configured || current == ComponentState::Idle)
                        && !eos_received
                    {
                        info!("State changed to {} - closing file", current);
                        if let Err(e) = writer.end_run() {
                            warn!(error = %e, "Failed to close file on state change");
                        }
                    }

                    // Reset EOS flag when starting new run
                    if current == ComponentState::Running {
                        eos_received = false;
                    }
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
        assert_eq!(
            path.to_str().unwrap(),
            "/data/run0042_0000_CRIB2026.delila"
        );

        writer.file_sequence = 5;
        let path = writer.generate_filename();
        assert_eq!(
            path.to_str().unwrap(),
            "/data/run0042_0005_CRIB2026.delila"
        );
    }
}
