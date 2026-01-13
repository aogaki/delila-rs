//! Configuration module for DELILA DAQ system
//!
//! Supports loading configuration from:
//! - TOML files (network topology, infrastructure)
//! - MongoDB (operational settings) - future
//!
//! # Example
//! ```ignore
//! let config = Config::load("config.toml")?;
//! let merger_net = config.network.merger.as_ref().unwrap();
//! ```

use serde::Deserialize;
use std::path::Path;
use thiserror::Error;

/// Configuration errors
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Failed to parse TOML: {0}")]
    TomlError(#[from] toml::de::Error),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("MongoDB not yet supported")]
    MongoDbNotSupported,
}

/// Top-level configuration
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub network: NetworkConfig,
    #[serde(default)]
    pub settings: SettingsConfig,
}

impl Config {
    /// Load configuration from a TOML file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    /// Load configuration from a TOML string (useful for testing)
    pub fn from_toml(content: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(content)?;
        Ok(config)
    }
}

// =============================================================================
// Network Configuration
// =============================================================================

/// Network topology configuration
#[derive(Debug, Clone, Deserialize)]
pub struct NetworkConfig {
    /// Cluster name for identification
    #[serde(default = "default_cluster_name")]
    pub cluster_name: String,

    /// Data source configurations
    #[serde(default)]
    pub sources: Vec<SourceNetworkConfig>,

    /// Merger configuration
    pub merger: Option<MergerNetworkConfig>,

    /// Recorder configuration
    pub recorder: Option<RecorderNetworkConfig>,

    /// Monitor configuration
    pub monitor: Option<MonitorNetworkConfig>,
}

fn default_cluster_name() -> String {
    "default".to_string()
}

/// Data source (emulator/digitizer) network config
#[derive(Debug, Clone, Deserialize)]
pub struct SourceNetworkConfig {
    /// Unique source ID
    pub id: u32,

    /// Human-readable name
    #[serde(default)]
    pub name: String,

    /// ZMQ bind address for data (e.g., "tcp://*:5555")
    pub bind: String,

    /// ZMQ bind address for commands (e.g., "tcp://*:5560")
    #[serde(default)]
    pub command: Option<String>,
}

/// Merger network configuration
#[derive(Debug, Clone, Deserialize)]
pub struct MergerNetworkConfig {
    /// ZMQ addresses to subscribe to (upstream sources)
    pub subscribe: Vec<String>,

    /// ZMQ address to publish to (downstream)
    pub publish: String,

    /// ZMQ bind address for commands (e.g., "tcp://*:5570")
    #[serde(default)]
    pub command: Option<String>,

    /// Internal channel capacity
    #[serde(default = "default_channel_capacity")]
    pub channel_capacity: usize,
}

fn default_channel_capacity() -> usize {
    1000
}

/// Recorder network configuration
#[derive(Debug, Clone, Deserialize)]
pub struct RecorderNetworkConfig {
    /// ZMQ address to subscribe to
    pub subscribe: String,

    /// ZMQ bind address for commands (e.g., "tcp://*:5580")
    #[serde(default)]
    pub command: Option<String>,

    /// Output directory for data files
    #[serde(default = "default_output_dir")]
    pub output_dir: String,
}

fn default_output_dir() -> String {
    "./data".to_string()
}

/// Monitor network configuration
#[derive(Debug, Clone, Deserialize)]
pub struct MonitorNetworkConfig {
    /// ZMQ address to subscribe to
    pub subscribe: String,

    /// HTTP server port for web UI
    #[serde(default = "default_http_port")]
    pub http_port: u16,
}

fn default_http_port() -> u16 {
    8080
}

// =============================================================================
// Settings Configuration
// =============================================================================

/// Where to load operational settings from
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SettingsSource {
    #[default]
    File,
    #[serde(rename = "mongodb")]
    MongoDB,
}

/// Settings configuration
#[derive(Debug, Clone, Deserialize)]
pub struct SettingsConfig {
    /// Source of settings
    #[serde(default)]
    pub source: SettingsSource,

    /// File-based settings
    #[serde(default)]
    pub file: FileSettings,

    /// MongoDB connection settings (future)
    pub mongodb: Option<MongoDbSettings>,
}

impl Default for SettingsConfig {
    fn default() -> Self {
        Self {
            source: SettingsSource::File,
            file: FileSettings::default(),
            mongodb: None,
        }
    }
}

impl SettingsConfig {
    /// Get the effective settings based on the configured source
    pub fn get_settings(&self) -> Result<Settings, ConfigError> {
        match self.source {
            SettingsSource::File => Ok(Settings::from(&self.file)),
            SettingsSource::MongoDB => Err(ConfigError::MongoDbNotSupported),
        }
    }
}

