//! Pixel drawing on the linear framebuffer handed over by the bootloader.
//!
//! There is no full-screen back buffer. To avoid flicker, callers redraw only
//! what changed, composing each piece (a board cell, a status line) in a
//! small off-screen [`Tile`] and copying it to the screen in one go.

use bootloader_api::info::{FrameBuffer, PixelFormat};
use spin::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color { r, g, b }
    }

    /// Mixes `self` over `under`; `alpha` 255 is fully `self`.
    pub fn blend_over(self, under: Color, alpha: u8) -> Color {
        let mix = |a: u8, b: u8| ((a as u32 * alpha as u32 + b as u32 * (255 - alpha as u32)) / 255) as u8;
        Color::rgb(mix(self.r, under.r), mix(self.g, under.g), mix(self.b, under.b))
    }
}

/// How much of the pixel at squared distance `d2` (in pixels²) from a
/// circle's centre is covered by a disc of radius `r`, from 0 to 255.
///
/// Working with 4x squared distances keeps this in integers: a pixel is fully
/// inside when d <= r - 1/2 and fully outside when d >= r + 1/2, with a
/// linear ramp in between.
pub fn disc_coverage(d2: isize, r: isize) -> u8 {
    if r <= 0 {
        return 0;
    }
    let d = 4 * d2;
    let outer = 4 * r * r + 4 * r + 1; // 4 * (r + 1/2)^2
    let inner = 4 * r * r - 4 * r + 1; // 4 * (r - 1/2)^2
    if d >= outer {
        0
    } else if d <= inner {
        255
    } else {
        ((outer - d) * 255 / (outer - inner)) as u8
    }
}

/// Something that can be drawn on: the screen or an off-screen tile.
/// Coordinates outside the canvas are clipped.
pub trait Canvas {
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    fn get_pixel(&self, x: usize, y: usize) -> Color;
    fn put_pixel(&mut self, x: usize, y: usize, c: Color);

    fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, c: Color) {
        let x_end = x.saturating_add(w).min(self.width());
        let y_end = y.saturating_add(h).min(self.height());
        for py in y..y_end {
            for px in x..x_end {
                self.put_pixel(px, py, c);
            }
        }
    }

    /// Horizontal line `thickness` pixels tall, starting at (x, y).
    fn hline(&mut self, x: usize, y: usize, len: usize, thickness: usize, c: Color) {
        self.fill_rect(x, y, len, thickness, c);
    }

    /// Vertical line `thickness` pixels wide, starting at (x, y).
    fn vline(&mut self, x: usize, y: usize, len: usize, thickness: usize, c: Color) {
        self.fill_rect(x, y, thickness, len, c);
    }

    /// Draws `c` over the existing pixel with the given opacity.
    fn blend_pixel(&mut self, x: usize, y: usize, c: Color, alpha: u8) {
        if x >= self.width() || y >= self.height() {
            return;
        }
        match alpha {
            0 => {}
            255 => self.put_pixel(x, y, c),
            _ => {
                let under = self.get_pixel(x, y);
                self.put_pixel(x, y, c.blend_over(under, alpha));
            }
        }
    }

    fn fill_circle(&mut self, cx: isize, cy: isize, r: isize, c: Color) {
        self.blend_circle(cx, cy, r, c, 255);
    }

    /// Anti-aliased filled circle centred on pixel (cx, cy), drawn with the
    /// given opacity.
    fn blend_circle(&mut self, cx: isize, cy: isize, r: isize, c: Color, alpha: u8) {
        for dy in -r..=r {
            for dx in -r..=r {
                let (x, y) = (cx + dx, cy + dy);
                if x < 0 || y < 0 {
                    continue;
                }
                let coverage = disc_coverage(dx * dx + dy * dy, r) as u32;
                let a = (coverage * alpha as u32 / 255) as u8;
                self.blend_pixel(x as usize, y as usize, c, a);
            }
        }
    }
}

pub struct Screen {
    buf: &'static mut [u8],
    width: usize,
    height: usize,
    /// Pixels per scanline (may be larger than `width`).
    stride: usize,
    bytes_per_pixel: usize,
    format: PixelFormat,
}

