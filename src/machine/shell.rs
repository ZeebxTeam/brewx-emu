//! `IShell`: the object factory, the timers and the device description.

use zeebx_core::brew::aee::Interface;

use super::{ECLASSNOTSUPPORT, EFAILED, Halt, Machine, SUCCESS};
use crate::cpu::CpuBackend;

/// ClassIDs from the BREW SDK headers (`AEEClassIDs.h`): `AEECLSID_CORE` is `0x01001000`, and
/// most core classes are small offsets from it.
const AEECLSID_DISPLAY: u32 = 0x0100_1001;
const AEECLSID_HEAP: u32 = 0x0100_1002;
const AEECLSID_FILEMGR: u32 = 0x0100_1003;
const AEECLSID_MEMASTREAM: u32 = 0x0100_100c;
const AEECLSID_LICENSE: u32 = 0x0100_100f;
const AEECLSID_SOURCEUTIL: u32 = 0x0100_1011;
const AEECLSID_UNZIPSTREAM: u32 = 0x0100_1014;
const AEECLSID_THREAD: u32 = 0x0100_1017;
const AEECLSID_SOUND: u32 = 0x0100_1056;
const AEECLSID_GRAPHICS: u32 = 0x0100_2001;
const AEECLSID_WINBMP: u32 = 0x0100_4001;
const AEECLSID_GIF: u32 = 0x0100_4003;
const AEECLSID_PNG: u32 = 0x0100_4004;
const AEECLSID_JPEG: u32 = 0x0100_4005;
const AEECLSID_WEB: u32 = 0x0100_5000;
const AEECLSID_MEDIAUTIL: u32 = 0x0100_550d;

/// `AEEDeviceInfo` up to `dwLang`; the extended part starts at `wStructSize`.
const DEVICE_INFO_BASE: u32 = 44;
const AEE_ENC_ISOLATIN1: u16 = 0x01;
const COLOR_DEPTH: u16 = 16;

fn interface_for(clsid: u32) -> Option<Interface> {
    Some(match clsid {
        AEECLSID_DISPLAY => Interface::Display,
        AEECLSID_HEAP => Interface::Heap,
        AEECLSID_FILEMGR => Interface::FileMgr,
        AEECLSID_MEMASTREAM => Interface::MemAStream,
        AEECLSID_LICENSE => Interface::License,
        AEECLSID_SOURCEUTIL => Interface::SourceUtil,
        AEECLSID_UNZIPSTREAM => Interface::UnzipStream,
        AEECLSID_THREAD => Interface::Thread,
        AEECLSID_SOUND => Interface::Sound,
        AEECLSID_GRAPHICS => Interface::Graphics,
        AEECLSID_WINBMP | AEECLSID_GIF | AEECLSID_PNG | AEECLSID_JPEG => Interface::Image,
        AEECLSID_WEB => Interface::Web,
        AEECLSID_MEDIAUTIL => Interface::MediaUtil,
        _ => return None,
    })
}

impl Machine {
    pub(super) fn shell_method(&mut self, name: &str) -> Result<Option<u32>, Halt> {
        let value = match name {
            "CreateInstance" => self.create_instance()?,
            "GetDeviceInfo" => self.device_info()?,
            "SetTimer" => {
                let (ms, func, user) = (self.arg(1)?, self.arg(2)?, self.arg(3)?);
                // BREW keeps one timer per (callback, user) pair: setting it again moves it.
                self.timers.retain(|t| !(t.func == func && t.user == user));
                self.schedule(u64::from(ms), func, user);
                SUCCESS
            }
            "CancelTimer" => {
                let (func, user) = (self.arg(1)?, self.arg(2)?);
                // A null callback cancels every timer of that user.
                self.timers
                    .retain(|t| !(t.user == user && (func == 0 || t.func == func)));
                SUCCESS
            }
            // AEECLSID GetHandler(IShell *, AEECLSID clsBase, const char *pszMIME)
            "GetHandler" => {
                let mime = String::from_utf8_lossy(&self.cstr(self.arg(2)?)?).to_ascii_lowercase();
                match mime.as_str() {
                    "image/bmp" | "image/x-bmp" | "image/wbmp" => AEECLSID_WINBMP,
                    "image/png" => AEECLSID_PNG,
                    "image/gif" => AEECLSID_GIF,
                    "image/jpeg" | "image/jpg" => AEECLSID_JPEG,
                    _ => {
                        self.missing(format!("handler para {mime:?}"));
                        0
                    }
                }
            }
            "LoadResData" => self.load_res_data(false)?,
            "LoadResDataEx" => self.load_res_data(true)?,
            "LoadResString" => self.load_res_string()?,
            "LoadResObject" => self.load_res_object()?,
            "FreeResData" => {
                let ptr = self.arg(1)?;
                if ptr != 0 {
                    self.heap.free(ptr);
                }
                SUCCESS
            }
            // No saved preferences: the game falls back to its defaults.
            "GetPrefs" => EFAILED,
            "SetPrefs" => SUCCESS,
            "Resume" => {
                // AEECallback: pNext, pmc, pfnCancel, pCancelData, pfnNotify, pNotifyData.
                let cb = self.arg(1)?;
                let (func, user) = (self.cpu.read_u32(cb + 16)?, self.cpu.read_u32(cb + 20)?);
                self.schedule(0, func, user);
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(value))
    }

    /// `ISHELL_CreateInstance(IShell *po, AEECLSID cls, void **ppobj)`.
    ///
    /// Every requested class goes into the report, served or not: which classes a title asks
    /// for is the best runtime clue to its BREW version. An unknown class gets
    /// `ECLASSNOTSUPPORT`, a legitimate answer games handle as "this handset lacks the feature".
    fn create_instance(&mut self) -> Result<u32, Halt> {
        let (clsid, out) = (self.arg(1)?, self.arg(2)?);
        let iface = interface_for(clsid);
        let entry = self.report.classes.entry(clsid).or_default();
        entry.count += 1;
        entry.served = iface;
        let Some(iface) = iface else {
            if out != 0 {
                self.cpu.write_u32(out, 0)?;
            }
            return Ok(ECLASSNOTSUPPORT);
        };
        let object = self.new_object(iface)?;
        if out != 0 {
            self.cpu.write_u32(out, object)?;
        }
        Ok(SUCCESS)
    }

    /// `ISHELL_GetDeviceInfo(IShell *po, AEEDeviceInfo *pi)`: where the chosen resolution reaches
    /// the game.
    fn device_info(&mut self) -> Result<u32, Halt> {
        let info = self.arg(1)?;
        if info == 0 {
            return Ok(SUCCESS);
        }
        // The caller fills `wStructSize` before the call to ask for the extended fields; BREW
        // 1.x has none, so only the base part is written.
        self.cpu.write_mem(info, &[0; DEVICE_INFO_BASE as usize])?;
        let r = self.config.resolution;
        let fields: [u16; 8] = [
            r.width,
            r.height,
            0, // cxAltScreen: no second screen
            0, // cyAltScreen
            8, // cxScrollBar
            AEE_ENC_ISOLATIN1,
            0, // wMenuTextScroll
            COLOR_DEPTH,
        ];
        let bytes: Vec<u8> = fields.iter().flat_map(|f| f.to_le_bytes()).collect();
        self.cpu.write_mem(info, &bytes)?;
        // dwRAM, "initial size of the BREW heap", after wMenuImageDelay and padding — the
        // offset zeebx-emu verified against real games.
        self.cpu
            .write_u32(info + 24, self.config.heap_size as u32)?;
        Ok(SUCCESS)
    }
}
