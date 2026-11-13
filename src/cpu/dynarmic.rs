//! ARM backend on top of Dynarmic, ported from zeebx-emu without its Zeebo-specific
//! optimizations (write watches on surfaces, JIT timing). The unmapped trampoline addresses are
//! still the stop that hands control back to the Rust dispatcher.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::mem::size_of;
use std::rc::Rc;

use dynarmic::a32::{ArchVersion, Callbacks, Dynarmic as Jit, VAddr};
use dynarmic::{CallbackImpl, GuestInt, HaltReason};

use super::mem::GuestMemory;
use super::{API_BASE, API_SIZE, RETURN_MAGIC};
use super::{CpuBackend, CpuError, Reg, StopReason};

/// ARM user-mode `CPSR`: a BREW applet is never privileged code.
const USER_MODE: u32 = 0x10;
const CPSR_THUMB: u32 = 1 << 5;
/// A string without a terminator must not trap the host in an endless read.
const MAX_SEMIHOSTING_STRING: u32 = 4096;
/// Granularity Dynarmic uses to index recompiled code.
const PAGE: u32 = 4096;
const PAGES: usize = 1 << 20;
/// A 4-byte read in the last bytes of a region crosses its edge in host memory; the slack keeps
/// it inside allocated memory.
const REGION_SLACK: usize = 16;
/// Semihosting `SVC` numbers: Thumb's and ARM's.
const SEMIHOSTING: [u32; 2] = [0xab, 0x12_3456];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Nothing,
    Fetch(u32),
    Memory(u32),
    Exception(u32),
}

/// State shared between the C++ callbacks and the Rust wrapper. The JIT lives in a `Box`, so the
/// pointer kept here stays valid when the `DynarmicCpu` moves.
struct State {
    memory: Rc<RefCell<GuestMemory>>,
    semihosting: Rc<RefCell<String>>,
    /// Pages already fetched as code: only a write to one of them can invalidate a block.
    executed_pages: RefCell<BTreeSet<u32>>,
    /// Invalidations requested during the running block; applied after `run`, never from inside
    /// a callback.
    dirty_code: RefCell<BTreeSet<u32>>,
    instructions: Cell<u64>,
    limit: Cell<u64>,
    stop: Cell<Stop>,
    jit: Cell<*mut Jit<State>>,
    /// Dynarmic's page table; see [`DynarmicCpu::table`].
    table: *mut *mut u8,
}

impl State {
    /// From now on reads and writes to `page` go through the callbacks.
    fn clear_page(&self, page: u32) {
        if (page as usize) < PAGES {
            unsafe { *self.table.add(page as usize) = std::ptr::null_mut() };
        }
    }

    fn halt(cb: &CallbackImpl<Self>, stop: Stop) {
        cb.stop.set(stop);
        let jit = cb.jit.get();
        if !jit.is_null() {
            // The documented way for a callback to hand control back; the concrete reason stays
            // in `stop`, since the public enum only offers generic user bits.
            unsafe { (*jit).halt(HaltReason::UserDefined1) };
        }
    }

    fn read(&self, addr: u32, out: &mut [u8]) -> bool {
        match self.memory.borrow().read(addr, out.len() as u32) {
            Ok(bytes) => {
                out.copy_from_slice(bytes);
                true
            }
            Err(_) => false,
        }
    }

    fn write(&self, addr: u32, bytes: &[u8]) -> bool {
        if self.memory.borrow_mut().write(addr, bytes).is_err() {
            return false;
        }
        self.mark_code_dirty(addr, bytes.len() as u32);
        true
    }

    fn mark_code_dirty(&self, addr: u32, len: u32) {
        let end = addr.saturating_add(len.saturating_sub(1));
        let executed = self.executed_pages.borrow();
        for page in (addr / PAGE)..=(end / PAGE) {
            if executed.contains(&page) {
                self.dirty_code.borrow_mut().insert(page);
            }
        }
    }
}

