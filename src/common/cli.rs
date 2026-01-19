//! CLI argument parsing for DELILA components
//!
//! # Design Principles (KISS)
//! - Use clap's derive macro for declarative argument definition
//! - Common arguments shared via composition, not inheritance
//! - Each binary has its own Args struct that embeds CommonArgs

use clap::Parser;

/// Common arguments shared across all DELILA components
#[derive(Parser, Debug, Clone)]
pub struct CommonArgs {
    /// Path to configuration file
    #[arg(short = 'f', long = "config", default_value = "config.toml")]
    pub config_file: String,
}

/// Arguments for source components (Reader, Emulator)
#[derive(Parser, Debug, Clone)]
pub struct SourceArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Source ID (0-indexed module number)
    #[arg(long = "source-id")]
    pub source_id: Option<u32>,

    /// Override bind address (e.g., tcp://*:5555)
    #[arg(long)]
    pub address: Option<String>,
}

/// Arguments for pipeline components (Recorder, Monitor, DataSink)
#[derive(Parser, Debug, Clone)]
pub struct PipelineArgs {
    #[command(flatten)]
    pub common: CommonArgs,
}

/// Arguments for Merger (supports multiple upstream sources)
#[derive(Parser, Debug, Clone)]
pub struct MergerArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Subscribe to upstream address (can specify multiple times)
    #[arg(short = 's', long = "sub", action = clap::ArgAction::Append)]
    pub sub_addresses: Vec<String>,

    /// Publish to downstream address
    #[arg(short = 'p', long = "pub")]
    pub pub_address: Option<String>,
}

/// Arguments for Recorder (file writer)
#[derive(Parser, Debug, Clone)]
pub struct RecorderArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// ZMQ address to subscribe to
    #[arg(short = 'a', long = "address")]
    pub address: Option<String>,

    /// Output directory for data files
    #[arg(short = 'o', long = "output")]
    pub output_dir: Option<String>,
}

/// Arguments for Monitor (web histogram display)
#[derive(Parser, Debug, Clone)]
pub struct MonitorArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// ZMQ address to subscribe to
    #[arg(short = 'a', long = "address")]
    pub address: Option<String>,

    /// HTTP server port
    #[arg(short = 'p', long = "port")]
    pub port: Option<u16>,
}

/// Arguments for DataSink (test subscriber)
#[derive(Parser, Debug, Clone)]
pub struct DataSinkArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// ZMQ address to subscribe to
    #[arg(short = 'a', long = "address")]
    pub address: Option<String>,
}

/// Arguments for Operator (Web UI / Control API)
#[derive(Parser, Debug, Clone)]
pub struct OperatorArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// HTTP server port
    #[arg(long, default_value = "8080")]
    pub port: u16,
}

/// Arguments for Controller (CLI control client)
#[derive(Parser, Debug, Clone)]
pub struct ControllerArgs {
    /// Target component's command address (e.g., tcp://localhost:5560)
    #[arg(short, long)]
    pub address: String,

    /// Command to send (e.g., GetStatus, Configure, Arm, Start, Stop)
    #[arg(short, long)]
    pub command: String,

    /// Run number (required for Configure command)
    #[arg(long)]
    pub run_number: Option<u32>,

    /// Experiment name (optional for Configure command)
    #[arg(long)]
    pub exp_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_common_args_default() {
        let args = CommonArgs::try_parse_from(["test"]).unwrap();
        assert_eq!(args.config_file, "config.toml");
    }

    #[test]
    fn test_common_args_custom_config() {
        let args = CommonArgs::try_parse_from(["test", "-f", "custom.toml"]).unwrap();
        assert_eq!(args.config_file, "custom.toml");
    }

    #[test]
    fn test_common_args_long_config() {
        let args = CommonArgs::try_parse_from(["test", "--config", "my_config.toml"]).unwrap();
        assert_eq!(args.config_file, "my_config.toml");
    }

    #[test]
    fn test_source_args_default() {
        let args = SourceArgs::try_parse_from(["test"]).unwrap();
        assert_eq!(args.common.config_file, "config.toml");
        assert_eq!(args.source_id, None);
        assert_eq!(args.address, None);
    }

    #[test]
    fn test_source_args_with_id() {
        let args = SourceArgs::try_parse_from(["test", "--source-id", "1"]).unwrap();
        assert_eq!(args.source_id, Some(1));
    }

    #[test]
    fn test_source_args_with_address() {
        let args = SourceArgs::try_parse_from(["test", "--address", "tcp://*:5555"]).unwrap();
        assert_eq!(args.address, Some("tcp://*:5555".to_string()));
    }

    #[test]
    fn test_source_args_full() {
        let args = SourceArgs::try_parse_from([
            "test",
            "-f",
            "custom.toml",
            "--source-id",
            "2",
            "--address",
            "tcp://*:6000",
        ])
        .unwrap();
        assert_eq!(args.common.config_file, "custom.toml");
        assert_eq!(args.source_id, Some(2));
        assert_eq!(args.address, Some("tcp://*:6000".to_string()));
    }

    #[test]
    fn test_pipeline_args_default() {
        let args = PipelineArgs::try_parse_from(["test"]).unwrap();
        assert_eq!(args.common.config_file, "config.toml");
    }

    #[test]
    fn test_pipeline_args_with_config() {
        let args = PipelineArgs::try_parse_from(["test", "-f", "daq.toml"]).unwrap();
        assert_eq!(args.common.config_file, "daq.toml");
    }

