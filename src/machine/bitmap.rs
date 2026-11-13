//! `IBitmap`, `IDIB` and drawing destinations.
//!
//! The screen is the device bitmap (`IDISPLAY_GetDeviceBitmap` / `GetDestination`), a host
//! [`Framebuffer`]. Every other bitmap is an **`IDIB` in guest memory**, because games treat
//! them as one: they `QueryInterface` for `AEECLSID_DIB` to reach the pixels, and some create a
//! 0x0 DIB with `CreateDIBitmap` and point its `pBmp`/`cx`/`cy`/`nPitch` at a buffer of their
//! own. So the guest's struct is the truth: before a host operation the header and pixels are
//! read into a framebuffer, and after it they are written back.
//!
//! `NativeColor` is RGB565, the handsets' 16-bit format.

use std::rc::Rc;

use zeebx_core::brew::aee::Interface;
use zeebx_core::layout::vtable_addr;

use super::{ECLASSNOTSUPPORT, EFAILED, Halt, Machine, SUCCESS};
use crate::cpu::{CpuBackend, CpuError};
use crate::video::{Framebuffer, Image, Rect, Rgb};

/// The default transparency color: magenta, as RGB565.
const DEFAULT_TRANSPARENT: u32 = 0xf81f;
const AEE_RO_TRANSPARENT: u32 = 6;
const AEECLSID_DIB: u32 = 0x0100_1045;
const AEEIID_BITMAP: u32 = 0x0100_1044;

/// `struct IDIB` (`AEEBitmap.h`): pvt, pPaletteMap, pBmp, pRGB, ncTransparent, cx, cy, nPitch,
/// cntRGB, nDepth, nColorScheme, reserved[6].
const DIB_SIZE: u32 = 36;
const DIB_PBMP: u32 = 8;
const DIB_PRGB: u32 = 12;
const DIB_TRANSPARENT: u32 = 16;
const DIB_CX: u32 = 20;
const DIB_PITCH: u32 = 24;
const DIB_CNTRGB: u32 = 26;
const DIB_DEPTH: u32 = 28;
const DIB_SCHEME: u32 = 29;
const IDIB_COLORSCHEME_565: u8 = 16;
/// Largest DIB read from the guest; bigger dimensions are a game's uninitialized struct.
const MAX_DIB_PIXELS: u32 = 1024 * 1024;

/// The header fields that say where the pixels are and how to read them.
#[derive(Debug, Clone, Copy)]
struct DibHeader {
    pixels: u32,
    palette: u32,
    colors: u32,
    transparent: u32,
    width: u32,
    height: u32,
    pitch: i32,
    depth: u8,
}

pub(super) fn native_to_rgb(n: u32) -> Rgb {
    let (r, g, b) = ((n >> 11) & 0x1f, (n >> 5) & 0x3f, n & 0x1f);
    Rgb::new(
        (r << 3 | r >> 2) as u8,
        (g << 2 | g >> 4) as u8,
        (b << 3 | b >> 2) as u8,
    )
}

pub(super) fn rgb_to_native(c: Rgb) -> u32 {
    (u32::from(c.r) & 0xf8) << 8 | (u32::from(c.g) & 0xfc) << 3 | u32::from(c.b) >> 3
}

impl Machine {
    /// The bitmap `IDisplay` draws on: the screen unless `SetDestination` chose a DIB.
    pub(super) fn target_mut(&mut self) -> &mut Framebuffer {
        let target = self.display_target;
        self.surface_or_screen(target)
    }

    pub(super) fn target_bounds(&self) -> Rect {
        self.surface_ref(self.display_target).bounds()
    }

    pub(super) fn gfx_target_mut(&mut self) -> &mut Framebuffer {
        let target = self.gfx_target;
        self.surface_or_screen(target)
    }

    pub(super) fn gfx_target_bounds(&self) -> Rect {
        self.surface_ref(self.gfx_target).bounds()
    }

