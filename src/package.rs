//! Encontra os aplicativos BREW num caminho: o `.mod`, o `.mif` que o descreve e os `.bar`.
//!
//! Dois arranjos de pasta convivem nos dumps, e eles próprios já dizem algo sobre a versão:
//!
//! ```text
//! Legacy (BREW 1.x/2.x)        Modern (BREW 3.x)
//! brew/jogo.mif                brew/mif/12345.mif
//! brew/jogo/jogo.mod           brew/mod/12345/jogo.mod
//! brew/jogo/jogo.bar           brew/mod/12345/jogo.bar
//! ```

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Profundidade máxima da varredura de uma pasta; dumps de aparelho inteiro raramente passam de 4.
const MAX_DEPTH: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// `.mif` ao lado da pasta do módulo, com o mesmo nome dela.
    Legacy,
    /// `mif/<id>.mif` e `mod/<id>/`.
    Modern,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// Nome da pasta do módulo (no layout moderno, o ID numérico).
    pub name: String,
    pub module: PathBuf,
    pub mif: Option<PathBuf>,
    pub resources: Vec<PathBuf>,
    /// `None` quando o `.mif` não foi achado em nenhum dos dois arranjos.
    pub layout: Option<Layout>,
}

/// Os pacotes em `path`, que pode ser um `.mod`, um `.mif` ou uma pasta.
pub fn discover(path: &Path) -> io::Result<Vec<Package>> {
    if path.is_dir() {
        let mut modules = Vec::new();
        collect_modules(path, 0, &mut modules)?;
        modules.sort();
        return Ok(modules.iter().map(|m| from_module(m)).collect());
    }
    match extension(path).as_deref() {
        Some("mod") => Ok(vec![from_module(path)]),
        Some("mif") => Ok(module_for_mif(path)
            .map(|m| from_module(&m))
            .into_iter()
            .collect()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} não é .mod, .mif nem pasta", path.display()),
        )),
    }
}

fn collect_modules(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) -> io::Result<()> {
    if depth > MAX_DEPTH {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_modules(&path, depth + 1, out)?;
        } else if extension(&path).as_deref() == Some("mod") {
            out.push(path);
        }
    }
    Ok(())
}

fn from_module(module: &Path) -> Package {
    let dir = module.parent().unwrap_or(Path::new("."));
    let name = file_name(dir)
        .or_else(|| module.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let parent = dir.parent();

    let modern = parent
        .filter(|p| file_name(p).is_some_and(|n| n.eq_ignore_ascii_case("mod")))
        .and_then(|p| p.parent())
        .and_then(|root| find_ci(root, "mif"))
        .and_then(|mif_dir| find_ci(&mif_dir, &format!("{name}.mif")));
    let legacy = || parent.and_then(|p| find_ci(p, &format!("{name}.mif")));
    let loose = || files_with_ext(dir, "mif").into_iter().next();

    let (mif, layout) = if let Some(m) = modern {
        (Some(m), Some(Layout::Modern))
    } else if let Some(m) = legacy() {
        (Some(m), Some(Layout::Legacy))
    } else {
        (loose(), None)
    };

    Package {
        name,
        module: module.to_path_buf(),
        mif,
        resources: files_with_ext(dir, "bar"),
        layout,
    }
}

/// O caminho inverso: do `.mif` para o `.mod` que ele descreve.
fn module_for_mif(mif: &Path) -> Option<PathBuf> {
    let stem = mif.file_stem()?.to_string_lossy().into_owned();
    let dir = mif.parent()?;
    let module_dir = if file_name(dir).is_some_and(|n| n.eq_ignore_ascii_case("mif")) {
        find_ci(&find_ci(dir.parent()?, "mod")?, &stem)?
    } else {
        find_ci(dir, &stem)?
    };
    files_with_ext(&module_dir, "mod").into_iter().next()
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
}

fn file_name(path: &Path) -> Option<String> {
    path.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Dumps vêm de FAT, onde `JOGO.MIF` e `jogo.mif` são o mesmo arquivo; no Linux não são.
fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| file_name(p).is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

fn files_with_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && extension(p).as_deref() == Some(ext))
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    #[test]
    fn finds_modern_layout() {
        let s = Scratch::new();
        s.file("brew/mif/12345.mif", b"");
        s.file("brew/mod/12345/jogo.mod", b"");
        s.file("brew/mod/12345/jogo.bar", b"");
        let found = discover(&s.path("brew")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "12345");
        assert_eq!(found[0].layout, Some(Layout::Modern));
        assert_eq!(found[0].mif, Some(s.path("brew/mif/12345.mif")));
        assert_eq!(found[0].resources, vec![s.path("brew/mod/12345/jogo.bar")]);
    }

    #[test]
    fn finds_legacy_layout_case_insensitive() {
        let s = Scratch::new();
        s.file("brew/JOGO.MIF", b"");
        s.file("brew/jogo/jogo.mod", b"");
        let found = discover(&s.path("brew/jogo/jogo.mod")).unwrap();
        assert_eq!(found[0].layout, Some(Layout::Legacy));
        assert_eq!(found[0].mif, Some(s.path("brew/JOGO.MIF")));
    }

    #[test]
    fn mif_leads_back_to_module() {
        let s = Scratch::new();
        s.file("brew/mif/7.mif", b"");
        s.file("brew/mod/7/a.mod", b"");
        let found = discover(&s.path("brew/mif/7.mif")).unwrap();
        assert_eq!(found[0].module, s.path("brew/mod/7/a.mod"));
    }

    #[test]
    fn loose_module_has_no_layout() {
        let s = Scratch::new();
        s.file("solto/a.mod", b"");
        s.file("solto/qualquer.mif", b"");
        let found = discover(&s.path("solto")).unwrap();
        assert_eq!(found[0].layout, None);
        assert_eq!(found[0].mif, Some(s.path("solto/qualquer.mif")));
    }
}
