//! The `AEEHelperFuncs` table: BREW's stdlib. Modules do not link libc; `MALLOC`, `STRCPY` and
//! friends are calls through this table. Dispatch is by name (from `zeebx-core`), so the slot
//! numbers live in one place.

use zeebx_core::brew::aee::Interface;
use zeebx_core::brew::cformat::{self, ArgSource};

use super::{Halt, Machine, SUCCESS};
use crate::cpu::{CpuBackend, CpuError};

/// Longest guest string read in one go: a missing terminator must not hang the host.
const MAX_STRING: u32 = 64 * 1024;
/// `REALLOC`'s flag for "do not zero the new part", in the size's top bit.
const ALLOC_NO_ZMEM: u32 = 0x8000_0000;
/// No guest buffer is larger than the heap; a bigger length is garbage from the game, and
/// honoring it would allocate gigabytes on the host.
const MAX_BLOCK: u32 = zeebx_core::layout::HEAP_MAX_SIZE as u32;
/// `aee_GetSeconds` counts from 1980-01-06 (GPS epoch); start the virtual clock in mid-2005,
/// when most of these titles shipped.
const EPOCH_SECONDS: u64 = 803_000_000;

impl Machine {
    pub(super) fn helper(&mut self, slot: u32) -> Result<Option<u32>, Halt> {
        let Some(name) = Interface::Helpers.method(slot) else {
            return Ok(None);
        };
        let a = |m: &Self, n| m.arg(n);
        let value = match name {
            "malloc" => {
                let size = a(self, 0)?;
                self.malloc(size)?
            }
            "free" => {
                let ptr = a(self, 0)?;
                if ptr != 0 {
                    self.heap.free(ptr);
                }
                SUCCESS
            }
            "realloc" => {
                // The top bit asks for no zero-fill; the size is the rest.
                let (ptr, size) = (a(self, 0)?, a(self, 1)? & !ALLOC_NO_ZMEM);
                self.realloc(ptr, size)?
            }
            "memmove" => {
                let (dst, src, n) = (a(self, 0)?, a(self, 1)?, len(a(self, 2)?)?);
                let mut buf = vec![0; n as usize];
                self.cpu.read_mem(src, &mut buf)?;
                self.cpu.write_mem(dst, &buf)?;
                dst
            }
            "memset" => {
                let (dst, c, n) = (a(self, 0)?, a(self, 1)?, len(a(self, 2)?)?);
                self.cpu.write_mem(dst, &vec![c as u8; n as usize])?;
                dst
            }
            "memcmp" => {
                let (x, y, n) = (a(self, 0)?, a(self, 1)?, len(a(self, 2)?)?);
                let (mut bx, mut by) = (vec![0; n as usize], vec![0; n as usize]);
                self.cpu.read_mem(x, &mut bx)?;
                self.cpu.read_mem(y, &mut by)?;
                ordering(bx.cmp(&by))
            }
            "memchr" => {
                let (s, c, n) = (a(self, 0)?, a(self, 1)?, len(a(self, 2)?)?);
                let mut buf = vec![0; n as usize];
                self.cpu.read_mem(s, &mut buf)?;
                buf.iter()
                    .position(|&b| b == c as u8)
                    .map_or(0, |i| s + i as u32)
            }
            "strlen" => {
                let s = a(self, 0)?;
                self.cstr(s)?.len() as u32
            }
            "strcpy" => {
                let (dst, src) = (a(self, 0)?, a(self, 1)?);
                let mut bytes = self.cstr(src)?;
                bytes.push(0);
                self.cpu.write_mem(dst, &bytes)?;
                dst
            }
            "strncpy" => {
                let (dst, src, n) = (a(self, 0)?, a(self, 1)?, len(a(self, 2)?)?);
                let mut bytes = self.cstr(src)?;
                bytes.resize(n as usize, 0);
                self.cpu.write_mem(dst, &bytes)?;
                dst
            }
            "strcat" => {
                let (dst, src) = (a(self, 0)?, a(self, 1)?);
                let end = dst + self.cstr(dst)?.len() as u32;
                let mut bytes = self.cstr(src)?;
                bytes.push(0);
                self.cpu.write_mem(end, &bytes)?;
                dst
            }
            "strcmp" | "stricmp" => {
                let (x, y) = (self.cstr(a(self, 0)?)?, self.cstr(a(self, 1)?)?);
                compare(&x, &y, name == "stricmp")
            }
            "strncmp" | "strnicmp" => {
                let n = a(self, 2)? as usize;
                let (mut x, mut y) = (self.cstr(a(self, 0)?)?, self.cstr(a(self, 1)?)?);
                x.truncate(n);
                y.truncate(n);
                compare(&x, &y, name == "strnicmp")
            }
            "strchr" | "strrchr" => {
                let (s, c) = (a(self, 0)?, a(self, 1)? as u8);
                let mut bytes = self.cstr(s)?;
                // The terminator itself can be searched for.
                bytes.push(0);
                let found = if name == "strchr" {
                    bytes.iter().position(|&b| b == c)
                } else {
                    bytes.iter().rposition(|&b| b == c)
                };
                found.map_or(0, |i| s + i as u32)
            }
            "strstr" => {
                let (s, needle) = (a(self, 0)?, a(self, 1)?);
                let (hay, needle) = (self.cstr(s)?, self.cstr(needle)?);
                if needle.is_empty() {
                    s
                } else {
                    hay.windows(needle.len())
                        .position(|w| w == needle.as_slice())
                        .map_or(0, |i| s + i as u32)
                }
            }
            "atoi" => {
                let text = String::from_utf8_lossy(&self.cstr(a(self, 0)?)?).into_owned();
                atoi(&text) as u32
            }
            "wstrlen" => {
                let s = a(self, 0)?;
                self.wstr(s)?.len() as u32
            }
            "wstrcpy" => {
                let (dst, src) = (a(self, 0)?, a(self, 1)?);
                let mut units = self.wstr(src)?;
                units.push(0);
                let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
                self.cpu.write_mem(dst, &bytes)?;
                dst
            }
            "sprintf" | "snprintf" => {
                let buf = a(self, 0)?;
                let (cap, fmt_at) = match name {
                    "snprintf" => (Some(a(self, 1)? as usize), 2),
                    _ => (None, 1),
                };
                let fmt = String::from_utf8_lossy(&self.cstr(a(self, fmt_at)?)?).into_owned();
                let text = cformat::format(&fmt, &mut Args::new(self, fmt_at + 1));
                let mut bytes = text.into_bytes();
                let len = bytes.len() as u32;
                if let Some(cap) = cap {
                    bytes.truncate(cap.saturating_sub(1));
                }
                if buf != 0 && cap != Some(0) {
                    bytes.push(0);
                    self.cpu.write_mem(buf, &bytes)?;
                }
                len
            }
            "dbgprintf" => {
                let fmt = String::from_utf8_lossy(&self.cstr(a(self, 0)?)?).into_owned();
                let text = cformat::format(&fmt, &mut Args::new(self, 1));
                self.log(text.trim_end().to_owned());
                SUCCESS
            }
            // void *SetupNativeImage(AEECLSID cls, void *pBuffer, AEEImageInfo *pii,
            //                        boolean *pbRealloc): what `CONVERTBMP` calls.
            "SetupNativeImage" => {
                let (buffer, info, realloc) = (a(self, 1)?, a(self, 2)?, a(self, 3)?);
                self.setup_native_image(buffer, info, realloc)?
            }
            // char *WSTRTOSTR(const AECHAR *src, char *dst, int nSize)
            "wstrtostr" => {
                let (src, dst, size) = (a(self, 0)?, a(self, 1)?, a(self, 2)? as usize);
                if dst != 0 && size > 0 {
                    let mut bytes: Vec<u8> = self
                        .wstr(src)?
                        .iter()
                        .map(|&u| u8::try_from(u).unwrap_or(b'?'))
                        .take(size - 1)
                        .collect();
                    bytes.push(0);
                    self.cpu.write_mem(dst, &bytes)?;
                }
                dst
            }
            // AECHAR *STRTOWSTR(const char *src, AECHAR *dst, int nSizeBytes)
            "strtowstr" => {
                let (src, dst, size) = (a(self, 0)?, a(self, 1)?, a(self, 2)? as usize);
                if dst != 0 && size >= 2 {
                    let mut bytes: Vec<u8> = self
                        .cstr(src)?
                        .iter()
                        .take(size / 2 - 1)
                        .flat_map(|&b| u16::from(b).to_le_bytes())
                        .collect();
                    bytes.extend([0, 0]);
                    self.cpu.write_mem(dst, &bytes)?;
                }
                dst
            }
            "GetAEEVersion" => self.aee_version(a(self, 0)?, a(self, 1)?)?,
            "GetAppInstance" => self.applet,
            "aee_GetTimeMS" | "aee_GetUpTimeMS" => self.now_ms() as u32,
            "aee_GetSeconds" => (EPOCH_SECONDS + self.now_ms() / 1000) as u32,
            "aee_GetRand" => {
                let (dst, n) = (a(self, 0)?, len(a(self, 1)?)?);
                // Deterministic on purpose: same run, same "random" numbers.
                let mut state = self.cpu.instructions() as u32 | 1;
                let bytes: Vec<u8> = (0..n)
                    .map(|_| {
                        state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                        (state >> 16) as u8
                    })
                    .collect();
                self.cpu.write_mem(dst, &bytes)?;
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(value))
    }

    /// Decodes a Windows BMP in guest memory into a native bitmap and returns its handle.
    fn setup_native_image(&mut self, buffer: u32, info: u32, realloc: u32) -> Result<u32, Halt> {
        if realloc != 0 {
            // The result lives in an allocation of ours, which is what this flag reports.
            self.cpu.write_mem(realloc, &[1])?;
        }
        let Some(image) = self
            .guest_bmp(buffer)?
            .and_then(|b| crate::video::Image::decode(&b))
        else {
            return Ok(0);
        };
        let handle = self.new_object(Interface::Bitmap)?;
        if info != 0 {
            // AEEImageInfo: cx, cy, nColors, bAnimated, cxFrame.
            let (cx, cy) = (image.width as u16, image.height as u16);
            let mut b = [0u8; 10];
            b[0..2].copy_from_slice(&cx.to_le_bytes());
            b[2..4].copy_from_slice(&cy.to_le_bytes());
            b[8..10].copy_from_slice(&cx.to_le_bytes());
            self.cpu.write_mem(info, &b)?;
        }
        self.bitmaps.insert(handle, image);
        Ok(handle)
    }

    /// `MALLOC` returns zeroed memory on every BREW version.
    fn malloc(&mut self, size: u32) -> Result<u32, CpuError> {
        // Checked here because `Heap::alloc` rounds up without overflow checks (zeebx-core 0.1.1).
        if size > MAX_BLOCK {
            return Ok(0);
        }
        let Some(ptr) = self.heap.alloc(size) else {
            return Ok(0);
        };
        self.cpu.write_mem(ptr, &vec![0; size as usize])?;
        Ok(ptr)
    }

    fn realloc(&mut self, ptr: u32, size: u32) -> Result<u32, CpuError> {
        if ptr == 0 {
            return self.malloc(size);
        }
        if size == 0 {
            self.heap.free(ptr);
            return Ok(0);
        }
        let old = self.heap.size_of(ptr).unwrap_or(0);
        let new = self.malloc(size)?;
        if new != 0 {
            let mut buf = vec![0; old.min(size) as usize];
            self.cpu.read_mem(ptr, &mut buf)?;
            self.cpu.write_mem(new, &buf)?;
            self.heap.free(ptr);
        }
        Ok(new)
    }

    /// `uint32 GETAEEVERSION(byte *pszFormatted, int nSize, int nFlags)`: the version packed as
    /// `major.minor.sub.build` in one byte each, and optionally as text.
    ///
    /// It is the one place where the selected version reaches the game directly.
    fn aee_version(&mut self, buf: u32, size: u32) -> Result<u32, CpuError> {
        let (major, minor) = (self.config.version.major(), self.config.version.minor());
        if buf != 0 && size > 0 {
            let mut text = format!("{major}.{minor}.0.0").into_bytes();
            text.truncate(size as usize - 1);
            text.push(0);
            self.cpu.write_mem(buf, &text)?;
        }
        Ok(u32::from(major) << 24 | u32::from(minor) << 16)
    }

    pub(super) fn cstr(&self, addr: u32) -> Result<Vec<u8>, CpuError> {
        let mut out = Vec::new();
        let mut byte = [0u8; 1];
        for offset in 0..MAX_STRING {
            self.cpu.read_mem(addr.wrapping_add(offset), &mut byte)?;
            if byte[0] == 0 {
                break;
            }
            out.push(byte[0]);
        }
        Ok(out)
    }

    fn wstr(&self, addr: u32) -> Result<Vec<u16>, CpuError> {
        let mut out = Vec::new();
        let mut unit = [0u8; 2];
        for offset in 0..MAX_STRING {
            self.cpu
                .read_mem(addr.wrapping_add(offset * 2), &mut unit)?;
            let u = u16::from_le_bytes(unit);
            if u == 0 {
                break;
            }
            out.push(u);
        }
        Ok(out)
    }
}

/// Variadic arguments of the current call, starting at argument `next`.
struct Args<'m> {
    machine: &'m Machine,
    next: usize,
}

