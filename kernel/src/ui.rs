//! Drawing the Go board, stones and the status panel.
//!
//! The board is laid out from the screen size: a square of wood on the left
//! holding the grid plus a label band on each side, and a status panel on the
//! right. Each intersection owns a `cell`-sized square, and nothing drawn for
//! a point leaves its square, so any single point can be redrawn on its own.
//! Points and status lines are composed off-screen and copied over in one
//! go, so redraws don't flicker.

use go_core::Color as Player;
use noto_sans_mono_bitmap::{FontWeight, RasterHeight};

use crate::font::{self, Font};
use crate::framebuffer::{Canvas, Color, Screen, TILE, TILE_PIXELS, disc_coverage};

pub const BACKGROUND: Color = Color::rgb(0x24, 0x28, 0x2E);
pub const WOOD: Color = Color::rgb(0xDC, 0xB3, 0x5C);
const LINE: Color = Color::rgb(0x2A, 0x1E, 0x10);
pub const TEXT: Color = Color::rgb(0xEC, 0xEC, 0xEC);
pub const DIM_TEXT: Color = Color::rgb(0x9A, 0xA0, 0xA8);
const BLACK_STONE: Color = Color::rgb(0x1C, 0x1C, 0x1E);
const BLACK_EDGE: Color = Color::rgb(0x00, 0x00, 0x00);
const BLACK_SHINE: Color = Color::rgb(0x6A, 0x6A, 0x70);
const WHITE_STONE: Color = Color::rgb(0xF4, 0xF2, 0xEC);
const WHITE_EDGE: Color = Color::rgb(0x70, 0x70, 0x70);
const CURSOR: Color = Color::rgb(0x1E, 0x5A, 0xC8);
const LAST_MOVE: Color = Color::rgb(0xD0, 0x30, 0x30);
pub const WARNING: Color = Color::rgb(0xF0, 0x80, 0x70);
pub const ACCENT: Color = Color::rgb(0xF0, 0xC8, 0x60);

/// Column labels: Go coordinates skip the letter I.
const COLUMNS: &[u8; 19] = b"ABCDEFGHJKLMNOPQRST";

/// Everything drawn at one intersection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PointView {
    pub stone: Option<Player>,
    /// Marked dead during scoring (drawn faded).
    pub dead: bool,
    /// Counted as this player's territory (drawn as a small square).
    pub territory: Option<Player>,
    pub last_move: bool,
    pub cursor: bool,
    /// Translucent preview of the stone the cursor would place.
    pub ghost: Option<Player>,
}

pub struct Layout {
    pub size: usize,
    /// Distance between grid lines in pixels.
    pub cell: usize,
    /// Screen position of the top-left intersection.
    origin_x: usize,
    origin_y: usize,
    board_x: usize,
    board_y: usize,
    board_px: usize,
    /// Distance from the outermost grid lines to the centre of the labels.
    label_gap: usize,
    show_labels: bool,
    /// Grid line width in pixels (odd, so lines centre on the intersection).
    line: usize,
    pub panel_x: usize,
    pub panel_y: usize,
    pub panel_w: usize,
    label_font: Font,
    pub text_font: Font,
    pub title_font: Font,
}

impl Layout {
    pub fn new(screen_w: usize, screen_h: usize, size: usize) -> Self {
        const MIN_PANEL: usize = 320;
        let avail = screen_h.min(screen_w.saturating_sub(MIN_PANEL));
        let margin = avail / 40;
        let label_font = Font::new(
            if avail / (size + 1) >= 56 { RasterHeight::Size24 } else { RasterHeight::Size16 },
            FontWeight::Regular,
        );
        // Labels live in a band outside the edge points' squares.
        let label_band = label_font.line_height() + 4;
        let cell = avail.saturating_sub(2 * margin + 2 * label_band) / size;
        // Distance from an outer grid line to the board edge: the far side
        // of an edge point's square, then the label band.
        let edge = cell - cell / 2 + label_band;
        let board_px = (size - 1) * cell + 2 * edge;
        let board_x = margin;
        let board_y = screen_h.saturating_sub(board_px) / 2;
        let panel_x = board_x + board_px + 2 * margin;
        let panel_w = screen_w.saturating_sub(panel_x + margin);
        // A bigger font on big screens, if the panel fits ~26 characters.
        let big_font = Font::new(RasterHeight::Size32, FontWeight::Regular);
        let big = screen_h >= 1000 && panel_w >= 26 * big_font.char_width();
        Layout {
            size,
            cell,
            origin_x: board_x + edge,
            origin_y: board_y + edge,
            board_x,
            board_y,
            board_px,
            label_gap: cell - cell / 2 + label_band / 2,
            // Below this, neighbouring labels would run into each other.
            show_labels: cell > label_font.line_height(),
            line: if cell >= 80 { 3 } else { 1 },
            panel_x,
            panel_y: board_y,
            panel_w,
            label_font,
            text_font: if big { big_font } else { Font::new(RasterHeight::Size24, FontWeight::Regular) },
            title_font: Font::new(RasterHeight::Size32, FontWeight::Bold),
        }
    }

