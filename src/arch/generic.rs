/// Generic (portable) fallback kernels (DESIGN.md §13.2).
///
/// These rely on the compiler's auto-vectorisation; they are correct but
/// will not match hand-written ISA assembly for peak bandwidth.
use crate::platform;
use std::hint::black_box;
use std::sync::atomic::{compiler_fence, Ordering};

// ---------------------------------------------------------------------------
// Bandwidth kernels
// ---------------------------------------------------------------------------

/// Sequential read — sum-accumulate 8-byte chunks.
pub fn seq_read(buf: &[u8]) -> u64 {
    let mut acc: u64 = 0;

    compiler_fence(Ordering::SeqCst);
    let t0 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    for chunk in buf.chunks_exact(8) {
        // SAFETY-ish: chunks_exact guarantees 8 bytes; try_into cannot fail.
        acc = acc.wrapping_add(u64::from_ne_bytes(chunk.try_into().unwrap()));
    }

    compiler_fence(Ordering::SeqCst);
    let t1 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    black_box(acc);
    t1 - t0
}

/// Sequential write — store a constant pattern.
pub fn seq_write(buf: &mut [u8]) -> u64 {
    let pattern: u64 = 0xDEAD_BEEF_CAFE_BABEu64;
    let bytes = pattern.to_ne_bytes();

    compiler_fence(Ordering::SeqCst);
    let t0 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    for chunk in buf.chunks_exact_mut(8) {
        chunk.copy_from_slice(&bytes);
    }

    compiler_fence(Ordering::SeqCst);
    let t1 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    t1 - t0
}

/// Sequential copy src → dst.
pub fn seq_copy(dst: &mut [u8], src: &[u8]) -> u64 {
    let n = dst.len().min(src.len());

    compiler_fence(Ordering::SeqCst);
    let t0 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    dst[..n].copy_from_slice(&src[..n]);

    compiler_fence(Ordering::SeqCst);
    let t1 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    t1 - t0
}

/// Zero-fill (memset equivalent).
pub fn seq_fill(buf: &mut [u8]) -> u64 {
    compiler_fence(Ordering::SeqCst);
    let t0 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    // The compiler should lower this to a fast memset / rep stosb.
    buf.fill(0);

    compiler_fence(Ordering::SeqCst);
    let t1 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    t1 - t0
}

// ---------------------------------------------------------------------------
// CPU frequency estimation — generic fallback (§14)
// ---------------------------------------------------------------------------

/// Estimate CPU frequency via dependent additions (no inline asm).
///
/// The `black_box` barrier discourages the compiler from eliminating the
/// loop, though it is less reliable than the assembly version.
pub fn estimate_cpu_freq_ghz() -> f64 {
    let n = crate::constants::CPU_FREQ_ITERATIONS;
    let mut acc: u64 = 1;

    compiler_fence(Ordering::SeqCst);
    let t0 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    for _ in 0..n {
        acc = black_box(acc.wrapping_add(black_box(acc)));
    }

    compiler_fence(Ordering::SeqCst);
    let t1 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    black_box(acc);
    let elapsed_ns = (t1 - t0) as f64;
    if elapsed_ns > 0.0 {
        n as f64 / elapsed_ns
    } else {
        0.0
    }
}
