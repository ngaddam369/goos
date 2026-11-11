//! Flat Monte Carlo move choice: every candidate move gets random playouts
//! in turn, and the move whose playouts were won most often is chosen.
//!
//! The search is incremental (`step` runs a few playouts and returns) so
//! the kernel can interleave thinking with keyboard input and drawing.

use crate::board::{Board, Color, MAX_SIZE, Point};
use crate::game::{Game, Move, Phase};
use crate::playout::{is_eye, playout, playout_limit, playout_winner, should_pass};
use crate::rng::Rng;

const AREA: usize = MAX_SIZE * MAX_SIZE;

/// Below this win rate (per mille) the game is considered lost and the
/// player passes instead of filling the board.
const RESIGN_PER_MILLE: u32 = 20;

pub struct FlatMc {
    root: Board,
    color: Color,
    komi_half: u16,
    /// Pass straight away (see `should_pass`).
    pass: bool,
    candidates: [Point; AREA],
    wins: [u32; AREA],
    visits: [u32; AREA],
    count: usize,
    /// Round-robin position in `candidates`.
    next: usize,
    playouts: u32,
    rng: Rng,
}

impl FlatMc {
    pub const fn new(seed: u64) -> Self {
        FlatMc {
            root: Board::new(MAX_SIZE),
            color: Color::Black,
            komi_half: 0,
            pass: true,
            candidates: [Point::new(0, 0); AREA],
            wins: [0; AREA],
            visits: [0; AREA],
            count: 0,
            next: 0,
            playouts: 0,
            rng: Rng::new(seed),
        }
    }

    /// Starts thinking about the position in `game`, for the player to move.
    /// Candidates are all legal moves (superko included) that don't fill
    /// the player's own eyes.
    pub fn start(&mut self, game: &Game) {
        let board = *game.board();
        let color = game.to_move();
        self.root = board;
        self.color = color;
        self.komi_half = game.komi_half();
        self.pass = game.phase() != Phase::Playing || should_pass(game);
        self.count = 0;
        self.next = 0;
        self.playouts = 0;
        if self.pass {
            return;
        }
        for p in board.points() {
            if board.is_legal(p, color) && !is_eye(&board, p, color) && game.check(p).is_ok() {
                self.candidates[self.count] = p;
                self.wins[self.count] = 0;
                self.visits[self.count] = 0;
                self.count += 1;
            }
        }
    }

    /// Runs up to `n` more playouts.
    pub fn step(&mut self, n: u32) {
        if self.pass || self.count == 0 {
            return;
        }
        for _ in 0..n {
            let i = self.next;
            self.next = (self.next + 1) % self.count;
            let mut board = self.root;
            let Ok(info) = board.play(self.candidates[i], self.color) else { continue };
            let limit = playout_limit(&board);
            if !playout(&mut board, self.color.opponent(), info.ko, &mut self.rng, limit) {
                continue; // never finished (endless ko): no verdict
            }
            self.visits[i] += 1;
            if playout_winner(&board, self.komi_half) == self.color {
                self.wins[i] += 1;
            }
            self.playouts += 1;
        }
    }

    /// True if there's nothing to think about: passing, or at most one
    /// candidate move.
    pub fn decided(&self) -> bool {
        self.pass || self.count <= 1
    }

    /// Playouts run so far.
    pub fn playouts(&self) -> u32 {
        self.playouts
    }

