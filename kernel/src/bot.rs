//! The computer opponent. `think` is called on every pass of the main loop
//! while it's the computer's turn. Each call runs Monte Carlo tree search
//! for a few milliseconds and returns, so the keyboard and screen stay
//! responsive while it thinks.

use go_core::{Game, Mcts, Move, Node};
use spin::{Mutex, MutexGuard};

use crate::interrupts::{TICK_HZ, ticks};
use crate::serial_println;

/// Even an instant decision waits this long, so the player sees their own
/// stone land before the reply appears.
const MIN_DELAY: u64 = TICK_HZ * 2 / 5;
/// Work done per `think` call before returning to the main loop.
const SLICE: u64 = TICK_HZ / 125;

/// Search tree storage: 4 MiB of zeroed nodes, so it lives in `.bss`
/// rather than in the kernel image.
const TREE_NODES: usize = 1 << 18;
static TREE: Mutex<[Node; TREE_NODES]> = Mutex::new([Node::EMPTY; TREE_NODES]);

/// Thinking time per move, by board size.
fn budget(size: usize) -> u64 {
    match size {
        0..=9 => 2 * TICK_HZ,
        10..=13 => 3 * TICK_HZ,
        _ => 5 * TICK_HZ,
    }
}

struct Thinking {
    /// The position being thought about (move number and hash), to notice
    /// if it changed under us.
    position: (u16, u64),
    started: u64,
    deadline: u64,
}

pub struct Bot {
    search: Mcts<'static>,
    thinking: Option<Thinking>,
}

impl Bot {
    /// Creates the computer player. There can only be one, as it takes
    /// over the static search tree for good.
    pub fn new(seed: u64) -> Self {
        let tree = TREE.try_lock().expect("only one Bot may exist");
        let nodes: &'static mut [Node; TREE_NODES] = MutexGuard::leak(tree);
        Bot { search: Mcts::new(nodes, seed), thinking: None }
    }

    /// Drops any thinking in progress (after undo, new game...).
    pub fn cancel(&mut self) {
        self.thinking = None;
    }

    /// Whether thinking needs the CPU (rather than just waiting out the
    /// minimum delay, which the timer interrupt can wake us for).
    pub fn computing(&self) -> bool {
        self.thinking.is_some() && !self.search.decided()
    }

    /// Playouts run for the current decision.
    pub fn playouts(&self) -> u32 {
        if self.thinking.is_some() { self.search.playouts() } else { 0 }
    }

    /// Thinks for one slice; returns the computer's move once it has decided.
    pub fn think(&mut self, game: &Game, now: u64) -> Option<Move> {
        let position = (game.move_number(), game.board().hash());
        if self.thinking.as_ref().is_none_or(|t| t.position != position) {
            self.search.start(game);
            let think_for = if self.search.decided() { 0 } else { budget(game.size()) };
            self.thinking = Some(Thinking {
                position,
                started: now,
                deadline: now + think_for.max(MIN_DELAY),
            });
        }
        let deadline = self.thinking.as_ref().map_or(now, |t| t.deadline);

        let slice_end = now + SLICE;
        while !self.search.decided() && ticks() < slice_end.min(deadline) {
            self.search.step(1);
        }

        if ticks() < deadline {
            return None;
        }
        let started = self.thinking.take().map_or(now, |t| t.started);
        let (mv, rate) = self.search.best();
        serial_println!(
            "computer: {mv:?}, win rate {}.{}% after {} playouts in {} ms, {} tree nodes",
            rate / 10,
            rate % 10,
            self.search.playouts(),
            (ticks() - started) * 1000 / TICK_HZ,
            self.search.tree_size()
        );
        Some(mv)
    }
}

/// A seed that differs from boot to boot: the CPU's cycle counter.
pub fn seed() -> u64 {
    // SAFETY: rdtsc is available on every x86_64 CPU and has no side effects.
    unsafe { core::arch::x86_64::_rdtsc() }
}
