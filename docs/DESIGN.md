# Resonance — Local Implementation Design Document

## 1. Overview

Resonance is a **memory hierarchy characterization tool** that automatically measures the hardware parameters of a computer's memory subsystem. Given a target machine, Resonance produces a complete, structured characterization of its cache hierarchy, TLB, memory bandwidth, and memory-level parallelism — without requiring any prior knowledge of the hardware.

This document describes the design of Resonance as a **native, locally-executed binary**. It supersedes the browser-based design (`WEB_DESIGN.md`), which was found to be infeasible due to fundamental browser sandbox constraints: no CPU affinity control, no `mlockall()`, no ISA-level assembly, and degraded timer resolution under Spectre mitigations. A native binary eliminates all of these limitations.

---

## 2. Language Selection: Rust

Resonance is implemented in **Rust**. The choice is argued below against the two most serious alternatives, C and C++.

### Why Not C

The reference tools (Calibrator, lmbench, tinymembench, X-Ray) are all written in C. C is the natural language for this domain and all prior art is directly readable. However, C has concrete liabilities for a new tool:

- **No safe abstractions for unsafe operations.** The measurement kernels require careful manual memory layout (aligned allocation, anti-aliased offsets, pre-touch loops, pointer-chase construction). In C, a single off-by-one or wrong cast is silent undefined behaviour that corrupts results without any compiler feedback.
- **No standard build system with dependency management.** The reference tools range from single-file to GNU make to CMake. None of them have a reproducible, cross-platform dependency story. Resonance needs platform-specific code paths (Linux/macOS/Windows), and managing that in C requires a substantial autoconf/CMake layer.
- **No standard testing framework.** Unit-testing timing infrastructure in C is painful. The analysis pipeline (plateau detection, binary search, level merging) has enough logic that it warrants proper unit tests.
- **32-bit float pitfalls.** The magsilva fork documented that `float` (the C default for `dbl`) caused order-of-magnitude errors. Rust's type system makes the `f64` vs `f32` distinction explicit and compiler-enforced everywhere.

### Why Not C++

C++ provides RAII, type safety improvements, and a richer standard library. However:

- **Build system and package management are still unsolved.** CMake is the dominant C++ build system and is famously complex. There is no standard equivalent of `cargo`.
- **The language itself is large and poorly defined at the edges.** The parts of C++ that matter here (inline assembly, `volatile`, memory barriers) are not meaningfully better than C, and the rest of the language adds surface area without benefit.
- **RAII doesn't prevent use-after-free or data races.** For a tool that pins threads, manages raw buffers, and uses platform-specific scheduling APIs, the lack of a borrow checker means that safety properties must be maintained entirely by convention.

### Why Rust

Rust gives the control of C without its pitfalls, and adds features that are directly valuable for Resonance:

| Property | How It Helps Resonance |
|---------|----------------------|
| Zero-cost abstractions | Measurement kernels compile to the same machine code as hand-written C; no GC, no runtime, no overhead |
| `unsafe` is explicit and localized | The handful of genuinely unsafe operations (inline assembly, raw pointer arithmetic for buffer layout, `mlockall`) are cordoned off; the analysis pipeline is fully safe |
| `f64` is the default floating-point type | Prevents the Calibrator float-precision bug by default |
| Inline assembly (`core::arch::asm!`) | Full access to ISA-specific bandwidth kernels (AVX2, AVX-512, NEON, SVE) with Rust's register allocation |
| `std::sync`, `std::thread`, atomics | The multi-chain MLP kernel and the benchmp-style process framework are expressible without reaching for pthreads directly |
| `cargo` + `cargo test` | A single reproducible build and test story across Linux, macOS, and Windows |
| Conditional compilation (`#[cfg(target_arch)]`, `#[cfg(target_os)]`) | Platform-specific code paths (Linux `sched_setaffinity`, macOS `thread_policy_set`, Windows `SetThreadAffinityMask`) are cleanly scoped |
| `libc` crate | Direct access to all POSIX APIs (mlock, madvise, sysconf, clock_gettime) without a C shim |

The primary cost of Rust — compile times — is not a concern for a tool of this size. The secondary cost — inline assembly syntax is different from GCC/clang inline asm — is manageable and well-documented.

### Unsafe Surface Area

The following operations require `unsafe` and are the complete list of unsafe code in Resonance. Everything else is safe Rust:

- Raw buffer allocation with `alloc::alloc::alloc` (for alignment control beyond what `Vec` provides)
- Anti-aliased offset pointer arithmetic
- `mlockall(MCL_CURRENT | MCL_FUTURE)` via the `libc` crate
- `sched_setaffinity` / platform equivalents
- Inline assembly bandwidth kernels
- `core::hint::black_box` in the stable-Rust form (used as a compiler barrier; technically safe but semantically related to the unsafe surface)

---

## 3. Goals and Non-Goals

### Goals

- Automatically detect the number of cache levels and their sizes, line sizes, associativity, and miss latencies
- Automatically detect TLB levels, entry counts, page sizes, and miss latencies
- Measure memory bandwidth across multiple access patterns (sequential read/write/copy/fill, non-temporal stores)
- Measure memory-level parallelism (MLP) with 1–16 independent chains
- Detect CPU frequency automatically
- Pin the measurement thread to a single core for the duration of the benchmark
- Lock measurement buffers into RAM (`mlockall`) to prevent page faults during timing
- Emit results as both human-readable terminal output and machine-readable JSON
- Support Linux, macOS, and Windows on x86-64 and AArch64

### Non-Goals

- Real-time or continuous monitoring
- Network-remote measurement
- Graphical output (gnuplot scripts, charts) — a separate visualization layer can consume the JSON output
- Instruction cache characterization
- Cache replacement policy identification
- Non-uniform memory access (NUMA) topology mapping (measurements reflect the local socket only)

---

## 4. Background and Prior Art

Resonance synthesizes lessons from five existing tools and one academic paper:

| Reference | Key Contribution |
|-----------|----------------|
| Calibrator (Manegold) | Foundational 2D (range × stride) sweep; plateau/jump state machine for boundary detection |
| Calibrator (fukien fork) | `sysconf(_SC_PAGESIZE)` portability fix; CMake build |
| Calibrator (magsilva fork) | Removing file I/O from the hot path restored accuracy by an order of magnitude; `double` precision fix; auto-detection of CPU frequency and free memory via sysfs/`/proc/meminfo` |
| lmbench | Multi-layer prefetch defeat (Fisher-Yates + bit-reversal + word rotation); benchmp fork/pipe framework; adaptive iteration calibration; page coloring mitigation via over-allocation and swap |
| tinymembench | LCG-based latency measurement; anti-aliased buffer allocation; std-dev-based early termination; hand-written ISA assembly bandwidth kernels |
| X-Ray (Yotov et al., 2005) | Formally correct cache parameter detection via Theorem 1 (compact/non-compact sequences); `switch(volatile)` anti-optimization; CPU frequency from dependent integer additions; register count measurement |

