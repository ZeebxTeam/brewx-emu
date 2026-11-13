// The `QObject`s written in Rust, the C++ glue and the `brewx` QML module. Qt 6 is found through
// `qmake6`, `qmake` or the `QMAKE` variable.
fn main() {
    use cxx_qt_build::{CxxQtBuilder, QmlModule};

    CxxQtBuilder::new_qml_module(
        QmlModule::new("brewx")
            .qml_files(["qml/Main.qml"])
            .depend("QtQuick"),
    )
    .files(["src/bridge.rs"])
    .qt_module("Quick")
    // Qt Qml requires Network on macOS.
    .qt_module("Network")
    .build();
}