impl Callbacks for State {
    fn memory_read_code(cb: &CallbackImpl<Self>, addr: VAddr) -> Option<u32> {
        let mut bytes = [0; 4];
        if cb.memory.borrow().executable(addr) && cb.read(addr, &mut bytes) {
            // A page that becomes code leaves the table, so every write to it reaches the
            // callback that invalidates the recompiled block.
            let page = addr / PAGE;
            if cb.executed_pages.borrow_mut().insert(page) {
                cb.clear_page(page);
            }
            Some(u32::from_le_bytes(bytes))
        } else {
            Self::halt(cb, Stop::Fetch(addr));
            None
        }
    }

    extern "C" fn memory_read<T: GuestInt>(cb: &CallbackImpl<Self>, addr: VAddr) -> T {
        let mut bytes = [0u8; 8];
        let len = size_of::<T>();
        if !cb.read(addr, &mut bytes[..len]) {
            Self::halt(cb, Stop::Memory(addr));
        }
        // Dynarmic hosts (x86-64, AArch64) are little-endian, and `T` is u8/u16/u32/u64.
        unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<T>()) }
    }

    extern "C" fn memory_write<T: GuestInt>(cb: &mut CallbackImpl<Self>, addr: VAddr, value: T) {
        let len = size_of::<T>();
        let bytes = unsafe { std::slice::from_raw_parts((&value as *const T).cast::<u8>(), len) };
        if !cb.write(addr, bytes) {
            Self::halt(cb, Stop::Memory(addr));
        }
    }

    extern "C" fn call_svc(cb: &mut CallbackImpl<Self>, swi: u32) {
        // ARM semihosting, used by some games as a debug log. Execution continues at the next
        // instruction; only the two output operations are served.
        if SEMIHOSTING.contains(&swi) {
            let jit = unsafe { &mut *cb.jit.get() };
            let (op, arg) = (jit.get_reg(0), jit.get_reg(1));
            let mut output = cb.semihosting.borrow_mut();
            match op {
                0x03 => {
                    let mut byte = [0u8; 1];
                    if cb.read(arg, &mut byte) {
                        output.push(byte[0] as char);
                    }
                }
                0x04 => {
                    for offset in 0..MAX_SEMIHOSTING_STRING {
                        let mut byte = [0u8; 1];
                        if !cb.read(arg.saturating_add(offset), &mut byte) || byte[0] == 0 {
                            break;
                        }
                        output.push(byte[0] as char);
                    }
                }
                _ => {}
            }
            super::trim_semihosting(&mut output);
            jit.set_reg(0, 0);
        } else if cb.stop.get() == Stop::Nothing {
            let pc = unsafe { (*cb.jit.get()).get_pc() };
            Self::halt(cb, Stop::Exception(pc));
        }
    }

    extern "C" fn add_ticks(cb: &mut CallbackImpl<Self>, ticks: u64) {
        cb.instructions
            .set(cb.instructions.get().saturating_add(ticks));
    }

    extern "C" fn get_ticks_remaining(cb: &CallbackImpl<Self>) -> u64 {
        cb.limit.get().saturating_sub(cb.instructions.get())
    }

    extern "C" fn raised_exception(
        cb: &mut CallbackImpl<Self>,
        pc: VAddr,
        _exception: ::dynarmic::a32::Exception,
    ) {
        // A failed fetch arrives here as `NoExecuteFault` after the fetch callback already
        // recorded it; not overwriting it keeps API calls distinguishable from bad instructions.
        if cb.stop.get() == Stop::Nothing {
            if pc == RETURN_MAGIC || (API_BASE..API_BASE.saturating_add(API_SIZE)).contains(&pc) {
                Self::halt(cb, Stop::Fetch(pc));
            } else {
                Self::halt(cb, Stop::Exception(pc));
            }
        }
    }
}

pub struct DynarmicCpu {
    memory: Rc<RefCell<GuestMemory>>,
    semihosting: Rc<RefCell<String>>,
    jit: Option<Box<Jit<State>>>,
    /// **Memory access without a callback.** Each entry points to the start of a page of a
    /// writable region, which does not move after `reset`. Read-only regions, executed pages and
    /// a region's partial last page stay null and go through the callbacks.
    table: Box<[*mut u8]>,
}