impl Screen {
    pub fn new(fb: &'static mut FrameBuffer) -> Self {
        let info = fb.info();
        let buf = fb.buffer_mut();
        // Never trust the reported height beyond what the buffer can hold.
        let row_bytes = (info.stride * info.bytes_per_pixel).max(1);
        let height = info.height.min(buf.len() / row_bytes);
        Screen {
            width: info.width,
            height,
            stride: info.stride,
            bytes_per_pixel: info.bytes_per_pixel,
            format: info.pixel_format,
            buf,
        }
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// The bytes of one pixel in the framebuffer's native layout.
    fn encode(&self, c: Color) -> [u8; 4] {
        match self.format {
            PixelFormat::Rgb => [c.r, c.g, c.b, 0],
            PixelFormat::Bgr => [c.b, c.g, c.r, 0],
            // Assumes a grayscale palette (QEMU never hands out palette modes).
            PixelFormat::U8 => {
                let gray = (c.r as u16 * 77 + c.g as u16 * 150 + c.b as u16 * 29) >> 8;
                [gray as u8, 0, 0, 0]
            }
            PixelFormat::Unknown { red_position, green_position, blue_position } => {
                let v = (c.r as u32) << red_position
                    | (c.g as u32) << green_position
                    | (c.b as u32) << blue_position;
                v.to_le_bytes()
            }
            _ => [c.r, c.g, c.b, 0],
        }
    }

    fn decode(&self, px: [u8; 4]) -> Color {
        match self.format {
            PixelFormat::Rgb => Color::rgb(px[0], px[1], px[2]),
            PixelFormat::Bgr => Color::rgb(px[2], px[1], px[0]),
            PixelFormat::U8 => Color::rgb(px[0], px[0], px[0]),
            PixelFormat::Unknown { red_position, green_position, blue_position } => {
                let v = u32::from_le_bytes(px);
                Color::rgb((v >> red_position) as u8, (v >> green_position) as u8, (v >> blue_position) as u8)
            }
            _ => Color::rgb(px[0], px[1], px[2]),
        }
    }

    fn offset(&self, x: usize, y: usize) -> usize {
        (y * self.stride + x) * self.bytes_per_pixel
    }

    pub fn clear(&mut self, c: Color) {
        self.fill_rect(0, 0, self.width, self.height, c);
    }

    /// Copies a tile to the screen with its top-left corner at (x, y).
    pub fn blit(&mut self, x: usize, y: usize, tile: &Tile) {
        let bpp = self.bytes_per_pixel.min(4);
        let w = tile.w.min(self.width.saturating_sub(x));
        for ty in 0..tile.h.min(self.height.saturating_sub(y)) {
            let row = &tile.buf[ty * tile.w..ty * tile.w + w];
            let start = self.offset(x, y + ty);
            for (i, &c) in row.iter().enumerate() {
                let at = start + i * self.bytes_per_pixel;
                let px = self.encode(c);
                self.buf[at..at + bpp].copy_from_slice(&px[..bpp]);
            }
        }
    }
}

impl Canvas for Screen {
    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    fn get_pixel(&self, x: usize, y: usize) -> Color {
        if x >= self.width || y >= self.height {
            return Color::rgb(0, 0, 0);
        }
        let bpp = self.bytes_per_pixel.min(4);
        let at = self.offset(x, y);
        let mut px = [0; 4];
        px[..bpp].copy_from_slice(&self.buf[at..at + bpp]);
        self.decode(px)
    }

    fn put_pixel(&mut self, x: usize, y: usize, c: Color) {
        if x >= self.width || y >= self.height {
            return;
        }
        let bpp = self.bytes_per_pixel.min(4);
        let px = self.encode(c);
        let at = self.offset(x, y);
        self.buf[at..at + bpp].copy_from_slice(&px[..bpp]);
    }

    /// Encodes the colour once and copies it along each row.
    fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, c: Color) {
        let x_end = x.saturating_add(w).min(self.width);
        let y_end = y.saturating_add(h).min(self.height);
        if x >= x_end || y >= y_end {
            return;
        }
        let bpp = self.bytes_per_pixel;
        let px = self.encode(c);
        let px = &px[..bpp.min(4)];
        for row in y..y_end {
            let start = self.offset(x, row);
            let end = self.offset(x_end, row);
            for chunk in self.buf[start..end].chunks_exact_mut(bpp) {
                chunk[..px.len()].copy_from_slice(px);
            }
        }
    }
}

/// Room for a 9x9 board cell at 2560x1600, or a full-width status line.
const TILE_PIXELS: usize = 96_000;

/// An off-screen drawing area in ordinary memory (fast to read back, unlike
/// video memory).
pub struct Tile {
    buf: [Color; TILE_PIXELS],
    w: usize,
    h: usize,
}

/// The one shared scratch tile (it's too big for the stack).
pub static TILE: Mutex<Tile> = Mutex::new(Tile { buf: [Color::rgb(0, 0, 0); TILE_PIXELS], w: 0, h: 0 });

impl Tile {
    /// Resizes the tile and fills it with `background`. The height is
    /// reduced if `w * h` doesn't fit.
    pub fn reset(&mut self, w: usize, h: usize, background: Color) {
        self.w = w.min(TILE_PIXELS);
        self.h = TILE_PIXELS.checked_div(self.w).map_or(0, |max_h| h.min(max_h));
        self.buf[..self.w * self.h].fill(background);
    }
}

impl Canvas for Tile {
    fn width(&self) -> usize {
        self.w
    }

    fn height(&self) -> usize {
        self.h
    }

    fn get_pixel(&self, x: usize, y: usize) -> Color {
        if x >= self.w || y >= self.h {
            return Color::rgb(0, 0, 0);
        }
        self.buf[y * self.w + x]
    }

    fn put_pixel(&mut self, x: usize, y: usize, c: Color) {
        if x < self.w && y < self.h {
            self.buf[y * self.w + x] = c;
        }
    }
}
