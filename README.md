# Voice Screen — Etapa 1

Aplicativo desktop Windows em Rust + Slint e servidor de sinalizacao Rust/Axum, para evoluir para ligacoes de voz e compartilhamento de tela em salas P2P Mesh (limite inicial de 5 participantes).

**Status deste pacote:** implementa interface Windows, salas efemeras, presenca e encaminhamento de mensagens de negociacao SDP/ICE, mas **ainda nao implementa WebRTC, captura de audio/tela, codecs nem TURN funcional no cliente**.

## Estrutura

- `apps/desktop`: UI Slint e cliente WebSocket para criar/entrar em salas.
- `apps/signaling-server`: API `/health` e WebSocket `/ws`.
- `crates/protocol`: comandos e eventos JSON entre cliente e servidor.
- `infrastructure`: Docker, exemplo Nginx e Coturn opcional.
- `scripts/smoke_test.py`: teste E2E de WebSocket com biblioteca padrao Python.

## 1. Teste local

Instale Rust/Cargo (stable atual) e Python 3. No terminal:

```sh
cargo test --workspace
cargo run -p signaling-server
```

Em outro terminal, para validar o servidor:

```sh
python scripts/smoke_test.py ws://127.0.0.1:8080/ws
```

No **Windows**, execute a UI:

```powershell
cargo run -p voice-desktop
```

Abra o programa duas vezes. Na primeira janela, clique em `Criar sala`; copie o codigo exibido. Na segunda, coloque o codigo e clique em `Entrar na sala`.

## 2. VPS CloudLinux com Docker

1. Verifique a versao: `cat /etc/os-release`; Docker Engine em CloudLinux OS 8+ e documentado pelo fornecedor.
2. Instale Docker + Docker Compose conforme orientacoes do fornecedor.
3. Extraia o repositorio na VPS e execute na raiz:

```sh
docker compose -f infrastructure/docker-compose.yml up -d --build signaling
docker compose -f infrastructure/docker-compose.yml logs -f signaling
curl http://127.0.0.1:8080/health
```

A porta 8080 e publicada **somente no loopback da VPS**, nao externamente. Configure HTTPS e WSS pelo seu Nginx existente. Exemplo de `location` em `infrastructure/nginx/signaling.conf.example`.

No Windows, substitua a URL inicial da interface por `wss://SEU_DOMINIO/ws` e teste duas maquinas em redes diferentes.

### Exemplo Nginx

Integre `infrastructure/nginx/signaling.conf.example` dentro de um `server { listen 443 ssl; ... }` ja configurado com certificado publico valido. Para WSS, e indispensavel encaminhar os cabecalhos `Upgrade` e `Connection`.

## 3. TURN (apenas para a proxima etapa)

No Linux, a partir da raiz do repositorio:

```sh
sh scripts/configure-turn.sh turn.seudominio.com
docker compose -f infrastructure/docker-compose.yml --profile turn up -d coturn
```

O script gera um arquivo local ignorado pelo Git com senha aleatoria forte. **Nao ative TURN publico antes de conferir firewall, endereco IP externo/NAT, quotas e geracao de credenciais temporarias pelo servidor.** O endpoint que entrega credenciais TURN aos clientes sera implementado na etapa WebRTC. A configuracao inicial desabilita TURN/TLS; redes restritivas podem exigir TLS pela porta 5349 depois.

Se utilizar TURN em uma VPS, os fluxos retransmitidos consomem banda. Liberacoes normalmente necessarias: 3478 UDP/TCP e faixa UDP 49160-49200, alem de 443 TCP para WSS. O Coturn utiliza `network_mode: host` no Linux para evitar publicar grande faixa UDP no Docker.

## 4. Protocolo de salas

O cliente envia:

```json
{"type":"create_room","name":"Alice"}
{"type":"join_room","room_id":"CODIGO_DA_SALA","name":"Bob"}
{"type":"signal","to":"ID_DO_PEER","payload":{"kind":"offer","sdp":"..."}}
```

O servidor responde com `room_created`, `room_joined`, `peer_joined`, `peer_left` e `signal`. O cliente nao consegue falsificar o campo `from`; ele e definido pelo servidor. O encaminhamento se limita aos peers na mesma sala.

As salas existem em memoria e desaparecem quando o ultimo participante desconecta ou o servidor reinicia. O codigo da sala utiliza UUID v4 (122 bits de aleatoriedade) e funciona como segredo de convite; **isto nao substitui autenticacao real para uso publico**.

## 5. Limites e tarefas de engenharia

- Sem banco de dados, login, persistencia ou moderacao por enquanto.
- O limite de 5 participantes e imposto no servidor, mas ainda nao representa garantia de desempenho multimidia.
- Falta rate limiting por IP, limites de conexoes, heartbeat/timeout de sockets mortos, tratamento forte de clientes lentos, observabilidade, testes de carga, e autorizacao de quem pode transmitir.
- WebSocket aqui transporta **apenas mensagens de controle**, nunca o audio ou o video.
- Proxima etapa: WebRTC com negociacao peer-to-peer, ICE/STUN, credenciais TURN efemeras e testes entre redes diferentes.
- Depois: CPAL/Opus e captura Windows + H.264 + Slint com renderizacao otimizada.
- Autenticacao de participantes, controle de compartilhamento, limites por cliente e protecao antiabuso sao necessarios antes de colocar um servidor TURN aberto na internet.

## 6. Validacao

A maquina onde este pacote foi preparado nao inclui Cargo/Rust nem Docker; portanto o **codigo nao foi compilado neste ambiente**. Os arquivos foram organizados para compilacao e testes na sua maquina Windows/VPS, com comandos acima. Confira erros de compilacao e ajuste as versoes caso o ecossistema mude.
