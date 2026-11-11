//! A game in progress: whose turn it is, passes, undo history, superko and
//! the phases play → scoring (marking dead stones) → finished.

use crate::board::{Board, Color, IllegalMove, MoveInfo, Point, PointSet};
use crate::score::{Score, score};

/// Moves remembered for undo and superko. If a game runs longer, the oldest
/// positions are forgotten.
const HISTORY: usize = 1024;

/// 7.5 points, stored in half points to avoid floats.
pub const DEFAULT_KOMI_HALF: u16 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Play(Point),
    Pass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Playing,
    /// Both players passed; dead stones are being marked.
    Scoring,
    /// The score was confirmed.
    Finished,
}

/// Everything a move changes, saved before the move so undo can restore it.
#[derive(Clone, Copy)]
struct Snapshot {
    board: Board,
    to_move: Color,
    passes: u8,
    last: Option<(Color, Move)>,
    move_number: u16,
    ko: Option<Point>,
}

pub struct Game {
    board: Board,
    to_move: Color,
    /// Consecutive passes; two end the play phase.
    passes: u8,
    last: Option<(Color, Move)>,
    move_number: u16,
    /// Where the player to move may not retake a ko right now.
    ko: Option<Point>,
    phase: Phase,
    dead: PointSet,
    komi_half: u16,
    history: [Snapshot; HISTORY],
    history_len: usize,
}

impl Game {
    /// A new game. This is a large value (the undo history), so the kernel
    /// keeps one in a static and uses `reset` rather than building new ones.
    pub const fn new(size: usize) -> Game {
        let board = Board::new(size);
        Game {
            board,
            to_move: Color::Black,
            passes: 0,
            last: None,
            move_number: 0,
            ko: None,
            phase: Phase::Playing,
            dead: PointSet::new(),
            komi_half: DEFAULT_KOMI_HALF,
            history: [Snapshot { board, to_move: Color::Black, passes: 0, last: None, move_number: 0, ko: None }; HISTORY],
            history_len: 0,
        }
    }

    /// Starts a new game of the given size, in place.
    pub fn reset(&mut self, size: usize) {
        self.set_position(Board::new(size), Color::Black);
    }

    /// Starts from an arbitrary position (handicap stones, tests).
    pub fn set_position(&mut self, board: Board, to_move: Color) {
        self.board = board;
        self.to_move = to_move;
        self.passes = 0;
        self.last = None;
        self.move_number = 0;
        self.ko = None;
        self.phase = Phase::Playing;
        self.dead = PointSet::new();
        self.history_len = 0;
    }

    pub fn board(&self) -> &Board {
        &self.board
    }

    pub fn size(&self) -> usize {
        self.board.size()
    }

