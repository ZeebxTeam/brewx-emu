# Guia para agentes

## O que é o brewx

Emulador de **Qualcomm BREW 1.0 a 3.1**, irmão do [zeebx-emu](https://github.com/ZeebxTeam/zeebx-emu)
(que é focado no Zeebo, BREW 4.0.2). A emulação é de alto nível, como lá: o jogo é um binário ARM
que fala com o sistema por tabelas de ponteiros de função, e o que precisa ser fiel é essa
conversa, não o silício.

A diferença para o zeebx é que aqui **a versão de BREW e a resolução da tela variam por jogo**:
cada título foi feito para um aparelho. Por isso as duas são escolhidas antes de abrir o jogo —
manualmente ou por detecção (`src/detect.rs`, heurísticas em [`docs/deteccao.md`](docs/deteccao.md)).

## O mapa

```
Cargo.toml              a biblioteca `brewx` — o emulador inteiro, sem interface
src/
  version.rs            BrewVersion (1.0…3.1) e VersionSelect (auto/manual)
  display.rs            Resolution, presets e ResolutionSelect (auto/manual)
  package.rs            acha .mod/.mif/.bar num caminho; layout Legacy (1.x/2.x) ou Modern (3.x)
  detect.rs             detecção de versão e resolução, com confiança e evidências
  images.rs             acha PNG/BMP em bytes quaisquer (para a detecção de resolução)
  lib.rs                LaunchConfig::resolve e o relatório `describe`
frontends/
  headless/             `brewx-headless`: linha de comando, sem janela — alvo dos testes
  qt-standalone/        `brewx-qt`: interface Qt Quick (cxx-qt 0.10, Qt 6)
roms/                   jogos para teste — no .gitignore, nunca versionar
```

**A plataforma BREW independente de host vem do crate [`zeebx-core`](https://crates.io/crates/zeebx-core)**
(o mesmo do zeebx-emu): parsers de `.mod`/`.mif`/`.bar`, vtables e trampolins (`brew::aee*`), mapa
de memória (`layout`), heap, objetos, VFS e formato de save state. Uma correção lá exige publicar
nova versão do crate; para desenvolver junto, descomente o `[patch.crates-io]` no `Cargo.toml`
apontando para `../zeebx-core`.

O `zeebx-core` foi extraído de um BREW **4.0.2**: tabelas de interface e o parser de `.mif` refletem
essa versão. O que mudar entre 1.x, 2.x, 3.x e 4.x fica **neste repositório**, selecionado por
`BrewVersion` — não faça o core conhecer versões que ele não descreve.

## Regras

- Código em inglês (nomes); comentários, docs e strings de UI em português.
- Comentário explica o **porquê**, nunca o quê.
- **Toda heurística de detecção vem com evidência legível** (`Detection::evidence`). É o que
  permite calibrar contra os dumps de `roms/` e o que o usuário vê para decidir se força manual.
- A biblioteca não sabe de interface. Nada de Qt, janela ou áudio de host em `src/`.
- Licença **GPL-2.0-or-later**, como o zeebx-emu. Antes de uma dependência nova, confira a licença:
  nada GPL-2.0-only.
- Nomes de binário (`brewx-headless`, `brewx-qt`) não mudam depois de publicados.

## Comandos

```bash
cargo test -p brewx                                   # testes da biblioteca
cargo build -p brewx-headless                         # CLI
./target/debug/brewx-headless info roms/<jogo>        # inspeciona e mostra a detecção
./target/debug/brewx-headless info <caminho> --brew-version 2.1 --resolution 176x208

cargo build -p brewx-standalone-qt                    # interface (Qt 6 via qmake6/qmake/QMAKE)
./target/debug/brewx-qt [caminho]
# verificação da interface sem tela:
QT_QPA_PLATFORM=offscreen QT_FORCE_STDERR_LOGGING=1 ./target/debug/brewx-qt <caminho> --screenshot /tmp/qt.png
```

Erros de QML só aparecem em runtime; com `QT_FORCE_STDERR_LOGGING=1` eles vão para o terminal.
Sem porta/servidor: não há dev server.

## Roteiro

1. ~~Estrutura, detecção de versão/resolução, headless `info`, Qt com seletores~~
2. CPU ARM (dynarmic, como no zeebx-emu) + loader do `.mod` no mapa do `zeebx-core`
3. Despacho de interfaces por versão (IShell, IDisplay, IGraphics, ISound… de 1.x a 3.x)
4. `brewx-headless run` com dump de quadro; depois vídeo/entrada/áudio no Qt
5. Persistência das escolhas por jogo