/// File-based operational settings
#[derive(Debug, Clone, Deserialize)]
pub struct FileSettings {
    /// Events per batch
    #[serde(default = "default_events_per_batch")]
    pub events_per_batch: u32,

    /// Batch interval in milliseconds
    #[serde(default = "default_batch_interval_ms")]
    pub batch_interval_ms: u64,

    /// Number of modules per digitizer
    #[serde(default = "default_num_modules")]
    pub num_modules: u32,

    /// Channels per module
    #[serde(default = "default_channels_per_module")]
    pub channels_per_module: u32,
}

impl Default for FileSettings {
    fn default() -> Self {
        Self {
            events_per_batch: default_events_per_batch(),
            batch_interval_ms: default_batch_interval_ms(),
            num_modules: default_num_modules(),
            channels_per_module: default_channels_per_module(),
        }
    }
}

fn default_events_per_batch() -> u32 {
    100
}
fn default_batch_interval_ms() -> u64 {
    100
}
fn default_num_modules() -> u32 {
    2
}
fn default_channels_per_module() -> u32 {
    16
}

/// MongoDB connection settings (future)
#[derive(Debug, Clone, Deserialize)]
pub struct MongoDbSettings {
    /// MongoDB URI
    pub uri: String,

    /// Database name
    pub database: String,

    /// Collection name
    #[serde(default = "default_collection")]
    pub collection: String,
}

fn default_collection() -> String {
    "run_config".to_string()
}

/// Unified operational settings (loaded from file or MongoDB)
#[derive(Debug, Clone)]
pub struct Settings {
    pub events_per_batch: u32,
    pub batch_interval_ms: u64,
    pub num_modules: u32,
    pub channels_per_module: u32,
}

impl From<&FileSettings> for Settings {
    fn from(file: &FileSettings) -> Self {
        Self {
            events_per_batch: file.events_per_batch,
            batch_interval_ms: file.batch_interval_ms,
            num_modules: file.num_modules,
            channels_per_module: file.channels_per_module,
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let toml = r#"
[network]
cluster_name = "test"
"#;
        let config = Config::from_toml(toml).unwrap();
        assert_eq!(config.network.cluster_name, "test");
        assert!(config.network.sources.is_empty());
    }

    #[test]
    fn parse_full_config() {
        let toml = r#"
[network]
cluster_name = "daq-cluster-1"

[[network.sources]]
id = 1
name = "digitizer-1"
bind = "tcp://*:5555"

[[network.sources]]
id = 2
name = "digitizer-2"
bind = "tcp://*:5556"

[network.merger]
subscribe = ["tcp://localhost:5555", "tcp://localhost:5556"]
publish = "tcp://*:5557"
channel_capacity = 2000

[network.recorder]
subscribe = "tcp://localhost:5557"
output_dir = "/data/runs"

[network.monitor]
subscribe = "tcp://localhost:5557"
http_port = 9000

[settings]
source = "file"

[settings.file]
events_per_batch = 200
batch_interval_ms = 50
"#;
        let config = Config::from_toml(toml).unwrap();

        // Network
        assert_eq!(config.network.cluster_name, "daq-cluster-1");
        assert_eq!(config.network.sources.len(), 2);
        assert_eq!(config.network.sources[0].id, 1);
        assert_eq!(config.network.sources[1].bind, "tcp://*:5556");

        // Merger
        let merger = config.network.merger.as_ref().unwrap();
        assert_eq!(merger.subscribe.len(), 2);
        assert_eq!(merger.publish, "tcp://*:5557");
        assert_eq!(merger.channel_capacity, 2000);

        // Recorder
        let recorder = config.network.recorder.as_ref().unwrap();
        assert_eq!(recorder.output_dir, "/data/runs");

        // Monitor
        let monitor = config.network.monitor.as_ref().unwrap();
        assert_eq!(monitor.http_port, 9000);

        // Settings
        assert_eq!(config.settings.source, SettingsSource::File);
        let settings = config.settings.get_settings().unwrap();
        assert_eq!(settings.events_per_batch, 200);
        assert_eq!(settings.batch_interval_ms, 50);
    }

    #[test]
    fn default_settings() {
        let toml = r#"
[network]
cluster_name = "test"
"#;
        let config = Config::from_toml(toml).unwrap();
        let settings = config.settings.get_settings().unwrap();

        assert_eq!(settings.events_per_batch, 100);
        assert_eq!(settings.batch_interval_ms, 100);
    }

    #[test]
    fn mongodb_not_supported() {
        let toml = r#"
[network]
cluster_name = "test"

[settings]
source = "mongodb"

[settings.mongodb]
uri = "mongodb://localhost:27017"
database = "delila"
"#;
        let config = Config::from_toml(toml).unwrap();
        assert!(config.settings.get_settings().is_err());
    }
}
