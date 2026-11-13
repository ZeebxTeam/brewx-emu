//! Builds the guest memory map for a BREW module.
//!
//! The SDK linker script (`elf2mod.x`) links at `ro-base = 0`, so mapping the image at
//! [`MODULE_BASE`] with every absolute address taken relative to it needs no relocation. The
//! entry point is the same from BREW 1.0 to 4.0:
//!
//! ```c
//! int AEEMod_Load(IShell *ps, void *ph, IModule **pMod);
//! ```

use zeebx_core::brew::aee::Interface;
use zeebx_core::formats::modfile::ModImage;
use zeebx_core::layout::*;

use crate::cpu::mem::{GuestMemory, MemError};

pub struct LoadedModule {
    pub mem: GuestMemory,
    /// Address of `AEEMod_Load`.
    pub entry: u32,
    /// The `IShell*` passed in `r0`.
    pub shell: u32,
    /// The `AEEHelperFuncs*` passed in `r1`.
    pub helpers: u32,
    /// Where `AEEMod_Load` writes the `IModule*`.
    pub out_module: u32,
    /// Where `IModule_CreateInstance` writes the `IApplet*`.
    pub out_applet: u32,
    /// First free address of the object region, after the words above.
    pub objects_start: u32,
}

/// `AEESTDLIB_VER` values seen in real modules: zeebx-core's 4.0.2 value and the one 21 of the
/// `roms/` titles check for.
pub const KNOWN_HELPER_VERSIONS: [u32; 2] = [HELPER_VERSION, 0x10000];

/// The helper versions worth trying for `image`, most likely first.
pub fn helper_version_candidates(image: &ModImage) -> Vec<u32> {
    let mut out: Vec<u32> = required_helper_version(image.image()).into_iter().collect();
    for v in KNOWN_HELPER_VERSIONS {
        if !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

pub fn load(
    image: &ModImage,
    heap_size: usize,
    helper_version: u32,
) -> Result<LoadedModule, MemError> {
    let mut mem = GuestMemory::new();
    // Writable: the image carries `.data`. It starts before `MODULE_BASE` to hold the two words
    // `AEEStdLib.h` reads before the module base, and ends with room for `.bss`.
    let mut module = vec![0u8; MODULE_PREFIX as usize];
    module.extend_from_slice(image.image());
    module.resize(module.len() + MODULE_BSS_SLACK, 0);
    mem.map("module", MODULE_BASE - MODULE_PREFIX, module, true)?;
    // Handsets had no memory protection: reading through a null pointer returned something and
    // the game moved on. Reads see zeros; writing or jumping there is still an error.
    mem.map_executable(
        "null",
        0,
        vec![0; (MODULE_BASE - MODULE_PREFIX) as usize],
        false,
        false,
    )?;
    mem.map_zeroed("heap", HEAP_BASE, heap_size.min(HEAP_MAX_SIZE))?;
    mem.map_zeroed("stack", STACK_BASE, STACK_SIZE)?;
    mem.map_zeroed("objects", OBJECT_BASE, OBJECT_SIZE)?;
    mem.map("helpers", HELPERS_BASE, build_helper_table(), false)?;
    mem.map("vtables", VTABLE_BASE, build_vtables(), false)?;

    let shell = OBJECT_BASE;
    mem.write_u32(shell, vtable_addr(Interface::Shell))?;
    let out_module = shell + 16;
    let out_applet = out_module + 4;

    let helpers = HELPERS_BASE;
    // GET_HELPER()     = *((AEEHelperFuncs **)AEEMod_Load - 1)  -> base - 4
    // GET_HELPER_VER() = *(uint32 *)(AEEMod_Load - 4 - 4)       -> base - 8
    mem.write_u32(MODULE_BASE - 8, helper_version)?;
    mem.write_u32(MODULE_BASE - 4, helpers)?;

    Ok(LoadedModule {
        mem,
        entry: MODULE_BASE + image.entry(),
        shell,
        helpers,
        out_module,
        out_applet,
        objects_start: out_applet + 16,
    })
}

/// The `AEESTDLIB_VER` the module was built against, when its ARM `AEEMod_Load` checks it with
/// an immediate. Modules whose check is in Thumb or against a literal are not found here;
/// [`helper_version_candidates`] covers them by retrying.
///
/// `AEEModGen.c` starts with `if (GET_HELPER_VER() != AEESTDLIB_VER) return EVERSIONNOTSUPPORT;`,
/// and the constant changed between SDKs: zeebx-core writes the 4.0.2 value (1), while 21 of the
/// 87 `roms/` titles demand `0x10000` and refuse to load otherwise. Older `AEEModGen` versions
/// do not check at all. Compiled, the check is `ldr rX, [rY, #-8]` followed shortly by
/// `cmp rX, #imm`; the search is limited to the start of the image, where `AEEMod_Load` lives.
pub fn required_helper_version(image: &[u8]) -> Option<u32> {
    let words: Vec<u32> = image
        .as_chunks::<4>()
        .0
        .iter()
        .take(512)
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    words.iter().enumerate().find_map(|(i, &w)| {
        // LDR (immediate, pre-indexed, subtract, no writeback) with offset 8.
        if w & 0x0ff0_0fff != 0x0510_0008 {
            return None;
        }
        let rd = (w >> 12) & 0xf;
        words[i + 1..].iter().take(3).find_map(|&next| {
            // CMP Rd, #imm: an 8-bit value rotated right by twice the 4-bit rotation.
            let is_cmp = next & 0x0ff0_f000 == 0x0350_0000 && (next >> 16) & 0xf == rd;
            is_cmp.then(|| (next & 0xff).rotate_right(2 * ((next >> 8) & 0xf)))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeebx_core::brew::aee;

    fn image() -> ModImage {
        // b . (an ARM branch is what makes a valid raw image), padded to the minimum size.
        let mut code = 0xeaff_fffeu32.to_le_bytes().to_vec();
        code.resize(0x100, 0);
        ModImage::parse(code).unwrap()
    }

    #[test]
    fn finds_the_helper_version_check() {
        // Start of tetris.mod's AEEMod_Load: push; mov ip, r2; ldr r2, [pc, #56];
        // add r2, r2, pc; ldr r2, [r2, #-8]; cmp r2, #0x10000; movne r0, #4.
        let code: Vec<u8> = [
            0xe92d_400eu32,
            0xe1a0_c002,
            0xe59f_2038,
            0xe082_200f,
            0xe512_2008,
            0xe352_0b40,
            0x13a0_0004,
        ]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
        assert_eq!(required_helper_version(&code), Some(0x10000));
        assert_eq!(required_helper_version(&code[..16]), None);
    }

    #[test]
    fn shell_points_to_its_trampolines_and_helpers_sit_before_the_base() {
        let m = load(&image(), DEFAULT_HEAP_SIZE, HELPER_VERSION).unwrap();
        let vtable = m.mem.read_u32(m.shell).unwrap();
        assert_eq!(
            aee::decode(m.mem.read_u32(vtable + 8).unwrap()),
            Some((Interface::Shell, 2))
        );
        assert_eq!(m.mem.read_u32(MODULE_BASE - 4), Ok(HELPERS_BASE));
        assert_eq!(m.entry, MODULE_BASE);
    }
}
