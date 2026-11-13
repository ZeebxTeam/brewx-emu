//! brewx-qt — the emulator with the Qt Quick interface. `brewx-qt <path>` opens with that
//! package; `--screenshot <png>` saves the window and exits.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod bridge;

use std::process::ExitCode;

use cxx_qt_lib::{
    QGuiApplication, QMap, QMapPair_QString_QVariant, QQmlApplicationEngine, QString, QUrl,
    QVariant,
};

/// The name Wayland matches against the installed `.desktop` to find the icon.
const APP_ID: &str = "brewx-qt";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Exits before creating the `QGuiApplication`: proves, without a screen, that the binary
    // finds the Qt libraries.
    if args.first().map(String::as_str) == Some("--version") {
        println!("brewx-qt {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let mut owned_app = QGuiApplication::new();
    let mut owned_engine = QQmlApplicationEngine::new();
    let (Some(mut app), Some(mut engine)) = (owned_app.as_mut(), owned_engine.as_mut()) else {
        eprintln!("erro: o Qt não abriu");
        return ExitCode::FAILURE;
    };
    app.as_mut().set_application_name(&QString::from("brewx"));
    QGuiApplication::set_desktop_file_name(&QString::from(APP_ID));

    // `--screenshot <png>`: the window saves itself and closes. Checks the interface without
    // looking at the screen, even with `QT_QPA_PLATFORM=offscreen`.
    let mut screenshot = String::new();
    let mut initial = String::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--screenshot" => screenshot = it.next().cloned().unwrap_or_default(),
            _ => initial = arg.clone(),
        }
    }
    let mut properties = QMap::<QMapPair_QString_QVariant>::default();
    properties.insert(
        QString::from("initialPath"),
        QVariant::from(&QString::from(&initial)),
    );
    properties.insert(
        QString::from("screenshotPath"),
        QVariant::from(&QString::from(&screenshot)),
    );
    engine.as_mut().set_initial_properties(&properties);
    engine.load(&QUrl::from("qrc:/qt/qml/brewx/qml/Main.qml"));

    let code = app.exec();
    // The engine before the application: QML items still reference the `QGuiApplication`.
    drop(owned_engine);
    drop(owned_app);
    if code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