    fn surface_ref(&self, id: u32) -> &Framebuffer {
        self.surfaces.get(&id).unwrap_or(&self.screen)
    }

    fn surface_or_screen(&mut self, id: u32) -> &mut Framebuffer {
        if self.surfaces.contains_key(&id) {
            return self.surfaces.get_mut(&id).expect("checked above");
        }
        &mut self.screen
    }

    /// The screen as an `IBitmap`, created on first use.
    pub(super) fn device_bitmap(&mut self) -> Result<u32, Halt> {
        if self.device_bitmap == 0 {
            self.device_bitmap = self.new_object(Interface::Bitmap)?;
        }
        Ok(self.device_bitmap)
    }

    fn is_bitmap(&self, id: u32) -> bool {
        id != 0 && (id == self.device_bitmap || self.dibs.contains(&id))
    }

    fn dib_header(&self, id: u32) -> Result<Option<DibHeader>, CpuError> {
        if !self.dibs.contains(&id) {
            return Ok(None);
        }
        let mut b = [0u8; DIB_SIZE as usize];
        self.cpu.read_mem(id, &mut b)?;
        let u32_at = |o: u32| u32::from_le_bytes(b[o as usize..o as usize + 4].try_into().unwrap());
        let u16_at = |o: u32| u16::from_le_bytes([b[o as usize], b[o as usize + 1]]);
        Ok(Some(DibHeader {
            pixels: u32_at(DIB_PBMP),
            palette: u32_at(DIB_PRGB),
            colors: u32::from(u16_at(DIB_CNTRGB)),
            transparent: u32_at(DIB_TRANSPARENT),
            width: u32::from(u16_at(DIB_CX)),
            height: u32::from(u16_at(DIB_CX + 2)),
            pitch: i32::from(u16_at(DIB_PITCH) as i16),
            depth: b[DIB_DEPTH as usize],
        }))
    }

