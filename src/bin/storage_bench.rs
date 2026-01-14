//! Storage benchmark for fsync performance
//!
//! Usage:
//!   cargo run --release --bin storage_bench -- /path/to/test/dir

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const BATCH_SIZE_BYTES: usize = 1_100_000; // ~1.1MB (50K events * 22 bytes)
const ITERATIONS: usize = 100;

struct BenchResult {
    name: String,
    #[allow(dead_code)]
    total_time: Duration,
    throughput_mbps: f64,
    latency_per_batch_ms: f64,
    max_event_rate: f64, // events/sec assuming 50K events per batch
}

fn benchmark_no_fsync(path: &Path, data: &[u8]) -> BenchResult {
    let file_path = path.join("bench_no_fsync.dat");
    let file = File::create(&file_path).expect("Failed to create file");
    let mut writer = BufWriter::with_capacity(64 * 1024, file);

    let start = Instant::now();
    for _ in 0..ITERATIONS {
        writer.write_all(data).expect("Write failed");
    }
    writer.flush().expect("Flush failed");
    let total_time = start.elapsed();

    let _ = fs::remove_file(&file_path);

    let total_bytes = ITERATIONS * data.len();
    let throughput_mbps = total_bytes as f64 / total_time.as_secs_f64() / 1_000_000.0;
    let latency_per_batch_ms = total_time.as_secs_f64() * 1000.0 / ITERATIONS as f64;
    let max_event_rate = 50_000.0 / (latency_per_batch_ms / 1000.0);

    BenchResult {
        name: "No fsync".to_string(),
        total_time,
        throughput_mbps,
        latency_per_batch_ms,
        max_event_rate,
    }
}

fn benchmark_fsync_every_batch(path: &Path, data: &[u8]) -> BenchResult {
    let file_path = path.join("bench_fsync_every.dat");
    let file = File::create(&file_path).expect("Failed to create file");
    let mut writer = BufWriter::with_capacity(64 * 1024, file);

    let start = Instant::now();
    for _ in 0..ITERATIONS {
        writer.write_all(data).expect("Write failed");
        writer.flush().expect("Flush failed");
        writer.get_ref().sync_data().expect("fsync failed");
    }
    let total_time = start.elapsed();

    let _ = fs::remove_file(&file_path);

    let total_bytes = ITERATIONS * data.len();
    let throughput_mbps = total_bytes as f64 / total_time.as_secs_f64() / 1_000_000.0;
    let latency_per_batch_ms = total_time.as_secs_f64() * 1000.0 / ITERATIONS as f64;
    let max_event_rate = 50_000.0 / (latency_per_batch_ms / 1000.0);

    BenchResult {
        name: "fsync every batch".to_string(),
        total_time,
        throughput_mbps,
        latency_per_batch_ms,
        max_event_rate,
    }
}

fn benchmark_fsync_every_n(path: &Path, data: &[u8], n: usize) -> BenchResult {
    let file_path = path.join(format!("bench_fsync_every_{}.dat", n));
    let file = File::create(&file_path).expect("Failed to create file");
    let mut writer = BufWriter::with_capacity(64 * 1024, file);

    let start = Instant::now();
    for i in 0..ITERATIONS {
        writer.write_all(data).expect("Write failed");
        if (i + 1) % n == 0 {
            writer.flush().expect("Flush failed");
            writer.get_ref().sync_data().expect("fsync failed");
        }
    }
    writer.flush().expect("Final flush failed");
    writer.get_ref().sync_data().expect("Final fsync failed");
    let total_time = start.elapsed();

    let _ = fs::remove_file(&file_path);

    let total_bytes = ITERATIONS * data.len();
    let throughput_mbps = total_bytes as f64 / total_time.as_secs_f64() / 1_000_000.0;
    let latency_per_batch_ms = total_time.as_secs_f64() * 1000.0 / ITERATIONS as f64;
    let max_event_rate = 50_000.0 / (latency_per_batch_ms / 1000.0);

    BenchResult {
        name: format!("fsync every {} batches", n),
        total_time,
        throughput_mbps,
        latency_per_batch_ms,
        max_event_rate,
    }
}

fn benchmark_realistic_recorder(path: &Path, data: &[u8], fsync_interval: usize) -> BenchResult {
    // Simulate realistic recorder behavior:
    // - Write batches
    // - fsync at intervals
    // - Simulate sorting overhead (small sleep)

    let file_path = path.join("bench_realistic.dat");
    let file = File::create(&file_path).expect("Failed to create file");
    let mut writer = BufWriter::with_capacity(256 * 1024, file); // Larger buffer

    let start = Instant::now();
    for i in 0..ITERATIONS {
        // Simulate some CPU work (sorting would take ~1-2ms for 50K events)
        // We'll just do the write here
        writer.write_all(data).expect("Write failed");

        if (i + 1) % fsync_interval == 0 {
            writer.flush().expect("Flush failed");
            writer.get_ref().sync_data().expect("fsync failed");
        }
    }
    writer.flush().expect("Final flush failed");
    writer.get_ref().sync_data().expect("Final fsync failed");
    let total_time = start.elapsed();

    let _ = fs::remove_file(&file_path);

    let total_bytes = ITERATIONS * data.len();
    let throughput_mbps = total_bytes as f64 / total_time.as_secs_f64() / 1_000_000.0;
    let latency_per_batch_ms = total_time.as_secs_f64() * 1000.0 / ITERATIONS as f64;
    let max_event_rate = 50_000.0 / (latency_per_batch_ms / 1000.0);

    BenchResult {
        name: format!("Realistic (fsync/{})", fsync_interval),
        total_time,
        throughput_mbps,
        latency_per_batch_ms,
        max_event_rate,
    }
}

