/// Linux platform implementation (DESIGN.md §6).
///
/// All functions use `libc` directly — no extra crates required.
use super::PlatformError;
use std::fs;

// ---------------------------------------------------------------------------
// Thread pinning — sched_setaffinity (§6.3)
// ---------------------------------------------------------------------------

pub fn pin_thread_to_core(core: usize) -> Result<usize, PlatformError> {
    // SAFETY: cpu_set_t is a plain-old-data bitmask; zeroing is correct
    // initialisation.  sched_setaffinity is a well-defined POSIX call.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(core, &mut set);

        let ret = libc::sched_setaffinity(
            0, // 0 = calling thread
            std::mem::size_of::<libc::cpu_set_t>(),
            &set,
        );

        if ret == 0 {
            Ok(core)
        } else {
            let errno = *libc::__errno_location();
            Err(PlatformError::OsError {
                call: "sched_setaffinity",
                errno,
                detail: format!("failed to pin to core {core}"),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Memory locking — mlockall (§6.4)
// ---------------------------------------------------------------------------

pub fn lock_memory() -> Result<(), PlatformError> {
    // SAFETY: mlockall with MCL_CURRENT | MCL_FUTURE is a well-defined POSIX
    // call.  It may fail if RLIMIT_MEMLOCK is too low.
    unsafe {
        let ret = libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE);
        if ret == 0 {
            Ok(())
        } else {
            let errno = *libc::__errno_location();
            Err(PlatformError::OsError {
                call: "mlockall",
                errno,
                detail: "failed to lock memory (check RLIMIT_MEMLOCK)".into(),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Timer — CLOCK_MONOTONIC_RAW (§6.2)
// ---------------------------------------------------------------------------

#[inline(always)]
pub fn clock_ns() -> u64 {
    // SAFETY: timespec is zeroed before use.  clock_gettime with
    // CLOCK_MONOTONIC_RAW is always available on Linux ≥ 2.6.28.
    unsafe {
        let mut ts: libc::timespec = std::mem::zeroed();
        libc::clock_gettime(libc::CLOCK_MONOTONIC_RAW, &mut ts);
        ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
    }
}

// ---------------------------------------------------------------------------
// Page size — sysconf (§6.5)
// ---------------------------------------------------------------------------

pub fn page_size() -> usize {
    // SAFETY: sysconf(_SC_PAGESIZE) is always valid on Linux.
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

// ---------------------------------------------------------------------------
// Available memory — /proc/meminfo (§6.5)
// ---------------------------------------------------------------------------

pub fn available_memory_bytes() -> Result<u64, PlatformError> {
    let contents =
        fs::read_to_string("/proc/meminfo").map_err(|e| PlatformError::SysfsReadError {
            path: "/proc/meminfo".into(),
            detail: e.to_string(),
        })?;

    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            return parse_meminfo_kb(rest).map(|kb| kb * 1024);
        }
    }
    // Fallback: MemFree + Buffers + Cached
    let mut free: Option<u64> = None;
    let mut buffers: Option<u64> = None;
    let mut cached: Option<u64> = None;
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("MemFree:") {
            free = parse_meminfo_kb(rest).ok();
        } else if let Some(rest) = line.strip_prefix("Buffers:") {
            buffers = parse_meminfo_kb(rest).ok();
        } else if let Some(rest) = line.strip_prefix("Cached:") {
            cached = parse_meminfo_kb(rest).ok();
        }
    }
    match (free, buffers, cached) {
        (Some(f), Some(b), Some(c)) => Ok((f + b + c) * 1024),
        _ => Err(PlatformError::SysfsReadError {
            path: "/proc/meminfo".into(),
            detail: "could not determine available memory".into(),
        }),
    }
}

/// Parse a `/proc/meminfo` value line like `"  12345 kB"` → `12345u64`.
fn parse_meminfo_kb(s: &str) -> Result<u64, PlatformError> {
    let trimmed = s.trim().trim_end_matches("kB").trim();
    trimmed
        .parse::<u64>()
        .map_err(|_| PlatformError::SysfsReadError {
            path: "/proc/meminfo".into(),
            detail: format!("could not parse '{trimmed}' as u64"),
        })
}

// ---------------------------------------------------------------------------
// CPU frequency — sysfs (§6.5)
// ---------------------------------------------------------------------------

pub fn cpu_freq_os_ghz() -> Result<f64, PlatformError> {
    let path = "/sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq";
    let contents = fs::read_to_string(path).map_err(|e| PlatformError::SysfsReadError {
        path: path.into(),
        detail: e.to_string(),
    })?;
    let khz: f64 = contents
        .trim()
        .parse()
        .map_err(|_| PlatformError::SysfsReadError {
            path: path.into(),
            detail: format!("could not parse '{}'", contents.trim()),
        })?;
    Ok(khz / 1_000_000.0) // kHz → GHz
}

// ---------------------------------------------------------------------------
// Hugepage size — /proc/meminfo (§6.5)
// ---------------------------------------------------------------------------

pub fn hugepage_size_bytes() -> Option<usize> {
    let contents = fs::read_to_string("/proc/meminfo").ok()?;
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("Hugepagesize:") {
            let kb = parse_meminfo_kb(rest).ok()?;
            return Some(kb as usize * 1024);
        }
    }
    None
}