    /// Reads a DIB's pixels from the guest into its host framebuffer.
    pub(super) fn dib_load(&mut self, id: u32) -> Result<(), CpuError> {
        let Some(h) = self.dib_header(id)? else {
            return Ok(());
        };
        let mut fb = Framebuffer::new(h.width, h.height);
        if h.pixels != 0 && h.width * h.height <= MAX_DIB_PIXELS && h.width > 0 {
            let palette = if h.depth <= 8 && h.palette != 0 {
                let mut p = vec![0u8; (h.colors.min(256) * 4) as usize];
                self.cpu.read_mem(h.palette, &mut p)?;
                p.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| Rgb::new(c[2], c[1], c[0]))
                    .collect()
            } else {
                Vec::new()
            };
            let row_bytes = (h.width * u32::from(h.depth)).div_ceil(8) as usize;
            let mut row = vec![0u8; row_bytes];
            let bounds = fb.bounds();
            for y in 0..h.height {
                let addr = h.pixels.wrapping_add_signed(y as i32 * h.pitch);
                self.cpu.read_mem(addr, &mut row)?;
                for x in 0..h.width as usize {
                    let c = match h.depth {
                        16 => native_to_rgb(u32::from(u16::from_le_bytes([
                            row[x * 2],
                            row[x * 2 + 1],
                        ]))),
                        24 => Rgb::new(row[x * 3 + 2], row[x * 3 + 1], row[x * 3]),
                        32 => Rgb::new(row[x * 4 + 2], row[x * 4 + 1], row[x * 4]),
                        8 => palette
                            .get(usize::from(row[x]))
                            .copied()
                            .unwrap_or(Rgb::BLACK),
                        _ => Rgb::BLACK,
                    };
                    fb.set_pixel(x as i32, y as i32, c, bounds);
                }
            }
        }
        self.surfaces.insert(id, fb);
        Ok(())
    }

    /// Writes a DIB's host framebuffer back to guest memory (16 and 32 bpp; palettized DIBs are
    /// only read).
    pub(super) fn dib_store(&mut self, id: u32) -> Result<(), CpuError> {
        let (Some(h), Some(fb)) = (self.dib_header(id)?, self.surfaces.get(&id)) else {
            return Ok(());
        };
        if h.pixels == 0
            || fb.width() != h.width
            || fb.height() != h.height
            || !matches!(h.depth, 16 | 32)
        {
            return Ok(());
        }
        let rows: Vec<Vec<u8>> = (0..h.height as i32)
            .map(|y| {
                (0..h.width as i32)
                    .flat_map(|x| {
                        let c = fb.pixel(x, y).unwrap_or(Rgb::BLACK);
                        match h.depth {
                            16 => (rgb_to_native(c) as u16).to_le_bytes().to_vec(),
                            _ => vec![c.b, c.g, c.r, 0],
                        }
                    })
                    .collect()
            })
            .collect();
        for (y, row) in rows.iter().enumerate() {
            self.cpu
                .write_mem(h.pixels.wrapping_add_signed(y as i32 * h.pitch), row)?;
        }
        Ok(())
    }

    /// The DIBs a call may touch: the destinations and the object and source arguments.
    pub(super) fn involved_dibs(&self) -> Vec<u32> {
        let mut ids = vec![self.display_target, self.gfx_target];
        ids.extend([self.arg(0), self.arg(5)].into_iter().flatten());
        ids.retain(|id| self.dibs.contains(id));
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// A new 16-bit DIB in guest memory: the struct, then the pixels.
    fn new_dib(&mut self, width: u32, height: u32) -> Result<u32, Halt> {
        let pitch = (width * 2 + 3) & !3;
        let Some(addr) = self.heap.alloc(DIB_SIZE + pitch * height) else {
            return Ok(0);
        };
        let mut b = vec![0u8; (DIB_SIZE + pitch * height) as usize];
        b[0..4].copy_from_slice(&vtable_addr(Interface::Bitmap).to_le_bytes());
        let pixels = if width * height > 0 {
            addr + DIB_SIZE
        } else {
            0
        };
        b[DIB_PBMP as usize..][..4].copy_from_slice(&pixels.to_le_bytes());
        b[DIB_TRANSPARENT as usize..][..4].copy_from_slice(&DEFAULT_TRANSPARENT.to_le_bytes());
        b[DIB_CX as usize..][..2].copy_from_slice(&(width as u16).to_le_bytes());
        b[DIB_CX as usize + 2..][..2].copy_from_slice(&(height as u16).to_le_bytes());
        b[DIB_PITCH as usize..][..2].copy_from_slice(&(pitch as u16).to_le_bytes());
        b[DIB_DEPTH as usize] = 16;
        b[DIB_SCHEME as usize] = IDIB_COLORSCHEME_565;
        self.cpu.write_mem(addr, &b)?;
        self.dibs.insert(addr);
        Ok(addr)
    }

    /// The pixels of a bitmap as an image for blitting; with `rop` transparent, its
    /// transparency color becomes see-through.
    pub(super) fn bitmap_image(
        &mut self,
        id: u32,
        rop: u32,
    ) -> Result<Option<Rc<Image>>, CpuError> {
        if !self.is_bitmap(id) {
            return Ok(None);
        }
        let key = match self.dib_header(id)? {
            Some(h) => {
                self.dib_load(id)?;
                h.transparent
            }
            None => DEFAULT_TRANSPARENT,
        };
        let key = (rop == AEE_RO_TRANSPARENT).then(|| native_to_rgb(key));
        Ok(Some(Rc::new(self.surface_ref(id).to_image(key))))
    }

    /// `IDisplay`'s bitmap and destination methods.
    pub(super) fn display_destination(&mut self, name: &str) -> Result<Option<u32>, Halt> {
        Ok(Some(match name {
            // int GetDeviceBitmap(IDisplay *, IBitmap **)
            "GetDeviceBitmap" => {
                let device = self.device_bitmap()?;
                self.write_out(self.arg(1)?, device)?;
                SUCCESS
            }
            // IBitmap *GetDestination(IDisplay *): returned directly, not through a pointer.
            "GetDestination" => match self.display_target {
                0 => self.device_bitmap()?,
                t => t,
            },
            "SetDestination" => {
                let target = self.arg(1)?;
                self.display_target = if self.dibs.contains(&target) {
                    target
                } else {
                    0
                };
                if self.display_target != 0 {
                    self.dib_load(target)?;
                }
                SUCCESS
            }
            // int CreateDIBitmap(IDisplay *, IDIB **, uint8 colorDepth, uint16 w, uint16 h)
            "CreateDIBitmap" => {
                let (out, w, h) = (self.arg(1)?, self.arg(3)? & 0xffff, self.arg(4)? & 0xffff);
                if w * h > MAX_DIB_PIXELS {
                    return Ok(Some(EFAILED));
                }
                let dib = self.new_dib(w, h)?;
                self.write_out(out, dib)?;
                if dib == 0 { EFAILED } else { SUCCESS }
            }
            _ => return Ok(None),
        }))
    }

    pub(super) fn graphics_destination(&mut self, name: &str) -> Result<Option<u32>, Halt> {
        Ok(Some(match name {
            "GetDestination" => match self.gfx_target {
                0 => self.device_bitmap()?,
                t => t,
            },
            "SetDestination" => {
                let target = self.arg(1)?;
                self.gfx_target = if self.dibs.contains(&target) {
                    target
                } else {
                    0
                };
                if self.gfx_target != 0 {
                    self.dib_load(target)?;
                }
                SUCCESS
            }
            _ => return Ok(None),
        }))
    }

    pub(super) fn bitmap_method(&mut self, name: &str) -> Result<Option<u32>, Halt> {
        let this = self.arg(0)?;
        if !self.is_bitmap(this) {
            return Ok(None);
        }
        if self.dibs.contains(&this) && !self.surfaces.contains_key(&this) {
            self.dib_load(this)?;
        }
        let bounds = self.surface_ref(this).bounds();
        Ok(Some(match name {
            // Bitmaps live until the machine goes; the guest frees a DIB's memory itself.
            "Release" => 0,
            // int QueryInterface(IBitmap *, AEECLSID, void **): a DIB is its own IDIB and IBitmap.
            "QueryInterface" => {
                let (iid, out) = (self.arg(1)?, self.arg(2)?);
                let found =
                    matches!(iid, AEECLSID_DIB | AEEIID_BITMAP) && self.dibs.contains(&this);
                self.write_out(out, if found { this } else { 0 })?;
                if found { SUCCESS } else { ECLASSNOTSUPPORT }
            }
            "RGBToNative" => rgb_to_native(Rgb::from_rgbval(self.arg(1)?)),
            "NativeToRGB" => native_to_rgb(self.arg(1)?).to_rgbval(),
            // int DrawPixel(IBitmap *, unsigned x, unsigned y, NativeColor, AEERasterOp)
            "DrawPixel" => {
                let (x, y, c) = (self.arg(1)? as i32, self.arg(2)? as i32, self.arg(3)?);
                self.surface_or_screen(this)
                    .set_pixel(x, y, native_to_rgb(c), bounds);
                SUCCESS
            }
            // int GetPixel(IBitmap *, unsigned x, unsigned y, NativeColor *)
            "GetPixel" => {
                let (x, y, out) = (self.arg(1)? as i32, self.arg(2)? as i32, self.arg(3)?);
                match self.surface_ref(this).pixel(x, y) {
                    Some(c) => {
                        self.write_out(out, rgb_to_native(c))?;
                        SUCCESS
                    }
                    None => EFAILED,
                }
            }
            // int DrawHScanline(IBitmap *, unsigned y, unsigned xMin, unsigned xMax, NativeColor,
            //                   AEERasterOp)
            "DrawHScanline" => {
                let (y, x0, x1) = (
                    self.arg(1)? as i32,
                    self.arg(2)? as i32,
                    self.arg(3)? as i32,
                );
                let c = native_to_rgb(self.arg(4)?);
                let r = Rect {
                    x: x0,
                    y,
                    w: x1 - x0 + 1,
                    h: 1,
                };
                self.surface_or_screen(this).fill_rect(r, c, bounds);
                SUCCESS
            }
            // int FillRect(IBitmap *, const AEERect *, NativeColor, AEERasterOp)
            "FillRect" => {
                let Some(r) = self.read_rect(self.arg(1)?)? else {
                    return Ok(Some(EFAILED));
                };
                let c = native_to_rgb(self.arg(2)?);
                self.surface_or_screen(this).fill_rect(r, c, bounds);
                SUCCESS
            }
            // int BltIn(IBitmap *dst, int x, int y, int dx, int dy, IBitmap *src, int xs, int ys,
            //           AEERasterOp) — and BltOut, the same with source and destination swapped.
            "BltIn" | "BltOut" => {
                let (x, y, w, h) = (
                    self.arg(1)? as i32,
                    self.arg(2)? as i32,
                    self.arg(3)? as i32,
                    self.arg(4)? as i32,
                );
                let (other, xs, ys, rop) = (
                    self.arg(5)?,
                    self.arg(6)? as i32,
                    self.arg(7)? as i32,
                    self.arg(8)?,
                );
                let (dst, src) = if name == "BltIn" {
                    (this, other)
                } else {
                    (other, this)
                };
                let image = match self.bitmap_image(src, rop)? {
                    Some(image) => Some(image),
                    None => self.bitmaps.get(&src).cloned(),
                };
                if let (Some(image), true) = (image, self.is_bitmap(dst)) {
                    let clip = self.surface_ref(dst).bounds();
                    let part = Rect { x: xs, y: ys, w, h };
                    let transparent = rop == AEE_RO_TRANSPARENT;
                    self.surface_or_screen(dst)
                        .draw_image(&image, x, y, part, transparent, clip);
                }
                SUCCESS
            }
            // int GetInfo(IBitmap *, AEEBitmapInfo *, int nSize): { uint32 cx, cy, nDepth }
            "GetInfo" => {
                let out = self.arg(1)?;
                if out != 0 {
                    let mut b = [0u8; 12];
                    b[0..4].copy_from_slice(&(bounds.w as u32).to_le_bytes());
                    b[4..8].copy_from_slice(&(bounds.h as u32).to_le_bytes());
                    b[8..12].copy_from_slice(&16u32.to_le_bytes());
                    self.cpu.write_mem(out, &b)?;
                }
                SUCCESS
            }
            // int CreateCompatibleBitmap(IBitmap *, IBitmap **ppIBitmap, uint16 w, uint16 h)
            "CreateCompatibleBitmap" => {
                let (out, w, h) = (self.arg(1)?, self.arg(2)? & 0xffff, self.arg(3)? & 0xffff);
                if w == 0 || h == 0 || w * h > MAX_DIB_PIXELS {
                    return Ok(Some(EFAILED));
                }
                let dib = self.new_dib(w, h)?;
                self.write_out(out, dib)?;
                if dib == 0 { EFAILED } else { SUCCESS }
            }
            "SetTransparencyColor" => {
                let c = self.arg(1)?;
                if self.dibs.contains(&this) {
                    self.cpu.write_u32(this + DIB_TRANSPARENT, c)?;
                }
                SUCCESS
            }
            "GetTransparencyColor" => {
                let c = match self.dib_header(this)? {
                    Some(h) => h.transparent,
                    None => DEFAULT_TRANSPARENT,
                };
                self.write_out(self.arg(1)?, c)?;
                SUCCESS
            }
            _ => return Ok(None),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_colors_are_rgb565() {
        assert_eq!(rgb_to_native(Rgb::new(255, 0, 255)), 0xf81f);
        assert_eq!(native_to_rgb(0xffff), Rgb::WHITE);
        assert_eq!(
            native_to_rgb(rgb_to_native(Rgb::new(0, 255, 0))),
            Rgb::new(0, 255, 0)
        );
    }
}
