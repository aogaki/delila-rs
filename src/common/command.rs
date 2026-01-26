//! Command and response types for component control
//!
//! This module defines the protocol for controlling DAQ components
//! via ZeroMQ REQ/REP sockets.
//!
//! ## State Machine (Phase B)
//! ```text
//!   ┌──────┐  Configure  ┌────────────┐
//!   │ Idle │ ──────────► │ Configured │ ◄─────────┐
//!   └──────┘             └────────────┘           │
//!       ▲                      │                  │
//!       │                      │ Arm              │ Stop
//!       │ Reset                ▼                  │
//!       │                ┌──────────┐             │
//!       │                │  Armed   │             │
//!       │                └──────────┘             │
//!       │                      │                  │
//!       │                      │ Start            │
//!       │                      ▼                  │
//!       │                ┌──────────┐             │
//!       │                │ Running  │ ────────────┘
//!       │                └──────────┘
//!       │                      │
//!       │                      │ (on error)
//!       │                      ▼
//!       │                ┌──────────┐
//!       └─────────────── │  Error   │
//!                        └──────────┘
//! ```

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Component state (Phase B: 5-state machine)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, ToSchema)]
pub enum ComponentState {
    /// Initial state, no configuration loaded
    #[default]
    Idle,
    /// Configuration loaded and validated
    Configured,
    /// Hardware/resources prepared, ready to start
    Armed,
    /// Actively acquiring/processing data
    Running,
    /// Recoverable error occurred
    Error,
}

impl ComponentState {
    /// Check if transition to target state is valid
    pub fn can_transition_to(&self, target: ComponentState) -> bool {
        use ComponentState::*;
        matches!(
            (self, target),
            // Normal flow
            (Idle, Configured)       // Configure
            | (Configured, Armed)    // Arm
            | (Armed, Running)       // Start
            | (Running, Configured)  // Stop (return to Configured for quick restart)
            // Reset from any state
            | (Configured, Idle)
            | (Armed, Idle)
            | (Running, Idle)
            | (Error, Idle)
            // Error can happen from any active state
            | (Configured, Error)
            | (Armed, Error)
            | (Running, Error)
        )
    }

    /// Get valid commands for current state
    pub fn valid_commands(&self) -> &'static [&'static str] {
        use ComponentState::*;
        match self {
            Idle => &["Configure", "GetStatus"],
            Configured => &["Arm", "Reset", "GetStatus"],
            Armed => &["Start", "Reset", "GetStatus"],
            Running => &["Stop", "GetStatus"],
            Error => &["Reset", "GetStatus"],
        }
    }
}

impl std::fmt::Display for ComponentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComponentState::Idle => write!(f, "Idle"),
            ComponentState::Configured => write!(f, "Configured"),
            ComponentState::Armed => write!(f, "Armed"),
            ComponentState::Running => write!(f, "Running"),
            ComponentState::Error => write!(f, "Error"),
        }
    }
}

/// Run configuration passed with Configure command
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunConfig {
    /// Run number for this acquisition
    pub run_number: u32,
    /// Optional description/comment
    #[serde(default)]
    pub comment: String,
    /// Experiment name (used in output filenames)
    #[serde(default)]
    pub exp_name: String,
}

/// Runtime-configurable emulator settings
///
/// These settings can be updated while the emulator is running.
/// Changes take effect immediately on the next batch generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmulatorRuntimeConfig {
    /// Events per batch
    pub events_per_batch: u32,
    /// Batch interval in milliseconds (0 = maximum speed)
    pub batch_interval_ms: u64,
    /// Enable waveform generation
    pub enable_waveform: bool,
    /// Waveform probe bitmask (1=analog1, 2=analog2, 3=both, 63=all)
    pub waveform_probes: u8,
    /// Number of waveform samples
    pub waveform_samples: u32,
}

