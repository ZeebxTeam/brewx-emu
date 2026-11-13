//! Guest memory, built from named regions instead of a flat 4 GiB block: an access outside the
//! map is an identifiable error, and that is exactly what makes a jump into the BREW vtable
//! trampolines detectable.

use std::fmt;

#[derive(Debug, Clone)]
pub struct Region {
    /// Used in logs and error messages.
    pub name: &'static str,
    pub base: u32,
    pub bytes: Vec<u8>,
    pub writable: bool,
    /// Only the null page is not: reading it is tolerated, jumping to it is a bug we want to see.
    pub executable: bool,
}

impl Region {
    fn end(&self) -> u64 {
        u64::from(self.base) + self.bytes.len() as u64
    }

    fn contains(&self, addr: u32, len: u32) -> bool {
        u64::from(addr) >= u64::from(self.base) && u64::from(addr) + u64::from(len) <= self.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemError {
    Unmapped {
        addr: u32,
        len: u32,
    },
    ReadOnly {
        addr: u32,
        region: &'static str,
    },
    /// Two regions overlap — a bug in building the map, not in the guest.
    Overlap {
        name: &'static str,
        base: u32,
    },
}

impl fmt::Display for MemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unmapped { addr, len } => {
                write!(f, "access to {addr:#010x} (+{len}) outside the memory map")
            }
            Self::ReadOnly { addr, region } => {
                write!(f, "write to {addr:#010x}, region '{region}' is read-only")
            }
            Self::Overlap { name, base } => {
                write!(f, "region '{name}' at {base:#010x} overlaps another")
            }
        }
    }
}

impl std::error::Error for MemError {}

#[derive(Debug, Default, Clone)]
pub struct GuestMemory {
    regions: Vec<Region>,
}

impl GuestMemory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn map(
        &mut self,
        name: &'static str,
        base: u32,
        bytes: Vec<u8>,
        writable: bool,
    ) -> Result<(), MemError> {
        self.map_executable(name, base, bytes, writable, true)
    }

    pub fn map_zeroed(
        &mut self,
        name: &'static str,
        base: u32,
        len: usize,
    ) -> Result<(), MemError> {
        self.map(name, base, vec![0; len], true)
    }

    pub fn map_executable(
        &mut self,
        name: &'static str,
        base: u32,
        bytes: Vec<u8>,
        writable: bool,
        executable: bool,
    ) -> Result<(), MemError> {
        let region = Region {
            name,
            base,
            bytes,
            writable,
            executable,
        };
        let overlaps = self
            .regions
            .iter()
            .any(|r| u64::from(region.base) < r.end() && u64::from(r.base) < region.end());
        if overlaps {
            return Err(MemError::Overlap { name, base });
        }
        self.regions.push(region);
        Ok(())
    }

    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    pub fn regions_mut(&mut self) -> &mut [Region] {
        &mut self.regions
    }

    fn find(&self, addr: u32, len: u32) -> Result<&Region, MemError> {
        self.regions
            .iter()
            .find(|r| r.contains(addr, len))
            .ok_or(MemError::Unmapped { addr, len })
    }

    fn find_mut(&mut self, addr: u32, len: u32) -> Result<&mut Region, MemError> {
        let region = self
            .regions
            .iter_mut()
            .find(|r| r.contains(addr, len))
            .ok_or(MemError::Unmapped { addr, len })?;
        if !region.writable {
            return Err(MemError::ReadOnly {
                addr,
                region: region.name,
            });
        }
        Ok(region)
    }

    pub fn executable(&self, addr: u32) -> bool {
        self.find(addr, 4).is_ok_and(|r| r.executable)
    }

    pub fn read(&self, addr: u32, len: u32) -> Result<&[u8], MemError> {
        let r = self.find(addr, len)?;
        let start = (addr - r.base) as usize;
        Ok(&r.bytes[start..start + len as usize])
    }

    pub fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemError> {
        let r = self.find_mut(addr, data.len() as u32)?;
        let start = (addr - r.base) as usize;
        r.bytes[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }

    pub fn fill(&mut self, addr: u32, value: u8, len: u32) -> Result<(), MemError> {
        let r = self.find_mut(addr, len)?;
        let start = (addr - r.base) as usize;
        r.bytes[start..start + len as usize].fill(value);
        Ok(())
    }

    pub fn read_u32(&self, addr: u32) -> Result<u32, MemError> {
        let b = self.read(addr, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn write_u32(&mut self, addr: u32, value: u32) -> Result<(), MemError> {
        self.write(addr, &value.to_le_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_writes_and_rejects() {
        let mut m = GuestMemory::new();
        m.map_zeroed("a", 0x1000, 0x100).unwrap();
        m.map("ro", 0x2000, vec![1, 2, 3, 4], false).unwrap();
        m.write_u32(0x1004, 0xdead_beef).unwrap();
        assert_eq!(m.read_u32(0x1004), Ok(0xdead_beef));
        assert_eq!(m.read_u32(0x2000), Ok(0x0403_0201));
        assert!(matches!(
            m.write_u32(0x2000, 0),
            Err(MemError::ReadOnly { .. })
        ));
        assert!(matches!(m.read_u32(0x10fe), Err(MemError::Unmapped { .. })));
        assert!(matches!(
            m.map_zeroed("b", 0x10f0, 0x20),
            Err(MemError::Overlap { .. })
        ));
    }
}
