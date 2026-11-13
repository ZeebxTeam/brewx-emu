//! `.bar` resources: `ISHELL_LoadResData`, `LoadResString` and `LoadResObject`.

use zeebx_core::brew::aee::Interface;
use zeebx_core::formats::resfile::{self, RESTYPE_IMAGE, RESTYPE_STRING, ResFile};

use super::{Halt, ImageObject, Machine};
use crate::cpu::CpuBackend;
use crate::video::Image;

impl Machine {
    /// The resource file named by the guest string at `name_ptr`, parsed once and cached.
    fn res_file(&mut self, name_ptr: u32) -> Result<Option<&ResFile>, Halt> {
        let name = String::from_utf8_lossy(&self.cstr(name_ptr)?).into_owned();
        let Some(path) = self.vfs.resolve(&name) else {
            self.missing(name);
            return Ok(None);
        };
        if !self.resources.contains_key(&path) {
            let parsed = std::fs::read(&path)
                .ok()
                .and_then(|d| ResFile::parse(d).ok());
            if parsed.is_none() {
                self.missing(name);
            }
            self.resources.insert(path.clone(), parsed);
        }
        Ok(self.resources.get(&path).and_then(Option::as_ref))
    }

    /// `void *ISHELL_LoadResData(IShell *, const char *file, uint16 id, ResType type)` and the
    /// `Ex` form with `void *pBuf, uint32 *pnSize`. The block comes out raw, `AEEResBlob` header
    /// included: the game reads it with `RESBLOB_DATA()`.
    pub(super) fn load_res_data(&mut self, with_buffer: bool) -> Result<u32, Halt> {
        let (file, id, kind) = (self.arg(1)?, self.arg(2)? as u16, self.arg(3)? as u16);
        let Some(data) = self
            .res_file(file)?
            .and_then(|r| r.get(kind, id))
            .map(<[u8]>::to_vec)
        else {
            return Ok(0);
        };
        let len = data.len() as u32;
        if !with_buffer {
            return self.copy_to_heap(&data);
        }
        let (buffer, size_out) = (self.arg(4)?, self.arg(5)?);
        // pBuf == -1 only asks for the size.
        if buffer == u32::MAX {
            self.write_out(size_out, len)?;
            return Ok(u32::MAX);
        }
        if buffer == 0 {
            let ptr = self.copy_to_heap(&data)?;
            self.write_out(size_out, len)?;
            return Ok(ptr);
        }
        let declared = if size_out == 0 {
            0
        } else {
            self.cpu.read_u32(size_out)?
        };
        // Not every game fills `*pnSize` in; the heap block's size is the honest capacity.
        let capacity = declared.max(self.heap.size_of(buffer).unwrap_or(0));
        self.write_out(size_out, len)?;
        if capacity < len {
            return Ok(0);
        }
        self.cpu.write_mem(buffer, &data)?;
        Ok(buffer)
    }

    /// `int ISHELL_LoadResString(IShell *, const char *file, int16 id, AECHAR *buf, int size)`:
    /// `size` is in bytes; returns the characters written.
    pub(super) fn load_res_string(&mut self) -> Result<u32, Halt> {
        let (file, id) = (self.arg(1)?, self.arg(2)? as u16);
        let (buffer, size) = (self.arg(3)?, self.arg(4)?);
        let Some(text) = self
            .res_file(file)?
            .and_then(|r| r.get(RESTYPE_STRING, id))
            .map(decode_string)
        else {
            return Ok(0);
        };
        if buffer == 0 || size < 2 {
            return Ok(0);
        }
        let written = text.len().min((size as usize / 2).saturating_sub(1));
        let bytes: Vec<u8> = text[..written]
            .iter()
            .chain(std::iter::once(&0))
            .flat_map(|u| u.to_le_bytes())
            .collect();
        self.cpu.write_mem(buffer, &bytes)?;
        Ok(written as u32)
    }

    /// `void *ISHELL_LoadResObject(IShell *, const char *file, uint16 id, AEECLSID cls)`: in
    /// practice `ISHELL_LoadResImage`, an `IImage` over an image resource. Id 0 loads the file
    /// itself as the image.
    pub(super) fn load_res_object(&mut self) -> Result<u32, Halt> {
        let (file, id) = (self.arg(1)?, self.arg(2)? as u16);
        let bytes = if id == 0 {
            let name = String::from_utf8_lossy(&self.cstr(file)?).into_owned();
            match self.vfs.resolve(&name).and_then(|p| std::fs::read(p).ok()) {
                Some(b) => b,
                None => {
                    self.missing(name);
                    return Ok(0);
                }
            }
        } else {
            let Some(raw) = self
                .res_file(file)?
                .and_then(|r| r.get(RESTYPE_IMAGE, id))
                .map(<[u8]>::to_vec)
            else {
                return Ok(0);
            };
            // `LoadResObject` hands out an object, so the blob header is ours to skip.
            resfile::blob_data(&raw).unwrap_or(&raw).to_vec()
        };
        self.new_image(&bytes, format!("imagem do recurso {id}"))
    }

    /// An `IImage` object over encoded bytes, or 0 when the format is not supported yet.
    pub(super) fn new_image(&mut self, bytes: &[u8], what: String) -> Result<u32, Halt> {
        let Some(image) = Image::decode(bytes) else {
            let magic: String = bytes.iter().take(4).map(|b| format!("{b:02x}")).collect();
            self.missing(format!("{what}: formato não suportado ({magic})"));
            return Ok(0);
        };
        let object = self.new_object(Interface::Image)?;
        self.images.insert(
            object,
            ImageObject {
                image,
                frame_width: 0,
                transparent: true,
                draw_size: None,
                offset: (0, 0),
            },
        );
        Ok(object)
    }

    fn copy_to_heap(&mut self, data: &[u8]) -> Result<u32, Halt> {
        let Some(ptr) = self.heap.alloc(data.len() as u32) else {
            return Ok(0);
        };
        self.cpu.write_mem(ptr, data)?;
        Ok(ptr)
    }
}

/// A string resource as UTF-16.
///
/// Zeebo `.bar` files put an encoding byte before Latin-1 text, and `zeebx-core`'s
/// `ResFile::string` always drops it. Older `.bar` files have no such byte: Asteroids packs
/// `"Play"`, `"Menu"`, ... back to back, and dropping the first byte showed "lay" on the title
/// screen. Encoding ids are small numbers, so a printable first byte is text.
fn decode_string(raw: &[u8]) -> Vec<u16> {
    let wide = |pairs: &[u8], big: bool| -> Vec<u16> {
        pairs
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| {
                if big {
                    u16::from_be_bytes([p[0], p[1]])
                } else {
                    u16::from_le_bytes([p[0], p[1]])
                }
            })
            .take_while(|&u| u != 0)
            .collect()
    };
    let latin1 = match raw {
        [0xff, 0xfe, rest @ ..] => return wide(rest, false),
        [0xfe, 0xff, rest @ ..] => return wide(rest, true),
        [first, rest @ ..] if *first < 0x20 => rest,
        _ => raw,
    };
    latin1
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| u16::from(b))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::decode_string;

    #[test]
    fn strings_with_and_without_encoding_byte() {
        let s = |v: Vec<u16>| String::from_utf16(&v).unwrap();
        assert_eq!(s(decode_string(b"Play")), "Play");
        assert_eq!(s(decode_string(b"\x01Play\0junk")), "Play");
        assert_eq!(
            s(decode_string(&[0xff, 0xfe, b'O', 0, b'i', 0, 0, 0])),
            "Oi"
        );
    }
}
