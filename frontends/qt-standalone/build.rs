// Os `QObject`s escritos em Rust, a cola C++ e o módulo QML `brewx`. O Qt 6 é achado pelo
// `qmake6`, `qmake` ou pela variável `QMAKE`.
fn main() {
    use cxx_qt_build::{CxxQtBuilder, QmlModule};

    CxxQtBuilder::new_qml_module(
        QmlModule::new("brewx")
            .qml_files(["qml/Main.qml"])
            .depend("QtQuick"),
    )
    .files(["src/bridge.rs"])
    .qt_module("Quick")
    // O Qt Qml exige o Network no macOS.
    .qt_module("Network")
    .build();
}
