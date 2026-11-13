//! `IDisplay`, `IGraphics` and `IImage`: everything that draws on the screen.

use super::{CLR_USER_BACKGROUND, CLR_USER_LINE, CLR_USER_TEXT, EBADPARM, Halt, Machine, SUCCESS};
use crate::cpu::{CpuBackend, CpuError};
use crate::video::{self, Rect, Rgb};

/// "Use the current color" in `IDISPLAY_DrawRect`.
const RGB_NONE: u32 = 0xffff_ffff;
const IDF_RECT_FRAME: u32 = 0x1;
const IDF_RECT_FILL: u32 = 0x2;
const IDF_ALIGN_CENTER: u32 = 0x20;
const IDF_ALIGN_RIGHT: u32 = 0x40;
const IDF_ALIGN_MIDDLE: u32 = 0x200;
const IDF_ALIGN_BOTTOM: u32 = 0x400;
/// `IIMAGE_SetParm` parameters (`AEEImage.h`).
const IPARM_SIZE: u32 = 1;
const IPARM_OFFSET: u32 = 2;
const IPARM_CXFRAME: u32 = 3;
const IPARM_NFRAMES: u32 = 4;
const IPARM_ROP: u32 = 6;
const AEE_RO_COPY: u32 = 2;
const COLOR_DEPTH: u32 = 16;
/// Longest text drawn in one call; a missing terminator must not hang the host.
const MAX_TEXT: usize = 1024;

impl Machine {
    fn clip_rect(&self) -> Rect {
        self.clip.unwrap_or(self.target_bounds())
    }

    pub(super) fn read_rect(&self, addr: u32) -> Result<Option<Rect>, CpuError> {
        if addr == 0 {
            return Ok(None);
        }
        let mut b = [0u8; 8];
        self.cpu.read_mem(addr, &mut b)?;
        Ok(Some(Rect::from_bytes(b)))
    }

