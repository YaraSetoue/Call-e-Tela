# Voice Screen - captura DXGI/Direct3D11 + Intel Quick Sync (experimental)

Esta e a **primeira etapa pratica** de aceleracao por hardware para a Intel HD Graphics 620, preservando o projeto Rust/Slint e toda a arquitetura atual de voz, som da tela, espectadores e sinalizacao.

**Nao e ainda Windows Graphics Capture (WGC) nativo nem Media Foundation dentro do Rust.** O modulo `hardware_video.rs` controla o executavel **FFmpeg para Windows** que captura por **DXGI Desktop Duplication/Direct3D11** (`ddagrab`), mapeia frames para o dominio **QSV** (`hwmap=derive_device=qsv`), redimensiona na GPU (`vpp_qsv`) e codifica via `h264_qsv`. O fluxo H.264 Annex B volta para o Rust por pipe, com AUD para separar quadros. Continua usando o **mesmo DataChannel de video** da versao atual. Sem alterar p2p.rs ou o servidor.

A pre-visualizacao no computador que compartilha e intencionalmente limitada a 2 FPS por `xcap` (e gerada separadamente); **nao** representa o FPS enviado ao espectador. A UI de espectadores mantem o pipeline atual de decodificacao (CPU/Slint).

## Requisitos

- Windows com driver Intel compativel; o teste atual e em Intel HD Graphics 620.
- `ffmpeg.exe` recente no PATH ou caminho em `VOICE_FFMPEG_PATH`.
- Build do FFmpeg incluindo `ddagrab`, `hwmap`, `vpp_qsv`, `h264_qsv` e `h264_metadata`. Use um build confiavel. O ZIP nao inclui FFmpeg.
- Windows grafico em sessao interativa (captura DXGI normalmente nao funciona numa sessao remota sem monitor ativo).

Para conferir recursos no PowerShell:

```powershell
ffmpeg -hide_banner -filters | Select-String 'ddagrab|vpp_qsv|hwmap'
ffmpeg -hide_banner -encoders | Select-String h264_qsv
```

Se FFmpeg nao estiver no PATH, antes de abrir o aplicativo:

```powershell
$env:VOICE_FFMPEG_PATH = 'C:\Ferramentas\ffmpeg\bin\ffmpeg.exe'
```

## Instalar

Substituir apenas os arquivos:

- `apps/desktop/src/screen.rs`
- `apps/desktop/src/hardware_video.rs` **(novo)**

`p2p.rs`, `main.rs`, `screen_audio.rs`, `audio.rs` e outros arquivos permanecem intactos. **Mantenha o screen.rs com diagnostico de FPS**, pois o patch ja o inclui. Preserve backup / commit anterior.

```powershell
cargo check -p voice-desktop
cargo test -p voice-desktop --bin voice-desktop screen::hardware_video::tests
cargo build --release -p voice-desktop
.\target\release\voice-desktop.exe
```

O cliente tenta QSV automaticamente quando o FFmpeg esta disponivel; em caso de falha do comando, da inicializacao ou do fluxo, volta ao encoder OpenH264 por CPU. O fluxo H.264 entregue ao receptor mantem o marcador `VSHF` e a fragmentacao `VSH1` do projeto atual.

## Teste do FFmpeg isolado (antes do aplicativo)

```powershell
ffmpeg -hide_banner -init_hw_device d3d11va=d3d -filter_hw_device d3d -filter_complex 'ddagrab=output_idx=0:framerate=30,hwmap=derive_device=qsv,format=qsv,vpp_qsv=w=1280:h=720[v]' -map '[v]' -frames:v 150 -c:v h264_qsv -preset veryfast -f null NUL
```

O FFmpeg deve codificar quadros, sem erros de dispositivo, filtro ou QSV. Alguns builds antigos nao suportam mapear D3D11->QSV; nesse caso o aplicativo volta ao OpenH264. O mapa entre `output_idx` do DXGI e o numero mostrado pelo XCap pode variar entre varios monitores.

## Teste no Voice Screen

1. Primeiro selecione **720p / 30 FPS**; deixe o audio e o microfone funcionando.
2. O apresentador compartilha, B e C clicam em Assistir.
3. Na aba Diagnostico do apresentador, confirme mensagens:
   - `QSV iniciando: Direct3D11/DXGI -> Intel Quick Sync`;
   - `QSV ativo: primeiro quadro H.264 enviado`;
   - `VIDEO ORIGEM QSV ... FPS codificados`.
4. Em B e C, compare `VIDEO RECEPCAO` e `VIDEO DECODER` com FPS de origem; confirme tela e audio da tela normais.
5. Somente depois teste **720p / 60 FPS**, **1080p / 30 FPS** e **1080p / 60 FPS**.
6. Se a captura QSV nao abrir, procure `FFmpeg QSV:` e `Captura Intel QSV indisponivel:` no diagnostico. As ultimas linhas do FFmpeg aparecem sem incluir qualquer dado secreto.

## Retornar ao software imediatamente

No PowerShell, antes de executar o aplicativo:

```powershell
$env:VOICE_VIDEO_BACKEND = 'software'
.\target\release\voice-desktop.exe
```

Para reativar a tentativa de QSV:

```powershell
Remove-Item Env:VOICE_VIDEO_BACKEND -ErrorAction SilentlyContinue
.\target\release\voice-desktop.exe
```

`VOICE_VIDEO_CODEC=jpeg` continua disponivel e tem prioridade sobre a tentativa de QSV.

## Limites e proximas etapas

- **O FFmpeg e um processo separado**: a superficie continua na GPU durante captura, mapeamento, escala e encode, mas os bytes comprimidos H.264 passam por um pipe para o Rust. Nao e um pipeline nativo em processo/zero-copy ate o viewer.
- A transmissao continua via DataChannel SCTP; em algumas redes/PCs, o proximo gargalo sera a replicacao e decodificacao da imagem. A evolucao correta e WebRTC video RTP/SRTP e decoder acelerado.
- O redimensionamento QSV usa a largura/altura escolhidas; fontes com proporcao diferente de 16:9 podem ficar esticadas nesta versao experimental. A via OpenH264 anterior continua preservando proporcoes.
- **Nao ha garantia de 1080p/60 FPS** na HD 620; o desempenho precisa ser medido.
- A API WGC (Windows Graphics Capture) com Direct3D 11 e a etapa seguinte, quando substituirmos a integracao FFmpeg por captura/encode nativos no processo Rust.
- Este patch nao foi compilado nem executado num PC Windows neste ambiente. Testes de parser fornecidos em Rust; a sintaxe externa do FFmpeg depende do build/driver Windows do usuario.
