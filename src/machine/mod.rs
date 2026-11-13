//! Runs a BREW applet: boot (`AEEMod_Load` → `IModule_CreateInstance` → `EVT_APP_START`), API
//! dispatch and the timer loop that drives frames.
//!
//! Everything the game asks for is recorded in a [`Report`]. At this stage that record is the
//! point: which interfaces and ClassIDs a title uses is the runtime signal that version
//! detection lacks, and the unimplemented calls are the backlog.

mod helpers;
mod shell;

use std::collections::{BTreeMap, VecDeque};
use std::fmt;

use zeebx_core::brew::aee::{self, Interface};
use zeebx_core::brew::heap::Heap;
use zeebx_core::formats::modfile::ModImage;
use zeebx_core::layout::{
    HEAP_BASE, OBJECT_BASE, OBJECT_SIZE, RETURN_MAGIC, STACK_BASE, STACK_SIZE, vtable_addr,
};

use crate::cpu::dynarmic::DynarmicCpu;
use crate::cpu::{CpuBackend, CpuError, Reg, StopReason};
use crate::display::Resolution;
use crate::loader;
use crate::version::BrewVersion;

pub const SUCCESS: u32 = 0;
pub const EFAILED: u32 = 1;
pub const ENOMEMORY: u32 = 2;
pub const ECLASSNOTSUPPORT: u32 = 3;
pub const EVERSIONNOTSUPPORT: u32 = 4;
pub const EBADPARM: u32 = 14;

const EVT_APP_START: u32 = 0x0000;

/// Instructions per virtual millisecond: ~100 MIPS, a generous ARM9/ARM11 handset. The game's
/// clock comes from the instruction count, so two runs give the same result.
const INSTRUCTIONS_PER_MS: u64 = 100_000;
/// Budget for one guest call from the host (boot step or timer callback) before it counts as
/// stuck in a loop.
const CALL_BUDGET: u64 = 2_000_000_000;
/// API calls per guest call: a game polling an unimplemented method forever stops here.
const CALL_API_LIMIT: u64 = 2_000_000;
const RECENT_CALLS: usize = 16;
const LOG_LINES: usize = 200;

#[derive(Debug, Clone, Copy)]
pub struct MachineConfig {
    pub version: BrewVersion,
    pub resolution: Resolution,
    pub heap_size: usize,
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
    /// Why execution stopped; `None` when the frame budget ran out normally.
    pub stop: Option<String>,
    pub frames: u32,
    pub instructions: u64,
    /// Every API method called, by name, with its count.
    pub calls: BTreeMap<String, u64>,
    /// The subset of `calls` answered with a default (`0`) instead of an implementation.
    pub unimplemented: BTreeMap<String, u64>,
    /// ClassIDs asked of `ISHELL_CreateInstance`.
    pub classes: BTreeMap<u32, ClassRequest>,
    /// The last API calls before the stop, oldest first.
    pub recent: Vec<String>,
    /// `DBGPRINTF` and semihosting output.
    pub log: Vec<String>,
}

