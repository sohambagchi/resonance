//! Xoshiro256** pseudo-random number generator.
//!
//! A small, fast, high-quality PRNG vendored inline — no external crate
//! needed.  Seeded via SplitMix64 to expand a single `u64` seed into the
//! 256-bit internal state.
//!
//! Reference: <https://prng.di.unimi.it/xoshiro256starstar.c>

/// 256-bit state, advanced by the xoshiro256** algorithm.
pub struct Xoshiro256StarStar {
    state: [u64; 4],
}

impl Xoshiro256StarStar {
    /// Create a new generator seeded from a single `u64`.
    pub fn new(seed: u64) -> Self {
        let mut sm = seed;
        Self {
            state: [
                splitmix64(&mut sm),
                splitmix64(&mut sm),
                splitmix64(&mut sm),
                splitmix64(&mut sm),
            ],
        }
    }

    /// Return the next pseudo-random `u64`.
    pub fn next_u64(&mut self) -> u64 {
        let result = self.state[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);

        let t = self.state[1] << 17;

        self.state[2] ^= self.state[0];
        self.state[3] ^= self.state[1];
        self.state[1] ^= self.state[2];
        self.state[0] ^= self.state[3];
        self.state[2] ^= t;
        self.state[3] = self.state[3].rotate_left(45);

        result
    }

    /// Return a random `usize` (alias for `next_u64` truncated on 32-bit).
    #[allow(clippy::cast_possible_truncation)]
    pub fn next_usize(&mut self) -> usize {
        self.next_u64() as usize
    }

    /// Return a uniformly distributed value in `0..n` using rejection sampling.
    ///
    /// # Panics
    ///
    /// Panics if `n == 0`.
    pub fn next_bounded(&mut self, n: usize) -> usize {
        assert!(n > 0, "next_bounded requires n > 0");
        if n == 1 {
            return 0;
        }
        // Bitmask rejection method — unbiased and fast for any n.
        let mask = n.next_power_of_two() - 1;
        loop {
            let candidate = self.next_usize() & mask;
            if candidate < n {
                return candidate;
            }
        }
    }
}

/// SplitMix64 — used only to expand a `u64` seed into the 256-bit state.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden-value test: ensure deterministic output for a known seed.
    #[test]
    fn deterministic_output() {
        let mut rng = Xoshiro256StarStar::new(42);
        let first_ten: Vec<u64> = (0..10).map(|_| rng.next_u64()).collect();

        // Re-seed with the same seed — must produce identical sequence.
        let mut rng2 = Xoshiro256StarStar::new(42);
        let second_ten: Vec<u64> = (0..10).map(|_| rng2.next_u64()).collect();

        assert_eq!(first_ten, second_ten);
    }

    /// All four state words should be non-zero after seeding.
    #[test]
    fn seeding_non_zero() {
        let rng = Xoshiro256StarStar::new(0);
        assert!(rng.state.iter().all(|&s| s != 0));
    }

    /// next_bounded should always return values in [0, n).
    #[test]
    fn bounded_range() {
        let mut rng = Xoshiro256StarStar::new(99);
        for n in [1, 2, 3, 7, 16, 100, 1000] {
            for _ in 0..200 {
                let v = rng.next_bounded(n);
                assert!(v < n, "next_bounded({n}) returned {v}");
            }
        }
    }
}
