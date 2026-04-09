/// Cache hierarchy 2D sweep (DESIGN.md §11.1).
///
/// Drives a `(range, stride)` sweep to populate a latency matrix, which is
/// then consumed by boundary-detection algorithms (§11.2, to be implemented).
use crate::buffer::{build_stride_chain, pre_touch, AlignedBuffer};
use crate::constants;
use crate::kernels::latency;
use crate::platform;
use crate::rng::Xoshiro256StarStar;
use crate::timer;

use std::fmt;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum working-set range if the user doesn't specify one.
/// `min(available_memory / 4, 128 MiB)`.
const DEFAULT_MAX_RANGE_CAP: usize = 128 * 1024 * 1024;

// ---------------------------------------------------------------------------
// SweepMatrix
// ---------------------------------------------------------------------------

/// Row-major matrix of ns-per-access measurements indexed by
/// `[range_idx][stride_idx]`.  Cells set to `NaN` were skipped.
#[derive(Clone)]
pub struct SweepMatrix {
    /// Sorted list of working-set ranges (bytes).
    pub ranges: Vec<usize>,
    /// Sorted list of strides (bytes).
    pub strides: Vec<usize>,
    /// Row-major data: `data[range_idx * strides.len() + stride_idx]`.
    data: Vec<f64>,
}

impl SweepMatrix {
    /// Create a new matrix filled with `NaN`.
    pub fn new(ranges: Vec<usize>, strides: Vec<usize>) -> Self {
        let n = ranges.len() * strides.len();
        Self {
            ranges,
            strides,
            data: vec![f64::NAN; n],
        }
    }

    /// Number of range rows.
    pub fn n_ranges(&self) -> usize {
        self.ranges.len()
    }

    /// Number of stride columns.
    pub fn n_strides(&self) -> usize {
        self.strides.len()
    }

    /// Get the value at `(range_idx, stride_idx)`.
    pub fn get(&self, range_idx: usize, stride_idx: usize) -> f64 {
        self.data[range_idx * self.strides.len() + stride_idx]
    }

    /// Set the value at `(range_idx, stride_idx)`.
    pub fn set(&mut self, range_idx: usize, stride_idx: usize, value: f64) {
        self.data[range_idx * self.strides.len() + stride_idx] = value;
    }

    /// Iterate over a column (one stride, all ranges) yielding
    /// `(range_idx, value)`.
    pub fn column(&self, stride_idx: usize) -> impl Iterator<Item = (usize, f64)> + '_ {
        let n_strides = self.strides.len();
        (0..self.ranges.len()).map(move |ri| (ri, self.data[ri * n_strides + stride_idx]))
    }
}

impl fmt::Display for SweepMatrix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Header row.
        write!(f, "{:>12}", "range\\stride")?;
        for &s in &self.strides {
            write!(f, " {:>8}", s)?;
        }
        writeln!(f)?;

        // Data rows.
        for (ri, &range) in self.ranges.iter().enumerate() {
            write!(f, "{range:>12}")?;
            for si in 0..self.strides.len() {
                let v = self.get(ri, si);
                if v.is_nan() {
                    write!(f, " {:>8}", "-")?;
                } else {
                    write!(f, " {:>8.2}", v)?;
                }
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

impl fmt::Debug for SweepMatrix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SweepMatrix {{ {} ranges × {} strides }}",
            self.ranges.len(),
            self.strides.len()
        )
    }
}

// ---------------------------------------------------------------------------
// Range / stride generation
// ---------------------------------------------------------------------------

/// Generate the sweep range schedule: 3 points per power-of-2 octave
/// (scale factors 0.75×, 1.0×, 1.25×) from `MINRANGE` to `max_range`.
///
/// All values are rounded to the nearest word-size multiple and deduped.
pub fn generate_ranges(max_range: usize) -> Vec<usize> {
    let word_size = std::mem::size_of::<usize>();
    let mut ranges = Vec::new();

    // Scale factors within each octave (0.75, 1.0, 1.25).
    let scales = [0.75_f64, 1.0, 1.25];

    // Walk powers of 2 from MINRANGE upward.
    let mut base = constants::MINRANGE;
    while base <= max_range {
        for &scale in &scales {
            let r = (base as f64 * scale) as usize;
            // Round down to word alignment.
            let r = r & !(word_size - 1);
            if r >= constants::MINRANGE && r <= max_range {
                ranges.push(r);
            }
        }
        base *= 2;
    }

    ranges.sort_unstable();
    ranges.dedup();
    ranges
}

