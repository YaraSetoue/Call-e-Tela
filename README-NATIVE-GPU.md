# Voice Screen - captura e H.264 por hardware diretamente em Rust (Windows)

## O que substitui

Este patch **remove a dependencia de `ffmpeg.exe`** do backend de captura/codificacao. O novo modulo `native_video.rs` usa `moq-video 0.1.16` para acessar diretamente as APIs nativas do Windows:

`Windows.Graphics.Capture -> Direct3D 11 (NV12) -> Media Foundation H.264 hardware MFT -> H264 Annex-B -> VSHF -> WebRTC atual`

Nao executa processos externos. O mesmo backend tenta utilizar encoder nativo compativel com Intel, AMD ou NVIDIA. O modulo exige *hardware*; se nao houver, `screen.rs` registra o erro e volta para seu OpenH264 de CPU. Isso evita reportar software como GPU.

**Importante:** `moq-video` e uma biblioteca Rust independente; nao ha necessidade de adotar seu protocolo de rede MoQ. Aqui so usamos captura e encoder, **mantendo o seu WebRTC**. O H264 gerado e Annex-B com SPS/PPS, compatível com o fragmentador `VSH1` e decodificador da versao anterior.

## Arquivos a copiar

Faça um commit/backup e **substitua** na raiz do projeto:

- `apps/desktop/src/screen.rs` (versao atualizada do patch de FPS/qualidade)
- `apps/desktop/src/native_video.rs` (**novo**)
- `apps/desktop/Cargo.toml` (**referencia**: nao sobrescreva seu Cargo.toml atual se tiver outros ajustes; adicione apenas a dependencia abaixo)

`main.rs`, `p2p.rs`, `audio.rs`, `screen_audio.rs`, `ui-controls.slint` e servidor nao precisam mudar. O arquivo antigo `hardware_video.rs` pode permanecer no diretorio, pois nao sera mais referenciado, mas pode ser removido depois do teste.

**Para preservar todas as dependencias da versao funcional:** mantenha seu Cargo.toml atual e acrescente somente isto na secao `[target.'cfg(windows)'.dependencies]`:

```toml
moq-video = { version = "=0.1.16", features = ["capture"] }
```

## Requisitos

- Windows 10 versao 2004 (build 19041) ou superior (Windows 11 recomendado).
- GPU + driver com um encoder H.264 suportado pelo Media Foundation. Intel HD Graphics 620 e GPUs AMD com encoder podem funcionar; **modelo e driver da AMD nao foram verificados ainda**.
- Rust moderno com toolchain MSVC e Windows SDK; internet na primeira compilacao para baixar dependencias do Cargo.
- Nao precisa `ffmpeg.exe`, QSV/AMF/NVENC SDK externo, servidores novos ou atualizacao no Coturn.

## Compilar e executar

PowerShell na raiz do projeto:

```powershell
cargo check -p voice-desktop
cargo test -p voice-desktop --bin voice-desktop screen::tests
cargo build --release -p voice-desktop
Remove-Item Env:VOICE_VIDEO_BACKEND -ErrorAction SilentlyContinue
Remove-Item Env:VOICE_VIDEO_CODEC -ErrorAction SilentlyContinue
.\target\release\voice-desktop.exe
```

Se o `cargo check` falhar, anexe o **primeiro erro completo**. Este patch nao foi compilado neste ambiente Linux, portanto podera necessitar de pequenos ajustes de API/versao especificos da maquina Windows.

## Testar

1. Abra a mesma sala em dois PCs atualizados. Confirme a voz e o audio da tela.
2. Compartilhe **720p/30 FPS** e, depois, **1080p/30 FPS** no PC Intel.
3. A aba Diagnostico do apresentador deve indicar:

```
NATIVE GPU H.264 ativo: WGC/D3D11 -> ..., 1080p / 30 FPS (sem FFmpeg)
NATIVE GPU: primeiro quadro H.264 enviado (... bytes)
VIDEO ORIGEM NATIVE 1080p / 30 FPS: WGC ... FPS, enviados ao encoder ... FPS, H264 ... FPS, ... Mbps
```

4. Repita no PC AMD. Se nao suportar MFT por hardware ou a captura falhar, o log explica a causa e o aplicativo retorna a OpenH264.
5. Compare VIDEO ENVIO, VIDEO RECEPCAO e VIDEO DECODER nos espectadores.
6. Depois teste 720p/60 e 1080p/60 FPS. Observar FPS efetivo, CPU/GPU e atraso da imagem.

### Recuperacao imediata

No PowerShell:

```powershell
$env:VOICE_VIDEO_BACKEND="software"
.\target\release\voice-desktop.exe
```

Isso retorna ao OpenH264 mesmo quando a GPU esta disponivel. Para testar JPEG, combine `VOICE_VIDEO_CODEC="jpeg"`.

## Limitacoes desta etapa

- **Nao garante 60 FPS**: depende da GPU, driver, captura e decoder do receptor.
- A captura GPU e o H264 hardware agora sao nativos; **o video ainda usa DataChannel SCTP**, nao uma trilha RTP. Migrar a RTP com controle de congestionamento e a etapa seguinte.
- `moq-video` gerencia uma fila pequena de captura e descarta frames antigos em atraso, para preservar a voz.
- A previa local e feita separadamente por XCap em **1 FPS** e gera uma pequena transferencia CPU. O stream principal nao depende dela.
- A enumeracao de monitores WGC (`display:N`) pode ter ordem diferente da lista XCap, principalmente em sistemas multimonitor. Confira o monitor efetivamente capturado. Se necessario, o mapeamento deve ser unificado na UI posteriormente.
- O bitrate inicial passa a ser escolhido pelo encoder nativo; o ajuste dinamico por congestionamento sera adicionado quando migrarmos o transporte para RTP.
- Uma alteracao de resolucao do Windows enquanto a captura esta ativa pode requerer reiniciar o compartilhamento; falha retorna ao backend CPU.

## Verificacao realizada

- Verificado que o modulo novo nao chama `Command::new` ou `ffmpeg.exe`.
- O patch mantem os bytes `VSHF` e os limites `MAX_FRAME_BYTES` ja usados pelos clientes.
- Conferidos caminhos dos 3 arquivos e sintaxe TOML estatica.
- **Nao foi possivel executar `cargo check` ou testar hardware no Windows** neste ambiente.

Documentacao de referencia: https://docs.rs/moq-video/0.1.16/moq_video/ e https://docs.rs/crate/moq-video/0.1.16
