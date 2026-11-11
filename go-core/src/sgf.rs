//! Saving and loading games as SGF (Smart Game Format, FF[4]) text.
//!
//! Only what GoOS needs: board size, komi, setup stones, the player to
//! move, the moves of the main line (variations and unknown properties are
//! skipped) and, for finished games, the result.

use core::fmt::{self, Write};

use crate::board::{Board, Cell, Color, IllegalMove, MAX_SIZE, Point};
use crate::game::{Game, Move, Phase};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SgfError {
    /// Not well-formed SGF (or no game tree at all).
    Syntax,
    /// SZ is missing a number or is outside 1..=19 (or not square).
    Size,
    /// A move or setup point that isn't on the board.
    Coordinate,
    /// Setup stones after the first move, which GoOS can't represent.
    Unsupported,
    /// Move `n` (counting from 1) was by the wrong player.
    OutOfTurn(u16),
    /// Move `n` was illegal.
    Illegal(u16, IllegalMove),
}

impl SgfError {
    pub const fn message(self) -> &'static str {
        match self {
            SgfError::Syntax => "not valid SGF",
            SgfError::Size => "unsupported board size",
            SgfError::Coordinate => "point off the board",
            SgfError::Unsupported => "setup or size change mid-game",
            SgfError::OutOfTurn(_) => "move out of turn",
            SgfError::Illegal(..) => "illegal move",
        }
    }
}

// ---------------------------------------------------------------------------
// Writing

fn write_point(out: &mut impl Write, p: Point) -> fmt::Result {
    out.write_char((b'a' + p.col) as char)?;
    out.write_char((b'a' + p.row) as char)
}

fn write_half_points(out: &mut impl Write, half: i32) -> fmt::Result {
    let sign = if half < 0 { "-" } else { "" };
    let half = half.abs();
    if half % 2 == 0 { write!(out, "{sign}{}", half / 2) } else { write!(out, "{sign}{}.5", half / 2) }
}

fn color_letter(c: Color) -> char {
    match c {
        Color::Black => 'B',
        Color::White => 'W',
    }
}

/// Writes `game` as an SGF game tree. A finished game gets its result.
pub fn write_sgf(game: &Game, out: &mut impl Write) -> fmt::Result {
    write!(out, "(;GM[1]FF[4]CA[UTF-8]AP[GoOS]RU[Chinese]SZ[{}]KM[", game.size())?;
    write_half_points(out, game.komi_half() as i32)?;
    out.write_char(']')?;

    let (start, to_move) = game.start_position();
    let mut setup = false;
    for (prop, color) in [("AB", Color::Black), ("AW", Color::White)] {
        let mut first = true;
        for p in start.points().filter(|&p| start.get(p) == Cell::Stone(color)) {
            if first {
                out.write_str(prop)?;
                first = false;
            }
            out.write_char('[')?;
            write_point(out, p)?;
            out.write_char(']')?;
            setup = true;
        }
    }
    if setup || to_move == Color::White {
        write!(out, "PL[{}]", color_letter(to_move))?;
    }
    if game.phase() == Phase::Finished {
        let score = game.score();
        out.write_str("RE[")?;
        match score.winner() {
            None => out.write_str("0")?,
            Some(w) => {
                write!(out, "{}+", color_letter(w))?;
                write_half_points(out, score.margin_half().abs())?;
            }
        }
        out.write_char(']')?;
    }

    for (i, (color, mv)) in game.moves().enumerate() {
        if i % 12 == 0 {
            out.write_char('\n')?;
        }
        write!(out, ";{}[", color_letter(color))?;
        if let Move::Play(p) = mv {
            write_point(out, p)?;
        }
        out.write_char(']')?;
    }
    out.write_str(")\n")
}

// ---------------------------------------------------------------------------
// Reading

