/// Sysfs oracle — reads hardware-reported cache/CPU parameters for
/// validation against measurement results.
///
/// This module provides ground-truth values from the OS/hardware that can
/// serve as an oracle in tests: if our measurements detect L1 = 32 KiB with
/// 64-byte lines and 8-way associativity, we can verify that against what
/// the kernel exposes in `/sys/devices/system/cpu/cpu0/cache/`.
///
/// Also useful at runtime for cross-checking (e.g., CPU frequency measured
/// vs. OS-reported).
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Cache oracle
// ---------------------------------------------------------------------------

/// Cache type as reported by sysfs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OracleCacheType {
    Data,
    Instruction,
    Unified,
    Unknown(String),
}

/// A single cache level read from sysfs.
#[derive(Debug, Clone)]
pub struct OracleCache {
    /// Level number (1, 2, 3, …).
    pub level: u32,
    /// Cache type.
    pub cache_type: OracleCacheType,
    /// Total size in bytes.
    pub size_bytes: u64,
    /// Cache line size in bytes.
    pub line_size_bytes: u32,
    /// Set associativity.
    pub associativity: u32,
    /// Number of sets.
    pub sets: u32,
}

/// Read all cache levels for CPU 0 from sysfs.
///
/// Returns an empty `Vec` if sysfs is not available (e.g., in a container
/// without `/sys` mounted).
pub fn read_sysfs_caches() -> Vec<OracleCache> {
    let base = PathBuf::from("/sys/devices/system/cpu/cpu0/cache");
    if !base.exists() {
        return Vec::new();
    }

    let mut caches = Vec::new();
    for i in 0..16 {
        let idx_path = base.join(format!("index{i}"));
        if !idx_path.exists() {
            break;
        }

        let level = read_u32(&idx_path, "level");
        let cache_type = read_cache_type(&idx_path);
        let size_bytes = read_size_field(&idx_path, "size");
        let line_size = read_u32(&idx_path, "coherency_line_size");
        let assoc = read_u32(&idx_path, "ways_of_associativity");
        let sets = read_u32(&idx_path, "number_of_sets");

        // Skip entries where parsing failed (returns 0).
        if level > 0 && size_bytes > 0 {
            caches.push(OracleCache {
                level,
                cache_type,
                size_bytes,
                line_size_bytes: line_size,
                associativity: assoc,
                sets,
            });
        }
    }

    caches
}

/// Return only data and unified caches (exclude instruction caches), sorted
/// by level.
pub fn read_sysfs_data_caches() -> Vec<OracleCache> {
    let mut caches: Vec<OracleCache> = read_sysfs_caches()
        .into_iter()
        .filter(|c| c.cache_type != OracleCacheType::Instruction)
        .collect();
    caches.sort_by_key(|c| c.level);
    caches
}

// ---------------------------------------------------------------------------
// CPU frequency oracle
// ---------------------------------------------------------------------------

/// Read the OS-reported CPU frequency in GHz.
///
/// Tries `scaling_cur_freq` first (current governor frequency), then falls
/// back to `cpuinfo_max_freq`.  Returns `None` if neither is available.
pub fn read_sysfs_cpu_freq_ghz() -> Option<f64> {
    let cur = "/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq";
    let max = "/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq";
    let content = fs::read_to_string(cur)
        .or_else(|_| fs::read_to_string(max))
        .ok()?;
    let khz: f64 = content.trim().parse().ok()?;
    Some(khz / 1_000_000.0)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_u32(dir: &Path, file: &str) -> u32 {
    fs::read_to_string(dir.join(file))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

fn read_cache_type(dir: &Path) -> OracleCacheType {
    match fs::read_to_string(dir.join("type")) {
        Ok(s) => match s.trim() {
            "Data" => OracleCacheType::Data,
            "Instruction" => OracleCacheType::Instruction,
            "Unified" => OracleCacheType::Unified,
            other => OracleCacheType::Unknown(other.to_string()),
        },
        Err(_) => OracleCacheType::Unknown("unreadable".into()),
    }
}

/// Parse sysfs size fields like `"32K"`, `"256K"`, `"8192K"`, `"16M"`.
fn read_size_field(dir: &Path, file: &str) -> u64 {
    let raw = match fs::read_to_string(dir.join(file)) {
        Ok(s) => s.trim().to_string(),
        Err(_) => return 0,
    };

    if let Some(kb) = raw.strip_suffix('K') {
        kb.parse::<u64>().unwrap_or(0) * 1024
    } else if let Some(mb) = raw.strip_suffix('M') {
        mb.parse::<u64>().unwrap_or(0) * 1024 * 1024
    } else {
        // Plain bytes.
        raw.parse::<u64>().unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Unit tests (run on any Linux system with sysfs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sysfs_caches_non_empty() {
        let caches = read_sysfs_caches();
        // On any real Linux machine this should find at least L1d.
        assert!(!caches.is_empty(), "sysfs should expose at least one cache");
    }

    #[test]
    fn sysfs_data_caches_sorted() {
        let caches = read_sysfs_data_caches();
        for pair in caches.windows(2) {
            assert!(
                pair[0].level <= pair[1].level,
                "caches should be sorted by level"
            );
        }
    }

    #[test]
    fn sysfs_cache_sanity() {
        let caches = read_sysfs_data_caches();
        for c in &caches {
            assert!(
                c.size_bytes >= 1024,
                "L{} size {} is unexpectedly small",
                c.level,
                c.size_bytes
            );
            assert!(
                [32, 64, 128].contains(&c.line_size_bytes),
                "L{} line size {} is unusual",
                c.level,
                c.line_size_bytes
            );
            assert!(
                c.associativity >= 1 && c.associativity <= 128,
                "L{} associativity {} is unusual",
                c.level,
                c.associativity
            );
        }
    }

    #[test]
    fn sysfs_cpu_freq_plausible() {
        if let Some(ghz) = read_sysfs_cpu_freq_ghz() {
            assert!(
                ghz > 0.5 && ghz < 8.0,
                "OS CPU freq {ghz} GHz is outside plausible range"
            );
        }
        // Not an error if sysfs doesn't have cpufreq (e.g., VM).
    }
}
