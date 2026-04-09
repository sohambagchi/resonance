/// Result data model (DESIGN.md §11.4, §12.4, §13.5, §15.3, §17.2).
///
/// All structs derive `Serialize`/`Deserialize` for JSON output and `Debug`
/// for diagnostic printing.
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Cache hierarchy (§11.4)
// ---------------------------------------------------------------------------

/// A single cache level detected by measurement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheLevel {
    /// Level number (1 = L1, 2 = L2, …).
    pub level: u32,
    /// Total capacity in bytes.
    pub size_bytes: u64,
    /// Cache line size in bytes.
    pub line_size_bytes: u32,
    /// Set associativity (0 = unknown / not converged).
    pub associativity: u32,
    /// Miss latency in nanoseconds.
    pub miss_latency_ns: f64,
    /// Miss latency in CPU cycles (0.0 if frequency unknown).
    pub miss_latency_cycles: f64,
    /// Replacement (throughput-pass) time in nanoseconds.
    pub replacement_time_ns: f64,
}

/// Method used to detect cache boundaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DetectionMethod {
    XRay,
    Calibrator,
    Hybrid,
}

/// Confidence level for a measurement result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

/// Complete cache hierarchy characterization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheInfo {
    pub levels: Vec<CacheLevel>,
    pub detection_method: DetectionMethod,
    pub memory_locked: bool,
    pub thread_pinned: bool,
    pub confidence: Confidence,
}

// ---------------------------------------------------------------------------
// TLB (§12.4)
// ---------------------------------------------------------------------------

/// A single TLB level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlbLevel {
    pub level: u32,
    pub entries: u32,
    pub page_size_bytes: u64,
    pub miss_latency_ns: f64,
    pub miss_latency_cycles: f64,
}

/// Complete TLB characterization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlbInfo {
    pub levels: Vec<TlbLevel>,
}

// ---------------------------------------------------------------------------
// Bandwidth (§13.5)
// ---------------------------------------------------------------------------

/// A single bandwidth measurement at a given buffer size and access pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandwidthPoint {
    pub buffer_size_bytes: u64,
    pub bandwidth_gbs: f64,
    pub variant: String,
}

/// Complete bandwidth characterization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandwidthResults {
    pub points: Vec<BandwidthPoint>,
    pub peak_read_gbs: f64,
    pub peak_write_gbs: f64,
    pub peak_copy_gbs: f64,
    pub peak_nt_write_gbs: f64,
    pub peak_nt_copy_gbs: f64,
}

// ---------------------------------------------------------------------------
// Memory-level parallelism (§15.3)
// ---------------------------------------------------------------------------

/// A single MLP data point (k chains → ns/access).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlpPoint {
    pub chains: u32,
    pub ns_per_access: f64,
    pub relative_throughput: f64,
}

/// Complete MLP characterization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlpResults {
    pub measurements: Vec<MlpPoint>,
    pub estimated_mlp: u32,
}

// ---------------------------------------------------------------------------
// Top-level report (§17.2)
// ---------------------------------------------------------------------------

/// Platform metadata included in the JSON report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformInfo {
    pub os: String,
    pub arch: String,
}

/// Top-level result structure — serialized as JSON with `--json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResonanceResults {
    pub timestamp: String,
    pub platform: PlatformInfo,
    pub cpu_freq_ghz: f64,
    pub cpu_freq_os_ghz: Option<f64>,
    pub timer_granularity_ns: f64,
    pub memory_locked: bool,
    pub thread_pinned: bool,
    pub thread_core: usize,
    pub cache: Option<CacheInfo>,
    pub tlb: Option<TlbInfo>,
    pub bandwidth: Option<BandwidthResults>,
    pub mlp: Option<MlpResults>,
    pub duration_ms: u64,
}
