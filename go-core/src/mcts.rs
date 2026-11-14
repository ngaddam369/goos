//! Monte Carlo tree search with UCB1 (UCT), using only integer and
//! fixed-point arithmetic.
//!
//! Each iteration walks down the tree from the current position, always
//! picking the child with the best UCB1 value, adds the children of the
//! node it ends on once that node has been visited a few times, finishes
//! the game with a random playout, and records the result on every node of
//! the path. The move played is the root child visited most.
//!
//! Nodes live in a caller-provided slice (no heap): the kernel keeps them
//! in a static, tests in a `Vec`. When the pool is full the tree simply
//! stops growing and the search carries on with playouts from its leaves.

use crate::board::{Board, Color, MAX_SIZE, Point};
use crate::game::{Game, Move, Phase};
use crate::playout::{is_eye, playout, playout_limit, playout_winner, should_pass};
use crate::rng::Rng;

/// Fixed-point scale: 1.0 is `ONE`.
const ONE: u64 = 1 << 16;
/// UCB1 exploration constant (0.5).
const EXPLORATION: u64 = ONE / 2;
/// A node's children are added once it has been visited this often.
const EXPAND_AFTER: u32 = 4;
/// Longest path through the tree that is followed.
const MAX_DEPTH: usize = 256;
/// Below this win rate (per mille) the game is considered lost and the
/// player passes instead of filling the board.
const RESIGN_PER_MILLE: u32 = 20;

/// A move in the tree. All-zero bytes are a valid empty node, so a pool of
/// them can live in `.bss`.
#[derive(Clone, Copy, Debug)]
pub struct Node {
    /// The point played to reach this node.
    point: Point,
    child_count: u16,
    /// Index of the first child in the pool; children are contiguous.
    /// 0 means not expanded yet (the root is never anyone's child), and
    /// `NO_CHILDREN` that expansion was done or given up on (no legal
    /// moves, or the pool was full), so it isn't retried.
    first_child: u32,
    visits: u32,
    /// Results for the player who made this node's move, in half wins
    /// (a win counts 2, an unfinished playout 1).
    half_wins: u32,
}

/// `first_child` of a node that is a leaf for good.
const NO_CHILDREN: u32 = u32::MAX;

impl Node {
    pub const EMPTY: Node = Node {
        point: Point::new(0, 0),
        child_count: 0,
        first_child: 0,
        visits: 0,
        half_wins: 0,
    };

    fn children(&self) -> core::ops::Range<usize> {
        let first = self.first_child as usize;
        first..first + self.child_count as usize
    }

    fn expanded(&self) -> bool {
        self.first_child != 0
    }
}

/// log2(x) in fixed point (x >= 1), computed bit by bit: normalise x into
/// [1, 2), then each squaring of the mantissa reveals the next bit.
fn log2_fixed(x: u64) -> u64 {
    let int = 63 - x.leading_zeros() as u64;
    // Mantissa in [ONE, 2 * ONE).
    let mut m = if int >= 16 {
        x >> (int - 16)
    } else {
        x << (16 - int)
    };
    let mut frac = 0;
    for bit in (0..16).rev() {
        m = m * m / ONE;
        if m >= 2 * ONE {
            m /= 2;
            frac |= 1 << bit;
        }
    }
    (int << 16) | frac
}

/// 1/sqrt(n) in fixed point for small n, computed at compile time: the
/// UCB formula needs it for every child on every step down the tree.
const RSQRT_TABLE: [u32; 4096] = {
    let mut table = [0; 4096];
    let mut n = 1;
    while n < 4096 {
        table[n] = (ONE * ONE / (n as u64 * ONE * ONE).isqrt()) as u32;
        n += 1;
    }
    table
};

/// 1/sqrt(n) in fixed point (n >= 1).
fn rsqrt_fixed(n: u64) -> u64 {
    match RSQRT_TABLE.get(n as usize) {
        Some(&r) => r as u64,
        None => ONE * ONE / (n * ONE * ONE).isqrt(),
    }
}

/// ln(x) in fixed point (x >= 1).
fn ln_fixed(x: u64) -> u64 {
    const LN_2: u64 = 45_426; // 0.693147 * ONE
    log2_fixed(x) * LN_2 / ONE
}

pub struct Mcts<'a> {
    nodes: &'a mut [Node],
    used: usize,
    root_board: Board,
    root_color: Color,
    root_ko: Option<Point>,
    komi_half: u16,
    /// Pass straight away (see `should_pass`).
    pass: bool,
    playouts: u32,
    rng: Rng,
}

