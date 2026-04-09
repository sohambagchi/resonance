# Resonance — Design Document

## 1. Overview

Resonance is a **memory hierarchy characterization tool** that automatically measures the hardware parameters of a computer's memory subsystem. Given a target machine, Resonance produces a complete, structured characterization of its cache hierarchy, TLB, memory bandwidth, and memory-level parallelism — without requiring any prior knowledge of the hardware.

This document describes the full design of Resonance as a **browser-based web application**. Rather than a native binary that must be compiled and run on the target machine, Resonance is designed to execute entirely within a modern web browser, using the browser's JavaScript runtime as the measurement engine.

---

## 2. Goals and Non-Goals

### Goals

- Automatically detect the number of cache levels and their sizes, line sizes, associativity, and miss latencies
- Automatically detect TLB levels, entry counts, page sizes, and miss latencies
- Measure memory bandwidth (read, write, copy, fill)
- Measure memory-level parallelism (MLP)
- Present results in a structured, human-readable format
- Require zero installation — run from a browser tab with no compilation step
- Be portable across operating systems and architectures (measurements mediated by the JS engine and browser runtime)

### Non-Goals (for this design)

- Providing assembly-level bandwidth kernels (the browser sandbox prevents direct ISA access)
- Controlling CPU affinity, NUMA placement, hugepages, or memory locking (`mlockall`)
- Measuring instruction latency and throughput at the micro-architectural level
- Outputting gnuplot scripts or other external visualization formats
- Measuring instruction cache parameters
- Providing frequency scaling control

---

## 3. Background and Prior Art

Resonance synthesizes lessons from five existing tools and one academic paper studied in the knowledge base:

| Reference | Key Contribution |
|-----------|----------------|
| Calibrator (Manegold) | Foundational 2D (range × stride) sweep with plateau/jump detection |
| Calibrator (fukien fork) | Modernization patches for musl libc compatibility |
| Calibrator (magsilva fork) | Critical insight: removing file I/O during measurement restored accuracy by an order of magnitude; `double` precision fixes; auto-detection of CPU frequency and memory size |
| lmbench | Multi-layer prefetch defeat (Fisher-Yates + bit-reversal + word rotation); benchmp multi-process framework; adaptive iteration calibration; page coloring mitigation |
| tinymembench | LCG-based latency measurement without pre-built linked lists; anti-aliased buffer allocation; standard-deviation-based early termination |
| X-Ray (Yotov et al., 2005) | Formal, provably correct cache parameter detection via Theorem 1 compact/non-compact sequences; `switch(volatile)` anti-optimization technique; CPU frequency auto-detection from integer ADD latency |

The most important lessons drawn from this prior art, which directly shape the Resonance design, are:

1. **Never perform I/O during measurement.** The magsilva fork demonstrated that file writes in the hot path corrupt timing results by an order of magnitude.
2. **Use multi-layer prefetch defeat.** Modern CPUs predict simple forward-stride and even shuffled-stride patterns. lmbench's Fisher-Yates inter-page + bit-reversal intra-page combination is the strongest heuristic approach.
3. **Formal detection outperforms heuristics.** X-Ray's Theorem 1 binary search is more accurate than threshold-based plateau/jump state machines.
4. **Use `double`, never `float`.** 32-bit floating-point causes false boundary detections due to precision loss in timing calculations.
5. **Take the minimum of multiple trials.** Noise only adds latency or reduces bandwidth; it never has the opposite effect.
6. **Adaptive iteration calibration.** Never hardcode iteration counts; always calibrate to a minimum timing window.
7. **Anti-aliased buffer allocation.** Buffers at complementary bit-pattern offsets prevent cache set conflicts between source, destination, and scratch areas.

---

## 4. Browser Feasibility Analysis

### What Is Available

Modern browsers provide several primitives that make memory measurement possible:

- **`performance.now()`** — sub-millisecond monotonic timer with microsecond resolution (in most browsers; some apply jitter for Spectre mitigation). This is the replacement for `gettimeofday()`.
- **`SharedArrayBuffer` + `Atomics`** — provides shared memory between the main thread and Web Workers, and atomic operations that can serve as synchronization primitives. When available (requires COOP/COEP headers), this enables a stable high-resolution timer via a counter thread.
- **Web Workers** — background threads for offloading computation, analogous to the child processes in lmbench's `benchmp` framework.
- **`WebAssembly` (Wasm)** — compilation target that executes at near-native speed, with predictable register allocation and minimal compiler optimization interference.
- **`ArrayBuffer` / `TypedArray`** — typed, flat memory regions analogous to `malloc`-allocated buffers in C.
- **`DataView`** — byte-level buffer access for precise memory layout control.

### Key Constraints

