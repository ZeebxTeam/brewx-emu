//! Runs a BREW applet: boot (`AEEMod_Load` → `IModule_CreateInstance` → `EVT_APP_START`), API
//! dispatch, timers and key events.
//!
//! Frontends drive it with [`Machine::start`], then [`Machine::advance`] and [`Machine::key`],
//! and read [`Machine::screen`]. Everything the game asks for is recorded in a [`Report`]: which
//! interfaces and ClassIDs a title uses is a runtime version signal, and the unimplemented calls
//! are the backlog.

mod bitmap;
mod display;
mod files;
mod helpers;
mod resources;
mod shell;
mod stream;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;

use zeebx_core::brew::aee::{self, Interface};
use zeebx_core::brew::heap::Heap;
use zeebx_core::brew::vfs::Vfs;
use zeebx_core::formats::modfile::ModImage;
use zeebx_core::formats::resfile::ResFile;
use zeebx_core::layout::{
    HEAP_BASE, OBJECT_BASE, OBJECT_SIZE, RETURN_MAGIC, STACK_BASE, STACK_SIZE, vtable_addr,
};

use crate::cpu::dynarmic::DynarmicCpu;
use crate::cpu::{CpuBackend, CpuError, Reg, StopReason};
use crate::display::Resolution;
use crate::loader;
use crate::version::BrewVersion;
use crate::video::{Framebuffer, Image, Rect, Rgb};

pub const SUCCESS: u32 = 0;
pub const EFAILED: u32 = 1;
pub const ENOMEMORY: u32 = 2;
pub const ECLASSNOTSUPPORT: u32 = 3;
pub const EVERSIONNOTSUPPORT: u32 = 4;
pub const EBADPARM: u32 = 14;
pub const EFILENOEXISTS: u32 = 0x201;

const EVT_APP_START: u32 = 0x0000;
const EVT_KEY: u32 = 0x0100;
const EVT_KEY_PRESS: u32 = 0x0101;
const EVT_KEY_RELEASE: u32 = 0x0102;

/// BREW virtual key codes (`AEEVCodes.h`).
pub mod avk {
    pub const ZERO: u32 = 0xe021;
    pub const STAR: u32 = 0xe02b;
    pub const POUND: u32 = 0xe02c;
    pub const END: u32 = 0xe02e;
    pub const SEND: u32 = 0xe02f;
    pub const CLR: u32 = 0xe030;
    pub const UP: u32 = 0xe031;
    pub const DOWN: u32 = 0xe032;
    pub const LEFT: u32 = 0xe033;
    pub const RIGHT: u32 = 0xe034;
    pub const SELECT: u32 = 0xe035;
    pub const SOFT1: u32 = 0xe036;
    pub const SOFT2: u32 = 0xe037;

    /// The code for a key name used by frontends and scripts: `up`, `select`, `5`, `star`…
    pub fn by_name(name: &str) -> Option<u32> {
        Some(match name {
            "up" => UP,
            "down" => DOWN,
            "left" => LEFT,
            "right" => RIGHT,
            "select" | "ok" => SELECT,
            "clr" | "back" => CLR,
            "soft1" => SOFT1,
            "soft2" => SOFT2,
            "send" => SEND,
            "end" => END,
            "star" | "*" => STAR,
            "pound" | "#" => POUND,
            d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => {
                ZERO + u32::from(d.as_bytes()[0] - b'0')
            }
            _ => return None,
        })
    }
}

/// Instructions per virtual millisecond: ~100 MIPS, a generous ARM9/ARM11 handset. The game's
/// clock comes from the instruction count, so two runs give the same result.
const INSTRUCTIONS_PER_MS: u64 = 100_000;
/// Budget for one guest call from the host before it counts as stuck in a loop.
const CALL_BUDGET: u64 = 2_000_000_000;
/// API calls per guest call: a game polling an unimplemented method forever stops here.
const CALL_API_LIMIT: u64 = 2_000_000;
/// Timer callbacks per `advance`: a game that re-arms with 0 ms must not freeze the frontend.
const TIMERS_PER_ADVANCE: usize = 64;
const RECENT_CALLS: usize = 16;
const LOG_LINES: usize = 200;

