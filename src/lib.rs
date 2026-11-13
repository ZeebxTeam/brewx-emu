//! brewx — a Qualcomm BREW 1.x to 3.x emulator.
//!
//! This library is the emulator without an interface; the frontends (`frontends/`) only present
//! it. The host-independent BREW platform (formats, vtables, heap, VFS, save state) comes from the
//! [`zeebx_core`] crate, the same one zeebx-emu uses.

pub mod cpu;
pub mod detect;
pub mod display;
pub mod images;
pub mod loader;
pub mod machine;
pub mod package;
pub mod version;
pub mod video;

#[cfg(test)]
pub(crate) mod scratch;

use std::fmt::Write as _;

pub use detect::{Confidence, Detection};
pub use display::{Resolution, ResolutionSelect};
pub use package::Package;
pub use version::{BrewVersion, VersionSelect};

/// What the user chose before opening a game. `Auto` in a field delegates it to detection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaunchConfig {
    pub version: VersionSelect,
    pub resolution: ResolutionSelect,
}

/// The final configuration of a package: a manual choice wins; otherwise, what detection found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub version: Detection<BrewVersion>,
    pub resolution: Detection<Resolution>,
}

impl LaunchConfig {
    pub fn resolve(&self, pkg: &Package) -> Resolved {
        let version = match self.version {
            VersionSelect::Manual(v) => manual(v),
            VersionSelect::Auto => detect::version(pkg),
        };
        let resolution = match self.resolution {
            ResolutionSelect::Manual(r) => manual(r),
            ResolutionSelect::Auto => detect::resolution(pkg),
        };
        Resolved {
            version,
            resolution,
        }
    }
}

fn manual<T>(value: T) -> Detection<T> {
    Detection {
        value,
        confidence: Confidence::Strong,
        evidence: vec!["escolhido manualmente".into()],
    }
}

/// Human-readable report of the packages found and the configuration of each. It is what the
/// headless frontend prints and what the Qt one shows in its inspection panel.
pub fn describe(packages: &[Package], config: &LaunchConfig) -> String {
    if packages.is_empty() {
        return "nenhum .mod encontrado\n".into();
    }
    let mut out = String::new();
    for pkg in packages {
        let r = config.resolve(pkg);
        let _ = writeln!(out, "== {} ==", pkg.name);
        let _ = writeln!(out, "módulo:   {}", pkg.module.display());
        match &pkg.mif {
            Some(m) => _ = writeln!(out, "mif:      {}", m.display()),
            None => _ = writeln!(out, "mif:      (não encontrado)"),
        }
        let _ = writeln!(out, "layout:   {:?}", pkg.layout);
        let _ = writeln!(
            out,
            "BREW:     {} ({:?})",
            r.version.value, r.version.confidence
        );
        for e in &r.version.evidence {
            let _ = writeln!(out, "  - {e}");
        }
        let _ = writeln!(
            out,
            "tela:     {} ({:?})",
            r.resolution.value, r.resolution.confidence
        );
        for e in &r.resolution.evidence {
            let _ = writeln!(out, "  - {e}");
        }
        out.push('\n');
    }
    out
}

/// Where games write their files, outside the ROM folders: `$XDG_DATA_HOME/brewx`,
/// `~/.local/share/brewx`, or `%APPDATA%\brewx`.
pub fn default_data_dir() -> std::path::PathBuf {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from)
    };
    env("XDG_DATA_HOME")
        .or_else(|| env("APPDATA"))
        .or_else(|| env("HOME").map(|h| h.join(".local/share")))
        .unwrap_or_else(std::env::temp_dir)
        .join("brewx")
}

