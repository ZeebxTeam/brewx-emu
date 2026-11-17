# Guide for agents

## What brewx is

An emulator for **Qualcomm BREW 1.0 to 3.1**, sibling of
[zeebx-emu](https://github.com/ZeebxTeam/zeebx-emu) (which targets the Zeebo, BREW 4.0.2). The
emulation is high-level, as there: the game is an ARM binary that talks to the system through
tables of function pointers, and what has to be faithful is that conversation, not the silicon.

The difference from zeebx is that here **the BREW version and the screen resolution vary per
game**: each title was built for one handset. So both are chosen before the game opens — by hand
or by detection (`src/detect.rs`, heuristics in [`docs/detection.md`](docs/detection.md)).

## The map

```
Cargo.toml              the `brewx` library — the whole emulator, with no interface
src/
  version.rs            BrewVersion (1.0…3.1) and VersionSelect (auto/manual)
  display.rs            Resolution, presets, name hints and ResolutionSelect (auto/manual)
  package.rs            finds .mod/.mif/.bar under a path; Legacy (flat) or Modern (3.x) layout
  detect.rs             version and resolution detection, with confidence and evidence
  images.rs             finds PNG/BMP inside arbitrary bytes (for resolution detection)
  library.rs            the game list for frontends: applets under a folder, title, .mif icon
  cpu/                  CpuBackend, region-based GuestMemory, Dynarmic backend (from zeebx-emu)
  loader.rs             guest memory map, AEESTDLIB_VER detection
  video.rs              framebuffer, 2D primitives, BMP/PNG decoding, built-in 8x8 font
  machine/              boot, dispatch, timers, keys; one file per area: helpers, shell,
                        resources (.bar), display (IDisplay/IGraphics/IImage), bitmap
                        (IBitmap/IDIB in guest memory), files (IFileMgr/IFile), stream
  lib.rs                LaunchConfig::resolve, `run`, and the reports
frontends/
  headless/             `brewx-headless`: command line, no window — the test target
  qt-standalone/        `brewx-qt`: Qt Quick interface (cxx-qt 0.10, Qt 6)
roms/                   test games — in .gitignore, never commit
```

**The host-independent BREW platform comes from the
[`zeebx-core`](https://crates.io/crates/zeebx-core) crate** (the same one zeebx-emu uses):
`.mod`/`.mif`/`.bar` parsers, vtables and trampolines (`brew::aee*`), the memory map (`layout`),
heap, objects, VFS and the save state format. The heap size is the host's choice
(`layout::DEFAULT_HEAP_SIZE` is the Zeebo's 64 MiB). A fix there means publishing a new crate
version; to develop alongside it, uncomment the `[patch.crates-io]` in `Cargo.toml` pointing at
`../zeebx-core`.

The CPU layer was ported from zeebx-emu and trimmed of Zeebo-specific optimizations (surface
write watches, JIT timing). Boot, dispatch and the `roms/` results are in
[`docs/runtime.md`](docs/runtime.md).

`zeebx-core` was extracted from a BREW **4.0.2**: its interface tables and `.mif` parser reflect
that version. Whatever changes between 1.x, 2.x, 3.x and 4.x lives **in this repository**,
selected by `BrewVersion` — do not teach the core versions it does not describe.

## Test corpus

`roms/` holds `.rar` archives from archive.org. They are extracted to `roms/_extracted/<name>/`
(also ignored):

```bash
cd roms && for f in *.rar; do mkdir -p "_extracted/${f%.rar}"; unrar x -o+ -inul "$f" "_extracted/${f%.rar}/"; done
```

brewx does not open `.rar` itself: the UnRAR license is not GPL-compatible. When changing a
heuristic, run `brewx-headless info roms/_extracted` before and after and compare; when changing
dispatch, do the same with `run` — the stage counts and the backlog ranking must not regress.

## Rules

- **Code, comments, docs and README in English.** User-facing strings (CLI output, Qt labels,
  detection evidence) are in Portuguese for now.
- Comments explain the **why**, never the what.
- **Every detection heuristic produces readable evidence** (`Detection::evidence`). That is what
  lets it be calibrated against `roms/` and what the user sees to decide whether to force a value.
- The library knows nothing about interfaces. No Qt, window or host audio in `src/`.
- License **GPL-2.0-or-later**, like zeebx-emu. Check the license of any new dependency:
  nothing GPL-2.0-only.
- Binary names (`brewx-headless`, `brewx-qt`) do not change once published.

## Commands

```bash
cargo test -p brewx                                   # library tests
cargo build -p brewx-headless                         # CLI
./target/debug/brewx-headless info roms/_extracted    # inspect and show detection
./target/debug/brewx-headless info <path> --brew-version 2.1 --resolution 176x208
./target/debug/brewx-headless run roms/_extracted/tetris          # boot one game, full report
./target/debug/brewx-headless run roms/_extracted --frames 300    # sweep + backlog ranking
./target/debug/brewx-headless run roms/_extracted/Asteroids --keys 100:select --screenshot /tmp/a.png
BREWX_TRACE=IDisplay:: ./target/debug/brewx-headless run <game> --frames 5   # calls + args on stderr

cargo build -p brewx-standalone-qt                    # interface (Qt 6 via qmake6/qmake/QMAKE)
./target/debug/brewx-qt [path]
# check the interface without a screen (the game runs for the delay first):
QT_QPA_PLATFORM=offscreen QT_FORCE_STDERR_LOGGING=1 ./target/debug/brewx-qt <path> --screenshot /tmp/qt.png --screenshot-delay 3000
```

QML errors only show up at runtime; `QT_FORCE_STDERR_LOGGING=1` sends them to the terminal.
No ports or dev server.

## Roadmap

1. ~~Structure, version/resolution detection, headless `info`, Qt with selectors~~
2. ~~ARM CPU (dynarmic) + loader + boot + API trace and backlog sweep~~
3. ~~First playable game: framebuffer, IDisplay/IGraphics/IImage/IBitmap/IDIB, `.bar`
   resources, files, memory streams, keys; Qt shows frames and takes the keyboard~~
4. Backlog by reach (`run roms/_extracted`): fonts (`IFont`, menus without text), clipping
   glitches, BCI images, `IHeap`, `GetDeviceInfoEx`, `IFileMgr` enumeration, sound
5. Per-version differences found by the sweep (vtables, `AEEDeviceInfo`, class IDs)
6. Per-game persistence of the chosen settings
