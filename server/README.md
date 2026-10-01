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

Hay dos formas de tener IA incluida:

- **Cuentas** (lo normal): en la app, Configuración → Tu cuenta, se entra con el correo (llega un
  código de 6 dígitos) o con Microsoft. Una cuenta nueva trae una prueba de 14 días; el plan se
  cambia con `nodex-ia plan`. Con la cuenta también viaja la configuración de la persona
  (calendarios, correo, To Do…), **cifrada en la app** con una clave que el servidor no tiene.
- **Códigos** entregados a mano (`nodex-ia nuevo`), para pruebas o casos especiales.

## Instalar en un VPS (Ubuntu 22.04 o más nuevo)

**Con Claude Code en el VPS:** crea una carpeta, deja ahí [`deploy/CLAUDE.md`](deploy/CLAUDE.md)
y pídele a Claude que instale el servidor: tiene los pasos (con Cloudflare Tunnel), lo que no debe
tocar, la actualización diaria desde GitHub y el respaldo.

```bash
mkdir -p ~/notas-vps && cd ~/notas-vps
curl -fsSLO https://raw.githubusercontent.com/jpreyes/nodex-notes/main/server/deploy/CLAUDE.md
claude    # y escribe: «instala el servidor siguiendo CLAUDE.md»
```

A mano: hace falta un VPS y un subdominio que apunte a él (por ejemplo `ia.tudominio.cl`).

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

4. **HTTPS.** Con **Cloudflare Tunnel** (`cloudflared`): una regla `hostname: ia.tudominio.cl` →
   `service: http://localhost:8080` (ver `deploy/CLAUDE.md`, paso 5); no hace falta abrir puertos.
   O con [Caddy](https://caddyserver.com/docs/install) y `deploy/Caddyfile` (cambia el dominio).

   **Que se actualice solo:** `deploy/actualizar.sh` (git pull + último release, reinicia y revisa
   `/salud`; si falla, vuelve al anterior) con `nodex-ia-actualizar.timer` (una vez al día), y
   `nodex-ia-respaldo.timer` para un respaldo diario de los datos en `/var/backups/nodex-ia/`.

5. **El correo de los códigos.** Crea una cuenta en [Resend](https://resend.com), verifica tu
   dominio y pon su clave en `NOTAS_IA_CORREO_CLAVE` y el remitente en `NOTAS_IA_CORREO_DE`
   (por ejemplo `Notas <hola@tudominio.cl>`). Sin clave, el código queda en el registro del
   servicio (`journalctl -u nodex-ia`), útil para probar.

6. **Códigos a mano (opcional).** El código se muestra una sola vez:

   ```bash
   sudo -u nodex-ia bash -c 'set -a; . /etc/nodex-ia.env; /opt/nodex-ia/nodex-ia nuevo "Ana Pérez"'
   ```

   Un límite propio (en tokens al mes) va después del nombre: `nuevo "Ana Pérez" 5000000`.

7. **En la app:** Configuración → Tu cuenta (o, con un código, Inteligencia artificial → «IA
   incluida de Notas»), con la dirección `https://ia.tudominio.cl`. Si la app se compila con
   `NODEX_SERVIDOR_IA=https://ia.tudominio.cl`, esa dirección viene puesta y no hay que escribirla,
   y el asistente de la primera vez ofrece crear la cuenta. Para los instaladores del release,
   crea en GitHub la variable del repositorio `NODEX_SERVIDOR_IA` (Settings → Secrets and
   variables → Actions → Variables).

## Día a día

```bash
nodex-ia lista                  # uso del mes de cada persona y costo estimado en USD
nodex-ia desactivar 3fa9c1      # deja sin IA a una persona (las primeras letras de su huella)
nodex-ia plan ana@correo.cl pro # plan de una cuenta: prueba [días], pro, fundador o gratis
```

(con las mismas variables de entorno que el servicio, como en el paso 6).

Los datos están en `NOTAS_IA_DATA`: `usuarios.json` (códigos), `cuentas.json` (cuentas y
sesiones), `config/` (la configuración cifrada de cada cuenta) y `uso/AAAA-MM.json` (uso de cada
mes). Conviene respaldar esa carpeta.

## Configuración

| Variable | Qué es | Por defecto |
|---|---|---|
| `NOTAS_IA_KEY` | Clave de la IA del servicio (obligatoria) | — |
| `NOTAS_IA_UPSTREAM` | Dirección de la IA (compatible con OpenAI) | `https://opencode.ai/zen/v1/` |
| `NOTAS_IA_MODEL` | Modelo que se usa para todos | `deepseek-v4.1-flash` |
| `NOTAS_IA_LIMITE` | Tokens al mes por persona (planes pro y fundador) | `3000000` |
| `NOTAS_IA_PRUEBA_DIAS` / `NOTAS_IA_LIMITE_PRUEBA` | Prueba de las cuentas nuevas: días y tokens al mes | `14` / `1000000` |
| `NOTAS_IA_CORREO_CLAVE` / `NOTAS_IA_CORREO_DE` | Envío de los códigos (Resend) | — |
| `NOTAS_IA_CORREO_API` | API de correo (compatible con Resend) | `https://api.resend.com/emails` |
| `NOTAS_IA_PRECIO_ENTRADA` / `_SALIDA` | USD por millón de tokens, para estimar el costo | `0.3` / `1.2` |
| `NOTAS_IA_DATA` | Carpeta de datos | `datos` |
| `NOTAS_IA_DIRECCION` / `NOTAS_IA_PUERTO` | Dónde escucha (detrás de Caddy) | `127.0.0.1` / `8080` |

## Compilar

```bash
cargo build --release -p nodex-ia     # desde la raíz del repositorio
cargo test -p nodex-ia
```