#[derive(Debug, Clone)]
pub struct MachineConfig {
    pub version: BrewVersion,
    pub resolution: Resolution,
    pub heap_size: usize,
    /// The module's folder: relative paths (`game.bar`) resolve here.
    pub app_dir: PathBuf,
    /// Where the game's writes go; the package itself is never modified.
    pub data_dir: PathBuf,
}

/// How far the boot got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Loaded,
    ModuleCreated,
    AppletCreated,
    Started,
    /// At least one timer or callback fired after `EVT_APP_START`.
    Running,
}

#[derive(Debug, Default, Clone)]
pub struct ClassRequest {
    pub count: u64,
    pub served: Option<Interface>,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub stage: Stage,
    /// The `AEESTDLIB_VER` written before the module base that `AEEMod_Load` accepted.
    pub helper_version: u32,
    /// Why execution stopped; `None` while it can go on.
    pub stop: Option<String>,
    /// Timer callbacks fired.
    pub frames: u32,
    /// `IDisplay_Update` / `IGraphics_Update` calls.
    pub updates: u64,
    pub instructions: u64,
    pub calls: BTreeMap<String, u64>,
    /// The subset of `calls` answered with a default (`0`) instead of an implementation.
    pub unimplemented: BTreeMap<String, u64>,
    pub classes: BTreeMap<u32, ClassRequest>,
    /// Files and resources the game asked for and did not get.
    pub missing: Vec<String>,
    /// The last API calls, oldest first.
    pub recent: Vec<String>,
    /// `DBGPRINTF` and semihosting output.
    pub log: Vec<String>,
}

#[derive(Debug)]
pub enum RunError {
    Load(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RunError {}

/// Why a guest call did not return normally.
#[derive(Debug)]
enum Halt {
    Cpu(CpuError),
    Fault(String),
    /// `AEEMod_Load` returned `EVERSIONNOTSUPPORT`: the helper table version is wrong.
    VersionRefused,
}

impl From<CpuError> for Halt {
    fn from(e: CpuError) -> Self {
        Self::Cpu(e)
    }
}

impl fmt::Display for Halt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cpu(e) => write!(f, "cpu: {e}"),
            Self::Fault(e) => f.write_str(e),
            Self::VersionRefused => f.write_str("AEEMod_Load: EVERSIONNOTSUPPORT"),
        }
    }
}

/// A pending `ISHELL_SetTimer` or `ISHELL_Resume`.
#[derive(Debug, Clone, Copy)]
struct Timer {
    deadline_ms: u64,
    func: u32,
    user: u32,
    /// Arguments after `user`, for callbacks that take more than one (`PFNIMAGEINFO`).
    extra: [u32; 3],
    extra_len: usize,
    /// Insertion order, so equal deadlines fire first-in, first-out.
    seq: u64,
}

/// `IDisplay` color items (`AEEClrItem`) the drawing calls fall back to.
const CLR_USER_TEXT: usize = 1;
const CLR_USER_BACKGROUND: usize = 2;
const CLR_USER_LINE: usize = 3;

/// `IGraphics` state, separate from `IDisplay`'s.
#[derive(Debug, Clone, Copy)]
struct Graphics {
    stroke: Rgb,
    fill: Rgb,
    background: Rgb,
    fill_mode: bool,
    origin: (i32, i32),
    clip: Option<Rect>,
}

