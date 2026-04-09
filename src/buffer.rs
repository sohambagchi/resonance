/// Buffer management (DESIGN.md §7).
///
/// Provides aligned, pre-touched, memory-locked buffer allocation and the
/// multi-layer pointer-chain construction that defeats hardware prefetching.
use crate::platform;
use crate::rng::Xoshiro256StarStar;

use std::fmt;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors from buffer allocation.
#[derive(Debug)]
pub enum AllocError {
    /// `posix_memalign` or `mmap` failed.
    OsError { call: &'static str, errno: i32 },
    /// The requested parameters are invalid.
    InvalidArgs(String),
}

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OsError { call, errno } => write!(f, "{call} failed (errno {errno})"),
            Self::InvalidArgs(msg) => write!(f, "invalid allocation args: {msg}"),
        }
    }
}

impl std::error::Error for AllocError {}

// ---------------------------------------------------------------------------
// Allocation method tracking
// ---------------------------------------------------------------------------

/// How the buffer was allocated — needed for correct deallocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AllocMethod {
    PosixMemalign,
    Mmap,
}

// ---------------------------------------------------------------------------
// AlignedBuffer (§7.1)
// ---------------------------------------------------------------------------

/// A page-aligned, pre-touched allocation suitable for latency/bandwidth
/// measurement.
///
/// Drop deallocates the memory using the matching free path.
pub struct AlignedBuffer {
    ptr: *mut u8,
    len: usize,
    alloc_len: usize,
    method: AllocMethod,
}

// SAFETY: The buffer owns its allocation exclusively; no aliasing.
unsafe impl Send for AlignedBuffer {}

impl AlignedBuffer {
    /// Allocate `len` bytes with alignment `align`.
    ///
    /// `align` must be a power of two and ≥ `sizeof(usize)`.
    /// For allocations ≥ `MMAP_THRESHOLD` (2 MiB) the buffer is allocated
    /// via `mmap` and `madvise(MADV_HUGEPAGE)` is attempted.
    pub fn new(len: usize, align: usize) -> Result<Self, AllocError> {
        if len == 0 {
            return Err(AllocError::InvalidArgs("len must be > 0".into()));
        }
        if !align.is_power_of_two() {
            return Err(AllocError::InvalidArgs(format!(
                "align ({align}) must be a power of 2"
            )));
        }
        let align = align.max(std::mem::size_of::<usize>());

        if len >= crate::constants::MMAP_THRESHOLD {
            Self::alloc_mmap(len, align)
        } else {
            Self::alloc_posix_memalign(len, align)
        }
    }

    /// Convenience: allocate `len` bytes aligned to the system page size.
    pub fn new_page_aligned(len: usize) -> Result<Self, AllocError> {
        Self::new(len, platform::page_size())
    }

    // -- posix_memalign path --------------------------------------------------

    fn alloc_posix_memalign(len: usize, align: usize) -> Result<Self, AllocError> {
        let mut ptr: *mut libc::c_void = std::ptr::null_mut();
        // SAFETY: posix_memalign is a standard POSIX call.  `align` is
        // validated above.
        let ret = unsafe { libc::posix_memalign(&mut ptr, align, len) };
        if ret != 0 {
            return Err(AllocError::OsError {
                call: "posix_memalign",
                errno: ret,
            });
        }
        Ok(Self {
            ptr: ptr as *mut u8,
            len,
            alloc_len: len,
            method: AllocMethod::PosixMemalign,
        })
    }

    // -- mmap path ------------------------------------------------------------

    fn alloc_mmap(len: usize, _align: usize) -> Result<Self, AllocError> {
        // Round up to page boundary.
        let page = platform::page_size();
        let alloc_len = (len + page - 1) & !(page - 1);

        // SAFETY: MAP_ANONYMOUS | MAP_PRIVATE is well-defined for any
        // non-negative length.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                alloc_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_ANONYMOUS | libc::MAP_PRIVATE,
                -1,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            let errno = unsafe { *libc::__errno_location() };
            return Err(AllocError::OsError {
                call: "mmap",
                errno,
            });
        }

