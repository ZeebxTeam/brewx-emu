//! The screen: a software framebuffer, the 2D primitives BREW draws with, image decoding and a
//! built-in bitmap font.

use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Self = Self::new(0, 0, 0);
    pub const WHITE: Self = Self::new(255, 255, 255);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// `RGBVAL` is `0xBBGGRRxx`: `MAKE_RGB(r, g, b)` shifts red by 8, green by 16, blue by 24.
    pub fn from_rgbval(value: u32) -> Self {
        Self::new((value >> 8) as u8, (value >> 16) as u8, (value >> 24) as u8)
    }

    pub fn to_rgbval(self) -> u32 {
        u32::from(self.r) << 8 | u32::from(self.g) << 16 | u32::from(self.b) << 24
    }

    fn pack(self) -> u32 {
        u32::from(self.r) << 16 | u32::from(self.g) << 8 | u32::from(self.b)
    }

    fn unpack(v: u32) -> Self {
        Self::new((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }
}

/// `AEERect`: `{ int16 x, y, dx, dy }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn from_bytes(b: [u8; 8]) -> Self {
        let r = |i: usize| i32::from(i16::from_le_bytes([b[i], b[i + 1]]));
        Self {
            x: r(0),
            y: r(2),
            w: r(4),
            h: r(6),
        }
    }

    pub fn to_bytes(self) -> [u8; 8] {
        let mut out = [0; 8];
        for (i, v) in [self.x, self.y, self.w, self.h].into_iter().enumerate() {
            out[i * 2..i * 2 + 2].copy_from_slice(&(v as i16).to_le_bytes());
        }
        out
    }

    pub fn intersect(self, o: Rect) -> Option<Rect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = (self.x + self.w).min(o.x + o.w);
        let y1 = (self.y + self.h).min(o.y + o.h);
        (x1 > x0 && y1 > y0).then_some(Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }

    fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

/// A decoded image. Pixels are `0xAARRGGBB`; alpha 0 is transparent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

/// The color BREW treats as transparent in `AEE_RO_TRANSPARENT` blits: magenta.
const TRANSPARENT_KEY: u32 = 0x00ff_00ff;

impl Image {
    /// PNG or Windows BMP, by signature. BREW's own BCI is not decoded yet.
    /// PNG with alpha, for frontends that show the image outside the emulated screen.
    pub fn to_png(&self) -> Vec<u8> {
        let rgba: Vec<u8> = self
            .pixels
            .iter()
            .flat_map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8])
            .collect();
        encode_png(self.width, self.height, &rgba)
    }

    pub fn decode(data: &[u8]) -> Option<Rc<Image>> {
        if data.starts_with(b"\x89PNG") {
            decode_png(data)
        } else if data.starts_with(b"BM") {
            decode_bmp(data)
        } else {
            None
        }
        .map(Rc::new)
    }
}

fn decode_png(data: &[u8]) -> Option<Image> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let bytes = &buf[..info.buffer_size()];
    let px = |c: &[u8], a: u8| {
        u32::from(a) << 24 | u32::from(c[0]) << 16 | u32::from(c[1]) << 8 | u32::from(c[2])
    };
    let pixels: Vec<u32> = match info.color_type {
        png::ColorType::Rgba => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| px(c, c[3]))
            .collect(),
        png::ColorType::Rgb => bytes
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| px(c, 255))
            .collect(),
        png::ColorType::GrayscaleAlpha => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| px(&[c[0], c[0], c[0]], c[1]))
            .collect(),
        png::ColorType::Grayscale => bytes.iter().map(|&g| px(&[g, g, g], 255)).collect(),
        png::ColorType::Indexed => return None,
    };
    Some(Image {
        width: info.width,
        height: info.height,
        pixels,
    })
}