/// An `IImage`: the decoded pixels plus the drawing parameters the game set.
#[derive(Debug, Clone)]
struct ImageObject {
    image: Rc<Image>,
    /// `IPARM_CXFRAME`: width of one animation frame; 0 means the whole image.
    frame_width: u32,
    /// `IPARM_ROP`: `AEE_RO_TRANSPARENT` (or none) honors transparency, the rest copies.
    transparent: bool,
    /// `IIMAGE_SetDrawSize` / `SetOffset`.
    draw_size: Option<(i32, i32)>,
    offset: (i32, i32),
}

pub struct Machine {
    cpu: DynarmicCpu,
    config: MachineConfig,
    heap: Heap,
    next_object: u32,
    shell: u32,
    applet: u32,
    handle_event: u32,
    timers: Vec<Timer>,
    timer_seq: u64,
    /// Virtual milliseconds added on top of the instruction clock when time jumps ahead.
    clock_offset_ms: u64,
    screen: Framebuffer,
    colors: [Rgb; 8],
    clip: Option<Rect>,
    graphics: Graphics,
    images: HashMap<u32, ImageObject>,
    /// The screen as an `IBitmap` (0 until a game asks for it).
    device_bitmap: u32,
    /// `IDisplay` / `IGraphics` destinations; 0 is the screen.
    display_target: u32,
    gfx_target: u32,
    /// `IDIB`s in guest memory, by struct address (see `bitmap.rs`).
    dibs: HashSet<u32>,
    /// Host copies of DIBs, loaded around each call that touches them.
    surfaces: HashMap<u32, Framebuffer>,
    /// `IMemAStream`s: guest buffer, size and read position.
    streams: HashMap<u32, stream::MemStream>,
    /// Native bitmaps from `CONVERTBMP` (`SetupNativeImage`), the source of `IDISPLAY_BitBlt`.
    bitmaps: HashMap<u32, Rc<Image>>,
    resources: HashMap<PathBuf, Option<ResFile>>,
    vfs: Vfs,
    files: HashMap<u32, files::OpenFile>,
    report: Report,
    recent: VecDeque<String>,
    /// `BREWX_TRACE=<substring>`: API calls whose name contains it go to stderr with their
    /// first arguments. For debugging a game without rebuilding.
    trace: Option<String>,
}

impl Machine {
    /// Loads and boots the applet `clsid` up to `EVT_APP_START`. A boot that stops early still
    /// returns the machine, with the reason in [`Machine::halted`].
    pub fn start(image: &ModImage, clsid: u32, config: MachineConfig) -> Result<Self, RunError> {
        // `AEEMod_Load` refuses a helper table version other than the SDK's it was built with,
        // before touching anything else; a refusal means trying the next known value.
        let candidates = loader::helper_version_candidates(image);
        let mut refused = None;
        for &helper_version in &candidates {
            let loaded = loader::load(image, config.heap_size, helper_version)
                .map_err(|e| RunError::Load(e.to_string()))?;
            let mut machine = Self::new(&loaded, config.clone())?;
            machine.report.helper_version = helper_version;
            match machine.boot(&loaded, clsid) {
                Err(Halt::VersionRefused) => refused = Some(machine),
                Err(stop) => {
                    machine.report.stop = Some(stop.to_string());
                    return Ok(machine);
                }
                Ok(()) => return Ok(machine),
            }
        }
        let mut machine = refused.ok_or_else(|| RunError::Load("no helper version".into()))?;
        let tried: Vec<String> = candidates.iter().map(|v| format!("{v:#x}")).collect();
        machine.report.stop = Some(format!(
            "AEEMod_Load recusou todas as versões de helper ({})",
            tried.join(", ")
        ));
        Ok(machine)
    }

