# Voice Screen - Etapa 2: teste P2P WebRTC (Windows)

**Base:** projeto da Etapa 1, com o servidor Rust ja publicado em `wss://voz.virtuabil.cloud/ws`.

## O que foi alterado

- `apps/desktop/Cargo.toml`: dependencias `webrtc = 0.21.1`, `async-trait`, `rustls/ring`.
- `apps/desktop/src/main.rs`: interface Slint com painel de eventos P2P, WebSocket ja publicado, ciclo de mensagens de sinalizacao.
- `apps/desktop/src/p2p.rs`: PeerConnection por participante, SDP offer/answer, ICE com STUN publico, DataChannel e saudacao automatica.
- **Nao ha alteracoes** no servidor Rust, no Nginx, no Docker nem no protocolo `voice-protocol`.

## Como usar

1. Use a versao completa deste pacote **ou** copie apenas os tres arquivos alterados para o projeto ja funcional no Windows.
2. Na raiz do workspace Rust:

```powershell
cargo check -p voice-desktop
cargo run -p voice-desktop
```

3. Abra o aplicativo em **dois computadores Windows em redes diferentes**, mantendo `wss://voz.virtuabil.cloud/ws`.
4. No computador A, crie a sala. No computador B, entre com o codigo.
5. Aguarde o estado `Connected` e `DataChannel P2P aberto` em ambos os PCs.
6. O DataChannel envia automaticamente uma frase curta ao abrir. A recepcao deve aparecer como `Recebido P2P...`.
7. Para testar o Mesh, entre com um terceiro computador e confira que o novo participante estabelece um canal com os dois anteriores.

A partir da abertura do DataChannel, **a mensagem de teste nao trafega pela VPS**. A VPS transmite somente a sinalizacao (SDP), e o STUN ajuda a descobrir caminhos de conectividade. A interface desta etapa nao transmite voz nem tela.

## Limites e pontos a verificar

- Este prototipo usa **non-trickle ICE**: aguarda até 30 segundos pelos candidatos ICE antes de enviar cada oferta/resposta. Pode demorar para conectar em algumas redes.
- Utiliza STUN publico `stun:stun.l.google.com:19302` para uma prova de conceito. **Nao usa TURN**: determinados roteadores NAT e firewalls corporativos impedirao a comunicacao direta. Nesses casos, um erro de ICE nao significa problema no Nginx ou no servidor Rust.
- O STUN escolhido e dependente de terceiro. Antes de producao: STUN/TURN proprios, credenciais temporarias, ICE trickle, seguranca, autenticacao e limites de abuso.
- WebRTC 0.21.1 introduziu APIs novas em relacao aos exemplos antigos de webrtc-rs, por isso os arquivos usam `PeerConnectionBuilder`, `PeerConnectionEventHandler` e polling de `DataChannel`.
- Cuidado: a equipe deve executar `cargo check`/`cargo run` no Windows. **Nao foi possivel compilar este pacote no ambiente em que foi preparado** (sem Cargo nem acesso aos crates). Trate como prototipo de integracao e envie qualquer erro de compilacao completo para corrigirmos.
- Mantenha a VPS e o Nginx inalterados. Nao e necessario refazer `docker compose up` para este teste.

## Proxima etapa

Após conexao P2P e testes Mesh: implantar Coturn na VPS, distribuir credenciais TURN temporarias, implementar voz Opus com captura/saida de audio e em seguida captura de tela Windows/H.264.