        // Best-effort: request transparent huge pages.
        unsafe {
            libc::madvise(ptr, alloc_len, libc::MADV_HUGEPAGE);
        }

        Ok(Self {
            ptr: ptr as *mut u8,
            len,
            alloc_len,
            method: AllocMethod::Mmap,
        })
    }

    // -- accessors ------------------------------------------------------------

    /// Length in bytes (as requested, not the rounded-up allocation size).
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the buffer is zero-length (should never happen after
    /// a successful `new`).
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Raw pointer to the start of the buffer.
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr
    }

    /// Mutable raw pointer.
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr
    }

    /// Borrow the buffer as a byte slice.
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: the allocation is valid for `self.len` bytes and we hold
        // an immutable borrow.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    /// Borrow the buffer as a mutable byte slice.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: same as above, with exclusive borrow.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }

    /// Reinterpret the buffer as a mutable slice of `usize` (word-sized)
    /// elements.
    ///
    /// # Panics
    ///
    /// Panics if `self.len` is not a multiple of `size_of::<usize>()`.
    pub fn as_usize_mut_slice(&mut self) -> &mut [usize] {
        let ws = std::mem::size_of::<usize>();
        assert!(
            self.len.is_multiple_of(ws),
            "buffer length {} is not a multiple of word size {ws}",
            self.len
        );
        // SAFETY: posix_memalign / mmap return suitably aligned pointers;
        // `usize` alignment ≤ page alignment.
        unsafe { std::slice::from_raw_parts_mut(self.ptr as *mut usize, self.len / ws) }
    }

    /// Reinterpret the buffer as an immutable slice of `usize`.
    pub fn as_usize_slice(&self) -> &[usize] {
        let ws = std::mem::size_of::<usize>();
        assert!(
            self.len.is_multiple_of(ws),
            "buffer length {} is not a multiple of word size {ws}",
            self.len
        );
        unsafe { std::slice::from_raw_parts(self.ptr as *const usize, self.len / ws) }
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        // SAFETY: each deallocation path matches its allocation path exactly.
        unsafe {
            match self.method {
                AllocMethod::PosixMemalign => {
                    libc::free(self.ptr as *mut libc::c_void);
                }
                AllocMethod::Mmap => {
                    libc::munmap(self.ptr as *mut libc::c_void, self.alloc_len);
                }
            }
        }
    }
}

impl fmt::Debug for AlignedBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlignedBuffer")
            .field("ptr", &self.ptr)
            .field("len", &self.len)
            .field("alloc_len", &self.alloc_len)
            .field("method", &self.method)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Pre-touch (§7.3)
// ---------------------------------------------------------------------------

/// Write one byte per page to commit all pages into physical RAM.
///
/// Combined with `mlockall` this ensures no page-fault latency occurs during
/// measurement.
pub fn pre_touch(buf: &mut [u8]) {
    let ps = platform::page_size();
    let mut i = 0;
    while i < buf.len() {
        buf[i] = 0;
        i += ps;
    }
    // Touch the last byte if not already covered.
    if let Some(last) = buf.last_mut() {
        *last = 0;
    }
}

// ---------------------------------------------------------------------------
// Anti-aliased buffer layout (§7.2)
// ---------------------------------------------------------------------------

/// Computed offsets for four non-overlapping sub-views within a single
/// backing buffer, placed at cache-set-diverse offsets.
#[derive(Debug, Clone)]
pub struct AntiAliasedLayout {
    /// Byte offsets of each view within the backing buffer.
    pub offsets: [usize; 4],
    /// Usable byte length of each view.
    pub slice_len: usize,
    /// Minimum total bytes the backing buffer must have.
    pub total_required: usize,
}