impl<'a> Mcts<'a> {
    /// A search using `nodes` as its tree storage (at least one node).
    pub fn new(nodes: &'a mut [Node], seed: u64) -> Self {
        Mcts {
            nodes,
            used: 0,
            root_board: Board::new(MAX_SIZE),
            root_color: Color::Black,
            root_ko: None,
            komi_half: 0,
            pass: true,
            playouts: 0,
            rng: Rng::new(seed),
        }
    }

    /// Starts thinking about the position in `game`, for the player to move.
    pub fn start(&mut self, game: &Game) {
        self.root_board = *game.board();
        self.root_color = game.to_move();
        self.root_ko = game.ko();
        self.komi_half = game.komi_half();
        self.pass = game.phase() != Phase::Playing || should_pass(game);
        self.playouts = 0;
        self.nodes[0] = Node::EMPTY;
        self.used = 1;
        if !self.pass {
            // Root moves are checked against superko as well.
            let board = self.root_board;
            self.expand(0, &board, self.root_color, self.root_ko, |p| {
                game.check(p).is_ok()
            });
        }
    }

    /// True if there's nothing to think about: passing, or no legal move.
    /// (A single legal move is still searched: if it loses badly, passing
    /// is better.)
    pub fn decided(&self) -> bool {
        self.pass || self.nodes[0].child_count == 0
    }

    /// Playouts run so far.
    pub fn playouts(&self) -> u32 {
        self.playouts
    }

    /// Adds the children of `node`: every legal move for `color` on
    /// `board` that doesn't fill its own eye or retake `ko`, in random
    /// order. If there are none, or they don't fit in the pool, the node
    /// stays a leaf for good.
    fn expand(
        &mut self,
        node: usize,
        board: &Board,
        color: Color,
        ko: Option<Point>,
        mut allowed: impl FnMut(Point) -> bool,
    ) {
        self.nodes[node].first_child = NO_CHILDREN;
        self.nodes[node].child_count = 0;
        let first = self.used;
        let mut count = 0;
        for p in board.points() {
            if Some(p) != ko && board.is_legal(p, color) && !is_eye(board, p, color) && allowed(p) {
                if first + count == self.nodes.len() {
                    return; // pool full
                }
                self.nodes[first + count] = Node {
                    point: p,
                    ..Node::EMPTY
                };
                count += 1;
            }
        }
        if count == 0 {
            return;
        }
        // Shuffle, so unvisited children are tried in random order.
        for i in (1..count).rev() {
            let j = self.rng.below(i + 1);
            self.nodes.swap(first + i, first + j);
        }
        self.used += count;
        self.nodes[node].first_child = first as u32;
        self.nodes[node].child_count = count as u16;
    }

    /// The child of `node` with the highest UCB1 value; unvisited children
    /// come first.
    fn select(&self, node: usize) -> usize {
        let parent = &self.nodes[node];
        // C * sqrt(ln N), shared by all children.
        let scale = EXPLORATION * (ln_fixed(parent.visits.max(1) as u64) * ONE).isqrt() / ONE;
        let mut best = parent.first_child as usize;
        let mut best_value = 0;
        for c in parent.children() {
            let child = &self.nodes[c];
            if child.visits == 0 {
                return c;
            }
            let visits = child.visits as u64;
            // Mean result in [0, ONE], plus C * sqrt(ln N) / sqrt(n).
            let mean = child.half_wins as u64 * ONE / (2 * visits);
            let explore = scale * rsqrt_fixed(visits) / ONE;
            let value = mean + explore;
            if value > best_value {
                best_value = value;
                best = c;
            }
        }
        best
    }

