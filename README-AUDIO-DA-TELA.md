# Voice Screen - Som do computador no compartilhamento de tela (MVP)

Este patch acrescenta a captura de audio do Windows a versao **ja funcional** do compartilhamento JPEG 720p/15 FPS. Nao altera os canais de microfone/voz P2P. Audio da tela = codec **Opus mono 48 kHz, quadros 20 ms**, canal WebRTC dedicado e nao confiavel (perda de pacotes e preferivel a atraso).

## Recursos
- Um botao independente `Som do computador: LIGADO/desligado` na aba Tela. **Desligado por padrao**, por privacidade.
- Quando o usuario compartilha a tela **e** ativa o som, a captura inicia; desligar qualquer um encerra o som da tela.
- Captura WASAPI de audio do sistema com **exclusao do proprio processo Voice Screen** para nao ecoar a chamada. Usa `wasapi::AudioClient::new_application_loopback_client(std::process::id(), false)`; o segundo parametro `false` corresponde a `PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE` no codigo da biblioteca, apesar da descricao abreviada de algumas paginas de documentacao.
- Audio capturado no formato float32 estereo do Windows e reduzido a mono para o MVP. Audio de voz permanece separado. A reproducao compartilhada usa o mesmo dispositivo de saida/volume mestre da chamada.
- O audio nao depende do monitor escolhido. Ele captura a mistura do sistema, *exceto o audio reproduzido pelo proprio aplicativo*, e nao somente audio da janela visualizada.

## Requisitos
- **Windows 11 ou Windows 10 build 20348+**, com suporte de `ActivateAudioInterfaceAsync` para loopback por processo.
- Rust 1.85+ (dependencia wasapi 0.26.0).
- O `voice-desktop` dos dois computadores precisa estar atualizado para aceitar o novo DataChannel.
- Nao precisa atualizar a VPS, servidor de sinalizacao, Nginx ou TURN.

## Instalar
Salve sua versao funcional no Git. Copie os arquivos para o mesmo caminho no workspace:
- `apps/desktop/src/main.rs`
- `apps/desktop/src/p2p.rs`
- `apps/desktop/src/screen_audio.rs` (novo)
- `apps/desktop/Cargo.toml`

O patch tambem inclui `screen.rs` apenas como referencia; esse arquivo e identico ao MVP anterior. `audio.rs`, `ui-controls.slint` e servidor permanecem inalterados.

```powershell
cargo check -p voice-desktop
cargo build --release -p voice-desktop
.\target\release\voice-desktop.exe
```

## Testar
1. Os dois PCs com a nova versao entram na mesma sala; **confirme que a voz segue normal**.
2. No PC A, abra Tela, clique `Som do computador: LIGADO` e depois `Compartilhar tela`.
3. Reproduza audio em navegador/media player do PC A. No PC B o audio deve tocar, incluindo com microfone A desligado.
4. Ligue o microfone A e verifique se voz+som da tela tocam simultaneamente.
5. Desligue `Som do computador`, mas deixe a tela ligada, e confirme que o audio para.
6. Pare o compartilhamento; a voz deve continuar.

## Diagnosticos
A aba Diagnostico registra `Canal de som da tela pronto`, `Som da tela WASAPI iniciado`, `Som da tela ENVIADO` e `Som da tela RECEBIDO`. Se nao ouvir, confira se o som esta realmente tocando no PC que transmite (na saida padrao) e se o PC receptor tem saida ativa/volume. Se `Som da tela indisponivel` ou `Falha capturando som da tela` aparecer, envie o erro e a versao/build do Windows.

## Limites e cautelas
- Windows com versao antiga pode rejeitar a captura por processo. O codigo **nao** ativa automaticamente captura global comum, porque isso causaria eco/retransmissao da chamada.
- O codec atual e mono para minimizar mudancas na cadeia de reproducao da voz; estereo pode ser incorporado depois.
- Alguns aplicativos com conteudo protegido/DRM podem bloquear a captura.
- Ao transmitir, participantes podem ouvir notificacoes e audio de outros aplicativos. Ative o som somente quando desejar compartilhar o audio do computador.
- O MVP ainda usa SCTP DataChannels. Para qualidade de producao migrar o compartilhamento de tela/voz para mecanismos RTP com controle de congestionamento.

**Verificacao**: arquivos gerados e avaliados estaticamente. Nao foi possivel compilar para Windows neste ambiente; rode `cargo check` no Windows e compartilhe erros se houver.
