//! Reader module for digitizer data acquisition
//!
//! This module provides:
//! - CAEN digitizer FFI bindings (caen)
//! - Data decoders (decoder)
//! - Reader integration with two-task architecture

pub mod caen;
pub mod decoder;

// Re-exports
pub use caen::{CaenError, CaenHandle, EndpointHandle};
pub use crate::config::FirmwareType;
pub use decoder::{
    DataType, DecodeResult, EventData, Psd1Config, Psd1Decoder, Psd2Config, Psd2Decoder, Waveform,
};

use crate::common::{
    handle_command, run_command_task, CommandHandlerExt, ComponentSharedState, ComponentState,
    EventData as CommonEventData, EventDataBatch, Message, Waveform as CommonWaveform,
};
use futures::SinkExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tmq::publish;
use tmq::Context;
use tokio::sync::{mpsc, watch, Mutex};
use tokio::time::interval;
use tracing::{debug, error, info, warn};

/// Reader error type
#[derive(Debug, Error)]
pub enum ReaderError {
    #[error("CAEN error: {0}")]
    Caen(#[from] CaenError),

    #[error("ZMQ error: {0}")]
    Zmq(#[from] tmq::TmqError),

    #[error("MessagePack serialization error: {0}")]
    MsgPack(#[from] rmp_serde::encode::Error),

    #[error("Decode error: {0}")]
    Decode(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Channel send error")]
    ChannelSend,
}

/// Enum-based decoder dispatch (KISS: only PSD1/PSD2/PHA1, no trait object needed)
enum DecoderKind {
    Psd2(Psd2Decoder),
    Psd1(Psd1Decoder),
}

impl DecoderKind {
    fn classify(&self, raw: &decoder::RawData) -> DataType {
        match self {
            Self::Psd2(d) => d.classify(raw),
            Self::Psd1(d) => d.classify(raw),
        }
    }

    fn decode(&mut self, raw: &decoder::RawData) -> Vec<decoder::EventData> {
        match self {
            Self::Psd2(d) => d.decode(raw),
            Self::Psd1(d) => d.decode(raw),
        }
    }
}

/// Reader configuration
#[derive(Debug, Clone)]
pub struct ReaderConfig {
    /// Device URL (e.g., "dig2://172.18.4.56")
    pub url: String,
    /// ZMQ data publish address
    pub data_address: String,
    /// ZMQ command address (REP socket)
    pub command_address: String,
    /// Source ID for this reader
    pub source_id: u32,
    /// Firmware type (determines decoder)
    pub firmware: FirmwareType,
    /// Module ID for decoded events
    pub module_id: u8,
    /// Read timeout in milliseconds
    pub read_timeout_ms: i32,
    /// Buffer size for raw data reads
    pub buffer_size: usize,
    /// Heartbeat interval in milliseconds (0 = disabled)
    pub heartbeat_interval_ms: u64,
    /// Time step in nanoseconds (for timestamp calculation)
    pub time_step_ns: f64,
    /// Path to digitizer configuration JSON file (optional)
    pub config_file: Option<String>,
}

impl Default for ReaderConfig {
    fn default() -> Self {
        Self {
            url: "dig2://localhost".to_string(),
            data_address: "tcp://*:5555".to_string(),
            command_address: "tcp://*:5556".to_string(),
            source_id: 0,
            firmware: FirmwareType::PSD2,
            module_id: 0,
            read_timeout_ms: 100,
            buffer_size: 1024 * 1024, // 1MB
            heartbeat_interval_ms: 1000,
            time_step_ns: 2.0, // 500 MHz ADC = 2ns per sample
            config_file: None,
        }
    }
}

impl ReaderConfig {
    /// Create ReaderConfig from Config and source ID
    ///
    /// Returns None if source_id is not found or source has no digitizer_url
    pub fn from_config(config: &crate::config::Config, source_id: u32) -> Option<Self> {
        let source = config.get_source(source_id)?;
        let url = source.digitizer_url.as_ref()?;

        let firmware = match source.source_type {
            crate::config::SourceType::Psd2 => FirmwareType::PSD2,
            crate::config::SourceType::Psd1 => FirmwareType::PSD1,
            crate::config::SourceType::Pha1 => FirmwareType::PHA,
            // Emulator/Zle sources shouldn't create a Reader — caller should handle
            _ => return None,
        };

        Some(Self {
            url: url.clone(),
            data_address: source.bind.clone(),
            command_address: source.command_address(),
            source_id,
            firmware,
            module_id: source.module_id.unwrap_or(source_id as u8),
            read_timeout_ms: 100,
            buffer_size: 1024 * 1024, // 1MB
            heartbeat_interval_ms: 1000,
            time_step_ns: source.time_step_ns.unwrap_or(2.0),
            config_file: source.config_file.clone(),
        })
    }
}

/// Metrics for monitoring
#[derive(Debug, Default)]
pub struct ReaderMetrics {
    /// Total events decoded
    pub events_decoded: AtomicU64,
    /// Total bytes read from digitizer
    pub bytes_read: AtomicU64,
    /// Total batches published
    pub batches_published: AtomicU64,
    /// Current decode queue length (approximate)
    pub queue_length: AtomicU64,
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

/// Command handler extension for Reader
struct ReaderCommandExt {
    metrics: Arc<ReaderMetrics>,
    rate_tracker: Arc<RateTracker>,
    /// Digitizer URL for Detect command (e.g., "dig2://172.18.4.56")
    url: String,
}

impl CommandHandlerExt for ReaderCommandExt {
    fn component_name(&self) -> &'static str {
        "Reader"
    }

    fn status_details(&self) -> Option<String> {
        let events = self.metrics.events_decoded.load(Ordering::Relaxed);
        let batches = self.metrics.batches_published.load(Ordering::Relaxed);
        let bytes = self.metrics.bytes_read.load(Ordering::Relaxed);
        Some(format!(
            "Events: {}, Batches: {}, Bytes: {}",
            events, batches, bytes
        ))
    }

    fn get_metrics(&self) -> Option<crate::common::ComponentMetrics> {
        let events = self.metrics.events_decoded.load(Ordering::Relaxed);
        let bytes = self.metrics.bytes_read.load(Ordering::Relaxed);
        let queue = self.metrics.queue_length.load(Ordering::Relaxed);
        self.rate_tracker.update(events);
        Some(crate::common::ComponentMetrics {
            events_processed: events,
            bytes_transferred: bytes,
            queue_size: queue as u32,
            queue_max: 0,
            event_rate: self.rate_tracker.get_rate(),
            data_rate: 0.0,
        })
    }

    fn on_start(&mut self, _run_number: u32) -> Result<(), String> {
        self.rate_tracker.reset();
        Ok(())
    }

    fn on_detect(&mut self) -> Result<serde_json::Value, String> {
        // Temporarily connect to digitizer, read DeviceInfo, and disconnect.
        // This blocks briefly (< 1s) but is acceptable for an infrequent
        // user-initiated action from Idle state.
        let handle = caen::handle::CaenHandle::open(&self.url)
            .map_err(|e| format!("Failed to connect to {}: {}", self.url, e))?;
        let info = handle
            .get_device_info()
            .map_err(|e| format!("Failed to read device info: {}", e))?;
        // handle dropped here → connection closed
        serde_json::to_value(&info).map_err(|e| format!("Failed to serialize DeviceInfo: {}", e))
    }
}

/// Send firmware-specific arm command to the digitizer.
///
/// For DIG1 (PSD1/PHA) with START_MODE_SW, the actual arm is deferred to start phase.
/// For DIG2 (PSD2), always sends armacquisition immediately.
fn send_arm_command(
    handle: &CaenHandle,
    firmware: FirmwareType,
) -> Result<(), caen::CaenError> {
    if firmware.is_dig1() {
        let startmode = handle.get_value("/par/startmode").unwrap_or_default();
        if startmode == "START_MODE_SW" {
            info!("START_MODE_SW detected - deferring arm to Start");
        } else {
            info!("Arming digitizer (DIG1, mode={})", startmode);
            handle.send_command("/cmd/armacquisition")?;
        }
    } else {
        info!("Arming digitizer (PSD2)");
        handle.send_command("/cmd/armacquisition")?;
    }
    Ok(())
}

/// Send firmware-specific start command to the digitizer.
///
/// For DIG2 (PSD2), sends swstartacquisition.
/// For DIG1 (PSD1/PHA) with START_MODE_SW, sends armacquisition (arm=start).
fn send_start_command(
    handle: &CaenHandle,
    firmware: FirmwareType,
) -> Result<(), caen::CaenError> {
    if firmware.is_dig1() {
        let startmode = handle.get_value("/par/startmode").unwrap_or_default();
        if startmode == "START_MODE_SW" {
            info!("Starting acquisition (DIG1, START_MODE_SW)");
            handle.send_command("/cmd/armacquisition")?;
        } else {
            info!("DIG1 acquisition already started on Arm");
        }
    } else {
        info!("Starting digitizer acquisition (PSD2)");
        handle.send_command("/cmd/swstartacquisition")?;
    }
    Ok(())
}

/// Reader for CAEN digitizer data acquisition
///
/// Uses two-task architecture:
/// - ReadLoop: Blocking reads from CAEN hardware (spawn_blocking)
/// - DecodeLoop: Async decoding and ZMQ publishing
pub struct Reader {
    config: ReaderConfig,
    data_socket: publish::Publish,
    shared_state: Arc<Mutex<ComponentSharedState>>,
    state_rx: watch::Receiver<ComponentState>,
    state_tx: watch::Sender<ComponentState>,
    metrics: Arc<ReaderMetrics>,
    rate_tracker: Arc<RateTracker>,
}

impl Reader {
    /// Create a new Reader with the given configuration
    pub async fn new(config: ReaderConfig) -> Result<Self, ReaderError> {
        let context = Context::new();
        let data_socket = publish(&context).bind(&config.data_address)?;

        info!(
            data_address = %config.data_address,
            command_address = %config.command_address,
            url = %config.url,
            "Reader bound to data address"
        );

        let (state_tx, state_rx) = watch::channel(ComponentState::Idle);

        Ok(Self {
            config,
            data_socket,
            shared_state: Arc::new(Mutex::new(ComponentSharedState::new())),
            state_rx,
            state_tx,
            metrics: Arc::new(ReaderMetrics::default()),
            rate_tracker: Arc::new(RateTracker::new()),
        })
    }

    /// Get current state
    pub fn state(&self) -> ComponentState {
        *self.state_rx.borrow()
    }

    /// Get metrics
    pub fn metrics(&self) -> &Arc<ReaderMetrics> {
        &self.metrics
    }

    /// Convert EventData to CommonEventData
    fn convert_event(event: &EventData) -> CommonEventData {
        if let Some(ref wf) = event.waveform {
            CommonEventData::with_waveform(
                event.module,
                event.channel,
                event.energy,
                event.energy_short,
                event.timestamp_ns,
                event.flags as u64,
                CommonWaveform {
                    analog_probe1: wf.analog_probe1.clone(),
                    analog_probe2: wf.analog_probe2.clone(),
                    digital_probe1: wf.digital_probe1.clone(),
                    digital_probe2: wf.digital_probe2.clone(),
                    digital_probe3: wf.digital_probe3.clone(),
                    digital_probe4: wf.digital_probe4.clone(),
                    time_resolution: wf.time_resolution,
                    trigger_threshold: wf.trigger_threshold,
                },
            )
        } else {
            CommonEventData::new(
                event.module,
                event.channel,
                event.energy,
                event.energy_short,
                event.timestamp_ns,
                event.flags as u64,
            )
        }
    }

    /// Publish a message via ZMQ
    async fn publish_message(&mut self, message: &Message) -> Result<(), ReaderError> {
        let bytes = message.to_msgpack()?;
        let msg: tmq::Multipart = vec![tmq::Message::from(bytes.as_slice())].into();
        self.data_socket.send(msg).await?;

        match message {
            Message::Data(batch) => {
                debug!(
                    seq = batch.sequence_number,
                    events = batch.len(),
                    "Published batch"
                );
                self.metrics
                    .batches_published
                    .fetch_add(1, Ordering::Relaxed);
            }
            Message::EndOfStream { source_id } => {
                info!(source_id = source_id, "Published EOS");
            }
            Message::Heartbeat(hb) => {
                debug!(
                    source_id = hb.source_id,
                    counter = hb.counter,
                    "Published heartbeat"
                );
            }
        }

        Ok(())
    }

    /// Send EOS (End Of Stream) signal
    async fn send_eos(&mut self) -> Result<(), ReaderError> {
        let eos = Message::eos(self.config.source_id);
        self.publish_message(&eos).await
    }

    /// ReadLoop task - runs in spawn_blocking to avoid blocking tokio runtime
    ///
    /// Reads raw data from CAEN digitizer and sends to decode channel.
    /// Respects state machine: only arms/starts digitizer when state transitions occur.
    fn read_loop(
        config: ReaderConfig,
        tx: mpsc::UnboundedSender<decoder::RawData>,
        state_rx: watch::Receiver<ComponentState>,
        metrics: Arc<ReaderMetrics>,
        shutdown: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), ReaderError> {
        info!(url = %config.url, "ReadLoop starting, connecting to digitizer");

        // Open connection to digitizer
        let handle = CaenHandle::open(&config.url)?;
        info!("Connected to digitizer");

        // Configure endpoint for RAW data
        let include_n_events = config.firmware.includes_n_events();
        let endpoint = handle.configure_endpoint(include_n_events)?;
        info!("Endpoint configured");

        // Track digitizer hardware state
        let mut hw_armed = false;
        let mut hw_running = false;
        let mut prev_state = ComponentState::Idle;

        loop {
            // Check shutdown flag
            if shutdown.load(Ordering::Relaxed) {
                info!("ReadLoop received shutdown signal");
                break;
            }

            // Get current state
            let current_state = *state_rx.borrow();

            // Handle state transitions
            if current_state != prev_state {
                info!(from = %prev_state, to = %current_state, "State transition");

                match (prev_state, current_state) {
                    // Configure digitizer when entering Configured state from Idle
                    (ComponentState::Idle, ComponentState::Configured) => {
                        // Apply configuration from JSON file if specified
                        if let Some(ref config_path) = config.config_file {
                            info!(path = %config_path, "Loading digitizer configuration");
                            match crate::config::digitizer::DigitizerConfig::load(config_path) {
                                Ok(dig_config) => {
                                    match handle.apply_config(&dig_config) {
                                        Ok(count) => {
                                            info!(count, "Digitizer configuration applied");
                                        }
                                        Err(e) => {
                                            error!(error = %e, "Failed to apply digitizer configuration");
                                            // Continue anyway - some parameters may have been applied
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!(error = %e, path = %config_path, "Failed to load digitizer configuration");
                                    // Continue without configuration
                                }
                            }
                        } else {
                            info!("No config_file specified, using current digitizer settings");
                        }
                    }

                    // Arm digitizer when entering Armed state
                    (_, ComponentState::Armed) => {
                        if !hw_armed {
                            send_arm_command(&handle, config.firmware)?;
                            hw_armed = true;
                        }
                    }

                    // Start acquisition when entering Running state
                    // Use (_, Running) to handle both Armed→Running and
                    // Configured→Running (when watch channel misses Armed state)
                    (_, ComponentState::Running) => {
                        if !hw_running {
                            // Arm if not yet armed (handles skipped Armed state)
                            if !hw_armed {
                                send_arm_command(&handle, config.firmware)?;
                                hw_armed = true;
                            }
                            send_start_command(&handle, config.firmware)?;
                            hw_running = true;
                        }
                    }

                    // Stop acquisition when leaving Running state
                    (ComponentState::Running, ComponentState::Configured) => {
                        if hw_running {
                            info!("Stopping digitizer acquisition");
                            let _ = handle.send_command("/cmd/disarmacquisition");
                            hw_armed = false;
                            hw_running = false;
                        }
                    }

                    // Reset: disarm if armed
                    (_, ComponentState::Idle) => {
                        if hw_armed || hw_running {
                            info!("Resetting digitizer");
                            let _ = handle.send_command("/cmd/disarmacquisition");
                            hw_armed = false;
                            hw_running = false;
                        }
                    }

                    _ => {}
                }

                prev_state = current_state;
            }

            // Only read data when Running
            if current_state != ComponentState::Running {
                // Not running, sleep briefly and check again
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }

            // Read data from digitizer
            match endpoint.read_data(config.read_timeout_ms, config.buffer_size) {
                Ok(Some(raw)) => {
                    metrics
                        .bytes_read
                        .fetch_add(raw.size as u64, Ordering::Relaxed);

                    // Convert to decoder RawData and send
                    let decoder_raw = decoder::RawData::from(raw);

                    // Update queue length metric (approximate)
                    metrics.queue_length.fetch_add(1, Ordering::Relaxed);

                    if tx.send(decoder_raw).is_err() {
                        warn!("Decode channel closed, stopping read loop");
                        break;
                    }
                }
                Ok(None) => {
                    // Timeout - no data available, continue
                }
                Err(e) => {
                    // Check if it's a stop signal
                    if e.code == caen::error::codes::STOP {
                        info!("Received STOP signal from digitizer");
                        break;
                    }
                    error!(error = %e, "Read error");
                    // Continue on non-fatal errors
                }
            }
        }

        // Cleanup: stop acquisition if still running
        if hw_armed || hw_running {
            let _ = handle.send_command("/cmd/disarmacquisition");
        }
        info!("ReadLoop stopped");
        Ok(())
    }

    /// DecodeLoop task - decodes raw data and publishes via ZMQ
    async fn decode_loop(
        config: ReaderConfig,
        mut rx: mpsc::UnboundedReceiver<decoder::RawData>,
        mut data_socket: publish::Publish,
        metrics: Arc<ReaderMetrics>,
        state_rx: watch::Receiver<ComponentState>,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), ReaderError> {
        info!("DecodeLoop starting");

        // Create decoder based on firmware type
        let mut decoder = match config.firmware {
            FirmwareType::PSD2 => {
                let psd2_config = Psd2Config {
                    time_step_ns: config.time_step_ns,
                    module_id: config.module_id,
                    dump_enabled: false,
                    num_channels: 32,
                };
                DecoderKind::Psd2(Psd2Decoder::new(psd2_config))
            }
            FirmwareType::PSD1 => {
                let psd1_config = Psd1Config {
                    time_step_ns: config.time_step_ns,
                    module_id: config.module_id,
                    dump_enabled: false,
                };
                DecoderKind::Psd1(Psd1Decoder::new(psd1_config))
            }
            FirmwareType::PHA => {
                return Err(ReaderError::Config(
                    "PHA1 decoder not yet implemented".to_string(),
                ));
            }
        };

        let mut sequence_number: u64 = 0;
        let mut heartbeat_counter: u64 = 0;

        // Heartbeat ticker
        let use_heartbeat = config.heartbeat_interval_ms > 0;
        let mut heartbeat_ticker =
            interval(Duration::from_millis(config.heartbeat_interval_ms.max(100)));

        loop {
            tokio::select! {
                biased;

                _ = shutdown.recv() => {
                    info!("DecodeLoop received shutdown signal");
                    break;
                }

                // Heartbeat (only when Running)
                _ = heartbeat_ticker.tick(), if use_heartbeat && *state_rx.borrow() == ComponentState::Running => {
                    let hb = Message::heartbeat(config.source_id, heartbeat_counter);
                    heartbeat_counter += 1;
                    let bytes = hb.to_msgpack()?;
                    let msg: tmq::Multipart = vec![tmq::Message::from(bytes.as_slice())].into();
                    data_socket.send(msg).await?;
                    debug!(counter = heartbeat_counter, "Published heartbeat");
                }

                // Receive raw data from ReadLoop
                raw = rx.recv() => {
                    match raw {
                        Some(raw_data) => {
                            // Update queue length metric
                            metrics.queue_length.fetch_sub(1, Ordering::Relaxed);

                            // Classify and decode
                            let data_type = decoder.classify(&raw_data);
                            match data_type {
                                DataType::Event => {
                                    // Decode events
                                    let events = decoder.decode(&raw_data);

                                    if events.is_empty() {
                                        continue;
                                    }

                                    // Convert to EventDataBatch
                                    let mut batch = EventDataBatch::with_capacity(
                                        config.source_id,
                                        sequence_number,
                                        events.len(),
                                    );

                                    for event in &events {
                                        batch.push(Self::convert_event(event));
                                    }

                                    // Update metrics
                                    metrics.events_decoded.fetch_add(events.len() as u64, Ordering::Relaxed);

                                    // Publish
                                    let msg = Message::data(batch);
                                    let bytes = msg.to_msgpack()?;
                                    let zmq_msg: tmq::Multipart = vec![tmq::Message::from(bytes.as_slice())].into();
                                    data_socket.send(zmq_msg).await?;

                                    sequence_number += 1;
                                    metrics.batches_published.fetch_add(1, Ordering::Relaxed);

                                    debug!(events = events.len(), seq = sequence_number - 1, "Decoded and published batch");
                                }
                                DataType::Start => {
                                    info!("Received START signal from digitizer");
                                    // Reset sequence number on Start
                                    sequence_number = 0;
                                    heartbeat_counter = 0;
                                    info!("Sequence number reset to 0 on Start");
                                }
                                DataType::Stop => {
                                    info!("Received STOP signal from digitizer");
                                    // Send EOS
                                    let eos = Message::eos(config.source_id);
                                    let bytes = eos.to_msgpack()?;
                                    let zmq_msg: tmq::Multipart = vec![tmq::Message::from(bytes.as_slice())].into();
                                    data_socket.send(zmq_msg).await?;
                                    info!(source_id = config.source_id, "Published EOS");
                                }
                                DataType::Unknown => {
                                    warn!("Received unknown data type");
                                }
                            }
                        }
                        None => {
                            info!("Raw data channel closed, stopping decode loop");
                            break;
                        }
                    }
                }
            }
        }

        info!(
            total_batches = sequence_number,
            total_events = metrics.events_decoded.load(Ordering::Relaxed),
            "DecodeLoop stopped"
        );
        Ok(())
    }

    /// Run the reader with command control
    ///
    /// Spawns three tasks:
    /// - Command task: handles control commands
    /// - ReadLoop task: reads from CAEN hardware (blocking)
    /// - DecodeLoop task: decodes and publishes data (async)
    pub async fn run(
        mut self,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), ReaderError> {
        info!(
            source_id = self.config.source_id,
            state = %self.state(),
            "Reader ready, waiting for commands"
        );

        // Create channels
        let (raw_tx, raw_rx) = mpsc::unbounded_channel::<decoder::RawData>();

        // Shutdown flag for ReadLoop (it runs in spawn_blocking, can't use async channel)
        let read_shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let read_shutdown_clone = read_shutdown.clone();

        // Spawn command handler task using common infrastructure
        let command_address = self.config.command_address.clone();
        let shared_state = self.shared_state.clone();
        let state_tx = self.state_tx.clone();
        let shutdown_for_cmd = shutdown.resubscribe();
        let metrics_for_cmd = self.metrics.clone();
        let rate_tracker_for_cmd = self.rate_tracker.clone();
        let url_for_cmd = self.config.url.clone();

        let cmd_handle = tokio::spawn(async move {
            run_command_task(
                command_address,
                shared_state,
                state_tx,
                shutdown_for_cmd,
                move |state, tx, cmd| {
                    let mut ext = ReaderCommandExt {
                        metrics: metrics_for_cmd.clone(),
                        rate_tracker: rate_tracker_for_cmd.clone(),
                        url: url_for_cmd.clone(),
                    };
                    handle_command(state, tx, cmd, Some(&mut ext))
                },
                "Reader",
            )
            .await;
        });

        // Spawn ReadLoop task (blocking)
        let read_config = self.config.clone();
        let read_state_rx = self.state_rx.clone();
        let read_metrics = self.metrics.clone();

        let read_handle = tokio::task::spawn_blocking(move || {
            Self::read_loop(
                read_config,
                raw_tx,
                read_state_rx,
                read_metrics,
                read_shutdown_clone,
            )
        });

        // Take ownership of data_socket for decode loop
        let data_socket = std::mem::replace(
            &mut self.data_socket,
            // Dummy socket - will not be used after this
            publish(&Context::new()).bind("tcp://127.0.0.1:0").unwrap(),
        );

        // Spawn DecodeLoop task
        let decode_config = self.config.clone();
        let decode_metrics = self.metrics.clone();
        let decode_state_rx = self.state_rx.clone();
        let shutdown_for_decode = shutdown.resubscribe();

        let decode_handle = tokio::spawn(async move {
            Self::decode_loop(
                decode_config,
                raw_rx,
                data_socket,
                decode_metrics,
                decode_state_rx,
                shutdown_for_decode,
            )
            .await
        });

        // Wait for shutdown signal
        let _ = shutdown.recv().await;
        info!("Reader received shutdown signal");

        // Signal ReadLoop to stop
        read_shutdown.store(true, Ordering::Relaxed);

        // Wait for tasks to complete
        let _ = cmd_handle.await;
        let _ = read_handle.await;
        let _ = decode_handle.await;

        // Send EOS if we were running
        if *self.state_rx.borrow() == ComponentState::Running {
            self.send_eos().await?;
        }

        info!(
            total_events = self.metrics.events_decoded.load(Ordering::Relaxed),
            total_bytes = self.metrics.bytes_read.load(Ordering::Relaxed),
            total_batches = self.metrics.batches_published.load(Ordering::Relaxed),
            "Reader stopped"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ReaderConfig::default();
        assert_eq!(config.source_id, 0);
        assert_eq!(config.firmware, FirmwareType::PSD2);
        assert_eq!(config.buffer_size, 1024 * 1024);
    }

    #[test]
    fn test_convert_event() {
        let event = EventData {
            timestamp_ns: 1234567.0,
            module: 1,
            channel: 5,
            energy: 1000,
            energy_short: 800,
            fine_time: 512,
            flags: 0x01,
            waveform: None,
        };

        let minimal = Reader::convert_event(&event);
        // CommonEventData is packed, so we need to copy values before comparing
        let module = minimal.module;
        let channel = minimal.channel;
        let energy = { minimal.energy };
        let energy_short = { minimal.energy_short };
        let timestamp_ns = { minimal.timestamp_ns };
        let flags = { minimal.flags };

        assert_eq!(module, 1);
        assert_eq!(channel, 5);
        assert_eq!(energy, 1000);
        assert_eq!(energy_short, 800);
        assert_eq!(timestamp_ns, 1234567.0);
        assert_eq!(flags, 0x01);
        assert!(minimal.waveform.is_none());
    }

    #[test]
    fn test_from_config_psd2_maps_firmware() {
        let toml = r#"
            [[network.sources]]
            id = 0
            type = "psd2"
            bind = "tcp://*:5555"
            digitizer_url = "dig2://172.18.4.56"

            [network.merger]
            subscribe = ["tcp://localhost:5555"]
            publish = "tcp://*:5557"

            [network.recorder]
            subscribe = "tcp://localhost:5557"
        "#;
        let config = crate::config::Config::from_toml(toml).unwrap();
        let reader_config = ReaderConfig::from_config(&config, 0).unwrap();
        assert_eq!(reader_config.firmware, FirmwareType::PSD2);
    }

    #[test]
    fn test_from_config_psd1_maps_firmware() {
        let toml = r#"
            [[network.sources]]
            id = 0
            type = "psd1"
            bind = "tcp://*:5555"
            digitizer_url = "dig1://caen.internal/usb?link_num=0"

            [network.merger]
            subscribe = ["tcp://localhost:5555"]
            publish = "tcp://*:5557"

            [network.recorder]
            subscribe = "tcp://localhost:5557"
        "#;
        let config = crate::config::Config::from_toml(toml).unwrap();
        let reader_config = ReaderConfig::from_config(&config, 0).unwrap();
        assert_eq!(reader_config.firmware, FirmwareType::PSD1);
    }

    #[test]
    fn test_from_config_emulator_returns_none() {
        let toml = r#"
            [[network.sources]]
            id = 0
            type = "emulator"
            bind = "tcp://*:5555"
            digitizer_url = "dig2://172.18.4.56"

            [network.merger]
            subscribe = ["tcp://localhost:5555"]
            publish = "tcp://*:5557"

            [network.recorder]
            subscribe = "tcp://localhost:5557"
        "#;
        let config = crate::config::Config::from_toml(toml).unwrap();
        // Emulator sources should NOT create a ReaderConfig
        assert!(ReaderConfig::from_config(&config, 0).is_none());
    }

    #[test]
    fn test_convert_event_with_waveform() {
        let wf = Waveform {
            analog_probe1: vec![100, 200, -300],
            analog_probe2: vec![10, 20, -30],
            digital_probe1: vec![1, 0, 1],
            digital_probe2: vec![0, 1, 0],
            digital_probe3: vec![1, 1, 0],
            digital_probe4: vec![0, 0, 1],
            time_resolution: 2,
            trigger_threshold: 500,
        };

        let event = EventData {
            timestamp_ns: 999.0,
            module: 0,
            channel: 3,
            energy: 2000,
            energy_short: 1500,
            fine_time: 100,
            flags: 0x00,
            waveform: Some(wf),
        };

        let converted = Reader::convert_event(&event);
        assert!(converted.waveform.is_some(), "Waveform should be preserved");
        let cwf = converted.waveform.unwrap();
        assert_eq!(cwf.analog_probe1, vec![100, 200, -300]);
        assert_eq!(cwf.analog_probe2, vec![10, 20, -30]);
        assert_eq!(cwf.digital_probe1, vec![1, 0, 1]);
        assert_eq!(cwf.time_resolution, 2);
        assert_eq!(cwf.trigger_threshold, 500);
    }
}
