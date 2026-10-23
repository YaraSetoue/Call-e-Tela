# Diagnostico de conexao P2P e voz

Este patch nao modifica o servidor. Ele acrescenta apenas logs e contagem de pacotes Opus locais para descobrir por que os pacotes enviados/recebidos estao em zero.

## Como aplicar

Copie `apps/desktop/src/p2p.rs`, `apps/desktop/src/main.rs` e `apps/desktop/src/audio.rs` sobre os arquivos equivalentes do projeto atual. Guarde backup antes. Aplique em **ambos os computadores Windows**.

No diretorio raiz do workspace Rust:

```powershell
cargo check -p voice-desktop
cargo build --release -p voice-desktop
```

Abra o aplicativo atualizado nos dois PCs. Crie uma nova sala; entre nela pelo segundo PC; ligue os microfones. Leia a secao Eventos P2P em ambos os PCs, preferencialmente antes de abrir ou fechar novas salas. Verifique:

- `Opus local OK`: o microfone conseguiu codificar um pacote (isso NAO confirma entrega P2P).
- `Comecando oferta` / `Recebi oferta` / `Recebi resposta`: negociacao SDP.
- `ICE gathering ...`: coleta de candidatos.
- `Conexao WebRTC ...: Connected`: transporte conectado.
- `Canal P2P ABERTO`: canal pronto para enviar.
- `enviados` e `recebidos` subindo: pacotes transitaram e foram decodificados.

Se nao aparecer `Comecando oferta`, verifique se o segundo PC entrou realmente na mesma sala. Se ambos estao na sala e a oferta nao aparece, copie o log dos dois lados para investigacao. Se a oferta aparece e nao ha `Resposta`, investigue negociacao e ICE antes de alterar os dispositivos de audio.

Os eventos sao exibidos em uma janela rolavel; o limite de linhas do historico foi ampliado de 7 para 35. O patch nao configura transmissao em RTP e nao comprova retransmissao TURN. Ele e exclusivamente diagnostico para o MVP atual.