/// Generate stride values: powers of 2 from `sizeof(usize)` to `max_stride`.
pub fn generate_strides(max_stride: usize) -> Vec<usize> {
    let word_size = std::mem::size_of::<usize>();
    let mut strides = Vec::new();
    let mut s = word_size;
    while s <= max_stride {
        strides.push(s);
        s *= 2;
    }
    strides
}

/// Compute the default `max_range`: `min(available_memory / 4, 128 MiB)`.
pub fn default_max_range() -> usize {
    let available = platform::available_memory_bytes()
        .map(|b| b as usize)
        .unwrap_or(DEFAULT_MAX_RANGE_CAP);
    (available / 4).min(DEFAULT_MAX_RANGE_CAP)
}

// ---------------------------------------------------------------------------
// 2D sweep (§11.1)
// ---------------------------------------------------------------------------

/// Perform the cache latency 2D sweep.
///
/// For each `(range, stride)` pair where `range / stride >= 2`, allocates a
/// buffer, builds a stride-based pointer-chase chain, and measures the
/// minimum ns/access over `n_trials` best-of-N trials.
///
/// Returns the populated [`SweepMatrix`].
pub fn cache_2d_sweep(
    mintime_ns: u64,
    n_trials: usize,
    max_range: usize,
    seed: u64,
) -> SweepMatrix {
    let page_size = platform::page_size();
    let ranges = generate_ranges(max_range);
    let strides = generate_strides(page_size);

    let mut matrix = SweepMatrix::new(ranges.clone(), strides.clone());
    let mut rng = Xoshiro256StarStar::new(seed);

    for (ri, &range) in ranges.iter().enumerate() {
        // Allocate and pre-touch the buffer once per range.
        let mut buf = match AlignedBuffer::new_page_aligned(range) {
            Ok(b) => b,
            Err(_) => continue, // skip if allocation fails (OOM)
        };
        pre_touch(buf.as_mut_slice());

        // Track previous latency for early stride termination.
        let mut prev_ns: Option<f64> = None;

        for (si, &stride) in strides.iter().enumerate() {
            let n_positions = range / stride;
            if n_positions < 2 {
                // Not enough positions for a meaningful chain.
                break;
            }

            // Build the stride-based chain.
            let chain = buf.as_usize_mut_slice();
            build_stride_chain(chain, range, stride, &mut rng);

            // Measure: best_of_n returns ns per outer-loop iteration
            // (which is UNROLL dereferences).
            let chain_ref = buf.as_usize_slice();
            let ns_per_unroll = timer::best_of_n(
                &|iters| latency::pointer_chase(chain_ref, iters),
                n_trials,
                mintime_ns,
            );
            let ns_per_access = ns_per_unroll / latency::unroll_factor() as f64;

            matrix.set(ri, si, ns_per_access);

            // Early termination: if stride change < EPSILON1 (10%) relative
            // to previous stride, skip remaining larger strides.
            if let Some(prev) = prev_ns {
                if prev > 0.0 {
                    let change = ((ns_per_access - prev) / prev).abs();
                    if change < constants::EPSILON1 {
                        break;
                    }
                }
            }
            prev_ns = Some(ns_per_access);
        }
    }

    matrix
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_ranges_starts_at_minrange() {
        let ranges = generate_ranges(1024 * 1024);
        assert!(!ranges.is_empty());
        // 0.75 × 1024 = 768, which is < MINRANGE=1024 and filtered out.
        // First entry should be MINRANGE itself (the 1.0× point).
        assert_eq!(*ranges.first().unwrap(), constants::MINRANGE);
    }

    #[test]
    fn generate_ranges_basic_properties() {
        let ranges = generate_ranges(1024 * 1024);
        assert!(!ranges.is_empty());
        // All values >= MINRANGE.
        for &r in &ranges {
            assert!(r >= constants::MINRANGE, "range {r} < MINRANGE");
        }
        // All values <= max_range.
        for &r in &ranges {
            assert!(r <= 1024 * 1024, "range {r} > max_range");
        }
        // Sorted and no duplicates.
        for w in ranges.windows(2) {
            assert!(w[0] < w[1], "not strictly sorted: {} >= {}", w[0], w[1]);
        }
        // Should contain exact powers of 2 within the range.
        assert!(ranges.contains(&1024));
        assert!(ranges.contains(&2048));
        assert!(ranges.contains(&4096));
        assert!(ranges.contains(&65536));
    }

    #[test]
    fn generate_ranges_three_per_octave() {
        let ranges = generate_ranges(8192);
        // For base=1024: 768 (filtered), 1024, 1280
        // For base=2048: 1536, 2048, 2560
        // For base=4096: 3072, 4096, 5120
        // For base=8192: 6144, 8192, 10240 (filtered)
        // Expected: [1024, 1280, 1536, 2048, 2560, 3072, 4096, 5120, 6144, 8192]
        assert!(ranges.contains(&1024));
        assert!(ranges.contains(&1280));
        assert!(ranges.contains(&1536));
        assert!(ranges.contains(&2048));
        assert!(ranges.contains(&8192));
    }

    #[test]
    fn generate_ranges_word_aligned() {
        let ws = std::mem::size_of::<usize>();
        let ranges = generate_ranges(1024 * 1024);
        for &r in &ranges {
            assert!(r.is_multiple_of(ws), "range {r} is not word-aligned");
        }
    }

    #[test]
    fn generate_strides_powers_of_two() {
        let strides = generate_strides(4096);
        let ws = std::mem::size_of::<usize>();
        assert_eq!(*strides.first().unwrap(), ws);
        assert_eq!(*strides.last().unwrap(), 4096);
        for &s in &strides {
            assert!(s.is_power_of_two(), "stride {s} is not a power of 2");
        }
        // Should be: 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096 (on 64-bit)
        assert!(strides.len() >= 3);
    }

    #[test]
    fn sweep_matrix_basic() {
        let ranges = vec![1024, 2048, 4096];
        let strides = vec![8, 64, 512];
        let mut m = SweepMatrix::new(ranges, strides);

        assert_eq!(m.n_ranges(), 3);
        assert_eq!(m.n_strides(), 3);

        // All NaN initially.
        assert!(m.get(0, 0).is_nan());
        assert!(m.get(2, 2).is_nan());

        // Set and get.
        m.set(1, 2, 42.5);
        assert_eq!(m.get(1, 2), 42.5);
    }

    #[test]
    fn sweep_matrix_column_iterator() {
        let ranges = vec![1024, 2048, 4096];
        let strides = vec![64, 512];
        let mut m = SweepMatrix::new(ranges, strides);

        m.set(0, 1, 1.0);
        m.set(1, 1, 2.0);
        m.set(2, 1, 3.0);

        let col: Vec<(usize, f64)> = m.column(1).collect();
        assert_eq!(col.len(), 3);
        assert_eq!(col[0], (0, 1.0));
        assert_eq!(col[1], (1, 2.0));
        assert_eq!(col[2], (2, 3.0));
    }

    #[test]
    fn sweep_matrix_display() {
        let ranges = vec![1024, 2048];
        let strides = vec![64, 512];
        let mut m = SweepMatrix::new(ranges, strides);
        m.set(0, 0, 1.23);
        m.set(1, 0, 4.56);
        // (0,1) and (1,1) stay NaN.

        let output = format!("{m}");
        assert!(output.contains("1024"));
        assert!(output.contains("2048"));
        assert!(output.contains("1.23"));
        assert!(output.contains("4.56"));
        assert!(output.contains("-")); // NaN shown as "-"
    }

    #[test]
    fn default_max_range_is_reasonable() {
        let mr = default_max_range();
        // Should be at least MINRANGE and at most 128 MiB.
        assert!(mr >= constants::MINRANGE);
        assert!(mr <= 128 * 1024 * 1024);
    }
}
