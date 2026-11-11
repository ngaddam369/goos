//! Board state: stones, captures, liberties and move legality (except
//! superko, which needs the game history and lives in `game.rs`).

use crate::zobrist::KEYS;

pub const MAX_SIZE: usize = 19;
/// Cells are stored with a fixed row stride of `MAX_SIZE`, whatever the
/// actual board size, so a point's index never depends on the size.
pub(crate) const AREA: usize = MAX_SIZE * MAX_SIZE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Black,
    White,
}

impl Color {
    pub const fn opponent(self) -> Color {
        match self {
            Color::Black => Color::White,
            Color::White => Color::Black,
        }
    }

    pub(crate) const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cell {
    #[default]
    Empty,
    Stone(Color),
}

/// An intersection. Row 0 is the top row (Go's highest-numbered line).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub col: u8,
    pub row: u8,
}

impl Point {
    pub const fn new(col: usize, row: usize) -> Point {
        debug_assert!(col < MAX_SIZE && row < MAX_SIZE);
        Point { col: col as u8, row: row as u8 }
    }

    pub(crate) const fn index(self) -> usize {
        self.row as usize * MAX_SIZE + self.col as usize
    }

    pub(crate) const fn from_index(i: usize) -> Point {
        Point::new(i % MAX_SIZE, i / MAX_SIZE)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IllegalMove {
    OutOfBounds,
    Occupied,
    Suicide,
    /// Retaking a ko immediately (the position from two moves ago).
    Ko,
    /// Recreating any other earlier position (positional superko).
    Superko,
    /// The game isn't in the play phase.
    NotPlaying,
}

impl IllegalMove {
    /// A short explanation for the player.
    pub const fn message(self) -> &'static str {
        match self {
            IllegalMove::OutOfBounds => "Off the board",
            IllegalMove::Occupied => "Occupied",
            IllegalMove::Suicide => "Suicide",
            IllegalMove::Ko => "Ko",
            IllegalMove::Superko => "Superko (repeats a position)",
            IllegalMove::NotPlaying => "Game is not in play",
        }
    }
}

/// What a legal move did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveInfo {
    pub captured: u16,
}

/// A set of points, as a bitset over point indices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PointSet([u64; AREA.div_ceil(64)]);

impl PointSet {
    pub const fn new() -> Self {
        PointSet([0; AREA.div_ceil(64)])
    }

    pub fn contains(&self, p: Point) -> bool {
        self.contains_index(p.index())
    }

    fn contains_index(&self, i: usize) -> bool {
        i < AREA && self.0[i / 64] & (1 << (i % 64)) != 0
    }

    pub fn insert(&mut self, p: Point) {
        self.insert_index(p.index());
    }

    fn insert_index(&mut self, i: usize) {
        self.0[i / 64] |= 1 << (i % 64);
    }

    pub fn remove(&mut self, p: Point) {
        let i = p.index();
        self.0[i / 64] &= !(1 << (i % 64));
    }

