/// x86-64 architecture-specific kernels (DESIGN.md §13.2, §14).
///
/// Provides hand-written inline assembly for:
/// - CPU frequency estimation via dependent `add` instructions
/// - AVX2 sequential-read bandwidth kernel
use crate::platform;
use std::sync::atomic::{compiler_fence, Ordering};

// ---------------------------------------------------------------------------
// CPU frequency estimation (§14)
// ---------------------------------------------------------------------------

/// Estimate CPU frequency by timing a chain of dependent `add` instructions.
///
/// Each `add rax, rax` has a latency of 1 cycle on all modern x86 cores.
/// `N additions / elapsed_ns` gives frequency in GHz.
pub fn estimate_cpu_freq_ghz() -> f64 {
    let n = crate::constants::CPU_FREQ_ITERATIONS;
    let mut acc: u64 = 1;

    compiler_fence(Ordering::SeqCst);
    let t0 = platform::clock_ns();

    // SAFETY: the assembly loop performs only register arithmetic; no memory
    // is accessed.  `nostack` is valid because no stack frame is needed.
    unsafe {
        core::arch::asm!(
            "2:",
            "add {acc}, {acc}",
            "dec {n}",
            "jnz 2b",
            n = inout(reg) n => _,
            acc = inout(reg) acc,
            options(nostack),
        );
    }

    let t1 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    std::hint::black_box(acc);
    let elapsed_ns = (t1 - t0) as f64;
    if elapsed_ns > 0.0 {
        crate::constants::CPU_FREQ_ITERATIONS as f64 / elapsed_ns
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// AVX2 sequential read (§13.2)
// ---------------------------------------------------------------------------

/// Sequential read using 256-bit `vmovdqa` loads (AVX2).
///
/// Reads the entire buffer in 256-byte strides (8 × 32-byte loads per
/// iteration) and accumulates with `vpor` to prevent dead-code elimination.
///
/// # Safety
///
/// Caller must verify `is_x86_feature_detected!("avx2")` before calling.
/// `buf` must be 32-byte aligned (guaranteed by `AlignedBuffer`).
#[target_feature(enable = "avx2")]
pub unsafe fn seq_read_avx2(buf: &[u8]) -> u64 {
    let len = buf.len();
    // Process in 256-byte chunks (8 × ymm loads).
    let chunks = len / 256;
    if chunks == 0 {
        return crate::arch::generic::seq_read(buf);
    }

    let ptr = buf.as_ptr();
    let end = ptr.add(chunks * 256);

    compiler_fence(Ordering::SeqCst);
    let t0 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    // SAFETY: `buf` is at least `chunks * 256` bytes, 32-byte aligned.
    // The ymm registers are caller-saved on System V ABI.
    core::arch::asm!(
        // Zero the accumulator.
        "vpxor ymm0, ymm0, ymm0",

        "2:",
        "vmovdqa ymm1,  [{ptr}]",
        "vmovdqa ymm2,  [{ptr} + 32]",
        "vmovdqa ymm3,  [{ptr} + 64]",
        "vmovdqa ymm4,  [{ptr} + 96]",
        "vmovdqa ymm5,  [{ptr} + 128]",
        "vmovdqa ymm6,  [{ptr} + 160]",
        "vmovdqa ymm7,  [{ptr} + 192]",
        "vmovdqa ymm8,  [{ptr} + 224]",

        // Accumulate to prevent elimination.
        "vpor ymm0, ymm0, ymm1",
        "vpor ymm0, ymm0, ymm2",
        "vpor ymm0, ymm0, ymm3",
        "vpor ymm0, ymm0, ymm4",
        "vpor ymm0, ymm0, ymm5",
        "vpor ymm0, ymm0, ymm6",
        "vpor ymm0, ymm0, ymm7",
        "vpor ymm0, ymm0, ymm8",

        "add {ptr}, 256",
        "cmp {ptr}, {end}",
        "jne 2b",

        ptr = inout(reg) ptr => _,
        end = in(reg) end,
        out("ymm0") _,
        out("ymm1") _,
        out("ymm2") _,
        out("ymm3") _,
        out("ymm4") _,
        out("ymm5") _,
        out("ymm6") _,
        out("ymm7") _,
        out("ymm8") _,
        options(nostack),
    );

    compiler_fence(Ordering::SeqCst);
    let t1 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    t1 - t0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_freq_is_reasonable() {
        let ghz = estimate_cpu_freq_ghz();
        // Should be between 0.5 GHz and 6 GHz for any modern x86-64 CPU.
        assert!(
            ghz > 0.5 && ghz < 6.0,
            "measured CPU frequency {ghz} GHz is outside reasonable range"
        );
    }

    #[test]
    fn avx2_seq_read_smoke() {
        if !is_x86_feature_detected!("avx2") {
            return; // skip on CPUs without AVX2
        }
        // 8 KiB buffer, 32-byte aligned (page alignment satisfies this).
        let buf = crate::buffer::AlignedBuffer::new(8192, 4096).unwrap();
        let elapsed = unsafe { seq_read_avx2(buf.as_slice()) };
        assert!(elapsed > 0, "elapsed should be positive");
    }
}