    fn new(loaded: &loader::LoadedModule, config: MachineConfig) -> Result<Self, RunError> {
        let mut cpu = DynarmicCpu::new();
        cpu.reset(&loaded.mem)
            .map_err(|e| RunError::Load(e.to_string()))?;
        cpu.write_reg(Reg::Sp, STACK_BASE + STACK_SIZE as u32 - 16);
        let mut vfs = Vfs::new(&config.app_dir);
        vfs.set_save_root(config.data_dir.join("mod"));
        vfs.set_device_root(config.data_dir.join("device"));
        let r = config.resolution;
        let mut colors = [Rgb::BLACK; 8];
        colors[CLR_USER_BACKGROUND] = Rgb::WHITE;
        Ok(Self {
            cpu,
            heap: Heap::new(HEAP_BASE, config.heap_size),
            next_object: loaded.objects_start,
            shell: loaded.shell,
            applet: 0,
            handle_event: 0,
            timers: Vec::new(),
            timer_seq: 0,
            clock_offset_ms: 0,
            screen: Framebuffer::new(u32::from(r.width), u32::from(r.height)),
            colors,
            clip: None,
            graphics: Graphics {
                stroke: Rgb::BLACK,
                fill: Rgb::BLACK,
                background: Rgb::WHITE,
                fill_mode: false,
                origin: (0, 0),
                clip: None,
            },
            images: HashMap::new(),
            bitmaps: HashMap::new(),
            streams: HashMap::new(),
            device_bitmap: 0,
            display_target: 0,
            gfx_target: 0,
            dibs: HashSet::new(),
            surfaces: HashMap::new(),
            resources: HashMap::new(),
            vfs,
            files: HashMap::new(),
            config,
            report: Report {
                stage: Stage::Loaded,
                helper_version: 0,
                stop: None,
                frames: 0,
                updates: 0,
                instructions: 0,
                calls: BTreeMap::new(),
                unimplemented: BTreeMap::new(),
                classes: BTreeMap::new(),
                missing: Vec::new(),
                recent: Vec::new(),
                log: Vec::new(),
            },
            recent: VecDeque::new(),
            trace: std::env::var("BREWX_TRACE").ok().filter(|t| !t.is_empty()),
        })
    }

    fn boot(&mut self, loaded: &loader::LoadedModule, clsid: u32) -> Result<(), Halt> {
        let code = self.call(
            loaded.entry,
            &[self.shell, loaded.helpers, loaded.out_module],
        )?;
        let module = self.cpu.read_u32(loaded.out_module)?;
        if code == EVERSIONNOTSUPPORT && module == 0 {
            return Err(Halt::VersionRefused);
        }
        if code != SUCCESS || module == 0 {
            return Err(Halt::Fault(format!(
                "AEEMod_Load devolveu {code}, IModule* = {module:#x}"
            )));
        }
        self.report.stage = Stage::ModuleCreated;

        // IModule::CreateInstance is slot 2 of the game's own IModule vtable (AEEModGen).
        let create = self.method(module, 2)?;
        let code = self.call(create, &[module, self.shell, clsid, loaded.out_applet])?;
        let applet = self.cpu.read_u32(loaded.out_applet)?;
        if code != SUCCESS || applet == 0 {
            return Err(Halt::Fault(format!(
                "IModule_CreateInstance({clsid:#010x}) devolveu {code}, IApplet* = {applet:#x}"
            )));
        }
        self.applet = applet;
        self.handle_event = self.method(applet, 2)?;
        self.report.stage = Stage::AppletCreated;

        let handled = self.call(self.handle_event, &[applet, EVT_APP_START, 0, 0])?;
        if handled == 0 {
            return Err(Halt::Fault("o applet recusou EVT_APP_START".into()));
        }
        self.report.stage = Stage::Started;
        Ok(())
    }

    /// Why the machine stopped, if it did. A halted machine ignores further input.
    pub fn halted(&self) -> Option<&str> {
        self.report.stop.as_deref()
    }

    pub fn screen(&self) -> &Framebuffer {
        &self.screen
    }

    /// Timer callbacks fired so far.
    pub fn frames(&self) -> u32 {
        self.report.frames
    }

