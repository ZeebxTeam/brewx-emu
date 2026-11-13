# Runtime: boot, dispatch and the `roms/` sweep

## Boot

1. `loader::load` builds the guest map from `zeebx-core`'s layout: module at `MODULE_BASE`
   (writable, with `.bss` slack), a readable null page, heap, stack, object region, the
   `AEEHelperFuncs` table and the vtables, whose slots all point into the trampoline range.
2. Before the module base sit `GET_HELPER_VER()` (base − 8) and `GET_HELPER()` (base − 4).
3. `AEEMod_Load(IShell*, AEEHelperFuncs*, IModule**)` runs; the game's own `AEEModGen` builds an
   `IModule` with its own vtable.
4. `IModule::CreateInstance` (slot 2) with the main applet ClassID from the `.mif`.
5. `IApplet::HandleEvent` (slot 2) with `EVT_APP_START`.
6. Timers (`ISHELL_SetTimer`) and callbacks (`ISHELL_Resume`) fire in deadline order. The clock
   is virtual — instructions / 100,000 per millisecond, plus jumps to the next deadline — so two
   runs give the same result.

Every jump into the trampoline range stops the core; `zeebx_core::brew::aee::decode` turns the
address into `(interface, slot)`, and dispatch goes by method name. Unimplemented methods return
`0` and are counted.

## `AEESTDLIB_VER`

`AEEModGen.c` refuses to load when `GET_HELPER_VER()` differs from the `AEESTDLIB_VER` of the SDK
it was built with, returning `EVERSIONNOTSUPPORT` (4) before any API call. zeebx-core writes the
4.0.2 value, `1`. In `roms/`:

| value     | games | note |
|-----------|-------|------|
| `0x10000` | 24    | older SDKs (Asteroids, MR2004, Spider-Man, Return of the King…) |
| `0x1`     | 59    | includes modules whose `AEEModGen` does not check at all |

`loader::required_helper_version` finds the ARM form of the check statically
(`ldr rX, [rY, #-8]` + `cmp rX, #imm`); Thumb and literal-pool forms are covered by retrying the
known values when `AEEMod_Load` refuses. The statically found value is also detection evidence:
it ties the module to an SDK generation.

## Sweep

```bash
./target/debug/brewx-headless run roms/_extracted --frames 300
```

One line per package, then the unimplemented methods and unsupported ClassIDs ranked by how
many games hit them — the backlog. First run (84 applets + 3 extension modules):

| stage reached   | games |
|-----------------|-------|
| `Running`       | 14 (9 complete 300 frames) |
| `Started`       | 6  |
| `AppletCreated` | 20 |
| `ModuleCreated` | 41 |
| `Loaded`        | 2  |

Top of the backlog: `IFileMgr::OpenFile` (20 games), `IDisplay::DrawRect` (16),
`ILicense::GetPurchaseInfo` (13), `IShell::LoadResObject` (13), `IDisplay::GetFontMetrics` and
`IDisplay::Update` (10). Most unsupported class: `0x01002000` (9), then `0x01014bc3` (7, the
OpenGL ES class zeebx-emu calls `AEECLSID_GL`).

## Second sweep: video, resources and files

With the framebuffer, `IDisplay`/`IGraphics`/`IImage`/`IBitmap`/`IDIB`, `.bar` resources,
`IFileMgr`/`IFile`, `IMemAStream` and `ISHELL_GetHandler`, **24 games run** (up from 14) and most
of them show recognizable screens: Asteroids is playable from title to gameplay with
`--keys 100:select`. Findings along the way:

- **Old `.bar` strings have no encoding byte.** Zeebo `.bar` files put one before Latin-1 text and
  `zeebx-core`'s `ResFile::string` always drops it; Asteroids packs `"Play"`, `"Menu"` back to
  back and showed "lay". `resources.rs` decodes strings itself (printable first byte = text).
- **Off-screen bitmaps must live in guest memory.** Games `QueryInterface` an `IBitmap` for
  `IDIB` to reach the pixels, and create 0x0 DIBs whose `pBmp`/`cx`/`cy`/`nPitch` they point at
  their own buffers. The guest struct is the truth; the host loads and stores around each call.
- Loading more resources can expose deeper paths: SHS used to idle after one frame, and now reads
  its files and dies in its own code after listing a directory (`IFileMgr::EnumInit` is a stub).

## Known gaps

- Slot order comes from SDK 4.0.2 (`zeebx-core`). The calls seen so far are plausible for 1.x–3.x,
  but a version-specific vtable that differs will show up as nonsense calls in the sweep.
- Extension modules (`dll_*`, imicro3d) are not loaded yet.
- No sound. Text uses a built-in 8x8 font; handset fonts (`IFont`) are not emulated, and some
  menus (Street Fighter II) come out without text. BCI images are not decoded.
- `zeebx-core` 0.1.1 `Heap::alloc` overflows on huge sizes; `malloc` guards it here.
