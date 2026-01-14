//! Operator module - REST API for DAQ control
//!
//! Provides HTTP endpoints to control DAQ components via ZeroMQ.
//! Includes Swagger UI for API documentation.

mod client;
mod routes;

pub use client::ComponentClient;
pub use routes::create_router;

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
}

impl Default for OperatorConfig {
    fn default() -> Self {
        Self {
            configure_timeout_ms: 5000,
            arm_timeout_ms: 5000,
            start_timeout_ms: 5000,
        }
    }
}
