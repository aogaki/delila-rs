//! delila_source_replay - reads `.delila` files and impersonates a Reader.
//!
//! Reads one or more `.delila` recorder output files, filters the embedded
//! [`EventDataBatch`] stream to a single `source_id`, and publishes the
//! matching batches verbatim over a ZMQ PUB socket — exactly as a real
//! CAEN digitizer Reader would. The state machine + command REP socket
//! mirror [`data_source_emulator`] so the Operator (REST + UI) can drive
//! Configure / Arm / Start / Stop / Reset transparently.
//!
//! # Production-like replay stack
//!
//! Run one instance per `source_id` (12 for the typical fission setup).
//! Each instance reads the same `.delila` set, skips batches whose
//! `source_id` doesn't match, and re-numbers `sequence_number` per-source
//! so the Merger sees one monotonic counter per upstream.
//!
//! Usage:
//!
//! ```text
//! cargo run --release --features dev-tools --bin delila_source_replay -- \
//!     --config config/config_eb_replay_stack.toml \
//!     --source-id 0 \
//!     --input data/run0042_0000.delila
//! ```
//!
//! The CLI computes its `bind` / `command` addresses from
//! `network.port_base_data` + `source_id` and `network.port_base_command`
//! + `source_id` so that the Operator's auto-resolved subscribe list
//!   lines up without any extra plumbing.

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use clap::Parser;
use futures::SinkExt;
use tmq::{publish, Context};
use tokio::sync::{watch, Mutex};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use delila_rs::common::{
    handle_command, pub_no_hwm, run_command_task, CommandHandlerExt, ComponentMetrics,
    ComponentSharedState, ComponentState, EventDataBatch, Message,
};
use delila_rs::config::Config;
use delila_rs::recorder::DataFileReader;

#[derive(Parser, Debug)]
#[command(
    name = "delila_source_replay",
    about = "Replay .delila files as a Reader-like data source (state machine + ZMQ PUB)"
)]
struct Args {
    /// Configuration file path (TOML) — used to compute bind / command addresses
    #[arg(short = 'f', long = "config", default_value = "config.toml")]
    config_file: String,

    /// Source ID this instance impersonates (filters batches whose `source_id`
    /// does not match)
    #[arg(long = "source-id", default_value_t = 0)]
    source_id: u32,

    /// Input `.delila` files (sent in order). Required.
    #[arg(short = 'i', long = "input", required = true, num_args = 1..)]
    input: Vec<PathBuf>,

    /// Loop the input forever — when the file list is exhausted, restart from
    /// the first file. Useful for long-running stress tests.
    #[arg(long = "loop-replay", default_value_t = false)]
    loop_replay: bool,

    /// Delay between PUB sends in milliseconds (0 = burst)
    #[arg(long = "delay-ms", default_value_t = 0)]
    delay_ms: u64,

    /// Override the auto-computed data bind address (e.g. `tcp://*:7200`)
    #[arg(long = "bind")]
    bind: Option<String>,

    /// Override the auto-computed command bind address (e.g. `tcp://*:7300`)
    #[arg(long = "command")]
    command: Option<String>,
}

/// Lock-free statistics shared between the publisher loop and the command
/// task (for `status_details` / `get_metrics` reporting).
#[derive(Default, Debug)]
struct Stats {
    events_sent: AtomicU64,
    batches_sent: AtomicU64,
    bytes_sent: AtomicU64,
    batches_skipped: AtomicU64,
}

impl Stats {
    fn reset(&self) {
        self.events_sent.store(0, Ordering::Relaxed);
        self.batches_sent.store(0, Ordering::Relaxed);
        self.bytes_sent.store(0, Ordering::Relaxed);
        self.batches_skipped.store(0, Ordering::Relaxed);
    }

    fn snapshot(&self) -> (u64, u64, u64, u64) {
        (
            self.events_sent.load(Ordering::Relaxed),
            self.batches_sent.load(Ordering::Relaxed),
            self.bytes_sent.load(Ordering::Relaxed),
            self.batches_skipped.load(Ordering::Relaxed),
        )
    }
}

/// 1-second rolling event-rate tracker (matches `data_source_emulator::RateTracker`).
struct RateTracker {
    prev_events: AtomicU64,
    prev_time: std::sync::Mutex<Option<Instant>>,
    current_rate: AtomicU64,
}

impl RateTracker {
    fn new() -> Self {
        Self {
            prev_events: AtomicU64::new(0),
            prev_time: std::sync::Mutex::new(None),
            current_rate: AtomicU64::new(0),
        }
    }