/// Commands sent from controller to components
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    /// Load configuration and prepare for run (Idle → Configured)
    Configure(RunConfig),
    /// Prepare hardware/resources for acquisition (Configured → Armed)
    Arm,
    /// Begin data acquisition (Armed → Running)
    /// run_number is passed at start time to allow changing it without re-configuring hardware
    Start { run_number: u32 },
    /// Stop data acquisition (Running → Configured)
    Stop,
    /// Reset to initial state (Any → Idle)
    Reset,
    /// Query current status
    GetStatus,
    /// Update emulator runtime configuration (Emulator-specific)
    /// Can be sent in any state, takes effect on next batch generation
    UpdateEmulatorConfig(EmulatorRuntimeConfig),
}

impl std::fmt::Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Command::Configure(cfg) => write!(f, "Configure(run={})", cfg.run_number),
            Command::Arm => write!(f, "Arm"),
            Command::Start { run_number } => write!(f, "Start(run={})", run_number),
            Command::Stop => write!(f, "Stop"),
            Command::Reset => write!(f, "Reset"),
            Command::GetStatus => write!(f, "GetStatus"),
            Command::UpdateEmulatorConfig(cfg) => {
                write!(f, "UpdateEmulatorConfig(events={})", cfg.events_per_batch)
            }
        }
    }
}

/// Response from component to controller
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResponse {
    /// Whether the command succeeded
    pub success: bool,
    /// Current component state after command
    pub state: ComponentState,
    /// Human-readable message
    pub message: String,
    /// Current run number (if configured)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_number: Option<u32>,
    /// Error code (if in error state)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<u32>,
    /// Component metrics (for status queries)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<super::ComponentMetrics>,
}

impl CommandResponse {
    /// Create a success response
    pub fn success(state: ComponentState, message: impl Into<String>) -> Self {
        Self {
            success: true,
            state,
            message: message.into(),
            run_number: None,
            error_code: None,
            metrics: None,
        }
    }

    /// Create a success response with run number
    pub fn success_with_run(
        state: ComponentState,
        message: impl Into<String>,
        run_number: u32,
    ) -> Self {
        Self {
            success: true,
            state,
            message: message.into(),
            run_number: Some(run_number),
            error_code: None,
            metrics: None,
        }
    }

    /// Create an error response
    pub fn error(state: ComponentState, message: impl Into<String>) -> Self {
        Self {
            success: false,
            state,
            message: message.into(),
            run_number: None,
            error_code: None,
            metrics: None,
        }
    }

    /// Create an error response with error code
    pub fn error_with_code(
        state: ComponentState,
        message: impl Into<String>,
        error_code: u32,
    ) -> Self {
        Self {
            success: false,
            state,
            message: message.into(),
            run_number: None,
            error_code: Some(error_code),
            metrics: None,
        }
    }

