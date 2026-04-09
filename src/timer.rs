/// Timing infrastructure (DESIGN.md §8).
///
/// Provides timer-granularity calibration, adaptive iteration calibration,
/// and best-of-N trial selection.
use crate::constants;
use crate::platform;

// ---------------------------------------------------------------------------
// Timer granularity calibration (§8.1)
// ---------------------------------------------------------------------------

/// Empirically measure the clock granularity by timing the smallest
/// observable tick.
///
/// Returns the 5th-percentile delta in nanoseconds — robust to OS noise but
/// not sensitive to outliers.
pub fn measure_granularity() -> f64 {
    let n = constants::GRANULARITY_SAMPLES;
    let mut deltas: Vec<u64> = Vec::with_capacity(n);

    for _ in 0..n {
        let t0 = platform::clock_ns();
        let mut t1 = platform::clock_ns();
        while t1 == t0 {
            t1 = platform::clock_ns();
        }
        deltas.push(t1 - t0);
    }

    deltas.sort_unstable();
    // 5th percentile.
    deltas[n / 20] as f64
}

/// Compute `MINTIME_NS` from the measured granularity.
///
/// `MINTIME = max(1 ms, granularity × 100)`.
pub fn compute_mintime_ns(granularity_ns: f64) -> u64 {
    let scaled = (granularity_ns * constants::GRANULARITY_MULTIPLIER as f64).ceil() as u64;
    scaled.max(constants::MIN_MINTIME_NS)
}

// ---------------------------------------------------------------------------
// Adaptive iteration calibration (§8.2)
// ---------------------------------------------------------------------------

/// Calibrate iteration count and run a single timing trial.
///
/// `kernel(iters)` should execute the measurement workload for `iters`
/// outer-loop iterations and return the total elapsed nanoseconds.
///
/// Returns `(ns_per_iter, iters_used)` — the per-iteration time and the
/// iteration count that was sufficient to meet `min_time_ns`.
pub fn calibrate_and_run<F>(kernel: &F, min_time_ns: u64) -> (f64, u64)
where
    F: Fn(u64) -> u64,
{
    let mut iters: u64 = 1;
    loop {
        let elapsed = kernel(iters);
        if elapsed >= min_time_ns {
            return (elapsed as f64 / iters as f64, iters);
        }
        // Scale up iterations proportionally with 10 % headroom.
        if elapsed == 0 {
            iters *= 8;
        } else {
            let scale = (min_time_ns as f64 / elapsed as f64 * 1.1).ceil() as u64;
            iters = if scale >= 8 {
                iters.saturating_mul(8)
            } else {
                iters.saturating_mul(scale)
            };
        }
    }
}

// ---------------------------------------------------------------------------
// Best-of-N trial selection (§8.3)
// ---------------------------------------------------------------------------

/// Run `n_trials` independent calibrated trials and return the **minimum**
/// ns-per-iteration.
///
/// Noise only inflates latency; it never has the opposite effect.  Taking
/// the minimum gives the best estimate of the true hardware latency.
pub fn best_of_n<F>(kernel: &F, n_trials: usize, min_time_ns: u64) -> f64
where
    F: Fn(u64) -> u64,
{
    let mut best = f64::INFINITY;
    for _ in 0..n_trials {
        let (ns_per_iter, _) = calibrate_and_run(kernel, min_time_ns);
        if ns_per_iter < best {
            best = ns_per_iter;
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Granularity should be positive and < 1 µs on any modern kernel.
    #[test]
    fn granularity_is_reasonable() {
        let g = measure_granularity();
        assert!(g > 0.0, "granularity must be positive");
        assert!(
            g < 1_000.0,
            "granularity {g} ns ≥ 1 µs — unexpectedly coarse"
        );
    }

    /// MINTIME should be at least 1 ms.
    #[test]
    fn mintime_floor() {
        let mintime = compute_mintime_ns(1.0);
        assert!(mintime >= constants::MIN_MINTIME_NS);
    }

    /// calibrate_and_run should converge for a trivial kernel.
    #[test]
    fn calibrate_trivial_kernel() {
        // Kernel: just burn time with a volatile-style loop.
        let kernel = |iters: u64| -> u64 {
            let t0 = platform::clock_ns();
            let mut acc: u64 = 0;
            for i in 0..iters * 100 {
                acc = acc.wrapping_add(i);
            }
            std::hint::black_box(acc);
            platform::clock_ns() - t0
        };
        let (ns_per_iter, used) = calibrate_and_run(&kernel, 1_000_000);
        assert!(ns_per_iter > 0.0, "per-iter time must be positive");
        assert!(used >= 1, "must have run at least 1 iteration");
    }

    /// best_of_n should return a finite, positive result.
    #[test]
    fn best_of_n_smoke() {
        let kernel = |iters: u64| -> u64 {
            let t0 = platform::clock_ns();
            let mut acc: u64 = 0;
            for i in 0..iters * 100 {
                acc = acc.wrapping_add(i);
            }
            std::hint::black_box(acc);
            platform::clock_ns() - t0
        };
        let result = best_of_n(&kernel, 3, 1_000_000);
        assert!(result.is_finite() && result > 0.0);
    }
}