    /// Runs up to `n` more search iterations.
    pub fn step(&mut self, n: u32) {
        if self.decided() {
            return;
        }
        let mut path = [0u32; MAX_DEPTH];
        for _ in 0..n {
            let mut board = self.root_board;
            let mut color = self.root_color;
            let mut ko = self.root_ko;
            let mut node = 0;
            let mut depth = 0;

            // Selection: walk down while the node has children.
            while self.nodes[node].child_count > 0 && depth < MAX_DEPTH {
                node = self.select(node);
                path[depth] = node as u32;
                depth += 1;
                // Children were generated as legal moves for this position.
                ko = board
                    .play(self.nodes[node].point, color)
                    .map_or(None, |info| info.ko);
                color = color.opponent();
            }

            // Expansion: grow the tree at well-visited leaves.
            if !self.nodes[node].expanded()
                && self.nodes[node].visits >= EXPAND_AFTER
                && depth < MAX_DEPTH
            {
                self.expand(node, &board, color, ko, |_| true);
                if self.nodes[node].child_count > 0 {
                    node = self.select(node);
                    path[depth] = node as u32;
                    depth += 1;
                    ko = board
                        .play(self.nodes[node].point, color)
                        .map_or(None, |info| info.ko);
                    color = color.opponent();
                }
            }

            // Simulation.
            let limit = playout_limit(&board);
            let winner = playout(&mut board, color, ko, &mut self.rng, limit)
                .then(|| playout_winner(&board, self.komi_half));

            // Backpropagation. Path node i holds a move by the root player
            // when i is even, by the opponent when odd.
            self.nodes[0].visits += 1;
            for (i, &n) in path[..depth].iter().enumerate() {
                let mover = if i % 2 == 0 {
                    self.root_color
                } else {
                    self.root_color.opponent()
                };
                let result = match winner {
                    Some(w) if w == mover => 2,
                    Some(_) => 0,
                    None => 1,
                };
                let node = &mut self.nodes[n as usize];
                node.visits += 1;
                node.half_wins += result;
            }
            self.playouts += 1;
        }
    }

    /// The most visited root move, and its win rate in per mille. Passes
    /// (in effect, resigns) when even that move almost never wins.
    pub fn best(&mut self) -> (Move, u32) {
        let root = self.nodes[0];
        if self.pass || root.child_count == 0 {
            return (Move::Pass, 0);
        }
        let best = root
            .children()
            .max_by_key(|&c| (self.nodes[c].visits, self.nodes[c].half_wins))
            .unwrap_or(root.first_child as usize);
        let node = self.nodes[best];
        let rate = (node.half_wins as u64 * 500 / node.visits.max(1) as u64) as u32;
        if node.visits >= 50 && rate < RESIGN_PER_MILLE {
            return (Move::Pass, rate);
        }
        (Move::Play(node.point), rate)
    }