    /// Add metrics to the response
    pub fn with_metrics(mut self, metrics: super::ComponentMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Serialize to JSON bytes (for ZMQ)
    pub fn to_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserialize from JSON bytes
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

impl Command {
    /// Serialize to JSON bytes (for ZMQ)
    pub fn to_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserialize from JSON bytes
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_json_roundtrip() {
        let cmd = Command::Start { run_number: 42 };
        let bytes = cmd.to_json().unwrap();
        let decoded = Command::from_json(&bytes).unwrap();
        assert!(matches!(decoded, Command::Start { run_number: 42 }));
    }

    #[test]
    fn configure_command_roundtrip() {
        let cmd = Command::Configure(RunConfig {
            run_number: 123,
            comment: "Test run".to_string(),
            exp_name: "TestExp".to_string(),
        });
        let bytes = cmd.to_json().unwrap();
        let decoded = Command::from_json(&bytes).unwrap();
        if let Command::Configure(cfg) = decoded {
            assert_eq!(cfg.run_number, 123);
            assert_eq!(cfg.comment, "Test run");
        } else {
            panic!("Expected Configure command");
        }
    }

    #[test]
    fn response_json_roundtrip() {
        let resp = CommandResponse::success(ComponentState::Running, "Started");
        let bytes = resp.to_json().unwrap();
        let decoded = CommandResponse::from_json(&bytes).unwrap();

        assert!(decoded.success);
        assert_eq!(decoded.state, ComponentState::Running);
        assert_eq!(decoded.message, "Started");
    }

    #[test]
    fn response_with_run_number() {
        let resp = CommandResponse::success_with_run(ComponentState::Configured, "Configured", 42);
        let bytes = resp.to_json().unwrap();
        let decoded = CommandResponse::from_json(&bytes).unwrap();

        assert!(decoded.success);
        assert_eq!(decoded.state, ComponentState::Configured);
        assert_eq!(decoded.run_number, Some(42));
    }

    #[test]
    fn error_response() {
        let resp = CommandResponse::error(ComponentState::Idle, "Already idle");
        assert!(!resp.success);
        assert_eq!(resp.state, ComponentState::Idle);
    }

    #[test]
    fn error_response_with_code() {
        let resp = CommandResponse::error_with_code(ComponentState::Error, "Hardware fault", 101);
        assert!(!resp.success);
        assert_eq!(resp.state, ComponentState::Error);
        assert_eq!(resp.error_code, Some(101));
    }

    #[test]
    fn state_display() {
        assert_eq!(format!("{}", ComponentState::Idle), "Idle");
        assert_eq!(format!("{}", ComponentState::Configured), "Configured");
        assert_eq!(format!("{}", ComponentState::Armed), "Armed");
        assert_eq!(format!("{}", ComponentState::Running), "Running");
        assert_eq!(format!("{}", ComponentState::Error), "Error");
    }

    #[test]
    fn command_display() {
        assert_eq!(
            format!(
                "{}",
                Command::Configure(RunConfig {
                    run_number: 99,
                    ..Default::default()
                })
            ),
            "Configure(run=99)"
        );
        assert_eq!(format!("{}", Command::Arm), "Arm");
        assert_eq!(
            format!("{}", Command::Start { run_number: 1 }),
            "Start(run=1)"
        );
        assert_eq!(format!("{}", Command::Stop), "Stop");
        assert_eq!(format!("{}", Command::Reset), "Reset");
        assert_eq!(format!("{}", Command::GetStatus), "GetStatus");
    }

    #[test]
    fn state_transitions() {
        use ComponentState::*;

        // Valid transitions
        assert!(Idle.can_transition_to(Configured));
        assert!(Configured.can_transition_to(Armed));
        assert!(Armed.can_transition_to(Running));
        assert!(Running.can_transition_to(Configured));

        // Reset transitions
        assert!(Configured.can_transition_to(Idle));
        assert!(Armed.can_transition_to(Idle));
        assert!(Running.can_transition_to(Idle));
        assert!(Error.can_transition_to(Idle));

        // Error transitions
        assert!(Running.can_transition_to(Error));
        assert!(Armed.can_transition_to(Error));
        assert!(Configured.can_transition_to(Error));

        // Invalid transitions
        assert!(!Idle.can_transition_to(Running)); // Must go through Configured, Armed
        assert!(!Idle.can_transition_to(Armed));
        assert!(!Configured.can_transition_to(Running)); // Must Arm first
        assert!(!Error.can_transition_to(Running)); // Must Reset first
    }

    #[test]
    fn valid_commands_per_state() {
        use ComponentState::*;

        assert!(Idle.valid_commands().contains(&"Configure"));
        assert!(!Idle.valid_commands().contains(&"Start"));

        assert!(Configured.valid_commands().contains(&"Arm"));
        assert!(Configured.valid_commands().contains(&"Reset"));

        assert!(Armed.valid_commands().contains(&"Start"));
        assert!(!Armed.valid_commands().contains(&"Configure"));

        assert!(Running.valid_commands().contains(&"Stop"));
        assert!(!Running.valid_commands().contains(&"Start"));

        assert!(Error.valid_commands().contains(&"Reset"));
        assert!(!Error.valid_commands().contains(&"Start"));
    }
}
