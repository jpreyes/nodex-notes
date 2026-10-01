#!/usr/bin/env bash
# Actualiza el servidor de la IA incluida (nodex-ia) a la última versión.
#
# - Trae lo último del repositorio (git pull) en REPO_DIR.
# - En x86_64 baja el programa del último release de GitHub; en otros procesadores (ARM) lo
#   compila desde el repositorio (hace falta Rust).
# - Si cambió, lo reemplaza, reinicia el servicio y revisa /salud. Si no responde, vuelve al
#   anterior.
#
# Uso (como root):  REPO_DIR=/opt/nodex-ia/repo /opt/nodex-ia/actualizar.sh
# Lo corre solo, una vez al día, nodex-ia-actualizar.timer.
set -euo pipefail

REPO_DIR="${REPO_DIR:-/opt/nodex-ia/repo}"
BIN=/opt/nodex-ia/nodex-ia
URL="https://github.com/jpreyes/nodex-notes/releases/latest/download/nodex-ia-servidor-linux-x64.tar.gz"
PORT="$(grep -E '^NOTAS_IA_PUERTO=' /etc/nodex-ia.env 2>/dev/null | cut -d= -f2 || true)"
PORT="${PORT:-8080}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

log() { echo "[nodex-ia-actualizar] $*"; }

if [ -d "$REPO_DIR/.git" ]; then
  git -C "$REPO_DIR" pull --ff-only --quiet && log "repositorio al día ($(git -C "$REPO_DIR" log -1 --format=%h))"
fi

if [ "$(uname -m)" = "x86_64" ]; then
  curl -fsSL "$URL" -o "$TMP/nodex-ia.tar.gz"
  tar -xzf "$TMP/nodex-ia.tar.gz" -C "$TMP"
else
  # Sin programa publicado para este procesador: se compila.
  [ -d "$REPO_DIR/.git" ] || { log "falta el repositorio en $REPO_DIR para compilar"; exit 1; }
  export PATH="$HOME/.cargo/bin:/root/.cargo/bin:$PATH"
  (cd "$REPO_DIR" && cargo build --release --locked -p nodex-ia --quiet)
  cp "$REPO_DIR/target/release/nodex-ia" "$TMP/nodex-ia"
fi

if [ -f "$BIN" ] && cmp -s "$TMP/nodex-ia" "$BIN"; then
  log "ya estaba en la última versión"
  exit 0
fi

# Reemplazar (el anterior queda como respaldo hasta comprobar que la nueva responde).
[ -f "$BIN" ] && cp -p "$BIN" "$BIN.anterior"
install -m 755 "$TMP/nodex-ia" "$BIN.nuevo"
mv -f "$BIN.nuevo" "$BIN"
systemctl restart nodex-ia

for _ in $(seq 1 20); do
  if curl -fsS "http://127.0.0.1:$PORT/salud" >/dev/null 2>&1; then
    log "actualizado y respondiendo"
    rm -f "$BIN.anterior"
    exit 0
  fi
  sleep 1
done

log "la versión nueva no responde: se vuelve a la anterior"
if [ -f "$BIN.anterior" ]; then
  mv -f "$BIN.anterior" "$BIN"
  systemctl restart nodex-ia
fi
exit 1
