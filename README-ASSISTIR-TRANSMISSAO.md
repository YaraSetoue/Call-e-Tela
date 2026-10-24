# Voice Screen - Entrar e sair da transmissao (estilo Discord)

Esta etapa usa a base com voz Opus, tela H.264/JPEG 720p/15 FPS e audio da tela WASAPI.

## O que muda

- O apresentador inicia a propria transmissao como antes.
- Os outros participantes enxergam a transmissao disponivel na aba **Chamada** e **Tela**.
- Cada espectador clica **Assistir** ou **Assistir transmissao** para ver a tela e ouvir o som do computador do apresentador.
- O espectador clica **Sair da transmissao (continuar na chamada)** para interromper imagem e som da tela. A voz do microfone da sala permanece conectada.
- Novos participantes descobrem transmissoes ja ativas, mesmo se entrarem depois de o apresentador iniciar.
- Quando o apresentador encerra ou se desconecta, a transmissao sai da lista e a visualizacao e encerrada.
- Um participante assiste a **uma transmissao por vez**, mas transmissoes de pessoas diferentes podem estar simultaneamente disponiveis.
- **Nao faz streaming da tela para quem nao clicou Assistir**: os quadros H.264/JPEG e os pacotes Opus da tela sao enviados apenas aos inscritos, reduzindo o trafego P2P.

O microfone permanece em seu canal habitual, sem controle de entrada/saida. O sistema continua capturando/encodificando localmente a tela enquanto o apresentador a compartilha, mesmo sem espectadores; o que e evitado e o envio remoto a nao espectadores.

## Arquivos

Copie estes arquivos do ZIP substituindo os existentes:

- `apps/desktop/src/main.rs`
- `apps/desktop/src/p2p.rs`

**NAO substitua** `screen.rs`, `screen_audio.rs`, `audio.rs` ou `Cargo.toml`. Preserve sua ultima correcao do OpenH264 (`skip_frames(true)`) e do WASAPI (`initialize_mta().ok()`).

## Compilacao no Windows (na raiz do workspace)

```powershell
cargo check -p voice-desktop
cargo build --release -p voice-desktop
.\target\release\voice-desktop.exe
```

Instale **a mesma versao nos dois/tres computadores**. A versao anterior nao conhece os comandos VSC1 e nao suporta a inscricao por espectador.

Nenhuma alteracao no servidor de sinalizacao, no Coturn ou no Nginx e necessaria.

## Como testar (2 ou 3 computadores)

1. Todos entram na mesma sala e confirmam voz.
2. Computador A inicia **Compartilhar tela**, com **Som do computador** ativado.
3. Computador B: aparece **A esta ao vivo**. Sem clicar, imagem e audio da tela nao devem chegar; a voz continua.
4. Computador B clica **Assistir transmissao**. Deve ver imagem H.264 720p/15 FPS e ouvir o som do computador A.
5. Computador B clica **Sair da transmissao**. A imagem para, o som da tela para, mas a voz continua.
6. Computador B clica **Assistir transmissao** novamente; a imagem H.264 pode demorar ate o proximo keyframe (~2 segundos).
7. Computador C entra na sala depois de A ter iniciado; a transmissao de A deve aparecer na lista.
8. A para a propria transmissao; B e C devem ver a transmissao removida da lista.
9. Opcional: A e C transmitem; B escolhe qual assistir, uma por vez.

## Desenho tecnico

- O DataChannel de voz `voice-screen-test` carrega comandos UTF-8 de controle (protocolo VSC1):
  - `VSC1:LIVE` / `VSC1:OFF`: anuncio do apresentador aos peers, inclusive quem entrou depois.
  - `VSC1:WATCH` / `VSC1:UNWATCH`: inscricao do espectador para aquele peer.
- Cada conexao P2P tem um flag atomico de espectador. Os DataChannels de video e som da tela **so transmitem** quando esse flag esta ativo.
- O receptor so decodifica os quadros de video e audio do peer selecionado. A saida do som da tela e removida ao sair da transmissao.
- A voz usa o fluxo anterior independente e continua funcionando.

## Observacoes

- A captura local e preview permanecem ativos no apresentador mesmo sem espectadores.
- A interface e implementacao ainda sao MVP: nao ha contagem de espectadores, tela cheia, adaptacao de bitrate, nem controle por espectador de resolucao e FPS.
- Baseado na ultima distribuicao H.264 + ajuste de avisos descrita nesta conversa, nao em uma copia Git atualizada do seu computador. Se existirem mudancas locais adicionais em `main.rs` ou `p2p.rs`, compare antes de sobrescrever.
- Nao foi possivel executar `cargo check` neste ambiente (Rust/Cargo nao estao instalados); primeiro valide no Windows. O ZIP passou por verificacoes estaticas e de integridade.
