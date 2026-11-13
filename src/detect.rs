//! Guesses the BREW version and the screen resolution from a package, before the game runs.
//!
//! Every answer carries its confidence and the evidence behind it, so the user knows when it is
//! worth forcing a value by hand, and so the heuristics can be calibrated against the `roms/`
//! dumps. See `docs/detection.md`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use zeebx_core::formats::miffile::{MifError, MifFile};
use zeebx_core::formats::modfile::ModImage;

use crate::display::{self, Resolution};
use crate::images;
use crate::package::{Layout, Package};
use crate::version::BrewVersion;

/// How many trailing path components count as the package's name: the `.mod` file, its folder
/// and the folder above it, which in archive dumps is the archive name (`GumBlox_240/`). Going
/// further up would read the user's own folders (`ssd500/`) as hints.
const NAME_COMPONENTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Nothing in the package pointed to a value: this is the default.
    Fallback,
    /// Indirect evidence (folder layout, images, a lone number in the name).
    Weak,
    /// Direct evidence (`WxH` in the name) or the user's choice.
    Strong,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection<T> {
    pub value: T,
    pub confidence: Confidence,
    /// Short sentences, in order, of what was seen in the package.
    pub evidence: Vec<String>,
}

pub fn version(pkg: &Package) -> Detection<BrewVersion> {
    let mut evidence = Vec::new();

    // Across the 87 packages in `roms/` the `.mif` header and index never vary, so neither
    // header decides anything yet. They stay as evidence for the next batch of dumps.
    if let Some(mif) = &pkg.mif {
        match fs::read(mif) {
            Ok(data) => evidence.push(match MifFile::parse(&data) {
                Ok(m) => format!(
                    ".mif no formato conhecido: {} applet(s), {} seção(ões)",
                    m.applets.len(),
                    m.sections.len()
                ),
                Err(MifError::BadMagic { found }) => {
                    format!(".mif com magic {found:#06x}, fora do formato conhecido")
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

    match pkg.layout {
        // Only a handset file system has `mif/` and `mod/`; nobody packs an archive that way.
        Some(Layout::Modern) => {
            evidence.push("pastas mif/ + mod/<id>/: layout do BREW 3.x".into());
            Detection {
                value: BrewVersion::V3_1,
                confidence: Confidence::Weak,
                evidence,
            }
        }
        // The flat layout is what 1.x/2.x handsets used, but also how archives pack any version:
        // Call of Duty: World at War, a 2008 BREW 3.x game, comes flat in `roms/`.
        Some(Layout::Legacy) => {
            evidence.push(
                ".mif ao lado da pasta do módulo: layout plano, comum a qualquer versão".into(),
            );
            fallback_version(evidence)
        }
        None => {
            evidence.push("layout de pastas desconhecido".into());
            fallback_version(evidence)
        }
    }
}

fn fallback_version(evidence: Vec<String>) -> Detection<BrewVersion> {
    Detection {
        value: BrewVersion::FALLBACK,
        confidence: Confidence::Fallback,
        evidence,
    }
}

pub fn resolution(pkg: &Package) -> Detection<Resolution> {
    let mut evidence = Vec::new();
    let names = name_text(&pkg.module);

    if let Some(found) = display::find_in_text(&names) {
        evidence.push(format!("{found} no nome do pacote"));
        return Detection {
            value: found,
            confidence: Confidence::Strong,
            evidence,
        };
    }

    let hint = display::find_hint(&names);
    if let Some(n) = hint {
        evidence.push(format!("número {n} no nome do pacote"));
    }

    // Splash and background art fill the screen; buttons and sprites match no preset and drop
    // out on their own. With a name hint, only screens that have that side count.
    let mut hits: HashMap<Resolution, usize> = HashMap::new();
    for path in pkg.resources.iter().chain([&pkg.module]) {
        let Ok(data) = fs::read(path) else { continue };
        for (w, h) in images::sniff_dimensions(&data) {
            if let Some(screen) = Resolution::screen_for_image(w, h)
                && hint.is_none_or(|n| screen.has_side(n))
            {
                *hits.entry(screen).or_default() += 1;
            }
        }
    }
    let best = hits
        .iter()
        .max_by_key(|&(r, &n)| (n, r.area()))
        .map(|(&r, &n)| (r, n));
    if let Some((r, n)) = best {
        evidence.push(format!(
            "{n} imagem(ns) de tela cheia para {r} nos recursos"
        ));
        return Detection {
            value: r,
            confidence: Confidence::Weak,
            evidence,
        };
    }

    if let Some(r) = hint.and_then(Resolution::for_hint) {
        return Detection {
            value: r,
            confidence: Confidence::Weak,
            evidence,
        };
    }

    evidence.push("nenhuma pista de tela no nome nem nas imagens".into());
    Detection {
        value: Resolution::FALLBACK,
        confidence: Confidence::Fallback,
        evidence,
    }
}

fn name_text(module: &Path) -> String {
    let parts: Vec<_> = module
        .components()
        .rev()
        .take(NAME_COMPONENTS)
        .map(|c| c.as_os_str().to_string_lossy())
        .collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::tests::{bmp_header, png_header};
    use crate::package::discover;
    use crate::scratch::Scratch;

    fn single(s: &Scratch, rel: &str) -> Package {
        discover(&s.path(rel)).unwrap().remove(0)
    }

    #[test]
    fn modern_layout_means_brew3() {
        let s = Scratch::new();
        s.file("brew/mif/1.mif", b"");
        s.file("brew/mod/1/a.mod", b"");
        let d = version(&single(&s, "brew"));
        assert_eq!(
            (d.value, d.confidence),
            (BrewVersion::V3_1, Confidence::Weak)
        );
    }

    #[test]
    fn flat_layout_is_inconclusive() {
        let s = Scratch::new();
        s.file("brew/a.mif", b"");
        s.file("brew/a/a.mod", b"");
        let d = version(&single(&s, "brew"));
        assert_eq!(
            (d.value, d.confidence),
            (BrewVersion::FALLBACK, Confidence::Fallback)
        );
    }

    #[test]
    fn resolution_from_name_wins() {
        let s = Scratch::new();
        s.file("Game_240x320/a.mod", &png_header(176, 220));
        let d = resolution(&single(&s, "Game_240x320"));
        assert_eq!(
            (d.value, d.confidence),
            (Resolution::new(240, 320), Confidence::Strong)
        );
    }

    #[test]
    fn resolution_from_most_frequent_screen_image() {
        let s = Scratch::new();
        let mut bar = png_header(128, 160);
        bar.extend(bmp_header(176, 206)); // splash minus the annunciator bar
        bar.extend(png_header(176, 208));
        bar.extend(png_header(37, 12)); // a button, not a screen
        s.file("game/a.bar", &bar);
        s.file("game/a.mod", b"");
        let d = resolution(&single(&s, "game"));
        assert_eq!(
            (d.value, d.confidence),
            (Resolution::new(176, 208), Confidence::Weak)
        );
    }

    #[test]
    fn name_hint_filters_images_and_stands_alone() {
        let s = Scratch::new();
        // Leftover 176-wide art from another build must not beat the `240` in the name.
        s.file("Zuma240/z/a.bar", &png_header(176, 208));
        s.file("Zuma240/z/a.mod", b"");
        let d = resolution(&single(&s, "Zuma240"));
        assert_eq!(
            (d.value, d.confidence),
            (Resolution::new(240, 320), Confidence::Weak)
        );
    }

    #[test]
    fn only_the_last_path_components_are_names() {
        let s = Scratch::new();
        s.file("disk_240x320/x/y/game/a.mod", b"");
        let pkg = single(&s, "disk_240x320/x/y/game");
        assert_eq!(resolution(&pkg).confidence, Confidence::Fallback);
    }

    #[test]
    fn falls_back_without_clues() {
        let s = Scratch::new();
        s.file("game/a.mod", b"");
        let pkg = single(&s, "game");
        assert_eq!(resolution(&pkg).confidence, Confidence::Fallback);
        assert_eq!(version(&pkg).value, BrewVersion::FALLBACK);
    }
}
