/// LCG random-read latency kernel (DESIGN.md §10.3).
///
/// Uses a linear congruential generator to compute pseudo-random addresses
/// and OR's the loaded byte into the seed, creating a dependency chain that
/// the compiler cannot eliminate.
use crate::platform;
use std::hint::black_box;
use std::sync::atomic::{compiler_fence, Ordering};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Perform `iters` LCG-driven random reads from `buf` and return the total
/// elapsed nanoseconds.
///
/// `buf.len()` **must** be a power of two for the address masking to work.
///
/// # Panics
///
/// Panics if `buf.len()` is not a power of two.
pub fn lcg_random_read(buf: &[u8], iters: u64) -> u64 {
    assert!(
        buf.len().is_power_of_two(),
        "lcg_random_read requires buf.len() to be a power of 2, got {}",
        buf.len()
    );

    compiler_fence(Ordering::SeqCst);
    let t0 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    let result = lcg_inner(buf, iters);

    compiler_fence(Ordering::SeqCst);
    let t1 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    black_box(result);
    t1 - t0
}

// ---------------------------------------------------------------------------
// Inner loop — #[inline(never)] prevents the compiler from proving the
// buffer is zero and eliminating the loads.
// ---------------------------------------------------------------------------

#[inline(never)]
fn lcg_inner(buf: &[u8], iters: u64) -> u32 {
    let mask = (buf.len() - 1) as u32;
    let mut seed: u32 = 0x1234_5678;

    for _ in 0..iters {
        // Two LCG steps per iteration — produces a 16-bit address from
        // two 8-bit halves for better distribution across the buffer.
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let v = (seed >> 8) & 0xFF00;
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let addr = ((seed >> 16) | v) & mask;

        // OR the loaded byte into the seed — forced dependency.
        seed |= buf[addr as usize] as u32;
    }

    seed
}

// ---------------------------------------------------------------------------
// Baseline measurement (§10.3)
// ---------------------------------------------------------------------------

/// Measure the ALU-only overhead of the LCG loop by running against a
/// 1-byte buffer (always index 0, always in L1).
///
/// Subtract this from real measurements to isolate memory latency.
pub fn lcg_baseline_ns(iters: u64) -> u64 {
    let tiny = [0u8; 1];
    // 1 is a power of 2.
    compiler_fence(Ordering::SeqCst);
    let t0 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    let result = lcg_inner(&tiny, iters);

    compiler_fence(Ordering::SeqCst);
    let t1 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    black_box(result);
    t1 - t0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lcg_smoke() {
        let buf = vec![0u8; 4096]; // 4 KiB, power of 2
        let elapsed = lcg_random_read(&buf, 10_000);
        assert!(elapsed > 0);
    }

    #[test]
    fn lcg_baseline_smoke() {
        let elapsed = lcg_baseline_ns(10_000);
        assert!(elapsed > 0);
    }

    #[test]
    #[should_panic(expected = "power of 2")]
    fn lcg_non_power_of_two_panics() {
        let buf = vec![0u8; 1000]; // not a power of 2
        lcg_random_read(&buf, 1);
    }
}
