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
use thiserror::Error;
use tmq::{publish, request_reply, Context};
use tokio::sync::{watch, Mutex};
use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::common::{
    flags, Command, CommandResponse, ComponentState, Message, MinimalEventData,
    MinimalEventDataBatch, RunConfig,
};

/// Emulator configuration
#[derive(Debug, Clone)]
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

/// Shared state between main task and command task
struct SharedState {
    state: ComponentState,
    run_config: Option<RunConfig>,
}

impl SharedState {
    fn new() -> Self {
        Self {
            state: ComponentState::Idle,
            run_config: None,
        }
    }
}

/// Emulator data source
///
/// Generates random event data and publishes via ZeroMQ.
/// Supports command control via REP socket in separate task.
pub struct Emulator {
    config: EmulatorConfig,
    data_socket: publish::Publish,
    shared_state: Arc<Mutex<SharedState>>,
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
            shared_state: Arc::new(Mutex::new(SharedState::new())),
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

    /// Generate a batch of random events
    ///
    /// Module number is set to source_id (each emulator = one module)
    fn generate_batch(&mut self) -> MinimalEventDataBatch {
        let mut rng = rand::thread_rng();
        let mut batch = MinimalEventDataBatch::with_capacity(
            self.config.source_id,
            self.sequence_number,
            self.config.events_per_batch,
        );

        // Module number = source_id (each emulator represents one digitizer module)
        let module = self.config.source_id as u8;

        for _ in 0..self.config.events_per_batch {
            let channel = rng.gen_range(0..self.config.channels_per_module);
            let energy: u16 = rng.gen_range(100..4000);
            let energy_short: u16 = (energy as f32 * rng.gen_range(0.6..0.9)) as u16;
            self.timestamp_ns += rng.gen_range(10.0..1000.0);

            let flags = if rng.gen_ratio(1, 100) {
                flags::FLAG_PILEUP
            } else if rng.gen_ratio(1, 1000) {
                flags::FLAG_OVER_RANGE
            } else {
                0
            };

            batch.push(MinimalEventData::new(
                module,
                channel,
                energy,
                energy_short,
                self.timestamp_ns,
                flags,
            ));
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
                debug!(source_id = hb.source_id, counter = hb.counter, "Published heartbeat");
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

    /// Handle incoming command (5-state machine)
    fn handle_command(
        state: &mut SharedState,
        state_tx: &watch::Sender<ComponentState>,
        cmd: Command,
    ) -> CommandResponse {
        let current = state.state;

        match cmd {
            Command::Configure(run_config) => {
                if !current.can_transition_to(ComponentState::Configured) {
                    return CommandResponse::error(
                        current,
                        format!("Cannot configure from {} state", current),
                    );
                }
                let run_number = run_config.run_number;
                state.run_config = Some(run_config);
                state.state = ComponentState::Configured;
                let _ = state_tx.send(ComponentState::Configured);
                info!(run_number, "Emulator configured");
                CommandResponse::success_with_run(ComponentState::Configured, "Configured", run_number)
            }

            Command::Arm => {
                if !current.can_transition_to(ComponentState::Armed) {
                    return CommandResponse::error(
                        current,
                        format!("Cannot arm from {} state", current),
                    );
                }
                state.state = ComponentState::Armed;
                let _ = state_tx.send(ComponentState::Armed);
                info!("Emulator armed");
                let run_number = state.run_config.as_ref().map(|c| c.run_number).unwrap_or(0);
                CommandResponse::success_with_run(ComponentState::Armed, "Armed", run_number)
            }

            Command::Start => {
                if !current.can_transition_to(ComponentState::Running) {
                    return CommandResponse::error(
                        current,
                        format!("Cannot start from {} state", current),
                    );
                }
                state.state = ComponentState::Running;
                let _ = state_tx.send(ComponentState::Running);
                info!("Emulator started");
                let run_number = state.run_config.as_ref().map(|c| c.run_number).unwrap_or(0);
                CommandResponse::success_with_run(ComponentState::Running, "Started", run_number)
            }

            Command::Stop => {
                if current != ComponentState::Running {
                    return CommandResponse::error(current, "Not running");
                }
                // Stop returns to Configured for quick restart
                state.state = ComponentState::Configured;
                let _ = state_tx.send(ComponentState::Configured);
                info!("Emulator stopped");
                let run_number = state.run_config.as_ref().map(|c| c.run_number).unwrap_or(0);
                CommandResponse::success_with_run(ComponentState::Configured, "Stopped", run_number)
            }

            Command::Reset => {
                state.state = ComponentState::Idle;
                state.run_config = None;
                let _ = state_tx.send(ComponentState::Idle);
                info!("Emulator reset");
                CommandResponse::success(ComponentState::Idle, "Reset to Idle")
            }

            Command::GetStatus => {
                let msg = if let Some(ref cfg) = state.run_config {
                    format!("State: {}, Run: {}", state.state, cfg.run_number)
                } else {
                    format!("State: {}", state.state)
                };
                let mut resp = CommandResponse::success(state.state, msg);
                resp.run_number = state.run_config.as_ref().map(|c| c.run_number);
                resp
            }
        }
    }

    /// Command handler task using tmq REQ/REP pattern
    ///
    /// tmq's REP socket uses a state machine pattern:
    /// recv() returns (Multipart, RequestSender)
    /// send() on RequestSender returns RequestReceiver for next cycle
    async fn command_task(
        command_address: String,
        shared_state: Arc<Mutex<SharedState>>,
        state_tx: watch::Sender<ComponentState>,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) {
        let context = Context::new();

        // Bind REP socket
        let receiver = match request_reply::reply(&context).bind(&command_address) {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "Failed to bind command socket");
                return;
            }
        };

        info!(address = %command_address, "Command task started");

        // REP socket state machine: must recv then send alternately
        let mut current_receiver = receiver;

        loop {
            tokio::select! {
                biased;

                _ = shutdown.recv() => {
                    info!("Command task received shutdown signal");
                    break;
                }

                // Receive command
                recv_result = current_receiver.recv() => {
                    match recv_result {
                        Ok((mut multipart, sender)) => {
                            // Process command
                            let response = if let Some(frame) = multipart.pop_front() {
                                match Command::from_json(&frame) {
                                    Ok(cmd) => {
                                        info!(command = %cmd, "Received command");
                                        let mut state = shared_state.lock().await;
                                        Self::handle_command(&mut state, &state_tx, cmd)
                                    }
                                    Err(e) => {
                                        warn!(error = %e, "Invalid command");
                                        let state = shared_state.lock().await;
                                        CommandResponse::error(state.state, format!("Invalid: {}", e))
                                    }
                                }
                            } else {
                                let state = shared_state.lock().await;
                                CommandResponse::error(state.state, "Empty message")
                            };

                            // Send response
                            let resp_bytes = match response.to_json() {
                                Ok(b) => b,
                                Err(e) => {
                                    warn!(error = %e, "Failed to serialize response");
                                    // Can't continue without sending - break out
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

        info!("Command task stopped");
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

        // Spawn command handler task
        let command_address = self.config.command_address.clone();
        let shared_state = self.shared_state.clone();
        let state_tx = self.state_tx.clone();
        let shutdown_for_cmd = shutdown.resubscribe();

        let cmd_handle = tokio::spawn(async move {
            Self::command_task(command_address, shared_state, state_tx, shutdown_for_cmd).await;
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
    }

    #[test]
    fn generate_batch_size() {
        let config = EmulatorConfig {
            events_per_batch: 50,
            ..Default::default()
        };
        assert_eq!(config.events_per_batch, 50);
    }
}
