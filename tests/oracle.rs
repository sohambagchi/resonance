/// Oracle-based integration tests.
///
/// Read ground-truth cache parameters from Linux sysfs and validate that
/// the measurement infrastructure produces consistent, plausible results.
use resonance::buffer::{build_chain, pre_touch, AlignedBuffer};
use resonance::kernels::latency;
use resonance::oracle;
use resonance::rng::Xoshiro256StarStar;
use resonance::timer;

// ---------------------------------------------------------------------------
// Oracle sanity — verify that the oracle itself reads plausible values.
// ---------------------------------------------------------------------------

#[test]
fn oracle_caches_present() {
    let caches = oracle::read_sysfs_data_caches();
    assert!(
        !caches.is_empty(),
        "oracle should find at least one data/unified cache"
    );
}

#[test]
fn oracle_l1d_exists() {
    let caches = oracle::read_sysfs_data_caches();
    let l1 = caches.iter().find(|c| c.level == 1);
    assert!(l1.is_some(), "oracle should find an L1 data cache");
    let l1 = l1.unwrap();
    assert!(l1.size_bytes >= 8 * 1024, "L1 should be at least 8 KiB");
    assert!(l1.size_bytes <= 256 * 1024, "L1 should be at most 256 KiB");
    assert!(
        l1.line_size_bytes == 64,
        "L1 line size should be 64 B on x86-64"
    );
}

// ---------------------------------------------------------------------------
// Timer plausibility
// ---------------------------------------------------------------------------

#[test]
fn timer_granularity_under_1us() {
    let g = timer::measure_granularity();
    assert!(g > 0.0 && g < 1000.0, "granularity {g} ns out of range");
}

// ---------------------------------------------------------------------------
// CPU frequency cross-check
// ---------------------------------------------------------------------------

#[test]
fn cpu_freq_plausible() {
    let measured = resonance::arch::estimate_cpu_freq_ghz();
    assert!(
        measured > 0.5 && measured < 8.0,
        "measured freq {measured} GHz implausible"
    );

    if let Some(os_ghz) = oracle::read_sysfs_cpu_freq_ghz() {
        // Allow 30% divergence (frequency scaling, thermal throttling).
        let ratio = (measured - os_ghz).abs() / os_ghz;
        assert!(
            ratio < 0.50,
            "measured {measured:.2} GHz vs OS {os_ghz:.2} GHz diverge by {:.0}%",
            ratio * 100.0
        );
    }
}

// ---------------------------------------------------------------------------
// Pointer-chase latency — L1 should be fast
// ---------------------------------------------------------------------------

#[test]
fn pointer_chase_l1_is_fast() {
    let caches = oracle::read_sysfs_data_caches();
    let l1 = match caches.iter().find(|c| c.level == 1) {
        Some(c) => c,
        None => return, // cannot test without oracle
    };

    // Working set = L1 size / 2 — should fit comfortably in L1.
    let range = (l1.size_bytes as usize) / 2;
    let line = l1.line_size_bytes as usize;
    let page = resonance::platform::page_size();

    let mut buf = AlignedBuffer::new(range, page).expect("alloc");
    pre_touch(buf.as_mut_slice());
    {
        let chain = buf.as_usize_mut_slice();
        let mut rng = Xoshiro256StarStar::new(42);
        build_chain(chain, range, line, page, &mut rng);
    }

    let chain = buf.as_usize_slice();
    let mintime_ns = timer::compute_mintime_ns(timer::measure_granularity());

    // Best-of-3 (faster for CI) — ns per 100 dereferences.
    let ns_per_100 = timer::best_of_n(&|iters| latency::pointer_chase(chain, iters), 3, mintime_ns);
    let ns_per_access = ns_per_100 / latency::unroll_factor() as f64;

    // L1 hit latency should be < 10 ns on any modern x86-64.
    assert!(
        ns_per_access < 10.0,
        "L1 latency {ns_per_access:.2} ns is unexpectedly high"
    );
}

// ---------------------------------------------------------------------------
// Pointer-chase latency — L2 should be slower than L1
// ---------------------------------------------------------------------------

#[test]
fn pointer_chase_l2_slower_than_l1() {
    let caches = oracle::read_sysfs_data_caches();
    let l1 = caches.iter().find(|c| c.level == 1);
    let l2 = caches.iter().find(|c| c.level == 2);
    let (l1, l2) = match (l1, l2) {
        (Some(a), Some(b)) => (a, b),
        _ => return,
    };

    // Pin to core 0 to reduce scheduling noise.
    let _ = resonance::platform::pin_thread_to_core(0);

    let line = l1.line_size_bytes as usize;
    let page = resonance::platform::page_size();
    let mintime_ns = timer::compute_mintime_ns(timer::measure_granularity());

    let measure = |range: usize| -> f64 {
        let mut buf = AlignedBuffer::new(range, page).expect("alloc");
        pre_touch(buf.as_mut_slice());
        {
            let chain = buf.as_usize_mut_slice();
            let mut rng = Xoshiro256StarStar::new(42);
            build_chain(chain, range, line, page, &mut rng);
        }
        let chain = buf.as_usize_slice();
        let ns_per_100 =
            timer::best_of_n(&|iters| latency::pointer_chase(chain, iters), 5, mintime_ns);
        ns_per_100 / latency::unroll_factor() as f64
    };

    // Half of L1 → should be an L1 hit.
    let l1_ns = measure(l1.size_bytes as usize / 2);

    // 2× L2 size → should spill out of L2 into L3, showing clearly higher
    // latency.  Cap at 64 MiB to avoid excessive memory use.
    let spill_range = ((l2.size_bytes as usize) * 2).min(64 * 1024 * 1024);
    let spill_ns = measure(spill_range);

    assert!(
        spill_ns > l1_ns * 1.2,
        "L3-spill range latency ({spill_ns:.2} ns) should be > 1.2× L1 latency ({l1_ns:.2} ns)"
    );
}
