//! brewx — a Qualcomm BREW 1.x to 3.x emulator.
//!
//! This library is the emulator without an interface; the frontends (`frontends/`) only present
//! it. The host-independent BREW platform (formats, vtables, heap, VFS, save state) comes from the
//! [`zeebx_core`] crate, the same one zeebx-emu uses.

pub mod detect;
pub mod display;
pub mod images;
pub mod package;
pub mod version;

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
