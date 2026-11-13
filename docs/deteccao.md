# Detecção de versão e resolução

Antes de abrir um jogo o brewx precisa de duas respostas: **qual BREW** emular e **qual tela**. As
duas podem ser forçadas (`--brew-version`, `--resolution`, ou os seletores do Qt); em `auto`, vêm
de `src/detect.rs`. Cada resposta tem uma confiança e a lista de evidências que a sustentou.

| Confiança  | Significado |
|------------|-------------|
| `Strong`   | indício direto, ou escolha manual |
| `Weak`     | indício indireto — vale conferir |
| `Fallback` | nada no pacote apontou; é o padrão |

## Versão

1. **Arranjo de pastas** (decide hoje):
   - `mif/<id>.mif` + `mod/<id>/` → BREW 3.x → **3.1**
   - `<nome>.mif` ao lado de `<nome>/` → BREW 1.x/2.x → **2.1** (superconjunto dos quatro)
2. **Cabeçalho do `.mif` e do `.mod`** — só registrados como evidência. O parser do `zeebx-core`
   foi derivado de `.mif` do BREW 4.0.2; ainda não sabemos se 1.x/2.x usam outro magic. Os dumps
   de `roms/` servem para descobrir isso: rode `brewx-headless info roms/` e compare.
3. **Fallback: 3.1.** BREW é retrocompatível — a versão mais nova roda a maioria dos títulos
   antigos, e o contrário quebra assim que o jogo pede uma interface nova.

A detecção em tempo de execução (quais interfaces/ClassIDs o jogo pede) virá com o despacho de
interfaces e é o sinal mais confiável para separar 3.0 de 3.1 e 1.x de 2.x.

## Resolução

1. **`LARGURAxALTURA` no caminho do módulo** (ex. `Jogo_176x208/`) → `Strong`.
2. **Imagens dos recursos**: PNG e BMP achados nos `.bar` e no `.mod`; conta só as do tamanho
   de uma tela conhecida (`Resolution::PRESETS`) e escolhe a mais frequente (empate: a maior).
   Splash e fundo têm o tamanho da tela; botões e sprites não batem com preset → `Weak`.
3. **Fallback: 176x220.**

Em tempo de execução, o tamanho que o jogo usa em `IDISPLAY`/`IBITMAP` confirmará ou corrigirá
a escolha.