impl Default for DynarmicCpu {
    fn default() -> Self {
        Self::new()
    }
}

impl DynarmicCpu {
    pub fn new() -> Self {
        Self {
            memory: Default::default(),
            semihosting: Default::default(),
            jit: None,
            table: vec![std::ptr::null_mut(); PAGES].into_boxed_slice(),
        }
    }

    fn jit(&self) -> Result<&Jit<State>, CpuError> {
        self.jit
            .as_deref()
            .ok_or_else(|| CpuError("Dynarmic received no memory map".into()))
    }

    fn jit_mut(&mut self) -> Result<&mut Jit<State>, CpuError> {
        self.jit
            .as_deref_mut()
            .ok_or_else(|| CpuError("Dynarmic received no memory map".into()))
    }

    /// The host also writes guest memory between JIT entries; a write to an executed page must
    /// recompile the next block.
    fn invalidate_written_code(&mut self, addr: u32, len: u32) {
        let Ok(jit) = self.jit_mut() else { return };
        jit.mark_code_dirty(addr, len);
        let pages = std::mem::take(&mut *jit.dirty_code.borrow_mut());
        for page in pages {
            jit.invalidate_cache_range(page * PAGE, PAGE as usize);
        }
    }
}

impl CpuBackend for DynarmicCpu {
    fn reset(&mut self, mem: &GuestMemory) -> Result<(), CpuError> {
        let mut copy = mem.clone();
        for region in copy.regions_mut() {
            region.bytes.reserve_exact(REGION_SLACK);
        }
        for page in 0..PAGES as u32 {
            self.table[page as usize] = page_pointer(&copy, page);
        }
        self.memory = Rc::new(RefCell::new(copy));
        self.semihosting.borrow_mut().clear();
        let state = State {
            memory: self.memory.clone(),
            semihosting: self.semihosting.clone(),
            executed_pages: Default::default(),
            dirty_code: Default::default(),
            instructions: Cell::new(0),
            limit: Cell::new(0),
            stop: Cell::new(Stop::Nothing),
            jit: Cell::new(std::ptr::null_mut()),
            table: self.table.as_mut_ptr(),
        };
        let mut config = Jit::<State>::new_config();
        // ARM9/ARM11 handsets: v5TE covers the ARM9 of BREW 1.x/2.x, v6K the ARM11 of 3.x, and
        // v6K is a superset for user code.
        config.arch_ver(ArchVersion::V6K);
        config.code_cache_size(64 * 1024 * 1024);
        // Entry = start of the page on the host, no offset, no attribute bits.
        config.page_table_mask(0);
        unsafe { config.page_table(self.table.as_mut_ptr().cast()) };
        let mut jit = Box::new(config.init(state));
        let ptr = &mut *jit as *mut Jit<State>;
        jit.jit.set(ptr);
        jit.set_cpsr(USER_MODE);
        self.jit = Some(jit);
        Ok(())
    }

    fn read_reg(&self, reg: Reg) -> u32 {
        self.jit().map_or(0, |jit| jit.get_reg(reg.index()))
    }

    fn write_reg(&mut self, reg: Reg, value: u32) {
        if let Ok(jit) = self.jit_mut() {
            jit.set_reg(reg.index(), value);
        }
    }

    fn instructions(&self) -> u64 {
        self.jit().map_or(0, |jit| jit.instructions.get())
    }

    fn in_thumb(&self) -> bool {
        self.jit().is_ok_and(|jit| jit.get_cpsr() & CPSR_THUMB != 0)
    }

    fn read_mem(&self, addr: u32, buf: &mut [u8]) -> Result<(), CpuError> {
        let memory = self.memory.borrow();
        buf.copy_from_slice(memory.read(addr, buf.len() as u32)?);
        Ok(())
    }

    fn write_mem(&mut self, addr: u32, data: &[u8]) -> Result<(), CpuError> {
        self.memory.borrow_mut().write(addr, data)?;
        self.invalidate_written_code(addr, data.len() as u32);
        Ok(())
    }

    fn semihosting(&self) -> String {
        self.semihosting.borrow().clone()
    }

