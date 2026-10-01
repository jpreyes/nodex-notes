# VPS de Notas: servidor de la IA incluida (nodex-ia)

Este VPS atiende el **servidor de la IA incluida** de la app Notas (repositorio público
https://github.com/jpreyes/nodex-notes, carpeta `server/`). Lo expone a internet **Cloudflare
Tunnel (`cloudflared`)**. Tu trabajo: dejarlo instalado, publicado en un subdominio, que se
actualice solo desde GitHub y que se respalde; y después, ayudar con el día a día.

Lee también `server/README.md` del repositorio (qué hace el servidor y sus variables).

## Reglas

- **Exposición: solo `cloudflared`.** No instales Caddy, nginx, Apache ni certbot, y no abras
  puertos en el firewall: el servidor escucha en `127.0.0.1:8080` y el túnel lo publica.
- **Si `cloudflared` ya está corriendo con otros servicios, no los toques.** Solo agrega una regla
  para el subdominio nuevo (ver paso 5).
- **Las claves no pasan por el chat.** No pidas que te peguen claves (OpenCode, Resend). Crea
  `/etc/nodex-ia.env` con los campos vacíos y pide que la persona los llene con
  `sudo nano /etc/nodex-ia.env`. No muestres ese archivo en pantalla (`cat`) una vez lleno; para
  revisar, usa `sudo grep -c '^NOTAS_IA_KEY=.\+' /etc/nodex-ia.env`.
- Pregunta antes de cualquier cosa que no esté en estos pasos (otro software, cambios en servicios
  que no son de Notas, borrar datos).
- Comandos que necesitan root: con `sudo`.
- Al terminar cada paso, comprueba que funcionó antes de seguir.

## Instalación (una vez)

Pregunta primero: **¿qué subdominio?** (por ejemplo `ia.tudominio.cl`; el dominio tiene que estar
en la cuenta de Cloudflare).

1. **Revisar el equipo.** `uname -m` (x86_64 o ARM), `lsb_release -a`, si existen `git`, `curl` y
   `cloudflared` (`which cloudflared`, `systemctl status cloudflared`). Instala lo que falte de
   `git curl ca-certificates` con apt. Si el procesador **no** es x86_64, instala además
   `build-essential pkg-config cmake` y Rust con rustup para root (`curl https://sh.rustup.rs -sSf
   | sudo sh -s -- -y`): no hay programa publicado para ARM y se compila.

2. **El repositorio y el usuario del servicio.**
   ```bash
   sudo mkdir -p /opt/nodex-ia
   sudo git clone https://github.com/jpreyes/nodex-notes /opt/nodex-ia/repo
   sudo useradd --system --home /var/lib/nodex-ia --create-home nodex-ia   # si no existe
   sudo install -m 755 /opt/nodex-ia/repo/server/deploy/actualizar.sh /opt/nodex-ia/actualizar.sh
   sudo install -m 755 /opt/nodex-ia/repo/server/deploy/nodex-ia-respaldo.sh /opt/nodex-ia/respaldo.sh
   ```

3. **La configuración.** Copia `server/deploy/nodex-ia.env` a `/etc/nodex-ia.env` con
   `chmod 600` (dueño root). Deja `NOTAS_IA_DIRECCION=127.0.0.1` y `NOTAS_IA_PUERTO=8080` (si el
   8080 está ocupado, elige otro y úsalo también en el túnel). Pide a la persona que llene:
   - `NOTAS_IA_KEY`: la clave de OpenCode Zen del servicio (obligatoria).
   - `NOTAS_IA_CORREO_CLAVE` y `NOTAS_IA_CORREO_DE`: Resend, para mandar los códigos de entrada
     (opcional al comienzo: sin clave, el código aparece en `journalctl -u nodex-ia`).

4. **El programa y el servicio.**
   ```bash
   sudo cp /opt/nodex-ia/repo/server/deploy/nodex-ia.service /etc/systemd/system/
   sudo REPO_DIR=/opt/nodex-ia/repo /opt/nodex-ia/actualizar.sh   # baja (o compila) y arranca
   sudo systemctl enable --now nodex-ia
   curl -s http://127.0.0.1:8080/salud    # tiene que responder: ok
   ```
   Si no arranca: `journalctl -u nodex-ia -n 50`. Lo más común es `NOTAS_IA_KEY` vacía.

5. **Publicarlo con Cloudflare Tunnel.** Mira cómo está `cloudflared`:
   - **Ya hay un túnel administrado desde el panel** (el servicio corre con `--token` y no hay
     `ingress` en `/etc/cloudflared/config.yml`): la regla se agrega en el panel de Cloudflare.
     Explica a la persona: Zero Trust → Networks → Tunnels → (su túnel) → Public Hostname → Add:
     subdominio = el elegido, servicio = `http://localhost:8080`. Espera a que confirme.
   - **Ya hay un túnel con `config.yml`** (con `ingress:`): agrega, **antes** de la regla final
     `- service: http_status:404`:
     ```yaml
       - hostname: ia.tudominio.cl
         service: http://localhost:8080
     ```
     Luego `sudo cloudflared tunnel route dns <nombre-del-túnel> ia.tudominio.cl` y
     `sudo systemctl restart cloudflared`. Haz una copia del `config.yml` antes de editarlo.
   - **No hay `cloudflared`:** instálalo desde el repositorio apt de Cloudflare
     (https://pkg.cloudflare.com), y luego `cloudflared tunnel login` (muestra un enlace: la persona
     lo abre en su navegador y elige el dominio), `cloudflared tunnel create notas`,
     `cloudflared tunnel route dns notas ia.tudominio.cl`, un `/etc/cloudflared/config.yml` con
     `tunnel:`, `credentials-file:` y el `ingress` de arriba, y `sudo cloudflared service install`.

   Comprueba desde el VPS: `curl -s https://ia.tudominio.cl/salud` → `ok`.

6. **Que se actualice y respalde solo.**
   ```bash
   cd /opt/nodex-ia/repo/server/deploy
   sudo cp nodex-ia-actualizar.service nodex-ia-actualizar.timer nodex-ia-respaldo.service nodex-ia-respaldo.timer /etc/systemd/system/
   sudo systemctl daemon-reload
   sudo systemctl enable --now nodex-ia-actualizar.timer nodex-ia-respaldo.timer
   systemctl list-timers 'nodex-ia*'
   ```
   El de actualizar hace `git pull` del repositorio, baja el último programa publicado (o lo
   compila en ARM), reinicia y revisa `/salud`; si la versión nueva no responde, vuelve a la
   anterior. El de respaldo deja `/var/backups/nodex-ia/AAAA-MM-DD.tar.gz` (30 días).

7. **Prueba de punta a punta.** Pide un código de entrada para un correo de la persona:
   ```bash
   curl -s -X POST https://ia.tudominio.cl/v1/cuenta/codigo -H 'Content-Type: application/json' -d '{"correo":"SU_CORREO"}'
   ```
   Tiene que llegar por correo (o, sin Resend, aparecer en `journalctl -u nodex-ia -n 20`).

8. **Al terminar, dile a la persona:**
   - La dirección del servidor: `https://ia.tudominio.cl`.
   - Que en GitHub cree la variable del repositorio `NODEX_SERVIDOR_IA` con esa dirección
     (Settings → Secrets and variables → Actions → Variables), para que los instaladores de las
     próximas versiones la traigan puesta. Mientras tanto, en la app: Configuración → Tu cuenta →
     «Servidor».
   - Que cambie su propia cuenta a un plan sin vencimiento, después de entrar desde la app:
     `nodex-ia plan SU_CORREO fundador` (ver «Día a día»).

## Día a día

Los comandos de `nodex-ia` necesitan las variables del servicio:

```bash
nia() { sudo -u nodex-ia bash -c "set -a; . /etc/nodex-ia.env; /opt/nodex-ia/nodex-ia $*"; }
nia lista                         # uso del mes por persona y costo estimado (USD)
nia plan ana@correo.cl pro        # plan de una cuenta: prueba [días], pro, fundador o gratis
nia nuevo '"Ana Pérez"'           # un código a mano (se muestra una sola vez)
nia desactivar 3fa9c1             # deja sin IA a una persona (primeras letras de su huella)
```

- **Actualizar ahora:** `sudo systemctl start nodex-ia-actualizar` y `journalctl -u nodex-ia-actualizar -n 20`.
- **Ver qué pasa:** `journalctl -u nodex-ia -f` · `systemctl status nodex-ia cloudflared`.
- **Respaldo ahora:** `sudo systemctl start nodex-ia-respaldo`. Los datos viven en `/var/lib/nodex-ia`
  (`cuentas.json`, `usuarios.json`, `config/`, `uso/`). Para restaurar: detener el servicio,
  descomprimir el respaldo en `/var/lib/`, `chown -R nodex-ia:nodex-ia /var/lib/nodex-ia` y arrancar.
- **Cambiar el modelo o los límites:** editar `/etc/nodex-ia.env` (la persona, si hay claves de por
  medio) y `sudo systemctl restart nodex-ia`.