    /// How many times the game pushed a frame (`IDisplay_Update`); a frontend redraws when it
    /// changes.
    pub fn updates(&self) -> u64 {
        self.report.updates
    }

    pub fn report(&self) -> Report {
        let mut report = self.report.clone();
        report.instructions = self.cpu.instructions();
        report.recent = self.recent.iter().cloned().collect();
        let semihosting = self.cpu.semihosting();
        report
            .log
            .extend(semihosting.lines().map(|l| format!("[semihosting] {l}")));
        report.log.truncate(LOG_LINES);
        report
    }

    /// Advances the virtual clock by `ms`, firing every timer that falls due on the way.
    pub fn advance(&mut self, ms: u64) {
        let target = self.now_ms() + ms;
        for _ in 0..TIMERS_PER_ADVANCE {
            if self.halted().is_some() {
                return;
            }
            let due = self.timers.iter().any(|t| t.deadline_ms <= target);
            if !due || !self.fire_next_timer() {
                break;
            }
        }
        let now = self.now_ms();
        if target > now {
            self.clock_offset_ms += target - now;
        }
    }

    /// Fires the earliest timer, jumping the clock to its deadline. `false` when there is none
    /// or the machine is halted.
    pub fn fire_next_timer(&mut self) -> bool {
        if self.halted().is_some() {
            return false;
        }
        let Some(timer) = self.next_timer() else {
            if self.report.stage >= Stage::Started && self.report.frames == 0 {
                // Not an error by itself: some games wait for a key before arming a timer.
                self.log("[brewx] nenhum timer pendente".into());
            }
            return false;
        };
        let mut args = vec![timer.user];
        args.extend_from_slice(&timer.extra[..timer.extra_len]);
        let result = self.call(timer.func, &args);
        self.report.frames += 1;
        self.report.stage = Stage::Running;
        self.settle(result)
    }

    /// Delivers a key press or release as BREW does: `EVT_KEY_PRESS` then `EVT_KEY` on press,
    /// `EVT_KEY_RELEASE` on release, with the `AVK_` code in `wParam`.
    pub fn key(&mut self, code: u32, pressed: bool) {
        if self.halted().is_some() || self.handle_event == 0 {
            return;
        }
        let events: &[u32] = if pressed {
            &[EVT_KEY_PRESS, EVT_KEY]
        } else {
            &[EVT_KEY_RELEASE]
        };
        for &evt in events {
            let result = self.call(self.handle_event, &[self.applet, evt, code, 0]);
            if !self.settle(result) {
                return;
            }
        }
    }

    fn settle(&mut self, result: Result<u32, Halt>) -> bool {
        match result {
            Ok(_) => true,
            Err(halt) => {
                self.report.stop = Some(halt.to_string());
                false
            }
        }
    }

    fn method(&self, object: u32, slot: u32) -> Result<u32, CpuError> {
        let vtable = self.cpu.read_u32(object)?;
        self.cpu.read_u32(vtable + slot * 4)
    }

    pub(crate) fn now_ms(&self) -> u64 {
        self.cpu.instructions() / INSTRUCTIONS_PER_MS + self.clock_offset_ms
    }

    fn next_timer(&mut self) -> Option<Timer> {
        let index = (0..self.timers.len())
            .min_by_key(|&i| (self.timers[i].deadline_ms, self.timers[i].seq))?;
        let timer = self.timers.swap_remove(index);
        let now = self.now_ms();
        if timer.deadline_ms > now {
            self.clock_offset_ms += timer.deadline_ms - now;
        }
        Some(timer)
    }

    fn schedule(&mut self, delay_ms: u64, func: u32, user: u32) {
        self.schedule_with(delay_ms, func, user, &[]);
    }

