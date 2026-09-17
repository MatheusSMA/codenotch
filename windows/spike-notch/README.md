# spike-notch

Spike descartavel, mantido como registro da medicao que decidiu o `codenotch-native`.

Desenha a pill e um arco girando numa janela layered do Win32, sem WebView, e
imprime o proprio working set a cada segundo.

Medido nesta maquina contra o build Tauri fazendo o mesmo trabalho:

| | WebView2 (Tauri) | layered nativo |
|---|---|---|
| RAM | 415 MB, 7 processos | 11,8 MB, 1 processo |
| FPS | 10 (capado de proposito) | 59 |
| CPU | — | 0,24% de um core |
| `dwm.exe` | — | 0% |

A conclusao que importa: `UpdateLayeredWindow` com alpha premultiplicado entrega
um bitmap pronto ao compositor, entao animacao suave aqui nao tem o custo que
uma superficie WebView2 transparente tem.

Rodar: `cargo run -p spike-notch --release`
