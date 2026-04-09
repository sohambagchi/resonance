# Stride-Based Chain for 2D Sweep

Date: 2026-04-08
Status: Accepted

## Context

The cache latency 2D sweep (DESIGN.md §11.1) generates range values at 3
points per power-of-2 octave (scale factors 0.75×, 1.0×, 1.25×). This
produces non-power-of-2 ranges like 768, 1280, 1536, 2560, 3072, 5120 bytes.

The existing `build_chain` function uses a bit-reversal permutation for
intra-page line ordering, which requires power-of-2 counts. For example,
range=768 with stride=64 gives n_lines=12, and `bit_reverse(i,
trailing_zeros(12))` produces duplicates because `trailing_zeros(12)=2` yields
only 4 distinct values for 12 indices.

The sweep also needs a different chain structure: one element per stride-sized
region (not every word), so the number of active cache lines equals
`range / max(stride, cache_line_size)` — directly controlling the cache
footprint for measurement.

## Decision

Introduced `build_stride_chain` — a simpler chain builder specifically for
the 2D sweep. It:

1. Places one chain position per stride-sized region: position `i` lives at
   byte offset `i * stride`.
2. Shuffles positions via Fisher-Yates (no bit-reversal needed).
3. Rotates so position 0 is first, then wires a circular linked list.

This naturally handles any range/stride combination. The existing
`build_chain` (3-layer bit-reversal) is preserved for other uses where
visiting every word at cache-line granularity matters.

The stride in the sweep directly controls inter-access spacing:
- When stride < cache_line: multiple positions share cache lines.
- When stride ≥ cache_line: each position occupies a distinct line.
- The transition from shared to separate reveals the cache line size.

Stride is capped at page_size (beyond that, TLB effects dominate).

## Consequences

- `build_stride_chain` is simpler and more general than `build_chain` for
  the sweep use case. No power-of-2 constraint on range or stride count.
- The 2D sweep matrix (`SweepMatrix`) stores ns/access for each
  (range, stride) pair, indexed as row-major `[range_idx][stride_idx]`.
- Early stride termination (< 10% change) limits unnecessary measurements.
- The raw matrix is available for boundary detection (§11.2, next phase).
- `build_chain` remains for scenarios requiring full word-level chain
  construction (e.g., latency-pass and MLP multi-chain).
