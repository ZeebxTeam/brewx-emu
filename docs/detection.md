# Version and resolution detection

Before opening a game, brewx needs two answers: **which BREW** to emulate and **which screen**.
Both can be forced (`--brew-version`, `--resolution`, or the Qt selectors); in `auto` they come
from `src/detect.rs`. Each answer carries a confidence and the evidence behind it.

| Confidence | Meaning |
|------------|---------|
| `Strong`   | direct evidence, or the user's choice |
| `Weak`     | indirect evidence — worth checking |
| `Fallback` | nothing in the package pointed anywhere; this is the default |

## Version

1. **Folder layout**: `mif/<id>.mif` + `mod/<id>/` only exists on a BREW 3.x handset file system
   → **3.1** (`Weak`). The flat layout (`<name>.mif` next to `<name>/`) decides nothing: 1.x/2.x
   handsets used it, but archives pack every version that way — Call of Duty: World at War, a
   2008 BREW 3.x title, comes flat.
2. **`.mif` and `.mod` headers** — recorded as evidence only. In the 87 packages of `roms/` every
   `.mif` has the same header (`magic 0x0011`, version 1) and the same index entry types
   (1, 6, 20480; type 6 is missing in some homebrew and Qualcomm demos). 78 `.mod` files are raw
   ARM, 9 carry the `BREW` header. None of it separates versions yet.
3. **`AEESTDLIB_VER` required by `AEEMod_Load`** (`0x10000` vs `1`), found statically in the
   ARM form of the check — evidence only for now; it marks the SDK generation, not yet a
   version. See [`runtime.md`](runtime.md).
4. **Fallback: 3.1.** BREW is backward compatible — the newest version runs most older titles,
   and the opposite breaks as soon as a game asks for a newer interface.

The reliable signal will come at runtime, once interfaces are dispatched: which ClassIDs and
interface methods a game asks for.

## Resolution

Only the last three path components count as names (the `.mod`, its folder and the folder
above, which in archive dumps is the archive name), so the user's own folders are not read as
hints.

1. **`WIDTHxHEIGHT` in the name** (`chess_240x320`, `MaximBalls320x240`) → `Strong`.
2. **A lone number in the name** (`GumBlox_240`, `bowl3d_176only`, `DragonIsland_320`) → a hint.
   `176` → 176x220, `240`/`320` → 240x320, `128` → 128x160, `208` → 176x208, `400` → 240x400.
3. **Full-screen images in the resources**: PNG and BMP found in the `.bar` files and the
   `.mod`. An image counts for the smallest preset of the same width whose height it fills by at
   least 85% — splash art leaves room for the annunciator bar (176x205..207, 240x298 and 240x378
   in the real dumps). With a name hint, only screens with that side count. The most frequent wins
   (tie: the larger) → `Weak`.
4. A name hint with no matching image → its usual screen, `Weak`.
5. **Fallback: 176x220.**

### Current results on `roms/` (87 packages)

26 get a `Strong` or `Weak` answer, 61 fall back. 54 packages have no PNG or BMP at all: their
art is in another format (BCI or game-specific packing). Decoding BCI, or reading the size the
game asks of `IDISPLAY`/`IBITMAP` at runtime, is what will close that gap.
