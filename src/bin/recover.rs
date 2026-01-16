//! delila-recover - Data recovery tool for DELILA data files
//!
//! Usage:
//!   delila-recover validate <file>      - Check file integrity
//!   delila-recover info <file>          - Show file metadata
//!   delila-recover recover <file> [--output <path>]  - Recover data from incomplete file
//!   delila-recover list <directory>     - List all .delila files with status

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use delila_rs::recorder::{ChecksumCalculator, DataFileReader, FileFooter, FileValidationResult};

#[derive(Parser)]
#[command(name = "delila-recover")]
#[command(about = "Data recovery tool for DELILA data files")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Validate file integrity
    Validate {
        /// Path to the .delila file
        file: PathBuf,
    },

    /// Show file metadata
    Info {
        /// Path to the .delila file
        file: PathBuf,
    },

    /// Recover data from incomplete file
    Recover {
        /// Path to the .delila file
        file: PathBuf,

        /// Output path (default: <input>_recovered.delila)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// List all .delila files in a directory
    List {
        /// Directory to scan
        directory: PathBuf,

        /// Include subdirectories
        #[arg(short, long)]
        recursive: bool,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Validate { file } => {
            if let Err(e) = validate_file(&file) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Info { file } => {
            if let Err(e) = show_info(&file) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Recover { file, output } => {
            if let Err(e) = recover_file(&file, output) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::List {
            directory,
            recursive,
        } => {
            if let Err(e) = list_files(&directory, recursive) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    }
}

fn validate_file(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    println!("Validating: {}", path.display());
    println!();

    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut data_reader = DataFileReader::new(reader)?;

    let result = data_reader.validate();
    print_validation_result(&result);

    if result.is_valid {
        println!("\n\x1b[32m✓ File is valid\x1b[0m");
    } else if result.needs_recovery() {
        println!("\n\x1b[33m⚠ File needs recovery\x1b[0m");
        println!("  Run: delila-recover recover \"{}\"", path.display());
    } else {
        println!("\n\x1b[31m✗ File is corrupted\x1b[0m");
    }

    Ok(())
}

fn show_info(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut data_reader = DataFileReader::new(reader)?;

    println!("File: {}", path.display());
    println!("Size: {} bytes", std::fs::metadata(path)?.len());
    println!();

    // Header info
    if let Some(header) = data_reader.header() {
        println!("=== Header ===");
        println!("  Version:        {}", header.version);
        println!("  Run Number:     {}", header.run_number);
        println!("  Experiment:     {}", header.exp_name);
        println!("  File Sequence:  {}", header.file_sequence);
        println!("  Comment:        {}", header.comment);
        println!("  Sorted:         {}", header.is_sorted);
        println!("  Sort Margin:    {}%", header.sort_margin_ratio * 100.0);

        let start_time = header.file_start_time_ns / 1_000_000_000;
        println!("  Start Time:     {} (unix timestamp)", start_time);

        if !header.source_ids.is_empty() {
            println!("  Source IDs:     {:?}", header.source_ids);
        }
        if !header.metadata.is_empty() {
            println!("  Metadata:       {:?}", header.metadata);
        }
    }

    // Footer info
    println!();
    match data_reader.read_footer() {
        Ok(footer) => {
            println!("=== Footer ===");
            println!("  Complete:       {}", footer.is_complete());
            println!("  Total Events:   {}", footer.total_events);
            println!("  Data Bytes:     {}", footer.data_bytes);
            println!("  Checksum:       {:016x}", footer.data_checksum);
            println!(
                "  Time Range:     {:.3} - {:.3} ns",
                footer.first_event_time_ns, footer.last_event_time_ns
            );

            let end_time = footer.file_end_time_ns / 1_000_000_000;
            println!("  End Time:       {} (unix timestamp)", end_time);
        }
        Err(e) => {
            println!("=== Footer ===");
            println!("  \x1b[33mCould not read footer: {}\x1b[0m", e);
        }
    }

    Ok(())
}

fn recover_file(
    input_path: &Path,
    output_path: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Recovering: {}", input_path.display());

    let file = File::open(input_path)?;
    let reader = BufReader::new(file);
    let mut data_reader = DataFileReader::new(reader)?;

    // Validate first
    let result = data_reader.validate();

    if result.is_valid {
        println!("\x1b[32m✓ File is already valid, no recovery needed\x1b[0m");
        return Ok(());
    }

    if !result.needs_recovery() {
        return Err("No recoverable data found".into());
    }

    println!(
        "  Found {} recoverable blocks with {} events",
        result.recoverable_blocks, result.recoverable_events
    );

    // Determine output path
    let output = output_path.unwrap_or_else(|| {
        let stem = input_path.file_stem().unwrap_or_default().to_string_lossy();
        let parent = input_path.parent().unwrap_or(Path::new("."));
        parent.join(format!("{}_recovered.delila", stem))
    });

    println!("  Output: {}", output.display());

    // Create output file
    let out_file = File::create(&output)?;
    let mut writer = BufWriter::with_capacity(64 * 1024, out_file);

    // Write header (copy from original)
    let header = result.header.clone().ok_or("No header found")?;
    let header_bytes = header
        .to_bytes()
        .map_err(|e| format!("Failed to serialize header: {}", e))?;
    writer.write_all(&header_bytes)?;

    // Prepare footer and checksum
    let mut footer = FileFooter::new();
    let mut checksum = ChecksumCalculator::new();

    // Copy recoverable data blocks
    let mut events_written = 0u64;
    let mut blocks_written = 0usize;

    for batch_result in data_reader.data_blocks() {
        match batch_result {
            Ok(batch) => {
                // Update timestamp range
                if let (Some(first), Some(last)) = (batch.events.first(), batch.events.last()) {
                    footer.update_timestamp_range(first.timestamp_ns, last.timestamp_ns);
                }

                // Serialize batch
                let data = batch
                    .to_msgpack()
                    .map_err(|e| format!("Serialization error: {}", e))?;
                let len_bytes = (data.len() as u32).to_le_bytes();

                writer.write_all(&len_bytes)?;
                writer.write_all(&data)?;

                // Update checksum
                checksum.update(&len_bytes);
                checksum.update(&data);

                events_written += batch.events.len() as u64;
                blocks_written += 1;
            }
            Err(e) => {
                println!(
                    "  \x1b[33mStopped at block {}: {}\x1b[0m",
                    blocks_written, e
                );
                break;
            }
        }
    }

    // Finalize and write footer
    footer.total_events = events_written;
    footer.data_checksum = checksum.finalize();
    footer.data_bytes = checksum.bytes_processed();
    footer.finalize();

    let footer_bytes = footer.to_bytes();
    writer.write_all(&footer_bytes)?;

    writer.flush()?;
    writer.get_ref().sync_all()?;

    println!();
    println!("\x1b[32m✓ Recovery complete\x1b[0m");
    println!("  Blocks written: {}", blocks_written);
    println!("  Events written: {}", events_written);
    println!(
        "  Output size:    {} bytes",
        std::fs::metadata(&output)?.len()
    );

    Ok(())
}

fn list_files(directory: &Path, recursive: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("Scanning: {}", directory.display());
    println!();

    let mut files = Vec::new();
    collect_delila_files(directory, recursive, &mut files)?;

    if files.is_empty() {
        println!("No .delila files found");
        return Ok(());
    }

    println!(
        "{:<50} {:>12} {:>10} {:>8}",
        "File", "Events", "Size (MB)", "Status"
    );
    println!("{}", "-".repeat(84));

    let mut total_events = 0u64;
    let mut total_size = 0u64;
    let mut valid_count = 0;
    let mut needs_recovery_count = 0;
    let mut corrupted_count = 0;

    for path in &files {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        total_size += size;

        let (events, status) = match File::open(path) {
            Ok(file) => {
                let reader = BufReader::new(file);
                match DataFileReader::new(reader) {
                    Ok(mut data_reader) => {
                        let result = data_reader.validate();
                        let events = if result.is_valid {
                            result
                                .footer
                                .as_ref()
                                .map(|f| f.total_events)
                                .unwrap_or(result.recoverable_events)
                        } else {
                            result.recoverable_events
                        };

                        let status = if result.is_valid {
                            valid_count += 1;
                            total_events += events;
                            "\x1b[32m✓ Valid\x1b[0m"
                        } else if result.needs_recovery() {
                            needs_recovery_count += 1;
                            total_events += events;
                            "\x1b[33m⚠ Needs recovery\x1b[0m"
                        } else {
                            corrupted_count += 1;
                            "\x1b[31m✗ Corrupted\x1b[0m"
                        };
                        (events, status)
                    }
                    Err(_) => {
                        corrupted_count += 1;
                        (0, "\x1b[31m✗ Corrupted\x1b[0m")
                    }
                }
            }
            Err(_) => {
                corrupted_count += 1;
                (0, "\x1b[31m✗ Unreadable\x1b[0m")
            }
        };

        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        let filename_display = if filename.len() > 48 {
            format!("...{}", &filename[filename.len() - 45..])
        } else {
            filename.to_string()
        };

        println!(
            "{:<50} {:>12} {:>10.2} {}",
            filename_display,
            events,
            size as f64 / 1_000_000.0,
            status
        );
    }

    println!("{}", "-".repeat(84));
    println!(
        "Total: {} files, {} events, {:.2} MB",
        files.len(),
        total_events,
        total_size as f64 / 1_000_000.0
    );
    println!(
        "Status: {} valid, {} needs recovery, {} corrupted",
        valid_count, needs_recovery_count, corrupted_count
    );

    Ok(())
}

fn collect_delila_files(
    dir: &Path,
    recursive: bool,
    files: &mut Vec<PathBuf>,
) -> Result<(), std::io::Error> {
    if !dir.is_dir() {
        return Ok(());
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext == "delila" {
                    files.push(path);
                }
            }
        } else if recursive && path.is_dir() {
            collect_delila_files(&path, recursive, files)?;
        }
    }

    files.sort();
    Ok(())
}

fn print_validation_result(result: &FileValidationResult) {
    println!("=== Validation Result ===");
    println!("  Valid:              {}", result.is_valid);
    println!("  Recoverable blocks: {}", result.recoverable_blocks);
    println!("  Recoverable events: {}", result.recoverable_events);

    if !result.errors.is_empty() {
        println!("  Errors:");
        for error in &result.errors {
            println!("    - {}", error);
        }
    }
}
