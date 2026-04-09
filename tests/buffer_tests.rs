/// Buffer management unit tests (external integration test form).
///
/// These test chain construction, anti-aliased layout computation, and
/// allocation correctness.
use resonance::buffer::*;
use resonance::rng::Xoshiro256StarStar;

// ---------------------------------------------------------------------------
// Chain construction — Hamiltonian cycle property
// ---------------------------------------------------------------------------

/// For various (range, line, page) combinations, verify that build_chain
/// produces a single Hamiltonian cycle visiting every element exactly once.
#[test]
fn chain_is_hamiltonian_cycle() {
    let page = resonance::platform::page_size();
    let ws = std::mem::size_of::<usize>();

    let test_cases: Vec<(usize, usize)> = vec![
        (1024, 64),  // range < page
        (4096, 64),  // range = page
        (8192, 64),  // 2 pages
        (16384, 64), // 4 pages
        (65536, 64), // 16 pages
        (4096, 128), // larger line size
        (4096, 32),  // smaller line size
        (1024, 8),   // line = word size
    ];

    for (range, line) in test_cases {
        let n = range / ws;
        let mut buf = vec![0usize; n];
        let mut rng = Xoshiro256StarStar::new(42);
        build_chain(&mut buf, range, line, page, &mut rng);

        let mut visited = vec![false; n];
        let mut idx = 0;
        let mut count = 0;
        loop {
            assert!(
                !visited[idx],
                "range={range} line={line}: revisits index {idx}"
            );
            visited[idx] = true;
            count += 1;
            idx = buf[idx];
            if idx == 0 {
                break;
            }
        }
        assert_eq!(
            count, n,
            "range={range} line={line}: visited {count}/{n} elements"
        );
    }
}

/// Different seeds should produce different traversal orders.
#[test]
fn chain_seed_sensitivity() {
    let range = 65536; // 16 pages — enough for Fisher-Yates to diverge
    let line = 64;
    let page = resonance::platform::page_size();
    let ws = std::mem::size_of::<usize>();
    let n = range / ws;

    let build = |seed: u64| -> Vec<usize> {
        let mut buf = vec![0usize; n];
        let mut rng = Xoshiro256StarStar::new(seed);
        build_chain(&mut buf, range, line, page, &mut rng);

        // Extract traversal order.
        let mut order = Vec::with_capacity(n);
        let mut idx = 0;
        loop {
            order.push(idx);
            idx = buf[idx];
            if idx == 0 {
                break;
            }
        }
        order
    };

    let order_a = build(42);
    let order_b = build(123);
    assert_ne!(
        order_a, order_b,
        "different seeds should produce different traversal orders"
    );
}

// ---------------------------------------------------------------------------
// Anti-aliased layout
// ---------------------------------------------------------------------------

#[test]
fn anti_aliased_layout_offsets_are_64_byte_aligned() {
    let layout = compute_anti_aliased_layout(65536);
    for (i, &off) in layout.offsets.iter().enumerate() {
        assert_eq!(off % 64, 0, "offset {i} ({off:#x}) is not 64-byte aligned");
    }
}

#[test]
fn anti_aliased_layout_fits_in_total() {
    for size in [4096, 65536, 1048576] {
        let layout = compute_anti_aliased_layout(size);
        for (i, &off) in layout.offsets.iter().enumerate() {
            assert!(
                off + layout.slice_len <= layout.total_required,
                "view {i} at offset {off} + len {} exceeds total {}",
                layout.slice_len,
                layout.total_required
            );
        }
    }
}

// ---------------------------------------------------------------------------
// AlignedBuffer allocation
// ---------------------------------------------------------------------------

#[test]
fn aligned_buffer_small() {
    let buf = AlignedBuffer::new(1024, 64).expect("small alloc");
    assert_eq!(buf.len(), 1024);
    assert_eq!(buf.as_ptr() as usize % 64, 0);
}

#[test]
fn aligned_buffer_large_uses_mmap() {
    // >= 2 MiB should use mmap path.
    let buf = AlignedBuffer::new(4 * 1024 * 1024, 4096).expect("large alloc");
    assert_eq!(buf.len(), 4 * 1024 * 1024);
}

#[test]
fn aligned_buffer_page_aligned() {
    let buf = AlignedBuffer::new_page_aligned(8192).expect("page-aligned alloc");
    let ps = resonance::platform::page_size();
    assert_eq!(buf.as_ptr() as usize % ps, 0);
}

#[test]
fn aligned_buffer_as_usize_slice() {
    let ws = std::mem::size_of::<usize>();
    let mut buf = AlignedBuffer::new(ws * 128, 64).expect("alloc");
    let slice = buf.as_usize_mut_slice();
    assert_eq!(slice.len(), 128);
    // Write and read back.
    slice[0] = 42;
    slice[127] = 99;
    assert_eq!(buf.as_usize_slice()[0], 42);
    assert_eq!(buf.as_usize_slice()[127], 99);
}