The twelve design lessons drawn from this prior art are:

1. **Never perform I/O during measurement.** The magsilva fork showed that gnuplot file writes corrupted timing results by an order of magnitude.
2. **Use multi-layer prefetch defeat.** lmbench's Fisher-Yates (inter-page) + bit-reversal (intra-page) + word rotation is the strongest published heuristic approach.
3. **Formal detection outperforms heuristics.** X-Ray's Theorem 1 binary search is more accurate than threshold-based plateau/jump state machines and has been validated on 11 platforms.
4. **Use `double` throughout.** The magsilva fork found that `float` (32-bit) causes false plateau/jump detections.
5. **Take the minimum across multiple trials.** Noise only inflates latency or deflates bandwidth; it never has the opposite effect.
6. **Adaptive iteration calibration.** Never hardcode iteration counts; calibrate to ≥ 100× the clock granularity.
7. **Anti-aliased buffer allocation.** Complementary bit-pattern offsets prevent cache-set conflicts between source, destination, and scratch buffers.
8. **Lock memory before measurement.** `mlockall(MCL_CURRENT | MCL_FUTURE)` prevents page faults from corrupting timing during measurement.
9. **Pin the thread to one core.** `sched_setaffinity` (Linux) or equivalent prevents OS migration from inflating latency unpredictably.
10. **Use `CLOCK_MONOTONIC` (or `CLOCK_MONOTONIC_RAW` on Linux).** `gettimeofday` is adjustable by NTP; all reference tools use it only because it was universal. A new tool should use the monotonic clock.
11. **Page coloring for large working sets.** For ranges approaching or exceeding L2/L3 size, detect and swap out conflicting physical pages (lmbench's `test_chunk`/`fixup_chunk` approach).
12. **Assembly kernels for bandwidth.** Compiler-generated code varies by optimization level and version; only hand-written ISA-specific kernels guarantee known instruction sequences and access widths.

---

## 5. Architecture

### 5.1 Component Overview

```
┌─────────────────────────────────────────────────────┐
│                    resonance binary                  │
│                                                      │
│  ┌─────────────┐                                     │
│  │     CLI     │  (clap: --json, --max-mem, --trials)│
│  └──────┬──────┘                                     │
│         │                                            │
│  ┌──────▼──────────────────────────────────────┐    │
│  │              Orchestrator                    │    │
│  │  schedules experiments, drives analysis,     │    │
│  │  collects results                            │    │
│  └──────┬──────────────────────────────────────┘    │
│         │                                            │
│   ┌─────┴──────────────────────────────────┐        │
│   │                                        │        │
│  ┌▼────────────────┐   ┌───────────────────▼──┐     │
│  │  Platform Layer  │   │   Analysis Pipeline   │     │
│  │                  │   │                       │     │
│  │ • cpu_pin()      │   │ • analyzeCache()      │     │
│  │ • mlockall()     │   │ • analyzeTLB()        │     │
│  │ • clock_now()    │   │ • analyzeBandwidth()  │     │
│  │ • page_size()    │   │ • analyzeMLP()        │     │
│  │ • free_memory()  │   └───────────────────────┘     │
│  └──────────────────┘                                 │
│                                                        │
│  ┌────────────────────────────────────────────────┐   │
│  │              Measurement Kernels               │   │
│  │                                                │   │
│  │  latency::pointer_chase()                      │   │
│  │  latency::lcg_random_read()                    │   │
│  │  bandwidth::sequential_{read,write,copy,fill}()│   │
│  │  bandwidth::nontemporal_{write,copy}()         │   │
│  │  mlp::multi_chain()                            │   │
│  └────────────────────────────────────────────────┘   │
│                                                        │
│  ┌────────────────────────────────────────────────┐   │
│  │              Buffer Manager                    │   │
│  │                                                │   │
│  │  AlignedBuffer (mmap/posix_memalign)           │   │
│  │  anti_alias_offsets()                          │   │
│  │  pre_touch()                                   │   │
│  │  build_chain()                                 │   │
│  └────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────  ┘
```

### 5.2 Module Structure

```
resonance/
├── src/
│   ├── main.rs               # CLI entry point, result output
│   ├── orchestrator.rs       # Experiment sequencing and result collection
│   ├── platform/
│   │   ├── mod.rs            # Platform trait definitions
│   │   ├── linux.rs          # sched_setaffinity, mlockall, CLOCK_MONOTONIC_RAW, /proc/meminfo
│   │   ├── macos.rs          # thread_policy_set, mlock, mach_absolute_time
│   │   └── windows.rs        # SetThreadAffinityMask, VirtualLock, QueryPerformanceCounter
│   ├── buffer.rs             # AlignedBuffer, anti-aliased allocation, pre-touch, chain construction
│   ├── timer.rs              # Clock abstraction, granularity calibration, adaptive calibration loop
│   ├── kernels/
│   │   ├── mod.rs
│   │   ├── latency.rs        # Pointer-chase (safe Rust) + dispatch to arch-specific asm
│   │   ├── lcg.rs            # LCG random-read kernel
│   │   ├── bandwidth.rs      # Sequential/strided kernels + dispatch to arch asm
│   │   └── mlp.rs            # Multi-chain kernel
│   ├── arch/
│   │   ├── mod.rs
│   │   ├── x86_64.rs         # AVX2 / AVX-512 bandwidth kernels (inline asm)
│   │   ├── aarch64.rs        # NEON / SVE bandwidth kernels (inline asm)
│   │   └── generic.rs        # Portable fallback (compiler auto-vectorization)
│   ├── analysis/
│   │   ├── mod.rs
│   │   ├── cache.rs          # Hybrid X-Ray + plateau/jump detection
│   │   ├── tlb.rs            # TLB boundary detection and refinement
│   │   ├── bandwidth.rs      # Peak selection and convergence
│   │   └── mlp.rs            # Saturation point detection
│   └── results.rs            # Data model structs and JSON serialization (serde)
├── tests/
│   ├── analysis_cache.rs     # Unit tests for cache detection algorithms
│   ├── analysis_tlb.rs       # Unit tests for TLB detection
│   └── buffer.rs             # Unit tests for chain construction and anti-aliasing
└── Cargo.toml
```

### 5.3 Crate Dependencies

```toml
[dependencies]
libc       = "0.2"    # POSIX APIs: mlockall, sched_setaffinity, madvise, sysconf
clap       = "4"      # CLI argument parsing
serde      = { version = "1", features = ["derive"] }
serde_json = "1"      # JSON output

[target.'cfg(target_os = "linux")'.dependencies]
# No extra crates; linux-specific code uses libc directly

[dev-dependencies]
approx = "0.5"        # Floating-point approximate equality in tests
```

No async runtime, no logging framework, no GUI crate. The binary is intentionally minimal.

---

## 6. Platform Layer

### 6.1 Trait Definition

```rust
pub trait Platform {
    /// Pin the calling thread to a single logical CPU core.
    /// Returns the core index that was selected, or an error.
    fn pin_thread_to_core(core: usize) -> Result<usize, PlatformError>;

    /// Lock all current and future memory mappings into RAM.
    fn lock_memory() -> Result<(), PlatformError>;

    /// Return a monotonic timestamp in nanoseconds.
    /// Must not be adjustable by NTP or wall-clock changes.
    fn clock_ns() -> u64;

    /// Return the system page size in bytes.
    fn page_size() -> usize;

    /// Return an estimate of available physical memory in bytes.
    fn available_memory_bytes() -> u64;
}
```

### 6.2 Timer Selection

| Platform | Clock Source | Notes |
|---------|-------------|-------|
| Linux | `clock_gettime(CLOCK_MONOTONIC_RAW)` | Not adjusted by NTP or `adjtimex`; ~1 ns resolution on modern kernels |
| macOS | `mach_absolute_time()` converted via `mach_timebase_info` | Hardware tick counter; sub-nanosecond resolution |
| Windows | `QueryPerformanceCounter` / `QueryPerformanceFrequency` | HPET or TSC-based; typically 100 ns resolution |

All platforms: timer granularity is measured empirically before any experiment (see §8.1), and `MINTIME` is set to ≥ 100× measured granularity.

### 6.3 Thread Pinning

| Platform | API |
|---------|-----|
| Linux | `sched_setaffinity(0, sizeof(cpu_set_t), &mask)` via `libc` |
| macOS | `thread_policy_set(mach_thread_self(), THREAD_AFFINITY_POLICY, ...)` |
| Windows | `SetThreadAffinityMask(GetCurrentThread(), mask)` |

The Orchestrator selects a core index (default: core 0, configurable via `--core`) and pins the main thread before any experiment begins. The pin is held for the entire binary execution.

### 6.4 Memory Locking

```rust
// Linux / macOS
unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) };

// Windows: VirtualLock per allocation (mlockall equivalent does not exist)
```

If `mlockall` fails (e.g., insufficient `RLIMIT_MEMLOCK`), Resonance warns and continues. Results in this case are annotated with `memory_locked: false`.

### 6.5 System Information Auto-Detection

| Information | Linux | macOS | Windows |
|------------|-------|-------|---------|
| CPU frequency (initial estimate) | `/sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq` | `sysctl hw.cpufrequency_max` | `NtQuerySystemInformation` |
| Available memory | `/proc/meminfo` (`MemAvailable`) | `sysctl hw.memsize` / `vm_stat` | `GlobalMemoryStatusEx` |
| Page size | `sysconf(_SC_PAGESIZE)` | `sysconf(_SC_PAGESIZE)` | `GetSystemInfo` |
| Hugepage size | `/proc/meminfo` (`Hugepagesize`) | Not available | Not available |

CPU frequency from the OS is used only as an initial estimate for converting ns to cycles. The measurement-based frequency detection (§14) is always performed and takes precedence.

---

## 7. Buffer Management

### 7.1 Aligned Allocation

`AlignedBuffer` wraps a page-aligned, pre-touched allocation:

```rust
pub struct AlignedBuffer {
    ptr: *mut u8,
    len: usize,
    layout: Layout,
}

impl AlignedBuffer {
    /// Allocate `len` bytes aligned to `align` bytes (must be a power of 2
    /// and at least the system page size).
    pub fn new(len: usize, align: usize) -> Result<Self, AllocError> {
        // Uses posix_memalign on Unix, _aligned_malloc on Windows.
        // On Linux, mmap(MAP_ANONYMOUS | MAP_PRIVATE) is preferred for
        // large allocations to enable madvise(MADV_HUGEPAGE).
        ...
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] { ... }
    pub fn as_ptr(&self) -> *mut u8 { self.ptr }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) { unsafe { libc::free(self.ptr as *mut libc::c_void) } }
}
```

### 7.2 Anti-Aliased Buffer Layout

For bandwidth measurements that require separate source and destination buffers, a single large allocation is split into views at complementary bit-pattern offsets, following tinymembench:

```rust
const ANTI_ALIAS_OFFSETS: [usize; 4] = [
    0xAAAA_AAAA & PAGE_MASK,
    0x5555_5555 & PAGE_MASK,
    0xCCCC_CCCC & PAGE_MASK,
    0x3333_3333 & PAGE_MASK,
];

/// Returns four non-overlapping sub-slices from `backing` at
/// maximally cache-set-diverse offsets.
pub fn anti_aliased_views(backing: &mut AlignedBuffer, slice_len: usize)
    -> [&mut [u8]; 4]
{ ... }
```

The `0xAAAA...` vs `0x5555...` pattern ensures that the physical cache set index bits (which index into the lower bits of the address for most direct-mapped and set-associative caches) are maximally different between source and destination, preventing the two buffers from evicting each other during a copy benchmark.

### 7.3 Pre-Touch

Before any timed measurement, every page of every buffer is written:

```rust
pub fn pre_touch(buf: &mut [u8]) {
    let page_size = platform::page_size();
    let mut i = 0;
    while i < buf.len() {
        buf[i] = 0;
        i += page_size;
    }
    // Write the last byte if not already covered
    if let Some(last) = buf.last_mut() { *last = 0; }
}
```

This commits all pages into physical RAM (combined with `mlockall`) so no page fault latency occurs during measurement.

### 7.4 Pointer-Chain Construction

A circular pointer-chase chain is built in a `&mut [usize]` slice (so elements are word-sized indices on the target platform). Construction applies the three-layer permutation from lmbench:

```rust
pub fn build_chain(
    buf: &mut [usize],
    range_bytes: usize,
    line_size: usize,
    page_size: usize,
) {
    let word_size = std::mem::size_of::<usize>();
    let n_pages   = range_bytes / page_size;
    let n_lines   = page_size   / line_size;
    let n_words   = line_size   / word_size;

    // Layer 1: Fisher-Yates shuffle of page indices
    let mut pages: Vec<usize> = (0..n_pages).map(|i| i * page_size).collect();
    fisher_yates_shuffle(&mut pages);

    // Layer 2: bit-reversal of line indices within page
    let lines: Vec<usize> = (0..n_lines)
        .map(|i| bit_reverse(i, ilog2(n_lines)) * line_size)
        .collect();

    // Layer 3: bit-reversal of word indices within line
    let words: Vec<usize> = (0..n_words)
        .map(|i| bit_reverse(i, ilog2(n_words)) * word_size)
        .collect();

    // Wire up the chain
    let mut prev_idx = 0usize;
    for &p in &pages {
        for &l in &lines {
            for &w in &words {
                let byte_offset = p + l + w;
                let idx = byte_offset / word_size;
                buf[prev_idx] = idx;
                prev_idx = idx;
            }
        }
    }
    buf[prev_idx] = 0; // close the loop
}
```

The `fisher_yates_shuffle` uses a seeded `xoshiro256**` PRNG (from a small vendored implementation — no external crate needed) to ensure reproducibility across runs given the same seed, while still producing a uniform random permutation.

---

## 8. Timing Infrastructure

### 8.1 Timer Granularity Calibration

Before any experiment the timer granularity is measured:

```rust
pub fn measure_granularity(n_samples: usize) -> f64 {
    // n_samples = 500
    let mut deltas: Vec<u64> = Vec::with_capacity(n_samples);
    for _ in 0..n_samples {
        let t0 = platform::clock_ns();
        let mut t1 = platform::clock_ns();
        while t1 == t0 { t1 = platform::clock_ns(); }
        deltas.push(t1 - t0);
    }
    deltas.sort_unstable();
    // 5th percentile: robust to OS noise, not sensitive to outliers
    deltas[n_samples / 20] as f64
}
```

`MINTIME_NS` is then set to `max(1_000_000, granularity_ns * 100)` — at least 1 ms, or 100× the granularity, whichever is larger. This follows Calibrator's convention but uses the actual measured granularity rather than a hardcoded constant.

### 8.2 Adaptive Iteration Calibration

For every measurement point, iterations are calibrated before timing:

```rust
pub fn calibrate_and_run<F>(kernel: F, min_time_ns: u64) -> (f64, u64)
where
    F: Fn(u64) -> u64,   // F(iterations) → elapsed_ns
{
    let mut iters: u64 = 1;
    loop {
        let elapsed = kernel(iters);
        if elapsed >= min_time_ns {
            return (elapsed as f64 / iters as f64, iters);
        }
        // Scale up iterations proportionally, with 10% headroom
        let scale = (min_time_ns as f64 / elapsed as f64 * 1.1).ceil() as u64;
        iters = if scale >= 8 { iters * 8 } else { iters * scale };
    }
}
```

### 8.3 Best-of-N Trial Selection

```rust
pub fn best_of_n<F>(kernel: F, n_trials: usize, min_time_ns: u64) -> f64
where
    F: Fn(u64) -> u64,
{
    let mut results: Vec<f64> = Vec::with_capacity(n_trials);
    for _ in 0..n_trials {
        let (ns_per_iter, _) = calibrate_and_run(&kernel, min_time_ns);
        results.push(ns_per_iter);
    }
    results.iter().cloned().fold(f64::INFINITY, f64::min)
}
```

`n_trials` defaults to 11 (`NUMTRIES`). The minimum is taken, not the average. The `calibrate_and_run` step ensures each trial meets `MINTIME_NS` independently.

---

## 9. Anti-Optimization Strategy

In native Rust/C, the compiler can eliminate measurement loops that produce no observable side effects. Resonance uses the following techniques — all standard practice in systems benchmarking:

### 9.1 `core::hint::black_box`

Rust's standard library provides `std::hint::black_box(x)`, which tells the compiler that `x` is "used" and cannot be eliminated or have its computation hoisted. This is used at the sink of every measurement loop:

```rust
use std::hint::black_box;

let mut ptr = chain[0] as *const usize;
for _ in 0..NUMLOADS {
    ptr = unsafe { *ptr as *const usize };
}
black_box(ptr);  // prevents the loop from being eliminated
```

### 9.2 `volatile` Reads via `std::ptr::read_volatile`

For the throughput pass where we want the compiler to issue actual loads without reordering:

```rust
// Force the compiler to emit a real load at this address
let val = unsafe { std::ptr::read_volatile(ptr) };
```

This is the Rust equivalent of `volatile T *p` in C.

### 9.3 Compiler Fence

Between the start/end timer calls and the measurement kernel, a compiler fence prevents the compiler from reordering loads/stores across the timing boundary:

```rust
use std::sync::atomic::{compiler_fence, Ordering};

let t0 = platform::clock_ns();
compiler_fence(Ordering::SeqCst);

// ... measurement kernel ...

compiler_fence(Ordering::SeqCst);
let t1 = platform::clock_ns();
```

### 9.4 LCG Zero-Buffer OR (tinymembench technique)

The LCG latency kernel uses a zero-filled buffer and OR's the load result into the seed. The compiler cannot prove the buffer is zero (it is passed as `&[u8]` through a function call boundary), so the load cannot be eliminated:

```rust
#[inline(never)]  // prevents inlining that would reveal the buffer is zero
fn lcg_random_read(buf: &[u8], n_loads: usize) -> u32 {
    let mask = (buf.len() - 1) as u32; // buf.len() is a power of 2
    let mut seed: u32 = 0x12345678;
    for _ in 0..n_loads {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let addr = ((seed >> 16) & mask) as usize;
        seed |= buf[addr] as u32; // load — cannot be eliminated
    }
    seed
}
```

The `#[inline(never)]` attribute is the Rust equivalent of tinymembench's `__attribute__((noinline))`.

### 9.5 Assembly Barrier for Bandwidth Kernels

For the assembly bandwidth kernels, a memory clobber in the inline asm ensures the compiler does not reorder memory operations around the kernel:

```rust
unsafe {
    core::arch::asm!(
        // ... bandwidth kernel ...
        options(nostack),
        // "memory" clobber: all memory may be read/written
    );
}
```

---

## 10. Latency Measurement

### 10.1 Pointer-Chase Kernel

The core latency kernel is an unrolled pointer-chase loop. The chain was built by `build_chain()` (§7.4) with Fisher-Yates + bit-reversal + word rotation. The traversal kernel:

```rust
/// Returns elapsed nanoseconds for `iters` full traversals of the chain.
/// `chain` is a slice of `usize` indices forming a circular linked list.
pub fn pointer_chase(chain: &[usize], iters: u64) -> u64 {
    use std::hint::black_box;
    use std::sync::atomic::{compiler_fence, Ordering};

    let base = chain.as_ptr();
    let mut idx: usize = 0;

    compiler_fence(Ordering::SeqCst);
    let t0 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    for _ in 0..iters {
        // 100 unrolled chases per iteration (HUNDRED macro equivalent)
        // The seq_macro crate or a build.rs code generator unrolls this.
        unsafe {
            idx = *base.add(*base.add(idx));   // 1
            idx = *base.add(*base.add(idx));   // 2
            // ... × 100
        }
    }

    compiler_fence(Ordering::SeqCst);
    let t1 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);

    black_box(idx);
    t1 - t0
}
```

100 dereferences are unrolled per outer-loop iteration to keep loop overhead negligible relative to L1 access time (~4 cycles).

### 10.2 Two-Pass Latency Isolation

Following Calibrator's two-pass approach:

**Pass 1 — Throughput pass** (`pointer_chase` as above): measures `T_replace` — how fast the hardware can sustain back-to-back dependent loads when the working set fits in a given level.

**Pass 2 — Latency pass**: inserts 100 dependent integer arithmetic operations between each pointer dereference. The arithmetic result depends on the previous load, preventing hoisting:

```rust
pub fn pointer_chase_with_delay(chain: &[usize], iters: u64) -> u64 {
    let base = chain.as_ptr();
    let mut idx: usize = 0;
    let mut delay: usize = 0;

    // REDUCE = 10: latency pass runs 10× fewer outer iterations
    let t0 = platform::clock_ns();
    for _ in 0..(iters / REDUCE) {
        unsafe {
            idx = *base.add(idx);     // one chase
        }
        // 100 dependent arithmetic ops (fill delay)
        for _ in 0..100 {
            delay = delay.wrapping_add(idx);  // depends on the load result
        }
    }
    let t1 = platform::clock_ns();
    black_box((idx, delay));
    t1 - t0
}
```

Miss latency (ns) is then:

```
miss_latency_ns = (latency_pass_ns / (iters / REDUCE))
                - (throughput_pass_ns / iters) * REDUCE_FACTOR
```

where `REDUCE_FACTOR = 10` accounts for the 100 arithmetic ops having negligible latency compared to a cache miss.

### 10.3 LCG Random-Read Kernel

The LCG kernel provides a cross-check that does not require pre-building a linked list:

```rust
pub fn lcg_random_read(buf: &[u8], iters: u64) -> u64 {
    let t0 = platform::clock_ns();
    let result = _lcg_inner(buf, iters);
    let t1 = platform::clock_ns();
    black_box(result);
    t1 - t0
}

#[inline(never)]
fn _lcg_inner(buf: &[u8], iters: u64) -> u32 {
    let mask = (buf.len() - 1) as u32;
    let mut seed: u32 = 0x12345678;
    for _ in 0..iters {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let v = ((seed >> 8) & 0xFF00) as u32;
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let addr = ((seed >> 16) & mask | v) as usize;
        seed |= buf[addr] as u32;  // forced load from computed address
    }
    seed
}
```

Baseline (ALU-only overhead) is measured with `buf.len() = 1` (always accesses index 0, always in L1). The baseline is subtracted from all LCG measurements to isolate memory latency.

---

## 11. Cache Hierarchy Detection

### 11.1 2D Sweep

The Orchestrator drives a 2D sweep over `(range, stride)` pairs. For each pair, `build_chain` constructs a chain with the given `range` and `stride`, and `best_of_n` returns the minimum ns/access over 11 trials.

**Range**: `MINRANGE = 1024` bytes to `maxRange`, at 3 points per power-of-2 octave (scale factors 0.75×, 1.0×, 1.25×).

**Stride**: swept from `sizeof(usize)` up to a maximum found by doubling until timing change is < 10% (EPSILON1), then swept back downward.

The raw results are stored in a `Matrix<f64>` indexed by `[range_idx][stride_idx]`, with dimensions encoded in the first element for serialization (following Calibrator's convention, though in Resonance this is explicit in the struct rather than a packed integer).

### 11.2 Cache Boundary Detection — Hybrid Algorithm

#### Primary: X-Ray Compact/Non-Compact Binary Search

Theorem 1 (Yotov et al.): for a cache with associativity `A`, block size `B`, and capacity `C`, let `T = C/A`. A sequence of `N` accesses at stride `S` is:
- **Compact** (all hits): `N ≤ A × ⌊T/S⌋`
- **Non-compact** (all misses): `N ≥ (A+1) × ⌊T/S⌋`

The `is_compact` test compares measured average access time against the L1 hit baseline with a 5% tolerance:

```rust
fn is_compact(
    chain: &mut [usize],
    stride: usize,
    n_elements: usize,
    l1_hit_ns: f64,
    timer: &dyn Platform,
) -> bool {
    build_chain_n(chain, stride, n_elements);
    let ns = best_of_n(|iters| pointer_chase(chain, iters), NUMTRIES, MINTIME_NS);
    ns < l1_hit_ns * 1.05
}
```

Cache parameter detection:

```rust
fn detect_cache_params(l1_hit_ns: f64, ...) -> (usize, usize) {
    // A = associativity, C = capacity
    let mut s: usize = 1;
    let mut n: usize = 1;

    // Phase 1: find non-compact N at S=1
    while is_compact(chain, s, n, l1_hit_ns) { n *= 2; }

    // Phase 2: double S, binary-search for smallest non-compact N
    loop {
        s *= 2;
        let n_old = n;
        n = binary_search(1, n_old, |candidate| !is_compact(chain, s, candidate, l1_hit_ns));
        if n == n_old { break; }
    }

    let associativity = n - 1;
    let capacity = (s / 2) * associativity;
    (associativity, capacity)
}
```

#### Secondary: Calibrator-Style Plateau/Jump Detector

Applied to the full `rawMatrix` as a cross-check and to detect any levels that the X-Ray binary search may miss (e.g., due to pseudo-associativity in victim caches):

```rust
fn analyze_plateaus(matrix: &Matrix<f64>, cpu_freq_ghz: f64) -> Vec<LevelCandidate> {
    let mut candidates = Vec::new();
    for stride_col in matrix.columns() {
        let mut window: VecDeque<f64> = VecDeque::with_capacity(LENPLATEAU);
        for (range_idx, &ns) in stride_col.iter().enumerate() {
            let cycles = ns * cpu_freq_ghz;
            window.push_back(cycles);
            if window.len() < LENPLATEAU { continue; }
            if window.len() > LENPLATEAU { window.pop_front(); }

            let span = window.iter().cloned().fold(0.0_f64, f64::max)
                     - window.iter().cloned().fold(f64::INFINITY, f64::min);

            if span >= EPSILON4 {
                // Jump detected: boundary at range_idx - LENPLATEAU + 1
                candidates.push(LevelCandidate { range_idx: range_idx - LENPLATEAU + 1 });
            }
        }
    }
    candidates
}
```

#### Merging

Results from both detectors are merged. Candidates within `|log10(latency_a) - log10(latency_b)| ≤ 0.3` (i.e., latencies within ~2×) are merged into a single level boundary. The final set is sorted by cache size.

#### Cache Line Size

For each detected level, the line size is the smallest stride `s` such that the measured latency remains within 10% (EPSILON1) of the plateau baseline for that level:

```rust
fn find_line_size(matrix: &Matrix<f64>, level: &CacheLevel) -> usize {
    let baseline = level.plateau_latency_ns;
    for &stride in STRIDES.iter() {   // strides from sizeof(usize) upward
        let ns = matrix.get(level.boundary_range_idx, stride);
        if ns <= baseline * (1.0 + EPSILON1) {
            return stride;
        }
    }
    // fallback: largest tested stride still on plateau
    STRIDES.last().copied().unwrap()
}
```

### 11.3 Page Coloring Mitigation

For large working sets (approaching or exceeding L3), physical cache set conflicts between pages can create false plateau artifacts. Resonance implements lmbench's approach:

1. Over-allocate the buffer by 2× (spare pages).
2. For each page in the active range, run `test_chunk`: measure latency with this page included vs. excluded.
3. If including the page causes a latency spike (ratio > 1.3×), swap it with a spare page from the over-allocation region (`fixup_chunk`).

This is implemented only for ranges > L2 size (to avoid the overhead for L1/L2 measurements).

### 11.4 Output: `CacheInfo`

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct CacheLevel {
    pub level: u32,
    pub size_bytes: u64,
    pub line_size_bytes: u32,
    pub associativity: u32,            // 0 = unknown (if X-Ray did not converge)
    pub miss_latency_ns: f64,
    pub miss_latency_cycles: f64,      // 0.0 if cpu_freq unknown
    pub replacement_time_ns: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CacheInfo {
    pub levels: Vec<CacheLevel>,
    pub detection_method: DetectionMethod,  // XRay | Calibrator | Hybrid
    pub memory_locked: bool,
    pub thread_pinned: bool,
}
```

---

## 12. TLB Detection

### 12.1 TLB Sweep

TLB parameters are detected by comparing latency when the working set spans many pages against the in-cache baseline. The stride is set to `cache_line_size + shift` (where `shift = page_size - cache_line_size` after modular arithmetic), ensuring each consecutive access lands on a different cache line and a different page:

```
tlb_stride = page_size   // simplest: one access per page, each on a different cache line
```

The sweep varies the number of distinct pages accessed (`spots`), from 1 up to a maximum where latency has clearly plateaued past the outermost TLB level.

For each spot count, the pointer chain links one element per page (one access per page, each access on a different cache line — guaranteed by the stride). Measured latency minus the in-cache L1 baseline isolates TLB miss penalty.

### 12.2 TLB Boundary Detection

```rust
fn find_tlb_levels(results: &[(usize, f64)], threshold: f64) -> Vec<TlbCandidate> {
    // results: (spot_count, latency_ns)
    let mut candidates = Vec::new();
    for window in results.windows(2) {
        let (spots_l, ns_l) = window[0];
        let (spots_r, ns_r) = window[1];
        if ns_r / ns_l > threshold {   // TLB_THRESHOLD = 1.15
            // TLB boundary between spots_l and spots_r
            // Binary search for exact entry count
            let exact = binary_search(spots_l, spots_r, |s| {
                measure_tlb_latency(s) / l1_baseline > threshold
            });
            candidates.push(TlbCandidate { entries: exact });
        }
    }
    candidates
}
```

### 12.3 Page Size

The actual system page size is obtained from `sysconf(_SC_PAGESIZE)` (or platform equivalent) — unlike the browser design, there is no ambiguity here. On Linux, hugepage size is also read from `/proc/meminfo`. If `madvise(MADV_HUGEPAGE)` is available, Resonance optionally runs a secondary TLB sweep with hugepages enabled to characterize the L2 TLB entry count for 2 MiB pages.

### 12.4 Output: `TlbInfo`

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct TlbLevel {
    pub level: u32,
    pub entries: u32,
    pub page_size_bytes: u64,
    pub miss_latency_ns: f64,
    pub miss_latency_cycles: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TlbInfo {
    pub levels: Vec<TlbLevel>,
}
```

---

## 13. Bandwidth Measurement

### 13.1 Kernel Variants

The following variants are measured at multiple buffer sizes (from L1 through main memory):

| Variant | Access Pattern | Notes |
|---------|---------------|-------|
| `seq_read` | Forward sequential read | Sum accumulation to prevent elimination |
| `seq_write` | Forward sequential write | Constant value stores |
| `seq_copy` | src → dst forward copy | Anti-aliased src/dst buffers |
| `seq_fill` | Zero-fill | `memset`-equivalent |
| `nt_write` | Non-temporal store (write-combining) | Bypasses cache; measures DRAM write BW |
| `nt_copy` | Load + non-temporal store | Peak DRAM copy bandwidth |
| `strided_read` | One read per cache line (stride = line_size) | Stresses prefetchers |
| `random_read` | LCG-addressed reads | Bandwidth under random access pressure |

### 13.2 Architecture-Specific Kernels

Resonance dispatches to architecture-specific inline assembly at runtime:

```rust
pub fn sequential_read(buf: &[u8]) -> u64 {
    #[cfg(target_arch = "x86_64")]
    return arch::x86_64::seq_read_avx2(buf);

    #[cfg(target_arch = "aarch64")]
    return arch::aarch64::seq_read_neon(buf);

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    return arch::generic::seq_read(buf);
}
```

**x86-64 AVX2 kernel** (256-bit loads, 8 loads before all stores):

```rust
unsafe fn seq_read_avx2(buf: &[u8]) -> u64 {
    let t0 = platform::clock_ns();
    core::arch::asm!(
        "2:",
        "vmovdqa ymm0, [{ptr}]",
        "vmovdqa ymm1, [{ptr} + 32]",
        // ... 8 loads total (256 bytes per iteration)
        "vpor ymm0, ymm0, ymm1",
        // ... accumulate
        "add {ptr}, 256",
        "cmp {ptr}, {end}",
        "jne 2b",
        ptr = inout(reg) ptr,
        end = in(reg) end,
        out("ymm0") _,
        // ...
        options(nostack),
    );
    let t1 = platform::clock_ns();
    t1 - t0
}
```

**x86-64 non-temporal write kernel** (256-bit `vmovntdq`):

```rust
unsafe fn nt_write_avx2(buf: &mut [u8]) -> u64 {
    core::arch::asm!(
        "vpxor ymm0, ymm0, ymm0",
        "2:",
        "vmovntdq [{ptr}], ymm0",
        "vmovntdq [{ptr} + 32]",
        // ...
        "sfence",
        // ...
    );
}
```

**AArch64 NEON kernel** (128-bit `ld1`, 4 loads per iteration):

```rust
unsafe fn seq_read_neon(buf: &[u8]) -> u64 {
    core::arch::asm!(
        "2:",
        "ld1 {{v0.16b, v1.16b, v2.16b, v3.16b}}, [{ptr}], #64",
        "orr v0.16b, v0.16b, v1.16b",
        // ...
        "cbnz {count}, 2b",
        // ...
    );
}
```

**Generic fallback** (auto-vectorized Rust, no inline asm):

```rust
pub fn seq_read_generic(buf: &[u8]) -> u64 {
    let mut acc: u64 = 0;
    let t0 = platform::clock_ns();
    for chunk in buf.chunks_exact(8) {
        acc = acc.wrapping_add(u64::from_le_bytes(chunk.try_into().unwrap()));
    }
    black_box(acc);
    platform::clock_ns() - t0
}
```

CPU feature detection uses `std::is_x86_feature_detected!("avx2")` and `std::is_aarch64_feature_detected!("neon")` at runtime for safe dispatch.

### 13.3 Measurement Methodology

Following tinymembench:

- **Adaptive timing**: double `iters` until elapsed ≥ `MINBW_TIME_NS` (500 ms default).
- **Statistical convergence**: after ≥ 3 samples, if `stddev < 0.001 × peak` → early termination.
- **Peak selection**: report the maximum bandwidth across all trials. Noise only reduces bandwidth.
- **`MAXREPEATS = 10`**: hard upper bound on trial count.

```
bandwidth_GB_s = (bytes_per_iter × iters) / elapsed_ns
```

### 13.4 Two-Pass L1 Bandwidth

For cache-level bandwidth (isolating L1/L2 from DRAM), data is transferred in 2 KiB chunks through a small temporary buffer that fits in L1. This ensures the bandwidth measurement reflects the L1↔L2 or L2↔L3 path, not the DRAM path.

### 13.5 Output: `BandwidthResults`

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct BandwidthPoint {
    pub buffer_size_bytes: u64,
    pub bandwidth_gbs: f64,
    pub variant: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BandwidthResults {
    pub points: Vec<BandwidthPoint>,
    pub peak_read_gbs: f64,
    pub peak_write_gbs: f64,
    pub peak_copy_gbs: f64,
    pub peak_nt_write_gbs: f64,
    pub peak_nt_copy_gbs: f64,
}
```

---

## 14. CPU Frequency Detection

Following X-Ray: a chain of dependent integer additions runs at approximately 1 addition per cycle. Measuring elapsed time over a known count gives CPU frequency.

```rust
pub fn estimate_cpu_freq_ghz() -> f64 {
    const N: u64 = 50_000_000;
    let mut acc: u64 = 1;
    let t0 = platform::clock_ns();
    compiler_fence(Ordering::SeqCst);
    for _ in 0..N {
        // Must be dependent additions to prevent out-of-order parallelism
        acc = acc.wrapping_add(acc);
    }
    compiler_fence(Ordering::SeqCst);
    let t1 = platform::clock_ns();
    black_box(acc);
    N as f64 / (t1 - t0) as f64  // additions/ns = GHz
}
```

Rust's optimizer may eliminate or vectorize the loop. To prevent this, the assembly form is preferred on supported architectures:

```rust
#[cfg(target_arch = "x86_64")]
pub fn estimate_cpu_freq_ghz() -> f64 {
    const N: u64 = 50_000_000;
    let mut acc: u64 = 1;
    let t0 = platform::clock_ns();
    unsafe {
        core::arch::asm!(
            "mov {n}, {iters}",
            "2: add {acc}, {acc}",
            "dec {n}",
            "jnz 2b",
            iters = in(reg) N,
            n = out(reg) _,
            acc = inout(reg) acc,
            options(nostack),
        );
    }
    let t1 = platform::clock_ns();
    N as f64 / (t1 - t0) as f64
}
```

The result is cross-checked against the OS-reported frequency (sysfs/sysctl). If they differ by > 20%, a warning is emitted (possible frequency scaling during measurement). The measurement-based value is used for cycle conversion; the OS value is reported alongside for reference.

---

## 15. Memory-Level Parallelism (MLP) Measurement

### 15.1 Method

MLP is measured by running 1–16 independent pointer-chasing chains simultaneously within the same loop body and measuring aggregate throughput. Each chain uses a different word offset within each cache line (lmbench's `words[k % nwords]` rotation), preventing aliasing.

```rust
/// Measures aggregate throughput for `k` simultaneous chains.
/// Returns ns per access (total accesses = k × traversal_length).
pub fn multi_chain_latency(chains: &[&[usize]], iters: u64) -> f64 {
    // chains.len() = k; each chain is independently built with different word offset
    let t0 = platform::clock_ns();
    let mut ptrs: [usize; 16] = [0; 16];
    for (i, chain) in chains.iter().enumerate() { ptrs[i] = 0; }

    for _ in 0..iters {
        // Unrolled for k chains (generated by macro or build.rs for k = 1..=16)
        unsafe {
            ptrs[0] = *chains[0].as_ptr().add(ptrs[0]);
            ptrs[1] = *chains[1].as_ptr().add(ptrs[1]);
            // ... up to ptrs[k-1]
        }
    }
    let t1 = platform::clock_ns();
    black_box(ptrs);
    (t1 - t0) as f64 / (iters * chains.len() as u64) as f64
}
```

### 15.2 MLP Derivation

```
mlp_factor(k) = ns_per_access(1) / ns_per_access(k)
```

When `mlp_factor(k)` stops increasing proportionally to `k`, the memory system's outstanding-request buffer is saturated. The estimated MLP is the last `k` where `mlp_factor(k) / k > 0.85`.

### 15.3 Output: `MlpResults`

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct MlpResults {
    pub measurements: Vec<MlpPoint>,
    pub estimated_mlp: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MlpPoint {
    pub chains: u32,
    pub ns_per_access: f64,
    pub relative_throughput: f64,
}
```

---

## 16. Analysis Pipeline

### 16.1 Execution Order

The Orchestrator runs experiments in the following order. Each step's output informs subsequent steps:

```
1. Timer calibration          → MINTIME_NS, granularity_ns
2. CPU frequency detection    → cpu_freq_ghz (used in all cycle conversions)
3. Cache latency 2D sweep     → rawCacheMatrix
4. Cache analysis             → CacheInfo (levels, line_size, associativity)
5. TLB sweep                  → rawTlbMatrix  (uses line_size from step 4)
6. TLB analysis               → TlbInfo
7. Bandwidth sweep            → BandwidthResults  (uses cache sizes from step 4
                                                   to select buffer sizes)
8. MLP sweep                  → MlpResults  (uses L3 size from step 4 as working set)
```

### 16.2 Confidence Scoring

Each result is annotated with a confidence level:

| Condition | Confidence |
|-----------|-----------|
| X-Ray and plateau/jump detector agree within 10% | `High` |
| Only one method produced a result, or methods agree within 25% | `Medium` |
| Methods disagree by > 25%, or `memory_locked = false`, or `thread_pinned = false` | `Low` |

### 16.3 No I/O During Measurement

All output (progress messages, intermediate logs) is buffered in memory and emitted only after the entire experiment sequence completes. No writes to stdout, stderr, or files occur between the start of timer calibration and the end of the MLP sweep. This is the most important lesson from the magsilva fork.

---

## 17. Output Format

### 17.1 Terminal Output

By default, Resonance prints a human-readable summary:

```
Resonance — Memory Hierarchy Characterization
==============================================
System : linux x86_64
CPU    : ~3.60 GHz (measured), 3600 MHz (reported)
Memory : 15.5 GiB available
Pinned : core 0
Locked : yes

Cache Hierarchy
───────────────
Level  Size      Line  Assoc  Miss Latency    Replacement
L1     32 KiB    64 B  8-way   4.2 ns  15 cy   0.8 ns
L2    512 KiB    64 B  8-way  14.1 ns  51 cy   4.2 ns
L3     16 MiB    64 B  16-way 41.8 ns 150 cy  12.1 ns
DRAM    —         —     —    86.3 ns  311 cy     —

TLB
───
Level  Entries  Page Size  Miss Latency
L1D       64     4 KiB      0.9 ns   3 cy
L2       1536    4 KiB     10.2 ns  37 cy

Bandwidth (peak, DRAM working set)
──────────────────────────────────
Sequential Read    42.1 GB/s
Sequential Write   24.8 GB/s
Sequential Copy    31.7 GB/s
Non-temporal Write 47.3 GB/s
Non-temporal Copy  38.2 GB/s
Random Read (DRAM)  8.4 GB/s

Memory-Level Parallelism
────────────────────────
Chains  1   2   4   8  12  16
Rel BW  1.0 1.9 3.7 7.0 9.1 9.8
Estimated MLP: 10
```

### 17.2 JSON Output

With `--json` flag, the full `ResonanceResults` struct is serialized:

```json
{
  "timestamp": "2026-04-08T10:23:45Z",
  "platform": { "os": "linux", "arch": "x86_64" },
  "cpu_freq_ghz": 3.601,
  "timer_granularity_ns": 23.0,
  "memory_locked": true,
  "thread_pinned": true,
  "thread_core": 0,
  "cache": {
    "detection_method": "Hybrid",
    "levels": [
      {
        "level": 1,
        "size_bytes": 32768,
        "line_size_bytes": 64,
        "associativity": 8,
        "miss_latency_ns": 4.2,
        "miss_latency_cycles": 15.1,
        "replacement_time_ns": 0.8
      }
    ]
  },
  "tlb": { ... },
  "bandwidth": { ... },
  "mlp": { ... },
  "duration_ms": 47230
}
```

---

## 18. CLI

```
resonance [OPTIONS]

OPTIONS:
  --json              Emit JSON to stdout instead of human-readable output
  --core <N>          Pin to CPU core N (default: 0)
  --max-mem <SIZE>    Maximum memory range, e.g. 512M, 2G (default: auto)
  --trials <N>        Trials per measurement point (default: 11)
  --no-lock           Skip mlockall (useful if running without privileges)
  --no-pin            Skip thread pinning
  --skip-bandwidth    Skip bandwidth measurements (faster run)
  --skip-mlp          Skip MLP measurements (faster run)
  --skip-tlb          Skip TLB measurements
  --seed <N>          RNG seed for chain construction (default: 42)
  -v, --verbose       Print per-experiment progress to stderr during run
  -h, --help          Print help
  -V, --version       Print version
```

Note: `--verbose` writes progress to **stderr**, never stdout. JSON or human-readable output always goes to stdout. This preserves the ability to pipe `--json` output through `jq` while still seeing progress.

---

## 19. Key Algorithmic Parameters

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| `MINRANGE` | 1024 bytes | Below this, L1 latency measurements are dominated by setup overhead |
| `RANGE_STEPS_PER_OCTAVE` | 3 | Sub-power-of-2 sampling (0.75×, 1.0×, 1.25×) for finer boundary resolution |
| `NUMLOADS` | 100 (unroll) × calibrated outer iters | 100-unrolled inner loop to amortize loop overhead |
| `NUMTRIES` | 11 | Best-of-N minimum selection; odd count, consistent with lmbench |
| `REDUCE` | 10 | Latency-pass iteration divisor (Calibrator convention) |
| `LENPLATEAU` | 3 | Consecutive readings to confirm a plateau |
| `EPSILON1` | 0.10 (10% relative) | Plateau continuity threshold across strides |
| `EPSILON4` | 1.0 cycle absolute | Jump detection threshold |
| `CACHE_THRESHOLD` | 1.50 (latency ratio) | Cache boundary detection ratio (lmbench) |
| `TLB_THRESHOLD` | 1.15 (latency ratio) | TLB boundary detection ratio (lmbench) |
| `LEVEL_MERGE_LOG` | 0.30 (log₁₀ difference) | Merge adjacent levels if latencies within ~2× |
| `MINBW_TIME_NS` | 500,000,000 (500 ms) | Minimum bandwidth trial duration |
| `BW_MAXREPEATS` | 10 | Maximum bandwidth trial repetitions |
| `BW_CONVERGENCE` | 0.001 (0.1% std/max) | Early termination for bandwidth |
| `MAX_MLP_CHAINS` | 16 | Maximum independent chains for MLP |
| `ALIGN_PADDING` | 1 MiB | Over-allocation for anti-aliased buffer layout |
| `PAGE_COLORING_RATIO` | 1.30 (30% latency increase) | Threshold for detecting a conflicting physical page |

---

## 20. Glossary

| Term | Definition |
|------|-----------|
| Cache line | Smallest unit of data transfer between cache levels (typically 32–128 bytes) |
| Cache miss | Access that cannot be served from cache; data must be fetched from a slower level |
| Compact sequence | X-Ray term: access sequence in which every access is a cache hit |
| Non-compact sequence | X-Ray term: access sequence in which every access is a cache miss |
| Fisher-Yates shuffle | Unbiased in-place random permutation; used for inter-page ordering in chain construction |
| Bit-reversal permutation | Maps index `i` to its bit-reversed value; maximizes stride variation within a page |
| LCG | Linear Congruential Generator: `seed = seed × 1103515245 + 12345 mod 2³²` |
| MLP | Memory-Level Parallelism: number of outstanding memory requests the hardware can service simultaneously |
| Plateau | Region of stable latency as working set grows → data fits within one cache level |
| Jump | Abrupt latency increase as working set exceeds a cache level boundary |
| Pointer chasing | Traversing a linked list where each element's value is the index of the next; creates serialized, non-parallelizable, non-prefetchable memory accesses |
| Spots | Number of distinct pages accessed in a TLB sweep (`range / page_stride`) |
| Stride | Step size in bytes between consecutively accessed addresses |
| TLB | Translation Lookaside Buffer: hardware cache for virtual-to-physical address translations |
| Working set | Total amount of data accessed in one measurement pass; determines which cache level is exercised |
| Anti-aliased buffers | Buffers placed at complementary bit-pattern offsets to map to maximally different cache sets |
| Two-pass measurement | Throughput pass + latency pass; difference isolates pure miss penalty from loop overhead |
| `mlockall` | POSIX system call locking all current and future memory mappings into physical RAM |
| `sched_setaffinity` | Linux system call for pinning a thread to a specific CPU core |
| `CLOCK_MONOTONIC_RAW` | Linux clock source unaffected by NTP or `adjtimex`; preferred for benchmarking |
| Non-temporal store | Memory write that bypasses the cache hierarchy (`vmovntdq` on x86); used to measure DRAM write bandwidth |
| Write-combining buffer | Hardware buffer that coalesces non-temporal stores before flushing to DRAM |
| Page coloring | Controlling which physical pages are used to avoid cache set conflicts in large working sets |
| `black_box` | `std::hint::black_box`: Rust compiler intrinsic that prevents dead-code elimination of its argument |
| `compiler_fence` | `std::sync::atomic::compiler_fence`: prevents the compiler from reordering memory accesses across the call |
