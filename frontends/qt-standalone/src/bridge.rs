//! The QML ↔ core bridge. The selector labels come from here, so the lists of versions and
//! resolutions live in one place (`brewx::BrewVersion::ALL`, `Resolution::PRESETS`).

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        type Inspector = super::InspectorRust;

        /// "Automático" followed by the versions, in the index order `inspect` receives.
        #[qinvokable]
        #[cxx_name = "versionOptions"]
        fn version_options(self: &Inspector) -> QStringList;

        /// "Automático" followed by the screen presets.
        #[qinvokable]
        #[cxx_name = "resolutionOptions"]
        fn resolution_options(self: &Inspector) -> QStringList;

        /// Report of the packages at `path` with the chosen configuration. Index 0 is automatic.
        #[qinvokable]
        fn inspect(self: &Inspector, path: &QString, version: i32, resolution: i32) -> QString;

        /// The games under `folder` as a JSON array of `{ title, path, icon }`; `icon` is a
        /// `file://` URL or empty.
        #[qinvokable]
        fn scan(self: &Inspector, folder: &QString) -> QString;
    }
}

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;

use cxx_qt_lib::{QString, QStringList};

use brewx::{BrewVersion, LaunchConfig, Resolution, ResolutionSelect, VersionSelect};

const AUTO_LABEL: &str = "Automático";

#[derive(Default)]
pub struct InspectorRust;

impl qobject::Inspector {
    fn version_options(&self) -> QStringList {
        std::iter::once(AUTO_LABEL.to_owned())
            .chain(BrewVersion::ALL.iter().map(|v| format!("BREW {v}")))
            .map(|s| QString::from(&s))
            .collect()
    }

    fn resolution_options(&self) -> QStringList {
        std::iter::once(AUTO_LABEL.to_owned())
            .chain(Resolution::PRESETS.iter().map(Resolution::to_string))
            .map(|s| QString::from(&s))
            .collect()
    }

    fn inspect(&self, path: &QString, version: i32, resolution: i32) -> QString {
        let config = launch_config(version, resolution);
        let path = PathBuf::from(path.to_string());
        let report = match brewx::package::discover(&path) {
            Ok(packages) => brewx::describe(&packages, &config),
            Err(e) => format!("erro: {e}"),
        };
        QString::from(&report)
    }

    fn scan(&self, folder: &QString) -> QString {
        let folder = PathBuf::from(folder.to_string());
        let Ok(entries) = brewx::library::scan(&folder) else {
            return QString::from("[]");
        };
        let items: Vec<String> = entries
            .iter()
            .map(|e| {
                format!(
                    "{{\"title\":{},\"path\":{},\"icon\":{}}}",
                    json_string(&e.title),
                    json_string(&e.package.module.to_string_lossy()),
                    json_string(&icon_url(e).unwrap_or_default()),
                )
            })
            .collect();
        QString::from(&format!("[{}]", items.join(",")))
    }
}

/// Where the icons go: regenerated on every scan, so a temporary folder is enough.
fn icon_dir() -> PathBuf {
    std::env::temp_dir().join("brewx-icons")
}

fn icon_url(entry: &brewx::library::Entry) -> Option<String> {
    let icon = entry.icon.as_ref()?;
    let mut hasher = DefaultHasher::new();
    entry.package.module.hash(&mut hasher);
    let dir = icon_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let file = dir.join(format!("{:016x}.png", hasher.finish()));
    std::fs::write(&file, icon.to_png()).ok()?;
    Some(format!("file://{}", file.display()))
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The selectors' indices as a launch configuration. An index outside the list (or 0) is
/// automatic.
pub(crate) fn launch_config(version: i32, resolution: i32) -> LaunchConfig {
    let pick = |i: i32| usize::try_from(i - 1).ok();
    LaunchConfig {
        version: pick(version)
            .and_then(|i| BrewVersion::ALL.get(i).copied())
            .map_or(VersionSelect::Auto, VersionSelect::Manual),
        resolution: pick(resolution)
            .and_then(|i| Resolution::PRESETS.get(i).copied())
            .map_or(ResolutionSelect::Auto, ResolutionSelect::Manual),
    }
}