    /// A deferred guest callback `func(user, extra...)`: how the host calls back into the game
    /// without nesting inside an API call.
    fn schedule_with(&mut self, delay_ms: u64, func: u32, user: u32, extra: &[u32]) {
        self.timer_seq += 1;
        let mut args = [0; 3];
        let extra_len = extra.len().min(3);
        args[..extra_len].copy_from_slice(&extra[..extra_len]);
        self.timers.push(Timer {
            deadline_ms: self.now_ms() + delay_ms,
            func,
            user,
            extra: args,
            extra_len,
            seq: self.timer_seq,
        });
    }

    /// Calls guest code at `func` with `args` (AAPCS) and serves API calls until it returns.
    fn call(&mut self, func: u32, args: &[u32]) -> Result<u32, Halt> {
        let saved_sp = self.cpu.read_reg(Reg::Sp);
        let stacked = args.len().saturating_sub(4) as u32;
        // AAPCS: 8-byte aligned stack at the call.
        let sp = (saved_sp - stacked * 4) & !7;
        for (i, &arg) in args.iter().enumerate() {
            match Reg::ARGS.get(i) {
                Some(&reg) => self.cpu.write_reg(reg, arg),
                None => self.cpu.write_u32(sp + (i as u32 - 4) * 4, arg)?,
            }
        }
        self.cpu.write_reg(Reg::Sp, sp);
        self.cpu.write_reg(Reg::Lr, RETURN_MAGIC);

        let start = self.cpu.instructions();
        let mut pc = func;
        let mut api_calls = 0u64;
        let result = loop {
            let spent = self.cpu.instructions() - start;
            if spent >= CALL_BUDGET {
                break Err(Halt::Fault(format!(
                    "{spent} instruções sem retornar: laço infinito?"
                )));
            }
            match self.cpu.run(pc, CALL_BUDGET - spent)? {
                StopReason::Returned => break Ok(self.cpu.read_reg(Reg::R0)),
                StopReason::ApiCall { addr } => {
                    api_calls += 1;
                    if api_calls > CALL_API_LIMIT {
                        break Err(Halt::Fault(format!(
                            "{api_calls} chamadas de API sem retornar"
                        )));
                    }
                    let value = self.dispatch(addr)?;
                    self.cpu.write_reg(Reg::R0, value);
                    pc = self.cpu.read_reg(Reg::Lr);
                }
                StopReason::MemoryFault { addr, pc } => {
                    // With pc at the faulting address (a jump through a null pointer), lr is
                    // the only trace of who jumped.
                    let lr = self.cpu.read_reg(Reg::Lr);
                    break Err(Halt::Fault(format!(
                        "acesso inválido a {addr:#010x} (pc {pc:#010x}, lr {lr:#010x})"
                    )));
                }
                StopReason::Exception { pc } => {
                    break Err(Halt::Fault(format!("exceção do núcleo ARM em {pc:#010x}")));
                }
                StopReason::Budget => continue,
            }
        };
        self.cpu.write_reg(Reg::Sp, saved_sp);
        result
    }