/// Calls `property(ident, value)` for each property value on the main line
/// (the first variation at every branch), in order.
fn parse(text: &[u8], mut property: impl FnMut(&[u8], &[u8]) -> Result<(), SgfError>) -> Result<(), SgfError> {
    let mut i = text.iter().position(|&c| c == b'(').ok_or(SgfError::Syntax)? + 1;
    let mut ident_start = 0;
    let mut ident_end = 0;
    loop {
        let &c = text.get(i).ok_or(SgfError::Syntax)?;
        match c {
            // A new variation: the main line continues into the first one.
            b'(' | b';' => {
                i += 1;
                ident_end = ident_start;
            }
            // End of the first (deepest) variation: the main line is done.
            b')' => return Ok(()),
            c if c.is_ascii_whitespace() => i += 1,
            c if c.is_ascii_alphabetic() => {
                // Property identifiers are upper-case; FF[3] allowed
                // lower-case letters mixed in, which are ignored.
                ident_start = i;
                while text.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
                    i += 1;
                }
                ident_end = i;
            }
            b'[' => {
                if ident_end == ident_start {
                    return Err(SgfError::Syntax); // a value without a property
                }
                let start = i + 1;
                i = start;
                loop {
                    match text.get(i) {
                        None => return Err(SgfError::Syntax),
                        Some(b'\\') => i += 2,
                        Some(b']') => break,
                        Some(_) => i += 1,
                    }
                }
                let ident = &text[ident_start..ident_end];
                property(ident, &text[start..i])?;
                i += 1;
            }
            _ => return Err(SgfError::Syntax),
        }
    }
}

/// The upper-case letters of a property identifier (FF[3] style
/// identifiers like `AddBlack` become `AB`).
fn ident_is(ident: &[u8], name: &str) -> bool {
    ident.iter().filter(|c| c.is_ascii_uppercase()).eq(name.as_bytes().iter())
}

/// "19", or the FF[4] "columns:rows" form when the board is square.
fn parse_size(value: &[u8]) -> Result<usize, SgfError> {
    let text = core::str::from_utf8(value).map_err(|_| SgfError::Size)?.trim();
    let (cols, rows) = text.split_once(':').unwrap_or((text, text));
    let cols: usize = cols.trim().parse().map_err(|_| SgfError::Size)?;
    let rows: usize = rows.trim().parse().map_err(|_| SgfError::Size)?;
    if cols == rows && (1..=MAX_SIZE).contains(&cols) { Ok(cols) } else { Err(SgfError::Size) }
}

/// Komi such as "7.5", "6", "0.5" or "-3" in half points (negative komi is
/// not supported and becomes 0).
fn parse_komi(value: &[u8]) -> u16 {
    let text = core::str::from_utf8(value).unwrap_or("").trim();
    let (int, frac) = text.split_once('.').unwrap_or((text, ""));
    let Ok(int) = int.parse::<i32>() else { return 0 };
    let half = frac.bytes().next().is_some_and(|d| d >= b'5');
    let value = int * 2 + if int < 0 || text.starts_with('-') { -(half as i32) } else { half as i32 };
    value.clamp(0, u16::MAX as i32) as u16
}

/// A point like "dd", or `None` for a pass ("" or, on boards up to 19x19,
/// the old "tt").
fn parse_point(value: &[u8], size: usize) -> Result<Option<Point>, SgfError> {
    match value {
        [] => Ok(None),
        b"tt" if size <= 19 => Ok(None),
        &[c, r] if c.is_ascii_lowercase() && r.is_ascii_lowercase() => {
            let (col, row) = ((c - b'a') as usize, (r - b'a') as usize);
            if col < size && row < size { Ok(Some(Point::new(col, row))) } else { Err(SgfError::Coordinate) }
        }
        _ => Err(SgfError::Coordinate),
    }
}

/// Setup points: a single point or a rectangle "aa:cc".
fn for_each_setup_point(value: &[u8], size: usize, mut f: impl FnMut(Point)) -> Result<(), SgfError> {
    let (a, b) = match value.iter().position(|&c| c == b':') {
        Some(i) => (&value[..i], &value[i + 1..]),
        None => (value, value),
    };
    let (Some(a), Some(b)) = (parse_point(a, size)?, parse_point(b, size)?) else {
        return Err(SgfError::Coordinate);
    };
    for row in a.row.min(b.row)..=a.row.max(b.row) {
        for col in a.col.min(b.col)..=a.col.max(b.col) {
            f(Point::new(col as usize, row as usize));
        }
    }
    Ok(())
}

