//! `IFileMgr` and `IFile`, over `zeebx-core`'s VFS: the package is read-only underneath, and
//! everything the game writes lands in the data directory.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use zeebx_core::brew::aee::Interface;
use zeebx_core::brew::vfs::OpenIntent;

use super::{EBADPARM, EFAILED, EFILENOEXISTS, Halt, Machine, SUCCESS};
use crate::cpu::{CpuBackend, CpuError};

/// `_OFM_*` open modes (`AEEFile.h`).
const OFM_READ: u32 = 0x1;
const OFM_READWRITE: u32 = 0x2;
const OFM_CREATE: u32 = 0x4;
const OFM_APPEND: u32 = 0x8;
const SEEK_START: u32 = 0;
const SEEK_END: u32 = 1;
const SEEK_CURRENT: u32 = 2;
/// `FileInfo`: `{ char attrib; uint32 dwCreationDate; uint32 dwSize; char szName[64] }`.
const FILE_INFO_NAME: u32 = 12;
const FILE_NAME_LEN: usize = 64;
/// What `GetFreeSpace` reports: generous for a handset, small enough for 32-bit math.
const FREE_SPACE: u32 = 4 * 1024 * 1024;
/// Largest single read or write; anything bigger is garbage from the game.
const MAX_IO: u32 = 16 * 1024 * 1024;

pub(super) struct OpenFile {
    file: fs::File,
    name: String,
}

impl Machine {
    fn guest_path(&self, ptr: u32) -> Result<String, CpuError> {
        Ok(String::from_utf8_lossy(&self.cstr(ptr)?).into_owned())
    }

    pub(super) fn filemgr_method(&mut self, name: &str) -> Result<Option<u32>, Halt> {
        Ok(Some(match name {
            // IFile *OpenFile(IFileMgr *, const char *path, OpenFileMode mode)
            "OpenFile" => {
                let (path, mode) = (self.guest_path(self.arg(1)?)?, self.arg(2)?);
                match self.open(&path, mode) {
                    Some(file) => {
                        let object = self.new_object(Interface::File)?;
                        self.files.insert(object, OpenFile { file, name: path });
                        object
                    }
                    None => {
                        if mode & OFM_CREATE == 0 {
                            self.missing(path);
                        }
                        0
                    }
                }
            }
            "Test" => {
                let path = self.guest_path(self.arg(1)?)?;
                match self.vfs.resolve(&path) {
                    Some(p) if p.exists() => SUCCESS,
                    _ => EFILENOEXISTS,
                }
            }
            // int GetInfo(IFileMgr *, const char *path, FileInfo *)
            "GetInfo" => {
                let (path, out) = (self.guest_path(self.arg(1)?)?, self.arg(2)?);
                match self.vfs.resolve(&path).and_then(|p| fs::metadata(&p).ok()) {
                    Some(meta) => {
                        self.write_file_info(out, &path, meta.len() as u32, meta.is_dir())?;
                        SUCCESS
                    }
                    None => EFILENOEXISTS,
                }
            }
            "Remove" => {
                let path = self.guest_path(self.arg(1)?)?;
                match self.vfs.writable(&path).map(fs::remove_file) {
                    Some(Ok(())) => SUCCESS,
                    _ => EFILENOEXISTS,
                }
            }
            "MkDir" => {
                let path = self.guest_path(self.arg(1)?)?;
                match self.vfs.writable(&path).map(fs::create_dir_all) {
                    Some(Ok(())) => SUCCESS,
                    _ => EFAILED,
                }
            }
            "RmDir" => {
                let path = self.guest_path(self.arg(1)?)?;
                match self.vfs.writable(&path).map(fs::remove_dir) {
                    Some(Ok(())) => SUCCESS,
                    _ => EFAILED,
                }
            }
            // uint32 GetFreeSpace(IFileMgr *, uint32 *pdwTotal)
            "GetFreeSpace" => {
                self.write_out(self.arg(1)?, FREE_SPACE)?;
                FREE_SPACE
            }
            "GetLastError" => EFILENOEXISTS,
            _ => return Ok(None),
        }))
    }