fn print_result(result: &BenchResult, target_rate: f64) {
    let status = if result.max_event_rate >= target_rate {
        "OK"
    } else {
        "NG"
    };

    println!(
        "  {:25} | {:>8.1} MB/s | {:>6.2} ms/batch | {:>8.1}M evt/s | {}",
        result.name,
        result.throughput_mbps,
        result.latency_per_batch_ms,
        result.max_event_rate / 1_000_000.0,
        status
    );
}

fn run_benchmarks(path: &Path, name: &str) {
    println!("\n{}", "=".repeat(60));
    println!("  Storage: {} ({})", name, path.display());
    println!("{}", "=".repeat(60));
    println!("  Batch size: {:.1} MB ({} events)",
             BATCH_SIZE_BYTES as f64 / 1_000_000.0,
             BATCH_SIZE_BYTES / 22);
    println!("  Iterations: {}", ITERATIONS);
    println!();

    // Create test data (random-ish to prevent compression)
    let data: Vec<u8> = (0..BATCH_SIZE_BYTES).map(|i| (i % 256) as u8).collect();

    let target_2m = 2_000_000.0;
    let target_10m = 10_000_000.0;

    println!("  {:25} | {:>11} | {:>13} | {:>13} | Status",
             "Mode", "Throughput", "Latency", "Max Rate");
    println!("  {:-<25}-+-{:-<11}-+-{:-<13}-+-{:-<13}-+-------", "", "", "", "");

    // Run benchmarks
    let results = vec![
        benchmark_no_fsync(path, &data),
        benchmark_fsync_every_batch(path, &data),
        benchmark_fsync_every_n(path, &data, 5),
        benchmark_fsync_every_n(path, &data, 10),
        benchmark_fsync_every_n(path, &data, 20),
        benchmark_realistic_recorder(path, &data, 10),
    ];

    println!("\n  Target: 2 MHz");
    for result in &results {
        print_result(result, target_2m);
    }

    println!("\n  Target: 10 MHz");
    for result in &results {
        print_result(result, target_10m);
    }

    // Summary
    println!();
    let best_for_2m = results.iter()
        .filter(|r| r.max_event_rate >= target_2m)
        .min_by(|a, b| a.latency_per_batch_ms.partial_cmp(&b.latency_per_batch_ms).unwrap());

    let best_for_10m = results.iter()
        .filter(|r| r.max_event_rate >= target_10m)
        .min_by(|a, b| a.latency_per_batch_ms.partial_cmp(&b.latency_per_batch_ms).unwrap());

    if let Some(best) = best_for_2m {
        println!("  Best for 2 MHz:  {} ({:.1}M evt/s)", best.name, best.max_event_rate / 1_000_000.0);
    } else {
        println!("  Best for 2 MHz:  NONE - storage too slow!");
    }

    if let Some(best) = best_for_10m {
        println!("  Best for 10 MHz: {} ({:.1}M evt/s)", best.name, best.max_event_rate / 1_000_000.0);
    } else {
        println!("  Best for 10 MHz: NONE - storage too slow!");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        println!("Storage Benchmark for DELILA Recorder");
        println!();
        println!("Usage: {} <path1> [path2] ...", args[0]);
        println!();
        println!("Example:");
        println!("  {} /tmp ./data /Volumes/Data20TB/bench", args[0]);
        std::process::exit(1);
    }

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║         DELILA Storage Benchmark                             ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║  Testing fsync performance for data integrity                ║");
    println!("║  Batch: ~1.1MB (50K events × 22 bytes)                       ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    for path_str in &args[1..] {
        let path = PathBuf::from(path_str);

        if !path.exists() {
            println!("\n  Creating directory: {}", path.display());
            if let Err(e) = fs::create_dir_all(&path) {
                println!("  ERROR: Failed to create directory: {}", e);
                continue;
            }
        }

        // Determine storage name
        let name = if path_str.contains("Volumes") {
            "USB HDD"
        } else if path_str == "/tmp" || path_str.starts_with("/tmp") {
            "NVMe SSD (tmpfs/SSD)"
        } else if path_str.contains("WorkSpace") || path_str == "." {
            "NVMe SSD (Project)"
        } else {
            "Unknown"
        };

        run_benchmarks(&path, name);
    }

    println!("\n{}", "=".repeat(60));
    println!("  Benchmark complete.");
    println!();
    println!("  Recommendation:");
    println!("  - For maximum data safety: fsync every batch (if rate allows)");
    println!("  - For balance: fsync every 10 batches (~10MB intervals)");
    println!("  - For maximum throughput: fsync at file rotation only");
    println!();
}
