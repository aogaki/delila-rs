//! ZeroMQ client for communicating with DAQ components

use std::time::Duration;

use tmq::{request_reply, Context};
use tokio::time::timeout;

use crate::common::{Command, CommandResponse, ComponentState, RunConfig};

use super::{CommandResult, ComponentConfig, ComponentStatus};

/// Timeout for ZMQ operations
const ZMQ_TIMEOUT: Duration = Duration::from_secs(5);

/// Client for communicating with DAQ components via ZMQ REQ/REP
pub struct ComponentClient {
    context: Context,
}

impl ComponentClient {
    /// Create a new component client
    pub fn new() -> Self {
        Self {
            context: Context::new(),
        }
    }

    /// Send a command to a single component and return the result
    async fn send_command(
        &self,
        address: &str,
        command: &Command,
    ) -> Result<CommandResponse, String> {
        // Create REQ socket and connect
        let requester = request_reply::request(&self.context)
            .connect(address)
            .map_err(|e| format!("Failed to connect to {}: {}", address, e))?;

        // Serialize command
        let cmd_bytes = command
            .to_json()
            .map_err(|e| format!("Failed to serialize command: {}", e))?;

        // Send command
        let msg: tmq::Multipart = vec![tmq::Message::from(cmd_bytes.as_slice())].into();
        let responder = timeout(ZMQ_TIMEOUT, requester.send(msg))
            .await
            .map_err(|_| format!("Timeout sending to {}", address))?
            .map_err(|e| format!("Failed to send to {}: {}", address, e))?;

        // Receive response
        let (mut response_msg, _) = timeout(ZMQ_TIMEOUT, responder.recv())
            .await
            .map_err(|_| format!("Timeout receiving from {}", address))?
            .map_err(|e| format!("Failed to receive from {}: {}", address, e))?;

        // Parse response
        if let Some(frame) = response_msg.pop_front() {
            CommandResponse::from_json(&frame)
                .map_err(|e| format!("Failed to parse response: {}", e))
        } else {
            Err("Empty response received".to_string())
        }
    }

    /// Get status of a single component
    pub async fn get_status(&self, config: &ComponentConfig) -> ComponentStatus {
        match self
            .send_command(&config.address, &Command::GetStatus)
            .await
        {
            Ok(response) => ComponentStatus {
                name: config.name.clone(),
                address: config.address.clone(),
                state: response.state,
                run_number: response.run_number,
                metrics: response.metrics,
                error: if response.state == ComponentState::Error {
                    Some(response.message)
                } else {
                    None
                },
                online: true,
            },
            Err(e) => ComponentStatus {
                name: config.name.clone(),
                address: config.address.clone(),
                state: ComponentState::Idle,
                run_number: None,
                metrics: None,
                error: Some(e),
                online: false,
            },
        }
    }

    /// Get status of multiple components
    pub async fn get_all_status(&self, configs: &[ComponentConfig]) -> Vec<ComponentStatus> {
        let mut statuses = Vec::with_capacity(configs.len());
        for config in configs {
            statuses.push(self.get_status(config).await);
        }
        statuses
    }

    /// Send configure command to a component
    pub async fn configure(
        &self,
        config: &ComponentConfig,
        run_config: RunConfig,
    ) -> CommandResult {
        self.execute_command(config, Command::Configure(run_config))
            .await
    }

    /// Send arm command to a component
    pub async fn arm(&self, config: &ComponentConfig) -> CommandResult {
        self.execute_command(config, Command::Arm).await
    }

    /// Send start command to a component
    pub async fn start(&self, config: &ComponentConfig) -> CommandResult {
        self.execute_command(config, Command::Start).await
    }

    /// Send stop command to a component
    pub async fn stop(&self, config: &ComponentConfig) -> CommandResult {
        self.execute_command(config, Command::Stop).await
    }

    /// Send reset command to a component
    pub async fn reset(&self, config: &ComponentConfig) -> CommandResult {
        self.execute_command(config, Command::Reset).await
    }

    /// Execute a command and return CommandResult
    async fn execute_command(&self, config: &ComponentConfig, command: Command) -> CommandResult {
        match self.send_command(&config.address, &command).await {
            Ok(response) => CommandResult {
                name: config.name.clone(),
                success: response.success,
                state: response.state,
                message: response.message,
            },
            Err(e) => CommandResult {
                name: config.name.clone(),
                success: false,
                state: ComponentState::Idle,
                message: e,
            },
        }
    }

    /// Execute command on all components
    pub async fn execute_on_all(
        &self,
        configs: &[ComponentConfig],
        command_fn: impl Fn(&ComponentConfig) -> Command,
    ) -> Vec<CommandResult> {
        let mut results = Vec::with_capacity(configs.len());
        for config in configs {
            let command = command_fn(config);
            results.push(self.execute_command(config, command).await);
        }
        results
    }

