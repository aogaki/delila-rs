//! Digitizer configuration module
//!
//! Provides data structures for CAEN digitizer configuration.
//! Supports serialization to/from JSON for REST API and file storage.
//!
//! # Parameter Path Format
//! CAEN FELib uses path-based parameter access:
//! - `/par/<parameter>` - Board-level settings
//! - `/ch/<N>/par/<parameter>` - Per-channel settings
//! - `/ch/0..31/par/<parameter>` - Channel range (expanded by FELib)
//!
//! # Design Decision
//! All parameter values are stored as `String` rather than enums because:
//! - CAEN FELib validates values at `SetValue` time
//! - DevTree JSON provides valid choices dynamically
//! - Different firmware versions may have different valid values

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

/// Digitizer configuration
///
/// Represents complete configuration for a CAEN digitizer.
/// Follows the "defaults + overrides" pattern from architecture design.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DigitizerConfig {
    /// Digitizer identifier (matches source.id in network config)
    pub digitizer_id: u32,

    /// Human-readable name
    pub name: String,

    /// Firmware type (e.g., "PSD1", "PSD2", "PHA")
    pub firmware: FirmwareType,

    /// Number of channels on this digitizer
    #[serde(default = "default_num_channels")]
    pub num_channels: u8,

    /// Master digitizer flag for synchronized start
    ///
    /// In multi-digitizer setups:
    /// - Master: Receives Start command, generates TrgOut for slaves
    /// - Slave: Listens on SIN for start signal from master
    #[serde(default)]
    pub is_master: bool,

    /// Synchronization settings (optional, for Master/Slave setup)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<SyncConfig>,

    /// Board-level parameters
    pub board: BoardConfig,

    /// Default channel settings (applied to all channels)
    #[serde(default)]
    pub channel_defaults: ChannelConfig,

    /// Per-channel overrides (sparse - only channels that differ from defaults)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub channel_overrides: HashMap<u8, ChannelConfig>,
}

/// Synchronization configuration for Master/Slave setups
///
/// Controls TrgOut (master) and SIN (slave) behavior for synchronized start.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct SyncConfig {
    /// TrgOut source (master only)
    /// PSD2: "Run", "TestPulse", "SWcmd", "GlobalTrg", etc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trgout_source: Option<String>,

    /// SIN source for sync input (slave only)
    /// PSD2: "Disabled", "SIN", "GPIO", etc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sin_source: Option<String>,

    /// Start source override
    /// Master: "SWcmd" (software start)
    /// Slave: "SIN" (start on SIN signal)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_source: Option<String>,
}

fn default_num_channels() -> u8 {
    32
}

/// Supported firmware types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum FirmwareType {
    /// DPP-PSD firmware (legacy x725/x730)
    PSD1,
    /// DPP-PSD2 firmware (x274x series)
    PSD2,
    /// DPP-PHA firmware (for spectroscopy)
    PHA,
}

impl FirmwareType {
    /// Get the URL scheme prefix for this firmware
    pub fn url_scheme(&self) -> &'static str {
        match self {
            FirmwareType::PSD1 => "dig1://",
            FirmwareType::PSD2 => "dig2://",
            FirmwareType::PHA => "dig2://", // PHA uses same scheme as PSD2
        }
    }
}

/// Board-level configuration parameters
///
/// All values are strings to match CAEN FELib's parameter format.
/// Validation is done by FELib at SetValue time.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct BoardConfig {
    /// Start trigger source (e.g., "SWcmd", "ITLA")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_source: Option<String>,

    /// GPIO mode (e.g., "Run")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpio_mode: Option<String>,

    /// Test pulse period in nanoseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_pulse_period: Option<u32>,

    /// Test pulse width in nanoseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_pulse_width: Option<u32>,

    /// Global trigger source (e.g., "SwTrg", "TestPulse", "ITLA")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_trigger_source: Option<String>,

    /// Record length in samples (PSD1)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_length: Option<u32>,

    /// Enable waveform readout
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waveforms_enabled: Option<bool>,

    /// Additional board parameters as key-value pairs
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Channel configuration parameters
///
/// All fields are optional to support sparse overrides.
/// `None` means "use default" or "unchanged".
/// String values match CAEN FELib parameter format exactly.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ChannelConfig {
    /// Channel enable (e.g., "True", "False", "TRUE", "FALSE")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<String>,

    /// DC offset as percentage (0-100%)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dc_offset: Option<f32>,

    /// Pulse polarity (e.g., "Positive", "Negative", "POLARITY_POSITIVE")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polarity: Option<String>,

    /// Trigger threshold in ADC counts
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_threshold: Option<u32>,

    /// Long gate length in nanoseconds (PSD)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_long_ns: Option<u32>,

    /// Short gate length in nanoseconds (PSD)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_short_ns: Option<u32>,

    /// Pre-gate length in nanoseconds (PSD1)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_pre_ns: Option<u32>,

    /// Event trigger source (e.g., "GlobalTriggerSource", "ChSelfTrigger")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_trigger_source: Option<String>,

    /// Wave trigger source (e.g., "Disabled", "ChSelfTrigger")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wave_trigger_source: Option<String>,

    /// CFD delay in nanoseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cfd_delay_ns: Option<u32>,

    /// Additional channel parameters
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, serde_json::Value>,
}