| Constraint | Impact on Design |
|-----------|----------------|
| No `sched_setaffinity()` | Cannot pin to a specific CPU core; threads may migrate between measurements |
| No `mlockall()` | Browser GC may interrupt measurements; mitigated by pre-touching all buffers |
| No `MADV_HUGEPAGE` | Cannot control TLB page size; TLB measurements reflect default 4 KiB pages |
| Timer resolution jitter (Spectre mitigations) | `performance.now()` may have 1ms granularity in some contexts; `SharedArrayBuffer` counter thread can bypass this |
| No ISA-specific assembly | Bandwidth kernels must use Wasm SIMD (128-bit) rather than SSE2/AVX or NEON |
| Garbage collector pauses | Pre-touch buffers; avoid allocations in hot paths; use pre-allocated typed arrays |
| JIT unpredictability | Anti-optimization techniques must be adapted for JS semantics (see §7) |
| Same-origin security | All measurement code runs in a single origin; no cross-origin concerns |
| `performance.now()` jitter baseline | Must measure timer granularity before setting minimum timing windows |

### The Timer Problem

The most significant browser constraint is timer resolution. Under standard browser contexts, `performance.now()` is quantized to 1ms (Firefox default, post-Spectre). However, when a page is served with:

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

`SharedArrayBuffer` becomes available, which enables the **counter thread trick**: a Web Worker increments a `SharedArrayBuffer` integer in a tight loop, and the main thread reads this integer as a monotonic counter with ~5µs effective resolution.

Resonance **requires** one of two deployment modes:
1. **High-resolution mode**: Page served with COOP/COEP headers → `SharedArrayBuffer` available → counter-thread timer.
2. **Low-resolution fallback**: Standard context → `performance.now()` with 1ms or 0.1ms granularity → minimum timing window extended to ensure measurements are ≥ 100× the clock granularity.

---

## 5. Architecture

### 5.1 Component Overview

```
┌─────────────────────────────────────────────────────────┐
│                     Browser Main Thread                  │
│                                                          │
│  ┌──────────────┐    ┌──────────────┐    ┌───────────┐  │
│  │   UI Layer   │    │  Orchestrator│    │  Results  │  │
│  │  (controls,  │◄──►│  (schedules  │◄──►│  Renderer │  │
│  │  progress)   │    │  experiments)│    │           │  │
│  └──────────────┘    └──────┬───────┘    └───────────┘  │
│                             │                            │
└─────────────────────────────┼──────────────────────────-┘
                              │ postMessage / SharedArrayBuffer
              ┌───────────────┼───────────────┐
              │               │               │
     ┌────────▼──────┐ ┌──────▼──────┐ ┌─────▼──────────┐
     │  Timer Worker │ │ Measurement │ │  Wasm Bandwidth │
     │ (SharedArrayBuf│ │  Worker     │ │  Worker        │
     │  counter loop)│ │ (pointer    │ │ (SIMD kernels) │
     └───────────────┘ │  chase, LCG,│ └────────────────┘
                       │  analysis)  │
                       └─────────────┘
```

### 5.2 Layers

**UI Layer** — Renders the control panel (start button, configuration options), a progress indicator, and the final results table. Receives structured result objects from the Orchestrator; does no computation.

**Orchestrator** — The control plane. Manages the sequence of experiments (cache latency sweep, TLB sweep, bandwidth sweep, MLP sweep). Schedules work to Web Workers via `postMessage`. Collects partial results and drives the analysis pipeline.

**Timer Worker** — A dedicated Web Worker that, when `SharedArrayBuffer` is available, runs a tight integer increment loop in a `SharedArrayBuffer`. Provides a sub-millisecond monotonic counter without accessing `performance.now()`. When `SharedArrayBuffer` is unavailable, this worker is not created and the system falls back to `performance.now()`.

**Measurement Worker** — A Web Worker that owns the pre-allocated `TypedArray` buffers and executes all measurement kernels (pointer chasing for latency, LCG random-read for latency cross-check, strided reads for bandwidth via JS). Receives experiment parameters from the Orchestrator and returns raw timing results. Never performs any I/O or DOM access during measurement.

**Wasm Bandwidth Worker** — A Web Worker loading a WebAssembly module compiled from a bandwidth kernel. Uses Wasm SIMD (128-bit `v128`) instructions for read, write, copy, and fill bandwidth measurements. Falls back to scalar Wasm on browsers without SIMD support.

### 5.3 Data Flow

```
Orchestrator
  │
  ├──► Timer Worker: start counting
  │
  ├──► Measurement Worker: { experiment: "cache_latency", ranges, strides }
  │        │
  │        └──► (runs 2D sweep, sends back raw result matrix)
  │
  ├──► Orchestrator: analyzeCache(rawMatrix) → CacheInfo[]
  │
  ├──► Measurement Worker: { experiment: "tlb_latency", spots, strides }
  │        └──► (raw TLB result matrix)
  │
  ├──► Orchestrator: analyzeTLB(rawMatrix, CacheInfo) → TLBInfo[]
  │
  ├──► Wasm Bandwidth Worker: { experiment: "bandwidth", sizes, variants }
  │        └──► (bandwidth in GB/s per variant per size)
  │
  ├──► Measurement Worker: { experiment: "mlp", chains: 1..16, size }
  │        └──► (aggregate throughput per chain count)
  │
  └──► Results Renderer: { cache: CacheInfo[], tlb: TLBInfo[], bw: BwResults, mlp: MlpResults }
```

