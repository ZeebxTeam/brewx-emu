//! Adivinha a versão de BREW e a resolução de tela a partir do pacote, antes de rodar o jogo.
//!
//! Cada resposta vem com a confiança e com as evidências que a sustentaram, para que o usuário
//! saiba quando vale forçar manualmente — e para calibrar as heurísticas contra os dumps de
//! `roms/`. Ver `docs/deteccao.md`.

use std::collections::HashMap;
use std::fs;

use zeebx_core::formats::miffile::{MifError, MifFile};
use zeebx_core::formats::modfile::ModImage;

use crate::display::{self, Resolution};
use crate::images;
use crate::package::{Layout, Package};
use crate::version::BrewVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Nada no pacote apontou um valor: é o padrão.
    Fallback,
    /// Um indício indireto (arranjo de pastas, imagens).
    Weak,
    /// Um indício direto (resolução no nome do arquivo) ou escolha do usuário.
    Strong,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection<T> {
    pub value: T,
    pub confidence: Confidence,
    /// Frases curtas, em ordem, do que foi visto no pacote.
    pub evidence: Vec<String>,
}

pub fn version(pkg: &Package) -> Detection<BrewVersion> {
    let mut evidence = Vec::new();

    // Ainda não sabemos se o cabeçalho do .mif ou do .mod muda entre versões: por enquanto só
    // entram como evidência, para calibrar com dumps reais.
    if let Some(mif) = &pkg.mif {
        match fs::read(mif) {
            Ok(data) => evidence.push(match MifFile::parse(&data) {
                Ok(m) => format!(
                    ".mif no formato conhecido (4.x/3.x?): {} applet(s), {} seção(ões)",
                    m.applets.len(),
                    m.sections.len()
                ),
                Err(MifError::BadMagic { found }) => {
                    format!(".mif com magic {found:#06x}, diferente do formato 4.x")
                }
                Err(e) => format!(".mif ilegível: {e}"),
            }),
            Err(e) => evidence.push(format!(".mif não abriu: {e}")),
        }
    }
    if let Ok(data) = fs::read(&pkg.module) {
        evidence.push(match ModImage::parse(data) {
            Ok(m) => format!(".mod {:?}, {} bytes", m.variant(), m.image().len()),
            Err(e) => format!(".mod não reconhecido: {e:?}"),
        });
    }

    let (value, confidence) = match pkg.layout {
        Some(Layout::Modern) => {
            evidence.push("pastas mif/ + mod/<id>/: layout do BREW 3.x".into());
            (BrewVersion::V3_1, Confidence::Weak)
        }
        Some(Layout::Legacy) => {
            // 1.0 a 2.1 usam o mesmo arranjo; 2.1 é o superconjunto das quatro.
            evidence.push(".mif ao lado da pasta do módulo: layout do BREW 1.x/2.x".into());
            (BrewVersion::V2_1, Confidence::Weak)
        }
        None => {
            evidence.push("layout de pastas desconhecido".into());
            (BrewVersion::FALLBACK, Confidence::Fallback)
        }
    };
    Detection {
        value,
        confidence,
        evidence,
    }
}

pub fn resolution(pkg: &Package) -> Detection<Resolution> {
    let mut evidence = Vec::new();

    if let Some(found) = display::find_in_text(&pkg.module.to_string_lossy()) {
        evidence.push(format!("{found} no caminho do módulo"));
        return Detection {
            value: found,
            confidence: Confidence::Strong,
            evidence,
        };
    }

    // A tela de abertura costuma ter o tamanho exato da tela; botões e sprites não batem com
    // nenhum preset, então ficam de fora sozinhos.
    let mut hits: HashMap<Resolution, usize> = HashMap::new();
    for path in pkg.resources.iter().chain([&pkg.module]) {
        let Ok(data) = fs::read(path) else { continue };
        for (w, h) in images::sniff_dimensions(&data) {
            let (Ok(w), Ok(h)) = (u16::try_from(w), u16::try_from(h)) else {
                continue;
            };
            let r = Resolution::new(w, h);
            if r.is_preset() {
                *hits.entry(r).or_default() += 1;
            }
        }
    }
    let best = hits
        .iter()
        .max_by_key(|&(r, &n)| (n, r.area()))
        .map(|(&r, &n)| (r, n));
    if let Some((r, n)) = best {
        evidence.push(format!("{n} imagem(ns) de {r} nos recursos"));
        return Detection {
            value: r,
            confidence: Confidence::Weak,
            evidence,
        };
    }

    evidence.push("nenhuma imagem do tamanho de uma tela conhecida".into());
    Detection {
        value: Resolution::FALLBACK,
        confidence: Confidence::Fallback,
        evidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::tests::{bmp_header, png_header};
    use crate::package::discover;
    use crate::scratch::Scratch;

    #[test]
    fn modern_layout_means_brew3() {
        let s = Scratch::new();
        s.file("brew/mif/1.mif", b"");
        s.file("brew/mod/1/a.mod", b"");
        let pkg = &discover(&s.path("brew")).unwrap()[0];
        let d = version(pkg);
        assert_eq!(
            (d.value, d.confidence),
            (BrewVersion::V3_1, Confidence::Weak)
        );
    }

    #[test]
    fn legacy_layout_means_brew2() {
        let s = Scratch::new();
        s.file("brew/a.mif", b"");
        s.file("brew/a/a.mod", b"");
        let pkg = &discover(&s.path("brew")).unwrap()[0];
        assert_eq!(version(pkg).value, BrewVersion::V2_1);
    }

    #[test]
    fn resolution_from_name_wins() {
        let s = Scratch::new();
        s.file("Jogo_240x320/a.mod", &png_header(176, 220));
        let pkg = &discover(&s.path("Jogo_240x320")).unwrap()[0];
        let d = resolution(pkg);
        assert_eq!(
            (d.value, d.confidence),
            (Resolution::new(240, 320), Confidence::Strong)
        );
    }

    #[test]
    fn resolution_from_most_frequent_screen_image() {
        let s = Scratch::new();
        let mut bar = png_header(128, 160);
        bar.extend(bmp_header(176, 208));
        bar.extend(png_header(176, 208));
        bar.extend(png_header(37, 12)); // botão: não é tela
        s.file("jogo/a.bar", &bar);
        s.file("jogo/a.mod", b"");
        let pkg = &discover(&s.path("jogo")).unwrap()[0];
        let d = resolution(pkg);
        assert_eq!(
            (d.value, d.confidence),
            (Resolution::new(176, 208), Confidence::Weak)
        );
    }

    #[test]
    fn falls_back_without_clues() {
        let s = Scratch::new();
        s.file("jogo/a.mod", b"");
        let pkg = &discover(&s.path("jogo")).unwrap()[0];
        assert_eq!(resolution(pkg).confidence, Confidence::Fallback);
        assert_eq!(version(pkg).value, BrewVersion::FALLBACK);
    }
}
