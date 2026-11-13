//! brewx — emulador de Qualcomm BREW 1.x a 3.x.
//!
//! Esta biblioteca é o emulador sem interface; os frontends (`frontends/`) só apresentam. A
//! plataforma BREW independente de host (formatos, vtables, heap, VFS, save state) vem do crate
//! [`zeebx_core`], o mesmo do zeebx-emu.

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

/// O que o usuário escolheu antes de abrir um jogo. `Auto` em cada campo delega à detecção.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaunchConfig {
    pub version: VersionSelect,
    pub resolution: ResolutionSelect,
}

/// A configuração final de um pacote: a escolha manual vence; senão, o que a detecção achou.
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

/// Relatório legível dos pacotes achados e da configuração de cada um. É o que o headless
/// imprime e o que o Qt mostra no painel de inspeção.
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