    pub fn len(&self) -> usize {
        self.0.iter().map(|w| w.count_ones() as usize).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|&w| w == 0)
    }

    pub fn iter(&self) -> impl Iterator<Item = Point> + '_ {
        self.0.iter().enumerate().flat_map(|(w, &word)| {
            let mut bits = word;
            core::iter::from_fn(move || {
                if bits == 0 {
                    return None;
                }
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                Some(Point::from_index(w * 64 + bit))
            })
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Board {
    size: u8,
    cells: [Cell; AREA],
    /// Zobrist hash of the stones on the board.
    hash: u64,
    /// Stones captured by black and by white.
    captures: [u16; 2],
}

impl Board {
    /// An empty board. Sizes 1..=19 work; the game uses 9, 13 and 19.
    pub const fn new(size: usize) -> Board {
        assert!(size >= 1 && size <= MAX_SIZE);
        Board { size: size as u8, cells: [Cell::Empty; AREA], hash: 0, captures: [0; 2] }
    }

    pub const fn size(&self) -> usize {
        self.size as usize
    }

    pub fn contains(&self, p: Point) -> bool {
        (p.col as usize) < self.size() && (p.row as usize) < self.size()
    }

    /// The cell at `p` (empty if `p` is off the board).
    pub fn get(&self, p: Point) -> Cell {
        if self.contains(p) { self.cells[p.index()] } else { Cell::Empty }
    }

    pub fn hash(&self) -> u64 {
        self.hash
    }

    /// Number of stones `color` has captured.
    pub fn captures(&self, color: Color) -> u16 {
        self.captures[color.index()]
    }

    /// All points of the board, row by row.
    pub fn points(&self) -> impl Iterator<Item = Point> + use<> {
        let size = self.size();
        (0..size * size).map(move |i| Point::new(i % size, i / size))
    }

    /// The up-to-four on-board neighbours of `p`.
    pub fn neighbors(&self, p: Point) -> impl Iterator<Item = Point> + use<> {
        let (col, row, size) = (p.col as isize, p.row as isize, self.size() as isize);
        [(col - 1, row), (col + 1, row), (col, row - 1), (col, row + 1)]
            .into_iter()
            .filter(move |&(c, r)| c >= 0 && r >= 0 && c < size && r < size)
            .map(|(c, r)| Point::new(c as usize, r as usize))
    }

    /// Puts a stone down (or removes one) without any rule checks.
    /// Useful for setting up positions. Off-board points are ignored.
    pub fn set(&mut self, p: Point, cell: Cell) {
        if !self.contains(p) {
            return;
        }
        let i = p.index();
        if let Cell::Stone(c) = self.cells[i] {
            self.hash ^= KEYS[i][c.index()];
        }
        if let Cell::Stone(c) = cell {
            self.hash ^= KEYS[i][c.index()];
        }
        self.cells[i] = cell;
    }

    /// True if both boards have the same stones (ignoring capture counts).
    pub fn same_stones(&self, other: &Board) -> bool {
        self.size == other.size && self.cells == other.cells
    }

    /// The group (chain of same-coloured stones, or connected empty region)
    /// containing `p` (empty if `p` is off the board).
    pub fn group(&self, p: Point) -> PointSet {
        self.flood(p).0
    }

    /// Number of distinct liberties of the group containing the stone at `p`.
    pub fn liberties(&self, p: Point) -> usize {
        self.flood(p).1
    }

    /// Flood fill from `p` over cells equal to `p`'s cell. Returns the group
    /// and the number of distinct empty points next to it.
    fn flood(&self, p: Point) -> (PointSet, usize) {
        let mut group = PointSet::new();
        let mut liberties = PointSet::new();
        if !self.contains(p) {
            return (group, 0);
        }
        let target = self.get(p);
        let mut stack = [0u16; AREA];
        let mut top = 0;
        group.insert(p);
        stack[top] = p.index() as u16;
        top += 1;
        while top > 0 {
            top -= 1;
            let here = Point::from_index(stack[top] as usize);
            for n in self.neighbors(here) {
                let cell = self.get(n);
                if cell == target {
                    if !group.contains(n) {
                        group.insert(n);
                        stack[top] = n.index() as u16;
                        top += 1;
                    }
                } else if cell == Cell::Empty {
                    liberties.insert(n);
                }
            }
        }
        (group, liberties.len())
    }

    fn remove_group(&mut self, group: &PointSet) -> u16 {
        let mut removed = 0;
        for p in group.iter() {
            self.set(p, Cell::Empty);
            removed += 1;
        }
        removed
    }

    /// Plays a stone, removing captured opponent groups. Superko is checked
    /// by `Game`, which knows the history. On error the board is unchanged.
    pub fn play(&mut self, p: Point, color: Color) -> Result<MoveInfo, IllegalMove> {
        if !self.contains(p) {
            return Err(IllegalMove::OutOfBounds);
        }
        if self.get(p) != Cell::Empty {
            return Err(IllegalMove::Occupied);
        }
        let mut next = *self;
        next.set(p, Cell::Stone(color));

        let mut captured = 0;
        for n in self.neighbors(p) {
            if next.get(n) == Cell::Stone(color.opponent()) {
                let (group, liberties) = next.flood(n);
                if liberties == 0 {
                    captured += next.remove_group(&group);
                }
            }
        }
        if next.liberties(p) == 0 {
            return Err(IllegalMove::Suicide);
        }
        next.captures[color.index()] += captured;
        *self = next;
        Ok(MoveInfo { captured })
    }
}

/// Builds a board from a diagram such as `". X O"` lines (X = black,
/// O = white, . = empty); the number of lines is the board size.
#[cfg(test)]
pub(crate) fn diagram(text: &str) -> Board {
    let rows: Vec<Vec<char>> = text
        .lines()
        .map(|l| l.chars().filter(|c| !c.is_whitespace()).collect::<Vec<_>>())
        .filter(|r| !r.is_empty())
        .collect();
    let mut board = Board::new(rows.len());
    for (row, cells) in rows.iter().enumerate() {
        assert_eq!(cells.len(), rows.len(), "diagram must be square");
        for (col, ch) in cells.iter().enumerate() {
            let cell = match ch {
                'X' => Cell::Stone(Color::Black),
                'O' => Cell::Stone(Color::White),
                '.' => Cell::Empty,
                other => panic!("bad diagram character {other:?}"),
            };
            board.set(Point::new(col, row), cell);
        }
    }
    board
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: Color = Color::Black;
    const W: Color = Color::White;

    fn p(col: usize, row: usize) -> Point {
        Point::new(col, row)
    }

    #[test]
    fn single_stone_capture() {
        let mut board = diagram(
            "
            . X .
            X O .
            . X .",
        );
        let info = board.play(p(2, 1), B).unwrap();
        assert_eq!(info.captured, 1);
        assert_eq!(board.get(p(1, 1)), Cell::Empty);
        assert_eq!(board.captures(B), 1);
        assert_eq!(board.captures(W), 0);
    }

    #[test]
    fn multi_stone_capture() {
        let mut board = diagram(
            "
            . . . . .
            . X X X .
            X O O O .
            . X X X .
            . . . . .",
        );
        let info = board.play(p(4, 2), B).unwrap();
        assert_eq!(info.captured, 3);
        for col in 1..=3 {
            assert_eq!(board.get(p(col, 2)), Cell::Empty);
        }
        assert_eq!(board.captures(B), 3);
    }

    #[test]
    fn capture_on_edge() {
        let mut board = diagram(
            "
            X O O . .
            . X X . .
            . . . . .
            . . . . .
            . . . . .",
        );
        assert_eq!(board.liberties(p(1, 0)), 1);
        assert_eq!(board.play(p(3, 0), B).unwrap().captured, 2);
        assert_eq!(board.get(p(1, 0)), Cell::Empty);
        assert_eq!(board.get(p(2, 0)), Cell::Empty);
    }

    #[test]
    fn capture_in_corner() {
        let mut board = diagram(
            "
            O X .
            . . .
            . . .",
        );
        assert_eq!(board.play(p(0, 1), B).unwrap().captured, 1);
        assert_eq!(board.get(p(0, 0)), Cell::Empty);
    }

    #[test]
    fn suicide_is_rejected() {
        let mut board = diagram(
            "
            . X .
            X . X
            . X .",
        );
        let before = board;
        assert_eq!(board.play(p(1, 1), W), Err(IllegalMove::Suicide));
        assert_eq!(board, before, "a rejected move leaves the board unchanged");

        // Multi-stone suicide: filling the last liberty of one's own group.
        let mut board = diagram(
            "
            O O X
            . X .
            X . .",
        );
        let before = board;
        assert_eq!(board.play(p(0, 1), W), Err(IllegalMove::Suicide));
        assert_eq!(board, before);
    }

    #[test]
    fn move_that_looks_like_suicide_but_captures_is_legal() {
        // White at (1,1) is surrounded by black, but it takes the last
        // liberty of the black stone at (2,1), which is captured first.
        let mut board = diagram(
            "
            . X O .
            X . X O
            . X O .
            . . . .",
        );
        let info = board.play(p(1, 1), W).unwrap();
        assert_eq!(info.captured, 1);
        assert_eq!(board.get(p(2, 1)), Cell::Empty);
        assert_eq!(board.liberties(p(1, 1)), 1);
    }

    #[test]
    fn occupied_and_out_of_bounds() {
        let mut board = Board::new(9);
        board.play(p(4, 4), B).unwrap();
        assert_eq!(board.play(p(4, 4), W), Err(IllegalMove::Occupied));
        assert_eq!(board.play(p(9, 0), W), Err(IllegalMove::OutOfBounds));
    }

    #[test]
    fn off_board_points_are_harmless() {
        let mut board = Board::new(9);
        board.set(p(12, 3), Cell::Stone(B));
        assert_eq!(board, Board::new(9));
        assert!(board.group(p(12, 3)).is_empty());
        assert_eq!(board.liberties(p(18, 18)), 0);
    }

    #[test]
    fn hash_tracks_position() {
        let mut a = Board::new(9);
        let mut b = Board::new(9);
        a.play(p(2, 2), B).unwrap();
        a.play(p(6, 6), W).unwrap();
        b.play(p(6, 6), W).unwrap();
        b.play(p(2, 2), B).unwrap();
        assert_eq!(a.hash(), b.hash());
        assert_ne!(a.hash(), Board::new(9).hash());
    }

    #[test]
    fn point_set_iterates_in_order() {
        let mut set = PointSet::new();
        for pt in [p(18, 18), p(0, 0), p(5, 3)] {
            set.insert(pt);
        }
        let got: Vec<Point> = set.iter().collect();
        assert_eq!(got, vec![p(0, 0), p(5, 3), p(18, 18)]);
        set.remove(p(0, 0));
        assert_eq!(set.len(), 2);
    }
}
