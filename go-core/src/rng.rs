//! A small, fast pseudo-random number generator (xorshift64*). Not
//! cryptographic; plenty for random playouts.

#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// Any seed works; zero is replaced since xorshift would get stuck.
    pub const fn new(seed: u64) -> Rng {
        Rng(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number in `0..n` (`n` must be non-zero).
    pub fn below(&mut self, n: usize) -> usize {
        // Multiply-shift: maps the top 32 bits evenly enough onto 0..n.
        (((self.next_u64() >> 32) * n as u64) >> 32) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let (mut a, mut b) = (Rng::new(42), Rng::new(42));
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn below_stays_in_range_and_covers_it() {
        let mut rng = Rng::new(7);
        let mut seen = [0u32; 9];
        for _ in 0..9000 {
            let v = rng.below(9);
            seen[v] += 1;
        }
        assert!(seen.iter().all(|&c| c > 800), "roughly uniform: {seen:?}");
        assert_eq!(Rng::new(0).below(1), 0);
    }
}
