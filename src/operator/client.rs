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

/// What was still sitting in the pipeline when a drain wait gave up (TODO 68).
#[derive(Debug, Clone)]
pub struct DrainResidual {
    /// Set when the wait aborted on an offline/Error component (vs timeout).
    pub error: Option<String>,
    /// (component name, queue_bytes, queue_items) with residual data.
    pub per_component: Vec<(String, u64, u64)>,
}

impl DrainResidual {
    /// Human-readable summary for logs / API responses / ELOG.
    pub fn summary(&self) -> String {
        let residual: Vec<String> = self
            .per_component
            .iter()
            .map(|(name, bytes, items)| format!("{}: {} bytes / {} batches", name, bytes, items))
            .collect();
        let residual = if residual.is_empty() {
            "no residual backlog observed".to_string()
        } else {
            residual.join("; ")
        };
        match &self.error {
            Some(e) => format!("{} ({})", e, residual),
            None => format!("drain timeout — {}", residual),
        }
    }
}

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
    pub async fn send_command(
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
        let role = if config.source_id.is_some() {
            "source"
        } else {
            "pipeline"
        }
        .to_string();

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
                role,
            },
            Err(e) => ComponentStatus {
                name: config.name.clone(),
                address: config.address.clone(),
                state: ComponentState::Idle,
                run_number: None,
                metrics: None,
                error: Some(e),
                online: false,
                role,
            },
        }
    }

    /// Get status of multiple components (parallel)
    pub async fn get_all_status(&self, configs: &[ComponentConfig]) -> Vec<ComponentStatus> {
        let futures: Vec<_> = configs.iter().map(|c| self.get_status(c)).collect();
        join_all(futures).await
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

    /// Execute command on all components (parallel)
    pub async fn execute_on_all(
        &self,
        configs: &[ComponentConfig],
        command_fn: impl Fn(&ComponentConfig) -> Command,
    ) -> Vec<CommandResult> {
        let futures: Vec<_> = configs
            .iter()
            .map(|config| self.execute_command(config, command_fn(config)))
            .collect();
        join_all(futures).await
    }

    /// Configure all components with parallel execution for same pipeline_order
    pub async fn configure_all(
        &self,
        configs: &[ComponentConfig],
        run_config: RunConfig,
    ) -> Vec<CommandResult> {
        self.execute_on_pipeline_groups(configs, true, "Configure", |_| {
            Command::Configure(run_config.clone())
        })
        .await
    }

    /// Arm all components with parallel execution for same pipeline_order
    pub async fn arm_all(&self, configs: &[ComponentConfig]) -> Vec<CommandResult> {
        self.execute_on_pipeline_groups(configs, true, "Arm", |_| Command::Arm)
            .await
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
            sorted
                .iter()
                .map(|c| (&c.name, c.pipeline_order))
                .collect::<Vec<_>>()
        );

        let mut results = Vec::with_capacity(configs.len());
        for config in sorted {
            tracing::info!(
                "Starting {} (pipeline_order={})",
                config.name,
                config.pipeline_order
            );
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
    /// Each Reader's `startmode` parameter determines its actual behavior:
    /// - `START_MODE_SW`: Sends armacquisition (master — triggers TrgOut)
    /// - `START_MODE_S_IN`: No-op (slave — already armed, waits for S_IN)
    ///
    /// Example: order=3 [Recorder, Monitor] → parallel start, wait all Running
    ///          order=2 [Merger] → start, wait Running
    ///          order=1 [Dig0, Dig1, Dig2] → all get Start, wait all Running
    pub async fn start_all_sequential(
        &self,
        configs: &[ComponentConfig],
        run_number: u32,
        per_component_timeout_ms: u64,
    ) -> Result<Vec<CommandResult>, String> {
        // Group by pipeline_order (descending: downstream first)
        let groups = Self::group_by_pipeline_order(configs, true);

        tracing::info!(
            "Start order (downstream first): {:?}",
            groups
                .iter()
                .map(|(order, cfgs)| (
                    *order,
                    cfgs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>(),
        );

        let mut results = Vec::with_capacity(configs.len());

        for (order, group_configs) in groups {
            let start_names: Vec<_> = group_configs.iter().map(|c| c.name.as_str()).collect();
            tracing::info!(
                "Starting group order={}: {:?} in parallel...",
                order,
                start_names
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
            // (including slaves which will auto-start via TrgOut)
            self.wait_for_state(
                &group_configs,
                ComponentState::Running,
                per_component_timeout_ms,
            )
            .await
            .map_err(|e| format!("Group order={} failed to reach Running: {}", order, e))?;

            tracing::info!("Group order={} all Running", order);
        }

        Ok(results)
    }

    /// Group components by pipeline_order.
    /// If `descending` is true, returns groups in descending order (downstream first).
    /// If false, returns groups in ascending order (upstream first).
    fn group_by_pipeline_order(
        configs: &[ComponentConfig],
        descending: bool,
    ) -> Vec<(u32, Vec<ComponentConfig>)> {
        let mut groups: BTreeMap<u32, Vec<ComponentConfig>> = BTreeMap::new();
        for config in configs {
            groups
                .entry(config.pipeline_order)
                .or_default()
                .push(config.clone());
        }
        let mut result: Vec<_> = groups.into_iter().collect();
        if descending {
            result.reverse();
        }
        result
    }

    /// Execute a command on all components grouped by pipeline_order.
    ///
    /// Components with the same pipeline_order are executed in parallel.
    /// Groups are processed sequentially in the specified order.
    async fn execute_on_pipeline_groups(
        &self,
        configs: &[ComponentConfig],
        descending: bool,
        action_name: &str,
        command_fn: impl Fn(&ComponentConfig) -> Command,
    ) -> Vec<CommandResult> {
        let groups = Self::group_by_pipeline_order(configs, descending);
        let direction = if descending {
            "downstream first"
        } else {
            "upstream first"
        };

        tracing::info!(
            "{} order ({}): {:?}",
            action_name,
            direction,
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
                "{} group order={}: {:?} in parallel...",
                action_name,
                order,
                names
            );

            let futures: Vec<_> = group_configs
                .iter()
                .map(|config| self.execute_command(config, command_fn(config)))
                .collect();
            let group_results = join_all(futures).await;

            results.extend(group_results);
        }

        results
    }

    /// Stop all components in pipeline order (ascending: upstream first)
    /// with parallel execution for same pipeline_order
    pub async fn stop_all(&self, configs: &[ComponentConfig]) -> Vec<CommandResult> {
        self.execute_on_pipeline_groups(configs, false, "Stop", |_| Command::Stop)
            .await
    }

    /// Reset all components with parallel execution for same pipeline_order
    pub async fn reset_all(&self, configs: &[ComponentConfig]) -> Vec<CommandResult> {
        self.execute_on_pipeline_groups(configs, false, "Reset", |_| Command::Reset)
            .await
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

    /// Wait until every component's backlog channel is drained (TODO 68).
    ///
    /// Success requires `queue_bytes == 0` AND an unchanged
    /// `events_processed` on every component for three consecutive polls
    /// (~600 ms of quiescence). The stability window is load-bearing, not
    /// polish: (i) a Reader acks Stop *before* its ordered decode pipeline
    /// flushes the final EOS, so the merger queue can read 0 while data is
    /// still arriving; (ii) bytes in flight on the merger→recorder ZMQ hop
    /// are invisible to both gauges. Both windows close once nothing has
    /// moved for the full stability window with the sources stopped.
    ///
    /// Fails fast (like `wait_for_state`) if a component goes offline or
    /// reports Error. On timeout, returns the residual backlog observed on
    /// the last poll so the caller can report exactly what was lost.
    pub async fn wait_for_backlog_drained(
        &self,
        configs: &[ComponentConfig],
        timeout_ms: u64,
    ) -> Result<(), DrainResidual> {
        const STABLE_POLLS: u32 = 3;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        let poll_interval = Duration::from_millis(200);

        let mut stable_count: u32 = 0;
        let mut prev_events: Option<Vec<u64>> = None;

        loop {
            let statuses = self.get_all_status(configs).await;

            // Fail fast on offline/Error, mirroring wait_for_state.
            let errors: Vec<String> = statuses
                .iter()
                .filter(|s| !s.online || s.state == ComponentState::Error)
                .map(|s| {
                    if !s.online {
                        format!("{}: offline", s.name)
                    } else {
                        format!("{}: {}", s.name, s.error.as_deref().unwrap_or("error"))
                    }
                })
                .collect();
            if !errors.is_empty() {
                return Err(DrainResidual {
                    error: Some(format!(
                        "Component errors during drain: {}",
                        errors.join(", ")
                    )),
                    per_component: Self::residual_snapshot(&statuses),
                });
            }

            let events: Vec<u64> = statuses
                .iter()
                .map(|s| s.metrics.as_ref().map_or(0, |m| m.events_processed))
                .collect();
            let all_empty = statuses
                .iter()
                .all(|s| s.metrics.as_ref().is_none_or(|m| m.queue_bytes == 0));
            let quiescent = prev_events.as_ref() == Some(&events);

            if all_empty && quiescent {
                stable_count += 1;
                if stable_count >= STABLE_POLLS {
                    return Ok(());
                }
            } else {
                stable_count = 0;
            }
            prev_events = Some(events);

            if tokio::time::Instant::now() >= deadline {
                return Err(DrainResidual {
                    error: None,
                    per_component: Self::residual_snapshot(&statuses),
                });
            }
            tokio::time::sleep(poll_interval).await;
        }
    }

    /// (name, queue_bytes, queue_items) for every component still holding data.
    fn residual_snapshot(statuses: &[super::ComponentStatus]) -> Vec<(String, u64, u64)> {
        statuses
            .iter()
            .filter_map(|s| {
                let m = s.metrics.as_ref()?;
                if m.queue_bytes > 0 || m.queue_size > 0 {
                    Some((s.name.clone(), m.queue_bytes, m.queue_size as u64))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Check results and wait for all components to reach target state.
    /// Returns early with results if any command failed.
    async fn wait_after_execute(
        &self,
        configs: &[ComponentConfig],
        results: Vec<CommandResult>,
        target_state: ComponentState,
        timeout_ms: u64,
    ) -> Result<Vec<CommandResult>, String> {
        if results.iter().any(|r| !r.success) {
            return Ok(results);
        }
        self.wait_for_state(configs, target_state, timeout_ms)
            .await?;
        Ok(results)
    }

    /// Two-phase configure: send configure and wait for all to reach Configured
    pub async fn configure_all_sync(
        &self,
        configs: &[ComponentConfig],
        run_config: RunConfig,
        timeout_ms: u64,
    ) -> Result<Vec<CommandResult>, String> {
        let results = self.configure_all(configs, run_config).await;
        self.wait_after_execute(configs, results, ComponentState::Configured, timeout_ms)
            .await
    }

    /// Two-phase arm: send arm and wait for all to reach Armed
    pub async fn arm_all_sync(
        &self,
        configs: &[ComponentConfig],
        timeout_ms: u64,
    ) -> Result<Vec<CommandResult>, String> {
        let results = self.arm_all(configs).await;
        self.wait_after_execute(configs, results, ComponentState::Armed, timeout_ms)
            .await
    }

    /// Two-phase reset: send reset and wait for all to reach Idle
    pub async fn reset_all_sync(
        &self,
        configs: &[ComponentConfig],
        timeout_ms: u64,
    ) -> Result<Vec<CommandResult>, String> {
        let results = self.reset_all(configs).await;
        self.wait_after_execute(configs, results, ComponentState::Idle, timeout_ms)
            .await
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
        self.start_all_sequential(configs, run_number, timeout_ms)
            .await
    }
}

impl Default for ComponentClient {
    fn default() -> Self {
        Self::new()
    }
}