---

## 6. Memory Buffer Management

### 6.1 Buffer Sizing

The Orchestrator first estimates total physical memory by iteratively allocating and pre-touching `TypedArray`s until allocation fails or a reasonable upper bound is reached. It then sets `maxRange` to a fraction of estimated available memory (default: 50% of detected or user-specified maximum).

### 6.2 Anti-Aliased Allocation

Following tinymembench's approach, the Measurement Worker allocates a single large `ArrayBuffer` and derives multiple views at offsets chosen to maximize cache-set diversity:

```
// Complementary bit-pattern offsets (in bytes)
const OFFSETS = [0xAAAAAAAA, 0x55555555, 0xCCCCCCCC, 0x33333333];

function allocAntiAliasedBuffers(totalSize, count) {
  const backing = new ArrayBuffer(totalSize + ALIGN_PADDING);
  const base = alignToPage(backing);
  return Array.from({ length: count }, (_, i) =>
    new Float64Array(backing, (base + OFFSETS[i]) % ALIGN_PADDING, totalSize / 8)
  );
}
```

The `0xAAAA...` vs `0x5555...` complementary pattern ensures buffers map to maximally different cache sets in set-associative caches, preventing false eviction interference between source and destination buffers during bandwidth measurements.

### 6.3 Pre-Touch

Before any timed measurement begins, the Measurement Worker writes to every page of every buffer to:
- Ensure all pages are committed (no page-fault latency during measurement)
- Warm caches in a controlled way
- Prevent GC from reclaiming the buffers

```
function preTouch(buffer) {
  const stride = 4096 / buffer.BYTES_PER_ELEMENT;
  for (let i = 0; i < buffer.length; i += stride) {
    buffer[i] = 0;
  }
}
```

---

## 7. Anti-Optimization Techniques

### 7.1 The Core Problem

JavaScript JIT compilers aggressively optimize: dead-code elimination, loop invariant hoisting, constant folding, and inlining can all remove the very loads that Resonance is trying to measure. Native tools use `volatile`, compiler barriers, and `asm volatile("":::"memory")`. None of these exist in JS.

### 7.2 Technique 1: Result Sinking via Shared State

Computed values are accumulated into a shared `result` variable that is read back after the loop and compared against an opaque external value. Because the final comparison is observable, the JIT cannot prove the loop is dead:

```js
let sink = 0;
for (let i = 0; i < count; i++) {
  sink += buffer[computeIndex(i)];  // load cannot be eliminated
}
// Force the JIT to preserve the loop: store result where it can be observed
resultCell[0] = sink;
```

`resultCell` is a `SharedArrayBuffer`-backed `Int32Array` shared with the Orchestrator. Writing to shared memory is a visible side-effect the JIT cannot remove.

### 7.3 Technique 2: Opaque Index Computation

For pointer chasing, the chain index is derived from the previous load value. The JIT cannot trace through an indeterminate memory load to determine the next index:

```js
let idx = startIdx;
for (let i = 0; i < UNROLL; i++) {
  idx = chainBuffer[idx];  // next index depends on memory content
}
```

### 7.4 Technique 3: WebAssembly for Critical Kernels

Wasm has a well-defined, stable execution model without JIT speculation. For latency measurements where predictability is critical, the pointer-chase and LCG kernels are implemented in Wasm, compiled offline and loaded as a `.wasm` blob. Wasm's `i32.load` and `i64.load` instructions are direct memory reads without JIT transformation.

### 7.5 Technique 4: Opaque Function Calls at Measurement Boundaries

Calls to `performance.now()` (or the SharedArrayBuffer counter read) at measurement start and end must not be hoisted into or out of the timed region. Wrapping them in functions that cross Wasm↔JS call boundaries prevents JIT cross-function optimization:

```js
// Called from Wasm — JIT cannot hoist across the boundary
function getTime() { return performance.now(); }
```

### 7.6 Technique 5: LCG with Zero-Buffer OR (tinymembench-inspired)

For LCG-based latency measurement, the buffer is zero-filled and the LCG address is OR'd with the load result. Because the JIT cannot prove the buffer is all zeros (it may have been mutated by a Worker), the load cannot be eliminated:

```js
const BUF = new Uint8Array(bufferSize);  // zero-filled, but JIT doesn't know
let seed = INITIAL_SEED;
for (let i = 0; i < NUMLOADS; i++) {
  seed = (seed * 1103515245 + 12345) >>> 0;
  const addr = (seed >>> 16) & addrMask;
  seed |= BUF[addr];  // load from buffer; cannot eliminate (BUF may be non-zero)
}
```

---

## 8. Timing Infrastructure

### 8.1 Timer Granularity Calibration

Before any experiment, the Orchestrator calibrates the available timer:

```
timerGranularity = calibrateTimer():
  readings = []
  for i in 1..200:
    t0 = readTimer()
    repeat until readTimer() != t0
    readings.push(readTimer() - t0)
  return percentile(readings, 5)  // 5th percentile of smallest observed steps
```

