# brewx

![Status: Still In Early Development](https://img.shields.io/badge/status-still%20in%20early%20development-orange)

An emulator for **Qualcomm BREW 1.0 to 3.1** feature-phone applications, written in Rust. It is
the sibling of [zeebx-emu](https://github.com/ZeebxTeam/zeebx-emu) — which emulates the Zeebo
(BREW 4.0.2) — and is built on the same platform core, the
[`zeebx-core`](https://crates.io/crates/zeebx-core) crate.

**Status:** early. Games boot on an ARM JIT (Dynarmic) and draw: Asteroids is playable, and
about a quarter of the test corpus reaches its menus. No sound yet.

## Features

- BREW version **automatic or manual** (1.0, 1.1, 2.0, 2.1, 3.0, 3.1)
- Screen resolution **automatic or manual** (presets from 128x128 to 240x400, or any `WxH`)
- Detection shows its confidence and evidence, so you know when to force a value —
  see [`docs/detection.md`](docs/detection.md)
- Frontends: `brewx-headless` (command line) and `brewx-qt` (Qt 6)

## Building

Rust 1.93+. The interface needs **Qt 6** (found through `qmake6`, `qmake` or `QMAKE`).

```bash
cargo build --release -p brewx-headless
cargo build --release -p brewx-standalone-qt

./target/release/brewx-headless info path/to/game
./target/release/brewx-qt path/to/game
```

A game can be a `.mod`, a `.mif` or a folder (a whole handset dump included). Archives are not
opened directly: extract them first.

## License

GPL-2.0-or-later — see [`LICENSE`](LICENSE). brewx ships no games and no firmware: use your own
dumps.
