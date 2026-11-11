//! Pixel drawing on the linear framebuffer handed over by the bootloader.

use bootloader_api::info::{FrameBuffer, PixelFormat};

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
}

pub struct Screen {
    buf: &'static mut [u8],
    pub width: usize,
    pub height: usize,
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
        let height = info.height.min(buf.len() / (info.stride * info.bytes_per_pixel));
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

    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, c: Color) {
        let x_end = x.saturating_add(w).min(self.width);
        let y_end = y.saturating_add(h).min(self.height);
        if x >= x_end || y >= y_end {
            return;
        }
        let bpp = self.bytes_per_pixel;
        let px = self.encode(c);
        let px = &px[..bpp.min(4)];
        for row in y..y_end {
            let start = (row * self.stride + x) * bpp;
            let end = (row * self.stride + x_end) * bpp;
            for chunk in self.buf[start..end].chunks_exact_mut(bpp) {
                chunk[..px.len()].copy_from_slice(px);
            }
        }
    }

    pub fn clear(&mut self, c: Color) {
        self.fill_rect(0, 0, self.width, self.height, c);
    }
}
