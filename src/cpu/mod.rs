//! The ARM core. The emulator talks to [`CpuBackend`], never to a concrete core; the
//! implementation is Dynarmic, which recompiles A32/Thumb blocks into native host code.

pub mod dynarmic;
pub mod mem;

use std::fmt;

use mem::GuestMemory;
pub use zeebx_core::layout::{API_BASE, API_SIZE, RETURN_MAGIC};

/// Registers API dispatch reads and writes. AAPCS: arguments in `r0..r3`, the rest on the stack,
/// result in `r0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg {
    R0,
    R1,
    R2,
    R3,
    R4,
    R5,
    R6,
    R7,
    R8,
    R9,
    R10,
    R11,
    R12,
    Sp,
    Lr,
    Pc,
}

impl Reg {
    pub const ARGS: [Reg; 4] = [Reg::R0, Reg::R1, Reg::R2, Reg::R3];

    pub fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// The guest jumped into the trampoline range: a BREW API call.
    ApiCall {
        addr: u32,
    },
    /// The guest returned to the sentinel placed in `lr`.
    Returned,
    MemoryFault {
        addr: u32,
        pc: u32,
    },
    /// Undefined instruction, unknown SVC or another core exception.
    Exception {
        pc: u32,
    },
    /// The instruction budget ran out.
    Budget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuError(pub String);

impl fmt::Display for CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CpuError {}

impl From<mem::MemError> for CpuError {
    fn from(e: mem::MemError) -> Self {
        Self(e.to_string())
    }
}

pub trait CpuBackend {
    /// Takes the memory map. After this the bytes live inside the core: API implementations go
    /// through [`CpuBackend::read_mem`]/[`CpuBackend::write_mem`].
    fn reset(&mut self, mem: &GuestMemory) -> Result<(), CpuError>;

    fn read_reg(&self, reg: Reg) -> u32;
    fn write_reg(&mut self, reg: Reg, value: u32);

    /// Runs from `pc` (bit 0 set = Thumb) until it stops, spending at most `max_instructions`.
    fn run(&mut self, pc: u32, max_instructions: u64) -> Result<StopReason, CpuError>;

    /// Instructions executed so far: the emulator's clock, so two runs give the same result.
    fn instructions(&self) -> u64;

    fn in_thumb(&self) -> bool;

    fn read_mem(&self, addr: u32, buf: &mut [u8]) -> Result<(), CpuError>;
    fn write_mem(&mut self, addr: u32, data: &[u8]) -> Result<(), CpuError>;

    /// Text written through ARM semihosting (`SVC 0xAB`/`0x123456`), which some games use as
    /// their debug log.
    fn semihosting(&self) -> String;

    fn read_u32(&self, addr: u32) -> Result<u32, CpuError> {
        let mut b = [0; 4];
        self.read_mem(addr, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    fn write_u32(&mut self, addr: u32, value: u32) -> Result<(), CpuError> {
        self.write_mem(addr, &value.to_le_bytes())
    }
}

/// Keeps the semihosting buffer bounded: a game that logs every frame must not grow it forever.
pub(crate) fn trim_semihosting(text: &mut String) {
    const MAX: usize = 64 * 1024;
    if text.len() > MAX {
        let mut cut = text.len() - MAX / 2;
        while !text.is_char_boundary(cut) {
            cut += 1;
        }
        text.drain(..cut);
    }
}