    fn update(&self, current_events: u64) {
        let now = Instant::now();
        let mut prev_time_guard = self.prev_time.lock().unwrap();
        if let Some(prev_time) = *prev_time_guard {
            let elapsed = now.duration_since(prev_time).as_secs_f64();
            if elapsed >= 1.0 {
                let prev_events = self.prev_events.load(Ordering::Relaxed);
                let delta = current_events.saturating_sub(prev_events);
                let rate = (delta as f64 / elapsed) as u64;
                self.current_rate.store(rate, Ordering::Relaxed);
                self.prev_events.store(current_events, Ordering::Relaxed);
                *prev_time_guard = Some(now);
            }
        } else {
            self.prev_events.store(current_events, Ordering::Relaxed);
            *prev_time_guard = Some(now);
        }
    }

    fn rate_hz(&self) -> u64 {
        self.current_rate.load(Ordering::Relaxed)
    }

    fn reset(&self) {
        self.prev_events.store(0, Ordering::Relaxed);
        self.current_rate.store(0, Ordering::Relaxed);
        *self.prev_time.lock().unwrap() = None;
    }
}

/// Command-handler hooks for the replay source.
///
/// `on_start` clears the stats and signals the publisher loop to reset its
/// per-source sequence counter and re-open the input files from index 0.
/// `on_stop` flips `pending_eos` so the publisher loop emits a single
/// `Message::EndOfStream` once it observes the state leaving `Running`.
struct ReplayCommandExt {
    stats: Arc<Stats>,
    rate_tracker: Arc<RateTracker>,
    /// Bumped by `on_start`. The publisher loop watches this to know when
    /// it should rewind to the first input file. (`state_rx` already
    /// signals the Running transition; the generation counter is just for
    /// idempotent rewinds when start-stop-start happens fast.)
    start_generation: Arc<AtomicU64>,
    /// Set by `on_stop` so the publisher loop emits EOS exactly once.
    pending_eos: Arc<AtomicBool>,
}

impl CommandHandlerExt for ReplayCommandExt {
    fn component_name(&self) -> &'static str {
        "DelilaReplay"
    }

    fn on_start(&mut self, _run_number: u32) -> Result<(), String> {
        self.stats.reset();
        self.rate_tracker.reset();
        self.start_generation.fetch_add(1, Ordering::Release);
        self.pending_eos.store(false, Ordering::Release);
        Ok(())
    }

    fn on_stop(&mut self) -> Result<(), String> {
        self.pending_eos.store(true, Ordering::Release);
        Ok(())
    }

    fn status_details(&self) -> Option<String> {
        let (events, batches, bytes, skipped) = self.stats.snapshot();
        Some(format!(
            "Events: {events}, Batches: {batches}, Bytes: {bytes}, Skipped: {skipped}"
        ))
    }

    fn get_metrics(&self) -> Option<ComponentMetrics> {
        let (events, _batches, bytes, _skipped) = self.stats.snapshot();
        self.rate_tracker.update(events);
        Some(ComponentMetrics {
            events_processed: events,
            bytes_transferred: bytes,
            queue_size: 0,
            queue_max: 0,
            queue_bytes: 0,
            queue_bytes_peak: 0,
            backlog_level: 0,
            event_rate: self.rate_tracker.rate_hz() as f64,
            data_rate: 0.0,
            trigger_loss_count: 0,
            trigger_loss_rate: 0.0,
            channel_counts: None,
        })
    }
}

