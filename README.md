# Voice Screen - Medicao de FPS + alivio da previa local

Patch baseado na versao "Tela cheia / espectadores / qualidade" da conversa.

## Por que a selecao de 60 FPS nao garante 60 FPS reais?

O `VideoProfile.fps` limita o loop ao maximo selecionado. A capturacao por `xcap::Monitor::capture_image()`, resize RGB, RGB->YUV, codificacao OpenH264 **software**, envio separado via SCTP DataChannel para cada espectador e decodificacao por software levam tempo. Se o trabalho por quadro passa de 16,7 ms, a origem ja nao consegue chegar a 60 FPS. `skip_frames(true)` pode reduzir o numero de quadros codificados para controlar bitrate. Cada espectador tem sua propria taxa de entrega.

## Alteracoes

1. Telemetria na aba Diagnostico a cada 5s:
  - `VIDEO ORIGEM`: FPS de captura, FPS codificados, ms de captura, ms de resize/RGB, ms de encoder/YUV e Mbps do fluxo antes de replicar por espectador.
  - `VIDEO ENVIO`: FPS completos enviados para cada espectador e FPS descartados/sobrescritos no canal WebRTC.
  - `VIDEO RECEPCAO`: FPS reagrupados na rede, antes de decodificar.
  - `VIDEO DECODER`: FPS decodificados e ms de decoder incluindo conversao RGB.
2. Previa **local** do apresentador limitada a 15 FPS para poupar copias massivas de RGB e atualizacoes no Slint; a transmissao WebRTC e a taxa do H.264 **nao sao limitadas** por isso. Compare fluidez NO ESPECTADOR, nao na previa local.
3. Sem alteracoes no microfone, audio do sistema, protocolo, UI ou VPS. Sem dependencias novas.

## Instalacao

Substitua apenas:

- `apps/desktop/src/screen.rs`
- `apps/desktop/src/p2p.rs`

Guarde um backup dos arquivos e mantenha os outros da versao funcional.

Compile e execute **RELEASE**, pois o perfil debug pode ser muito mais lento:

```powershell
cargo check -p voice-desktop
cargo build --release -p voice-desktop
.\target\release\voice-desktop.exe
```

Recomendo usar release tambem em todos os espectadores.

## Teste recomendado

1. Escolha 720p / 60 FPS ANTES de clicar em `Compartilhar tela` (mudar durante a transmissao so entra na proxima).
2. Emissao de A com B e C assistindo. Reproduza um video ou mova uma janela rapidamente.
3. Depois de 10-15 segundos, abra Diagnostico e anote as mensagens `VIDEO ORIGEM` em A e `VIDEO ENVIO` em A para B e C.
4. Em B e C, anote `VIDEO RECEPCAO` e `VIDEO DECODER`.
5. Repita em 720p/30 e 720p/15 FPS.

### Interpretacao

- Origem capturada << 60 e tempo de captura alto: o metodo de screenshot/captura e lento. Proxima arquitetura deve usar Windows Graphics Capture (captura por frames, GPU).
- Capturada ~60, mas codificada <<60: o encoder OpenH264 CPU ou frame skip/bitrate esta limitando. Proxima etapa e encoder de hardware (Media Foundation/Intel/NVIDIA/AMD) e controle de bitrate.
- Codificada ~60, mas envio <<60: gargalo WebRTC DataChannel/CPU de replicacao, especialmente com 2 espectadores. Melhor usar trilha de video RTP e controle de congestionamento.
- Envio ~60, recepcao <<60: rede/canal de video ou fragmentos perdidos.
- Recepcao ~60, decoder <<60: CPU do espectador/decodificacao.
- Decoder ~60, interface pouco fluida: apresentacao Slint/copia RGB/renderer e o gargalo.

**Limites**: a revisao foi estatica; cargo e Windows nao estao disponiveis neste ambiente. As mensagens de telemetria sao resultados a serem obtidos no Windows, nao FPS ja medidos. O patch prioriza diagnosticar e aliviar a previa, mas nao garante 60 FPS reais.
