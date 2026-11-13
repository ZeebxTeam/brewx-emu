//! brewx-qt — o emulador com a interface Qt Quick. `brewx-qt <caminho>` já abre com o pacote;
//! `--screenshot <png>` grava a janela e sai.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod bridge;

use std::process::ExitCode;

use cxx_qt_lib::{
    QGuiApplication, QMap, QMapPair_QString_QVariant, QQmlApplicationEngine, QString, QUrl,
    QVariant,
};

/// Nome que o Wayland casa com o `.desktop` instalado para achar o ícone.
const APP_ID: &str = "brewx-qt";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Sai antes de criar o `QGuiApplication`: prova, sem tela, que o binário acha as libs do Qt.
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

    // `--screenshot <png>`: a janela se grava e fecha. Verifica a interface sem olhar a tela, até
    // com `QT_QPA_PLATFORM=offscreen`.
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
    // O engine antes da aplicação: os itens QML ainda referenciam o `QGuiApplication`.
    drop(owned_engine);
    drop(owned_app);
    if code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