/// Loads a package and boots it with the resolved configuration.
pub fn start(pkg: &Package, config: &LaunchConfig) -> Result<(Resolved, machine::Machine), String> {
    use zeebx_core::formats::{miffile::MifFile, modfile::ModImage};

    let resolved = config.resolve(pkg);
    let mif = pkg
        .mif
        .as_ref()
        .ok_or("sem .mif: não há ClassID do applet")?;
    let mif_bytes = std::fs::read(mif).map_err(|e| format!("{}: {e}", mif.display()))?;
    let clsid = MifFile::parse(&mif_bytes)
        .map_err(|e| format!("{}: {e}", mif.display()))?
        .main_applet()
        .ok_or("o .mif não declara applet")?;
    let module =
        std::fs::read(&pkg.module).map_err(|e| format!("{}: {e}", pkg.module.display()))?;
    let image = ModImage::parse(module).map_err(|e| format!("{}: {e:?}", pkg.module.display()))?;
    let app_dir = pkg
        .module
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let machine_config = machine::MachineConfig {
        version: resolved.version.value,
        resolution: resolved.resolution.value,
        heap_size: zeebx_core::layout::DEFAULT_HEAP_SIZE,
        app_dir,
        data_dir: default_data_dir().join("data").join(&pkg.name),
    };
    let machine =
        machine::Machine::start(&image, clsid, machine_config).map_err(|e| e.to_string())?;
    Ok((resolved, machine))
}

/// Boots a package and fires up to `frames` timer callbacks; returns the report and the last
/// screen.
///
/// `keys` is a script of `(frame, AVK code)`: the key is pressed when that frame is reached and
/// released three frames later.
pub fn run(
    pkg: &Package,
    config: &LaunchConfig,
    frames: u32,
    keys: &[(u32, u32)],
) -> Result<(Resolved, machine::Report, video::Framebuffer), String> {
    const HOLD_FRAMES: u32 = 3;
    let (resolved, mut machine) = start(pkg, config)?;
    while machine.halted().is_none() && machine.frames() < frames {
        let frame = machine.frames();
        for &(at, code) in keys {
            if at == frame {
                machine.key(code, true);
            }
            if at + HOLD_FRAMES == frame {
                machine.key(code, false);
            }
        }
        if !machine.fire_next_timer() {
            break;
        }
    }
    Ok((resolved, machine.report(), machine.screen().clone()))
}

/// One line per package: how far it got and why it stopped. For sweeping `roms/`.
pub fn summarize_run(pkg: &Package, report: &machine::Report) -> String {
    format!(
        "{:<24} {:<13?} stdlib={:<7} frames={:<4} classes={:<3} sem-impl={:<3} {}",
        pkg.name,
        report.stage,
        format!("{:#x}", report.helper_version),
        report.frames,
        report.classes.len(),
        report.unimplemented.len(),
        report.stop.as_deref().unwrap_or("ok"),
    )
}

/// The full run report of one package.
pub fn describe_run(pkg: &Package, resolved: &Resolved, report: &machine::Report) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "== {} ==", pkg.name);
    let _ = writeln!(
        out,
        "BREW {} / tela {}",
        resolved.version.value, resolved.resolution.value
    );
    let _ = writeln!(out, "estágio:     {:?}", report.stage);
    let _ = writeln!(
        out,
        "helpers:     AEESTDLIB_VER {:#x}",
        report.helper_version
    );
    let _ = writeln!(
        out,
        "frames:      {} (updates de tela: {})",
        report.frames, report.updates
    );
    let _ = writeln!(out, "instruções:  {}", report.instructions);
    let _ = writeln!(
        out,
        "parada:      {}",
        report.stop.as_deref().unwrap_or("-")
    );
    let _ = writeln!(out, "\nclasses pedidas:");
    for (clsid, req) in &report.classes {
        let served = req
            .served
            .map_or("não suportada".to_owned(), |i| i.name().to_owned());
        let _ = writeln!(out, "  {clsid:#010x} x{:<5} {served}", req.count);
    }
    let _ = writeln!(out, "\nchamadas (sem implementação marcadas com *):");
    let mut calls: Vec<_> = report.calls.iter().collect();
    calls.sort_by(|a, b| b.1.cmp(a.1));
    for (name, n) in calls {
        let mark = if report.unimplemented.contains_key(name) {
            "*"
        } else {
            " "
        };
        let _ = writeln!(out, " {mark} {n:>8}  {name}");
    }
    if !report.missing.is_empty() {
        let _ = writeln!(out, "\narquivos e recursos que faltaram:");
        for m in &report.missing {
            let _ = writeln!(out, "  {m}");
        }
    }
    if report.stop.is_some() {
        let _ = writeln!(out, "\núltimas chamadas antes da parada:");
        for c in &report.recent {
            let _ = writeln!(out, "  {c}");
        }
    }
    if !report.log.is_empty() {
        let _ = writeln!(out, "\nlog do jogo:");
        for l in &report.log {
            let _ = writeln!(out, "  {l}");
        }
    }
    out
}
