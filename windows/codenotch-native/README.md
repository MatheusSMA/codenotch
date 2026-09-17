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
- Anel girando enquanto ha um turno em andamento

## O que ainda nao faz

- **Nao busca nada.** Le os snapshots que o app original persiste
  (`usage.json`, `codex.json`). Sem ele rodando, os numeros congelam.
- **Sem o estado `attention`** (o amarelo de "o chat espera por voce"). Ele chega
  como hook event na porta 48666, que e do app original, e nao tem assinatura em
  disco: um transcript que parou de crescer e identico quer o turno tenha acabado
  ou esteja esperando resposta. Mostrar amarelo aqui seria chute.
- Sem icone de bandeja e sem tela de configuracoes.

O passo que resolve os tres e tomar a porta 48666, o que obriga a portar tambem
`usage.rs` e `codex.rs`, e aposenta o app original.

## Rodar

```
cargo run -p codenotch-native --release
```

Le `%APPDATA%\codenotch\config.json`: `scale`, `notch_y` e `notch_slots`
(lista vazia significa todos os providers).
