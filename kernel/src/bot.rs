//! The computer opponent. `think` is called on every pass of the main loop
//! while it's the computer's turn and does a bounded amount of work, so
//! the keyboard and screen stay responsive while it thinks.

use go_core::{Game, Move, Rng, random_bot};

use crate::interrupts::TICK_HZ;

/// Even an instant decision waits this long, so the player sees their own
/// stone land before the reply appears.
const MIN_DELAY: u64 = TICK_HZ * 2 / 5;

pub struct Bot {
    rng: Rng,
    /// When the current decision may be played (`None`: not thinking).
    ready_at: Option<u64>,
}

impl Bot {
    pub fn new(seed: u64) -> Self {
        Bot { rng: Rng::new(seed), ready_at: None }
    }

    /// Drops any thinking in progress (after undo, new game...).
    pub fn cancel(&mut self) {
        self.ready_at = None;
    }

    /// Thinks a little; returns the computer's move once it has decided.
    pub fn think(&mut self, game: &Game, now: u64) -> Option<Move> {
        let ready = *self.ready_at.get_or_insert(now + MIN_DELAY);
        if now < ready {
            return None;
        }
        self.ready_at = None;
        Some(random_bot(game, &mut self.rng))
    }
}

/// A seed that differs from boot to boot: the CPU's cycle counter.
pub fn seed() -> u64 {
    // SAFETY: rdtsc is available on every x86_64 CPU and has no side effects.
    unsafe { core::arch::x86_64::_rdtsc() }
}
