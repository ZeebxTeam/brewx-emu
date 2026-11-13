//! A ponte QML ↔ núcleo. Os rótulos dos seletores saem daqui, para que a lista de versões e de
//! resoluções exista num lugar só (`brewx::BrewVersion::ALL`, `Resolution::PRESETS`).

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

        /// "Automático" seguido das versões, na ordem dos índices que `inspect` recebe.
        #[qinvokable]
        #[cxx_name = "versionOptions"]
        fn version_options(self: &Inspector) -> QStringList;

        /// "Automático" seguido dos presets de tela.
        #[qinvokable]
        #[cxx_name = "resolutionOptions"]
        fn resolution_options(self: &Inspector) -> QStringList;

        /// Relatório dos pacotes em `path` com a configuração escolhida. Índice 0 é automático.
        #[qinvokable]
        fn inspect(self: &Inspector, path: &QString, version: i32, resolution: i32) -> QString;
    }
}

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
        // Índice fora da lista (ou 0) é automático.
        let pick = |i: i32| usize::try_from(i - 1).ok();
        let config = LaunchConfig {
            version: pick(version)
                .and_then(|i| BrewVersion::ALL.get(i).copied())
                .map_or(VersionSelect::Auto, VersionSelect::Manual),
            resolution: pick(resolution)
                .and_then(|i| Resolution::PRESETS.get(i).copied())
                .map_or(ResolutionSelect::Auto, ResolutionSelect::Manual),
        };
        let path = std::path::PathBuf::from(path.to_string());
        let report = match brewx::package::discover(&path) {
            Ok(packages) => brewx::describe(&packages, &config),
            Err(e) => format!("erro: {e}"),
        };
        QString::from(&report)
    }
}