/// Compute anti-aliased offsets for four sub-views of `slice_len` bytes.
///
/// The offsets ensure that bits 6–11 of each view's start address (the L1
/// cache-set index bits for 64-byte lines) are maximally different,
/// preventing the views from evicting each other during copy benchmarks.
pub fn compute_anti_aliased_layout(slice_len: usize) -> AntiAliasedLayout {
    let ps = platform::page_size();
    // Round slice_len up to a page boundary.
    let aligned_len = (slice_len + ps - 1) & !(ps - 1);

    // Extra byte offsets that place each view at a different L1 cache-set
    // index.  Derived from the complementary bit-pattern idea in DESIGN.md
    // §7.2 / tinymembench.
    //
    //   bits 6-11 for offset 0x000 → 000000
    //   bits 6-11 for offset 0xA80 → 101010  (0x2A << 6)
    //   bits 6-11 for offset 0x540 → 010101  (0x15 << 6)
    //   bits 6-11 for offset 0xCC0 → 110011  (0x33 << 6)
    //
    // All values are 64-byte aligned (required for AVX2/AVX-512).
    const EXTRAS: [usize; 4] = [0x000, 0xA80, 0x540, 0xCC0];

    // Guarantee that no two views overlap: stride between view start
    // addresses is aligned_len + max(EXTRAS) + page-alignment slop.
    let max_extra = *EXTRAS.iter().max().unwrap();
    let stride = aligned_len + max_extra + ps;

    let offsets = [
        EXTRAS[0],
        stride + EXTRAS[1],
        2 * stride + EXTRAS[2],
        3 * stride + EXTRAS[3],
    ];
    let total_required = offsets[3] + slice_len;

    AntiAliasedLayout {
        offsets,
        slice_len,
        total_required,
    }
}

// ---------------------------------------------------------------------------
// Pointer-chain construction (§7.4)
// ---------------------------------------------------------------------------

/// Build a circular pointer-chase chain in `buf` using the three-layer
/// permutation from lmbench (Fisher-Yates inter-page + bit-reversal
/// intra-page + word rotation).
///
/// `buf` is treated as a `[usize]` index array: `buf[i]` holds the index of
/// the next element.  The chain forms a single Hamiltonian cycle that visits
/// every index in `0 .. n_elements` exactly once.
///
/// `range_bytes` is the working-set size.  `line_size` and `page_size` are
/// in bytes.  `rng` provides deterministic shuffling.
pub fn build_chain(
    buf: &mut [usize],
    range_bytes: usize,
    line_size: usize,
    page_size: usize,
    rng: &mut Xoshiro256StarStar,
) {
    let word_size = std::mem::size_of::<usize>();
    let n_elements = range_bytes / word_size;
    assert!(
        n_elements > 0 && n_elements <= buf.len(),
        "range_bytes={range_bytes} gives {n_elements} elements but buf.len()={}",
        buf.len(),
    );

    // --- Layer 1: Fisher-Yates shuffle of page offsets ----------------------
    let effective_page = if range_bytes >= page_size {
        page_size
    } else {
        range_bytes
    };
    let n_pages = range_bytes / effective_page;
    let mut pages: Vec<usize> = (0..n_pages).map(|i| i * effective_page).collect();
    fisher_yates_shuffle(&mut pages, rng);

    // --- Layer 2: bit-reversal of line offsets within a page ----------------
    let n_lines = effective_page / line_size;
    let line_bits = if n_lines > 1 {
        n_lines.trailing_zeros()
    } else {
        0
    };
    let lines: Vec<usize> = (0..n_lines)
        .map(|i| bit_reverse(i, line_bits) * line_size)
        .collect();

    // --- Layer 3: bit-reversal of word offsets within a line ----------------
    let n_words = line_size / word_size;
    let word_bits = if n_words > 1 {
        n_words.trailing_zeros()
    } else {
        0
    };
    let words: Vec<usize> = (0..n_words)
        .map(|i| bit_reverse(i, word_bits) * word_size)
        .collect();

    // --- Flatten the traversal order ----------------------------------------
    let mut order: Vec<usize> = Vec::with_capacity(n_elements);
    for &p in &pages {
        for &l in &lines {
            for &w in &words {
                let byte_offset = p + l + w;
                let idx = byte_offset / word_size;
                if idx < n_elements {
                    order.push(idx);
                }
            }
        }
    }

    // Rotate so that index 0 is the first element.  This ensures the chain
    // starts at buf[0] and every element is reachable.
    if let Some(pos) = order.iter().position(|&x| x == 0) {
        order.rotate_left(pos);
    }

    // --- Wire up the circular linked list -----------------------------------
    for i in 0..order.len() - 1 {
        buf[order[i]] = order[i + 1];
    }
    if let Some(&last) = order.last() {
        buf[last] = order[0];
    }
}

