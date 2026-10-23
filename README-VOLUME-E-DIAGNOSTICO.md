# Etapa: selecao de saida, volumes individuais e diagnostico de audio

Base: a versao de audio corrigida em 23/10/2026 (Rust + Slint + Opus por DataChannel WebRTC).

## Atualizacoes

- Selecionar o dispositivo de saida no Windows (`Padrao do Windows` ou dispositivo especifico) sem abandonar a sala.
- Botao **Atualizar** para listar novamente os dispositivos de saida.
- Controle de volume geral (0 a 200%).
- Controles independentes de volume para cada participante remoto (0 a 200%; 0 = mudo).
- Troca de saida usando thread dedicada do CPAL. Se a nova saida falhar, conserva a anterior e informa o erro no log.
- Indicadores cumulativos de pacotes de voz Opus enviados e recebidos por participante.
- Selecao de microfone e teste do alto-falante mantidos.

Os volumes sao processados localmente no receptor. Nao modificam o sinal transmitido aos outros participantes.
O som de teste tambem passa pelo volume geral.

## Como aplicar

1. Faca um backup dos arquivos existentes do aplicativo Windows.
2. Extraia o pacote **patch** na raiz do seu projeto, preservando as pastas `apps/desktop/src`.
3. Sobrescreva **apenas** `apps/desktop/src/audio.rs`, `apps/desktop/src/main.rs` e `apps/desktop/src/p2p.rs`.
4. Na raiz do workspace, execute:

```powershell
cargo check -p voice-desktop
cargo run -p voice-desktop
```

Depois, para gerar o executavel:

```powershell
cargo build --release -p voice-desktop
```

O executavel fica em `target\release\voice-desktop.exe`.

**Nao e necessario atualizar o servidor Rust da VPS, Coturn ou Nginx.**

## Teste de diagnostico

Usar **fones de ouvido** e abrir a mesma sala em dois computadores em redes diferentes.

1. Escolha o microfone, ligue-o e fale: o indicador `Microfone: sinal capturado` deve reagir.
2. Selecione a saida correta (fones ou caixa de som). Clique em **Testar saida** e confirme o som.
3. Na secao **Pacotes de audio por participante**, veja `enviados` e `recebidos` em ambos os computadores.
4. O canal deve informar `Voz P2P disponivel com ...` nos eventos P2P.
5. Deixe volume geral e volume da outra pessoa em 100%, depois ajuste conforme necessario.

| Resultado | Interpretacao |
|---|---|
| `enviados = 0`, apesar de ter sinal no microfone | Falta canal de voz pronto ou falha no fluxo codificador/broadcast para o DataChannel. Verificar `Voz P2P disponivel` e erros nos eventos. |
| `enviados > 0`, `recebidos = 0` no outro computador | A voz nao chegou pelo DataChannel. Investigar canal WebRTC, ICE e erros `Falha enviando voz`. |
| `recebidos > 0` e teste local funciona, mas voz continua inaudivel | Conferir saida selecionada, volumes (ambos em 100%), mensagens Opus, e buffer de reproducao. |
| `recebidos > 0` mas o som entrecorta | Investigar jitter, perda de quadros, codec, saturacao do processador e conexao P2P. |

**Importante:** este e um prototipo para teste. O transporte Opus atual usa DataChannel SCTP sem retransmissao. Para um produto final, preferir trilhas RTP/RTCP WebRTC e tratar buffer de jitter, perda, cancelamento de eco e adaptacao de taxa.

## Problemas conhecidos

- Usa 48 kHz: se um dispositivo nao anunciar esse formato, a abertura pode falhar. O erro aparecera em `Eventos P2P`.
- Os indices de dispositivos podem mudar quando dispositivos USB sao conectados/desconectados. Use **Atualizar** e selecione novamente.
- Elevar volume acima de 100% pode distorcer o audio quando varias pessoas falam ao mesmo tempo.
- Ainda nao existe cancelamento de eco. Use fones para os primeiros testes.
- A compilacao Rust nao foi executada no ambiente de geracao; use `cargo check` no Windows para validar APIs e dependencias locais.
