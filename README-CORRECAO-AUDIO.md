# Ajuste de voz: selecao de microfone e diagnostico de audio (Windows)

Esta atualizacao altera apenas o cliente desktop. **Nao atualizar a VPS.**

## Arquivos alterados

- `apps/desktop/src/audio.rs`: enumera microfones, permite troca durante a chamada, aceita F32/I16/U16 a 48000 Hz, mostra nivel de entrada, toca teste local e explicita erros de captura/reproducao.
- `apps/desktop/src/main.rs`: seletor de microfone, botao "Atualizar", teste de saida e indicador de nivel. Usa `watch` para atualizar o microfone enquanto participa da sala.
- `apps/desktop/src/p2p.rs`: conta pacotes Opus de entrada e saida e mostra a primeira falha de `try_send` em vez de ignora-la silenciosamente.

**O teste de audio nao substitui a avaliacao em dois PCs.** O transporte ainda e um DataChannel WebRTC P2P, e nao uma trilha RTP.

## Instalar o patch

Copie os tres arquivos Rust para as mesmas pastas no projeto que ja funciona. Os demais arquivos da VPS, incluindo `docker-compose.yml`, `coturn` e `nginx`, devem permanecer inalterados.

```powershell
cargo check -p voice-desktop
cargo run -p voice-desktop
# Para gerar EXE otimizado:
cargo build --release -p voice-desktop
```

Esta versao exige a mesma toolchain da etapa anterior (Rust, MSVC e CMake para Opus).

## Diagnostico rapido (fones de ouvido)

1. Abra o programa e escolha seu microfone na lista. Use "Atualizar" se conectar um novo microfone USB.
2. Crie/entre em uma sala; clique em "Testar saida". Deve ouvir um tom curto. O teste utiliza o dispositivo de saida padrao do Windows.
3. Clique em "Ligar microfone", fale e observe o texto "Microfone: sinal capturado" com nivel variando.
4. Com outra pessoa na sala, aguarde "Voz P2P disponivel". Verifique "Opus enviado" no computador A e "Opus recebido" no computador B (e vice-versa).
5. Se o tom funciona e ambos recebem Opus mas nao ha voz audivel, guarde os eventos e verifique a configuracao/volume do Windows, bem como os logs de decodificacao.

### Interpretacao

- **Falha ao abrir microfone**: dispositivo indisponivel, permissao do Windows ou formato nao suportado.
- **Nivel zero / muito baixo**: nao ha sinal util capturado. Verifique permissao, mute fisico e dispositivo escolhido.
- **Nivel sobe mas nao aparece Opus enviado**: canal de voz P2P nao abriu, ou envio foi rejeitado. Veja log "Voz P2P disponivel" e "Falha enviando voz".
- **Opus enviado mas nao recebido**: problema na transmissao P2P/DataChannel.
- **Opus recebido e nao sai som**: verifique "Falha ao abrir alto-falante" e se o teste de saida toca.

## Limitacoes

- So pode usar dispositivos que suportem F32/I16/U16 a 48 kHz. Se a interface de audio usar 44,1 kHz, ainda precisaremos implementar resampling; nao basta alterar a taxa do Opus.
- A selecao usa a posicao da lista enumerada no inicio/apos atualizar; a ordem pode mudar ao conectar/desconectar dispositivos.
- Nao ha cancelamento de eco, controle automatico de ganho nem jitter buffer adaptativo.
- O teste de saida esta disponivel quando o cliente esta conectado a uma sala.
- Os arquivos foram revisados estaticamente neste ambiente; o `cargo check` deve ser executado no Windows.
