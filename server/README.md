# nodex-ia: el servidor de la IA incluida

Es lo que permite vender Notas «con IA incluida»: la persona no necesita una clave de IA, solo un
código. La app le habla a este servidor como a cualquier servicio compatible con OpenAI; el
servidor:

- reconoce a la persona por su código (guarda solo su huella SHA-256, nunca el código);
- revisa que no haya llegado a su límite del mes (si llegó, la app muestra «Llegaste al límite de
  IA incluida de este mes; se renueva el 1 de …»);
- reenvía el pedido a la IA con **la clave del servicio** y **el modelo del servicio**;
- suma los tokens que usó cada persona, por mes, para medir el costo real.

No guarda el texto de las notas: solo pasa por el servidor y se cuentan los tokens.

## Instalar en un VPS (Ubuntu 22.04 o más nuevo)

Hace falta un VPS y un subdominio que apunte a él (por ejemplo `ia.tudominio.cl`).

1. **El programa.** Descarga `nodex-ia-servidor-linux-x64.tar.gz` del último release y déjalo en
   `/opt/nodex-ia/`:

   ```bash
   sudo mkdir -p /opt/nodex-ia && sudo tar -xzf nodex-ia-servidor-linux-x64.tar.gz -C /opt/nodex-ia
   sudo useradd --system --home /var/lib/nodex-ia --create-home nodex-ia
   ```

2. **La configuración.** Copia `deploy/nodex-ia.env` a `/etc/nodex-ia.env`, pon la clave de la IA
   (`NOTAS_IA_KEY`) y deja el archivo legible solo por root:

   ```bash
   sudo cp deploy/nodex-ia.env /etc/nodex-ia.env && sudo chmod 600 /etc/nodex-ia.env
   ```

3. **El servicio.** Copia `deploy/nodex-ia.service` a `/etc/systemd/system/` y actívalo:

   ```bash
   sudo systemctl enable --now nodex-ia
   curl http://127.0.0.1:8080/salud   # responde "ok"
   ```

4. **HTTPS.** Instala [Caddy](https://caddyserver.com/docs/install) y usa `deploy/Caddyfile`
   (cambia el dominio). Caddy saca el certificado solo.

5. **Las personas.** Crea una por cada suscriptor; el código se muestra una sola vez:

   ```bash
   sudo -u nodex-ia bash -c 'set -a; . /etc/nodex-ia.env; /opt/nodex-ia/nodex-ia nuevo "Ana Pérez"'
   ```

   Un límite propio (en tokens al mes) va después del nombre: `nuevo "Ana Pérez" 5000000`.

6. **En la app:** Configuración → Inteligencia artificial → Proveedor «IA incluida de Notas»,
   pega el código y la dirección (`https://ia.tudominio.cl`).

## Día a día

```bash
nodex-ia lista                  # uso del mes de cada persona y costo estimado en USD
nodex-ia desactivar 3fa9c1      # deja sin IA a una persona (las primeras letras de su huella)
```

(con las mismas variables de entorno que el servicio, como en el paso 5).

Los datos están en `NOTAS_IA_DATA`: `usuarios.json` (personas) y `uso/AAAA-MM.json` (uso de cada
mes). Conviene respaldar esa carpeta.

## Configuración

| Variable | Qué es | Por defecto |
|---|---|---|
| `NOTAS_IA_KEY` | Clave de la IA del servicio (obligatoria) | — |
| `NOTAS_IA_UPSTREAM` | Dirección de la IA (compatible con OpenAI) | `https://opencode.ai/zen/v1/` |
| `NOTAS_IA_MODEL` | Modelo que se usa para todos | `deepseek-v4.1-flash` |
| `NOTAS_IA_LIMITE` | Tokens al mes por persona | `3000000` |
| `NOTAS_IA_PRECIO_ENTRADA` / `_SALIDA` | USD por millón de tokens, para estimar el costo | `0.3` / `1.2` |
| `NOTAS_IA_DATA` | Carpeta de datos | `datos` |
| `NOTAS_IA_DIRECCION` / `NOTAS_IA_PUERTO` | Dónde escucha (detrás de Caddy) | `127.0.0.1` / `8080` |

## Compilar

```bash
cargo build --release -p nodex-ia     # desde la raíz del repositorio
cargo test -p nodex-ia
```
