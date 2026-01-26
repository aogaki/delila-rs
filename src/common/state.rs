//! Shared state and command handler infrastructure
//!
//! This module provides common state management and command handling
//! that is shared across all DAQ components (Emulator, Reader, Merger, DataSink).

use super::command::{Command, CommandResponse, ComponentState, EmulatorRuntimeConfig, RunConfig};
use tokio::sync::watch;
use tracing::info;

/// Shared state between component tasks
///
/// This struct holds the current state and run configuration that needs
/// to be shared between multiple tasks within a component.
#[derive(Debug, Clone)]
pub struct ComponentSharedState {
    /// Current component state (Idle, Configured, Armed, Running, Error)
    pub state: ComponentState,
    /// Current run configuration (if configured)
    pub run_config: Option<RunConfig>,
}

impl Default for ComponentSharedState {
    fn default() -> Self {
        Self::new()
    }
}

impl ComponentSharedState {
    /// Create a new shared state in Idle
    pub fn new() -> Self {
        Self {
            state: ComponentState::Idle,
            run_config: None,
        }
    }

    /// Get current run number (if configured)
    pub fn run_number(&self) -> Option<u32> {
        self.run_config.as_ref().map(|c| c.run_number)
    }
}

/// Trait for component-specific command handling extensions
///
/// Components implement this trait to add custom behavior to state transitions.
/// The default implementations do nothing, allowing components to override
/// only the hooks they need.
pub trait CommandHandlerExt {
    /// Component name for logging
    fn component_name(&self) -> &'static str;

    /// Called before Configure transition
    /// Return Err to reject the transition
    fn on_configure(&mut self, _config: &RunConfig) -> Result<(), String> {
        Ok(())
    }

    /// Called before Arm transition
    fn on_arm(&mut self) -> Result<(), String> {
        Ok(())
    }

    /// Called before Start transition
    /// run_number is provided to allow updating run number at start time
    fn on_start(&mut self, _run_number: u32) -> Result<(), String> {
        Ok(())
    }

    /// Called before Stop transition
    fn on_stop(&mut self) -> Result<(), String> {
        Ok(())
    }

    /// Called before Reset transition
    fn on_reset(&mut self) -> Result<(), String> {
        Ok(())
    }

    /// Get additional status details for GetStatus command
    fn status_details(&self) -> Option<String> {
        None
    }

    /// Get component metrics for GetStatus command
    /// Override this to return actual metrics from the component
    fn get_metrics(&self) -> Option<super::ComponentMetrics> {
        None
    }

    /// Called when UpdateEmulatorConfig command is received
    /// Only implemented by Emulator; other components return error
    fn on_update_emulator_config(&mut self, _config: &EmulatorRuntimeConfig) -> Result<(), String> {
        Err("UpdateEmulatorConfig not supported by this component".to_string())
    }
}