    pub fn to_move(&self) -> Color {
        self.to_move
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The last move and who made it.
    pub fn last_move(&self) -> Option<(Color, Move)> {
        self.last
    }

    /// Moves (including passes) made so far.
    pub fn move_number(&self) -> u16 {
        self.move_number
    }

    pub fn captures(&self, color: Color) -> u16 {
        self.board.captures(color)
    }

    /// Consecutive passes so far (reset by a move or by resuming play).
    pub fn passes(&self) -> u8 {
        self.passes
    }

    /// The point the player to move may not play because it would retake
    /// a ko immediately (superko catches longer cycles via `check`).
    pub fn ko(&self) -> Option<Point> {
        self.ko
    }

    pub fn komi_half(&self) -> u16 {
        self.komi_half
    }

    /// Sets komi in half points (15 = 7.5).
    pub fn set_komi_half(&mut self, komi_half: u16) {
        self.komi_half = komi_half;
    }

    pub fn is_dead(&self, p: Point) -> bool {
        self.dead.contains(p)
    }

    pub fn play(&mut self, p: Point) -> Result<MoveInfo, IllegalMove> {
        let (next, info) = self.try_play(p)?;
        self.save_snapshot();
        self.board = next;
        self.passes = 0;
        self.ko = info.ko;
        self.finish_turn(Move::Play(p));
        Ok(info)
    }

    /// Whether the player to move may play at `p`, without playing it.
    pub fn check(&self, p: Point) -> Result<(), IllegalMove> {
        self.try_play(p).map(|_| ())
    }

    /// The position after playing `p`, if that's legal.
    fn try_play(&self, p: Point) -> Result<(Board, MoveInfo), IllegalMove> {
        if self.phase != Phase::Playing {
            return Err(IllegalMove::NotPlaying);
        }
        let mut next = self.board;
        let info = next.play(p, self.to_move)?;

        // Positional superko: the new position may not repeat any earlier
        // one. The most recent snapshot is the position before the
        // opponent's last move, so matching it is a plain ko.
        let earlier = &self.history[..self.history_len];
        let repeats = |s: &Snapshot| s.board.hash() == next.hash() && s.board.same_stones(&next);
        if let Some(i) = earlier.iter().rposition(repeats) {
            return Err(if i + 1 == self.history_len { IllegalMove::Ko } else { IllegalMove::Superko });
        }
        Ok((next, info))
    }

    pub fn pass(&mut self) -> Result<(), IllegalMove> {
        if self.phase != Phase::Playing {
            return Err(IllegalMove::NotPlaying);
        }
        self.save_snapshot();
        self.passes += 1;
        self.ko = None;
        self.finish_turn(Move::Pass);
        if self.passes >= 2 {
            self.phase = Phase::Scoring;
            self.dead = PointSet::new();
        }
        Ok(())
    }

    fn finish_turn(&mut self, mv: Move) {
        self.last = Some((self.to_move, mv));
        self.to_move = self.to_move.opponent();
        self.move_number += 1;
    }

    fn save_snapshot(&mut self) {
        if self.history_len == HISTORY {
            self.history.copy_within(1.., 0);
            self.history_len -= 1;
        }
        self.history[self.history_len] = Snapshot {
            board: self.board,
            to_move: self.to_move,
            passes: self.passes,
            last: self.last,
            move_number: self.move_number,
            ko: self.ko,
        };
        self.history_len += 1;
    }

    /// Takes back the last move or pass (also leaving the scoring phase).
    /// After the score was confirmed, it goes back to marking dead stones
    /// instead. Returns false if there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        if self.phase == Phase::Finished {
            self.phase = Phase::Scoring;
            return true;
        }
        if self.history_len == 0 {
            return false;
        }
        self.history_len -= 1;
        let s = self.history[self.history_len];
        self.board = s.board;
        self.to_move = s.to_move;
        self.passes = s.passes;
        self.last = s.last;
        self.move_number = s.move_number;
        self.ko = s.ko;
        self.phase = Phase::Playing;
        self.dead = PointSet::new();
        true
    }

    /// Leaves the scoring phase and continues playing.
    pub fn resume(&mut self) {
        if self.phase == Phase::Scoring {
            self.phase = Phase::Playing;
            self.passes = 0;
            self.ko = None;
            self.dead = PointSet::new();
        }
    }

    /// During scoring, marks the whole group at `p` dead, or alive again.
    /// Returns false if there's no stone there or we're not scoring.
    pub fn toggle_dead(&mut self, p: Point) -> bool {
        if self.phase != Phase::Scoring || self.board.get(p) == crate::Cell::Empty {
            return false;
        }
        let group = self.board.group(p);
        let mark = !self.dead.contains(p);
        for q in group.iter() {
            if mark { self.dead.insert(q) } else { self.dead.remove(q) }
        }
        true
    }

    /// Area score of the current position, with marked stones removed.
    pub fn score(&self) -> Score {
        score(&self.board, &self.dead, self.komi_half)
    }

    /// Confirms the score and ends the game.
    pub fn finish(&mut self) {
        if self.phase == Phase::Scoring {
            self.phase = Phase::Finished;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{Cell, diagram};

    const B: Color = Color::Black;
    const W: Color = Color::White;

    fn p(col: usize, row: usize) -> Point {
        Point::new(col, row)
    }

    /// A ko on a 5x5 board: black to capture at (2,1).
    fn ko_game() -> Box<Game> {
        let mut game = Box::new(Game::new(5));
        game.set_position(
            diagram(
                "
                . X O . .
                X O . O .
                . X O . .
                . . . . .
                . . . . .",
            ),
            B,
        );
        game
    }

    #[test]
    fn simple_ko_rejected_then_allowed_after_a_move_elsewhere() {
        let mut game = ko_game();
        assert_eq!(game.play(p(2, 1)).unwrap().captured, 1);
        assert_eq!(game.ko(), Some(p(1, 1)));
        // White may not retake straight away.
        assert_eq!(game.play(p(1, 1)), Err(IllegalMove::Ko));
        assert_eq!(game.to_move(), W, "a rejected move doesn't change the turn");
        // A ko threat and answer elsewhere change the position...
        game.play(p(4, 4)).unwrap();
        game.play(p(0, 4)).unwrap();
        // ...so now the retake is legal.
        assert_eq!(game.play(p(1, 1)).unwrap().captured, 1);
    }

    #[test]
    fn superko_rejects_older_repetition() {
        // On a 2x2 board this sequence returns to the position after move 1
        // six moves later, which plain ko (two moves back) wouldn't catch.
        let (a2, b2, a1, b1) = (p(0, 0), p(1, 0), p(0, 1), p(1, 1));
        let mut game = Box::new(Game::new(2));
        game.play(a1).unwrap(); // B
        game.play(b2).unwrap(); // W
        game.play(b1).unwrap(); // B
        assert_eq!(game.play(a2).unwrap().captured, 2); // W captures a1, b1
        game.play(a1).unwrap(); // B
        assert_eq!(game.play(b1).unwrap().captured, 1); // W captures a1
        // Black at a1 would capture three stones and leave just a black
        // stone at a1: the same position as after move 1.
        assert_eq!(game.play(a1), Err(IllegalMove::Superko));
    }

    #[test]
    fn undo_restores_board_captures_and_turn() {
        let mut game = ko_game();
        let board_before = *game.board();
        game.play(p(2, 1)).unwrap();
        assert_eq!(game.captures(B), 1);
        assert_eq!(game.to_move(), W);
        assert_eq!(game.move_number(), 1);

        assert!(game.undo());
        assert_eq!(game.ko(), None);
        assert_eq!(*game.board(), board_before);
        assert_eq!(game.captures(B), 0);
        assert_eq!(game.to_move(), B);
        assert_eq!(game.move_number(), 0);
        assert_eq!(game.last_move(), None);
        assert!(!game.undo(), "nothing left to undo");
    }

    #[test]
    fn two_passes_start_scoring_and_resume_continues() {
        let mut game = Box::new(Game::new(9));
        game.play(p(4, 4)).unwrap();
        game.pass().unwrap();
        assert_eq!(game.phase(), Phase::Playing);
        assert_eq!(game.last_move(), Some((W, Move::Pass)));
        game.pass().unwrap();
        assert_eq!(game.phase(), Phase::Scoring);
        assert_eq!(game.play(p(0, 0)), Err(IllegalMove::NotPlaying));

        game.resume();
        assert_eq!(game.phase(), Phase::Playing);
        game.pass().unwrap();
        assert_eq!(game.phase(), Phase::Playing, "the pass count restarts after resuming");
    }

    #[test]
    fn undo_leaves_scoring() {
        let mut game = Box::new(Game::new(9));
        game.pass().unwrap();
        game.pass().unwrap();
        assert!(game.undo());
        assert_eq!(game.phase(), Phase::Playing);
        assert_eq!(game.to_move(), W);
    }

    #[test]
    fn undo_after_finishing_returns_to_scoring() {
        let mut game = Box::new(Game::new(5));
        game.play(p(2, 2)).unwrap();
        game.pass().unwrap();
        game.pass().unwrap();
        game.toggle_dead(p(2, 2));
        game.finish();
        assert_eq!(game.phase(), Phase::Finished);
        assert!(game.undo());
        assert_eq!(game.phase(), Phase::Scoring);
        assert!(game.is_dead(p(2, 2)), "dead marks survive");
    }

    #[test]
    fn toggling_dead_marks_the_whole_group() {
        let mut game = Box::new(Game::new(5));
        let mut board = Board::new(5);
        board.set(p(1, 1), Cell::Stone(W));
        board.set(p(2, 1), Cell::Stone(W));
        game.set_position(board, B);
        assert!(!game.toggle_dead(p(1, 1)), "only during scoring");
        game.pass().unwrap();
        game.pass().unwrap();
        assert!(game.toggle_dead(p(1, 1)));
        assert!(game.is_dead(p(2, 1)));
        assert!(game.toggle_dead(p(2, 1)));
        assert!(!game.is_dead(p(1, 1)));
        assert!(!game.toggle_dead(p(4, 4)), "empty point");
    }
}