    /// The best move found so far, and its win rate in per mille. Passes
    /// (in effect, resigns) when even the best move almost never wins.
    pub fn best(&mut self) -> (Move, u32) {
        if self.pass || self.count == 0 {
            return (Move::Pass, 0);
        }
        // Rank by (wins + 1) / (visits + 2), so a move won 1 out of 1 doesn't
        // beat one won 90 out of 100. Compare by cross-multiplying; break
        // ties by visits, then randomly (never by board position).
        let score = |i: usize| (self.wins[i] as u64 + 1, self.visits[i] as u64 + 2);
        let mut best = 0;
        let mut ties = 1;
        for i in 1..self.count {
            let ((wa, va), (wb, vb)) = (score(i), score(best));
            let order = (wa * vb).cmp(&(wb * va)).then(self.visits[i].cmp(&self.visits[best]));
            match order {
                core::cmp::Ordering::Greater => {
                    best = i;
                    ties = 1;
                }
                core::cmp::Ordering::Equal => {
                    // Reservoir sampling: each tied move is equally likely.
                    ties += 1;
                    if self.rng.below(ties) == 0 {
                        best = i;
                    }
                }
                core::cmp::Ordering::Less => {}
            }
        }
        let rate = (self.wins[best] as u64 * 1000 / self.visits[best].max(1) as u64) as u32;
        let well_sampled = self.visits[best] >= 50;
        if well_sampled && rate < RESIGN_PER_MILLE {
            return (Move::Pass, rate);
        }
        (Move::Play(self.candidates[best]), rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::diagram;
    use crate::playout::random_bot;

    fn p(col: usize, row: usize) -> Point {
        Point::new(col, row)
    }

    fn game_from(text: &str, to_move: Color, komi_half: u16) -> Box<Game> {
        let mut game = Box::new(Game::new(5));
        let board = diagram(text);
        game.reset(board.size());
        game.set_position(board, to_move);
        game.set_komi_half(komi_half);
        game
    }

    #[test]
    fn single_candidate_and_no_candidate() {
        // (0,0) and (2,0) are black eyes; black's only other move is
        // capturing the white stone at (4,1).
        let game = game_from(
            "
            . X . X X
            X X X X O
            X X X X .
            X X X X X
            X X X X X",
            Color::Black,
            1,
        );
        let mut mc = FlatMc::new(1);
        mc.start(&game);
        mc.step(10);
        assert_eq!(mc.best().0, Move::Play(p(4, 2)));

        let game = game_from(
            "
            . X . X .
            X X X X X
            X X X X X
            X X X X X
            X X X X X",
            Color::Black,
            1,
        );
        mc.start(&game);
        mc.step(10);
        assert_eq!(mc.best().0, Move::Pass);
    }

    #[test]
    fn passes_when_the_game_is_lost() {
        // White owns almost everything; black can't live.
        let game = game_from(
            "
            . O . O .
            O O O O O
            O O . O O
            O O O O O
            . . . . .",
            Color::Black,
            15,
        );
        let mut mc = FlatMc::new(4);
        mc.start(&game);
        mc.step(2000);
        assert_eq!(mc.best().0, Move::Pass);
    }

    #[test]
    fn prefers_the_centre_of_an_empty_5x5_board() {
        let mut game = Box::new(Game::new(5));
        game.set_komi_half(1);
        let mut mc = FlatMc::new(7);
        mc.start(&game);
        mc.step(25 * 400);
        let (Move::Play(m), rate) = mc.best() else { panic!("should not pass") };
        assert!((1..=3).contains(&m.col) && (1..=3).contains(&m.row), "edge move {m:?} ({rate}‰)");
    }

    #[test]
    fn captures_to_save_its_own_group() {
        // Black's pair at (1,2),(2,2) and the white stone at (2,3) are both
        // in atari, sharing the liberty (1,3): whoever plays there captures.
        let game = game_from(
            "
            . . . . .
            . O O . .
            O X X O .
            . . O X .
            . . X . .",
            Color::Black,
            1,
        );
        assert_eq!(game.board().liberties(p(1, 2)), 1);
        assert_eq!(game.board().liberties(p(2, 3)), 1);
        let mut mc = FlatMc::new(3);
        mc.start(&game);
        mc.step(4000);
        assert_eq!(mc.best().0, Move::Play(p(1, 3)));
    }

    /// Flat MC with a small budget against the random bot on 9x9.
    /// Run with `cargo test -p go-core --release -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn flat_mc_beats_random() {
        let mut rng = Rng::new(2024);
        let mut mc = FlatMc::new(99);
        let mut wins = 0;
        let games = 10;
        for g in 0..games {
            let mut game = Box::new(Game::new(9));
            let mc_color = if g % 2 == 0 { Color::Black } else { Color::White };
            while game.phase() == Phase::Playing && game.move_number() < 300 {
                let mv = if game.to_move() == mc_color {
                    mc.start(&game);
                    mc.step(2000);
                    mc.best().0
                } else {
                    random_bot(&game, &mut rng)
                };
                match mv {
                    Move::Play(pt) => {
                        game.play(pt).unwrap();
                    }
                    Move::Pass => game.pass().unwrap(),
                }
            }
            if game.score().winner() == Some(mc_color) {
                wins += 1;
            }
        }
        println!("flat MC won {wins}/{games} against random");
        assert!(wins >= 9);
    }
}
