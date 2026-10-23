# Correção de diagnóstico de voz: enviados=0 / recebidos=0

## Objetivo
A base atual abre dois DataChannels (`voice-screen-test` para teste, `voice-opus-v1` para voz). Mesmo quando o primeiro funciona, o segundo pode não ter entrado em estado `OnOpen`; o contador `enviados` permanece em 0. Este patch usa **o canal de teste já validado** também para Opus. É um teste diagnóstico: **não é a arquitetura final de áudio de produção**.

## Aplicar
1. Faça backup de `apps/desktop/src/p2p.rs` do seu projeto.
2. Substitua apenas esse arquivo pelo `apps/desktop/src/p2p.rs` deste ZIP.
3. Use a mesma versão atual do resto do projeto (com microfone, seleção de saída e volumes).
4. Na raiz do workspace, rode:

```powershell
cargo check -p voice-desktop
cargo build --release -p voice-desktop
```

5. Atualize **ambos** os computadores com o novo executável e crie uma **nova sala**.
6. Selecione o microfone e ligue-o nos dois computadores. Use fones de ouvido.
7. Observe os logs de Eventos P2P e os contadores de áudio.

## Mensagens esperadas
- `Canal P2P criado ... aguardando OnOpen`: a conexão ainda não abriu.
- `Canal P2P ABERTO ... (teste e voz)`: o mesmo DataChannel agora pode levar mensagens de teste e Opus.
- `Mensagem P2P ...`: o texto de teste chegou entre os clientes.
- `enviados` e `recebidos` aumentando: quadros Opus saíram e foram decodificados.
- `Canal P2P aberto ... mas ainda sem Opus enviado`: houve abertura de canal, mas não houve dados de Opus por pelo menos 5 segundos; confirme microfone ligado, se o pico do microfone aumenta e se há falhas de codificação Opus nos eventos.
- `Falha enviando Opus ...`: erro ao encaminhar áudio pelo WebRTC.

## Observações
- O modo confiável padrão do canal serve apenas para este teste. Pode ter maior latência sob perda de pacotes; antes da produção migraremos voz para RTP/RTCP e implementaremos tratamento de perda/jitter e cancelamento de eco.
- Não é necessário atualizar o servidor da VPS, Coturn ou Nginx.
- Não foi possível executar `cargo check` neste ambiente (toolchain Rust indisponível); valide a compilação no Windows.
- Se os contadores permanecerem em zero, copie as mensagens em Eventos P2P que contenham `Canal`, `P2P` ou `Opus` para a análise seguinte.
