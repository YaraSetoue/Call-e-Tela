# Etapa 3.1 - Ativar TURN no cliente Windows

## O que muda

- O servidor Rust passa a gerar credenciais TURN por participante, validas por 1 hora, usando HMAC-SHA1 e o mesmo `static-auth-secret` do Coturn.
- A chave secreta fica apenas na VPS. O script `scripts/prepare-turn-secret.sh` extrai a mesma chave configurada no Coturn para um arquivo dedicado, com permissao `0400`, propriedade do UID 10001 do container Rust.
- O protocolo agora envia `TurnCredentials` ANTES de `RoomCreated` ou `RoomJoined`.
- O cliente inclui STUN e TURN ao criar cada `PeerConnection`; continua usando conexao direta quando viavel.
- Mudancas em arquivos: `apps/signaling-server/{Cargo.toml,src/main.rs}`, `crates/protocol/src/lib.rs`, `apps/desktop/src/{main.rs,p2p.rs}` e um exemplo em `infrastructure/docker-compose.yml`.

## IMPORTANTE: cuidado ao atualizar a VPS

**Nao substitua seu docker-compose.yml por este exemplo**, porque voce ja personalizou as redes Docker que conectam Nginx e Rust. Em vez disso, mescle apenas as linhas abaixo no servico `signaling` do seu Compose atual:

```yaml
    environment:
      BIND_ADDR: "0.0.0.0:8080"
      RUST_LOG: "signaling_server=info"
      TURN_SECRET_FILE: "/run/secrets/turn_secret"
      TURN_HOST: "voz.virtuabil.cloud"
    volumes:
      - ./coturn/signaling-turn-secret:/run/secrets/turn_secret:ro
```

Preserve todos os demais campos atuais, especialmente `networks`, `ports` e `security_opt`.

Antes de iniciar o servidor, prepare o segredo privado. Execute na raiz do workspace Rust na VPS:

```bash
sudo sh scripts/prepare-turn-secret.sh
sudo ls -l infrastructure/coturn/signaling-turn-secret
```

A segunda linha deve mostrar o arquivo com permissao `-r--------` e proprietario numerico `10001`. **Nao use `chmod 644` neste segredo.** Nao envie esse arquivo para o Windows, nao o publique em Git e nao o inclua no ZIP.

## Implantacao

1. Faca backup dos arquivos funcionais e copie os arquivos alterados do codigo Rust para a VPS e o Windows.
2. Mescle os ajustes citados no Compose `signaling` atual.
3. Gere o arquivo privado: `sudo sh scripts/prepare-turn-secret.sh`.
4. Na VPS: `docker compose -f infrastructure/docker-compose.yml config` (nao exibe o segredo armazenado dentro do volume).
5. Compile e recrie somente o servidor: `docker compose -f infrastructure/docker-compose.yml up -d --build signaling`.
6. Confira: `docker logs --tail 80 voice-signaling`; a mensagem esperada e `Credenciais TURN temporarias habilitadas`.
7. Verifique se o Coturn esta ativo: `docker ps --filter name=voice-coturn`.
8. No Windows: `cargo test -p voice-protocol`, `cargo check -p voice-desktop`, `cargo run -p voice-desktop`.
9. Em duas redes diferentes, entre na sala, confira `STUN/TURN configurados` antes da negociacao P2P e teste a abertura do DataChannel.

## O que este patch nao implementa

- **Ainda nao ha transmissao de voz**. A proxima etapa integra CPAL, Opus e audio via trilhas WebRTC/RTP.
- Nao ha ICE restart/renovacao da sessao apos expirar a credencial. Novas salas recebem uma credencial nova; chamadas prolongadas e reconexoes precisarao de renovacao.
- Nao ha autenticacao de usuarios, apenas conhecimento do codigo de sala. Antes de uso publico, adicionar limite de conexoes/solicitacoes e protecao de abuso TURN.
- O dominio TURN via UDP/TCP porta 3478 nao utiliza TLS. Isso nao significa que o audio WebRTC fique sem criptografia, mas TURN/TLS devera ser avaliado para redes restritivas.
- **Nao foi possivel executar Cargo neste ambiente**: projeto precisa ser compilado e validado no Windows e na VPS.

## Teste efetivo do TURN (nao apenas do P2P)

Uma conexao direta pode funcionar mesmo com TURN quebrado. Para afirmar que o relay esta funcionando, inspecione as estatisticas ICE do cliente e confirme que o par selecionado inclui um candidato tipo `relay`, ou execute um teste separado com politica `relay-only`. Esse ultimo teste sera incorporado no cliente de diagnostico da proxima etapa.
