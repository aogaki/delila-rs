//! Emulator data source - generates dummy event data for testing
//!
//! This module provides a data source that generates random event data
//! and publishes it via ZeroMQ PUB socket.
//!
//! Architecture:
//! - Main task: generates and publishes data when Running
//! - Command task: handles REQ/REP commands, updates shared state via watch channel

use std::sync::Arc;
use std::time::Duration;

use futures::SinkExt;
use rand::Rng;
use rand_distr::{Distribution, Normal};
use thiserror::Error;
use tmq::{publish, Context};
use tokio::sync::{watch, Mutex};
use tokio::time::interval;
use tracing::{debug, info};

use crate::common::{
    flags, handle_command_simple, run_command_task, ComponentSharedState, ComponentState,
    EventData, EventDataBatch, Message, Waveform,
};

/// Waveform probe bit masks
pub mod waveform_probes {
    /// Analog probe 1
    pub const ANALOG_PROBE1: u8 = 0b0000_0001;
    /// Analog probe 2
    pub const ANALOG_PROBE2: u8 = 0b0000_0010;
    /// Digital probe 1
    pub const DIGITAL_PROBE1: u8 = 0b0000_0100;
    /// Digital probe 2
    pub const DIGITAL_PROBE2: u8 = 0b0000_1000;
    /// Digital probe 3
    pub const DIGITAL_PROBE3: u8 = 0b0001_0000;
    /// Digital probe 4
    pub const DIGITAL_PROBE4: u8 = 0b0010_0000;
    /// All analog probes
    pub const ALL_ANALOG: u8 = ANALOG_PROBE1 | ANALOG_PROBE2;
    /// All digital probes
    pub const ALL_DIGITAL: u8 = DIGITAL_PROBE1 | DIGITAL_PROBE2 | DIGITAL_PROBE3 | DIGITAL_PROBE4;
    /// All probes
    pub const ALL: u8 = ALL_ANALOG | ALL_DIGITAL;
}

/// Emulator configuration
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct EmulatorConfig {
    /// ZMQ bind address for data (e.g., "tcp://*:5555")
    pub address: String,
    /// ZMQ bind address for commands (e.g., "tcp://*:5560")
    pub command_address: String,
    /// Source ID for this emulator instance
    pub source_id: u32,
    /// Number of events per batch
    pub events_per_batch: usize,
    /// Interval between batches in milliseconds
    pub batch_interval_ms: u64,
    /// Heartbeat interval in milliseconds (0 = disabled)
    pub heartbeat_interval_ms: u64,
    /// Number of modules to simulate
    pub num_modules: u8,
    /// Number of channels per module
    pub channels_per_module: u8,
    /// Enable waveform generation for all events
    pub enable_waveform: bool,
    /// Bitmask of enabled probes (see waveform_probes module)
    pub waveform_probes: u8,
    /// Number of samples per waveform
    pub waveform_samples: usize,
}

impl Default for EmulatorConfig {
    fn default() -> Self {
        Self {
            address: "tcp://*:5555".to_string(),
            command_address: "tcp://*:5560".to_string(),
            source_id: 0,
            events_per_batch: 100,
            batch_interval_ms: 100,
            heartbeat_interval_ms: 1000, // 1Hz heartbeat
            num_modules: 1,
            channels_per_module: 16,
            enable_waveform: false,
            waveform_probes: waveform_probes::ALL_ANALOG, // analog_probe1 & 2 by default
            waveform_samples: 512,
        }
    }
}

