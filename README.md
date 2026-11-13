# brewx

Emulador de **Qualcomm BREW 1.0 a 3.1**, em Rust. Projeto irmão do
[zeebx-emu](https://github.com/ZeebxTeam/zeebx-emu) — que emula o Zeebo (BREW 4.0.2) — e construído
sobre o mesmo núcleo de plataforma, o crate [`zeebx-core`](https://crates.io/crates/zeebx-core).

> *An emulator for Qualcomm BREW 1.x–3.x feature-phone apps, sibling of zeebx-emu.*

**Estado:** estrutura inicial. Já acha os pacotes e detecta versão de BREW e resolução; ainda não
executa jogos.

## Recursos

- Versão de BREW **automática ou manual** (1.0, 1.1, 2.0, 2.1, 3.0, 3.1)
- Resolução de tela **automática ou manual** (presets de 128x128 a 240x400, ou qualquer `LxA`)
- Detecção mostra a confiança e as evidências — dá para saber quando vale forçar
- Frontends: `brewx-headless` (linha de comando) e `brewx-qt` (Qt 6)

## Compilar

Rust 1.93+. A interface precisa de **Qt 6** (achado por `qmake6`, `qmake` ou `QMAKE`).

```bash
cargo build --release -p brewx-headless
cargo build --release -p brewx-standalone-qt

./target/release/brewx-headless info caminho/do/jogo
./target/release/brewx-qt caminho/do/jogo
```

O jogo pode ser um `.mod`, um `.mif` ou uma pasta (dump do aparelho inteiro, inclusive).

## Licença

GPL-2.0-or-later — ver [`LICENSE`](LICENSE). O brewx não inclui jogos nem firmware: use dumps
seus.
