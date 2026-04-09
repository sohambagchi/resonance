# Initial Scaffold and Oracle Testing Strategy

Date: 2026-04-08
Status: Accepted

## Context

Resonance is a new project with no existing code.  The DESIGN.md specifies a
large surface area (platform layer, buffer management, timing infrastructure,
measurement kernels, analysis pipeline, CLI).  We need to decide:

1. What to implement first.
2. How to test correctness as we build incrementally.
3. Which platforms to target initially.

## Decision

### Implementation order

We implement bottom-up, starting with the foundational layers everything else
depends on:

1. Platform layer (Linux x86-64 only)
2. Buffer management (AlignedBuffer, chain construction)
3. Timing infrastructure (granularity calibration, adaptive iteration)
4. Measurement kernels (pointer-chase, LCG)
5. Architecture-specific kernels (x86-64 inline asm for CPU freq + AVX2)
6. Result data model
7. Orchestrator skeleton
8. CLI

The analysis pipeline (cache/TLB detection algorithms, bandwidth/MLP analysis)
is deferred to a subsequent phase — it depends on all of the above being
correct first.

### Platform scope

Linux x86-64 on Intel CPUs only for this phase.  macOS, Windows, and AArch64
support will be added later.  The code is structured with `cfg(target_os)` and
`cfg(target_arch)` gates so platform extensions require no refactoring.

### Testing strategy — Oracle-based validation

Linux exposes the full cache hierarchy through sysfs:

    /sys/devices/system/cpu/cpu0/cache/index{0,1,2,3}/
        level, type, size, coherency_line_size, ways_of_associativity

We read these values as **ground truth** and compare our measurements against
them.  This gives us a reliable, automated way to verify that:

- The pointer-chase kernel measures L1 latency < 10 ns
- L2-range latency is measurably higher than L1
- CPU frequency estimation is within 30% of the OS-reported value
- Timer granularity is < 1 µs

This "oracle" approach scales as we add analysis: once the cache detection
pipeline is implemented, we can assert that detected L1/L2/L3 sizes match
sysfs within a tolerance.

### Unit tests

Pure-logic components (PRNG, bit reversal, chain construction, anti-aliased
layout computation) have conventional unit tests with known inputs/outputs.

## Consequences

- The project compiles and passes tests from day one, even before the analysis
  pipeline exists.
- Oracle tests run on real hardware only (not in cross-compilation or
  containerised CI without `/sys`).  They are skipped gracefully when sysfs is
  unavailable.
- The deferred analysis pipeline is the next major work item.