    /// Configure all components
    pub async fn configure_all(
        &self,
        configs: &[ComponentConfig],
        run_config: RunConfig,
    ) -> Vec<CommandResult> {
        let mut results = Vec::with_capacity(configs.len());
        for config in configs {
            results.push(self.configure(config, run_config.clone()).await);
        }
        results
    }

    /// Arm all components
    pub async fn arm_all(&self, configs: &[ComponentConfig]) -> Vec<CommandResult> {
        let mut results = Vec::with_capacity(configs.len());
        for config in configs {
            results.push(self.arm(config).await);
        }
        results
    }

    /// Start all components
    pub async fn start_all(&self, configs: &[ComponentConfig]) -> Vec<CommandResult> {
        let mut results = Vec::with_capacity(configs.len());
        for config in configs {
            results.push(self.start(config).await);
        }
        results
    }

    /// Stop all components
    pub async fn stop_all(&self, configs: &[ComponentConfig]) -> Vec<CommandResult> {
        let mut results = Vec::with_capacity(configs.len());
        for config in configs {
            results.push(self.stop(config).await);
        }
        results
    }

    /// Reset all components
    pub async fn reset_all(&self, configs: &[ComponentConfig]) -> Vec<CommandResult> {
        let mut results = Vec::with_capacity(configs.len());
        for config in configs {
            results.push(self.reset(config).await);
        }
        results
    }

    /// Wait for all components to reach the expected state
    /// Returns true if all reached the state, false if timeout
    pub async fn wait_for_state(
        &self,
        configs: &[ComponentConfig],
        expected_state: ComponentState,
        timeout_ms: u64,
    ) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        let poll_interval = Duration::from_millis(100);

        loop {
            let statuses = self.get_all_status(configs).await;

            // Check if all are in expected state
            let all_ready = statuses
                .iter()
                .all(|s| s.online && s.state == expected_state);
            if all_ready {
                return Ok(());
            }

            // Check for errors
            let errors: Vec<_> = statuses
                .iter()
                .filter(|s| !s.online || s.state == ComponentState::Error)
                .collect();
            if !errors.is_empty() {
                let error_msgs: Vec<_> = errors
                    .iter()
                    .map(|s| {
                        if !s.online {
                            format!("{}: offline", s.name)
                        } else {
                            format!("{}: {}", s.name, s.error.as_deref().unwrap_or("error"))
                        }
                    })
                    .collect();
                return Err(format!("Component errors: {}", error_msgs.join(", ")));
            }

            // Check timeout
            if tokio::time::Instant::now() >= deadline {
                let not_ready: Vec<_> = statuses
                    .iter()
                    .filter(|s| s.state != expected_state)
                    .map(|s| format!("{}: {:?}", s.name, s.state))
                    .collect();
                return Err(format!(
                    "Timeout waiting for {:?} state. Not ready: {}",
                    expected_state,
                    not_ready.join(", ")
                ));
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Two-phase configure: send configure and wait for all to reach Configured
    pub async fn configure_all_sync(
        &self,
        configs: &[ComponentConfig],
        run_config: RunConfig,
        timeout_ms: u64,
    ) -> Result<Vec<CommandResult>, String> {
        let results = self.configure_all(configs, run_config).await;

        // Check if any failed immediately
        if results.iter().any(|r| !r.success) {
            return Ok(results);
        }

        // Wait for all to reach Configured state
        self.wait_for_state(configs, ComponentState::Configured, timeout_ms)
            .await?;

        Ok(results)
    }

    /// Two-phase arm: send arm and wait for all to reach Armed
    pub async fn arm_all_sync(
        &self,
        configs: &[ComponentConfig],
        timeout_ms: u64,
    ) -> Result<Vec<CommandResult>, String> {
        let results = self.arm_all(configs).await;

        // Check if any failed immediately
        if results.iter().any(|r| !r.success) {
            return Ok(results);
        }

        // Wait for all to reach Armed state (sync point)
        self.wait_for_state(configs, ComponentState::Armed, timeout_ms)
            .await?;

        Ok(results)
    }

    /// Two-phase start: send start and wait for all to reach Running
    pub async fn start_all_sync(
        &self,
        configs: &[ComponentConfig],
        timeout_ms: u64,
    ) -> Result<Vec<CommandResult>, String> {
        let results = self.start_all(configs).await;

        // Check if any failed immediately
        if results.iter().any(|r| !r.success) {
            return Ok(results);
        }

        // Wait for all to reach Running state
        self.wait_for_state(configs, ComponentState::Running, timeout_ms)
            .await?;

        Ok(results)
    }
}

impl Default for ComponentClient {
    fn default() -> Self {
        Self::new()
    }
}