All subsequent `MINTIME` values are set to at least `100 × timerGranularity`, following Calibrator's convention.

### 8.2 High-Resolution Counter Thread

When `SharedArrayBuffer` is available:

```js
// Timer Worker
const counter = new Int32Array(new SharedArrayBuffer(4));
while (true) {
  Atomics.add(counter, 0, 1);  // tight increment loop, ~5µs resolution
}

// Measurement kernel reads:
function readTimer() {
  return Atomics.load(counter, 0);
}
```

The counter wraps at 2³¹; the Measurement Worker handles wrap-around in delta calculations.

### 8.3 Adaptive Iteration Calibration

Analogous to lmbench's BENCH_INNER and Calibrator's doubling loop:

```
iterations = 1
loop:
  elapsed = runKernel(iterations)
  if elapsed < MINTIME:
    if elapsed < MINTIME / 8:
      iterations *= 8
    else:
      iterations = ceil(iterations * MINTIME / elapsed * 1.1)
    continue
  break
```

This ensures the timing window is always at least `MINTIME`, regardless of memory access speed.

### 8.4 Best-of-N Trial Selection

Following all reference tools, Resonance takes the **minimum** across `NUMTRIES = 11` trials:

```
results = []
for i in 1..NUMTRIES:
  results.push(runKernelWithCalibration())
return min(results)
```

Noise only inflates measured latency or deflates measured bandwidth. The minimum across many trials is the most reliable estimator of true hardware performance.

---

## 9. Latency Measurement

### 9.1 Pointer-Chasing Chain Construction

For cache latency measurement, a circular pointer-chasing chain is embedded in a `Uint32Array` (indices, since JS has no raw pointers). The chain is constructed using lmbench's multi-layer permutation to defeat hardware prefetchers:

**Layer 1 — Inter-page permutation (Fisher-Yates)**:
```
pages = [0, pageSize, 2*pageSize, ..., (nPages-1)*pageSize]
fisherYatesShuffle(pages)  // uniformly random page visit order
```

**Layer 2 — Intra-page permutation (bit-reversal)**:
```
function bitReverse(x, nbits):
  result = 0
  for i in 0..nbits-1:
    result = (result << 1) | (x & 1)
    x >>= 1
  return result

lines = Array.from({length: nLines}, (_, i) => bitReverse(i, log2(nLines)) * lineSize)
```

**Layer 3 — Word rotation within cache lines**:
```
words = Array.from({length: nWords}, (_, i) => bitReverse(i, log2(nWords)) * wordSize)
// Each chain k uses word offset: words[k % nWords]
```

Chain construction iterates `pages × lines × words` in the permuted order, writing each entry's index into the previous entry's slot:

```
function buildChain(buffer, range, lineSize, pageSize):
  nPages = range / pageSize
  nLines = pageSize / lineSize
  nWords = lineSize / 4  // 4 bytes per Uint32 index
  
  pages = shuffledPages(nPages, pageSize)
  lines = bitReversedLines(nLines, lineSize)
  words = bitReversedWords(nWords)
  
  let prev = 0
  for p of pages:
    for l of lines:
      for w of words:
        addr = (p + l + w) / 4  // convert byte offset to Uint32 index
        buffer[prev] = addr
        prev = addr
  buffer[prev] = 0  // close the loop
```

### 9.2 Chain Traversal Kernel

The traversal kernel is implemented in Wasm for predictable JIT behavior. The core loop in WAT (WebAssembly Text Format):

```wat
(loop $chase
  (local.set $idx (i32.load (i32.add (local.get $base) (i32.shl (local.get $idx) (i32.const 2)))))
  (local.set $n   (i32.sub (local.get $n) (i32.const 1)))
  (br_if $chase   (local.get $n))
)
```

