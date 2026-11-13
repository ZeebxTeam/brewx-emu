//! brewx-headless — the emulator without a window, for tests and scripts.
//!
//! For now it only inspects: it finds the packages and shows the BREW version and resolution
//! that would be used. `run` comes once there is a CPU and interface dispatch.

use std::path::PathBuf;
use std::process::ExitCode;

use brewx::{LaunchConfig, ResolutionSelect, VersionSelect};

const USAGE: &str = "\
uso: brewx-headless <comando> [opções]

comandos:
  info <caminho>          pacotes encontrados (.mod, .mif ou pasta) e a configuração detectada
  --version               versão do brewx

opções:
  --brew-version <v>      auto (padrão), 1.0, 1.1, 2.0, 2.1, 3.0 ou 3.1
  --resolution <LxA>      auto (padrão) ou ex. 176x220
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("erro: {msg}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let mut config = LaunchConfig::default();
    let mut positional = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or(format!("{name} pede um valor"));
        match arg.as_str() {
            "--version" => {
                println!("brewx-headless {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            "--brew-version" => {
                config.version = value(arg)?
                    .parse::<VersionSelect>()
                    .map_err(|e| e.to_string())?
            }
            "--resolution" => {
                config.resolution = value(arg)?
                    .parse::<ResolutionSelect>()
                    .map_err(|e| e.to_string())?
            }
            _ if arg.starts_with("--") => return Err(format!("opção desconhecida: {arg}")),
            _ => positional.push(arg.as_str()),
        }
    }

    match positional.as_slice() {
        ["info", path] => {
            let packages =
                brewx::package::discover(&PathBuf::from(path)).map_err(|e| e.to_string())?;
            print!("{}", brewx::describe(&packages, &config));
            Ok(())
        }
        [] => Err("falta o comando".into()),
        [cmd, ..] => Err(format!("comando desconhecido ou incompleto: {cmd}")),
    }
}
