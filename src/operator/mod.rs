//! Operator module - REST API for DAQ control
//!
//! Provides HTTP endpoints to control DAQ components via ZeroMQ.
//! Includes Swagger UI for API documentation.

mod client;
mod routes;
mod run_repository;

pub use client::ComponentClient;
pub use routes::{create_router, create_router_with_config, create_router_with_mongodb};
pub use run_repository::{
    CurrentRunInfo, ErrorLogEntry, LastRunInfo, RepositoryError, RunDocument, RunNote,
    RunRepository, RunStats, RunStatus,
};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::common::{ComponentMetrics, ComponentState, RunConfig};

/// Component status returned by status endpoint
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ComponentStatus {
    /// Component name
    pub name: String,
    /// ZMQ address
    pub address: String,
    /// Current state
    pub state: ComponentState,
    /// Current run number (if configured)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_number: Option<u32>,
    /// Component metrics (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<ComponentMetrics>,
    /// Error message (if in error state)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether communication succeeded
    pub online: bool,
}

/// System-wide status
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SystemStatus {
    /// All component statuses
    pub components: Vec<ComponentStatus>,
    /// Overall system state (derived from components)
    pub system_state: SystemState,
    /// Current run information (if a run is active)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_info: Option<CurrentRunInfo>,
    /// Experiment name (server-authoritative, from config file)
    pub experiment_name: String,
    /// Next run number (from MongoDB, for multi-client sync)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_number: Option<i32>,
    /// Last run info for pre-filling comment (comment + notes from previous run)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_info: Option<LastRunInfo>,
}

/// Aggregated system state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum SystemState {
    /// All components are Idle
    Idle,
    /// All components are Configured
    Configured,
    /// All components are Armed
    Armed,
    /// All components are Running
    Running,
    /// At least one component is in Error
    Error,
    /// Components are in mixed states
    Mixed,
    /// At least one component is offline
    Degraded,
}

impl SystemState {
    /// Derive system state from component statuses
    pub fn from_components(components: &[ComponentStatus]) -> Self {
        if components.is_empty() {
            return SystemState::Idle;
        }

        // Check for offline or error first
        if components.iter().any(|c| !c.online) {
            return SystemState::Degraded;
        }
        if components.iter().any(|c| c.state == ComponentState::Error) {
            return SystemState::Error;
        }

        // Check if all are in same state
        let first_state = components[0].state;
        if components.iter().all(|c| c.state == first_state) {
            match first_state {
                ComponentState::Idle => SystemState::Idle,
                ComponentState::Configured => SystemState::Configured,
                ComponentState::Armed => SystemState::Armed,
                ComponentState::Running => SystemState::Running,
                ComponentState::Error => SystemState::Error,
            }
        } else {
            SystemState::Mixed
        }
    }
}

/// Request body for configure command
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ConfigureRequest {
    /// Run number
    pub run_number: u32,
    /// Optional comment
    #[serde(default)]
    pub comment: String,
    /// Experiment name (used in output filenames)
    #[serde(default)]
    pub exp_name: String,
}

impl From<ConfigureRequest> for RunConfig {
    fn from(req: ConfigureRequest) -> Self {
        RunConfig {
            run_number: req.run_number,
            comment: req.comment,
            exp_name: req.exp_name,
        }
    }
}

/// Request body for start command
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StartRequest {
    /// Run number to use for this start (allows changing run number without re-configure)
    pub run_number: u32,
    /// Comment for this run (optional, stored in MongoDB)
    #[serde(default)]
    pub comment: String,
}

/// Generic API response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiResponse {
    /// Whether the operation succeeded
    pub success: bool,
    /// Human-readable message
    pub message: String,
    /// Component results (for batch operations)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results: Option<Vec<CommandResult>>,
}

/// Result of a command sent to a single component
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CommandResult {
    /// Component name
    pub name: String,
    /// Whether command succeeded
    pub success: bool,
    /// New state after command
    pub state: ComponentState,
    /// Message from component
    pub message: String,
}

