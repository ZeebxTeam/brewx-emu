//! The game list a frontend shows: every applet under a folder, with a title and the icon the
//! `.mif` carries.

use std::io;
use std::path::Path;
use std::rc::Rc;

use zeebx_core::formats::miffile::MifFile;

use crate::package::{self, Layout, Package};
use crate::video::Image;

pub struct Entry {
    pub title: String,
    pub package: Package,
    /// The largest image in the `.mif` that decodes. BCI icons do not decode yet.
    pub icon: Option<Rc<Image>>,
}

/// The applets under `root`, sorted by title. Extension modules (no applet in the `.mif`) are
/// left out: they cannot be opened on their own.
pub fn scan(root: &Path) -> io::Result<Vec<Entry>> {
    let mut entries: Vec<Entry> = package::discover(root)?
        .into_iter()
        .filter_map(|package| {
            let data = package.mif.as_ref().and_then(|m| std::fs::read(m).ok());
            let mif = data.as_deref().and_then(|d| MifFile::parse(d).ok());
            if mif.as_ref().is_some_and(|m| m.main_applet().is_none()) {
                return None;
            }
            let icon = mif
                .zip(data.as_deref())
                .and_then(|(m, d)| largest_icon(&m, d));
            Some(Entry {
                title: title(&package),
                package,
                icon,
            })
        })
        .collect();
    entries.sort_by_key(|e| e.title.to_lowercase());
    Ok(entries)
}

/// The folder name a person would recognize: in the modern layout the module folder is a numeric
/// ID, so the title comes from the folder that holds `mif/` and `mod/`.
fn title(package: &Package) -> String {
    let root = || package.module.ancestors().nth(3)?.file_name();
    match package.layout {
        Some(Layout::Modern) => {
            root().map_or(package.name.clone(), |n| n.to_string_lossy().into_owned())
        }
        _ => package.name.clone(),
    }
}

fn largest_icon(mif: &MifFile, data: &[u8]) -> Option<Rc<Image>> {
    mif.images(data)
        .into_iter()
        .filter_map(Image::decode)
        .max_by_key(|i| i.width * i.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    #[test]
    fn modern_layout_titled_by_its_root_folder() {
        let s = Scratch::new();
        s.file("Jogo Legal/mif/12345.mif", b"");
        s.file("Jogo Legal/mod/12345/jogo.mod", b"");
        s.file("Outro/outro.mif", b"");
        s.file("Outro/outro/outro.mod", b"");
        let titles: Vec<String> = scan(&s.path(""))
            .unwrap()
            .into_iter()
            .map(|e| e.title)
            .collect();
        assert_eq!(titles, ["Jogo Legal", "outro"]);
    }
}