/// Emulator errors
#[derive(Error, Debug)]
pub enum EmulatorError {
    #[error("ZMQ error: {0}")]
    Zmq(#[from] tmq::TmqError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] rmp_serde::encode::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Emulator data source
///
/// Generates random event data and publishes via ZeroMQ.
/// Supports command control via REP socket in separate task.
pub struct Emulator {
    config: EmulatorConfig,
    data_socket: publish::Publish,
    shared_state: Arc<Mutex<ComponentSharedState>>,
    state_rx: watch::Receiver<ComponentState>,
    state_tx: watch::Sender<ComponentState>,
    sequence_number: u64,
    timestamp_ns: f64,
    heartbeat_counter: u64,
}

impl Emulator {
    /// Create a new emulator with the given configuration
    pub async fn new(config: EmulatorConfig) -> Result<Self, EmulatorError> {
        let context = Context::new();
        let data_socket = publish(&context).bind(&config.address)?;

        info!(
            data_address = %config.address,
            command_address = %config.command_address,
            "Emulator bound to data address"
        );

        let (state_tx, state_rx) = watch::channel(ComponentState::Idle);

        Ok(Self {
            config,
            data_socket,
            shared_state: Arc::new(Mutex::new(ComponentSharedState::new())),
            state_rx,
            state_tx,
            sequence_number: 0,
            timestamp_ns: 0.0,
            heartbeat_counter: 0,
        })
    }

    /// Get current state
    pub fn state(&self) -> ComponentState {
        *self.state_rx.borrow()
    }

    /// Generate a simulated waveform
    ///
    /// Creates a realistic pulse shape: baseline -> fast rise -> exponential decay
    /// The pulse timing is randomized within the waveform window.
    fn generate_waveform(&self, energy: u16) -> Waveform {
        let mut rng = rand::thread_rng();
        let n = self.config.waveform_samples;
        let probes = self.config.waveform_probes;

        // Pulse parameters
        let baseline: i16 = rng.gen_range(-50..50); // Small baseline fluctuation
        let amplitude = (energy as f64 / 65535.0 * 8000.0) as i16; // Scale to ~8000 max
        let rise_time = 5; // samples
        let decay_tau = 50.0; // decay time constant in samples
        let pulse_start = rng.gen_range(n / 4..n / 2); // Random trigger position

        // Generate analog probe 1 (main signal)
        let analog_probe1 = if probes & waveform_probes::ANALOG_PROBE1 != 0 {
            (0..n)
                .map(|i| {
                    if i < pulse_start {
                        baseline
                    } else if i < pulse_start + rise_time {
                        // Fast linear rise
                        let frac = (i - pulse_start) as f64 / rise_time as f64;
                        baseline + (amplitude as f64 * frac) as i16
                    } else {
                        // Exponential decay
                        let t = (i - pulse_start - rise_time) as f64;
                        baseline + (amplitude as f64 * (-t / decay_tau).exp()) as i16
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        // Generate analog probe 2 (differentiated signal or second integration)
        let analog_probe2 = if probes & waveform_probes::ANALOG_PROBE2 != 0 {
            (0..n)
                .map(|i| {
                    if i < pulse_start || i >= pulse_start + rise_time + 100 {
                        0i16
                    } else if i < pulse_start + rise_time {
                        // Positive during rise
                        amplitude / 4
                    } else {
                        // Negative during decay
                        let t = (i - pulse_start - rise_time) as f64;
                        (-(amplitude as f64 / 4.0) * (-t / decay_tau).exp()) as i16
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        // Digital probes: packed bits (1 bit per sample)
        let digital_probe1 = if probes & waveform_probes::DIGITAL_PROBE1 != 0 {
            // Trigger signal: high during pulse
            let mut bits = vec![0u8; n.div_ceil(8)];
            for i in pulse_start..(pulse_start + 50).min(n) {
                bits[i / 8] |= 1 << (i % 8);
            }
            bits
        } else {
            Vec::new()
        };

        let digital_probe2 = if probes & waveform_probes::DIGITAL_PROBE2 != 0 {
            // Gate signal: high during integration window
            let mut bits = vec![0u8; n.div_ceil(8)];
            for i in pulse_start..(pulse_start + 100).min(n) {
                bits[i / 8] |= 1 << (i % 8);
            }
            bits
        } else {
            Vec::new()
        };

        let digital_probe3 = if probes & waveform_probes::DIGITAL_PROBE3 != 0 {
            // Short gate
            let mut bits = vec![0u8; n.div_ceil(8)];
            for i in pulse_start..(pulse_start + 30).min(n) {
                bits[i / 8] |= 1 << (i % 8);
            }
            bits
        } else {
            Vec::new()
        };

        let digital_probe4 = if probes & waveform_probes::DIGITAL_PROBE4 != 0 {
            // Pileup indicator (always low in this simple simulation)
            vec![0u8; n.div_ceil(8)]
        } else {
            Vec::new()
        };

        Waveform {
            analog_probe1,
            analog_probe2,
            digital_probe1,
            digital_probe2,
            digital_probe3,
            digital_probe4,
            time_resolution: 0, // 1x resolution
            trigger_threshold: 100,
        }
    }

    /// Generate a batch of random events with Gaussian peak + uniform background
    ///
    /// Energy distribution:
    /// - 70% Gaussian peak: mean = module * 1000 + channel * 50 + 500, sigma = 50
    /// - 30% Uniform background: 0 to 4095 (simulating random noise/cosmic rays)
    ///
    /// This creates distinct peaks for each channel with a realistic background,
    /// useful for testing fitting algorithms.
    fn generate_batch(&mut self) -> EventDataBatch {
        let mut rng = rand::thread_rng();
        let mut batch = EventDataBatch::with_capacity(
            self.config.source_id,
            self.sequence_number,
            self.config.events_per_batch,
        );

        // Module number = source_id (each emulator represents one digitizer module)
        let module = self.config.source_id as u8;

        // Background ratio: 30% uniform, 70% Gaussian peak
        const BACKGROUND_RATIO: f64 = 0.3;

        for _ in 0..self.config.events_per_batch {
            let channel = rng.gen_range(0..self.config.channels_per_module);

            let energy: u16 = if rng.gen_bool(BACKGROUND_RATIO) {
                // Uniform background: 0 to 4095 (12-bit ADC range)
                rng.gen_range(0..4096)
            } else {
                // Gaussian peak: mean = module*1000 + channel*50 + 500, sigma = 50
                let mean = (module as f64) * 1000.0 + (channel as f64) * 50.0 + 500.0;
                let sigma = 50.0;
                let normal = Normal::new(mean, sigma).unwrap();
                let energy_f64 = normal.sample(&mut rng);
                // Clamp to valid u16 range
                energy_f64.clamp(0.0, 65535.0) as u16
            };

            // Short gate energy: ~70-80% of long gate with some noise
            let short_ratio = 0.75 + rng.gen_range(-0.05..0.05);
            let energy_short: u16 = ((energy as f64) * short_ratio).clamp(0.0, 65535.0) as u16;

            self.timestamp_ns += rng.gen_range(10.0..1000.0);

            let flags = if rng.gen_ratio(1, 100) {
                flags::FLAG_PILEUP
            } else if rng.gen_ratio(1, 1000) {
                flags::FLAG_OVER_RANGE
            } else {
                0
            };

            let event = if self.config.enable_waveform {
                let waveform = self.generate_waveform(energy);
                EventData::with_waveform(
                    module,
                    channel,
                    energy,
                    energy_short,
                    self.timestamp_ns,
                    flags,
                    waveform,
                )
            } else {
                EventData::new(
                    module,
                    channel,
                    energy,
                    energy_short,
                    self.timestamp_ns,
                    flags,
                )
            };

            batch.push(event);
        }

        self.sequence_number += 1;
        batch
    }

    /// Publish a message via ZMQ
    async fn publish_message(&mut self, message: &Message) -> Result<(), EmulatorError> {
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
    async fn send_eos(&mut self) -> Result<(), EmulatorError> {
        let eos = Message::eos(self.config.source_id);
        self.publish_message(&eos).await
    }

    /// Send heartbeat message
    async fn send_heartbeat(&mut self) -> Result<(), EmulatorError> {
        let hb = Message::heartbeat(self.config.source_id, self.heartbeat_counter);
        self.heartbeat_counter += 1;
        self.publish_message(&hb).await
    }

    /// Run the emulator with command control
    ///
    /// Spawns command task in separate tokio task.
    /// Main task generates data when state is Running.
    /// If batch_interval_ms is 0, runs at full speed without delay.
    pub async fn run(
        &mut self,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), EmulatorError> {
        let use_ticker = self.config.batch_interval_ms > 0;
        let mut ticker = interval(Duration::from_millis(self.config.batch_interval_ms.max(1)));

        // Heartbeat ticker (only if enabled)
        let use_heartbeat = self.config.heartbeat_interval_ms > 0;
        let mut heartbeat_ticker = interval(Duration::from_millis(
            self.config.heartbeat_interval_ms.max(100),
        ));

        info!(
            source_id = self.config.source_id,
            state = %self.state(),
            batch_interval_ms = self.config.batch_interval_ms,
            heartbeat_interval_ms = self.config.heartbeat_interval_ms,
            "Emulator ready, waiting for commands"
        );

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
                |state, tx, cmd| handle_command_simple(state, tx, cmd, "Emulator"),
                "Emulator",
            )
            .await;
        });

        // Main data generation loop
        let mut state_rx = self.state_rx.clone();

        loop {
            if use_ticker {
                // Throttled mode: wait for interval
                tokio::select! {
                    biased;

                    _ = shutdown.recv() => {
                        info!("Emulator received shutdown signal");
                        break;
                    }

                    _ = state_rx.changed() => {
                        let current = *state_rx.borrow();
                        info!(state = %current, "State changed");
                        // Reset sequence number on Start
                        if current == ComponentState::Running {
                            self.sequence_number = 0;
                            self.timestamp_ns = 0.0;
                            self.heartbeat_counter = 0;
                            info!("Sequence number reset to 0 on Start");
                        }
                    }

                    _ = ticker.tick(), if *state_rx.borrow() == ComponentState::Running => {
                        let batch = self.generate_batch();
                        let msg = Message::data(batch);
                        self.publish_message(&msg).await?;
                    }

                    _ = heartbeat_ticker.tick(), if use_heartbeat && *state_rx.borrow() == ComponentState::Running => {
                        self.send_heartbeat().await?;
                    }
                }
            } else {
                // Full speed mode: no delay between batches
                // Check for shutdown/state change/heartbeat with zero timeout
                tokio::select! {
                    biased;

                    _ = shutdown.recv() => {
                        info!("Emulator received shutdown signal");
                        break;
                    }

                    _ = state_rx.changed() => {
                        let current = *state_rx.borrow();
                        info!(state = %current, "State changed");
                        // Reset sequence number on Start
                        if current == ComponentState::Running {
                            self.sequence_number = 0;
                            self.timestamp_ns = 0.0;
                            self.heartbeat_counter = 0;
                            info!("Sequence number reset to 0 on Start");
                        }
                        continue;
                    }

                    _ = heartbeat_ticker.tick(), if use_heartbeat && *state_rx.borrow() == ComponentState::Running => {
                        self.send_heartbeat().await?;
                        continue;
                    }

                    _ = tokio::time::sleep(Duration::ZERO) => {
                        // Immediate timeout - proceed to data generation
                    }
                }

                // Generate and send data if running
                if *state_rx.borrow() == ComponentState::Running {
                    let batch = self.generate_batch();
                    let msg = Message::data(batch);
                    self.publish_message(&msg).await?;
                } else {
                    // Idle: yield to avoid busy loop
                    tokio::task::yield_now().await;
                }
            }
        }

        // Send EOS if we were running
        if *self.state_rx.borrow() == ComponentState::Running {
            self.send_eos().await?;
        }

        // Wait for command task to finish
        let _ = cmd_handle.await;

        info!(total_batches = self.sequence_number, "Emulator stopped");
        Ok(())
    }

    /// Run for a fixed number of batches (useful for testing)
    ///
    /// Ignores command socket, immediately starts generating data.
    pub async fn run_batches(&mut self, count: u64) -> Result<(), EmulatorError> {
        let mut ticker = interval(Duration::from_millis(self.config.batch_interval_ms));

        for _ in 0..count {
            ticker.tick().await;
            let batch = self.generate_batch();
            let msg = Message::data(batch);
            self.publish_message(&msg).await?;
        }

        self.send_eos().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = EmulatorConfig::default();
        assert_eq!(config.events_per_batch, 100);
        assert_eq!(config.batch_interval_ms, 100);
        assert_eq!(config.command_address, "tcp://*:5560");
        assert_eq!(config.heartbeat_interval_ms, 1000);
        assert_eq!(config.num_modules, 1);
        assert_eq!(config.channels_per_module, 16);
        assert!(!config.enable_waveform);
        assert_eq!(config.waveform_probes, waveform_probes::ALL_ANALOG);
        assert_eq!(config.waveform_samples, 512);
    }

    #[test]
    fn generate_batch_size() {
        let config = EmulatorConfig {
            events_per_batch: 50,
            ..Default::default()
        };
        assert_eq!(config.events_per_batch, 50);
    }

    #[test]
    fn test_config_custom() {
        let config = EmulatorConfig {
            address: "tcp://*:6000".to_string(),
            command_address: "tcp://*:6001".to_string(),
            source_id: 42,
            events_per_batch: 200,
            batch_interval_ms: 50,
            heartbeat_interval_ms: 500,
            num_modules: 2,
            channels_per_module: 8,
            enable_waveform: true,
            waveform_probes: waveform_probes::ALL,
            waveform_samples: 1024,
        };
        assert_eq!(config.source_id, 42);
        assert_eq!(config.events_per_batch, 200);
        assert_eq!(config.batch_interval_ms, 50);
        assert_eq!(config.num_modules, 2);
        assert!(config.enable_waveform);
        assert_eq!(config.waveform_samples, 1024);
    }

    #[test]
    fn test_emulator_error_json() {
        // Test JSON error variant (easier to create than ZMQ errors)
        let invalid_json = "not valid json";
        let result: Result<serde_json::Value, _> = serde_json::from_str(invalid_json);
        if let Err(e) = result {
            let err: EmulatorError = e.into();
            let err_str = format!("{}", err);
            assert!(err_str.contains("JSON error"));
        }
    }

    #[test]
    fn test_emulator_error_debug() {
        // Test JSON error debug output
        let invalid_json = "not valid json";
        let result: Result<serde_json::Value, _> = serde_json::from_str(invalid_json);
        if let Err(e) = result {
            let err: EmulatorError = e.into();
            let debug = format!("{:?}", err);
            assert!(debug.contains("Json"));
        }
    }

    #[tokio::test]
    async fn test_emulator_creation() {
        // Use unique ports to avoid conflicts
        let config = EmulatorConfig {
            address: "tcp://127.0.0.1:15555".to_string(),
            command_address: "tcp://127.0.0.1:15560".to_string(),
            source_id: 0,
            ..Default::default()
        };

        let emulator = Emulator::new(config).await;
        assert!(emulator.is_ok());

        let emu = emulator.unwrap();
        assert_eq!(emu.state(), ComponentState::Idle);
    }

    #[tokio::test]
    async fn test_emulator_initial_state() {
        let config = EmulatorConfig {
            address: "tcp://127.0.0.1:15556".to_string(),
            command_address: "tcp://127.0.0.1:15561".to_string(),
            source_id: 1,
            ..Default::default()
        };

        let emulator = Emulator::new(config).await.unwrap();
        assert_eq!(emulator.state(), ComponentState::Idle);
        assert_eq!(emulator.sequence_number, 0);
        assert_eq!(emulator.heartbeat_counter, 0);
    }

    #[test]
    fn test_flag_constants() {
        // Verify flag constants are defined correctly
        assert_eq!(flags::FLAG_PILEUP, 1);
        assert_eq!(flags::FLAG_OVER_RANGE, 4);
    }

    #[test]
    fn test_message_data_creation() {
        let batch = EventDataBatch::with_capacity(0, 0, 10);
        let msg = Message::data(batch);
        match msg {
            Message::Data(_) => (),
            _ => panic!("Expected Data message"),
        }
    }

    #[test]
    fn test_message_eos_creation() {
        let msg = Message::eos(42);
        match msg {
            Message::EndOfStream { source_id } => {
                assert_eq!(source_id, 42);
            }
            _ => panic!("Expected EndOfStream message"),
        }
    }

    #[test]
    fn test_message_heartbeat_creation() {
        let msg = Message::heartbeat(1, 100);
        match msg {
            Message::Heartbeat(hb) => {
                assert_eq!(hb.source_id, 1);
                assert_eq!(hb.counter, 100);
            }
            _ => panic!("Expected Heartbeat message"),
        }
    }
}