#[derive(Debug)]
pub enum RunError {
    Io(std::io::Error),
    Load(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
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
    /// Insertion order, so equal deadlines fire first-in, first-out.
    seq: u64,
}

pub struct Machine {
    cpu: DynarmicCpu,
    config: MachineConfig,
    heap: Heap,
    next_object: u32,
    shell: u32,
    applet: u32,
    timers: Vec<Timer>,
    timer_seq: u64,
    /// Virtual milliseconds added on top of the instruction clock when the loop jumps ahead to
    /// the next timer.
    clock_offset_ms: u64,
    report: Report,
    recent: VecDeque<String>,
}

impl Machine {
    /// Boots the applet `clsid` and fires up to `frames` timers.
    pub fn run(
        image: &ModImage,
        clsid: u32,
        config: MachineConfig,
        frames: u32,
    ) -> Result<Report, RunError> {
        // `AEEMod_Load` refuses a helper table version other than the SDK's it was built with,
        // before touching anything else; a refusal means trying the next known value.
        let candidates = loader::helper_version_candidates(image);
        let mut last = None;
        for &helper_version in &candidates {
            let loaded = loader::load(image, config.heap_size, helper_version)
                .map_err(|e| RunError::Load(e.to_string()))?;
            let mut machine = Self::new(&loaded, config)?;
            machine.report.helper_version = helper_version;
            match machine.boot(&loaded, clsid, frames) {
                Err(Halt::VersionRefused) => last = Some(machine),
                Err(stop) => {
                    machine.report.stop = Some(stop.to_string());
                    return Ok(machine.finish());
                }
                Ok(()) => return Ok(machine.finish()),
            }
        }
        let mut machine = last.expect("at least one helper version candidate");
        machine.report.stop = Some(format!(
            "AEEMod_Load recusou todas as versões de helper ({})",
            candidates
                .iter()
                .map(|v| format!("{v:#x}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        Ok(machine.finish())
    }

    fn new(loaded: &loader::LoadedModule, config: MachineConfig) -> Result<Self, RunError> {
        let mut cpu = DynarmicCpu::new();
        cpu.reset(&loaded.mem)
            .map_err(|e| RunError::Load(e.to_string()))?;
        cpu.write_reg(Reg::Sp, STACK_BASE + STACK_SIZE as u32 - 16);
        let machine = Self {
            cpu,
            config,
            heap: Heap::new(HEAP_BASE, config.heap_size),
            next_object: loaded.objects_start,
            shell: loaded.shell,
            applet: 0,
            timers: Vec::new(),
            timer_seq: 0,
            clock_offset_ms: 0,
            report: Report {
                stage: Stage::Loaded,
                helper_version: 0,
                stop: None,
                frames: 0,
                instructions: 0,
                calls: BTreeMap::new(),
                unimplemented: BTreeMap::new(),
                classes: BTreeMap::new(),
                recent: Vec::new(),
                log: Vec::new(),
            },
            recent: VecDeque::new(),
        };
        Ok(machine)
    }

    fn boot(&mut self, loaded: &loader::LoadedModule, clsid: u32, frames: u32) -> Result<(), Halt> {
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
        self.report.stage = Stage::AppletCreated;

        let handle_event = self.method(applet, 2)?;
        let handled = self.call(handle_event, &[applet, EVT_APP_START, 0, 0])?;
        if handled == 0 {
            return Err(Halt::Fault("o applet recusou EVT_APP_START".into()));
        }
        self.report.stage = Stage::Started;

        for _ in 0..frames {
            let Some(timer) = self.next_timer() else {
                return Err(Halt::Fault(
                    "nenhum timer pendente: o jogo espera um evento".into(),
                ));
            };
            self.call(timer.func, &[timer.user])?;
            self.report.frames += 1;
            self.report.stage = Stage::Running;
        }
        Ok(())
    }

    fn finish(mut self) -> Report {
        self.report.instructions = self.cpu.instructions();
        self.report.recent = self.recent.into_iter().collect();
        let semihosting = self.cpu.semihosting();
        self.report
            .log
            .extend(semihosting.lines().map(|l| format!("[semihosting] {l}")));
        self.report.log.truncate(LOG_LINES);
        self.report
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
        self.timer_seq += 1;
        self.timers.push(Timer {
            deadline_ms: self.now_ms() + delay_ms,
            func,
            user,
            seq: self.timer_seq,
        });
    }

    /// Calls guest code at `func` with `args` (AAPCS) and serves API calls until it returns.
    fn call(&mut self, func: u32, args: &[u32]) -> Result<u32, Halt> {
        let saved_sp = self.cpu.read_reg(Reg::Sp);
        let mut sp = saved_sp;
        let stacked = args.len().saturating_sub(4) as u32;
        // AAPCS: 8-byte aligned stack at the call.
        sp = (sp - stacked * 4) & !7;
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
                    break Err(Halt::Fault(format!(
                        "acesso inválido a {addr:#010x} (pc {pc:#010x})"
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

        let served = match iface {
            Interface::Helpers => self.helper(slot)?,
            Interface::Shell => self.shell_method(slot)?,
            _ => None,
        };
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

    /// The n-th argument of the current API call: `r0..r3`, then the caller's stack.
    pub(crate) fn arg(&self, n: usize) -> Result<u32, CpuError> {
        match Reg::ARGS.get(n) {
            Some(&reg) => Ok(self.cpu.read_reg(reg)),
            None => self
                .cpu
                .read_u32(self.cpu.read_reg(Reg::Sp) + (n as u32 - 4) * 4),
        }
    }

    /// A new object of `iface`: just the vtable pointer for now, in the object region.
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
}
