//! Text drawing with the pre-rasterised Noto Sans Mono bitmap font, plus a
//! small fixed-size buffer for formatting text without a heap.

use core::fmt;

use noto_sans_mono_bitmap::{FontWeight, RasterHeight, get_raster, get_raster_width};

use crate::framebuffer::{Canvas, Color};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Font {
    pub height: RasterHeight,
    pub weight: FontWeight,
}

impl Font {
    pub const fn new(height: RasterHeight, weight: FontWeight) -> Self {
        Font { height, weight }
    }

    pub const fn char_width(self) -> usize {
        get_raster_width(self.weight, self.height)
    }

    pub const fn line_height(self) -> usize {
        self.height.val()
    }

    pub fn text_width(self, text: &str) -> usize {
        text.chars().count() * self.char_width()
    }
}

/// Draws `text` with its top-left corner at (x, y), blending the glyphs'
/// anti-aliased edges into whatever is already on screen.
pub fn draw_text(canvas: &mut impl Canvas, x: usize, y: usize, text: &str, font: Font, color: Color) {
    draw_text_scaled(canvas, x, y, text, font, color, 1);
}

/// Like `draw_text`, with every glyph pixel drawn as a `scale` x `scale`
/// block (for big titles).
pub fn draw_text_scaled(canvas: &mut impl Canvas, x: usize, y: usize, text: &str, font: Font, color: Color, scale: usize) {
    let mut cx = x;
    for ch in text.chars() {
        let glyph = get_raster(ch, font.weight, font.height)
            .or_else(|| get_raster('?', font.weight, font.height));
        if let Some(glyph) = glyph {
            for (gy, row) in glyph.rows().enumerate() {
                for (gx, &intensity) in row.iter().enumerate() {
                    for (sx, sy) in (0..scale * scale).map(|i| (i % scale, i / scale)) {
                        canvas.blend_pixel(cx + gx * scale + sx, y + gy * scale + sy, color, intensity);
                    }
                }
            }
        }
        cx += font.char_width() * scale;
    }
}

/// Draws `text` horizontally and vertically centred on (cx, cy).
pub fn draw_text_centered(canvas: &mut impl Canvas, cx: usize, cy: usize, text: &str, font: Font, color: Color) {
    let x = cx.saturating_sub(font.text_width(text) / 2);
    let y = cy.saturating_sub(font.line_height() / 2);
    draw_text(canvas, x, y, text, font, color);
}

/// A stack-allocated string for `write!`-formatting short UI texts.
/// Text that doesn't fit is cut off.
pub struct TextBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> TextBuf<N> {
    pub const fn new() -> Self {
        TextBuf { buf: [0; N], len: 0 }
    }

    pub fn as_str(&self) -> &str {
        // Only whole UTF-8 characters are ever copied in (see write_str).
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> fmt::Write for TextBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for ch in s.chars() {
            let mut tmp = [0; 4];
            let bytes = ch.encode_utf8(&mut tmp).as_bytes();
            if self.len + bytes.len() > N {
                return Ok(());
            }
            self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
        }
        Ok(())
    }
}
