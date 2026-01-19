//! Common error types for DELILA components
//!
//! # Design Principles (KISS)
//! - Provide common error variants used across multiple components
//! - Each component can wrap these or define additional variants
//! - Use thiserror for ergonomic error handling

use thiserror::Error;

/// Common pipeline errors shared across components
///
/// These errors represent common failure modes in the DAQ pipeline.
/// Components can either use these directly or wrap them in component-specific types.
#[derive(Error, Debug)]
pub enum PipelineError {
    /// ZeroMQ transport error (tmq)
    #[error("ZMQ transport error: {0}")]
    ZmqTransport(#[from] tmq::TmqError),

    /// ZeroMQ socket error
    #[error("ZMQ socket error: {0}")]
    ZmqSocket(#[from] zmq::Error),

    /// MessagePack serialization error
    #[error("Serialization error: {0}")]
    Serialization(#[from] rmp_serde::encode::Error),

    /// MessagePack deserialization error
    #[error("Deserialization error: {0}")]
    Deserialization(#[from] rmp_serde::decode::Error),

    /// JSON serialization/deserialization error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// I/O error (file operations)
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Channel send failed (receiver dropped)
    #[error("Channel send failed: receiver dropped")]
    ChannelSend,

    /// Channel receive failed (sender dropped)
    #[error("Channel receive failed: sender dropped")]
    ChannelRecv,

    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),

    /// Component not in expected state
    #[error("Invalid state: expected {expected}, got {actual}")]
    InvalidState { expected: String, actual: String },

    /// Timeout waiting for operation
    #[error("Operation timed out: {0}")]
    Timeout(String),

    /// Generic error with message
    #[error("{0}")]
    Other(String),
}

impl PipelineError {
    /// Create a configuration error
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    /// Create an invalid state error
    pub fn invalid_state(expected: impl Into<String>, actual: impl Into<String>) -> Self {
        Self::InvalidState {
            expected: expected.into(),
            actual: actual.into(),
        }
    }

    /// Create a timeout error
    pub fn timeout(msg: impl Into<String>) -> Self {
        Self::Timeout(msg.into())
    }

    /// Create a generic error
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}

/// Result type alias using PipelineError
pub type PipelineResult<T> = Result<T, PipelineError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_error() {
        let err = PipelineError::config("missing required field");
        assert!(err.to_string().contains("Configuration error"));
        assert!(err.to_string().contains("missing required field"));
    }

    #[test]
    fn test_invalid_state_error() {
        let err = PipelineError::invalid_state("Running", "Idle");
        let msg = err.to_string();
        assert!(msg.contains("Running"));
        assert!(msg.contains("Idle"));
    }

    #[test]
    fn test_timeout_error() {
        let err = PipelineError::timeout("waiting for response");
        assert!(err.to_string().contains("timed out"));
    }

    #[test]
    fn test_channel_send_error() {
        let err = PipelineError::ChannelSend;
        assert!(err.to_string().contains("Channel send failed"));
    }

    #[test]
    fn test_io_error_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: PipelineError = io_err.into();
        assert!(err.to_string().contains("I/O error"));
    }

    #[test]
    fn test_other_error() {
        let err = PipelineError::other("something went wrong");
        assert!(err.to_string().contains("something went wrong"));
    }
}