/// CAEN parameter path-value pair
#[derive(Debug, Clone)]
pub struct CaenParameter {
    pub path: String,
    pub value: String,
}

/// Error type for digitizer configuration
#[derive(Debug)]
pub enum DigitizerConfigError {
    /// IO error reading config file
    Io(std::io::Error),
    /// JSON parse error
    Json(serde_json::Error),
}

impl std::fmt::Display for DigitizerConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DigitizerConfigError::Io(e) => write!(f, "Failed to read config file: {}", e),
            DigitizerConfigError::Json(e) => write!(f, "Failed to parse JSON: {}", e),
        }
    }
}

impl std::error::Error for DigitizerConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DigitizerConfigError::Io(e) => Some(e),
            DigitizerConfigError::Json(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for DigitizerConfigError {
    fn from(err: std::io::Error) -> Self {
        DigitizerConfigError::Io(err)
    }
}

impl From<serde_json::Error> for DigitizerConfigError {
    fn from(err: serde_json::Error) -> Self {
        DigitizerConfigError::Json(err)
    }
}

impl DigitizerConfig {
    /// Load digitizer configuration from a JSON file
    pub fn load<P: AsRef<std::path::Path>>(path: P) -> Result<Self, DigitizerConfigError> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = serde_json::from_str(&content)?;
        Ok(config)
    }

    /// Save digitizer configuration to a JSON file
    pub fn save<P: AsRef<std::path::Path>>(&self, path: P) -> Result<(), DigitizerConfigError> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Create a new digitizer config with defaults for the given firmware
    pub fn new(digitizer_id: u32, name: impl Into<String>, firmware: FirmwareType) -> Self {
        let num_channels = match firmware {
            FirmwareType::PSD1 => 8,
            FirmwareType::PSD2 | FirmwareType::PHA => 32,
        };

        Self {
            digitizer_id,
            name: name.into(),
            firmware,
            num_channels,
            is_master: false,
            sync: None,
            board: BoardConfig::default(),
            channel_defaults: ChannelConfig::default(),
            channel_overrides: HashMap::new(),
        }
    }

    /// Create a master digitizer config
    pub fn new_master(
        digitizer_id: u32,
        name: impl Into<String>,
        firmware: FirmwareType,
    ) -> Self {
        let mut config = Self::new(digitizer_id, name, firmware);
        config.is_master = true;
        config.sync = Some(SyncConfig {
            trgout_source: Some("Run".to_string()),
            sin_source: None,
            start_source: Some("SWcmd".to_string()),
        });
        config
    }

    /// Create a slave digitizer config
    pub fn new_slave(digitizer_id: u32, name: impl Into<String>, firmware: FirmwareType) -> Self {
        let mut config = Self::new(digitizer_id, name, firmware);
        config.is_master = false;
        config.sync = Some(SyncConfig {
            trgout_source: None,
            sin_source: Some("SIN".to_string()),
            start_source: Some("SIN".to_string()),
        });
        config
    }

    /// Get effective channel configuration (defaults merged with overrides)
    pub fn get_channel_config(&self, channel: u8) -> ChannelConfig {
        let mut config = self.channel_defaults.clone();

        if let Some(override_config) = self.channel_overrides.get(&channel) {
            // Merge override into defaults
            if override_config.enabled.is_some() {
                config.enabled = override_config.enabled.clone();
            }
            if override_config.dc_offset.is_some() {
                config.dc_offset = override_config.dc_offset;
            }
            if override_config.polarity.is_some() {
                config.polarity = override_config.polarity.clone();
            }
            if override_config.trigger_threshold.is_some() {
                config.trigger_threshold = override_config.trigger_threshold;
            }
            if override_config.gate_long_ns.is_some() {
                config.gate_long_ns = override_config.gate_long_ns;
            }
            if override_config.gate_short_ns.is_some() {
                config.gate_short_ns = override_config.gate_short_ns;
            }
            if override_config.gate_pre_ns.is_some() {
                config.gate_pre_ns = override_config.gate_pre_ns;
            }
            if override_config.event_trigger_source.is_some() {
                config.event_trigger_source = override_config.event_trigger_source.clone();
            }
            if override_config.wave_trigger_source.is_some() {
                config.wave_trigger_source = override_config.wave_trigger_source.clone();
            }
            if override_config.cfd_delay_ns.is_some() {
                config.cfd_delay_ns = override_config.cfd_delay_ns;
            }
            // Merge extra parameters
            for (k, v) in &override_config.extra {
                config.extra.insert(k.clone(), v.clone());
            }
        }

        config
    }

    /// Generate CAEN parameter path-value pairs for hardware configuration
    ///
    /// Returns parameters in the order they should be applied:
    /// 1. Board-level parameters
    /// 2. Channel defaults (using range syntax)
    /// 3. Channel-specific overrides
    pub fn to_caen_parameters(&self) -> Vec<CaenParameter> {
        let mut params = Vec::new();

        // Board parameters
        self.add_board_parameters(&mut params);

        // Channel defaults using range syntax
        self.add_channel_defaults(&mut params);

        // Channel-specific overrides
        self.add_channel_overrides(&mut params);

        params
    }

    fn add_board_parameters(&self, params: &mut Vec<CaenParameter>) {
        let board = &self.board;

        // Sync parameters (applied before other board params)
        if let Some(ref sync) = self.sync {
            // Start source (from sync config takes priority)
            if let Some(ref v) = sync.start_source {
                params.push(CaenParameter {
                    path: "/par/startsource".to_string(),
                    value: v.clone(),
                });
            }

            // TrgOut source (master only)
            if let Some(ref v) = sync.trgout_source {
                params.push(CaenParameter {
                    path: "/par/trgoutsource".to_string(),
                    value: v.clone(),
                });
            }

            // SIN source (slave only)
            if let Some(ref v) = sync.sin_source {
                params.push(CaenParameter {
                    path: "/par/sinsource".to_string(),
                    value: v.clone(),
                });
            }
        }

        // Board start source (if not set by sync config)
        if self.sync.as_ref().and_then(|s| s.start_source.as_ref()).is_none() {
            if let Some(ref v) = board.start_source {
                params.push(CaenParameter {
                    path: "/par/startsource".to_string(),
                    value: v.clone(),
                });
            }
        }

        if let Some(ref v) = board.gpio_mode {
            params.push(CaenParameter {
                path: "/par/gpiomode".to_string(),
                value: v.clone(),
            });
        }

        if let Some(v) = board.test_pulse_period {
            params.push(CaenParameter {
                path: "/par/testpulseperiod".to_string(),
                value: v.to_string(),
            });
        }

        if let Some(v) = board.test_pulse_width {
            params.push(CaenParameter {
                path: "/par/testpulsewidth".to_string(),
                value: v.to_string(),
            });
        }

        if let Some(ref v) = board.global_trigger_source {
            params.push(CaenParameter {
                path: "/par/globaltriggersource".to_string(),
                value: v.clone(),
            });
        }

        // PSD1-specific parameters
        if let Some(v) = board.record_length {
            let param_name = match self.firmware {
                FirmwareType::PSD1 => "/par/reclen",
                _ => "/par/chrecordlengths",
            };
            params.push(CaenParameter {
                path: param_name.to_string(),
                value: v.to_string(),
            });
        }

        if let Some(v) = board.waveforms_enabled {
            params.push(CaenParameter {
                path: "/par/waveforms".to_string(),
                value: v.to_string().to_lowercase(),
            });
        }

        // Extra parameters
        for (key, value) in &board.extra {
            let path = if key.starts_with('/') {
                key.clone()
            } else {
                format!("/par/{}", key)
            };
            params.push(CaenParameter {
                path,
                value: json_value_to_string(value),
            });
        }
    }

    fn add_channel_defaults(&self, params: &mut Vec<CaenParameter>) {
        let defaults = &self.channel_defaults;
        let ch_range = format!("/ch/0..{}/par", self.num_channels - 1);

        self.add_channel_params(params, &ch_range, defaults);
    }

    fn add_channel_overrides(&self, params: &mut Vec<CaenParameter>) {
        for (ch, config) in &self.channel_overrides {
            let ch_path = format!("/ch/{}/par", ch);
            self.add_channel_params(params, &ch_path, config);
        }
    }

    fn add_channel_params(
        &self,
        params: &mut Vec<CaenParameter>,
        ch_path: &str,
        config: &ChannelConfig,
    ) {
        // Parameter names differ between PSD1 and PSD2
        let (enable_name, offset_name, polarity_name, threshold_name) = match self.firmware {
            FirmwareType::PSD1 => ("ch_enabled", "ch_dcoffset", "ch_polarity", "ch_threshold"),
            FirmwareType::PSD2 | FirmwareType::PHA => {
                ("ChEnable", "DCOffset", "PulsePolarity", "TriggerThr")
            }
        };

        if let Some(ref v) = config.enabled {
            params.push(CaenParameter {
                path: format!("{}/{}", ch_path, enable_name),
                value: v.clone(),
            });
        }

        if let Some(v) = config.dc_offset {
            params.push(CaenParameter {
                path: format!("{}/{}", ch_path, offset_name),
                value: v.to_string(),
            });
        }

        if let Some(ref v) = config.polarity {
            params.push(CaenParameter {
                path: format!("{}/{}", ch_path, polarity_name),
                value: v.clone(),
            });
        }

        if let Some(v) = config.trigger_threshold {
            params.push(CaenParameter {
                path: format!("{}/{}", ch_path, threshold_name),
                value: v.to_string(),
            });
        }

        // Gate parameters (PSD-specific)
        if let Some(v) = config.gate_long_ns {
            let param_name = match self.firmware {
                FirmwareType::PSD1 => "ch_gate",
                _ => "GateLongLengthT",
            };
            params.push(CaenParameter {
                path: format!("{}/{}", ch_path, param_name),
                value: v.to_string(),
            });
        }

        if let Some(v) = config.gate_short_ns {
            let param_name = match self.firmware {
                FirmwareType::PSD1 => "ch_gateshort",
                _ => "GateShortLengthT",
            };
            params.push(CaenParameter {
                path: format!("{}/{}", ch_path, param_name),
                value: v.to_string(),
            });
        }

        if let Some(v) = config.gate_pre_ns {
            params.push(CaenParameter {
                path: format!("{}/ch_gatepre", ch_path),
                value: v.to_string(),
            });
        }

        // Trigger sources
        if let Some(ref v) = config.event_trigger_source {
            params.push(CaenParameter {
                path: format!("{}/EventTriggerSource", ch_path),
                value: v.clone(),
            });
        }

        if let Some(ref v) = config.wave_trigger_source {
            params.push(CaenParameter {
                path: format!("{}/WaveTriggerSource", ch_path),
                value: v.clone(),
            });
        }

        if let Some(v) = config.cfd_delay_ns {
            params.push(CaenParameter {
                path: format!("{}/ch_cfd_delay", ch_path),
                value: v.to_string(),
            });
        }

        // Extra parameters
        for (key, value) in &config.extra {
            let path = if key.starts_with('/') {
                key.clone()
            } else {
                format!("{}/{}", ch_path, key)
            };
            params.push(CaenParameter {
                path,
                value: json_value_to_string(value),
            });
        }
    }
}