    fn read_points(&self, addr: u32, count: usize) -> Result<Vec<(i32, i32)>, CpuError> {
        let mut bytes = vec![0u8; count * 4];
        self.cpu.read_mem(addr, &mut bytes)?;
        let (ox, oy) = self.graphics.origin;
        Ok(bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| {
                (
                    i32::from(i16::from_le_bytes([c[0], c[1]])) + ox,
                    i32::from(i16::from_le_bytes([c[2], c[3]])) + oy,
                )
            })
            .collect())
    }

    /// `AECHAR` text, `n` characters or up to the terminator when `n` is negative.
    fn read_text(&self, addr: u32, n: i32) -> Result<String, CpuError> {
        let limit = if n < 0 {
            MAX_TEXT
        } else {
            (n as usize).min(MAX_TEXT)
        };
        let mut units = Vec::new();
        let mut unit = [0u8; 2];
        for i in 0..limit as u32 {
            self.cpu.read_mem(addr + i * 2, &mut unit)?;
            let u = u16::from_le_bytes(unit);
            if u == 0 {
                break;
            }
            units.push(u);
        }
        Ok(String::from_utf16_lossy(&units))
    }

    pub(super) fn display_method(&mut self, name: &str) -> Result<Option<u32>, Halt> {
        Ok(Some(match name {
            // int GetFontMetrics(IDisplay *, AEEFont, int *pnAscent, int *pnDescent)
            "GetFontMetrics" => {
                self.write_out(self.arg(2)?, video::FONT_ASCENT as u32)?;
                self.write_out(self.arg(3)?, video::FONT_DESCENT as u32)?;
                (video::FONT_ASCENT + video::FONT_DESCENT) as u32
            }
            // int MeasureTextEx(IDisplay *, AEEFont, const AECHAR *, int nChars, int nMaxWidth,
            //                   int *pnFits)
            "MeasureTextEx" => {
                let text = self.read_text(self.arg(2)?, self.arg(3)? as i32)?;
                let max = self.arg(4)? as i32;
                let count = text.chars().count() as i32;
                let fits = if max < 0 {
                    count
                } else {
                    count.min(max / video::FONT_ADVANCE)
                };
                self.write_out(self.arg(5)?, fits as u32)?;
                (fits * video::FONT_ADVANCE) as u32
            }
            // int DrawText(IDisplay *, AEEFont, const AECHAR *, int nChars, int x, int y,
            //              const AEERect *prcBackground, uint32 dwFlags)
            "DrawText" => {
                let text = self.read_text(self.arg(2)?, self.arg(3)? as i32)?;
                let (mut x, mut y) = (self.arg(4)? as i32, self.arg(5)? as i32);
                let background = self.read_rect(self.arg(6)?)?;
                let flags = self.arg(7)?;
                let clip = self.clip_rect();
                if let Some(r) = background {
                    if flags & IDF_RECT_FILL != 0 {
                        let color = self.colors[CLR_USER_BACKGROUND];
                        self.target_mut().fill_rect(r, color, clip);
                    }
                    // With an alignment flag, the rectangle decides that axis, not x/y.
                    let (w, h) = (
                        video::text_width(&text),
                        video::FONT_ASCENT + video::FONT_DESCENT,
                    );
                    if flags & IDF_ALIGN_CENTER != 0 {
                        x = r.x + (r.w - w) / 2;
                    } else if flags & IDF_ALIGN_RIGHT != 0 {
                        x = r.x + r.w - w;
                    }
                    if flags & IDF_ALIGN_MIDDLE != 0 {
                        y = r.y + (r.h - h) / 2;
                    } else if flags & IDF_ALIGN_BOTTOM != 0 {
                        y = r.y + r.h - h;
                    }
                }
                let clip = background.and_then(|r| r.intersect(clip)).unwrap_or(clip);
                let color = self.colors[CLR_USER_TEXT];
                self.target_mut().draw_text(&text, x, y, color, clip);
                SUCCESS
            }
            // void DrawRect(IDisplay *, const AEERect *, RGBVAL frame, RGBVAL fill, uint32 flags)
            // A null rectangle is the whole screen: that is how `IDISPLAY_ClearScreen` is built.
            "DrawRect" => {
                let rect = self
                    .read_rect(self.arg(1)?)?
                    .unwrap_or(self.target_bounds());
                let pick = |v: u32, item: usize, colors: &[Rgb]| match v {
                    RGB_NONE => colors[item],
                    v => Rgb::from_rgbval(v),
                };
                let frame = pick(self.arg(2)?, CLR_USER_LINE, &self.colors);
                let fill = pick(self.arg(3)?, CLR_USER_BACKGROUND, &self.colors);
                let flags = self.arg(4)?;
                let clip = self.clip_rect();
                if flags & IDF_RECT_FILL != 0 {
                    self.target_mut().fill_rect(rect, fill, clip);
                }
                if flags & IDF_RECT_FRAME != 0 {
                    self.target_mut().draw_frame(rect, frame, clip);
                }
                SUCCESS
            }
            "Update" | "UpdateEx" => {
                self.report.updates += 1;
                SUCCESS
            }
            // void BitBlt(IDisplay *, int xd, int yd, int w, int h, const void *pbmSource,
            //             int xs, int ys, AEERasterOp rop)
            "BitBlt" => {
                let (x, y, w, h) = (
                    self.arg(1)? as i32,
                    self.arg(2)? as i32,
                    self.arg(3)? as i32,
                    self.arg(4)? as i32,
                );
                let (source, xs, ys, rop) = (
                    self.arg(5)?,
                    self.arg(6)? as i32,
                    self.arg(7)? as i32,
                    self.arg(8)?,
                );
                if let Some(image) = self.blit_source(source, rop)? {
                    let clip = self.clip_rect();
                    let src = Rect { x: xs, y: ys, w, h };
                    self.target_mut()
                        .draw_image(&image, x, y, src, rop != AEE_RO_COPY, clip);
                }
                SUCCESS
            }
            // RGBVAL SetColor(IDisplay *, AEEClrItem, RGBVAL): returns the previous color.
            "SetColor" => {
                let item = self.arg(1)? as usize;
                let color = Rgb::from_rgbval(self.arg(2)?);
                match self.colors.get_mut(item) {
                    Some(slot) => std::mem::replace(slot, color).to_rgbval(),
                    None => RGB_NONE,
                }
            }
            "SetClipRect" => {
                self.clip = self.read_rect(self.arg(1)?)?;
                SUCCESS
            }
            "GetClipRect" => {
                let out = self.arg(1)?;
                if out != 0 {
                    let bytes = self.clip_rect().to_bytes();
                    self.cpu.write_mem(out, &bytes)?;
                }
                SUCCESS
            }
            // Returns the previous IFont*; there are no font objects yet.
            "SetFont" => 0,
            "SetAnnunciators" | "Backlight" => SUCCESS,
            _ => return Ok(None),
        }))
    }

    /// The image behind a `BitBlt` source: a native bitmap of ours, or a Windows BMP still in
    /// guest memory (BREW accepts both). Guest BMPs are decoded once per address and size.
    fn blit_source(
        &mut self,
        source: u32,
        rop: u32,
    ) -> Result<Option<std::rc::Rc<video::Image>>, Halt> {
        if let Some(image) = self.bitmaps.get(&source) {
            return Ok(Some(image.clone()));
        }
        if let Some(image) = self.bitmap_image(source, rop)? {
            return Ok(Some(image));
        }
        let Some(bytes) = self.guest_bmp(source)? else {
            return Ok(None);
        };
        let image = video::Image::decode(&bytes);
        if let Some(image) = &image {
            self.bitmaps.insert(source, image.clone());
        }
        Ok(image)
    }

    /// A Windows BMP at `addr`, read whole using the size its header declares.
    pub(super) fn guest_bmp(&self, addr: u32) -> Result<Option<Vec<u8>>, CpuError> {
        if addr == 0 {
            return Ok(None);
        }
        let mut header = [0u8; 6];
        if self.cpu.read_mem(addr, &mut header).is_err() || &header[..2] != b"BM" {
            return Ok(None);
        }
        let len = u32::from_le_bytes([header[2], header[3], header[4], header[5]]);
        if !(54..=8 * 1024 * 1024).contains(&len) {
            return Ok(None);
        }
        let mut bytes = vec![0u8; len as usize];
        self.cpu.read_mem(addr, &mut bytes)?;
        Ok(Some(bytes))
    }

    pub(super) fn graphics_method(&mut self, name: &str) -> Result<Option<u32>, Halt> {
        // Colors arrive as loose uint8 r, g, b (and alpha), not as an RGBVAL.
        let rgb = |m: &Self| -> Result<Rgb, CpuError> {
            Ok(Rgb::new(m.arg(1)? as u8, m.arg(2)? as u8, m.arg(3)? as u8))
        };
        let clip = self.graphics.clip.unwrap_or(self.gfx_target_bounds());
        let g = self.graphics;
        let color = rgb(self)?;
        let a1 = self.arg(1)?;
        Ok(Some(match name {
            "SetBackground" => std::mem::replace(&mut self.graphics.background, color).to_rgbval(),
            "SetColor" => std::mem::replace(&mut self.graphics.stroke, color).to_rgbval(),
            "SetFillColor" => std::mem::replace(&mut self.graphics.fill, color).to_rgbval(),
            "GetBackground" => g.background.to_rgbval(),
            "GetColor" => g.stroke.to_rgbval(),
            "GetFillColor" => g.fill.to_rgbval(),
            "SetFillMode" => u32::from(std::mem::replace(&mut self.graphics.fill_mode, a1 != 0)),
            "GetFillMode" => u32::from(g.fill_mode),
            "GetColorDepth" => COLOR_DEPTH,
            "Translate" => {
                self.graphics.origin = (self.arg(1)? as i16 as i32, self.arg(2)? as i16 as i32);
                SUCCESS
            }
            "DrawPoint" => {
                let p = self.read_points(self.arg(1)?, 1)?[0];
                self.gfx_target_mut().set_pixel(p.0, p.1, g.stroke, clip);
                SUCCESS
            }
            "DrawLine" => {
                let p = self.read_points(self.arg(1)?, 2)?;
                self.gfx_target_mut().draw_line(p[0], p[1], g.stroke, clip);
                SUCCESS
            }
            "DrawRect" | "ClearRect" => {
                let Some(mut r) = self.read_rect(self.arg(1)?)? else {
                    return Ok(Some(EBADPARM));
                };
                r.x += g.origin.0;
                r.y += g.origin.1;
                if name == "ClearRect" {
                    self.gfx_target_mut().fill_rect(r, g.background, clip);
                } else {
                    if g.fill_mode {
                        self.gfx_target_mut().fill_rect(r, g.fill, clip);
                    }
                    self.gfx_target_mut().draw_frame(r, g.stroke, clip);
                }
                SUCCESS
            }
            // AEECircle: { int16 cx, cy, r }.
            "DrawCircle" => {
                let addr = self.arg(1)?;
                let c = self.read_points(addr, 1)?[0];
                let mut r = [0u8; 2];
                self.cpu.read_mem(addr + 4, &mut r)?;
                let fill = g.fill_mode.then_some(g.fill);
                self.gfx_target_mut().draw_circle(
                    c.0,
                    c.1,
                    i32::from(i16::from_le_bytes(r)),
                    g.stroke,
                    fill,
                    clip,
                );
                SUCCESS
            }
            "DrawTriangle" => {
                let p = self.read_points(self.arg(1)?, 3)?;
                self.draw_polygon(&p, true, clip);
                SUCCESS
            }
            // AEEPolygon / AEEPolyline: { int16 len; AEEPoint *points }.
            "DrawPolygon" | "DrawPolyline" => {
                let addr = self.arg(1)?;
                let count = (self.cpu.read_u32(addr)? & 0xffff).min(1024) as usize;
                let points = self.read_points(self.cpu.read_u32(addr + 4)?, count)?;
                self.draw_polygon(&points, name == "DrawPolygon", clip);
                SUCCESS
            }
            "ClearViewport" => {
                let bounds = self.gfx_target_bounds();
                self.gfx_target_mut().fill_rect(bounds, g.background, clip);
                SUCCESS
            }
            // AEEClip: { int type; AEERect rect } — the rectangle comes after the type.
            "SetClip" => {
                let addr = self.arg(1)?;
                self.graphics.clip = if addr == 0 {
                    None
                } else {
                    self.read_rect(addr + 4)?
                };
                SUCCESS
            }
            "Update" => {
                self.report.updates += 1;
                SUCCESS
            }
            "EnableDoubleBuffer" | "SetPaintMode" | "SetViewport" | "SetAlgorithmHint"
            | "SetStrokeStyle" | "Pan" => SUCCESS,
            _ => return Ok(None),
        }))
    }

    fn draw_polygon(&mut self, points: &[(i32, i32)], closed: bool, clip: Rect) {
        let g = self.graphics;
        if closed && g.fill_mode {
            self.gfx_target_mut().fill_polygon(points, g.fill, clip);
        }
        for pair in points.windows(2) {
            self.gfx_target_mut()
                .draw_line(pair[0], pair[1], g.stroke, clip);
        }
        if closed && points.len() > 2 {
            self.gfx_target_mut()
                .draw_line(points[points.len() - 1], points[0], g.stroke, clip);
        }
    }

    pub(super) fn image_method(&mut self, name: &str) -> Result<Option<u32>, Halt> {
        let this = self.arg(0)?;
        match name {
            // void SetStream(IImage *, IAStream *): decode from a memory stream.
            "SetStream" => {
                let bytes = self.stream_bytes(self.arg(1)?)?;
                match crate::video::Image::decode(&bytes) {
                    Some(image) => {
                        self.images.insert(
                            this,
                            super::ImageObject {
                                image,
                                frame_width: 0,
                                transparent: true,
                                draw_size: None,
                                offset: (0, 0),
                            },
                        );
                    }
                    None => {
                        let magic: String =
                            bytes.iter().take(4).map(|b| format!("{b:02x}")).collect();
                        self.missing(format!("imagem de stream: formato não suportado ({magic})"));
                    }
                }
                return Ok(Some(SUCCESS));
            }
            // void Notify(IImage *, PFNIMAGEINFO pfn, void *pUser): the callback receives
            // (pUser, pImage, AEEImageInfo *, nErr) once the image is ready — deferred, since
            // calling into the guest from inside an API call would clobber its registers.
            "Notify" => {
                let (func, user) = (self.arg(1)?, self.arg(2)?);
                if func != 0 {
                    let info = self.heap.alloc(12).unwrap_or(0);
                    let err = match self.images.get(&this) {
                        Some(obj) => {
                            if info != 0 {
                                let mut b = [0u8; 10];
                                b[0..2].copy_from_slice(&(obj.image.width as u16).to_le_bytes());
                                b[2..4].copy_from_slice(&(obj.image.height as u16).to_le_bytes());
                                b[8..10].copy_from_slice(&(obj.image.width as u16).to_le_bytes());
                                self.cpu.write_mem(info, &b)?;
                            }
                            SUCCESS
                        }
                        None => super::EFAILED,
                    };
                    self.schedule_with(0, func, user, &[this, info, err]);
                }
                return Ok(Some(SUCCESS));
            }
            _ => {}
        }
        let Some(obj) = self.images.get(&this).cloned() else {
            return Ok(None);
        };
        let frame_w = if obj.frame_width == 0 {
            obj.image.width
        } else {
            obj.frame_width
        } as i32;
        Ok(Some(match name {
            "Release" => {
                self.images.remove(&this);
                0
            }
            // void Draw(IImage *, int x, int y) and DrawFrame(IImage *, int frame, int x, int y)
            "Draw" | "DrawFrame" => {
                let (frame, x, y) = if name == "Draw" {
                    (0, self.arg(1)? as i32, self.arg(2)? as i32)
                } else {
                    (
                        self.arg(1)? as i32,
                        self.arg(2)? as i32,
                        self.arg(3)? as i32,
                    )
                };
                let (w, h) = obj.draw_size.unwrap_or((frame_w, obj.image.height as i32));
                let src = Rect {
                    x: frame.max(0) * frame_w + obj.offset.0,
                    y: obj.offset.1,
                    w: w.min(frame_w),
                    h,
                };
                let clip = self.clip_rect();
                self.target_mut()
                    .draw_image(&obj.image, x, y, src, obj.transparent, clip);
                SUCCESS
            }
            // AEEImageInfo: { uint16 cx, cy, nColors; boolean bAnimated; uint16 cxFrame }.
            "GetInfo" => {
                let out = self.arg(1)?;
                if out != 0 {
                    let mut b = [0u8; 10];
                    b[0..2].copy_from_slice(&(obj.image.width as u16).to_le_bytes());
                    b[2..4].copy_from_slice(&(obj.image.height as u16).to_le_bytes());
                    b[4..6].copy_from_slice(&(1u16 << 15).to_le_bytes());
                    b[6] = u8::from(obj.frame_width != 0);
                    b[8..10].copy_from_slice(&(frame_w as u16).to_le_bytes());
                    self.cpu.write_mem(out, &b)?;
                }
                SUCCESS
            }
            // void SetParm(IImage *, int nParm, int p1, int p2)
            "SetParm" => {
                let (parm, p1, p2) = (self.arg(1)?, self.arg(2)?, self.arg(3)?);
                if let Some(o) = self.images.get_mut(&this) {
                    match parm {
                        IPARM_SIZE => o.draw_size = Some((p1 as i32, p2 as i32)),
                        IPARM_OFFSET => o.offset = (p1 as i32, p2 as i32),
                        IPARM_CXFRAME => o.frame_width = p1,
                        IPARM_NFRAMES if p1 > 0 => o.frame_width = o.image.width / p1,
                        IPARM_ROP => o.transparent = p1 != AEE_RO_COPY,
                        _ => {}
                    }
                }
                SUCCESS
            }
            // Animation is not driven yet; the first frame stays.
            "Start" | "Stop" => SUCCESS,
            _ => return Ok(None),
        }))
    }
}
