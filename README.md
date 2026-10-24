DIAGNOSTICO ICE / TURN - WINDOWS

O que sabemos: SDP offer/answer foi negociado, mas ICE terminou em Failed. Nao ha prova de que candidatos remotos tenham sido aplicados e nenhum candidato relay apareceu nos logs existentes.

Instalacao:
1) Faca backup do p2p.rs atual.
2) Copie apps/desktop/src/p2p.rs deste pacote sobre o original.
3) Execute cargo check -p voice-desktop e cargo build --release -p voice-desktop.
4) Atualize o executavel nos dois computadores.

Teste A - modo normal (recomendado primeiro):
Abra ambos os clientes, entre na mesma sala e confira se aparecem mensagens:
- Credenciais TURN recebidas: ... URLs
- ICE local: candidato host/outro, srflx ou relay
- ICE remoto recebido de ...
- ICE remoto aplicado de ...
- Conexao WebRTC: Connected ou Failed

Teste B - forcar TURN, apenas para diagnostico (nos dois PCs, PowerShell):
$env:VOICE_FORCE_RELAY="1"
.\target\release\voice-desktop.exe

Aparecera 'MODO TESTE: ICE somente TURN/relay'. Se nao surgirem candidatos 'relay', investigar autenticacao TURN, acesso UDP/TCP 3478, faixa UDP 49160-49200, logs do Coturn e IP publico/NAT da VPS.

Para voltar ao modo normal em PowerShell:
Remove-Item Env:VOICE_FORCE_RELAY -ErrorAction SilentlyContinue

VPS, sem modificar configuracao:
docker ps --filter name=voice-coturn
docker logs --tail 100 voice-coturn
sudo ss -lunpt | grep ':3478'
sudo firewall-cmd --list-ports

IMPORTANTE: nao compartilhe senha TURN, segredo static-auth-secret nem capturas contendo credenciais. O patch exibe somente tipos de candidatos, nao enderecos IP ou senhas.

O projeto e diagnostico nao foram compilados neste ambiente, mas a alteracao foi conferida estaticamente. Se houver erro do Cargo, envie-o na conversa.
