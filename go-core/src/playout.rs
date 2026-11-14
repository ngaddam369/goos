//! Random play, the building block of the computer opponent: a random legal
//! move that doesn't fill the player's own eyes, whole random games
//! ("playouts") from a position, and quick scoring of their final boards.

use crate::board::{Board, Cell, Color, MAX_SIZE, Point};
use crate::game::{Game, Move, Phase};
use crate::rng::Rng;

/// Whether the empty point `p` is a true-looking eye of `color`: every
/// neighbour is `color`, and the diagonals don't let the opponent break in
/// (at most one opponent diagonal in the middle of the board, none on the
/// edge or in the corner). Filling such a point only ever hurts.
pub fn is_eye(board: &Board, p: Point, color: Color) -> bool {
    if board.get(p) != Cell::Empty
        || board
            .neighbors(p)
            .any(|n| board.get(n) != Cell::Stone(color))
    {
        return false;
    }
    let size = board.size() as isize;
    let (col, row) = (p.col as isize, p.row as isize);
    let mut off_board = 0;
    let mut opponent = 0;
    for (dc, dr) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
        let (c, r) = (col + dc, row + dr);
        if c < 0 || r < 0 || c >= size || r >= size {
            off_board += 1;
        } else if board.get(Point::new(c as usize, r as usize)) == Cell::Stone(color.opponent()) {
            opponent += 1;
        }
    }
    if off_board > 0 {
        opponent == 0
    } else {
        opponent <= 1
    }
}

/// A uniformly random empty point for which `accept` holds, or `None`.
///
/// First a few tries at random points (rejection sampling: uniform over
/// the acceptable points, and fast while there are many of them). If those
/// all miss, lazy Fisher-Yates over the empty points: pick a random
/// candidate; if it's rejected, swap it out of the list and pick again, so
/// each point is tested at most once.
fn random_empty_point(
    board: &Board,
    rng: &mut Rng,
    mut accept: impl FnMut(Point) -> bool,
) -> Option<Point> {
    let size = board.size();
    for _ in 0..16 {
        let i = rng.below(size * size);
        let p = Point::new(i % size, i / size);
        if board.get(p) == Cell::Empty && accept(p) {
            return Some(p);
        }
    }
    let mut candidates = [Point::new(0, 0); MAX_SIZE * MAX_SIZE];
    let mut n = 0;
    for p in board.points() {
        if board.get(p) == Cell::Empty {
            candidates[n] = p;
            n += 1;
        }
    }
    while n > 0 {
        let i = rng.below(n);
        let p = candidates[i];
        if accept(p) {
            return Some(p);
        }
        n -= 1;
        candidates[i] = candidates[n];
    }
    None
}

/// A random move for `color` that is legal (ignoring superko, but not
/// retaking `ko`) and doesn't fill one of its own eyes. `None` means the
/// player should pass.
pub fn random_move(board: &Board, color: Color, ko: Option<Point>, rng: &mut Rng) -> Option<Point> {
    random_empty_point(board, rng, |p| {
        Some(p) != ko && board.is_legal(p, color) && !is_eye(board, p, color)
    })
}

/// Plays random moves from `board` (with `to_move` to play) until both
/// sides pass, or `max_moves` turns (moves and passes) have gone by.
/// Returns whether the game really finished; if not (e.g. an endless ko
/// fight), the final board shouldn't be scored.
pub fn playout(
    board: &mut Board,
    mut to_move: Color,
    mut ko: Option<Point>,
    rng: &mut Rng,
    max_moves: usize,
) -> bool {
    let mut passes = 0;
    for _ in 0..max_moves {
        match random_move(board, to_move, ko, rng) {
            Some(p) => {
                // `random_move` only returns legal moves.
                ko = board.play(p, to_move).map_or(None, |info| info.ko);
                passes = 0;
            }
            // Forced to pass only by the ko: the retake may be legal next
            // turn, so this pass doesn't count towards ending the game.
            None if ko.is_some() => ko = None,
            None => {
                passes += 1;
                if passes == 2 {
                    return true;
                }
            }
        }
        to_move = to_move.opponent();
    }
    false
}

/// A sensible `max_moves` for `playout`: long enough for any normal game.
pub fn playout_limit(board: &Board) -> usize {
    3 * board.size() * board.size()
}

