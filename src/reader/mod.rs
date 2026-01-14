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
pub use decoder::{DataType, DecodeResult, EventData, Psd2Config, Psd2Decoder, Waveform};

use crate::common::{
    handle_command_simple, run_command_task, ComponentSharedState, ComponentState, Message,
    MinimalEventData, MinimalEventDataBatch,
};
use futures::SinkExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
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

/// Firmware type for decoder selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareType {
    /// PSD2 firmware (x27xx series, 64-bit words)
    Psd2,
    /// PSD1 firmware (x725/x730, 32-bit words) - not yet implemented
    Psd1,
    /// PHA1 firmware - not yet implemented
    Pha1,
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
}

impl Default for ReaderConfig {
    fn default() -> Self {
        Self {
            url: "dig2://localhost".to_string(),
            data_address: "tcp://*:5555".to_string(),
            command_address: "tcp://*:5556".to_string(),
            source_id: 0,
            firmware: FirmwareType::Psd2,
            module_id: 0,
            read_timeout_ms: 100,
            buffer_size: 1024 * 1024, // 1MB
            heartbeat_interval_ms: 1000,
            time_step_ns: 2.0, // 500 MHz ADC = 2ns per sample
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

        Some(Self {
            url: url.clone(),
            data_address: source.bind.clone(),
            command_address: source.command_address(),
            source_id,
            firmware: FirmwareType::Psd2, // Currently only PSD2 is supported
            module_id: source.module_id.unwrap_or(source_id as u8),
            read_timeout_ms: 100,
            buffer_size: 1024 * 1024, // 1MB
            heartbeat_interval_ms: 1000,
            time_step_ns: source.time_step_ns.unwrap_or(2.0),
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

    /// Convert EventData to MinimalEventData
    fn convert_event(event: &EventData) -> MinimalEventData {
        MinimalEventData::new(
            event.module,
            event.channel,
            event.energy,
            event.energy_short,
            event.timestamp_ns,
            event.flags as u64,
        )
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
        let endpoint = handle.configure_endpoint()?;
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
                    // Arm digitizer when entering Armed state
                    (_, ComponentState::Armed) => {
                        if !hw_armed {
                            info!("Arming digitizer");
                            handle.send_command("/cmd/armacquisition")?;
                            hw_armed = true;
                        }
                    }

                    // Start acquisition when entering Running state
                    (ComponentState::Armed, ComponentState::Running) => {
                        if hw_armed && !hw_running {
                            info!("Starting digitizer acquisition");
                            handle.send_command("/cmd/swstartacquisition")?;
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
            FirmwareType::Psd2 => {
                let psd2_config = Psd2Config {
                    time_step_ns: config.time_step_ns,
                    module_id: config.module_id,
                    dump_enabled: false,
                };
                Psd2Decoder::new(psd2_config)
            }
            FirmwareType::Psd1 => {
                return Err(ReaderError::Config(
                    "PSD1 decoder not yet implemented".to_string(),
                ));
            }
            FirmwareType::Pha1 => {
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

                                    // Convert to MinimalEventDataBatch
                                    let mut batch = MinimalEventDataBatch::with_capacity(
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

        let cmd_handle = tokio::spawn(async move {
            run_command_task(
                command_address,
                shared_state,
                state_tx,
                shutdown_for_cmd,
                |state, tx, cmd| handle_command_simple(state, tx, cmd, "Reader"),
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
        assert_eq!(config.firmware, FirmwareType::Psd2);
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
        // MinimalEventData is packed, so we need to copy values before comparing
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
    }
}
