# codenotch-native

A notch do Codenotch desenhada direto numa janela layered do Win32, sem WebView.

## Por que

O build Tauri carrega o WebView2, que mediu **415 MB em 7 processos** para uma
faixa que mostra duas porcentagens. O arco de trabalho dele ainda precisava ser
capado em ~10 fps porque um transform de SVG re-rasteriza na thread principal.

Aqui `UpdateLayeredWindow` recebe um bitmap pronto: **16 MB, 1 processo**, 60 fps
sem tocar no `dwm.exe`.

## O que ja faz

- Pill na borda com os fillets concavos do `notch.html`
- Anel de uso por provider, com as tres faixas de cor
- Marca do provider, rasterizada dos SVGs que o app ja versiona
- Porcentagem na Segoe UI Semibold
- Auto-hide na borda, com mola de verdade (velocidade sobrevive a interrupcao)
- Card de detalhes: clique na pill abre, clique de novo fecha, sair com o mouse fecha
- Anel girando enquanto ha um turno em andamento, e anel ambar pulsando quando
  uma sessao espera resposta sua
- Servidor de hooks na porta 48666: recebe os eventos do Claude Code direto
- Um painel por provider, cada um com uma barra de uso por limite e a lista de
  sessoes vivas com o que cada uma esta fazendo

- Busca os proprios numeros: `usage.rs` e `codex.rs` vieram do build Tauri com as
  mesmas tres linhas trocadas por um Hub. O app original nao precisa mais rodar.

## O que ainda nao faz

- Sem icone de bandeja e sem tela de configuracoes. Editar `config.json` na mao
  funciona (cuidado com o BOM: o parser serde rejeita).
- Cursor e Antigravity nao tem buscador aqui; se quiser esses dois, ainda e o app
  original que os escreve.

## Sobre a porta 48666

Esta binario **toma** a porta dos hooks. E o que torna o `attention` possivel:
so um hook event distingue um turno terminado de um esperando resposta, e isso
nao tem assinatura em disco. Consequencia: o app original nao pode rodar junto.

Se o bind falhar (o original ainda esta de pe), a notch continua funcionando e
cai para inferir "trabalhando" dos transcripts — so nao consegue mostrar ambar.

## Diagnostico

`CODENOTCH_TRACE=1` faz o app registrar cada mudanca de estado em
`%TEMP%\codenotch-native.log`. Vale a pena porque cutucar essa janela de fora
engana: ela e per-monitor DPI aware e a maioria das ferramentas nao e, entao um
cursor lido de outro processo nao concorda com o que ela ve.

## Rodar

```
cargo run -p codenotch-native --release
```

Le `%APPDATA%\codenotch\config.json`: `scale`, `notch_y` e `notch_slots`
(lista vazia significa todos os providers).
