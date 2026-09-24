# Notas

Notas rápidas estilo Notepad, con espacios de trabajo, reuniones y una IA que organiza sola: detecta a qué espacio pertenece cada nota, la etiqueta, resume las reuniones y anota tareas y fechas en la agenda.

App de escritorio nativa en Rust ([egui](https://github.com/emilk/egui)). No usa navegador ni Electron. Funciona en Windows, macOS y Linux, y guarda todo en archivos de texto plano.

## Descargar

| Sistema | Archivo |
|---|---|
| Windows 10/11 | [Notas-windows-x64.exe](https://github.com/jpreyes/nodex-notes/releases/latest/download/Notas-windows-x64.exe) |
| macOS (Apple Silicon: M1, M2, M3…) | [Notas-macos-apple-silicon.zip](https://github.com/jpreyes/nodex-notes/releases/latest/download/Notas-macos-apple-silicon.zip) |
| macOS (Intel) | [Notas-macos-intel.zip](https://github.com/jpreyes/nodex-notes/releases/latest/download/Notas-macos-intel.zip) |
| Linux x64 | [Notas-linux-x64.tar.gz](https://github.com/jpreyes/nodex-notes/releases/latest/download/Notas-linux-x64.tar.gz) |

Todas las versiones están en [Releases](https://github.com/jpreyes/nodex-notes/releases).

- **Windows:** es un único `.exe`, no requiere instalación. Como no está firmado, Windows SmartScreen puede advertir: haz clic en *Más información → Ejecutar de todas formas*.
- **macOS:** descomprime y mueve `Notas.app` a Aplicaciones. Como no está firmada por Apple, la primera vez ábrela con clic derecho → *Abrir*. Si macOS dice que "está dañada", ejecuta `xattr -dr com.apple.quarantine /Applications/Notas.app`.
- **Linux:** `tar -xzf Notas-linux-x64.tar.gz && ./nodex-notes`

## Uso

- **Escribir:** se escribe directo en el editor y se guarda solo.
- **Etiquetas:** `#palabra` es una etiqueta; haz clic en ella en la barra lateral para ver todas sus líneas.
- **Espacios de trabajo:** cada espacio es una carpeta; cada nota es un archivo `.md`.
- **Reuniones:** con **Ctrl+R**, cada línea lleva su hora y **Esc** agrega `## fin · hora`. También se cierra sola tras 30 minutos sin escribir, al abrir otra reunión o al cerrar la app.
- **Tareas y Agenda:** las tareas se marcan como hechas con doble clic. La agenda muestra eventos y tareas con fecha.

| Atajo | Acción |
|---|---|
| Ctrl+N | Nueva nota |
| Ctrl+D | Nota de hoy |
| Ctrl+R | Nueva reunión |
| Ctrl+F | Buscar en todas las notas |
| Esc | Cerrar la reunión, la búsqueda o la vista |

## IA

Al dejar una nota (o tras 45 segundos sin tocarla), la IA:

- detecta si es una reunión, le pone `#reunión` y un resumen;
- la mueve a su espacio de trabajo, solo cuando está segura;
- agrega etiquetas (reutiliza las existentes) y pone nombre a las notas "Sin título";
- extrae tareas a `tareas.txt` (formato [todo.txt](https://github.com/todotxt/todo.txt)) y eventos a `agenda.txt`, los sincroniza con [Google Calendar](#google-calendar) y genera `agenda.ics` para Outlook u otros calendarios.

Cada cambio se puede deshacer desde la barra inferior. El botón ✦ organiza las notas antiguas pendientes.

## Configuración

Botón ⚙ en la app, o el archivo:

- Windows: `%APPDATA%\nodex-notes\config.toml`
- macOS: `~/Library/Application Support/nodex-notes/config.toml`
- Linux: `~/.config/nodex-notes/config.toml`

```toml
carpeta_notas = 'C:\Users\tu-usuario\Dropbox\Notas'  # por defecto: Dropbox/Notas
proveedor = "opencode"     # opencode (opencode.ai/zen), anthropic, openai, gemini u ollama
modelo = "deepseek-v4.1-flash"
clave_api = ""             # o variable OPENCODE_API_KEY / ANTHROPIC_API_KEY / OPENAI_API_KEY / GEMINI_API_KEY
ia_automatica = true
google_client_id = ""      # Google Calendar (ver más abajo)
google_client_secret = ""
```

Por defecto la IA usa **DeepSeek V4.1 Flash** a través de [OpenCode Zen](https://opencode.ai/zen): crea tu clave en opencode.ai y pégala en `clave_api`. Se puede usar cualquier otro modelo de Zen cambiando `modelo` (por ejemplo `deepseek-v4-pro`).

Sin clave API la app funciona igual; solo se desactiva la IA. Con `ollama` no hace falta clave.

## Google Calendar

La app puede sincronizar la agenda con tu Google Calendar. Crea un calendario propio llamado **"Notas"** y solo toca ese: el permiso que pide (`calendar.app.created`) no le da acceso a tus otros calendarios. Envía los eventos de la agenda y las tareas pendientes con fecha. Si una tarea se marca como hecha, o se deshace un cambio de la IA, el evento se quita de Google. Sincroniza después de cada cambio y cada 10 minutos.

Google exige que cada app tenga su propio ID de cliente OAuth. Se crea gratis, una sola vez (unos 5 minutos):

1. Entra a [Google Cloud Console](https://console.cloud.google.com/projectcreate) y crea un proyecto (por ejemplo "Notas").
2. Habilita la [Google Calendar API](https://console.cloud.google.com/apis/library/calendar-json.googleapis.com) en ese proyecto.
3. En **Google Auth Platform → Branding**, pon el nombre de la app ("Notas") y tu correo. En **Público** elige *Externo*.
4. En **Público**, presiona **Publicar app** (pasa a *En producción*). Si la dejas en *Prueba*, Google corta el permiso cada 7 días. Para uso personal no hace falta la verificación de Google.
5. En **Clientes → Crear cliente**, elige el tipo **App de escritorio**. Copia el *ID de cliente* y el *secreto* en `config.toml`:

   ```toml
   google_client_id = "123456-abc.apps.googleusercontent.com"
   google_client_secret = "GOCSPX-..."
   ```

6. Reinicia la app y, en **Agenda**, presiona **Conectar Google Calendar**. Se abrirá el navegador para que elijas tu cuenta y des permiso. Si Google avisa que la app no está verificada, entra en *Configuración avanzada → Ir a Notas*: es tu propia app.

El permiso queda guardado solo en este equipo (`google_token.json`, junto a `config.toml`), nunca en la carpeta de Dropbox. En otro computador hay que conectar de nuevo. La correspondencia entre elementos y eventos de Google se guarda en `.nodex/google.json`, para que dos equipos no dupliquen eventos.

## Archivos

```text
Notas/
  Proyecto Edificio A/
    Coordinación.md      ← una nota por archivo
  Docencia/
  tareas.txt             ← 2026-09-24 Enviar planos +Proyecto_Edificio_A due:2026-09-25
  agenda.txt             ← 2026-09-28 10:00 Visita del inspector +Proyecto_Edificio_A
  agenda.ics
  .nodex/                ← qué notas ya analizó la IA
  .papelera/             ← notas eliminadas
```

Si un archivo cambia por fuera (por ejemplo, Dropbox lo sincroniza desde otro equipo), la app lo recarga. Si justo lo estabas editando, la otra versión se guarda como copia "(conflicto)".

## Código

| Archivo | Qué hace |
|---|---|
| `src/main.rs` | Punto de entrada: lee la configuración y abre la ventana. |
| `src/app.rs` | Interfaz y comportamiento: barra de íconos, barra lateral, editor, reuniones, lo que hace la IA (y deshacer), vistas Tareas y Agenda. |
| `src/ai.rs` | Conexión con la IA (genai) en un hilo aparte y el prompt con las reglas. |
| `src/gcal.rs` | Google Calendar: OAuth con PKCE, calendario "Notas" y sincronización de eventos. |
| `src/agenda.rs` | `tareas.txt` (todo.txt), `agenda.txt` y `agenda.ics`. |
| `src/vault.rs` | Carpeta de notas: espacios, notas `.md`, cambios en disco, papelera. |
| `src/config.rs` | `config.toml` y `estado.toml` (última nota abierta). |
| `src/theme.rs` | Colores del tema claro, fuentes e íconos. |
| `src/tags.rs` | Reconoce las etiquetas `#palabra`. |
| `.github/workflows/release.yml` | Compila y publica los ejecutables al subir una etiqueta `vX.Y.Z`. |

## Compilar

```bash
cargo build --release
```

En Linux se necesitan `libxkbcommon-dev libgl1-mesa-dev libwayland-dev libx11-dev`.

Para publicar una versión, sube una etiqueta `vX.Y.Z`: GitHub Actions compila para los tres sistemas y adjunta los archivos al Release.

## Licencia

MIT
