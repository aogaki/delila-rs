//! Controller CLI - sends commands to DAQ components
//!
//! Usage:
//!   cargo run --bin controller -- configure tcp://localhost:5560 --run 123
//!   cargo run --bin controller -- arm tcp://localhost:5560
//!   cargo run --bin controller -- start tcp://localhost:5560 --run 123
//!   cargo run --bin controller -- stop tcp://localhost:5560
//!   cargo run --bin controller -- reset tcp://localhost:5560
//!   cargo run --bin controller -- status tcp://localhost:5560

use clap::{Parser, Subcommand};
use delila_rs::common::{Command, CommandResponse, RunConfig};
use tmq::{request_reply, Context};

#[derive(Parser, Debug)]
#[command(
    name = "controller",
    about = "DELILA controller - send commands to DAQ components"
)]
#[command(
    after_help = "State Machine:\n  Idle → Configure → Configured → Arm → Armed → Start → Running\n  Running → Stop → Configured (quick restart possible)\n  Any → Reset → Idle"
)]
struct Args {
    #[command(subcommand)]
    command: ControllerCommand,
}

#[derive(Subcommand, Debug)]
enum ControllerCommand {
    /// Configure component for run (Idle → Configured)
    Configure {
        /// Target component's command address (e.g., tcp://localhost:5560)
        address: String,
        /// Run number (required)
        #[arg(long = "run")]
        run_number: u32,
        /// Optional comment
        #[arg(long)]
        comment: Option<String>,
    },
    /// Prepare for acquisition (Configured → Armed)
    Arm {
        /// Target component's command address
        address: String,
    },
    /// Begin data acquisition (Armed → Running)
    Start {
        /// Target component's command address
        address: String,
        /// Run number (required)
        #[arg(long = "run")]
        run_number: u32,
    },
    /// Stop acquisition (Running → Configured)
    Stop {
        /// Target component's command address
        address: String,
    },
    /// Reset to idle state (Any → Idle)
    Reset {
        /// Target component's command address
        address: String,
    },
    /// Query current status
    Status {
        /// Target component's command address
        address: String,
    },
}

impl ControllerCommand {
    fn to_command(&self) -> Command {
        match self {
            ControllerCommand::Configure {
                run_number,
                comment,
                ..
            } => Command::Configure(RunConfig {
                run_number: *run_number,
                comment: comment.clone().unwrap_or_default(),
                exp_name: String::new(),
            }),
            ControllerCommand::Arm { .. } => Command::Arm,
            ControllerCommand::Start { run_number, .. } => Command::Start {
                run_number: *run_number,
            },
            ControllerCommand::Stop { .. } => Command::Stop,
            ControllerCommand::Reset { .. } => Command::Reset,
            ControllerCommand::Status { .. } => Command::GetStatus,
        }
    }

    fn address(&self) -> &str {
        match self {
            ControllerCommand::Configure { address, .. }
            | ControllerCommand::Arm { address }
            | ControllerCommand::Start { address, .. }
            | ControllerCommand::Stop { address }
            | ControllerCommand::Reset { address }
            | ControllerCommand::Status { address } => address,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let command = args.command.to_command();
    let address = args.command.address();

    println!("Sending {} to {}", command, address);

    // Create REQ socket and connect
    let context = Context::new();
    let requester = request_reply::request(&context).connect(address)?;

    // Send command
    let cmd_bytes = command.to_json()?;
    let msg: tmq::Multipart = vec![tmq::Message::from(cmd_bytes.as_slice())].into();
    let responder = requester.send(msg).await?;

    // Receive response
    let (mut response_msg, _) = responder.recv().await?;

    if let Some(frame) = response_msg.pop_front() {
        let response = CommandResponse::from_json(&frame)?;
        println!();
        println!("Response:");
        println!("  Success: {}", response.success);
        println!("  State:   {}", response.state);
        if let Some(run) = response.run_number {
            println!("  Run:     {}", run);
        }
        if let Some(code) = response.error_code {
            println!("  Error:   {}", code);
        }
        println!("  Message: {}", response.message);
    } else {
        eprintln!("Empty response received");
    }

    Ok(())
}