    /// The intersection whose cell contains screen position (x, y), if any.
    pub fn point_at(&self, x: usize, y: usize) -> Option<(usize, usize)> {
        let half = self.cell / 2;
        let col = (x + half).checked_sub(self.origin_x)? / self.cell.max(1);
        let row = (y + half).checked_sub(self.origin_y)? / self.cell.max(1);
        (col < self.size && row < self.size).then_some((col, row))
    }

    /// Screen position of the intersection at (col, row); row 0 is the top.
    pub fn intersection(&self, col: usize, row: usize) -> (usize, usize) {
        (self.origin_x + col * self.cell, self.origin_y + row * self.cell)
    }

    fn stone_radius(&self) -> isize {
        (self.cell * 47 / 100) as isize
    }

    fn panel_line_height(&self) -> usize {
        self.text_font.line_height() * 3 / 2
    }

    /// How many text lines fit in the status panel.
    pub fn panel_lines(&self) -> usize {
        self.board_px / self.panel_line_height()
    }
}

fn is_hoshi(size: usize, col: usize, row: usize) -> bool {
    let (lo, mid, hi) = match size {
        9 => (2, 4, 6),
        13 => (3, 6, 9),
        19 => (3, 9, 15),
        _ => return false,
    };
    let edge = |v| v == lo || v == hi;
    if size == 19 {
        // All nine star points.
        (edge(col) || col == mid) && (edge(row) || row == mid)
    } else {
        // Four corner points plus the centre.
        (edge(col) && edge(row)) || (col == mid && row == mid)
    }
}

/// Draws the whole screen: background, board, labels and every point.
pub fn draw_board(screen: &mut Screen, layout: &Layout, point: impl Fn(usize, usize) -> PointView) {
    screen.clear(BACKGROUND);
    screen.fill_rect(layout.board_x, layout.board_y, layout.board_px, layout.board_px, WOOD);
    if layout.show_labels {
        draw_labels(screen, layout);
    }
    for row in 0..layout.size {
        for col in 0..layout.size {
            draw_point(screen, layout, col, row, point(col, row));
        }
    }
}

fn draw_labels(screen: &mut Screen, layout: &Layout) {
    let font = layout.label_font;
    let gap = layout.label_gap;
    let (left, top) = layout.intersection(0, 0);
    let (right, bottom) = layout.intersection(layout.size - 1, layout.size - 1);
    for (i, &letter) in COLUMNS.iter().take(layout.size).enumerate() {
        let (x, y) = layout.intersection(i, i);
        let letter = [letter];
        let letter = core::str::from_utf8(&letter).unwrap_or("?");
        font::draw_text_centered(screen, x, top - gap, letter, font, LINE);
        font::draw_text_centered(screen, x, bottom + gap, letter, font, LINE);

        let mut number = font::TextBuf::<2>::new();
        let _ = core::fmt::write(&mut number, format_args!("{}", layout.size - i));
        font::draw_text_centered(screen, left - gap, y, number.as_str(), font, LINE);
        font::draw_text_centered(screen, right + gap, y, number.as_str(), font, LINE);
    }
}

/// Redraws one intersection's square: wood, grid line pieces, star point,
/// then whatever sits on top.
pub fn draw_point(screen: &mut Screen, layout: &Layout, col: usize, row: usize, view: PointView) {
    let (x, y) = layout.intersection(col, row);
    let cell = layout.cell;
    let half = cell / 2;
    let last = layout.size - 1;
    // The outermost lines are drawn a little heavier.
    let h_width = if row == 0 || row == last { layout.line + 2 } else { layout.line };
    let v_width = if col == 0 || col == last { layout.line + 2 } else { layout.line };

    let mut tile = TILE.lock();
    tile.reset(cell, cell, WOOD);
    // The intersection sits at (half, half) inside the tile. Lines are
    // centred on it and stop there at the board edge.
    let h_start = if col == 0 { half - v_width / 2 } else { 0 };
    let h_end = if col == last { half + v_width / 2 + 1 } else { cell };
    tile.hline(h_start, half - h_width / 2, h_end - h_start, h_width, LINE);
    let v_start = if row == 0 { half - h_width / 2 } else { 0 };
    let v_end = if row == last { half + h_width / 2 + 1 } else { cell };
    tile.vline(half - v_width / 2, v_start, v_end - v_start, v_width, LINE);

    let c = half as isize;
    let r = layout.stone_radius();
    if is_hoshi(layout.size, col, row) {
        tile.fill_circle(c, c, (cell / 10).max(2) as isize, LINE);
    }
    if let Some(stone) = view.stone {
        draw_stone(&mut *tile, c, c, r, stone, if view.dead { 100 } else { 255 });
    } else if let Some(ghost) = view.ghost {
        draw_stone(&mut *tile, c, c, r, ghost, 90);
    }
    if let Some(owner) = view.territory {
        let side = (cell / 4).max(3);
        let (fill, edge) = match owner {
            Player::Black => (BLACK_STONE, BLACK_EDGE),
            Player::White => (WHITE_STONE, WHITE_EDGE),
        };
        tile.fill_rect(half - side / 2, half - side / 2, side, side, edge);
        tile.fill_rect(half - side / 2 + 1, half - side / 2 + 1, side - 2, side - 2, fill);
    }
    if view.last_move {
        tile.fill_circle(c, c, (r / 3).max(2), LAST_MOVE);
    }
    if view.cursor {
        draw_cursor(&mut *tile, cell);
    }

    screen.blit(x - half, y - half, &tile);
}

