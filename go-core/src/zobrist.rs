//! Zobrist hashing: every (point, colour) pair gets a random 64-bit key and
//! a position's hash is the XOR of the keys of all its stones. Placing or
//! removing a stone is a single XOR.

use crate::board::AREA;

/// One step of the splitmix64 generator: returns (next state, output).
const fn splitmix64(state: u64) -> (u64, u64) {
    let state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (state, z ^ (z >> 31))
}

/// Keys indexed by point index, then colour (0 = black, 1 = white).
/// Generated at compile time.
pub const KEYS: [[u64; 2]; AREA] = {
    let mut keys = [[0; 2]; AREA];
    let mut state = 0x60_6F_6F_73; // "GoOS"
    let mut i = 0;
    while i < AREA {
        let (s, black) = splitmix64(state);
        let (s, white) = splitmix64(s);
        keys[i] = [black, white];
        state = s;
        i += 1;
    }
    keys
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_distinct_and_nonzero() {
        let mut all: Vec<u64> = KEYS.iter().flatten().copied().collect();
        assert!(all.iter().all(|&k| k != 0));
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), AREA * 2);
    }
}