    fn run(&mut self, pc: u32, max_instructions: u64) -> Result<StopReason, CpuError> {
        let jit = self.jit_mut()?;
        // Bit 0 is the mode, not part of the address: an `lr` saved by Thumb code has it set.
        let cpsr = jit.get_cpsr();
        jit.set_cpsr(if pc & 1 == 1 {
            cpsr | CPSR_THUMB
        } else {
            cpsr & !CPSR_THUMB
        });
        jit.set_pc(pc & !1);
        // `HaltExecution` stays armed from the previous exit; clear it before re-entering.
        jit.clear_halt(HaltReason::UserDefined1);
        jit.stop.set(Stop::Nothing);
        jit.limit
            .set(jit.instructions.get().saturating_add(max_instructions));
        let _ = unsafe { jit.run() };
        let pages = std::mem::take(&mut *jit.dirty_code.borrow_mut());
        for page in pages {
            jit.invalidate_cache_range(page * PAGE, PAGE as usize);
        }
        Ok(match jit.stop.get() {
            Stop::Nothing => StopReason::Budget,
            Stop::Fetch(addr) if addr == RETURN_MAGIC => StopReason::Returned,
            Stop::Fetch(addr) if (API_BASE..API_BASE.saturating_add(API_SIZE)).contains(&addr) => {
                StopReason::ApiCall { addr }
            }
            Stop::Fetch(addr) => StopReason::MemoryFault { addr, pc: addr },
            Stop::Memory(addr) => StopReason::MemoryFault {
                addr,
                pc: jit.get_pc(),
            },
            Stop::Exception(pc) => StopReason::Exception { pc },
        })
    }
}

/// The start of `page` in host memory, when it is writable and lies entirely in one region.
fn page_pointer(memory: &GuestMemory, page: u32) -> *mut u8 {
    let start = u64::from(page) * u64::from(PAGE);
    let end = start + u64::from(PAGE);
    memory
        .regions()
        .iter()
        .find(|r| {
            r.writable
                && start >= u64::from(r.base)
                && end <= u64::from(r.base) + r.bytes.len() as u64
        })
        .map_or(std::ptr::null_mut(), |r| unsafe {
            // Regions are never reallocated after `reset`: they are written, never grown.
            r.bytes.as_ptr().add((start - u64::from(r.base)) as usize) as *mut u8
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu_with(code: &[u32]) -> DynarmicCpu {
        let mut mem = GuestMemory::new();
        let mut bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
        bytes.resize(0x1000, 0);
        mem.map("code", 0, bytes, true).unwrap();
        mem.map_zeroed("stack", 0x2000_0000, 0x1000).unwrap();
        let mut cpu = DynarmicCpu::new();
        cpu.reset(&mem).unwrap();
        cpu.write_reg(Reg::Sp, 0x2000_0f00);
        cpu
    }

    #[test]
    fn executes_and_returns_to_the_sentinel() {
        // mov r0, #42 ; bx lr
        let mut cpu = cpu_with(&[0xe3a0_002a, 0xe12f_ff1e]);
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 100), Ok(StopReason::Returned));
        assert_eq!(cpu.read_reg(Reg::R0), 42);
    }

    #[test]
    fn a_jump_into_the_api_range_becomes_a_call() {
        // bx r1
        let mut cpu = cpu_with(&[0xe12f_ff11]);
        cpu.write_reg(Reg::R1, API_BASE + 0x40);
        assert_eq!(
            cpu.run(0, 100),
            Ok(StopReason::ApiCall {
                addr: API_BASE + 0x40
            })
        );
    }

    #[test]
    fn host_writes_to_code_are_recompiled() {
        // mov r0, #1 ; bx lr
        let mut cpu = cpu_with(&[0xe3a0_0001, 0xe12f_ff1e]);
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        cpu.run(0, 100).unwrap();
        cpu.write_u32(0, 0xe3a0_0007).unwrap(); // mov r0, #7
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        cpu.run(0, 100).unwrap();
        assert_eq!(cpu.read_reg(Reg::R0), 7);
    }
}