    fn open(&self, guest: &str, mode: u32) -> Option<fs::File> {
        let intent = if mode & OFM_CREATE != 0 {
            OpenIntent::Create
        } else if mode & OFM_APPEND != 0 {
            OpenIntent::Append
        } else if mode & OFM_READWRITE != 0 {
            OpenIntent::ReadWrite
        } else {
            OpenIntent::Read
        };
        let target = self.vfs.open_target(guest, intent)?;
        if intent == OpenIntent::Read || mode == OFM_READ {
            return fs::File::open(&target.path).ok();
        }
        if let Some(parent) = target.path.parent() {
            fs::create_dir_all(parent).ok()?;
        }
        // Copy-on-write: a package file opened for writing is copied out first.
        if let Some(from) = &target.copy_from
            && !target.path.exists()
        {
            fs::copy(from, &target.path).ok()?;
        }
        if intent == OpenIntent::Create && target.path.exists() {
            // BREW refuses to create over an existing file.
            return None;
        }
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(intent == OpenIntent::Create)
            .append(intent == OpenIntent::Append)
            .open(&target.path)
            .ok()
    }

    fn write_file_info(
        &mut self,
        out: u32,
        name: &str,
        size: u32,
        dir: bool,
    ) -> Result<(), CpuError> {
        if out == 0 {
            return Ok(());
        }
        let mut b = vec![0u8; FILE_INFO_NAME as usize + FILE_NAME_LEN];
        b[0] = if dir { 0x10 } else { 0 };
        b[8..12].copy_from_slice(&size.to_le_bytes());
        let base = Path::new(name)
            .file_name()
            .map_or(name.to_owned(), |n| n.to_string_lossy().into_owned());
        let len = base.len().min(FILE_NAME_LEN - 1);
        b[12..12 + len].copy_from_slice(&base.as_bytes()[..len]);
        self.cpu.write_mem(out, &b)?;
        Ok(())
    }

    pub(super) fn file_method(&mut self, name: &str) -> Result<Option<u32>, Halt> {
        let this = self.arg(0)?;
        if name == "Release" {
            self.files.remove(&this);
            return Ok(Some(0));
        }
        if !self.files.contains_key(&this) {
            return Ok(None);
        }
        Ok(Some(match name {
            // int32 Read(IFile *, void *buf, uint32 n)
            "Read" => {
                let (buf, n) = (self.arg(1)?, self.arg(2)?.min(MAX_IO));
                let mut data = vec![0u8; n as usize];
                let read = self
                    .files
                    .get_mut(&this)
                    .map_or(0, |f| f.file.read(&mut data).unwrap_or(0));
                self.cpu.write_mem(buf, &data[..read])?;
                read as u32
            }
            // uint32 Write(IFile *, const void *buf, uint32 n)
            "Write" => {
                let (buf, n) = (self.arg(1)?, self.arg(2)?.min(MAX_IO));
                let mut data = vec![0u8; n as usize];
                self.cpu.read_mem(buf, &mut data)?;
                self.files
                    .get_mut(&this)
                    .map_or(0, |f| f.file.write(&data).unwrap_or(0)) as u32
            }
            // int Seek(IFile *, FileSeekType, int32 pos)
            "Seek" => {
                let (whence, pos) = (self.arg(1)?, self.arg(2)? as i32 as i64);
                let to = match whence {
                    SEEK_START => SeekFrom::Start(pos.max(0) as u64),
                    SEEK_END => SeekFrom::End(pos),
                    SEEK_CURRENT => SeekFrom::Current(pos),
                    _ => return Ok(Some(EBADPARM)),
                };
                match self.files.get_mut(&this).map(|f| f.file.seek(to)) {
                    Some(Ok(_)) => SUCCESS,
                    _ => EFAILED,
                }
            }
            "Truncate" => {
                let pos = u64::from(self.arg(1)?);
                match self.files.get_mut(&this).map(|f| f.file.set_len(pos)) {
                    Some(Ok(())) => SUCCESS,
                    _ => EFAILED,
                }
            }
            "GetInfo" => {
                let out = self.arg(1)?;
                let (size, file_name) = match self.files.get(&this) {
                    Some(f) => (
                        f.file.metadata().map_or(0, |m| m.len() as u32),
                        f.name.clone(),
                    ),
                    None => (0, String::new()),
                };
                self.write_file_info(out, &file_name, size, false)?;
                SUCCESS
            }
            _ => return Ok(None),
        }))
    }
}