/// Uncompressed Windows BMP at 1/4/8/16/24/32 bpp — what BREW tools emit. Magenta becomes
/// transparent: it is the color key BREW titles paint their sprite backgrounds with.
fn decode_bmp(data: &[u8]) -> Option<Image> {
    let u32_at = |o: usize| Some(u32::from_le_bytes(data.get(o..o + 4)?.try_into().ok()?));
    let u16_at = |o: usize| Some(u16::from_le_bytes(data.get(o..o + 2)?.try_into().ok()?));
    let offset = u32_at(10)? as usize;
    let header = u32_at(14)? as usize;
    let (width, raw_height, bpp, compression) = if header == 12 {
        (
            i32::from(u16_at(18)?),
            i32::from(u16_at(20)?),
            u16_at(24)?,
            0,
        )
    } else {
        (
            u32_at(18)? as i32,
            u32_at(22)? as i32,
            u16_at(28)?,
            u32_at(30)?,
        )
    };
    // BI_BITFIELDS (3) with the standard 565 masks is common for 16-bit art.
    if width <= 0 || raw_height == 0 || !(compression == 0 || compression == 3) {
        return None;
    }
    let (w, h) = (width as usize, raw_height.unsigned_abs() as usize);
    let palette_at = 14 + header;
    let colors = if header == 12 { 3 } else { 4 };
    let palette = |i: usize| -> u32 {
        let o = palette_at + i * colors;
        match data.get(o..o + 3) {
            Some(c) => u32::from(c[2]) << 16 | u32::from(c[1]) << 8 | u32::from(c[0]),
            None => 0,
        }
    };
    let stride = (w * bpp as usize).div_ceil(32) * 4;
    let mut pixels = vec![0u32; w * h];
    for row in 0..h {
        let src_row = if raw_height > 0 { h - 1 - row } else { row };
        let line = data.get(offset + src_row * stride..offset + (src_row + 1) * stride)?;
        for x in 0..w {
            let rgb = match bpp {
                1 => palette(usize::from(line[x / 8] >> (7 - x % 8) & 1)),
                4 => palette(usize::from(line[x / 2] >> (4 * (1 - x % 2)) & 0xf)),
                8 => palette(usize::from(line[x])),
                16 => {
                    let v = u16::from_le_bytes([line[x * 2], line[x * 2 + 1]]);
                    let (r, g, b) = if compression == 3 {
                        ((v >> 11) & 0x1f, (v >> 5) & 0x3f, v & 0x1f)
                    } else {
                        ((v >> 10) & 0x1f, ((v >> 5) & 0x1f) << 1, v & 0x1f)
                    };
                    let (r, g, b) = (r as u32, g as u32, b as u32);
                    (r << 3 | r >> 2) << 16 | (g << 2 | g >> 4) << 8 | (b << 3 | b >> 2)
                }
                24 | 32 => {
                    let n = bpp as usize / 8;
                    let c = &line[x * n..x * n + 3];
                    u32::from(c[2]) << 16 | u32::from(c[1]) << 8 | u32::from(c[0])
                }
                _ => return None,
            };
            let alpha = if rgb == TRANSPARENT_KEY {
                0
            } else {
                0xff00_0000
            };
            pixels[row * w + x] = alpha | rgb;
        }
    }
    Some(Image {
        width: w as u32,
        height: h as u32,
        pixels,
    })
}

/// The built-in font: 8x8 glyphs drawn in a 12-pixel line. A placeholder until handset fonts
/// are emulated, but text in the right place beats no text.
pub const FONT_ASCENT: i32 = 10;
pub const FONT_DESCENT: i32 = 2;
pub const FONT_ADVANCE: i32 = 7;

