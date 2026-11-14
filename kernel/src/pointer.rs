//! The mouse pointer: a small arrow drawn straight onto the screen. The
//! pixels underneath are saved first, so it can be taken off again before
//! anything else is drawn there.

use crate::framebuffer::{Canvas, Color, Screen};

/// `X` outline, `.` fill, space transparent. The hot spot is the top left.
const ARROW: [&[u8]; 17] = [
    b"X",
    b"XX",
    b"X.X",
    b"X..X",
    b"X...X",
    b"X....X",
    b"X.....X",
    b"X......X",
    b"X.......X",
    b"X........X",
    b"X.....XXXXX",
    b"X..X..X",
    b"X.X X..X",
    b"XX  X..X",
    b"X    X..X",
    b"     X..X",
    b"      XX",
];
const WIDTH: usize = 11;
const HEIGHT: usize = ARROW.len();
const OUTLINE: Color = Color::rgb(0x10, 0x10, 0x10);
const FILL: Color = Color::rgb(0xFA, 0xFA, 0xFA);

pub struct Pointer {
    /// Where it is drawn, if it is on screen.
    shown_at: Option<(usize, usize)>,
    saved: [Color; WIDTH * HEIGHT],
}

impl Pointer {
    pub const fn new() -> Self {
        Pointer {
            shown_at: None,
            saved: [Color::rgb(0, 0, 0); WIDTH * HEIGHT],
        }
    }

    fn pixels() -> impl Iterator<Item = (usize, usize, u8)> {
        ARROW
            .iter()
            .enumerate()
            .flat_map(|(y, row)| row.iter().enumerate().map(move |(x, &c)| (x, y, c)))
    }

    /// Draws the arrow with its tip at (x, y), remembering what it covers.
    pub fn show(&mut self, screen: &mut Screen, x: usize, y: usize) {
        self.hide(screen);
        for (dx, dy, c) in Self::pixels() {
            if c == b' ' {
                continue;
            }
            self.saved[dy * WIDTH + dx] = screen.get_pixel(x + dx, y + dy);
            screen.put_pixel(x + dx, y + dy, if c == b'X' { OUTLINE } else { FILL });
        }
        self.shown_at = Some((x, y));
    }

    /// Puts back what was under the arrow.
    pub fn hide(&mut self, screen: &mut Screen) {
        let Some((x, y)) = self.shown_at.take() else {
            return;
        };
        for (dx, dy, c) in Self::pixels() {
            if c != b' ' {
                screen.put_pixel(x + dx, y + dy, self.saved[dy * WIDTH + dx]);
            }
        }
    }
}