100 iterations are unrolled per outer loop body (matching Calibrator's `HUNDRED` macro), avoiding loop overhead dominating for fast L1 accesses.

### 9.3 LCG Random-Read Kernel (Cross-Check)

An alternative latency measurement using the tinymembench LCG approach is run in parallel as a cross-check. The Wasm implementation:

```wat
;; seed = seed * 1103515245 + 12345
;; addr = (seed >> 16) & addrMask
;; seed |= mem[addr]  ;; zero-buffer OR forces load without linked list
```

Three LCG iterations produce a 31-bit address. Because the buffer is zero-filled, the OR is a no-op at runtime, but the JIT/Wasm runtime cannot prove this and must emit the load.

The LCG baseline (measured with `addrMask = 0`, always accessing element 0) is subtracted from all LCG measurements to isolate the memory latency component from ALU overhead.

### 9.4 Two-Pass Latency Isolation

Following Calibrator's two-pass approach, for each `(range, stride)` point two measurements are taken:

1. **Throughput pass**: tight pointer-chase loop with no injected delay. Measures `replacement_time`.
2. **Latency pass**: pointer-chase loop with 100 arithmetic fill operations between each access (using results from the previous load to prevent hoisting). Measures `replacement_time + miss_penalty_contribution`.

`miss_latency ≈ latency_pass_time − throughput_pass_time × REDUCE_FACTOR`

where `REDUCE_FACTOR = 10` accounts for the reduced iteration count in the latency pass.

---

## 10. Cache Hierarchy Detection

### 10.1 2D Sweep

The Measurement Worker performs a 2D sweep over `(range, stride)` pairs:

**Range**: From `MINRANGE = 1024` bytes to `maxRange`, at 3 points per power-of-2 octave (factors 0.75×, 1.0×, 1.25×), following Calibrator's sub-power-of-2 sampling for finer boundary resolution.

**Stride**: From `sizeof(Uint32) = 4` bytes up to a maximum detected via convergence (doubles until timing change < 10%), then swept downward.

For each point, the pointer-chain is rebuilt with the current range and stride, the kernel is run with adaptive calibration, and the result (ns/iteration) is stored in a result matrix `rawMatrix[rangeIdx][strideIdx]`.

### 10.2 Cache Boundary Detection — Hybrid Algorithm

Resonance uses a **hybrid detection algorithm** combining the formal correctness of X-Ray with the practical coverage of Calibrator's plateau/jump detector:

**Primary: X-Ray Compact/Non-Compact Binary Search**

For each candidate stride `S`, determine whether a sequence of `N` accesses is compact (all cache hits):

```
is_compact(S, N):
  build chain of N elements at stride S
  measure average access time
  return avg_time < l_hit_threshold * 1.05  // 5% tolerance

detect_cache_level():
  S = 1; N = 1
  // Phase 1: find non-compact N at S=1
  while is_compact(S, N): N *= 2
  // Phase 2: double S, binary-search for smallest non-compact N
  loop:
    S *= 2
    N_old = N
    N = binary_search(1, N_old, n => !is_compact(S, n))
    if N == N_old: break
  return { associativity: N-1, capacity: (S/2) * (N-1) }
```

**Secondary: Plateau/Jump Detector (Calibrator-style)**

Applied to the full `rawMatrix` as a cross-check and for detecting additional levels that the binary search may split differently:

```
analyze_plateaus(rawMatrix):
  for each stride column:
    sliding_window = deque(maxlen=3)
    for each range row (increasing):
      sliding_window.push(rawMatrix[range][stride])
      if all values in window within EPSILON4=1.0 cycle of each other:
        extend current plateau
      elif max_window - min_window >= EPSILON4:
        record jump → candidate level boundary
```

**Merging**: Results from both detectors are merged. Candidate boundaries within `log10(latency)` difference ≤ 0.3 (i.e., latencies within ~2×) are merged into a single level. The final set of levels is sorted by cache size.

**Cache Line Size**: For each detected level, the line size is the smallest stride `s` such that the latency is still on the plateau for that level (within 10% of the plateau baseline), following Calibrator's EPSILON1 approach.

### 10.3 Output: `CacheInfo`

```typescript
interface CacheLevel {
  level: number;            // 1, 2, 3, ...
  sizeBytes: number;        // capacity in bytes
  lineSizeBytes: number;    // cache line size in bytes
  associativity: number;    // set associativity (from X-Ray algorithm)
  missLatencyNs: number;    // miss penalty in nanoseconds
  missLatencyCycles: number; // miss penalty in cycles (if CPU freq known)
  replacementTimeNs: number; // throughput-pass time (ns/access)
}

interface CacheInfo {
  levels: CacheLevel[];
  detectionMethod: 'xray' | 'calibrator' | 'hybrid';
  confidence: 'high' | 'medium' | 'low';
}
```

---

## 11. TLB Detection

### 11.1 TLB Sweep

TLB detection sweeps over `(spots, stride)` pairs where:
- `stride = cacheLineSize + shift` — each access hits a different cache line AND a different page
- `spots = range / stride` — number of distinct TLB entries touched

The stride is chosen such that consecutive accesses fall on different physical pages, ensuring TLB misses are triggered as the number of spots grows.

For each spot count, the pointer chain spans exactly `spots` distinct pages. The measured latency is compared against the in-cache baseline (from the L1 latency measurement):

```
tlb_miss_latency(spots):
  measure latency with chain spanning `spots` pages
  return measurement - l1_latency_baseline
```

### 11.2 TLB Boundary Detection

A ratio-based threshold detector (analogous to lmbench's THRESHOLD = 1.15) identifies TLB level boundaries:

```
find_tlb_levels(results):
  for each pair of consecutive spot counts (left, right):
    ratio = results[right] / results[left]
    if ratio > TLB_THRESHOLD (1.15):
      mark boundary; previous spot count = TLB entries at this level
```

Binary search refines the boundary: exponential search to bracket, then binary search to find exact entry count.

### 11.3 Page Size

The default page size is assumed to be 4096 bytes (4 KiB), as browsers do not expose `getpagesize()`. If `SharedArrayBuffer` is available, a page-size probing experiment can detect 2 MiB transparent huge pages by comparing TLB hit latency profiles at strides of 4 KiB vs 2 MiB.

### 11.4 Output: `TLBInfo`

```typescript
interface TLBLevel {
  level: number;
  entries: number;
  pageSizeBytes: number;
  missLatencyNs: number;
  missLatencyCycles: number;
}

interface TLBInfo {
  levels: TLBLevel[];
}
```

---

## 12. Bandwidth Measurement

### 12.1 Wasm SIMD Kernels

Bandwidth is measured using WebAssembly SIMD (128-bit `v128`). The following variants are measured:

| Variant | Description | Wasm Instructions |
|---------|-------------|-------------------|
| `sequential_read` | Forward sequential read, 128-bit loads | `v128.load` |
| `sequential_write` | Forward sequential write, constant fill | `v128.store` |
| `sequential_copy` | src → dst sequential copy | `v128.load` + `v128.store` |
| `sequential_fill` | Fill with zero value | `v128.store` |
| `strided_read` | Every 64 bytes (one cache line step) | `v128.load` |
| `strided_write` | Every 64 bytes | `v128.store` |
| `random_read` | LCG-addressed reads (bandwidth under random access) | `v128.load` |

Each variant is run at multiple buffer sizes (from L1 through main memory) to produce a bandwidth-vs-working-set profile.

Fallback C-equivalent scalar Wasm kernels are provided for browsers without SIMD support.

### 12.2 Measurement Methodology

Following tinymembench:
- **Adaptive timing**: iterate until elapsed ≥ `MINBW_TIME` (0.5 seconds minimum)
- **Statistical convergence**: after ≥ 3 samples, if standard deviation < 0.1% of maximum → early termination
- **Peak selection**: report the maximum bandwidth observed across all trials (noise only reduces, never increases bandwidth)
- **MAXREPEATS = 10**

Bandwidth formula:
```
bandwidth_GB_s = (bytes_transferred / elapsed_s) / 1e9
```

### 12.3 Two-Pass Mode

For cache-level bandwidth (testing L1/L2 throughput without memory controller involvement), a 2-pass approach is used: data is transferred in 2 KiB chunks through a small temporary buffer that fits entirely in L1. This isolates cache bandwidth from DRAM bandwidth.

### 12.4 Output: `BandwidthResults`

```typescript
interface BandwidthPoint {
  bufferSizeBytes: number;
  bandwidthGBs: number;
  variant: string;
}

interface BandwidthResults {
  points: BandwidthPoint[];
  peakReadGBs: number;
  peakWriteGBs: number;
  peakCopyGBs: number;
}
```

---

## 13. Memory-Level Parallelism (MLP) Measurement

### 13.1 Method

MLP is measured by running multiple independent pointer-chasing chains simultaneously and measuring aggregate throughput. Following lmbench's approach, 1 through 16 chains are tested.

Each chain `k` uses a different word offset within each cache line (as in lmbench's `mem_benchmark_k` variants), preventing aliasing and ensuring each chain independently exercises the memory system.

For `K` chains:
- Each chain traverses the same `range` with the same `stride`
- All chains run concurrently in the same Wasm function (unrolled K times)
- Aggregate throughput = `(K × accesses) / elapsed`

### 13.2 MLP Derivation

```
mlp_factor(K):
  throughput_K = measure_K_chains(K)
  throughput_1 = measure_K_chains(1)
  return throughput_K / throughput_1
```

MLP saturates when adding another chain no longer increases throughput proportionally. The saturation point indicates the hardware's outstanding request buffer depth.

### 13.3 Output: `MLPResults`

```typescript
interface MLPResults {
  measurements: Array<{ chains: number; throughputRelative: number }>;
  estimatedMLP: number;  // chain count at saturation
}
```

---

## 14. CPU Frequency Detection

Following X-Ray's approach: a chain of dependent integer additions executes at approximately 1 addition per cycle on most architectures. Running a known number of additions and dividing by elapsed time yields an estimate of CPU frequency.

```
estimate_cpu_frequency():
  N = 10,000,000
  seed = 1
  t0 = readTimer()
  for i in 1..N:
    seed = seed + seed  // dependent additions — 1/cycle
  t1 = readTimer()
  return N / (t1 - t0)  // cycles/ns = GHz
```

Caveat: JIT compilers may optimize dependent addition chains. The Wasm implementation avoids this — the chain is implemented as a Wasm loop with explicit `local.get`/`local.set`, which the Wasm runtime cannot reorder.

The frequency estimate is used for converting ns measurements to cycle counts throughout all results.

---

## 15. Analysis Pipeline

### 15.1 Raw Data → Structured Results

The analysis pipeline runs entirely in the Orchestrator (main thread), after raw timing data is received from the Measurement Worker. No analysis is performed during timing-critical paths.

```
rawCacheMatrix: number[][]     → analyzeCache()  → CacheInfo
rawTLBMatrix:   number[][]     → analyzeTLB()    → TLBInfo
rawBwPoints:    BandwidthPoint[] → (direct)      → BandwidthResults
rawMlpPoints:   number[]       → analyzeMLP()    → MLPResults
cpuFreqHz:      number         → (direct)        → used in all ns→cycle conversions
```

### 15.2 Confidence Scoring

Each result is annotated with a confidence level:

| Condition | Confidence |
|-----------|-----------|
| X-Ray and Calibrator agree within 10% | `high` |
| Only one method produced a result, or methods agree within 25% | `medium` |
| Methods disagree by > 25%, or fewer than 3 trials completed | `low` |

Low-confidence results are displayed with a warning in the UI.

### 15.3 Unit Conversion

All internal timing uses nanoseconds. The UI displays:
- Cache/TLB latencies: nanoseconds (primary) and cycles (secondary, if CPU freq detected)
- Sizes: bytes, KiB, MiB as appropriate
- Bandwidth: GB/s

---

## 16. Data Models

### 16.1 Experiment Configuration

```typescript
interface ExperimentConfig {
  // Range sweep
  minRangeBytes: number;        // default: 1024
  maxRangeBytes: number;        // auto-detected or user-specified
  rangeStepsPerOctave: number;  // default: 3 (factors 0.75, 1.0, 1.25)
  
  // Timing
  minTimeMs: number;            // set to 100× timer granularity
  numTrials: number;            // default: 11 (NUMTRIES)
  
  // Analysis thresholds
  cacheThreshold: number;       // latency ratio for boundary detection (default: 1.5)
  tlbThreshold: number;         // latency ratio for TLB detection (default: 1.15)
  epsilon1: number;             // relative plateau continuity (default: 0.1)
  epsilon4: number;             // absolute jump detection in cycles (default: 1.0)
  levelMergeLogThreshold: number; // log10 latency diff to merge levels (default: 0.3)
  
  // Features
  timerMode: 'sharedArrayBuffer' | 'performanceNow';
  useWasmSIMD: boolean;
  maxMLPChains: number;         // default: 16
}
```

### 16.2 Measurement State

```typescript
interface MeasurementState {
  phase: 'idle' | 'calibrating' | 'cache' | 'tlb' | 'bandwidth' | 'mlp' | 'analyzing' | 'done' | 'error';
  progress: number;       // 0.0 to 1.0
  currentExperiment: string;
  timerGranularityNs: number;
  estimatedCpuFreqHz: number;
  rawCacheMatrix: number[][] | null;
  rawTLBMatrix: number[][] | null;
  rawBwPoints: BandwidthPoint[] | null;
  rawMlpPoints: number[] | null;
}
```

### 16.3 Full Results

```typescript
interface ResonanceResults {
  timestamp: string;
  userAgent: string;
  timerMode: string;
  timerGranularityNs: number;
  estimatedCpuFreqGHz: number;
  cache: CacheInfo;
  tlb: TLBInfo;
  bandwidth: BandwidthResults;
  mlp: MLPResults;
  durationMs: number;
}
```

---

## 17. User Interface

### 17.1 Layout

The UI is structured as a single-page application with three main sections:

**Header** — Application name, brief description, and a "Run Benchmark" button. A secondary "Advanced Settings" toggle reveals the configuration panel.

**Progress Panel** — Shown during measurement. Displays current phase, progress bar (0–100%), and an activity log of completed steps.

**Results Panel** — Shown after measurement completes. Structured into four tabs:

1. **Cache** — Table of cache levels (size, line size, associativity, latency ns, latency cycles). A latency-vs-working-set chart (log-log scale) shows the step function with level boundaries annotated.
2. **TLB** — Table of TLB levels (entries, page size, miss latency).
3. **Bandwidth** — Table of peak bandwidths per variant. A bandwidth-vs-working-set chart shows the profile from L1 through DRAM.
4. **MLP** — Chart of relative throughput vs chain count, with saturation point annotated.

An **Export** button serializes the `ResonanceResults` object to JSON for download.

### 17.2 Configuration Panel

Exposed via "Advanced Settings":
- Maximum memory range (auto / custom value in MiB)
- Number of trials per measurement (3 / 7 / 11)
- Enable/disable Wasm SIMD bandwidth kernels
- Timer mode indicator (read-only: shows whether SharedArrayBuffer is available)
- Reset to defaults button

### 17.3 Warnings and Confidence Indicators

Low-confidence results display an inline warning icon with a tooltip explaining the disagreement between detection methods. If the timer granularity is > 100µs (indicative of heavy Spectre mitigation), a top-level banner warns that results may be less accurate and recommends serving the page with COOP/COEP headers.

---

## 18. Deployment Requirements

### 18.1 Required HTTP Headers for Full Functionality

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

These headers enable `SharedArrayBuffer`, which is required for the high-resolution counter thread. Without them, the application falls back to `performance.now()` with degraded timer resolution.

### 18.2 Static Hosting

Resonance has no server-side computation requirements. All measurement and analysis logic runs in the browser. The application can be hosted as static files on any HTTP server, CDN, or GitHub Pages — provided the COOP/COEP headers are configurable.

### 18.3 Browser Compatibility

| Feature | Minimum Browser Version |
|---------|------------------------|
| Web Workers | Chrome 4, Firefox 3.5, Safari 4 |
| WebAssembly | Chrome 57, Firefox 52, Safari 11 |
| SharedArrayBuffer | Chrome 92, Firefox 79, Safari 15.2 (requires COOP/COEP) |
| Wasm SIMD | Chrome 91, Firefox 89, Safari 16.4 |
| `performance.now()` | Chrome 21, Firefox 15, Safari 8 |

Minimum supported: Chrome 91, Firefox 89, Safari 16.4 for full functionality. Older browsers fall back gracefully (no SIMD, no SharedArrayBuffer timer).

---

## 19. Limitations and Known Constraints

| Limitation | Root Cause | Mitigation |
|-----------|-----------|-----------|
| No CPU pinning | Browser sandbox | Run on a quiet system; results averaged across migrations |
| No `mlockall()` | Browser sandbox | Pre-touch all buffers; measure in burst to minimize GC interference |
| No hugepage control | Browser sandbox | Assume 4 KiB pages; note in results |
| Timer jitter (Spectre mitigations) | Browser security | SharedArrayBuffer counter thread; warn if granularity > 100µs |
| JIT non-determinism | JavaScript semantics | Wasm kernels for critical paths; result sinking anti-optimization |
| No ISA assembly | Browser sandbox | Wasm SIMD (128-bit); scalar fallback |
| No NUMA awareness | Browser sandbox | Document that results reflect current socket only |
| No frequency scaling control | Browser sandbox | Auto-detect frequency; warn if frequency appears to be scaling |
| Background tab throttling | Browser policy | Detect throttling; pause and retry if timing intervals are disrupted |

---

## 20. Key Algorithmic Parameters

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| `MINRANGE` | 1024 bytes | Below this, L1 latency is unresolvable |
| `NUMLOADS` | 100,000 | 100 unrolled × 1000 outer iterations per trial |
| `NUMTRIES` | 11 | Best-of-N minimum selection (lmbench TRIES) |
| `LENPLATEAU` | 3 | Consecutive readings to confirm plateau |
| `EPSILON1` | 0.10 (10% relative) | Plateau continuity across strides |
| `EPSILON4` | 1.0 cycle absolute | Jump detection threshold |
| `CACHE_THRESHOLD` | 1.50 (latency ratio) | Cache boundary detection (lmbench) |
| `TLB_THRESHOLD` | 1.15 (latency ratio) | TLB miss detection (lmbench) |
| `LEVEL_MERGE_LOG` | 0.30 (log₁₀ diff) | Merge adjacent levels if latencies within ~2× |
| `MINBW_TIME` | 500 ms | Minimum bandwidth trial duration (tinymembench) |
| `BW_MAXREPEATS` | 10 | Maximum bandwidth trial repetitions |
| `BW_CONVERGENCE` | 0.001 (0.1% std/max) | Early termination threshold (tinymembench) |
| `MAX_MLP_CHAINS` | 16 | Maximum independent chains for MLP (lmbench) |
| `ALIGN_PADDING` | 1 MiB | Anti-aliased buffer allocation padding |

---

## 21. Glossary

| Term | Definition |
|------|-----------|
| Cache line | Smallest unit of data transfer between cache levels (typically 32–128 bytes) |
| Cache miss | Access that cannot be served from cache; data fetched from a slower level |
| Compact sequence | X-Ray term: access sequence where every access is a cache hit |
| Non-compact sequence | X-Ray term: access sequence where every access is a cache miss |
| Fisher-Yates shuffle | Unbiased in-place random permutation; used for inter-page ordering |
| Bit-reversal permutation | Maps index i to its bit-reversed value; used for intra-page ordering |
| LCG | Linear Congruential Generator: `seed = seed × A + C mod 2³²` |
| MLP | Memory-Level Parallelism: number of outstanding memory requests hardware can service simultaneously |
| Plateau | Stable latency region as working set grows → data still fits in one cache level |
| Jump | Abrupt latency increase as working set exceeds a cache level boundary |
| Pointer chasing | Traversing a linked list where each node's value is the address of the next node; creates serialized, dependent memory accesses that cannot be parallelized or prefetched away |
| Spots | Number of distinct memory locations accessed in a TLB sweep (`range / stride`) |
| Stride | Step size in bytes between consecutively accessed addresses |
| TLB | Translation Lookaside Buffer: hardware cache for virtual-to-physical address translations |
| Working set | Total amount of data accessed in a measurement pass; determines which cache level is exercised |
| Anti-aliased buffers | Buffers at complementary bit-pattern offsets that map to maximally different cache sets |
| Two-pass measurement | Running a throughput pass and a latency pass; the difference isolates pure miss penalty |
| SharedArrayBuffer | Browser API for shared memory between main thread and Web Workers |
| COOP/COEP | Cross-Origin-Opener-Policy / Cross-Origin-Embedder-Policy HTTP headers required for SharedArrayBuffer |
| Wasm SIMD | WebAssembly 128-bit SIMD extension (`v128` type) |
