#!/usr/bin/env bash
# Respaldo diario de los datos del servidor (cuentas, códigos, uso, configuración cifrada).
# Deja /var/backups/nodex-ia/AAAA-MM-DD.tar.gz y borra los de más de 30 días.
set -euo pipefail
DATA="$(grep -E '^NOTAS_IA_DATA=' /etc/nodex-ia.env 2>/dev/null | cut -d= -f2 || true)"
DATA="${DATA:-/var/lib/nodex-ia}"
OUT=/var/backups/nodex-ia
mkdir -p "$OUT"
chmod 700 "$OUT"
tar -czf "$OUT/$(date +%F).tar.gz" -C "$(dirname "$DATA")" "$(basename "$DATA")"
find "$OUT" -name '*.tar.gz' -mtime +30 -delete
echo "[nodex-ia-respaldo] $OUT/$(date +%F).tar.gz"
