//! Area (Chinese) scoring: each side's stones on the board plus the empty
//! regions bordered only by that side, with dead stones removed first.

use crate::board::{AREA, Board, Cell, Color, Point, PointSet};

#[derive(Clone, Copy, Debug)]
pub struct Score {
    pub black: u16,
    pub white: u16,
    pub komi_half: u16,
    /// Who owns each empty (or dead-stone) point as territory.
    owner: [Option<Color>; AREA],
}

impl Score {
    /// The territory owner of `p`, if it is empty or a dead stone.
    pub fn owner(&self, p: Point) -> Option<Color> {
        self.owner.get(p.index()).copied().flatten()
    }

    /// Black's lead in half points, komi included (negative: white leads).
    pub fn margin_half(&self) -> i32 {
        2 * self.black as i32 - 2 * self.white as i32 - self.komi_half as i32
    }

    /// The winner, or `None` for a draw (only possible with integer komi).
    pub fn winner(&self) -> Option<Color> {
        match self.margin_half() {
            m if m > 0 => Some(Color::Black),
            m if m < 0 => Some(Color::White),
            _ => None,
        }
    }
}

pub fn score(board: &Board, dead: &PointSet, komi_half: u16) -> Score {
    let mut board = *board;
    for p in dead.iter() {
        board.set(p, Cell::Empty);
    }

    let mut area = [0u16; 2];
    let mut owner = [None; AREA];
    let mut seen = PointSet::new();
    for p in board.points() {
        match board.get(p) {
            Cell::Stone(c) => area[c as usize] += 1,
            Cell::Empty if !seen.contains(p) => {
                let region = board.group(p);
                let mut borders = [false; 2];
                for q in region.iter() {
                    seen.insert(q);
                    for n in board.neighbors(q) {
                        if let Cell::Stone(c) = board.get(n) {
                            borders[c as usize] = true;
                        }
                    }
                }
                let region_owner = match borders {
                    [true, false] => Some(Color::Black),
                    [false, true] => Some(Color::White),
                    _ => None,
                };
                if let Some(c) = region_owner {
                    area[c as usize] += region.len() as u16;
                    for q in region.iter() {
                        owner[q.index()] = Some(c);
                    }
                }
            }
            Cell::Empty => {}
        }
    }
    Score {
        black: area[0],
        white: area[1],
        komi_half,
        owner,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::diagram;
    use crate::game::{DEFAULT_KOMI_HALF, Game};

    /// Black walls off columns A-D, white columns E-J; a white stone has
    /// been played inside black's area and is dead.
    fn finished_9x9() -> Board {
        diagram(
            "
            . . . X O . . . .
            . . . X O . . . .
            . . . X O . . . .
            . . . X O . . . .
            . O . X O . . . .
            . . . X O . . . .
            . . . X O . . . .
            . . . X O . . . .
            . . . X O . . . .",
        )
    }

    #[test]
    fn scores_a_finished_9x9_game() {
        let mut game = Box::new(Game::new(9));
        game.set_position(finished_9x9(), Color::Black);
        game.pass().unwrap();
        game.pass().unwrap();

        // Unmarked, the white stone is alive and black's area has no
        // territory (the region touches white): 9 black stones; white has
        // 10 stones + 36 territory.
        let s = game.score();
        assert_eq!((s.black, s.white), (9, 46));

        // Marked dead, black owns columns A-D: 4 * 9 = 36 points. White has
        // E-J: 5 * 9 = 45. With 7.5 komi white wins by 16.5.
        assert!(game.toggle_dead(Point::new(1, 4)));
        let s = game.score();
        assert_eq!((s.black, s.white), (36, 45));
        assert_eq!(s.komi_half, DEFAULT_KOMI_HALF);
        assert_eq!(s.margin_half(), -33);
        assert_eq!(s.winner(), Some(Color::White));
        assert_eq!(
            s.owner(Point::new(1, 4)),
            Some(Color::Black),
            "dead stone's point is black territory"
        );
        assert_eq!(s.owner(Point::new(8, 0)), Some(Color::White));
        assert_eq!(s.owner(Point::new(3, 0)), None, "stones are not territory");
    }

    #[test]
    fn neutral_points_count_for_nobody() {
        let board = diagram(
            "
            X . O
            X . O
            X . O",
        );
        let s = score(&board, &PointSet::new(), 0);
        assert_eq!((s.black, s.white), (3, 3));
        assert_eq!(s.winner(), None);
    }
}
