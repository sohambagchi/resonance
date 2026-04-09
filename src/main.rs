/// Resonance — Memory Hierarchy Characterisation Tool
///
/// CLI entry point.  All measurement logic lives in the library crate.
use clap::Parser;
use resonance::orchestrator::{self, Config};

// ---------------------------------------------------------------------------
// CLI definition (DESIGN.md §18)
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "resonance",
    version,
    about = "Automatically characterise the memory hierarchy of the current machine"
)]
struct Cli {
    /// Emit JSON to stdout instead of human-readable output.
    #[arg(long)]
    json: bool,

    /// Pin to CPU core N (default: 0).
    #[arg(long, default_value_t = 0)]
    core: usize,

    /// Maximum memory range, e.g. 512M, 2G (default: auto).
    #[arg(long)]
    max_mem: Option<String>,

    /// Trials per measurement point (default: 11).
    #[arg(long, default_value_t = 11)]
    trials: usize,

    /// Skip mlockall (useful if running without privileges).
    #[arg(long)]
    no_lock: bool,

    /// Skip thread pinning.
    #[arg(long)]
    no_pin: bool,

    /// Skip bandwidth measurements (faster run).
    #[arg(long)]
    skip_bandwidth: bool,

    /// Skip MLP measurements (faster run).
    #[arg(long)]
    skip_mlp: bool,

    /// Skip TLB measurements.
    #[arg(long)]
    skip_tlb: bool,

    /// RNG seed for chain construction (default: 42).
    #[arg(long, default_value_t = 42)]
    seed: u64,

    /// Print per-experiment progress to stderr during run.
    #[arg(short, long)]
    verbose: bool,
}

// ---------------------------------------------------------------------------
// Size parsing
// ---------------------------------------------------------------------------

/// Parse a human-readable size string like `"512M"` or `"2G"` into bytes.
fn parse_size(s: &str) -> Result<usize, String> {
    let s = s.trim();
    let (num_part, multiplier) = if let Some(n) = s.strip_suffix('G') {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix('M') {
        (n, 1024 * 1024)
    } else if let Some(n) = s.strip_suffix('K') {
        (n, 1024)
    } else {
        (s, 1)
    };
    let value: usize = num_part
        .parse()
        .map_err(|_| format!("invalid size: '{s}'"))?;
    Ok(value * multiplier)
}

// ---------------------------------------------------------------------------
// Human-readable output (DESIGN.md §17.1)
// ---------------------------------------------------------------------------

fn print_human(results: &resonance::results::ResonanceResults) {
    println!("Resonance — Memory Hierarchy Characterisation");
    println!("==============================================");
    println!("System : {} {}", results.platform.os, results.platform.arch);
    println!(
        "CPU    : ~{:.2} GHz (measured){}",
        results.cpu_freq_ghz,
        results
            .cpu_freq_os_ghz
            .map(|g| format!(", {:.0} MHz (reported)", g * 1000.0))
            .unwrap_or_default()
    );
    println!("Pinned : core {}", results.thread_core);
    println!(
        "Locked : {}",
        if results.memory_locked { "yes" } else { "no" }
    );
    println!(
        "Timer  : {:.1} ns granularity",
        results.timer_granularity_ns
    );
    println!();

    if results.cache.is_none()
        && results.tlb.is_none()
        && results.bandwidth.is_none()
        && results.mlp.is_none()
    {
        println!("(No measurement phases executed yet — scaffold only.)");
    }

    println!();
    println!("Completed in {} ms.", results.duration_ms);
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();

    let config = Config {
        json: cli.json,
        core: cli.core,
        max_mem_bytes: cli.max_mem.as_deref().map(|s| {
            parse_size(s).unwrap_or_else(|e| {
                eprintln!("error: {e}");
                std::process::exit(1);
            })
        }),
        trials: cli.trials,
        no_lock: cli.no_lock,
        no_pin: cli.no_pin,
        skip_bandwidth: cli.skip_bandwidth,
        skip_mlp: cli.skip_mlp,
        skip_tlb: cli.skip_tlb,
        seed: cli.seed,
        verbose: cli.verbose,
    };

    match orchestrator::run(&config) {
        Ok(results) => {
            if config.json {
                match serde_json::to_string_pretty(&results) {
                    Ok(json) => println!("{json}"),
                    Err(e) => {
                        eprintln!("error serialising results: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                print_human(&results);
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
