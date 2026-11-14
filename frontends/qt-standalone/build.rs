// The `QObject`s written in Rust, the C++ glue and the `brewx` QML module. Qt 6 is found through
// `qmake6`, `qmake` or the `QMAKE` variable.
fn main() {
    use cxx_qt_build::{CxxQtBuilder, QmlModule};

    CxxQtBuilder::new_qml_module(
        QmlModule::new("brewx")
            .qml_files(["qml/Main.qml"])
            .depend("QtQuick"),
    )
    .files(["src/bridge.rs", "src/screen.rs"])
    .include_dir("cpp")
    // The header goes in for moc: `FrameItem` has `Q_OBJECT`.
    .cpp_files(["cpp/frame.h", "cpp/frame.cpp"])
    .qt_module("Quick")
    // Qt Qml requires Network on macOS.
    .qt_module("Network")
    .build();
    println!("cargo:rerun-if-changed=cpp/frame.h");
    println!("cargo:rerun-if-changed=cpp/frame.cpp");
}
