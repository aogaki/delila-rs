//! Controller CLI - sends commands to DAQ components
//!
//! Usage:
//!   cargo run --bin controller -- configure tcp://localhost:5560 --run 123
//!   cargo run --bin controller -- arm tcp://localhost:5560
//!   cargo run --bin controller -- start tcp://localhost:5560
//!   cargo run --bin controller -- stop tcp://localhost:5560
//!   cargo run --bin controller -- reset tcp://localhost:5560
//!   cargo run --bin controller -- status tcp://localhost:5560

use delila_rs::common::{Command, CommandResponse, RunConfig};
use tmq::{request_reply, Context};

fn print_usage() {
    println!("Controller - send commands to DAQ components (5-state machine)");
    println!();
    println!("Usage: controller <command> <address> [options]");
    println!();
    println!("Commands:");
    println!("  configure  Configure component for run (Idle → Configured)");
    println!("  arm        Prepare for acquisition (Configured → Armed)");
    println!("  start      Begin data acquisition (Armed → Running)");
    println!("  stop       Stop acquisition (Running → Configured)");
    println!("  reset      Reset to idle state (Any → Idle)");
    println!("  status     Query current status");
    println!();
    println!("Options for 'configure':");
    println!("  --run <number>     Run number (required)");
    println!("  --comment <text>   Optional comment");
    println!();
    println!("Examples:");
    println!("  controller configure tcp://localhost:5560 --run 123");
    println!("  controller configure tcp://localhost:5560 --run 123 --comment \"Test run\"");
    println!("  controller arm tcp://localhost:5560");
    println!("  controller start tcp://localhost:5560");
    println!("  controller stop tcp://localhost:5560");
    println!("  controller reset tcp://localhost:5560");
    println!("  controller status tcp://localhost:5570");
    println!();
    println!("State Machine:");
    println!("  Idle → Configure → Configured → Arm → Armed → Start → Running");
    println!("  Running → Stop → Configured (quick restart possible)");
    println!("  Any → Reset → Idle");
}

fn parse_args() -> Option<(Command, String)> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 3 {
        return None;
    }

    let command_str = &args[1];
    let address = args[2].clone();

    let command = match command_str.as_str() {
        "configure" => {
            // Parse --run <number> and optional --comment <text>
            let mut run_number: Option<u32> = None;
            let mut comment = String::new();
            let mut i = 3;

            while i < args.len() {
                match args[i].as_str() {
                    "--run" => {
                        if i + 1 < args.len() {
                            run_number = args[i + 1].parse().ok();
                            i += 2;
                        } else {
                            eprintln!("Error: --run requires a number");
                            std::process::exit(1);
                        }
                    }
                    "--comment" => {
                        if i + 1 < args.len() {
                            comment = args[i + 1].clone();
                            i += 2;
                        } else {
                            eprintln!("Error: --comment requires text");
                            std::process::exit(1);
                        }
                    }
                    _ => {
                        eprintln!("Unknown option: {}", args[i]);
                        std::process::exit(1);
                    }
                }
            }

            let run_number = match run_number {
                Some(n) => n,
                None => {
                    eprintln!("Error: configure requires --run <number>");
                    std::process::exit(1);
                }
            };

            Command::Configure(RunConfig { run_number, comment, exp_name: String::new() })
        }
        "arm" => Command::Arm,
        "start" => Command::Start,
        "stop" => Command::Stop,
        "reset" => Command::Reset,
        "status" => Command::GetStatus,
        _ => {
            eprintln!("Unknown command: {}", command_str);
            eprintln!("Use: configure, arm, start, stop, reset, or status");
            std::process::exit(1);
        }
    };

    Some((command, address))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (command, address) = match parse_args() {
        Some(result) => result,
        None => {
            print_usage();
            return Ok(());
        }
    };

    println!("Sending {} to {}", command, address);

    // Create REQ socket and connect
    let context = Context::new();
    let requester = request_reply::request(&context).connect(&address)?;

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