/// Area score of a finished playout as (black, white): stones, plus empty
/// points whose neighbours are all one colour. When a playout ends, every
/// empty point is such an eye (any other point would still be a legal,
/// non-eye move for someone), so this matches full area scoring there and
/// is much cheaper.
pub fn playout_area(board: &Board) -> (u16, u16) {
    let mut area = [0u16; 2];
    for p in board.points() {
        let owner = match board.get(p) {
            Cell::Stone(c) => Some(c),
            Cell::Empty => {
                let mut colors = board.neighbors(p).map(|n| board.get(n));
                let first = colors.next();
                match first {
                    Some(Cell::Stone(c)) if colors.all(|x| x == Cell::Stone(c)) => Some(c),
                    _ => None,
                }
            }
        };
        if let Some(c) = owner {
            area[c as usize] += 1;
        }
    }
    (area[0], area[1])
}

/// The winner of a finished playout, given komi in half points.
pub fn playout_winner(board: &Board, komi_half: u16) -> Color {
    let (black, white) = playout_area(board);
    // With half-point komi there are no draws; with integer komi a draw
    // counts as a white win.
    if 2 * black as i32 > 2 * white as i32 + komi_half as i32 {
        Color::Black
    } else {
        Color::White
    }
}

/// Whether the player to move should just pass: the opponent has passed
/// and the board as it stands (all stones counted alive) is already a win.
pub fn should_pass(game: &Game) -> bool {
    let opponent_passed = matches!(game.last_move(), Some((_, Move::Pass)));
    opponent_passed && game.score().winner() == Some(game.to_move())
}