impl<'m> Args<'m> {
    fn new(machine: &'m Machine, next: usize) -> Self {
        Self { machine, next }
    }
}

impl ArgSource for Args<'_> {
    fn next_word(&mut self) -> u32 {
        let word = self.machine.arg(self.next).unwrap_or(0);
        self.next += 1;
        word
    }

    fn read_cstring(&mut self, addr: u32) -> String {
        // Handset text is Latin-1 more often than UTF-8; each byte maps to one char.
        self.machine
            .cstr(addr)
            .map(|b| b.iter().map(|&c| c as char).collect())
            .unwrap_or_default()
    }

    fn read_wide_string(&mut self, addr: u32) -> String {
        self.machine
            .wstr(addr)
            .map(|u| String::from_utf16_lossy(&u))
            .unwrap_or_default()
    }
}

fn len(n: u32) -> Result<u32, Halt> {
    if n > MAX_BLOCK {
        return Err(Halt::Fault(format!("tamanho de bloco absurdo: {n:#x}")));
    }
    Ok(n)
}

fn ordering(o: std::cmp::Ordering) -> u32 {
    o as i32 as u32
}

fn compare(x: &[u8], y: &[u8], ignore_case: bool) -> u32 {
    if ignore_case {
        ordering(x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase()))
    } else {
        ordering(x.cmp(y))
    }
}

fn atoi(text: &str) -> i32 {
    let t = text.trim_start();
    let (sign, digits) = match t.as_bytes().first() {
        Some(b'-') => (-1, &t[1..]),
        Some(b'+') => (1, &t[1..]),
        _ => (1, t),
    };
    let n = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i32, |acc, d| {
            acc.wrapping_mul(10).wrapping_add(i32::from(d - b'0'))
        });
    sign * n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atoi_like_c() {
        assert_eq!(atoi("  -42abc"), -42);
        assert_eq!(atoi("+7"), 7);
        assert_eq!(atoi("x1"), 0);
    }

    #[test]
    fn comparisons_return_c_signs() {
        assert_eq!(compare(b"abc", b"abd", false) as i32, -1);
        assert_eq!(compare(b"ABC", b"abc", true), 0);
        assert_eq!(compare(b"b", b"a", false), 1);
    }
}
