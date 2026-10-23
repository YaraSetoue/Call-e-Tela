#!/bin/sh
set -eu
if [ "$#" -ne 1 ]; then
  echo "Uso: sh scripts/configure-turn.sh turn.seudominio.com" >&2
  exit 1
fi
DOMAIN="$1"
case "$DOMAIN" in
  *[!a-zA-Z0-9.-]*|.*|*..*|*.|"") echo "Nome de dominio invalido" >&2; exit 1 ;;
esac
command -v openssl >/dev/null || { echo "Instale openssl" >&2; exit 1; }
FILE="infrastructure/coturn/turnserver.conf"
if [ -e "$FILE" ]; then
  echo "$FILE ja existe: configuracao preservada" >&2
  exit 1
fi
SECRET=$(openssl rand -hex 32)
umask 077
cat > "$FILE" <<CONF
# Gerado localmente. NUNCA comitar este arquivo.
listening-port=3478
realm=$DOMAIN
fingerprint
use-auth-secret
static-auth-secret=$SECRET
stale-nonce
no-multicast-peers
no-loopback-peers
no-cli
# TLS sera adicionado quando certificados TURN estiverem configurados.
no-tls
no-dtls
min-port=49160
max-port=49200
log-file=stdout
CONF
echo "Configuracao TURN gerada em $FILE (segredo nao exibido)."
echo "IMPORTANTE: o endpoint de credenciais TURN temporarias ainda sera implementado na etapa 2."
echo "Antes de ativar TURN, valide IP publico, NAT/firewall UDP 3478 e 49160-49200 e TLS futuro."