/// Handle a command using the 5-state machine logic
///
/// This function implements the common state machine logic used by all components.
/// Components can customize behavior by implementing `CommandHandlerExt`.
///
/// # Arguments
/// * `state` - Mutable reference to the shared state
/// * `state_tx` - Watch channel sender to broadcast state changes
/// * `cmd` - The command to handle
/// * `ext` - Optional extension trait for component-specific behavior
/// * `component_name` - Name of the component (for logging)
///
/// # Returns
/// A `CommandResponse` indicating success/failure and the new state
pub fn handle_command<E: CommandHandlerExt>(
    state: &mut ComponentSharedState,
    state_tx: &watch::Sender<ComponentState>,
    cmd: Command,
    mut ext: Option<&mut E>,
) -> CommandResponse {
    let current = state.state;
    let component_name = ext
        .as_ref()
        .map(|e| e.component_name())
        .unwrap_or("Component");

    match cmd {
        Command::Configure(run_config) => {
            if !current.can_transition_to(ComponentState::Configured) {
                return CommandResponse::error(
                    current,
                    format!("Cannot configure from {} state", current),
                );
            }

            // Call extension hook if provided
            if let Some(ref mut e) = ext {
                if let Err(msg) = e.on_configure(&run_config) {
                    return CommandResponse::error(current, msg);
                }
            }

            let run_number = run_config.run_number;
            state.run_config = Some(run_config);
            state.state = ComponentState::Configured;
            let _ = state_tx.send(ComponentState::Configured);

            info!(component = component_name, run_number, "Configured");
            CommandResponse::success_with_run(ComponentState::Configured, "Configured", run_number)
        }

        Command::Arm => {
            if !current.can_transition_to(ComponentState::Armed) {
                return CommandResponse::error(
                    current,
                    format!("Cannot arm from {} state", current),
                );
            }

            if let Some(ref mut e) = ext {
                if let Err(msg) = e.on_arm() {
                    return CommandResponse::error(current, msg);
                }
            }

            state.state = ComponentState::Armed;
            let _ = state_tx.send(ComponentState::Armed);

            info!(component = component_name, "Armed");
            let run_number = state.run_number().unwrap_or(0);
            CommandResponse::success_with_run(ComponentState::Armed, "Armed", run_number)
        }

        Command::Start { run_number } => {
            if !current.can_transition_to(ComponentState::Running) {
                return CommandResponse::error(
                    current,
                    format!("Cannot start from {} state", current),
                );
            }

            // Update run_number in run_config if it exists
            if let Some(ref mut cfg) = state.run_config {
                cfg.run_number = run_number;
            }

            if let Some(ref mut e) = ext {
                if let Err(msg) = e.on_start(run_number) {
                    return CommandResponse::error(current, msg);
                }
            }

            state.state = ComponentState::Running;
            let _ = state_tx.send(ComponentState::Running);

            info!(component = component_name, run_number, "Started");
            CommandResponse::success_with_run(ComponentState::Running, "Started", run_number)
        }

        Command::Stop => {
            if current != ComponentState::Running {
                return CommandResponse::error(current, "Not running");
            }

            if let Some(ref mut e) = ext {
                if let Err(msg) = e.on_stop() {
                    return CommandResponse::error(current, msg);
                }
            }

            state.state = ComponentState::Configured;
            let _ = state_tx.send(ComponentState::Configured);

            info!(component = component_name, "Stopped");
            let run_number = state.run_number().unwrap_or(0);
            CommandResponse::success_with_run(ComponentState::Configured, "Stopped", run_number)
        }

        Command::Reset => {
            if let Some(ref mut e) = ext {
                if let Err(msg) = e.on_reset() {
                    return CommandResponse::error(current, msg);
                }
            }

            state.state = ComponentState::Idle;
            state.run_config = None;
            let _ = state_tx.send(ComponentState::Idle);

            info!(component = component_name, "Reset");
            CommandResponse::success(ComponentState::Idle, "Reset to Idle")
        }

        Command::GetStatus => {
            let base_msg = if let Some(ref cfg) = state.run_config {
                format!("State: {}, Run: {}", state.state, cfg.run_number)
            } else {
                format!("State: {}", state.state)
            };

            let msg = if let Some(ref e) = ext {
                if let Some(details) = e.status_details() {
                    format!("{}, {}", base_msg, details)
                } else {
                    base_msg
                }
            } else {
                base_msg
            };

            let mut resp = CommandResponse::success(state.state, msg);
            resp.run_number = state.run_number();

            // Add metrics if available
            if let Some(ref e) = ext {
                if let Some(metrics) = e.get_metrics() {
                    resp = resp.with_metrics(metrics);
                }
            }

            resp
        }

        Command::UpdateEmulatorConfig(ref config) => {
            // This command can be received in any state
            if let Some(ref mut e) = ext {
                match e.on_update_emulator_config(config) {
                    Ok(()) => {
                        info!(
                            component = component_name,
                            events_per_batch = config.events_per_batch,
                            "Emulator config updated"
                        );
                        CommandResponse::success(current, "Config updated")
                    }
                    Err(msg) => CommandResponse::error(current, msg),
                }
            } else {
                CommandResponse::error(
                    current,
                    "UpdateEmulatorConfig not supported by this component",
                )
            }
        }
    }
}

