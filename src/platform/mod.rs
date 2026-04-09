//! Platform abstraction layer (DESIGN.md §6).
//!
//! Provides a uniform interface to OS-specific operations required by the
//! measurement infrastructure: thread pinning, memory locking, monotonic
//! timing, and system information queries.
//!
//! Only Linux/x86-64 is implemented for now; macOS and Windows stubs will
//! follow in a later phase.

#[cfg(target_os = "linux")]
mod linux;

use std::fmt;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors originating from platform-specific operations.
#[derive(Debug)]
pub enum PlatformError {
    /// The requested operation is not supported on this platform.
    Unsupported(String),
    /// An OS call failed.  The inner value is `errno` (or equivalent).
    OsError {
        call: &'static str,
        errno: i32,
        detail: String,
    },
    /// A sysfs / procfs read failed.
    SysfsReadError { path: String, detail: String },
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(msg) => write!(f, "unsupported: {msg}"),
            Self::OsError {
                call,
                errno,
                detail,
            } => {
                write!(f, "{call} failed (errno {errno}): {detail}")
            }
            Self::SysfsReadError { path, detail } => {
                write!(f, "sysfs read {path}: {detail}")
            }
        }
    }
}

impl std::error::Error for PlatformError {}

// ---------------------------------------------------------------------------
// Public free functions — delegate to the platform-specific module.
// ---------------------------------------------------------------------------

/// Pin the calling thread to logical core `core`.
/// Returns the core index on success.
pub fn pin_thread_to_core(core: usize) -> Result<usize, PlatformError> {
    #[cfg(target_os = "linux")]
    return linux::pin_thread_to_core(core);

    #[cfg(not(target_os = "linux"))]
    Err(PlatformError::Unsupported(
        "thread pinning not implemented for this platform".into(),
    ))
}

/// Lock all current and future memory into RAM (`mlockall`).
pub fn lock_memory() -> Result<(), PlatformError> {
    #[cfg(target_os = "linux")]
    return linux::lock_memory();

    #[cfg(not(target_os = "linux"))]
    Err(PlatformError::Unsupported(
        "memory locking not implemented for this platform".into(),
    ))
}

/// Monotonic timestamp in nanoseconds.
///
/// On Linux this is `CLOCK_MONOTONIC_RAW` — immune to NTP adjustment.
#[inline(always)]
pub fn clock_ns() -> u64 {
    #[cfg(target_os = "linux")]
    return linux::clock_ns();

    #[cfg(not(target_os = "linux"))]
    {
        // Fallback: Instant-based (less precise but portable).
        use std::time::Instant;
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let start = START.get_or_init(Instant::now);
        start.elapsed().as_nanos() as u64
    }
}

/// System page size in bytes.
pub fn page_size() -> usize {
    #[cfg(target_os = "linux")]
    return linux::page_size();

    #[cfg(not(target_os = "linux"))]
    4096 // sensible default
}

/// Estimate of available physical memory in bytes.
pub fn available_memory_bytes() -> Result<u64, PlatformError> {
    #[cfg(target_os = "linux")]
    return linux::available_memory_bytes();

    #[cfg(not(target_os = "linux"))]
    Err(PlatformError::Unsupported(
        "available_memory_bytes not implemented for this platform".into(),
    ))
}

/// OS-reported maximum CPU frequency in GHz (best-effort).
pub fn cpu_freq_os_ghz() -> Result<f64, PlatformError> {
    #[cfg(target_os = "linux")]
    return linux::cpu_freq_os_ghz();

    #[cfg(not(target_os = "linux"))]
    Err(PlatformError::Unsupported(
        "cpu_freq_os_ghz not implemented for this platform".into(),
    ))
}

/// Hugepage size in bytes (Linux only; returns `None` elsewhere).
pub fn hugepage_size_bytes() -> Option<usize> {
    #[cfg(target_os = "linux")]
    return linux::hugepage_size_bytes();

    #[cfg(not(target_os = "linux"))]
    None
}