    /// Nodes in use (for diagnostics).
    pub fn tree_size(&self) -> usize {
        self.used
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::diagram;
    use crate::mc::FlatMc;
    use crate::playout::random_bot;

    fn p(col: usize, row: usize) -> Point {
        Point::new(col, row)
    }

    fn game_from(text: &str, to_move: Color, komi_half: u16) -> Box<Game> {
        let board = diagram(text);
        let mut game = Box::new(Game::new(board.size()));
        game.set_position(board, to_move);
        game.set_komi_half(komi_half);
        game
    }

    fn pool(n: usize) -> Vec<Node> {
        vec![Node::EMPTY; n]
    }

    #[test]
    fn fixed_point_logarithms() {
        use core::f64::consts::{LN_2, LN_10};
        for (x, expected) in [
            (1u64, 0.0f64),
            (2, LN_2),
            (10, LN_10),
            (1000, 6.907_755),
            (123_456, 11.723_637),
        ] {
            let got = ln_fixed(x) as f64 / ONE as f64;
            assert!(
                (got - expected).abs() < 0.001,
                "ln({x}) = {got}, expected {expected}"
            );
        }
        assert_eq!(log2_fixed(1 << 20), 20 << 16);
        for n in [1u64, 2, 3, 100, 4095, 4096, 1_000_000] {
            let got = rsqrt_fixed(n) as f64 / ONE as f64;
            let expected = 1.0 / (n as f64).sqrt();
            assert!((got - expected).abs() < 0.0001, "1/sqrt({n}) = {got}");
        }
    }

    #[test]
    fn captures_to_save_its_own_group() {
        // Black's pair and the white stone at (2,3) share their last
        // liberty (1,3): whoever plays there captures.
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
        let mut nodes = pool(50_000);
        let mut mcts = Mcts::new(&mut nodes, 3);
        mcts.start(&game);
        mcts.step(3000);
        assert_eq!(mcts.best().0, Move::Play(p(1, 3)));
    }

    #[test]
    fn passes_when_lost_or_nothing_to_do() {
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
        let mut nodes = pool(50_000);
        let mut mcts = Mcts::new(&mut nodes, 4);
        mcts.start(&game);
        mcts.step(2000);
        assert_eq!(mcts.best().0, Move::Pass);

        let only_eyes = game_from(
            "
            . X . X .
            X X X X X
            X X X X X
            X X X X X
            X X X X X",
            Color::Black,
            1,
        );
        mcts.start(&only_eyes);
        assert!(mcts.decided());
        assert_eq!(mcts.best().0, Move::Pass);
    }

    #[test]
    fn leaves_without_moves_are_not_expanded_again() {
        // After black's only move (capturing at (4,2)), white has only a
        // suicide or nothing: that leaf has no children, and must not be
        // rescanned on every later visit.
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
        let mut nodes = pool(1000);
        let mut mcts = Mcts::new(&mut nodes, 8);
        mcts.start(&game);
        assert!(!mcts.decided(), "a single move is still searched");
        mcts.step(200);
        assert_eq!(mcts.tree_size(), 2, "root plus its one child, nothing more");
        assert_eq!(mcts.best().0, Move::Play(p(4, 2)));
    }

    #[test]
    fn a_full_pool_only_stops_the_tree_growing() {
        let game = Box::new(Game::new(9));
        let mut nodes = pool(200);
        let mut mcts = Mcts::new(&mut nodes, 5);
        mcts.start(&game);
        mcts.step(500);
        assert!(mcts.tree_size() <= 200);
        assert_eq!(mcts.playouts(), 500);
        assert!(matches!(mcts.best().0, Move::Play(_)));
    }

    #[test]
    fn root_moves_respect_superko() {
        // The 2x2 cycle from the game tests: a1 is black's only empty
        // point, and playing it would repeat the position after move 1.
        // With no legal move left, the search must pass.
        let mut game = Box::new(Game::new(2));
        let (a2, b2, a1, b1) = (p(0, 0), p(1, 0), p(0, 1), p(1, 1));
        for m in [a1, b2, b1, a2, a1, b1] {
            game.play(m).unwrap();
        }
        let mut nodes = pool(1000);
        let mut mcts = Mcts::new(&mut nodes, 6);
        mcts.start(&game);
        mcts.step(100);
        assert_eq!(mcts.best().0, Move::Pass);
    }

    fn play_match(
        games: usize,
        mut mcts_move: impl FnMut(&Game) -> Move,
        mut other: impl FnMut(&Game) -> Move,
    ) -> usize {
        let mut wins = 0;
        for g in 0..games {
            let mut game = Box::new(Game::new(9));
            let mcts_color = if g % 2 == 0 {
                Color::Black
            } else {
                Color::White
            };
            while game.phase() == Phase::Playing && game.move_number() < 300 {
                let mv = if game.to_move() == mcts_color {
                    mcts_move(&game)
                } else {
                    other(&game)
                };
                match mv {
                    Move::Play(pt) => {
                        game.play(pt).unwrap();
                    }
                    Move::Pass => game.pass().unwrap(),
                }
            }
            if game.score().winner() == Some(mcts_color) {
                wins += 1;
            }
        }
        wins
    }

    /// Strength checks; run with
    /// `cargo test -p go-core --release mcts -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn mcts_beats_random_and_flat_mc() {
        let budget = 1500;
        let mut nodes = pool(1 << 18);
        let mut mcts = Mcts::new(&mut nodes, 1);
        let mut rng = Rng::new(2);
        let mut search = |game: &Game| {
            mcts.start(game);
            mcts.step(budget);
            mcts.best().0
        };
        let wins = play_match(10, &mut search, |g| random_bot(g, &mut rng));
        println!("MCTS won {wins}/10 against random");
        assert!(wins >= 9);

        let mut flat = FlatMc::new(3);
        let wins = play_match(10, &mut search, |g| {
            flat.start(g);
            flat.step(budget);
            flat.best().0
        });
        println!("MCTS won {wins}/10 against flat MC with the same playouts");
        assert!(wins >= 7);
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use crate::mc::FlatMc;

    /// `cargo test -p go-core --release mcts::bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn search_speed() {
        let game = Box::new(Game::new(9));
        let mut nodes = vec![Node::EMPTY; 1 << 18];
        let mut mcts = Mcts::new(&mut nodes, 1);
        mcts.start(&game);
        let t = std::time::Instant::now();
        mcts.step(5000);
        println!(
            "MCTS: {:?} per iteration, {} nodes",
            t.elapsed() / 5000,
            mcts.tree_size()
        );

        let mut flat = FlatMc::new(1);
        flat.start(&game);
        let t = std::time::Instant::now();
        flat.step(5000);
        println!("flat: {:?} per playout", t.elapsed() / 5000);
    }
}
