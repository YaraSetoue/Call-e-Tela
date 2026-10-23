# Etapa 5 - Ligacao de voz P2P (prototipo Windows)

Esta etapa parte do projeto testado com WebRTC DataChannel e credenciais TURN temporarias.
**Nenhuma alteracao no servidor Rust da VPS, Nginx, Coturn nem Docker Compose e necessaria.**

## O que mudou

- `apps/desktop/src/audio.rs`: captura de microfone, Opus mono (48 kHz, 20 ms), fila limitada,
  decoder por participante, pequena compensacao de jitter, mixagem e reproducao CPAL.
- `apps/desktop/src/p2p.rs`: adiciona `voice-opus-v1` (WebRTC DataChannel binario,
  nao ordenado, sem retransmissao), mantem o DataChannel de teste.
- `apps/desktop/src/main.rs`: liga/desliga microfone (desligado por padrao) e limpeza das sessoes.
- `apps/desktop/Cargo.toml`: CPAL 0.16, Opus 0.4, bytes 1.

**Transporte desta versao:** Opus sobre DataChannel WebRTC para validar o audio rapidamente.
Nao sao trilhas RTP. Uma evolucao futura deve utilizar RTP/RTCP, buffer jitter mais robusto,
controle de congestionamento, AEC/NS e selecao de dispositivos.

## Instalar dependencias de compilacao no Windows

- Rust e compilador MSVC (Visual Studio Build Tools, C++).
- CMake no PATH (usado para compilar o codec Opus estaticamente na versao escolhida).
- Microfone e alto-falantes configurados para oferecer `48.000 Hz`, formato `f32`.
  CPAL procura explicitamente essa configuracao; se nao existir, registra erro no painel.

## Atualizacao sem perder a configuracao da VPS

Para atualizar o seu projeto existente, substitua APENAS estes arquivos:

1. `apps/desktop/Cargo.toml`
2. `apps/desktop/src/main.rs`
3. `apps/desktop/src/p2p.rs`
4. Adicione `apps/desktop/src/audio.rs`

Nao substitua o `infrastructure/docker-compose.yml` da VPS. Nenhum rebuild do
container `voice-signaling` e necessario.

## Compilar no Windows

```powershell
cargo check -p voice-desktop
cargo test -p voice-desktop opus_roundtrip_mono
cargo run -p voice-desktop
```

Para distribuir sem instalar Rust no PC do destinatario:

```powershell
cargo build --release -p voice-desktop
```

Binario: `target\\release\\voice-desktop.exe`. Dependencias de runtime Windows
(MSVC) podem ser necessarias conforme o ambiente.

## Teste de ponta a ponta

1. Em dois computadores Windows em redes diferentes, abra o cliente atualizado.
2. Confirme `wss://voz.virtuabil.cloud/ws` e entre na mesma sala.
3. Aguarde o evento `Voz P2P disponivel com ...` em ambos.
4. Com fones de ouvido, clique em **Ligar microfone** em ambos.
5. Verifique se as pessoas conseguem conversar nos dois sentidos.
6. Clique em **Desligar microfone**: a captura deve parar.
7. Repita o teste com 3 participantes e depois com 5.

Se aparecer `Falha no microfone` ou `Falha na reproducao`, confira o dispositivo
padrao Windows e sua configuracao 48 kHz. Se travar ou houver eco, teste com fones.

## Limitacoes relevantes

- A versao nao esta validada em compilacao neste ambiente (nao ha Cargo instalado).
  Execute `cargo check` no Windows e envie a mensagem caso haja erro.
- Suporte inicial apenas a dispositivos que anunciem F32/48 kHz.
- Sem cancelamento de eco, supressao de ruido ou ganho automatico.
- A mistura eh simples e a fila por participante usa pre-buffer de 40ms.
- Nao eh uma versao para producao: autenticar participantes e proteger o acesso
  as salas antes de disponibiliza-la publicamente.
- O TURN pode ser selecionado pelo ICE se a conexao direta nao funcionar;
  isso nao significa que o caminho `relay` ja tenha sido testado.