/// Replaces `game` with the game in `text`. The text is checked completely
/// before `game` is touched, except for move legality: if move `n` turns
/// out to be illegal, the game is left just before it and
/// `SgfError::Illegal(n, ..)` is returned.
pub fn read_sgf(text: &[u8], game: &mut Game) -> Result<(), SgfError> {
    // Pass 1: syntax, size and coordinates.
    let mut size = 19;
    let mut seen_move = false;
    parse(text, |ident, value| {
        if ident_is(ident, "SZ") {
            if seen_move {
                return Err(SgfError::Unsupported); // the size must come first
            }
            size = parse_size(value)?;
        } else if ident_is(ident, "B") || ident_is(ident, "W") {
            parse_point(value, size)?;
            seen_move = true;
        } else if ident_is(ident, "AB") || ident_is(ident, "AW") || ident_is(ident, "AE") {
            if seen_move {
                return Err(SgfError::Unsupported);
            }
            for_each_setup_point(value, size, |_| {})?;
        }
        Ok(())
    })?;

    // Pass 2: set up the position and play the moves.
    let mut board = Board::new(size);
    let mut komi_half = 0;
    let mut first_player = None;
    let mut started = false;
    let mut moves = 0u16;
    let start = |game: &mut Game, board: &Board, first: Color, komi_half: u16| {
        game.set_position(*board, first);
        game.set_komi_half(komi_half);
    };
    parse(text, |ident, value| {
        if ident_is(ident, "KM") {
            komi_half = parse_komi(value);
        } else if ident_is(ident, "PL") {
            first_player = match value.first() {
                Some(b'B' | b'b') => Some(Color::Black),
                Some(b'W' | b'w') => Some(Color::White),
                _ => first_player,
            };
        } else if ident_is(ident, "AB") || ident_is(ident, "AW") || ident_is(ident, "AE") {
            let cell = if ident_is(ident, "AB") {
                Cell::Stone(Color::Black)
            } else if ident_is(ident, "AW") {
                Cell::Stone(Color::White)
            } else {
                Cell::Empty
            };
            for_each_setup_point(value, size, |p| board.set(p, cell))?;
        } else if ident_is(ident, "B") || ident_is(ident, "W") {
            let color = if ident_is(ident, "B") { Color::Black } else { Color::White };
            if !started {
                start(game, &board, first_player.unwrap_or(color), komi_half);
                started = true;
            }
            moves += 1;
            if game.phase() == Phase::Scoring {
                game.resume(); // play continued after two passes
            }
            if color != game.to_move() {
                return Err(SgfError::OutOfTurn(moves));
            }
            let result = match parse_point(value, size)? {
                Some(p) => game.play(p).map(|_| ()),
                None => game.pass(),
            };
            result.map_err(|e| SgfError::Illegal(moves, e))?;
        }
        Ok(())
    })?;
    if !started {
        start(game, &board, first_player.unwrap_or(Color::Black), komi_half);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(col: usize, row: usize) -> Point {
        Point::new(col, row)
    }

    fn sgf_of(game: &Game) -> String {
        let mut out = String::new();
        write_sgf(game, &mut out).unwrap();
        out
    }

    #[test]
    fn writes_a_simple_game() {
        let mut game = Box::new(Game::new(9));
        game.play(p(4, 4)).unwrap();
        game.play(p(2, 6)).unwrap();
        game.pass().unwrap();
        assert_eq!(
            sgf_of(&game),
            "(;GM[1]FF[4]CA[UTF-8]AP[GoOS]RU[Chinese]SZ[9]KM[7.5]\n;B[ee];W[cg];B[])\n"
        );
    }

    #[test]
    fn round_trip_keeps_position_turn_captures_and_moves() {
        let mut game = Box::new(Game::new(9));
        // A capture in the corner, then some more moves and a pass.
        for (c, r) in [(1, 0), (0, 0), (0, 1), (5, 5), (3, 3)] {
            game.play(p(c, r)).unwrap();
        }
        game.pass().unwrap();
        game.play(p(6, 6)).unwrap();
        let text = sgf_of(&game);

        let mut loaded = Box::new(Game::new(19));
        read_sgf(text.as_bytes(), &mut loaded).unwrap();
        assert_eq!(loaded.board(), game.board());
        assert_eq!(loaded.to_move(), game.to_move());
        assert_eq!(loaded.captures(Color::Black), 1);
        assert_eq!(loaded.move_number(), game.move_number());
        assert!(loaded.moves().eq(game.moves()));
        assert_eq!(sgf_of(&loaded), text);
    }

    #[test]
    fn setup_stones_and_player_to_move() {
        let mut board = Board::new(9);
        board.set(p(2, 2), Cell::Stone(Color::Black));
        board.set(p(6, 6), Cell::Stone(Color::Black));
        let mut game = Box::new(Game::new(9));
        game.set_position(board, Color::White);
        game.play(p(4, 4)).unwrap();
        let text = sgf_of(&game);
        assert!(text.contains("AB[cc][gg]PL[W]"), "{text}");

        let mut loaded = Box::new(Game::new(9));
        read_sgf(text.as_bytes(), &mut loaded).unwrap();
        assert_eq!(loaded.board(), game.board());
        assert_eq!(loaded.start_position(), game.start_position());
    }

    #[test]
    fn finished_games_record_the_result() {
        let mut game = Box::new(Game::new(9));
        game.play(p(4, 4)).unwrap();
        game.pass().unwrap();
        game.pass().unwrap();
        game.finish();
        // Black: 81 points of area; white: 7.5 komi.
        assert!(sgf_of(&game).contains("RE[B+73.5]"));
    }

    #[test]
    fn reads_sgf_from_other_programs() {
        // Variations, comments with escaped brackets, unknown properties,
        // "tt" passes, compressed setup, lower-case FF[3] identifiers and
        // whitespace everywhere.
        let text = br"
            (;FF[3]GM[1]SZ[ 9 ]KM[6.5]PB[Someone]C[a \] comment]
             AddBlack[aa:ab]
             ;B[ee]C[first]
             (;W[dc];B[tt];W[cd])
             (;W[gg];B[cc])
            )";
        let mut game = Box::new(Game::new(19));
        read_sgf(text, &mut game).unwrap();
        assert_eq!(game.size(), 9);
        assert_eq!(game.komi_half(), 13);
        assert_eq!(game.board().get(p(0, 1)), Cell::Stone(Color::Black));
        assert_eq!(game.board().get(p(3, 2)), Cell::Stone(Color::White));
        assert_eq!(game.board().get(p(2, 3)), Cell::Stone(Color::White));
        assert_eq!(game.board().get(p(6, 6)), Cell::Empty, "second variation ignored");
        assert_eq!(game.move_number(), 4);
    }

    #[test]
    fn rejects_bad_input_without_touching_the_game() {
        let mut game = Box::new(Game::new(9));
        game.play(p(4, 4)).unwrap();
        for (text, error) in [
            (&b"no tree here"[..], SgfError::Syntax),
            (b"(;SZ[9];B[ee]", SgfError::Syntax),
            (b"(;SZ[25])", SgfError::Size),
            (b"(;SZ[9];B[jj])", SgfError::Coordinate),
            (b"(;SZ[9];B[ee];AB[aa])", SgfError::Unsupported),
            (b"(;B[pp];SZ[9])", SgfError::Unsupported),
            (b"(;SZ[9:13])", SgfError::Size),
        ] {
            assert_eq!(read_sgf(text, &mut game), Err(error), "{}", String::from_utf8_lossy(text));
            assert_eq!(game.move_number(), 1, "game must be untouched");
        }
    }

    #[test]
    fn stops_at_an_illegal_or_out_of_turn_move() {
        let mut game = Box::new(Game::new(9));
        assert_eq!(read_sgf(b"(;SZ[9];B[ee];W[ee])", &mut game), Err(SgfError::Illegal(2, IllegalMove::Occupied)));
        assert_eq!(game.move_number(), 1, "loaded up to the bad move");
        assert_eq!(read_sgf(b"(;SZ[9];B[ee];B[cc])", &mut game), Err(SgfError::OutOfTurn(2)));
    }

    #[test]
    fn square_size_in_fff4_form_and_komi_reset() {
        let mut game = Box::new(Game::new(9));
        read_sgf(b"(;SZ[13:13]KM[0.5];B[gg])", &mut game).unwrap();
        assert_eq!((game.size(), game.komi_half()), (13, 1));
        game.reset(9);
        assert_eq!(game.komi_half(), 15, "a new game gets the default komi back");
    }

    #[test]
    fn komi_values() {
        assert_eq!(parse_komi(b"7.5"), 15);
        assert_eq!(parse_komi(b"6"), 12);
        assert_eq!(parse_komi(b"0.5"), 1);
        assert_eq!(parse_komi(b"-3"), 0);
        assert_eq!(parse_komi(b"junk"), 0);
    }
}