/// Handle a command without extension hooks
///
/// Convenience function for components that don't need custom behavior.
pub fn handle_command_simple(
    state: &mut ComponentSharedState,
    state_tx: &watch::Sender<ComponentState>,
    cmd: Command,
    component_name: &'static str,
) -> CommandResponse {
    // Create a simple no-op extension
    struct SimpleExt(&'static str);
    impl CommandHandlerExt for SimpleExt {
        fn component_name(&self) -> &'static str {
            self.0
        }
    }

    let mut ext = SimpleExt(component_name);
    handle_command(state, state_tx, cmd, Some(&mut ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestComponent {
        configure_called: bool,
        arm_called: bool,
        start_called: bool,
        stop_called: bool,
        reset_called: bool,
    }

    impl TestComponent {
        fn new() -> Self {
            Self {
                configure_called: false,
                arm_called: false,
                start_called: false,
                stop_called: false,
                reset_called: false,
            }
        }
    }

    impl CommandHandlerExt for TestComponent {
        fn component_name(&self) -> &'static str {
            "TestComponent"
        }

        fn on_configure(&mut self, _config: &RunConfig) -> Result<(), String> {
            self.configure_called = true;
            Ok(())
        }

        fn on_arm(&mut self) -> Result<(), String> {
            self.arm_called = true;
            Ok(())
        }

        fn on_start(&mut self, _run_number: u32) -> Result<(), String> {
            self.start_called = true;
            Ok(())
        }

        fn on_stop(&mut self) -> Result<(), String> {
            self.stop_called = true;
            Ok(())
        }

        fn on_reset(&mut self) -> Result<(), String> {
            self.reset_called = true;
            Ok(())
        }

        fn status_details(&self) -> Option<String> {
            Some("custom details".to_string())
        }
    }

    #[test]
    fn test_state_transitions() {
        let mut state = ComponentSharedState::new();
        let (state_tx, _state_rx) = watch::channel(ComponentState::Idle);
        let mut ext = TestComponent::new();

        // Configure
        let config = RunConfig {
            run_number: 42,
            comment: "test".to_string(),
            exp_name: "TestExp".to_string(),
        };
        let resp = handle_command(
            &mut state,
            &state_tx,
            Command::Configure(config),
            Some(&mut ext),
        );
        assert!(resp.success);
        assert_eq!(state.state, ComponentState::Configured);
        assert!(ext.configure_called);

        // Arm
        let resp = handle_command(&mut state, &state_tx, Command::Arm, Some(&mut ext));
        assert!(resp.success);
        assert_eq!(state.state, ComponentState::Armed);
        assert!(ext.arm_called);

        // Start
        let resp = handle_command(
            &mut state,
            &state_tx,
            Command::Start { run_number: 42 },
            Some(&mut ext),
        );
        assert!(resp.success);
        assert_eq!(state.state, ComponentState::Running);
        assert!(ext.start_called);

        // Stop
        let resp = handle_command(&mut state, &state_tx, Command::Stop, Some(&mut ext));
        assert!(resp.success);
        assert_eq!(state.state, ComponentState::Configured);
        assert!(ext.stop_called);

        // Reset
        let resp = handle_command(&mut state, &state_tx, Command::Reset, Some(&mut ext));
        assert!(resp.success);
        assert_eq!(state.state, ComponentState::Idle);
        assert!(ext.reset_called);
    }

    #[test]
    fn test_invalid_transition() {
        let mut state = ComponentSharedState::new();
        let (state_tx, _state_rx) = watch::channel(ComponentState::Idle);

        // Cannot start from Idle
        let resp = handle_command_simple(
            &mut state,
            &state_tx,
            Command::Start { run_number: 1 },
            "Test",
        );
        assert!(!resp.success);
        assert_eq!(state.state, ComponentState::Idle);
    }

    #[test]
    fn test_status_with_details() {
        let mut state = ComponentSharedState::new();
        state.run_config = Some(RunConfig {
            run_number: 99,
            comment: "".to_string(),
            exp_name: "".to_string(),
        });
        state.state = ComponentState::Running;

        let (state_tx, _state_rx) = watch::channel(ComponentState::Running);
        let mut ext = TestComponent::new();

        let resp = handle_command(&mut state, &state_tx, Command::GetStatus, Some(&mut ext));
        assert!(resp.success);
        assert!(resp.message.contains("custom details"));
        assert_eq!(resp.run_number, Some(99));
    }

    #[test]
    fn test_simple_handler() {
        let mut state = ComponentSharedState::new();
        let (state_tx, _state_rx) = watch::channel(ComponentState::Idle);

        let config = RunConfig {
            run_number: 1,
            comment: "".to_string(),
            exp_name: "".to_string(),
        };
        let resp =
            handle_command_simple(&mut state, &state_tx, Command::Configure(config), "Simple");
        assert!(resp.success);
        assert_eq!(state.state, ComponentState::Configured);
    }
}