/// Fisher-Yates (Knuth) in-place shuffle.
pub fn fisher_yates_shuffle(slice: &mut [usize], rng: &mut Xoshiro256StarStar) {
    let n = slice.len();
    for i in (1..n).rev() {
        let j = rng.next_bounded(i + 1);
        slice.swap(i, j);
    }
}

/// Reverse the lowest `bits` bits of `value`.
pub fn bit_reverse(value: usize, bits: u32) -> usize {
    if bits == 0 {
        return 0;
    }
    let mut result: usize = 0;
    let mut v = value;
    for _ in 0..bits {
        result = (result << 1) | (v & 1);
        v >>= 1;
    }
    result
}

// ---------------------------------------------------------------------------
// Stride-based chain for 2D sweep (§11.1)
// ---------------------------------------------------------------------------

/// Build a circular pointer-chase chain with one element per stride-sized
/// region, shuffled via Fisher-Yates.
///
/// Unlike [`build_chain`], this function is designed for the `(range, stride)`
/// 2D sweep: it visits exactly `range / stride` positions spaced `stride`
/// bytes apart, in a random order.  This works with any range/stride
/// combination (no power-of-2 requirement).
///
/// Each position `i` is at byte offset `i * stride`, stored as an index into
/// a `[usize]` buffer: `buf[i * (stride / word_size)]`.  The chain forms a
/// single Hamiltonian cycle starting at position 0.
///
/// # Panics
///
/// - `stride` must be a multiple of `size_of::<usize>()`.
/// - `range / stride` must be ≥ 2 (at least two positions to form a cycle).
/// - `range` must be ≤ `buf.len() * size_of::<usize>()`.
pub fn build_stride_chain(
    buf: &mut [usize],
    range: usize,
    stride: usize,
    rng: &mut Xoshiro256StarStar,
) {
    let word_size = std::mem::size_of::<usize>();
    assert!(
        stride.is_multiple_of(word_size),
        "stride ({stride}) must be a multiple of word size ({word_size})"
    );
    let n_positions = range / stride;
    assert!(
        n_positions >= 2,
        "need at least 2 positions: range={range}, stride={stride}, n_positions={n_positions}"
    );
    assert!(
        range <= std::mem::size_of_val(buf),
        "range ({range}) exceeds buffer capacity ({})",
        std::mem::size_of_val(buf)
    );

    let stride_words = stride / word_size;

    // Create position indices [0, 1, 2, ..., n_positions-1] and shuffle.
    let mut positions: Vec<usize> = (0..n_positions).collect();
    fisher_yates_shuffle(&mut positions, rng);

    // Rotate so that position 0 is first (chain starts at buf[0]).
    if let Some(pos) = positions.iter().position(|&x| x == 0) {
        positions.rotate_left(pos);
    }

    // Wire up the circular linked list.
    // buf[positions[i] * stride_words] = positions[i+1] * stride_words
    for i in 0..positions.len() - 1 {
        buf[positions[i] * stride_words] = positions[i + 1] * stride_words;
    }
    // Close the cycle: last → first.
    buf[positions[positions.len() - 1] * stride_words] = positions[0] * stride_words;
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_reverse_basic() {
        assert_eq!(bit_reverse(0b0000, 4), 0b0000);
        assert_eq!(bit_reverse(0b0001, 4), 0b1000);
        assert_eq!(bit_reverse(0b1010, 4), 0b0101);
        assert_eq!(bit_reverse(0b1111, 4), 0b1111);
        assert_eq!(bit_reverse(0b110, 3), 0b011);
    }

    #[test]
    fn bit_reverse_identity_single_bit() {
        assert_eq!(bit_reverse(0, 1), 0);
        assert_eq!(bit_reverse(1, 1), 1);
    }

    #[test]
    fn bit_reverse_zero_bits() {
        assert_eq!(bit_reverse(42, 0), 0);
    }

    #[test]
    fn fisher_yates_preserves_elements() {
        let mut rng = Xoshiro256StarStar::new(42);
        let mut v: Vec<usize> = (0..64).collect();
        let original: Vec<usize> = v.clone();
        fisher_yates_shuffle(&mut v, &mut rng);

        // Same elements, possibly different order.
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(sorted, original);

        // Very unlikely (1/64!) that the shuffle is a no-op.
        assert_ne!(v, original, "shuffle should change the order");
    }

    #[test]
    fn build_chain_single_cycle() {
        // 8 KiB range, 64-byte line, 4096-byte page.
        let range = 8192;
        let line = 64;
        let page = 4096;
        let ws = std::mem::size_of::<usize>();
        let n = range / ws;

        let mut buf = vec![0usize; n];
        let mut rng = Xoshiro256StarStar::new(42);
        build_chain(&mut buf, range, line, page, &mut rng);

        // Follow the chain starting at 0 and count distinct visits.
        let mut visited = vec![false; n];
        let mut idx = 0usize;
        let mut count = 0usize;
        loop {
            assert!(!visited[idx], "cycle revisits index {idx}");
            visited[idx] = true;
            count += 1;
            idx = buf[idx];
            if idx == 0 {
                break;
            }
        }
        assert_eq!(count, n, "chain must visit every element exactly once");
    }

    #[test]
    fn build_chain_small_range() {
        // Range smaller than a page.
        let range = 1024;
        let line = 64;
        let page = 4096;
        let ws = std::mem::size_of::<usize>();
        let n = range / ws;

        let mut buf = vec![0usize; n];
        let mut rng = Xoshiro256StarStar::new(7);
        build_chain(&mut buf, range, line, page, &mut rng);

        let mut visited = vec![false; n];
        let mut idx = 0usize;
        let mut count = 0usize;
        loop {
            assert!(!visited[idx], "cycle revisits index {idx}");
            visited[idx] = true;
            count += 1;
            idx = buf[idx];
            if idx == 0 {
                break;
            }
        }
        assert_eq!(count, n);
    }

    #[test]
    fn pre_touch_does_not_panic() {
        let mut v = vec![0xFFu8; 16384];
        pre_touch(&mut v);
        // First byte of each page and last byte should be 0.
        assert_eq!(v[0], 0);
        assert_eq!(v[v.len() - 1], 0);
    }

    #[test]
    fn aligned_buffer_basic() {
        let buf = AlignedBuffer::new(4096, 4096).expect("allocation should succeed");
        assert_eq!(buf.len(), 4096);
        assert!(!buf.is_empty());
        // Pointer should be page-aligned.
        assert_eq!(buf.as_ptr() as usize % 4096, 0);
    }

    #[test]
    fn anti_aliased_layout_no_overlap() {
        let layout = compute_anti_aliased_layout(65536);
        // All four views must be non-overlapping.
        for i in 0..4 {
            for j in (i + 1)..4 {
                let a_start = layout.offsets[i];
                let a_end = a_start + layout.slice_len;
                let b_start = layout.offsets[j];
                assert!(
                    a_end <= b_start || layout.offsets[j] + layout.slice_len <= a_start,
                    "views {i} and {j} overlap"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // build_stride_chain
    // -----------------------------------------------------------------------

    #[test]
    fn stride_chain_is_hamiltonian_cycle() {
        let ws = std::mem::size_of::<usize>();
        // Test various (range, stride) combinations including non-power-of-2.
        let test_cases: Vec<(usize, usize)> = vec![
            (1024, 8),     // stride = word size
            (1024, 64),    // 16 positions
            (4096, 64),    // 64 positions
            (4096, 512),   // 8 positions
            (768, 64),     // non-power-of-2 range: 12 positions
            (1280, 64),    // non-power-of-2 range: 20 positions
            (3072, 256),   // non-power-of-2 range: 12 positions
            (65536, 4096), // large stride: 16 positions
        ];

        for (range, stride) in test_cases {
            let n_elements = range / ws;
            let n_positions = range / stride;
            let stride_words = stride / ws;

            let mut buf = vec![0usize; n_elements];
            let mut rng = Xoshiro256StarStar::new(42);
            build_stride_chain(&mut buf, range, stride, &mut rng);

            // Follow the chain starting at index 0, visiting stride-spaced positions.
            let mut visited = vec![false; n_positions];
            let mut idx = 0usize; // index into buf (in words)
            let mut count = 0usize;
            loop {
                let pos = idx / stride_words;
                assert!(
                    pos < n_positions,
                    "range={range} stride={stride}: out-of-bounds position {pos}"
                );
                assert!(
                    !visited[pos],
                    "range={range} stride={stride}: revisits position {pos}"
                );
                visited[pos] = true;
                count += 1;
                idx = buf[idx];
                if idx == 0 {
                    break;
                }
            }
            assert_eq!(
                count, n_positions,
                "range={range} stride={stride}: visited {count}/{n_positions} positions"
            );
        }
    }

    #[test]
    fn stride_chain_seed_sensitivity() {
        let ws = std::mem::size_of::<usize>();
        let range = 65536;
        let stride = 64;
        let n_elements = range / ws;
        let stride_words = stride / ws;

        let build_order = |seed: u64| -> Vec<usize> {
            let mut buf = vec![0usize; n_elements];
            let mut rng = Xoshiro256StarStar::new(seed);
            build_stride_chain(&mut buf, range, stride, &mut rng);

            let mut order = Vec::new();
            let mut idx = 0usize;
            loop {
                order.push(idx / stride_words);
                idx = buf[idx];
                if idx == 0 {
                    break;
                }
            }
            order
        };

        let order_a = build_order(42);
        let order_b = build_order(123);
        assert_ne!(
            order_a, order_b,
            "different seeds should produce different traversal orders"
        );
    }

    #[test]
    #[should_panic(expected = "need at least 2 positions")]
    fn stride_chain_rejects_single_position() {
        let ws = std::mem::size_of::<usize>();
        let range = 64;
        let stride = 64;
        let mut buf = vec![0usize; range / ws];
        let mut rng = Xoshiro256StarStar::new(42);
        build_stride_chain(&mut buf, range, stride, &mut rng);
    }

    #[test]
    fn anti_aliased_layout_diverse_cache_sets() {
        let layout = compute_anti_aliased_layout(65536);
        // Extract bits 6-11 (L1 cache-set index for 64-byte lines).
        let set_indices: Vec<usize> = layout
            .offsets
            .iter()
            .map(|&off| (off >> 6) & 0x3F)
            .collect();
        // All four should be distinct.
        for i in 0..4 {
            for j in (i + 1)..4 {
                assert_ne!(
                    set_indices[i], set_indices[j],
                    "views {i} and {j} map to the same L1 cache set"
                );
            }
        }
    }
}
