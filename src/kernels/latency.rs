/// Pointer-chase latency kernels (DESIGN.md §10).
///
/// The chain is a `[usize]` buffer where `buf[i]` stores the index of the
/// next element.  The traversal is fully serialised — each load depends on
/// the result of the previous one — defeating both hardware prefetching and
/// out-of-order execution.
use crate::platform;
use std::hint::black_box;
use std::sync::atomic::{compiler_fence, Ordering};

/// Number of unrolled dereferences per outer-loop iteration.
const UNROLL: u64 = 100;

// ---------------------------------------------------------------------------
// chase! macro — unrolls 10 dependent pointer dereferences.
// ---------------------------------------------------------------------------

/// Ten dependent loads from a pointer-chase chain.
///
/// # Safety
///
/// `$base` must point to an allocation containing at least `$idx + 1`
/// readable `usize` elements, and the value at each accessed position must
/// itself be a valid index into the same allocation.
macro_rules! chase10 {
    ($base:expr, $idx:ident) => {
        $idx = *$base.add($idx);
        $idx = *$base.add($idx);
        $idx = *$base.add($idx);
        $idx = *$base.add($idx);
        $idx = *$base.add($idx);
        $idx = *$base.add($idx);
        $idx = *$base.add($idx);
        $idx = *$base.add($idx);
        $idx = *$base.add($idx);
        $idx = *$base.add($idx);
    };
}

// ---------------------------------------------------------------------------
// Throughput pass (§10.1)
// ---------------------------------------------------------------------------

/// Traverse the pointer-chase chain for `iters` outer iterations, each
/// performing [`UNROLL`] (100) dependent dereferences.
///
/// Returns the total elapsed nanoseconds.
///
/// The result can be divided by `iters` to obtain ns per 100 dereferences,
/// then by [`UNROLL`] to obtain ns per single memory access.
pub fn pointer_chase(chain: &[usize], iters: u64) -> u64 {
    let base = chain.as_ptr();
    let mut idx: usize = 0;

    compiler_fence(Ordering::SeqCst);
    let t0 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    for _ in 0..iters {
        // SAFETY: `chain` was constructed by `build_chain` which guarantees
        // every stored index is in-bounds and the chain forms a single cycle.
        unsafe {
            chase10!(base, idx); // 10
            chase10!(base, idx); // 20
            chase10!(base, idx); // 30
            chase10!(base, idx); // 40
            chase10!(base, idx); // 50
            chase10!(base, idx); // 60
            chase10!(base, idx); // 70
            chase10!(base, idx); // 80
            chase10!(base, idx); // 90
            chase10!(base, idx); // 100
        }
    }

    compiler_fence(Ordering::SeqCst);
    let t1 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    black_box(idx);
    t1 - t0
}

/// Per-access unroll factor.  Callers divide the result of `pointer_chase`
/// by `iters * unroll_factor()` to get ns per single dereference.
pub const fn unroll_factor() -> u64 {
    UNROLL
}

// ---------------------------------------------------------------------------
// Latency pass (§10.2)
// ---------------------------------------------------------------------------

/// Pointer-chase with 100 dependent arithmetic operations inserted between
/// each dereference.  Runs `iters / REDUCE` outer iterations.
///
/// Used for the two-pass latency isolation technique: the arithmetic fills
/// the pipeline so that the cache miss is fully exposed.
pub fn pointer_chase_with_delay(chain: &[usize], iters: u64) -> u64 {
    let base = chain.as_ptr();
    let mut idx: usize = 0;
    let mut delay: usize = 0;
    let reduced = iters / crate::constants::REDUCE;

    compiler_fence(Ordering::SeqCst);
    let t0 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    for _ in 0..reduced {
        // SAFETY: same invariant as pointer_chase.
        unsafe {
            idx = *base.add(idx);
        }
        // 100 dependent arithmetic operations — depend on the load result
        // so the compiler cannot hoist them.
        for _ in 0..100 {
            delay = delay.wrapping_add(idx);
        }
    }

    compiler_fence(Ordering::SeqCst);
    let t1 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    black_box((idx, delay));
    t1 - t0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{build_chain, AlignedBuffer};
    use crate::rng::Xoshiro256StarStar;

    /// Smoke test: pointer_chase on a tiny chain should complete and return
    /// a positive elapsed time.
    #[test]
    fn pointer_chase_smoke() {
        let range = 8192; // 8 KiB
        let ws = std::mem::size_of::<usize>();
        let _n = range / ws;

        let mut buf = AlignedBuffer::new(range, 4096).unwrap();
        {
            let chain = buf.as_usize_mut_slice();
            let mut rng = Xoshiro256StarStar::new(42);
            build_chain(chain, range, 64, 4096, &mut rng);
        }
        let chain = buf.as_usize_slice();
        let elapsed = pointer_chase(chain, 10);
        assert!(elapsed > 0, "elapsed should be positive, got {elapsed}");
    }

    /// pointer_chase_with_delay should also complete.
    #[test]
    fn latency_pass_smoke() {
        let range = 8192;
        let mut buf = AlignedBuffer::new(range, 4096).unwrap();
        {
            let chain = buf.as_usize_mut_slice();
            let mut rng = Xoshiro256StarStar::new(42);
            build_chain(chain, range, 64, 4096, &mut rng);
        }
        let chain = buf.as_usize_slice();
        // Need enough iters for REDUCE division.
        let elapsed = pointer_chase_with_delay(chain, 1000);
        assert!(elapsed > 0);
    }
}