#[derive(Debug, Clone)]
pub struct Framebuffer {
    width: u32,
    height: u32,
    /// `0x00RRGGBB`.
    pixels: Vec<u32>,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width * height) as usize],
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn bounds(&self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            w: self.width as i32,
            h: self.height as i32,
        }
    }

    pub fn pixel(&self, x: i32, y: i32) -> Option<Rgb> {
        self.bounds()
            .contains(x, y)
            .then(|| Rgb::unpack(self.pixels[(y as u32 * self.width + x as u32) as usize]))
    }

    pub fn set_pixel(&mut self, x: i32, y: i32, c: Rgb, clip: Rect) {
        if clip.contains(x, y) && self.bounds().contains(x, y) {
            self.pixels[(y as u32 * self.width + x as u32) as usize] = c.pack();
        }
    }

    pub fn fill_rect(&mut self, r: Rect, c: Rgb, clip: Rect) {
        let Some(r) = r.intersect(clip).and_then(|r| r.intersect(self.bounds())) else {
            return;
        };
        let v = c.pack();
        for y in r.y..r.y + r.h {
            let row = (y as u32 * self.width) as usize;
            self.pixels[row + r.x as usize..row + (r.x + r.w) as usize].fill(v);
        }
    }

    pub fn draw_frame(&mut self, r: Rect, c: Rgb, clip: Rect) {
        if r.w <= 0 || r.h <= 0 {
            return;
        }
        let edges = [
            Rect { h: 1, ..r },
            Rect {
                y: r.y + r.h - 1,
                h: 1,
                ..r
            },
            Rect { w: 1, ..r },
            Rect {
                x: r.x + r.w - 1,
                w: 1,
                ..r
            },
        ];
        for e in edges {
            self.fill_rect(e, c, clip);
        }
    }

    pub fn draw_line(
        &mut self,
        (mut x0, mut y0): (i32, i32),
        (x1, y1): (i32, i32),
        c: Rgb,
        clip: Rect,
    ) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let mut err = dx + dy;
        loop {
            self.set_pixel(x0, y0, c, clip);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    pub fn draw_circle(&mut self, cx: i32, cy: i32, r: i32, c: Rgb, fill: Option<Rgb>, clip: Rect) {
        for dy in -r..=r {
            let half = f64::from(r * r - dy * dy).sqrt() as i32;
            if let Some(f) = fill {
                self.fill_rect(
                    Rect {
                        x: cx - half,
                        y: cy + dy,
                        w: 2 * half + 1,
                        h: 1,
                    },
                    f,
                    clip,
                );
            }
            self.set_pixel(cx - half, cy + dy, c, clip);
            self.set_pixel(cx + half, cy + dy, c, clip);
        }
    }

    /// Scanline fill of a polygon, even-odd rule.
    pub fn fill_polygon(&mut self, points: &[(i32, i32)], c: Rgb, clip: Rect) {
        if points.len() < 3 {
            return;
        }
        let (min_y, max_y) = points
            .iter()
            .fold((i32::MAX, i32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
        for y in min_y..=max_y {
            let mut xs: Vec<i32> = Vec::new();
            for i in 0..points.len() {
                let (a, b) = (points[i], points[(i + 1) % points.len()]);
                if (a.1 <= y && b.1 > y) || (b.1 <= y && a.1 > y) {
                    xs.push(a.0 + (y - a.1) * (b.0 - a.0) / (b.1 - a.1));
                }
            }
            xs.sort_unstable();
            for pair in xs.as_chunks::<2>().0.iter() {
                self.fill_rect(
                    Rect {
                        x: pair[0],
                        y,
                        w: pair[1] - pair[0] + 1,
                        h: 1,
                    },
                    c,
                    clip,
                );
            }
        }
    }

    /// Draws `src` (the `src_rect` part of it) at `(x, y)`. `transparent` honors alpha and the
    /// color key; otherwise every pixel is copied.
    pub fn draw_image(
        &mut self,
        src: &Image,
        x: i32,
        y: i32,
        src_rect: Rect,
        transparent: bool,
        clip: Rect,
    ) {
        let src_bounds = Rect {
            x: 0,
            y: 0,
            w: src.width as i32,
            h: src.height as i32,
        };
        let Some(part) = src_rect.intersect(src_bounds) else {
            return;
        };
        for sy in 0..part.h {
            for sx in 0..part.w {
                let p =
                    src.pixels[((part.y + sy) as u32 * src.width + (part.x + sx) as u32) as usize];
                let alpha = p >> 24;
                if transparent && alpha == 0 {
                    continue;
                }
                let (dx, dy) = (x + sx, y + sy);
                let c = Rgb::unpack(p);
                if !transparent || alpha == 255 {
                    self.set_pixel(dx, dy, c, clip);
                } else if let Some(under) = self.pixel(dx, dy) {
                    let mix = |a: u8, b: u8| {
                        ((u32::from(a) * alpha + u32::from(b) * (255 - alpha)) / 255) as u8
                    };
                    self.set_pixel(
                        dx,
                        dy,
                        Rgb::new(mix(c.r, under.r), mix(c.g, under.g), mix(c.b, under.b)),
                        clip,
                    );
                }
            }
        }
    }

    /// Text in the built-in font, top-left at `(x, y)`. Returns the width drawn.
    pub fn draw_text(&mut self, text: &str, x: i32, y: i32, c: Rgb, clip: Rect) -> i32 {
        use font8x8::UnicodeFonts;
        let mut pen = x;
        for ch in text.chars() {
            let glyph = font8x8::BASIC_FONTS
                .get(ch)
                .or_else(|| font8x8::LATIN_FONTS.get(ch))
                .unwrap_or([0; 8]);
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..8 {
                    if bits >> col & 1 == 1 {
                        self.set_pixel(pen + col, y + FONT_ASCENT - 8 + row as i32, c, clip);
                    }
                }
            }
            pen += FONT_ADVANCE;
        }
        pen - x
    }

    /// The pixels as an image; pixels of color `key` become transparent.
    pub fn to_image(&self, key: Option<Rgb>) -> Image {
        let key = key.map(Rgb::pack);
        Image {
            width: self.width,
            height: self.height,
            pixels: self
                .pixels
                .iter()
                .map(|&p| if Some(p) == key { p } else { 0xff00_0000 | p })
                .collect(),
        }
    }

    /// `0xRRGGBBAA`-ordered bytes, opaque, for PNG and for frontends.
    pub fn to_rgba(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .flat_map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8, 255])
            .collect()
    }

    pub fn to_png(&self) -> Vec<u8> {
        encode_png(self.width, self.height, &self.to_rgba())
    }
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    if let Ok(mut writer) = encoder.write_header() {
        let _ = writer.write_image_data(rgba);
    }
    out
}

