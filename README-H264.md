# Voice Screen - H.264 720p / 15 FPS (primeira etapa)

Este patch parte da versao que ja funciona com **voz Opus, tela JPEG e som da tela WASAPI**. Muda apenas o codec de **imagem da tela**; a voz e o som do computador continuam com seus fluxos anteriores. A sinalizacao e o Coturn da VPS nao precisam ser alterados.

## O que esta implementado

- H.264 **por software** com OpenH264 0.9.8, 1280x720, ate 15 FPS, bitrate alvo 2 Mbps e keyframe/IDR a cada 30 quadros (aprox. 2 segundos).
- Mesmo DataChannel WebRTC/SCTP de tela ja funcional. Agora quadros H.264 sao enviados em fragmentos `VSH1`, diferentes dos quadros JPEG (`VSS1`). Nao se trata ainda de uma trilha WebRTC RTP de video.
- Decoder H.264 executa em thread separada, sem bloquear o loop de sinalizacao/WebRTC. A fila de decodificacao e limitada; atrasos/quadros perdidos podem exigir esperar o proximo IDR.
- Visualizacao RGB no Slint. Previa local usa diretamente o RGB capturado, sem compressao adicional.
- Fallback JPEG se a inicializacao do encoder falhar ou ocorrerem falhas repetidas.
- Reversao manual: `VOICE_VIDEO_CODEC=jpeg`, sem precisar reverter os fontes.
- Testes unitarios dos pacotes fragmentados para H.264 e JPEG.

**Importante:** OpenH264 usa a CPU; esta etapa NAO implementa NVENC / Intel Quick Sync / AMD AMF e NAO garante melhor latencia/CPU em todo hardware. O ganho de largura de banda precisa ser medido no teste real. Para 1080p/1440p/60 FPS, a proxima etapa deve considerar hardware + trilha RTP e controle de congestionamento.

## Arquivos a substituir

Faça backup/commit da versao funcional primeiro. Copie estes quatro arquivos do ZIP, preservando o caminho:

- `apps/desktop/src/screen.rs`
- `apps/desktop/src/p2p.rs`
- `apps/desktop/src/main.rs`
- `apps/desktop/Cargo.toml`

O `screen_audio.rs` **nao e substituido**: mantenha o codigo WASAPI ja corrigido (com `.ok()` em `initialize_mta()`). O `audio.rs`, o `ui-controls.slint`, os protocolos e os arquivos da VPS permanecem inalterados.

## Compilar no Windows

```powershell
cargo check -p voice-desktop
cargo test -p voice-desktop --bin voice-desktop screen::tests
cargo build --release -p voice-desktop
.\target\release\voice-desktop.exe
```

A dependencia nova compila C/C++ nativo. Voce ja usa Visual Studio Build Tools para outros crates. Opcionalmente instale NASM para acelerar OpenH264; se houver erro de build, envie o **primeiro erro**.

## Testar

1. Atualize o EXE de **todos** os computadores participantes. O protocolo de tela H.264 exige clientes novos, embora audio e mensagens existentes nao sejam modificados.
2. Crie sala; confirme que microfones continuam funcionando em ambas as direcoes.
3. No emissor, compartilhe a tela e ative **Som do computador**.
4. No diagnostico do emissor, procure `Video H.264 OpenH264 iniciado` e `Primeiro quadro H.264 enviado`.
5. No receptor, procure `Primeiro quadro H.264 DECODIFICADO` e confira a imagem/atualizacao.
6. Confira se o som do computador e a voz permanecem funcionando durante a tela.
7. Pare e reinicie a tela. Se a imagem demorar a voltar, aguarde o proximo keyframe (~2s).

## Comparacao JPEG / H.264

Para voltar temporariamente ao JPEG no computador que compartilha, feche o programa e inicie **no mesmo PowerShell**:

```powershell
$env:VOICE_VIDEO_CODEC="jpeg"
.\target\release\voice-desktop.exe
```

Para voltar ao H.264:

```powershell
Remove-Item Env:VOICE_VIDEO_CODEC -ErrorAction SilentlyContinue
.\target\release\voice-desktop.exe
```

Observe a velocidade de atualizacao, uso de CPU e consumo de rede no Gerenciador de Tarefas. A qualidade configuravel 144p, 560p, 720p, 1080p, 1440p e 15/30/60 FPS continua planejada, mas somente 720p/15 esta habilitada neste patch.

## Limitacoes de validacao

Foi feita verificacao de codigo, API do OpenH264 e estrutura do ZIP, mas o ambiente em que o patch foi gerado **nao possui cargo/rustc**. Portanto, ainda nao foi compilado nem testado no Windows. Se o cargo reportar erro, compartilhe a mensagem completa para uma correção pontual.
