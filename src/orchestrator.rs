/// Experiment orchestrator (DESIGN.md §5.1, §16.1).
///
/// Drives the measurement sequence, wires platform initialisation into the
/// timer/kernel infrastructure, and collects results into
/// [`ResonanceResults`](crate::results::ResonanceResults).
use crate::arch;
use crate::constants;
use crate::platform;
use crate::results::{PlatformInfo, ResonanceResults};
use crate::timer;

use std::fmt;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum OrchestratorError {
    Platform(platform::PlatformError),
    Other(String),
}

impl fmt::Display for OrchestratorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Platform(e) => write!(f, "platform: {e}"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for OrchestratorError {}

impl From<platform::PlatformError> for OrchestratorError {
    fn from(e: platform::PlatformError) -> Self {
        Self::Platform(e)
    }
}

// ---------------------------------------------------------------------------
// Configuration — mirrors CLI flags
// ---------------------------------------------------------------------------

/// Runtime configuration collected from CLI arguments.
#[derive(Debug, Clone)]
pub struct Config {
    pub json: bool,
    pub core: usize,
    pub max_mem_bytes: Option<usize>,
    pub trials: usize,
    pub no_lock: bool,
    pub no_pin: bool,
    pub skip_bandwidth: bool,
    pub skip_mlp: bool,
    pub skip_tlb: bool,
    pub seed: u64,
    pub verbose: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            json: false,
            core: 0,
            max_mem_bytes: None,
            trials: constants::NUMTRIES,
            no_lock: false,
            no_pin: false,
            skip_bandwidth: false,
            skip_mlp: false,
            skip_tlb: false,
            seed: 42,
            verbose: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Log buffer — all output is deferred until after measurement (§16.3)
// ---------------------------------------------------------------------------

/// Buffered log messages emitted to stderr after measurement completes.
pub struct LogBuffer {
    entries: Vec<String>,
}

impl LogBuffer {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Record a log message (not yet printed).
    pub fn log(&mut self, msg: impl Into<String>) {
        self.entries.push(msg.into());
    }

    /// Flush all buffered messages to stderr.
    pub fn flush(&self) {
        for entry in &self.entries {
            eprintln!("{entry}");
        }
    }
}

// ---------------------------------------------------------------------------
// Orchestrator
// ---------------------------------------------------------------------------

/// Run the full experiment sequence described in DESIGN.md §16.1.
///
/// Returns the top-level result structure ready for JSON serialisation or
/// human-readable printing.
pub fn run(config: &Config) -> Result<ResonanceResults, OrchestratorError> {
    let mut log = LogBuffer::new();
    let start_ns = platform::clock_ns();

    // ------------------------------------------------------------------
    // 0. Platform setup
    // ------------------------------------------------------------------
    let thread_pinned;
    if config.no_pin {
        thread_pinned = false;
        log.log("thread pinning: skipped (--no-pin)");
    } else {
        match platform::pin_thread_to_core(config.core) {
            Ok(c) => {
                thread_pinned = true;
                log.log(format!("pinned to core {c}"));
            }
            Err(e) => {
                thread_pinned = false;
                log.log(format!("warning: could not pin thread: {e}"));
            }
        }
    }

    let memory_locked;
    if config.no_lock {
        memory_locked = false;
        log.log("memory locking: skipped (--no-lock)");
    } else {
        match platform::lock_memory() {
            Ok(()) => {
                memory_locked = true;
                log.log("memory locked (mlockall)");
            }
            Err(e) => {
                memory_locked = false;
                log.log(format!("warning: mlockall failed: {e}"));
            }
        }
    }

    // ------------------------------------------------------------------
    // 1. Timer calibration (§8.1)
    // ------------------------------------------------------------------
    let granularity_ns = timer::measure_granularity();
    let mintime_ns = timer::compute_mintime_ns(granularity_ns);
    log.log(format!(
        "timer granularity: {granularity_ns:.1} ns, MINTIME: {mintime_ns} ns"
    ));

    // ------------------------------------------------------------------
    // 2. CPU frequency detection (§14)
    // ------------------------------------------------------------------
    let cpu_freq_ghz = arch::estimate_cpu_freq_ghz();
    let cpu_freq_os = platform::cpu_freq_os_ghz().ok();
    log.log(format!("CPU frequency: {cpu_freq_ghz:.3} GHz (measured)"));
    if let Some(os_ghz) = cpu_freq_os {
        log.log(format!("CPU frequency: {os_ghz:.3} GHz (OS-reported)"));
        let ratio = (cpu_freq_ghz - os_ghz).abs() / os_ghz;
        if ratio > constants::CPU_FREQ_WARN_THRESHOLD {
            log.log(format!(
                "WARNING: measured vs OS frequency differ by {:.0}% — possible frequency scaling",
                ratio * 100.0
            ));
        }
    }

    // ------------------------------------------------------------------
    // 3–8. Measurement phases (to be implemented)
    // ------------------------------------------------------------------
    // The remaining phases (cache sweep, cache analysis, TLB sweep,
    // TLB analysis, bandwidth sweep, MLP sweep) will be added in
    // subsequent implementation phases.  For now the fields are None.

    let end_ns = platform::clock_ns();
    let duration_ms = (end_ns - start_ns) / 1_000_000;

    // ------------------------------------------------------------------
    // Flush log
    // ------------------------------------------------------------------
    if config.verbose {
        log.flush();
    }

    Ok(ResonanceResults {
        timestamp: chrono_lite_now(),
        platform: PlatformInfo {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        },
        cpu_freq_ghz,
        cpu_freq_os_ghz: cpu_freq_os,
        timer_granularity_ns: granularity_ns,
        memory_locked,
        thread_pinned,
        thread_core: config.core,
        cache: None,
        tlb: None,
        bandwidth: None,
        mlp: None,
        duration_ms,
    })
}

/// Minimal ISO-8601 timestamp without pulling in the `chrono` crate.
fn chrono_lite_now() -> String {
    // SAFETY: time(NULL) is always valid.
    let epoch = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: gmtime_r is thread-safe and writes into our stack-local tm.
    unsafe {
        libc::gmtime_r(&epoch, &mut tm);
    }
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
    )
}