/// Corner brackets around the cell, inset a little from its edges.
fn draw_cursor(canvas: &mut impl Canvas, cell: usize) {
    let inset = (cell / 12).max(1);
    let len = cell / 4;
    let t = (cell / 20).clamp(2, 4);
    let (lo, hi) = (inset, cell - inset - t);
    for (x, y) in [(lo, lo), (hi, lo), (lo, hi), (hi, hi)] {
        // Each corner: one horizontal and one vertical arm pointing inwards.
        let hx = if x == lo { x } else { x + t - len };
        let vy = if y == lo { y } else { y + t - len };
        canvas.hline(hx, y, len, t, CURSOR);
        canvas.vline(x, vy, len, t, CURSOR);
    }
}

/// Draws a stone in a single pass, so a translucent (dead) stone blends
/// cleanly with what's underneath.
pub fn draw_stone(canvas: &mut impl Canvas, cx: isize, cy: isize, r: isize, stone: Player, alpha: u8) {
    let rim_width = (r / 14).max(1);
    let (rim, body) = match stone {
        Player::Black => (BLACK_EDGE, BLACK_STONE),
        Player::White => (WHITE_EDGE, WHITE_STONE),
    };
    // Black stones get a soft highlight towards the top left.
    let (shine_x, shine_y, shine_r) = (-r / 3, -r / 3, r / 4);
    for dy in -r..=r {
        for dx in -r..=r {
            let (x, y) = (cx + dx, cy + dy);
            let d2 = dx * dx + dy * dy;
            let coverage = disc_coverage(d2, r);
            if coverage == 0 || x < 0 || y < 0 {
                continue;
            }
            let mut color = body.blend_over(rim, disc_coverage(d2, r - rim_width));
            if stone == Player::Black {
                let s2 = (dx - shine_x) * (dx - shine_x) + (dy - shine_y) * (dy - shine_y);
                color = BLACK_SHINE.blend_over(color, disc_coverage(s2, shine_r) / 3);
            }
            let a = (coverage as u32 * alpha as u32 / 255) as u8;
            canvas.blend_pixel(x as usize, y as usize, color, a);
        }
    }
}

/// Redraws one line of text in the status panel, replacing what was there.
/// Text is clipped to the panel.
pub fn panel_line(screen: &mut Screen, layout: &Layout, line: usize, text: &str, font: Font, color: Color) {
    let line_h = layout.panel_line_height();
    let mut tile = TILE.lock();
    tile.reset(layout.panel_w, line_h, BACKGROUND);
    font::draw_text(&mut *tile, 0, 0, text, font, color);
    screen.blit(layout.panel_x, layout.panel_y + line * line_h, &tile);
}

/// Draws one line of text centred horizontally on the screen, replacing a
/// band of background `line_h` pixels tall at `y`.
pub fn centered_line(screen: &mut Screen, y: usize, line_h: usize, text: &str, font: Font, color: Color) {
    centered_line_scaled(screen, y, line_h, text, font, color, 1);
}

/// `centered_line` with the text enlarged `scale` times.
pub fn centered_line_scaled(
    screen: &mut Screen,
    y: usize,
    line_h: usize,
    text: &str,
    font: Font,
    color: Color,
    scale: usize,
) {
    let width = screen.width().min(1600).min(TILE_PIXELS / line_h.max(1));
    let mut tile = TILE.lock();
    tile.reset(width, line_h, BACKGROUND);
    let x = width.saturating_sub(font.text_width(text) * scale) / 2;
    let text_y = line_h.saturating_sub(font.line_height() * scale) / 2;
    font::draw_text_scaled(&mut *tile, x, text_y, text, font, color, scale);
    screen.blit((screen.width() - width) / 2, y, &tile);
}
