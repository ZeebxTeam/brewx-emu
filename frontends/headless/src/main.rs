//! brewx-headless — the emulator without a window, for tests and scripts.
//!
//! For now it only inspects: it finds the packages and shows the BREW version and resolution
//! that would be used. `run` comes once there is a CPU and interface dispatch.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use brewx::{LaunchConfig, ResolutionSelect, VersionSelect};

const USAGE: &str = "\
uso: brewx-headless <comando> [opções]

comandos:
  info <caminho>          pacotes encontrados (.mod, .mif ou pasta) e a configuração detectada
  run <caminho>           inicia o jogo e relata até onde foi; com vários pacotes, uma linha
                          por pacote
  --version               versão do brewx

opções:
  --brew-version <v>      auto (padrão), 1.0, 1.1, 2.0, 2.1, 3.0 ou 3.1
  --resolution <LxA>      auto (padrão) ou ex. 176x220
  --frames <n>            callbacks de timer a disparar depois do início (padrão 300)
  --screenshot <png>      com um pacote só, salva a tela final
  --keys <f:tecla,...>    aperta a tecla no frame f (solta 3 frames depois); teclas: up, down,
                          left, right, select, clr, soft1, soft2, star, pound, 0-9
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
    let mut frames = 300;
    let mut screenshot: Option<String> = None;
    let mut keys: Vec<(u32, u32)> = Vec::new();
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
            "--screenshot" => screenshot = Some(value(arg)?),
            "--keys" => keys = parse_keys(&value(arg)?)?,
            "--frames" => {
                frames = value(arg)?
                    .parse()
                    .map_err(|_| "--frames pede um número".to_owned())?
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
        ["run", path] => {
            let packages =
                brewx::package::discover(&PathBuf::from(path)).map_err(|e| e.to_string())?;
            let single = packages.len() == 1;
            // How many games hit each unimplemented method / unsupported class: the backlog,
            // ordered by reach.
            let mut missing: BTreeMap<String, usize> = BTreeMap::new();
            let mut classes: BTreeMap<u32, usize> = BTreeMap::new();
            for pkg in &packages {
                match brewx::run(pkg, &config, frames, &keys) {
                    Ok((resolved, report, screen)) if single => {
                        print!("{}", brewx::describe_run(pkg, &resolved, &report));
                        if let Some(path) = &screenshot {
                            std::fs::write(path, screen.to_png()).map_err(|e| e.to_string())?;
                            println!("\ntela salva em {path}");
                        }
                    }
                    Ok((_, report, _)) => {
                        println!("{}", brewx::summarize_run(pkg, &report));
                        for name in report.unimplemented.keys() {
                            *missing.entry(name.clone()).or_default() += 1;
                        }
                        for (clsid, req) in &report.classes {
                            if req.served.is_none() {
                                *classes.entry(*clsid).or_default() += 1;
                            }
                        }
                    }
                    Err(e) => println!("{:<24} erro: {e}", pkg.name),
                }
            }
            if !single {
                print_ranking("métodos sem implementação (jogos)", missing);
                print_ranking(
                    "classes não suportadas (jogos)",
                    classes
                        .into_iter()
                        .map(|(c, n)| (format!("{c:#010x}"), n))
                        .collect(),
                );
            }
            Ok(())
        }
        [] => Err("falta o comando".into()),
        [cmd, ..] => Err(format!("comando desconhecido ou incompleto: {cmd}")),
    }
}

fn print_ranking(title: &str, counts: BTreeMap<String, usize>) {
    let mut rows: Vec<_> = counts.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    println!("\n{title}:");
    for (name, n) in rows.iter().take(30) {
        println!("  {n:>4}  {name}");
    }
}

fn parse_keys(text: &str) -> Result<Vec<(u32, u32)>, String> {
    text.split(',')
        .map(|item| {
            let (frame, key) = item
                .split_once(':')
                .ok_or(format!("--keys: {item:?} não é frame:tecla"))?;
            let frame = frame
                .trim()
                .parse()
                .map_err(|_| format!("--keys: frame inválido em {item:?}"))?;
            let code = brewx::machine::avk::by_name(key.trim())
                .ok_or(format!("--keys: tecla desconhecida {key:?}"))?;
            Ok((frame, code))
        })
        .collect()
}