/// Convert serde_json::Value to string for CAEN parameter
fn json_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_digitizer_config() {
        let config = DigitizerConfig::new(0, "Test Digitizer", FirmwareType::PSD2);
        assert_eq!(config.digitizer_id, 0);
        assert_eq!(config.name, "Test Digitizer");
        assert_eq!(config.firmware, FirmwareType::PSD2);
        assert_eq!(config.num_channels, 32);
    }

    #[test]
    fn test_psd1_has_8_channels() {
        let config = DigitizerConfig::new(0, "PSD1", FirmwareType::PSD1);
        assert_eq!(config.num_channels, 8);
    }

    #[test]
    fn test_serialize_deserialize() {
        let mut config = DigitizerConfig::new(1, "Digitizer 1", FirmwareType::PSD2);
        config.board.start_source = Some("SWcmd".to_string());
        config.channel_defaults.enabled = Some("True".to_string());
        config.channel_defaults.dc_offset = Some(20.0);
        config.channel_defaults.polarity = Some("Negative".to_string());
        config.channel_defaults.trigger_threshold = Some(500);

        // Add override for channel 0
        let ch0_override = ChannelConfig {
            trigger_threshold: Some(1000),
            ..Default::default()
        };
        config.channel_overrides.insert(0, ch0_override);

        // Serialize to JSON
        let json = serde_json::to_string_pretty(&config).unwrap();
        println!("{}", json);

        // Deserialize back
        let restored: DigitizerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.digitizer_id, 1);
        assert_eq!(restored.board.start_source, Some("SWcmd".to_string()));
        assert_eq!(restored.channel_defaults.trigger_threshold, Some(500));
        assert_eq!(
            restored
                .channel_overrides
                .get(&0)
                .unwrap()
                .trigger_threshold,
            Some(1000)
        );
    }

    #[test]
    fn test_get_channel_config_with_override() {
        let mut config = DigitizerConfig::new(0, "Test", FirmwareType::PSD2);
        config.channel_defaults.enabled = Some("True".to_string());
        config.channel_defaults.dc_offset = Some(20.0);
        config.channel_defaults.trigger_threshold = Some(500);

        // Override only trigger threshold for channel 0
        let override_config = ChannelConfig {
            trigger_threshold: Some(1000),
            ..Default::default()
        };
        config.channel_overrides.insert(0, override_config);

        // Channel 0 should have overridden threshold but default offset
        let ch0 = config.get_channel_config(0);
        assert_eq!(ch0.enabled, Some("True".to_string()));
        assert_eq!(ch0.dc_offset, Some(20.0));
        assert_eq!(ch0.trigger_threshold, Some(1000)); // Overridden

        // Channel 1 should have all defaults
        let ch1 = config.get_channel_config(1);
        assert_eq!(ch1.trigger_threshold, Some(500)); // Default
    }

    #[test]
    fn test_to_caen_parameters_psd2() {
        let mut config = DigitizerConfig::new(0, "Test", FirmwareType::PSD2);
        config.board.start_source = Some("SWcmd".to_string());
        config.channel_defaults.enabled = Some("True".to_string());
        config.channel_defaults.polarity = Some("Negative".to_string());

        let params = config.to_caen_parameters();

        // Check board parameter (PSD2 uses lowercase parameter names)
        assert!(params
            .iter()
            .any(|p| p.path == "/par/startsource" && p.value == "SWcmd"));

        // Check channel default (should use range syntax)
        assert!(params
            .iter()
            .any(|p| p.path == "/ch/0..31/par/ChEnable" && p.value == "True"));
        assert!(params
            .iter()
            .any(|p| p.path == "/ch/0..31/par/PulsePolarity" && p.value == "Negative"));
    }

    #[test]
    fn test_master_config_sync_params() {
        let config = DigitizerConfig::new_master(0, "Master", FirmwareType::PSD2);
        assert!(config.is_master);
        assert!(config.sync.is_some());

        let sync = config.sync.as_ref().unwrap();
        assert_eq!(sync.start_source, Some("SWcmd".to_string()));
        assert_eq!(sync.trgout_source, Some("Run".to_string()));
        assert!(sync.sin_source.is_none());

        let params = config.to_caen_parameters();

        // Master should have TrgOut set to Run
        assert!(params
            .iter()
            .any(|p| p.path == "/par/trgoutsource" && p.value == "Run"));

        // Master start source should be SWcmd
        assert!(params
            .iter()
            .any(|p| p.path == "/par/startsource" && p.value == "SWcmd"));
    }

    #[test]
    fn test_slave_config_sync_params() {
        let config = DigitizerConfig::new_slave(0, "Slave", FirmwareType::PSD2);
        assert!(!config.is_master);
        assert!(config.sync.is_some());

        let sync = config.sync.as_ref().unwrap();
        assert_eq!(sync.start_source, Some("SIN".to_string()));
        assert_eq!(sync.sin_source, Some("SIN".to_string()));
        assert!(sync.trgout_source.is_none());

        let params = config.to_caen_parameters();

        // Slave should have SIN source set
        assert!(params
            .iter()
            .any(|p| p.path == "/par/sinsource" && p.value == "SIN"));

        // Slave start source should be SIN
        assert!(params
            .iter()
            .any(|p| p.path == "/par/startsource" && p.value == "SIN"));
    }

    #[test]
    fn test_sync_config_json_roundtrip() {
        // Test that sync config can be serialized and deserialized from JSON
        let json = r#"{
            "digitizer_id": 0,
            "name": "Master Digitizer",
            "firmware": "PSD2",
            "is_master": true,
            "sync": {
                "trgout_source": "Run",
                "start_source": "SWcmd"
            },
            "board": {},
            "channel_defaults": {}
        }"#;

        let config: DigitizerConfig = serde_json::from_str(json).unwrap();
        assert!(config.is_master);
        assert!(config.sync.is_some());

        let sync = config.sync.as_ref().unwrap();
        assert_eq!(sync.trgout_source, Some("Run".to_string()));
        assert_eq!(sync.start_source, Some("SWcmd".to_string()));
        assert!(sync.sin_source.is_none());

        let params = config.to_caen_parameters();
        assert!(params.iter().any(|p| p.path == "/par/trgoutsource"));
        assert!(params
            .iter()
            .any(|p| p.path == "/par/startsource" && p.value == "SWcmd"));
    }

    #[test]
    fn test_sync_config_slave_json() {
        let json = r#"{
            "digitizer_id": 1,
            "name": "Slave Digitizer",
            "firmware": "PSD2",
            "is_master": false,
            "sync": {
                "sin_source": "SIN",
                "start_source": "SIN"
            },
            "board": {},
            "channel_defaults": {}
        }"#;

        let config: DigitizerConfig = serde_json::from_str(json).unwrap();
        assert!(!config.is_master);

        let params = config.to_caen_parameters();
        assert!(params
            .iter()
            .any(|p| p.path == "/par/sinsource" && p.value == "SIN"));
        assert!(params
            .iter()
            .any(|p| p.path == "/par/startsource" && p.value == "SIN"));
        // Slave should NOT have trgout set
        assert!(!params.iter().any(|p| p.path == "/par/trgoutsource"));
    }

    #[test]
    fn test_to_caen_parameters_psd1() {
        let mut config = DigitizerConfig::new(0, "Test", FirmwareType::PSD1);
        config.channel_defaults.enabled = Some("TRUE".to_string());
        config.channel_defaults.polarity = Some("POLARITY_NEGATIVE".to_string());

        let params = config.to_caen_parameters();

        // PSD1 uses different parameter names
        assert!(params
            .iter()
            .any(|p| p.path == "/ch/0..7/par/ch_enabled" && p.value == "TRUE"));
        assert!(params
            .iter()
            .any(|p| p.path == "/ch/0..7/par/ch_polarity" && p.value == "POLARITY_NEGATIVE"));
    }

    #[test]
    fn test_json_example_config() {
        // Example JSON that would come from REST API
        let json = r#"{
            "digitizer_id": 0,
            "name": "LaBr3 Digitizer",
            "firmware": "PSD2",
            "num_channels": 32,
            "board": {
                "start_source": "SWcmd",
                "gpio_mode": "Run",
                "test_pulse_period": 10000,
                "global_trigger_source": "TestPulse"
            },
            "channel_defaults": {
                "enabled": "True",
                "dc_offset": 20.0,
                "polarity": "Negative",
                "trigger_threshold": 500,
                "gate_long_ns": 400,
                "gate_short_ns": 100
            },
            "channel_overrides": {
                "0": {
                    "trigger_threshold": 300
                },
                "1": {
                    "enabled": "False"
                }
            }
        }"#;

        let config: DigitizerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.name, "LaBr3 Digitizer");
        assert_eq!(config.firmware, FirmwareType::PSD2);
        assert_eq!(config.channel_defaults.gate_long_ns, Some(400));

        // Check that overrides work
        let ch0 = config.get_channel_config(0);
        assert_eq!(ch0.trigger_threshold, Some(300)); // Overridden
        assert_eq!(ch0.gate_long_ns, Some(400)); // From default

        let ch1 = config.get_channel_config(1);
        assert_eq!(ch1.enabled, Some("False".to_string())); // Overridden
    }
}
