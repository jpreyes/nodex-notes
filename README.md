# Notas

Notas rápidas estilo Notepad, con espacios de trabajo, reuniones y una IA que organiza sola: detecta a qué espacio pertenece cada nota, la etiqueta, resume las reuniones y anota tareas y fechas en la agenda.

App de escritorio nativa en Rust ([egui](https://github.com/emilk/egui)). No usa navegador ni Electron. Funciona en Windows, macOS y Linux, y guarda todo en archivos de texto plano.

## Descargar

| Sistema | Archivo |
|---|---|
| Windows 10/11 (instalador) | [Notas-windows-x64.msi](https://github.com/jpreyes/nodex-notes/releases/latest/download/Notas-windows-x64.msi) |
| Windows 10/11 (portable, sin instalar) | [Notas-windows-x64.exe](https://github.com/jpreyes/nodex-notes/releases/latest/download/Notas-windows-x64.exe) |
| macOS (Apple Silicon: M1, M2, M3…) | [Notas-macos-apple-silicon.zip](https://github.com/jpreyes/nodex-notes/releases/latest/download/Notas-macos-apple-silicon.zip) |
| macOS (Intel) | [Notas-macos-intel.zip](https://github.com/jpreyes/nodex-notes/releases/latest/download/Notas-macos-intel.zip) |
| Debian / Ubuntu | [Notas-linux-x64.deb](https://github.com/jpreyes/nodex-notes/releases/latest/download/Notas-linux-x64.deb) |
| Linux x64 (otras distribuciones) | [Notas-linux-x64.tar.gz](https://github.com/jpreyes/nodex-notes/releases/latest/download/Notas-linux-x64.tar.gz) |

Todas las versiones están en [Releases](https://github.com/jpreyes/nodex-notes/releases).

- **Windows:** el `.msi` instala Notas en Archivos de programa y la agrega al menú Inicio (se desinstala desde *Aplicaciones instaladas*); el `.exe` funciona sin instalar. Como no está firmado, Windows SmartScreen puede advertir: haz clic en *Más información → Ejecutar de todas formas*.
- **macOS:** descomprime y mueve `Notas.app` a Aplicaciones. Como no está firmada por Apple, la primera vez ábrela con clic derecho → *Abrir*. Si macOS dice que "está dañada", ejecuta `xattr -dr com.apple.quarantine /Applications/Notas.app`.
- **Debian / Ubuntu:** `sudo apt install ./Notas-linux-x64.deb` (queda en el menú de aplicaciones como "Notas").
- **Otras distribuciones Linux:** `tar -xzf Notas-linux-x64.tar.gz && ./nodex-notes`

## Uso

- **Escribir:** se escribe directo en el editor y se guarda solo.
- **Cada línea es una nota** y lleva su número a la izquierda. Con **Tab** la línea pasa a ser parte de la nota de arriba; con **Tab Tab**, un ítem de lista de esa nota (Tab Tab Tab, un subítem). **Shift+Tab** quita un nivel y **Enter** sigue la lista (en un ítem vacío, sale de ella). En el archivo queda como Markdown normal: `  texto`, `  - ítem`, `    - subítem`.
- **Etiquetas:** `#palabra` es una etiqueta. Se ve como una píldora de color, sin el `#`, y cada etiqueta tiene siempre el mismo color. Haz clic en ella en la barra lateral para ver todas sus líneas. En la línea donde está el cursor se ve el texto tal cual, para poder editarlo.
- **Tareas en la nota:** `- [ ] tarea` se ve con una casilla. Un clic la marca como hecha, también en Tareas y en Google Calendar. `due:2026-09-26` se ve como una fecha ("mañana", "vie 26") y en rojo si venció.
- **Rutas y webs:** las direcciones web y las rutas de carpetas se abren con un clic. Una ruta escrita a mano, como `/workspace/proyectos/consorcio/04 Trincheras`, se busca dentro de Dropbox aunque tenga mayúsculas distintas o un error de tipeo.
- **Preguntar (Ctrl+K):** pregúntale a la IA sobre todas tus notas, por ejemplo "¿cómo eran las notas de la reunión de la semana pasada con el CIC?", "resumen de las notas del proyecto LaVet" o "¿cuáles son todas las tareas que me faltan?". Responde citando cada dato con el número de su nota (un clic la abre en esa línea), las tareas de la respuesta se pueden marcar ahí mismo, y puedes seguir preguntando sobre lo mismo. La respuesta se puede guardar como nota o copiar.
- **La IA pregunta cuando duda:** si no está segura de algo (a qué proyecto va una línea, si una línea es detalle de otra, qué fecha es "la próxima semana"), no adivina: deja una pregunta con opciones arriba de la nota y en la vista Hoy (el ícono ✦ muestra cuántas hay). Tu respuesta se aplica al tiro (mover, unir, fecha, etiquetas) y se puede deshacer. También puedes escribir tu propia respuesta, o ignorar la pregunta. Lo que conviene recordar queda en `aprendido.txt` (en tu carpeta de notas; puedes editarlo), y la IA lo usa en cada análisis para no volver a preguntar lo mismo.
- **Hoy (Ctrl+H):** lo atrasado, lo de hoy, lo de mañana y lo que viene en la semana. Aparece solo la primera vez que abres la app cada día, si hay algo pendiente.
- **Espacios de trabajo:** cada espacio es una carpeta; cada nota es un archivo `.md`.
- **Reuniones:** con **Ctrl+R**, cada línea lleva su hora y **Esc** agrega `## fin · hora`. También se cierra sola tras 30 minutos sin escribir, al abrir otra reunión o al cerrar la app.
- **Tareas y Agenda:** las tareas se marcan como hechas con doble clic. La agenda muestra eventos y tareas con fecha.

| Atajo | Acción |
|---|---|
| Ctrl+N | Nueva nota |
| Ctrl+D | Nota de hoy |
| Ctrl+H | Hoy: atrasado, hoy y esta semana |
| Ctrl+K | Preguntar a tus notas |
| Ctrl+R | Nueva reunión |
| Ctrl+F | Buscar en todas las notas |
| Ctrl+, | Configuración |
| Esc | Cerrar la reunión, la búsqueda o la vista |
| Tab / Shift+Tab | Unir la línea a la nota de arriba o hacerla ítem de lista / quitar un nivel |

## IA

Al dejar una nota (o tras 45 segundos sin tocarla), la IA la organiza.

**Cada línea es una nota distinta**, con sus líneas con sangría. Un bloque que empieza con `## Título` va junto hasta `## fin`, el siguiente `##` o una línea en blanco. En todas las notas, la IA trabaja nota por nota:

- **Etiquetas por nota:** cada línea recibe sus propias etiquetas, en esa misma línea (reutiliza las que ya existen).
- **Tareas en su línea:** la línea de donde sale una tarea pasa a tener casilla y fecha: `- [ ] Debo entregar el informe a la UTalca #utalca due:2026-09-26 ^k3f9a`. El `^k3f9a` (que no se ve) une la línea con su tarea en `tareas.txt`, para que marcarla en un lado la marque en el otro.
- **Detalles juntos:** si una línea es detalle de otra (por ejemplo, dónde está la carpeta de un informe), la une a ella con sangría.

**En la nota del día y en las "Sin título" (captura rápida)**, además, cada nota se va a donde corresponde:

- a una nota existente del espacio que corresponde (por ejemplo, "Trincheras" en *Consorcio*), o a una nota nueva con un título breve, junto con sus detalles;
- los bloques de reunión se mueven completos, con un resumen y `#reunión`;
- lo que no puede atribuir con seguridad se queda donde está.

**En las notas con título propio**, las líneas se quedan donde están, y la IA también:

- detecta si es una reunión, le pone `#reunión` y un resumen;
- la mueve a su espacio de trabajo, solo cuando está segura.

En ambos casos, extrae tareas a `tareas.txt` (formato [todo.txt](https://github.com/todotxt/todo.txt)) y eventos a `agenda.txt`, los sincroniza con [Google Calendar](#google-calendar) y genera `agenda.ics` para Outlook u otros calendarios.

Cada cambio se puede deshacer desde la barra inferior. El botón ✦ organiza las notas antiguas pendientes.

**Preguntar** envía a la IA la pregunta junto con tus tareas, la agenda y tus notas, con sus líneas numeradas para que pueda citarlas. Si tienes pocas notas (hasta unos 80.000 caracteres) van todas. Si son más, la IA primero elige cuáles leer a partir de un índice (espacio, título, fecha, etiquetas y el comienzo de cada una) y después responde leyendo solo esas. Si la respuesta no está en tus notas, lo dice. La última pregunta queda en `ia-pregunta.txt` (junto a `config.toml`).

## Configuración

Todo se configura desde la ventana **Configuración** (botón ⚙ abajo a la izquierda, o **Ctrl+,**): carpeta de notas, proveedor y modelo de IA, clave API, prueba de conexión, Google Calendar y búsqueda de actualizaciones. Cada cambio se guarda y se aplica al instante.

Por debajo se guarda en un archivo de texto que también se puede editar a mano:

- Windows: `%APPDATA%\nodex-notes\config.toml`
- macOS: `~/Library/Application Support/nodex-notes/config.toml`
- Linux: `~/.config/nodex-notes/config.toml`

```toml
carpeta_notas = 'C:\Users\tu-usuario\Dropbox\Notas'  # por defecto: Dropbox/Notas
proveedor = "opencode"     # opencode (Zen), opencode-go (plan Go), anthropic, openai, gemini u ollama
modelo = "deepseek-v4.1-flash"
clave_api = ""             # o variable OPENCODE_API_KEY / ANTHROPIC_API_KEY / OPENAI_API_KEY / GEMINI_API_KEY
ia_automatica = true
google_client_id = ""      # Google Calendar (ver más abajo)
google_client_secret = ""
```

Por defecto la IA usa **DeepSeek V4.1 Flash** a través de [OpenCode Zen](https://opencode.ai/zen): crea tu clave en opencode.ai y pégala en `clave_api`. Se puede usar cualquier otro modelo de Zen cambiando `modelo` (por ejemplo `deepseek-v4-pro`).

Si tienes el plan **OpenCode Go** (suscripción mensual con límite de uso), usa `proveedor = "opencode-go"`: misma clave y mismo modelo, pero cobra contra tu suscripción (`https://opencode.ai/zen/go/v1/`) en vez de tu saldo de Zen (`https://opencode.ai/zen/v1/`). La app se identifica como `nodex-notes/<versión>` y envía el ID de sesión `x-opencode-session` que Go exige. Ten en cuenta que OpenCode diseñó Go para agentes de programación y vigila el tipo de uso; para organizar notas, Zen no tiene esa restricción.

Sin clave API la app funciona igual; solo se desactiva la IA. Con `ollama` no hace falta clave.

## Google Calendar

La app puede sincronizar la agenda con tu Google Calendar. Crea un calendario propio llamado **"Notas"** y solo toca ese: el permiso que pide (`calendar.app.created`) no le da acceso a tus otros calendarios. Envía los eventos de la agenda y las tareas pendientes con fecha. Si una tarea se marca como hecha, o se deshace un cambio de la IA, el evento se quita de Google. Sincroniza después de cada cambio y cada 10 minutos.

Google exige que cada app tenga su propio ID de cliente OAuth. Se crea gratis, una sola vez (unos 5 minutos):

1. Entra a [Google Cloud Console](https://console.cloud.google.com/projectcreate) y crea un proyecto (por ejemplo "Notas").
2. Habilita la [Google Calendar API](https://console.cloud.google.com/apis/library/calendar-json.googleapis.com) en ese proyecto.
3. En **Google Auth Platform → Branding**, pon el nombre de la app ("Notas") y tu correo. En **Público** elige *Externo*.
4. En **Público**, presiona **Publicar app** (pasa a *En producción*). Si la dejas en *Prueba*, Google corta el permiso cada 7 días. Para uso personal no hace falta la verificación de Google.
5. En **Clientes → Crear cliente**, elige el tipo **App de escritorio**. Copia el *ID de cliente* y el *secreto* en **Configuración → Calendar** (la misma ventana trae estos pasos con enlaces directos).
6. Presiona **Conectar**. Se abrirá el navegador para que elijas tu cuenta y des permiso. Si Google avisa que la app no está verificada, entra en *Configuración avanzada → Ir a Notas*: es tu propia app.

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
  aprendido.txt          ← lo que la IA aprendió de tus respuestas (editable)
  .nodex/                ← qué notas ya analizó la IA
  .papelera/             ← notas eliminadas
```

Si la IA falla o responde vacío, el último intercambio queda en `ia-ultima.txt` (junto a `config.toml`) para ver qué pasó.

Si un archivo cambia por fuera (por ejemplo, Dropbox lo sincroniza desde otro equipo), la app lo recarga. Si justo lo estabas editando, la otra versión se guarda como copia "(conflicto)".

## Código

| Archivo | Qué hace |
|---|---|
| `src/main.rs` | Punto de entrada: lee la configuración y abre la ventana. |
| `src/app.rs` | Interfaz y comportamiento: barra de íconos, barra lateral, reuniones, lo que hace la IA (y deshacer), vistas Tareas y Agenda. |
| `src/app/editor.rs` | El editor: números, píldoras de etiquetas, sangrías y listas, casillas, fechas, enlaces; Tab, Enter y Retroceso. |
| `src/app/today.rs` | Vista Hoy (atrasado, hoy, mañana y la semana). |
| `src/app/ask_view.rs` | Vista Preguntar: conversación, citas clicables, casillas de tareas, guardar y copiar. |
| `src/ask.rs` | Preguntar: elige qué notas leer, arma la pregunta con las líneas numeradas y separa la respuesta en párrafos, listas y citas. |
| `src/app/settings.rs` | Ventana de Configuración (General, IA, Calendar, Atajos, Acerca de). |
| `src/ai.rs` | Conexión con la IA (genai) en un hilo aparte y el prompt con las reglas. |
| `src/lines.rs` | Estructura de una nota: niveles de sangría, casillas, `due:`/`^id` y qué líneas forman cada nota. |
| `src/organize.rs` | Aplica al texto lo que respondió la IA: etiquetas por línea, tareas con casilla, detalles unidos y a dónde va cada nota. |
| `src/doubts.rs` | Preguntas de la IA cuando duda (`.nodex/dudas.json`) y lo aprendido (`aprendido.txt`). |
| `src/app/doubts_ui.rs` | La tarjeta de cada pregunta y lo que pasa al responder. |
| `src/capture.rs` | Qué notas son de captura (la del día y las "Sin título"). |
| `src/links.rs` | Encuentra rutas (tolerando errores de tipeo) y direcciones web en las líneas. |
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
