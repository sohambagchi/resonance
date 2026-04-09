//! Architecture-specific kernel dispatch (DESIGN.md §13.2).
//!
//! Provides top-level dispatch functions that select the best available
//! implementation at compile time (via `cfg`) or runtime (via feature
//! detection).

#[cfg(target_arch = "x86_64")]
pub mod x86_64;

pub mod generic;

// ---------------------------------------------------------------------------
// Bandwidth kernel dispatch
// ---------------------------------------------------------------------------

/// Sequential read — returns elapsed nanoseconds.
pub fn sequential_read(buf: &[u8]) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            // SAFETY: feature detection passed; buffer is valid.
            return unsafe { x86_64::seq_read_avx2(buf) };
        }
    }
    generic::seq_read(buf)
}

/// Sequential write — returns elapsed nanoseconds.
pub fn sequential_write(buf: &mut [u8]) -> u64 {
    generic::seq_write(buf)
}

/// Sequential copy src → dst — returns elapsed nanoseconds.
pub fn sequential_copy(dst: &mut [u8], src: &[u8]) -> u64 {
    generic::seq_copy(dst, src)
}

/// Zero-fill — returns elapsed nanoseconds.
pub fn sequential_fill(buf: &mut [u8]) -> u64 {
    generic::seq_fill(buf)
}

// ---------------------------------------------------------------------------
// CPU frequency estimation dispatch
// ---------------------------------------------------------------------------

/// Estimate CPU frequency in GHz via dependent-addition chain (§14).
///
/// On x86-64 this uses inline assembly to prevent loop elimination.
/// Falls back to a Rust loop with `black_box` on other architectures.
pub fn estimate_cpu_freq_ghz() -> f64 {
    #[cfg(target_arch = "x86_64")]
    return x86_64::estimate_cpu_freq_ghz();

    #[cfg(not(target_arch = "x86_64"))]
    return generic::estimate_cpu_freq_ghz();
}
