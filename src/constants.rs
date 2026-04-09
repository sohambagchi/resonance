//! Key algorithmic parameters (DESIGN.md §19).
//!
//! Every constant is documented with its rationale from the design document.

// ---------------------------------------------------------------------------
// Latency sweep
// ---------------------------------------------------------------------------

/// Smallest working-set range tested (bytes).
/// Below this, L1 measurements are dominated by setup overhead.
pub const MINRANGE: usize = 1024;

/// Number of sub-power-of-2 sample points per octave (0.75x, 1.0x, 1.25x).
/// Gives finer boundary resolution than pure power-of-2 sweeps.
pub const RANGE_STEPS_PER_OCTAVE: usize = 3;

/// Unrolled pointer-chase dereferences per outer-loop iteration.
/// 100 is enough to amortise loop overhead relative to L1 latency (~4 cy).
pub const NUMLOADS: usize = 100;

/// Best-of-N trial count for latency measurements (odd, consistent with lmbench).
pub const NUMTRIES: usize = 11;

/// Latency-pass iteration divisor (Calibrator convention).
/// The latency pass runs `iters / REDUCE` outer iterations, each inserting
/// 100 dependent arithmetic operations between dereferences.
pub const REDUCE: u64 = 10;

// ---------------------------------------------------------------------------
// Plateau / jump detection
// ---------------------------------------------------------------------------

/// Consecutive readings to confirm a plateau.
pub const LENPLATEAU: usize = 3;

/// Plateau-continuity threshold across strides (10 % relative).
pub const EPSILON1: f64 = 0.10;

/// Jump-detection threshold (1.0 cycle absolute).
pub const EPSILON4: f64 = 1.0;

/// Cache-boundary detection ratio (lmbench-style latency ratio).
pub const CACHE_THRESHOLD: f64 = 1.50;

/// TLB-boundary detection ratio.
pub const TLB_THRESHOLD: f64 = 1.15;

/// Merge adjacent level candidates whose log10(latency) values differ by
/// less than this (≈ 2x in linear latency).
pub const LEVEL_MERGE_LOG: f64 = 0.30;

// ---------------------------------------------------------------------------
// Bandwidth
// ---------------------------------------------------------------------------

/// Minimum bandwidth trial duration (500 ms).
pub const MINBW_TIME_NS: u64 = 500_000_000;

/// Maximum bandwidth trial repetitions.
pub const BW_MAXREPEATS: usize = 10;

/// Early-termination threshold for bandwidth (std / peak < 0.1 %).
pub const BW_CONVERGENCE: f64 = 0.001;

// ---------------------------------------------------------------------------
// Memory-level parallelism
// ---------------------------------------------------------------------------

/// Maximum independent pointer-chase chains for MLP measurement.
pub const MAX_MLP_CHAINS: usize = 16;

// ---------------------------------------------------------------------------
// Buffer management
// ---------------------------------------------------------------------------

/// Over-allocation for anti-aliased buffer layout (1 MiB).
pub const ALIGN_PADDING: usize = 1_048_576;

/// Threshold for detecting a conflicting physical page during page-coloring
/// mitigation (30 % latency increase).
pub const PAGE_COLORING_RATIO: f64 = 1.30;

// ---------------------------------------------------------------------------
// Timer calibration
// ---------------------------------------------------------------------------

/// Number of samples for timer-granularity measurement.
pub const GRANULARITY_SAMPLES: usize = 500;

/// Absolute minimum measurement time (1 ms).
pub const MIN_MINTIME_NS: u64 = 1_000_000;

/// Multiplier: MINTIME = max(MIN_MINTIME_NS, granularity × this).
pub const GRANULARITY_MULTIPLIER: u64 = 100;

// ---------------------------------------------------------------------------
// CPU frequency estimation
// ---------------------------------------------------------------------------

/// Number of dependent additions for frequency estimation.
pub const CPU_FREQ_ITERATIONS: u64 = 50_000_000;

/// Acceptable divergence between measured and OS-reported frequency before
/// emitting a warning (20 %).
pub const CPU_FREQ_WARN_THRESHOLD: f64 = 0.20;

// ---------------------------------------------------------------------------
// Mmap threshold
// ---------------------------------------------------------------------------

/// Allocations >= this size use `mmap` instead of `posix_memalign` so we can
/// request transparent huge pages via `madvise(MADV_HUGEPAGE)`.
pub const MMAP_THRESHOLD: usize = 2 * 1024 * 1024;
