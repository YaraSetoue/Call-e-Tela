#!/bin/sh
# Executar como root na raiz do workspace (sudo sh scripts/prepare-turn-secret.sh)
set -eu
SOURCE="infrastructure/coturn/turnserver.conf"
TARGET="infrastructure/coturn/signaling-turn-secret"
[ "$(id -u)" -eq 0 ] || { echo "Execute com sudo" >&2; exit 1; }
[ -f "$SOURCE" ] || { echo "Coturn nao configurado: $SOURCE" >&2; exit 1; }
SECRET=$(sed -n 's/^[[:space:]]*static-auth-secret=//p' "$SOURCE" | head -n 1)
[ -n "$SECRET" ] || { echo "static-auth-secret ausente" >&2; exit 1; }
umask 077
# O sinalizador UID/GID 10001 foi definido no Dockerfile.server
printf '%s\n' "$SECRET" > "$TARGET"
chown 10001:10001 "$TARGET"
chmod 0400 "$TARGET"
echo "Chave TURN privada preparada para o container Rust (UID 10001)."