/// The simplest computer player: a random legal move (superko included)
/// that doesn't fill its own eyes, or a pass.
pub fn random_bot(game: &Game, rng: &mut Rng) -> Move {
    if game.phase() != Phase::Playing || should_pass(game) {
        return Move::Pass;
    }
    let board = game.board();
    let color = game.to_move();
    random_empty_point(board, rng, |p| {
        board.is_legal(p, color) && !is_eye(board, p, color) && game.check(p).is_ok()
    })
    .map_or(Move::Pass, Move::Play)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::diagram;

    const B: Color = Color::Black;
    const W: Color = Color::White;

    fn p(col: usize, row: usize) -> Point {
        Point::new(col, row)
    }

    #[test]
    fn recognises_eyes() {
        let board = diagram(
            "
            . X . X .
            X X X X X
            X . X . X
            X X O X X
            . . . . .",
        );
        assert!(is_eye(&board, p(0, 0), B), "corner eye");
        assert!(is_eye(&board, p(2, 0), B), "edge eye");
        assert!(
            is_eye(&board, p(1, 2), B),
            "one opponent diagonal is fine in the middle"
        );
        assert!(!is_eye(&board, p(4, 4), B), "not surrounded");
        assert!(!is_eye(&board, p(0, 0), W), "eyes belong to one colour");
        let board = diagram(
            "
            . . . . .
            . X O . .
            X . X . .
            . X O . .
            . . . . .",
        );
        assert!(
            !is_eye(&board, p(1, 2), B),
            "two opponent diagonals: false eye"
        );
    }

    #[test]
    fn random_moves_are_legal_and_never_fill_own_eyes() {
        let board = diagram(
            "
            . X . X O
            X X X X O
            O O O O O
            . . . . .
            . . . . .",
        );
        let mut rng = Rng::new(1);
        for _ in 0..200 {
            let m = random_move(&board, B, None, &mut rng).expect("black has moves");
            assert!(board.is_legal(m, B));
            assert!(m != p(0, 0) && m != p(2, 0), "black filled its own eye");
        }
    }

    #[test]
    fn random_move_respects_ko_and_passes_when_nothing_is_left() {
        let mut board = diagram(
            "
            . X O . .
            X O . O .
            . X O . .
            . . . . .
            . . . . .",
        );
        let ko = board.play(p(2, 1), B).unwrap().ko;
        let mut rng = Rng::new(3);
        for _ in 0..100 {
            assert_ne!(random_move(&board, W, ko, &mut rng), Some(p(1, 1)));
        }
        let only_eyes = diagram(
            "
            . X .
            X X X
            . X .",
        );
        assert_eq!(random_move(&only_eyes, B, None, &mut rng), None);
    }

    #[test]
    fn playouts_finish_with_only_eyes_left() {
        let mut rng = Rng::new(99);
        for _ in 0..50 {
            let mut board = Board::new(9);
            let limit = playout_limit(&board);
            assert!(
                playout(&mut board, B, None, &mut rng, limit),
                "playout should end by two passes"
            );
            // Neither side has a move left that isn't an own eye.
            assert_eq!(random_move(&board, B, None, &mut rng), None);
            assert_eq!(random_move(&board, W, None, &mut rng), None);
            // Every point is a stone or an eye.
            let (b, w) = playout_area(&board);
            assert_eq!(b + w, 81);
        }
    }

    #[test]
    fn playout_area_counts_stones_and_eyes() {
        let board = diagram(
            "
            . X O .
            X X O O
            X X O O
            . X O .",
        );
        assert_eq!(playout_area(&board), (8, 8));
        assert_eq!(playout_winner(&board, 15), W);
        assert_eq!(playout_winner(&board, 0), W, "a draw goes to white");
    }

    #[test]
    fn random_moves_are_uniform() {
        // Black stones fill most of the top row; the old "scan from a random
        // start" picked the point right after them far too often.
        let board = diagram(
            "
            X X X X .
            . . . . .
            . . . . .
            . . . . .
            . . . . .",
        );
        let mut rng = Rng::new(11);
        let mut counts = [0u32; 25];
        for _ in 0..21_000 {
            let m = random_move(&board, W, None, &mut rng).unwrap();
            counts[m.row as usize * 5 + m.col as usize] += 1;
        }
        // 21 empty points, ~1000 each.
        for (i, &c) in counts.iter().enumerate().skip(4) {
            assert!((850..1150).contains(&c), "point {i} picked {c} times");
        }
    }

    #[test]
    fn rare_moves_are_uniform_too() {
        // Only three acceptable points for white among many empty black
        // eyes, so the random tries nearly always miss and the fallback
        // picks the move.
        let board = diagram(
            "
            . X . X . X .
            X X X X X X X
            . X . X . X .
            X X X X X X X
            . X . X . X .
            X X X X X X X
            O O O . . . O",
        );
        let mut rng = Rng::new(12);
        let mut counts = [0u32; 3];
        for _ in 0..3000 {
            let m = random_move(&board, W, None, &mut rng).unwrap();
            assert_eq!(m.row, 6);
            counts[m.col as usize - 3] += 1;
        }
        assert!(
            counts.iter().all(|&c| (850..1150).contains(&c)),
            "{counts:?}"
        );
    }

    #[test]
    fn bot_passes_when_ahead_after_opponent_passed() {
        let mut game = Box::new(Game::new(5));
        let mut board = Board::new(5);
        for row in 0..5 {
            board.set(p(1, row), Cell::Stone(W));
        }
        game.set_position(board, B);
        let mut rng = Rng::new(1);
        game.pass().unwrap(); // black passes; white is far ahead
        assert_eq!(random_bot(&game, &mut rng), Move::Pass);
    }

    #[test]
    fn random_bot_respects_superko_and_passes_at_the_end() {
        let mut rng = Rng::new(5);
        let mut game = Box::new(Game::new(9));
        // Two random bots play a whole game: every move must be accepted.
        for _ in 0..400 {
            match random_bot(&game, &mut rng) {
                Move::Play(pt) => game.play(pt).expect("bot move must be legal"),
                Move::Pass => {
                    game.pass().unwrap();
                    if game.phase() != Phase::Playing {
                        break;
                    }
                    continue;
                }
            };
        }
        assert_eq!(
            game.phase(),
            Phase::Scoring,
            "the game should end by passing"
        );
    }

    /// Run with `cargo test -p go-core --release -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn playout_speed() {
        let mut rng = Rng::new(1);
        let start = std::time::Instant::now();
        let n = 2000;
        for _ in 0..n {
            let mut board = Board::new(9);
            playout(&mut board, B, None, &mut rng, 243);
        }
        let per = start.elapsed() / n;
        println!(
            "9x9 playout: {per:?} each, {:.0}/s",
            1.0 / per.as_secs_f64()
        );
    }
}