pub fn text_width(text: &str) -> i32 {
    text.chars().count() as i32 * FONT_ADVANCE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgbval_round_trips() {
        let c = Rgb::new(1, 2, 3);
        assert_eq!(c.to_rgbval(), 0x0302_0100);
        assert_eq!(Rgb::from_rgbval(0x0302_01ff), c);
    }

    #[test]
    fn fill_and_frame_respect_clip() {
        let mut fb = Framebuffer::new(10, 10);
        let clip = Rect {
            x: 0,
            y: 0,
            w: 5,
            h: 10,
        };
        fb.fill_rect(
            Rect {
                x: 2,
                y: 2,
                w: 6,
                h: 2,
            },
            Rgb::WHITE,
            clip,
        );
        assert_eq!(fb.pixel(4, 2), Some(Rgb::WHITE));
        assert_eq!(fb.pixel(5, 2), Some(Rgb::BLACK));
    }

    #[test]
    fn decodes_bottom_up_24bit_bmp_with_color_key() {
        // 2x1, 24 bpp: blue pixel then magenta.
        let mut bmp = b"BM".to_vec();
        bmp.extend([0; 8]);
        bmp.extend(54u32.to_le_bytes());
        bmp.extend(40u32.to_le_bytes());
        bmp.extend(2i32.to_le_bytes());
        bmp.extend(1i32.to_le_bytes());
        bmp.extend(1u16.to_le_bytes());
        bmp.extend(24u16.to_le_bytes());
        bmp.extend([0; 24]);
        bmp.extend([255, 0, 0, 255, 0, 255, 0, 0]);
        let img = Image::decode(&bmp).unwrap();
        assert_eq!(img.pixels, vec![0xff00_00ff, 0x00ff_00ff]);
    }
}
