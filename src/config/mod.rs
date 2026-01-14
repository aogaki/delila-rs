//! Configuration module for DELILA DAQ system
//!
//! Supports loading configuration from:
//! - TOML files (network topology, infrastructure)
//! - JSON files (digitizer settings)
//! - MongoDB (operational settings) - future
//!
//! # Example
//! ```ignore
//! let config = Config::load("config.toml")?;
//! let merger_net = config.network.merger.as_ref().unwrap();
//! ```

pub mod digitizer;

pub use digitizer::{BoardConfig, CaenParameter, ChannelConfig, DigitizerConfig, FirmwareType};

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

    /// Get source configuration by ID
    pub fn get_source(&self, source_id: u32) -> Option<&SourceNetworkConfig> {
        self.network.sources.iter().find(|s| s.id == source_id)
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

    /// Digitizer URL (e.g., "dig2://172.18.4.56")
    /// If not set, this source is assumed to be an emulator
    #[serde(default)]
    pub digitizer_url: Option<String>,

    /// Module ID for event tagging
    #[serde(default)]
    pub module_id: Option<u8>,

    /// ADC time step in nanoseconds (default: 2.0 for 500 MHz)
    #[serde(default)]
    pub time_step_ns: Option<f64>,
}

impl SourceNetworkConfig {
    /// Check if this source is a real digitizer (has URL configured)
    pub fn is_digitizer(&self) -> bool {
        self.digitizer_url.is_some()
    }

    /// Get command address with default fallback
    pub fn command_address(&self) -> String {
        self.command
            .clone()
            .unwrap_or_else(|| format!("tcp://*:{}", 5560 + self.id as u16))
    }
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

    /// Maximum file size in MB (default: 1024 = 1GB)
    #[serde(default = "default_max_file_size_mb")]
    pub max_file_size_mb: u64,

    /// Maximum file duration in seconds (default: 600 = 10 minutes)
    #[serde(default = "default_max_file_duration_sec")]
    pub max_file_duration_sec: u64,
}

fn default_output_dir() -> String {
    "./data".to_string()
}

fn default_max_file_size_mb() -> u64 {
    1024 // 1GB
}

fn default_max_file_duration_sec() -> u64 {
    600 // 10 minutes
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
    8081
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

    #[test]
    fn parse_digitizer_source() {
        let toml = r#"
[network]
cluster_name = "test"

[[network.sources]]
id = 0
name = "digitizer-0"
bind = "tcp://*:5555"
command = "tcp://*:5560"
digitizer_url = "dig2://172.18.4.56"
module_id = 1
time_step_ns = 4.0
"#;
        let config = Config::from_toml(toml).unwrap();
        assert_eq!(config.network.sources.len(), 1);

        let source = &config.network.sources[0];
        assert!(source.is_digitizer());
        assert_eq!(source.digitizer_url, Some("dig2://172.18.4.56".to_string()));
        assert_eq!(source.module_id, Some(1));
        assert_eq!(source.time_step_ns, Some(4.0));
        assert_eq!(source.command_address(), "tcp://*:5560".to_string());
    }

    #[test]
    fn emulator_source_is_not_digitizer() {
        let toml = r#"
[network]
cluster_name = "test"

[[network.sources]]
id = 0
name = "emulator-0"
bind = "tcp://*:5555"
"#;
        let config = Config::from_toml(toml).unwrap();
        let source = &config.network.sources[0];

        // No digitizer_url = not a digitizer (emulator)
        assert!(!source.is_digitizer());
        assert!(source.digitizer_url.is_none());

        // Command address uses default
        assert_eq!(source.command_address(), "tcp://*:5560".to_string());
    }

    #[test]
    fn get_source_by_id() {
        let toml = r#"
[network]
cluster_name = "test"

[[network.sources]]
id = 0
name = "source-0"
bind = "tcp://*:5555"

[[network.sources]]
id = 2
name = "source-2"
bind = "tcp://*:5557"
digitizer_url = "dig2://192.168.1.100"
"#;
        let config = Config::from_toml(toml).unwrap();

        // Find source 0
        let s0 = config.get_source(0);
        assert!(s0.is_some());
        assert_eq!(s0.unwrap().name, "source-0");

        // Find source 2
        let s2 = config.get_source(2);
        assert!(s2.is_some());
        assert_eq!(s2.unwrap().name, "source-2");
        assert!(s2.unwrap().is_digitizer());

        // Source 1 doesn't exist
        assert!(config.get_source(1).is_none());
    }
}