    #[test]
    fn test_operator_args_default() {
        let args = OperatorArgs::try_parse_from(["test"]).unwrap();
        assert_eq!(args.common.config_file, "config.toml");
        assert_eq!(args.port, 8080);
    }

    #[test]
    fn test_operator_args_port() {
        let args = OperatorArgs::try_parse_from(["test", "--port", "9090"]).unwrap();
        assert_eq!(args.port, 9090);
    }

    #[test]
    fn test_operator_args_full() {
        let args =
            OperatorArgs::try_parse_from(["test", "-f", "op.toml", "--port", "8888"]).unwrap();
        assert_eq!(args.common.config_file, "op.toml");
        assert_eq!(args.port, 8888);
    }

    #[test]
    fn test_controller_args() {
        let args = ControllerArgs::try_parse_from([
            "test",
            "-a",
            "tcp://localhost:5560",
            "-c",
            "GetStatus",
        ])
        .unwrap();
        assert_eq!(args.address, "tcp://localhost:5560");
        assert_eq!(args.command, "GetStatus");
        assert_eq!(args.run_number, None);
    }

    #[test]
    fn test_controller_args_configure() {
        let args = ControllerArgs::try_parse_from([
            "test",
            "-a",
            "tcp://localhost:5560",
            "-c",
            "Configure",
            "--run-number",
            "42",
            "--exp-name",
            "test_exp",
        ])
        .unwrap();
        assert_eq!(args.command, "Configure");
        assert_eq!(args.run_number, Some(42));
        assert_eq!(args.exp_name, Some("test_exp".to_string()));
    }

    #[test]
    fn test_merger_args_default() {
        let args = MergerArgs::try_parse_from(["test"]).unwrap();
        assert_eq!(args.common.config_file, "config.toml");
        assert!(args.sub_addresses.is_empty());
        assert_eq!(args.pub_address, None);
    }

    #[test]
    fn test_merger_args_single_sub() {
        let args =
            MergerArgs::try_parse_from(["test", "-s", "tcp://localhost:5555"]).unwrap();
        assert_eq!(args.sub_addresses, vec!["tcp://localhost:5555"]);
    }

    #[test]
    fn test_merger_args_multiple_subs() {
        let args = MergerArgs::try_parse_from([
            "test",
            "-s",
            "tcp://localhost:5555",
            "-s",
            "tcp://localhost:5556",
            "--sub",
            "tcp://localhost:5557",
        ])
        .unwrap();
        assert_eq!(
            args.sub_addresses,
            vec![
                "tcp://localhost:5555",
                "tcp://localhost:5556",
                "tcp://localhost:5557"
            ]
        );
    }

    #[test]
    fn test_merger_args_full() {
        let args = MergerArgs::try_parse_from([
            "test",
            "-f",
            "custom.toml",
            "-s",
            "tcp://localhost:5555",
            "-s",
            "tcp://localhost:5556",
            "-p",
            "tcp://*:5557",
        ])
        .unwrap();
        assert_eq!(args.common.config_file, "custom.toml");
        assert_eq!(
            args.sub_addresses,
            vec!["tcp://localhost:5555", "tcp://localhost:5556"]
        );
        assert_eq!(args.pub_address, Some("tcp://*:5557".to_string()));
    }

    #[test]
    fn test_recorder_args_default() {
        let args = RecorderArgs::try_parse_from(["test"]).unwrap();
        assert_eq!(args.common.config_file, "config.toml");
        assert_eq!(args.address, None);
        assert_eq!(args.output_dir, None);
    }

    #[test]
    fn test_recorder_args_full() {
        let args = RecorderArgs::try_parse_from([
            "test",
            "-f",
            "rec.toml",
            "-a",
            "tcp://localhost:5557",
            "-o",
            "./output",
        ])
        .unwrap();
        assert_eq!(args.common.config_file, "rec.toml");
        assert_eq!(args.address, Some("tcp://localhost:5557".to_string()));
        assert_eq!(args.output_dir, Some("./output".to_string()));
    }

    #[test]
    fn test_monitor_args_default() {
        let args = MonitorArgs::try_parse_from(["test"]).unwrap();
        assert_eq!(args.common.config_file, "config.toml");
        assert_eq!(args.address, None);
        assert_eq!(args.port, None);
    }

    #[test]
    fn test_monitor_args_full() {
        let args = MonitorArgs::try_parse_from([
            "test",
            "-f",
            "mon.toml",
            "-a",
            "tcp://localhost:5557",
            "-p",
            "9090",
        ])
        .unwrap();
        assert_eq!(args.common.config_file, "mon.toml");
        assert_eq!(args.address, Some("tcp://localhost:5557".to_string()));
        assert_eq!(args.port, Some(9090));
    }

    #[test]
    fn test_data_sink_args_default() {
        let args = DataSinkArgs::try_parse_from(["test"]).unwrap();
        assert_eq!(args.common.config_file, "config.toml");
        assert_eq!(args.address, None);
    }

    #[test]
    fn test_data_sink_args_full() {
        let args = DataSinkArgs::try_parse_from([
            "test",
            "-f",
            "sink.toml",
            "-a",
            "tcp://localhost:5555",
        ])
        .unwrap();
        assert_eq!(args.common.config_file, "sink.toml");
        assert_eq!(args.address, Some("tcp://localhost:5555".to_string()));
    }
}