    fn dispatch(&mut self, addr: u32) -> Result<u32, Halt> {
        let Some((iface, slot)) = aee::decode(addr) else {
            return Err(Halt::Fault(format!(
                "endereço de API desconhecido {addr:#010x}"
            )));
        };
        let name = aee::describe(addr);
        *self.report.calls.entry(name.clone()).or_default() += 1;
        if self.recent.len() == RECENT_CALLS {
            self.recent.pop_front();
        }
        self.recent.push_back(name.clone());
        if let Some(filter) = &self.trace
            && name.contains(filter.as_str())
        {
            let args: Vec<String> = (0..9)
                .map(|i| self.arg(i).map_or("?".into(), |v| format!("{v:#x}")))
                .collect();
            eprintln!("[{}] {name}({})", self.report.frames, args.join(", "));
        }

        let method = iface.method(slot).unwrap_or("");
        let draws = matches!(
            iface,
            Interface::Display | Interface::Graphics | Interface::Image | Interface::Bitmap
        );
        // DIBs live in guest memory: bring the ones this call may touch to the host, and
        // write them back after.
        let dibs = if draws {
            self.involved_dibs()
        } else {
            Vec::new()
        };
        for &id in &dibs {
            self.dib_load(id)?;
        }
        let served = match iface {
            Interface::Helpers => self.helper(slot)?,
            Interface::Shell => self.shell_method(method)?,
            Interface::Display => match self.display_destination(method)? {
                Some(v) => Some(v),
                None => self.display_method(method)?,
            },
            Interface::Graphics => match self.graphics_destination(method)? {
                Some(v) => Some(v),
                None => self.graphics_method(method)?,
            },
            Interface::Bitmap => self.bitmap_method(method)?,
            Interface::Image => self.image_method(method)?,
            Interface::FileMgr => self.filemgr_method(method)?,
            Interface::File => self.file_method(method)?,
            Interface::License => self.license_method(method)?,
            Interface::MemAStream => self.memstream_method(method)?,
            _ => None,
        };
        for &id in &dibs {
            self.dib_store(id)?;
        }
        // Every BREW interface inherits IBase: AddRef and Release answer the same everywhere.
        let served = served.or(match slot {
            0 => Some(1),
            1 => Some(0),
            _ => None,
        });
        Ok(served.unwrap_or_else(|| {
            *self.report.unimplemented.entry(name).or_default() += 1;
            SUCCESS
        }))
    }

    /// `ILicense`: every title is treated as purchased, with no expiry.
    fn license_method(&mut self, method: &str) -> Result<Option<u32>, Halt> {
        const LT_NONE: u32 = 0;
        const PT_PURCHASE: u32 = 2;
        const UNLIMITED: u32 = 0xffff_ffff;
        Ok(Some(match method {
            "IsExpired" => 0,
            "GetInfo" => {
                self.write_out(self.arg(1)?, UNLIMITED)?;
                LT_NONE
            }
            "SetUsesRemaining" => EFAILED,
            // AEEPriceType GetPurchaseInfo(ILicense *, AEELicenseType *, uint32 *, uint32 *)
            "GetPurchaseInfo" => {
                let lt = self.arg(1)?;
                if lt != 0 {
                    self.cpu.write_mem(lt, &[LT_NONE as u8])?;
                }
                self.write_out(self.arg(2)?, UNLIMITED)?;
                self.write_out(self.arg(3)?, 0)?;
                PT_PURCHASE
            }
            _ => return Ok(None),
        }))
    }

    /// The n-th argument of the current API call: `r0..r3`, then the caller's stack.
    pub(crate) fn arg(&self, n: usize) -> Result<u32, CpuError> {
        match Reg::ARGS.get(n) {
            Some(&reg) => Ok(self.cpu.read_reg(reg)),
            None => self
                .cpu
                .read_u32(self.cpu.read_reg(Reg::Sp) + (n as u32 - 4) * 4),
        }
    }

    /// Writes `value` through an optional out pointer.
    fn write_out(&mut self, ptr: u32, value: u32) -> Result<(), CpuError> {
        if ptr != 0 {
            self.cpu.write_u32(ptr, value)?;
        }
        Ok(())
    }

    /// A new object of `iface`: the vtable pointer, in the object region.
    fn new_object(&mut self, iface: Interface) -> Result<u32, Halt> {
        let addr = self.next_object;
        if addr + 16 > OBJECT_BASE + OBJECT_SIZE as u32 {
            return Err(Halt::Fault("região de objetos cheia".into()));
        }
        self.next_object += 16;
        self.cpu.write_u32(addr, vtable_addr(iface))?;
        Ok(addr)
    }

    fn log(&mut self, line: String) {
        if self.report.log.len() < LOG_LINES {
            self.report.log.push(line);
        }
    }

    fn missing(&mut self, what: String) {
        if self.report.missing.len() < LOG_LINES && !self.report.missing.contains(&what) {
            self.report.missing.push(what);
        }
    }
}