fn resolve_bind_addresses(args: &Args, config: &Config) -> (String, String) {
    let net = &config.network;
    let bind = args
        .bind
        .clone()
        .unwrap_or_else(|| format!("tcp://*:{}", net.port_base_data + args.source_id as u16));
    let command = args
        .command
        .clone()
        .unwrap_or_else(|| format!("tcp://*:{}", net.port_base_command + args.source_id as u16));
    (bind, command)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("delila_rs=info".parse()?))
        .init();

    let args = Args::parse();

    // Validate that each input exists up front (Configure semantics).
    for path in &args.input {
        if !path.exists() {
            anyhow::bail!("Input file does not exist: {}", path.display());
        }
    }

    let config = Config::load(&args.config_file)
        .with_context(|| format!("Failed to load TOML config from {}", args.config_file))?;

    let (bind_addr, cmd_addr) = resolve_bind_addresses(&args, &config);

    let ctx = Context::new();
    let mut data_socket = publish(&ctx).bind(&bind_addr)?;
    pub_no_hwm(&data_socket).map_err(|e| anyhow::anyhow!("pub_no_hwm: {e}"))?;

    let (state_tx, mut state_rx) = watch::channel(ComponentState::Idle);
    let shared_state = Arc::new(Mutex::new(ComponentSharedState::new()));
    let stats = Arc::new(Stats::default());
    let rate_tracker = Arc::new(RateTracker::new());
    let start_generation = Arc::new(AtomicU64::new(0));
    let pending_eos = Arc::new(AtomicBool::new(false));

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);

    // Spawn ctrl-C / SIGTERM handler that broadcasts shutdown.
    let shutdown_tx_signal = shutdown_tx.clone();
    tokio::spawn(async move {
        let mut sigterm =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(s) => s,
                Err(e) => {
                    warn!(error = %e, "Failed to install SIGTERM handler");
                    return;
                }
            };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => info!("SIGINT received"),
            _ = sigterm.recv() => info!("SIGTERM received"),
        }
        let _ = shutdown_tx_signal.send(());
    });

    // Spawn command task (Operator-facing REP).
    let cmd_shutdown = shutdown_tx.subscribe();
    let cmd_shared = shared_state.clone();
    let cmd_state_tx = state_tx.clone();
    let cmd_stats = stats.clone();
    let cmd_rate = rate_tracker.clone();
    let cmd_start_gen = start_generation.clone();
    let cmd_pending_eos = pending_eos.clone();
    let cmd_addr_for_task = cmd_addr.clone();
    let cmd_handle = tokio::spawn(async move {
        run_command_task(
            cmd_addr_for_task,
            cmd_shared,
            cmd_state_tx,
            cmd_shutdown,
            move |state, tx, cmd| {
                let mut ext = ReplayCommandExt {
                    stats: cmd_stats.clone(),
                    rate_tracker: cmd_rate.clone(),
                    start_generation: cmd_start_gen.clone(),
                    pending_eos: cmd_pending_eos.clone(),
                };
                handle_command(state, tx, cmd, Some(&mut ext))
            },
            "DelilaReplay",
        )
        .await;
    });

    println!("========================================");
    println!("  DELILA Source Replay");
    println!("========================================");
    println!("  Source ID:    {}", args.source_id);
    println!("  Data PUB:     {bind_addr}");
    println!("  Command REP:  {cmd_addr}");
    println!("  Inputs:       {} file(s)", args.input.len());
    println!("  loop_replay:  {}", args.loop_replay);
    println!("  delay_ms:     {}", args.delay_ms);
    println!("========================================");

    // ── Publisher main loop ─────────────────────────────────────────────────
    //
    // Outer loop: wait for `Running` → replay → wait for state-leave →
    // emit EOS → loop. Inside the replay, we re-check `state_rx` every
    // batch so the operator's Stop is honored mid-file.
    let mut next_seq: u64 = 0;
    let mut current_run_number: u32 = 0;
    let mut last_start_gen: u64 = 0;

    'outer: loop {
        // Wait for Running (or shutdown).
        while *state_rx.borrow() != ComponentState::Running {
            tokio::select! {
                biased;
                _ = shutdown_rx.recv() => break 'outer,
                _ = state_rx.changed() => continue,
            }
        }

        // Capture run number + generation once per Start.
        current_run_number = shared_state.lock().await.run_number().unwrap_or(0);
        next_seq = 0;
        last_start_gen = start_generation.load(Ordering::Acquire);
        info!(
            run_number = current_run_number,
            source_id = args.source_id,
            "Running — beginning replay"
        );

        let mut auto_eos_sent = false;
        let mut files_exhausted = false;

        // Inner replay loop.
        'replay: loop {
            for path in &args.input {
                let file = match File::open(path) {
                    Ok(f) => f,
                    Err(e) => {
                        warn!(
                            file = %path.display(),
                            error = %e,
                            "Failed to open input file, skipping"
                        );
                        continue;
                    }
                };
                let reader = match DataFileReader::new(BufReader::new(file)) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(
                            file = %path.display(),
                            error = %e,
                            "Failed to parse .delila header, skipping"
                        );
                        continue;
                    }
                };

                let mut reader = reader;
                for block_result in reader.data_blocks() {
                    // Honor Stop / Reset mid-file.
                    if *state_rx.borrow() != ComponentState::Running {
                        break 'replay;
                    }
                    if start_generation.load(Ordering::Acquire) != last_start_gen {
                        // A new Start arrived (e.g. fast restart) — rewind.
                        break 'replay;
                    }

                    match block_result {
                        Ok(mut batch) => {
                            if batch.source_id != args.source_id {
                                stats.batches_skipped.fetch_add(1, Ordering::Relaxed);
                                continue;
                            }
                            batch.sequence_number = next_seq;
                            next_seq += 1;
                            let nevents = batch.events.len() as u64;
                            if let Err(e) = publish_data(&mut data_socket, &stats, batch).await {
                                warn!(error = %e, "Publish failed; breaking replay loop");
                                break 'replay;
                            }
                            if args.delay_ms > 0 {
                                tokio::time::sleep(Duration::from_millis(args.delay_ms)).await;
                            }
                            let _ = nevents; // covered by publish_data stats
                        }
                        Err(e) => {
                            warn!(
                                file = %path.display(),
                                error = %e,
                                "Skipping corrupted block"
                            );
                            continue;
                        }
                    }
                }
            }

            // File list exhausted.
            if args.loop_replay && *state_rx.borrow() == ComponentState::Running {
                info!("Reached end of input list — looping (--loop-replay)");
                continue 'replay;
            }
            files_exhausted = *state_rx.borrow() == ComponentState::Running;
            break 'replay;
        }

        // Emit auto-EOS if files ran out while still Running so downstream
        // (recorder / EB) can finalize without requiring an Operator Stop.
        if files_exhausted {
            info!(
                source_id = args.source_id,
                run_number = current_run_number,
                "Input exhausted while Running — emitting auto-EOS"
            );
            let eos = Message::eos(args.source_id, current_run_number);
            let _ = publish_message(&mut data_socket, &stats, &eos).await;
            auto_eos_sent = true;
        }

        // Wait for state to leave Running (Stop / Reset) — or shutdown.
        if *state_rx.borrow() == ComponentState::Running {
            tokio::select! {
                biased;
                _ = shutdown_rx.recv() => break 'outer,
                _ = state_rx.changed() => {}
            }
        }

        // Clear the pending_eos flag (it's now consumed by this transition).
        pending_eos.store(false, Ordering::Release);

        // After Stop / Reset: if we didn't auto-EOS, send it now so the
        // downstream EB pipeline closes cleanly.
        if !auto_eos_sent {
            let eos = Message::eos(args.source_id, current_run_number);
            let _ = publish_message(&mut data_socket, &stats, &eos).await;
        }
    }

    // Shutdown path: if we were Running, emit one last EOS.
    if *state_rx.borrow() == ComponentState::Running {
        let eos = Message::eos(args.source_id, current_run_number);
        let _ = publish_message(&mut data_socket, &stats, &eos).await;
    }
    drop(data_socket);

    // Wait for command task to terminate.
    let _ = cmd_handle.await;

    let (events, batches, bytes, skipped) = stats.snapshot();
    println!();
    println!("========================================");
    println!("  delila_source_replay finished");
    println!("========================================");
    println!("  Events sent:     {events}");
    println!("  Batches sent:    {batches}");
    println!("  Batches skipped: {skipped}");
    println!("  Bytes sent:      {bytes}");
    println!("========================================");
    let _ = next_seq;
    let _ = last_start_gen;

    Ok(())
}

async fn publish_data(
    socket: &mut publish::Publish,
    stats: &Stats,
    batch: EventDataBatch,
) -> anyhow::Result<()> {
    let n = batch.events.len() as u64;
    let msg = Message::data(batch);
    let bytes = msg.to_msgpack()?;
    let bytes_len = bytes.len() as u64;
    let zmq_msg: tmq::Multipart = vec![tmq::Message::from(bytes.as_slice())].into();
    socket.send(zmq_msg).await?;
    stats.events_sent.fetch_add(n, Ordering::Relaxed);
    stats.batches_sent.fetch_add(1, Ordering::Relaxed);
    stats.bytes_sent.fetch_add(bytes_len, Ordering::Relaxed);
    Ok(())
}

async fn publish_message(
    socket: &mut publish::Publish,
    stats: &Stats,
    msg: &Message,
) -> anyhow::Result<()> {
    let bytes = msg.to_msgpack()?;
    let bytes_len = bytes.len() as u64;
    let zmq_msg: tmq::Multipart = vec![tmq::Message::from(bytes.as_slice())].into();
    socket.send(zmq_msg).await?;
    stats.bytes_sent.fetch_add(bytes_len, Ordering::Relaxed);
    Ok(())
}