impl ApiResponse {
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
            results: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            results: None,
        }
    }

    pub fn with_results(mut self, results: Vec<CommandResult>) -> Self {
        let all_success = results.iter().all(|r| r.success);
        self.success = all_success;
        self.results = Some(results);
        self
    }
}

/// Component configuration (from config file)
#[derive(Debug, Clone)]
pub struct ComponentConfig {
    pub name: String,
    pub address: String,
    /// Pipeline order: position in data flow (1 = upstream/source, higher = downstream)
    /// - Start: descending order (downstream first, then upstream)
    /// - Stop: ascending order (upstream first, then downstream)
    pub pipeline_order: u32,
}

/// Operator configuration with timeouts
#[derive(Debug, Clone)]
pub struct OperatorConfig {
    /// Timeout for configure phase (ms)
    pub configure_timeout_ms: u64,
    /// Timeout for arm phase (ms)
    pub arm_timeout_ms: u64,
    /// Timeout for start phase (ms)
    pub start_timeout_ms: u64,
    /// Experiment name (server-authoritative, from config file)
    pub experiment_name: String,
}

impl Default for OperatorConfig {
    fn default() -> Self {
        Self {
            configure_timeout_ms: 5000,
            arm_timeout_ms: 5000,
            start_timeout_ms: 5000,
            experiment_name: "DefaultExp".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_status(name: &str, state: ComponentState, online: bool) -> ComponentStatus {
        ComponentStatus {
            name: name.to_string(),
            address: format!("tcp://localhost:555{}", name.len()),
            state,
            run_number: None,
            metrics: None,
            error: if state == ComponentState::Error {
                Some("Test error".to_string())
            } else {
                None
            },
            online,
        }
    }

    #[test]
    fn test_system_state_empty_components() {
        let components: Vec<ComponentStatus> = vec![];
        assert_eq!(SystemState::from_components(&components), SystemState::Idle);
    }

    #[test]
    fn test_system_state_all_idle() {
        let components = vec![
            make_status("A", ComponentState::Idle, true),
            make_status("B", ComponentState::Idle, true),
        ];
        assert_eq!(SystemState::from_components(&components), SystemState::Idle);
    }

    #[test]
    fn test_system_state_all_configured() {
        let components = vec![
            make_status("A", ComponentState::Configured, true),
            make_status("B", ComponentState::Configured, true),
        ];
        assert_eq!(
            SystemState::from_components(&components),
            SystemState::Configured
        );
    }

    #[test]
    fn test_system_state_all_armed() {
        let components = vec![
            make_status("A", ComponentState::Armed, true),
            make_status("B", ComponentState::Armed, true),
        ];
        assert_eq!(
            SystemState::from_components(&components),
            SystemState::Armed
        );
    }

    #[test]
    fn test_system_state_all_running() {
        let components = vec![
            make_status("A", ComponentState::Running, true),
            make_status("B", ComponentState::Running, true),
        ];
        assert_eq!(
            SystemState::from_components(&components),
            SystemState::Running
        );
    }

    #[test]
    fn test_system_state_mixed() {
        let components = vec![
            make_status("A", ComponentState::Idle, true),
            make_status("B", ComponentState::Running, true),
        ];
        assert_eq!(
            SystemState::from_components(&components),
            SystemState::Mixed
        );
    }

    #[test]
    fn test_system_state_degraded_offline() {
        let components = vec![
            make_status("A", ComponentState::Running, true),
            make_status("B", ComponentState::Running, false), // offline
        ];
        assert_eq!(
            SystemState::from_components(&components),
            SystemState::Degraded
        );
    }

    #[test]
    fn test_system_state_error() {
        let components = vec![
            make_status("A", ComponentState::Running, true),
            make_status("B", ComponentState::Error, true),
        ];
        assert_eq!(
            SystemState::from_components(&components),
            SystemState::Error
        );
    }

    #[test]
    fn test_system_state_degraded_takes_priority_over_error() {
        // If a component is offline, we report Degraded (can't know true state)
        let components = vec![
            make_status("A", ComponentState::Error, true),
            make_status("B", ComponentState::Running, false), // offline
        ];
        assert_eq!(
            SystemState::from_components(&components),
            SystemState::Degraded
        );
    }

    #[test]
    fn test_api_response_success() {
        let resp = ApiResponse::success("OK");
        assert!(resp.success);
        assert_eq!(resp.message, "OK");
        assert!(resp.results.is_none());
    }

    #[test]
    fn test_api_response_error() {
        let resp = ApiResponse::error("Failed");
        assert!(!resp.success);
        assert_eq!(resp.message, "Failed");
        assert!(resp.results.is_none());
    }

    #[test]
    fn test_api_response_with_results_all_success() {
        let results = vec![
            CommandResult {
                name: "A".to_string(),
                success: true,
                state: ComponentState::Running,
                message: "OK".to_string(),
            },
            CommandResult {
                name: "B".to_string(),
                success: true,
                state: ComponentState::Running,
                message: "OK".to_string(),
            },
        ];
        let resp = ApiResponse::success("Commands sent").with_results(results);
        assert!(resp.success);
        assert!(resp.results.is_some());
        assert_eq!(resp.results.unwrap().len(), 2);
    }

    #[test]
    fn test_api_response_with_results_partial_failure() {
        let results = vec![
            CommandResult {
                name: "A".to_string(),
                success: true,
                state: ComponentState::Running,
                message: "OK".to_string(),
            },
            CommandResult {
                name: "B".to_string(),
                success: false,
                state: ComponentState::Error,
                message: "Failed".to_string(),
            },
        ];
        let resp = ApiResponse::success("Commands sent").with_results(results);
        // Should be false because not all succeeded
        assert!(!resp.success);
    }

    #[test]
    fn test_configure_request_to_run_config() {
        let req = ConfigureRequest {
            run_number: 42,
            comment: "Test run".to_string(),
            exp_name: "experiment1".to_string(),
        };
        let config: RunConfig = req.into();
        assert_eq!(config.run_number, 42);
        assert_eq!(config.comment, "Test run");
        assert_eq!(config.exp_name, "experiment1");
    }

    #[test]
    fn test_operator_config_default() {
        let config = OperatorConfig::default();
        assert_eq!(config.configure_timeout_ms, 5000);
        assert_eq!(config.arm_timeout_ms, 5000);
        assert_eq!(config.start_timeout_ms, 5000);
    }

    #[test]
    fn test_component_config() {
        let config = ComponentConfig {
            name: "Merger".to_string(),
            address: "tcp://localhost:5570".to_string(),
            pipeline_order: 2,
        };
        assert_eq!(config.name, "Merger");
        assert!(config.address.contains("5570"));
        assert_eq!(config.pipeline_order, 2);
    }

    #[test]
    fn test_component_status_serialization() {
        let status = make_status("Test", ComponentState::Running, true);
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"name\":\"Test\""));
        assert!(json.contains("\"online\":true"));

        let deserialized: ComponentStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "Test");
        assert_eq!(deserialized.state, ComponentState::Running);
    }

    #[test]
    fn test_system_status_serialization() {
        let status = SystemStatus {
            components: vec![make_status("A", ComponentState::Idle, true)],
            system_state: SystemState::Idle,
            run_info: None,
            experiment_name: "TestExp".to_string(),
            next_run_number: Some(1),
            last_run_info: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"system_state\":\"Idle\""));
        assert!(json.contains("\"experiment_name\":\"TestExp\""));
    }

    #[test]
    fn test_command_result_debug() {
        let result = CommandResult {
            name: "Test".to_string(),
            success: true,
            state: ComponentState::Configured,
            message: "OK".to_string(),
        };
        let debug = format!("{:?}", result);
        assert!(debug.contains("CommandResult"));
        assert!(debug.contains("Test"));
    }
}
