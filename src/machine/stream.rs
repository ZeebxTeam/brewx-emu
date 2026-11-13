//! `IMemAStream`: a stream over a buffer in guest memory. Games use it to feed image decoders
//! (`IIMAGE_SetStream`) with data they unpacked themselves.

use super::{Halt, Machine, SUCCESS};
use crate::cpu::{CpuBackend, CpuError};

/// Largest stream read in one go; bigger sizes are garbage.
const MAX_STREAM: u32 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct MemStream {
    buffer: u32,
    size: u32,
    pos: u32,
}

impl Machine {
    pub(super) fn memstream_method(&mut self, name: &str) -> Result<Option<u32>, Halt> {
        let this = self.arg(0)?;
        Ok(Some(match name {
            // void Set(IMemAStream *, byte *pBuf, uint32 dwSize, uint32 dwOffset, boolean bSysMem)
            // void SetEx(IMemAStream *, byte *pBuf, uint32 dwSize, uint32 dwOffset,
            //            PFNNOTIFY pfnFree, void *pUser)
            "Set" | "SetEx" => {
                let (buffer, size, pos) =
                    (self.arg(1)?, self.arg(2)?.min(MAX_STREAM), self.arg(3)?);
                self.streams.insert(
                    this,
                    MemStream {
                        buffer,
                        size,
                        pos: pos.min(size),
                    },
                );
                SUCCESS
            }
            // int32 Read(IMemAStream *, void *pDest, uint32 nWant)
            "Read" => {
                let (dest, want) = (self.arg(1)?, self.arg(2)?);
                let Some(s) = self.streams.get(&this).copied() else {
                    return Ok(Some(0));
                };
                let n = want.min(s.size - s.pos);
                let mut buf = vec![0u8; n as usize];
                self.cpu.read_mem(s.buffer + s.pos, &mut buf)?;
                self.cpu.write_mem(dest, &buf)?;
                if let Some(s) = self.streams.get_mut(&this) {
                    s.pos += n;
                }
                n
            }
            // void Readable(IMemAStream *, PFNNOTIFY, void *): memory is always readable.
            "Readable" => {
                let (func, user) = (self.arg(1)?, self.arg(2)?);
                if func != 0 {
                    self.schedule(0, func, user);
                }
                SUCCESS
            }
            "Cancel" => SUCCESS,
            _ => return Ok(None),
        }))
    }

    /// The unread bytes of a memory stream, for decoders that take the whole stream.
    pub(super) fn stream_bytes(&self, stream: u32) -> Result<Vec<u8>, CpuError> {
        let Some(s) = self.streams.get(&stream) else {
            return Ok(Vec::new());
        };
        let mut buf = vec![0u8; (s.size - s.pos) as usize];
        self.cpu.read_mem(s.buffer + s.pos, &mut buf)?;
        Ok(buf)
    }
}
