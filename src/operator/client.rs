//! ZeroMQ client for communicating with DAQ components

use std::collections::BTreeMap;
use std::time::Duration;

use futures::future::join_all;
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

    /// Send start command to a component with run number
    pub async fn start(&self, config: &ComponentConfig, run_number: u32) -> CommandResult {
        self.execute_command(config, Command::Start { run_number })
            .await
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

    /// Configure all components with parallel execution for same pipeline_order
    ///
    /// Components with the same pipeline_order are configured in parallel.
    /// Order doesn't matter for Configure (no data flow yet), but we maintain
    /// consistency with start ordering.
    pub async fn configure_all(
        &self,
        configs: &[ComponentConfig],
        run_config: RunConfig,
    ) -> Vec<CommandResult> {
        // Group by pipeline_order (descending for consistency with start)
        let groups = Self::group_by_pipeline_order_desc(configs);

        tracing::info!(
            "Configure order: {:?}",
            groups
                .iter()
                .map(|(order, cfgs)| (
                    *order,
                    cfgs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>()
        );

        let mut results = Vec::with_capacity(configs.len());

        for (order, group_configs) in groups {
            let names: Vec<_> = group_configs.iter().map(|c| c.name.as_str()).collect();
            tracing::info!("Configuring group order={}: {:?} in parallel...", order, names);

            // Configure all components in this group in parallel
            let futures: Vec<_> = group_configs
                .iter()
                .map(|config| self.configure(config, run_config.clone()))
                .collect();
            let group_results = join_all(futures).await;

            results.extend(group_results);
        }

        results
    }

    /// Arm all components with parallel execution for same pipeline_order
    pub async fn arm_all(&self, configs: &[ComponentConfig]) -> Vec<CommandResult> {
        // Group by pipeline_order (descending for consistency)
        let groups = Self::group_by_pipeline_order_desc(configs);

        tracing::info!(
            "Arm order: {:?}",
            groups
                .iter()
                .map(|(order, cfgs)| (
                    *order,
                    cfgs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>()
        );

        let mut results = Vec::with_capacity(configs.len());

        for (order, group_configs) in groups {
            let names: Vec<_> = group_configs.iter().map(|c| c.name.as_str()).collect();
            tracing::info!("Arming group order={}: {:?} in parallel...", order, names);

            // Arm all components in this group in parallel
            let futures: Vec<_> = group_configs
                .iter()
                .map(|config| self.arm(config))
                .collect();
            let group_results = join_all(futures).await;

            results.extend(group_results);
        }

        results
    }

    /// Start all components in pipeline order (descending: downstream first)
    ///
    /// NOTE: This sends start commands sequentially but does NOT wait for each
    /// component to reach Running state. For synchronized startup where each
    /// component reaches Running before starting the next, use start_all_sequential.
    pub async fn start_all(
        &self,
        configs: &[ComponentConfig],
        run_number: u32,
    ) -> Vec<CommandResult> {
        // Sort by pipeline_order descending (downstream first, then upstream)
        let mut sorted: Vec<_> = configs.iter().collect();
        sorted.sort_by(|a, b| b.pipeline_order.cmp(&a.pipeline_order));

        // Log the start order for debugging
        tracing::info!(
            "Start order (downstream first): {:?}",
            sorted.iter().map(|c| (&c.name, c.pipeline_order)).collect::<Vec<_>>()
        );

        let mut results = Vec::with_capacity(configs.len());
        for config in sorted {
            tracing::info!("Starting {} (pipeline_order={})", config.name, config.pipeline_order);
            results.push(self.start(config, run_number).await);
        }
        results
    }

    /// Start all components in pipeline order, with parallel execution for same order.
    ///
    /// Components with the same pipeline_order are started in parallel, then we wait
    /// for all of them to reach Running before proceeding to the next order group.
    /// This prevents data buffer overflow while maximizing parallelism.
    ///
    /// Example: order=3 [Recorder, Monitor] → parallel start, wait all Running
    ///          order=2 [Merger] → start, wait Running
    ///          order=1 [Emulator-0, Emulator-1] → parallel start, wait all Running
    pub async fn start_all_sequential(
        &self,
        configs: &[ComponentConfig],
        run_number: u32,
        per_component_timeout_ms: u64,
    ) -> Result<Vec<CommandResult>, String> {
        // Group by pipeline_order (descending: downstream first)
        let groups = Self::group_by_pipeline_order_desc(configs);

        tracing::info!(
            "Start order (downstream first): {:?}",
            groups
                .iter()
                .map(|(order, cfgs)| (
                    *order,
                    cfgs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>()
        );

        let mut results = Vec::with_capacity(configs.len());

        for (order, group_configs) in groups {
            let names: Vec<_> = group_configs.iter().map(|c| c.name.as_str()).collect();
            tracing::info!(
                "Starting group order={}: {:?} in parallel...",
                order,
                names
            );

            // Start all components in this group in parallel
            let futures: Vec<_> = group_configs
                .iter()
                .map(|config| self.start(config, run_number))
                .collect();
            let group_results = join_all(futures).await;

            // Check for failures - find first failure and build error message
            let error_msg = group_results
                .iter()
                .find(|r| !r.success)
                .map(|f| format!("Failed to start {}: {}", f.name, f.message));

            results.extend(group_results);

            if let Some(msg) = error_msg {
                return Err(msg);
            }

            // Wait for all components in this group to reach Running
            self.wait_for_state(&group_configs, ComponentState::Running, per_component_timeout_ms)
                .await
                .map_err(|e| format!("Group order={} failed to reach Running: {}", order, e))?;

            tracing::info!("Group order={} ({:?}) all Running", order, names);
        }

        Ok(results)
    }

    /// Group components by pipeline_order in descending order (for Start: downstream first)
    fn group_by_pipeline_order_desc(configs: &[ComponentConfig]) -> Vec<(u32, Vec<ComponentConfig>)> {
        let mut groups: BTreeMap<u32, Vec<ComponentConfig>> = BTreeMap::new();
        for config in configs {
            groups
                .entry(config.pipeline_order)
                .or_default()
                .push(config.clone());
        }
        // Convert to Vec and reverse for descending order
        let mut result: Vec<_> = groups.into_iter().collect();
        result.reverse();
        result
    }

    /// Group components by pipeline_order in ascending order (for Stop: upstream first)
    fn group_by_pipeline_order_asc(configs: &[ComponentConfig]) -> Vec<(u32, Vec<ComponentConfig>)> {
        let mut groups: BTreeMap<u32, Vec<ComponentConfig>> = BTreeMap::new();
        for config in configs {
            groups
                .entry(config.pipeline_order)
                .or_default()
                .push(config.clone());
        }
        groups.into_iter().collect()
    }

    /// Stop all components in pipeline order (ascending: upstream first)
    /// with parallel execution for same pipeline_order
    pub async fn stop_all(&self, configs: &[ComponentConfig]) -> Vec<CommandResult> {
        // Group by pipeline_order (ascending: upstream first)
        let groups = Self::group_by_pipeline_order_asc(configs);

        tracing::info!(
            "Stop order (upstream first): {:?}",
            groups
                .iter()
                .map(|(order, cfgs)| (
                    *order,
                    cfgs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>()
        );

        let mut results = Vec::with_capacity(configs.len());

        for (order, group_configs) in groups {
            let names: Vec<_> = group_configs.iter().map(|c| c.name.as_str()).collect();
            tracing::info!("Stopping group order={}: {:?} in parallel...", order, names);

            // Stop all components in this group in parallel
            let futures: Vec<_> = group_configs
                .iter()
                .map(|config| self.stop(config))
                .collect();
            let group_results = join_all(futures).await;

            results.extend(group_results);
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

    /// Sequential start: start each component and wait for Running before next
    ///
    /// This ensures downstream components (Recorder, Monitor) are fully ready
    /// before upstream data producers (Emulator) start generating data.
    /// The timeout is per-component, not total.
    pub async fn start_all_sync(
        &self,
        configs: &[ComponentConfig],
        run_number: u32,
        timeout_ms: u64,
    ) -> Result<Vec<CommandResult>, String> {
        // Use sequential start to prevent buffer overflow
        // Each component must reach Running before the next starts
        self.start_all_sequential(configs, run_number, timeout_ms).await
    }
}

impl Default for ComponentClient {
    fn default() -> Self {
        Self::new()
    }
}
