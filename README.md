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

- **La barra izquierda** tiene seis accesos, cada uno con su nombre: Inicio, Reunión, Tareas, Agenda, Correo e IA (abajo, la carpeta de notas y los Ajustes).
- **Inicio (Ctrl+H):** un resumen de todo en una página. Desde ahí puedes anotar algo rápido (va a la nota de hoy y la IA lo ordena después) o preguntarle a tus notas. **Tu día** muestra lo atrasado, lo de hoy, lo de mañana y lo que viene en la semana, todo marcable ahí mismo (en un evento de hoy de tus calendarios, **Tomar notas** abre una reunión con su nombre). También muestra lo que la IA tiene pendiente, las reuniones recientes con sus acuerdos abiertos, las notas recientes, tus espacios con sus tareas y cómo va la semana. Se abre la primera vez de cada día.
- **Pestañas:** cada pestaña muestra una nota o una vista (Inicio, IA, Semana, Tareas…), para cambiar de conversación o de tema con un clic. Una nota o vista nunca queda en dos pestañas: si ya está abierta, se va a su pestaña, y lo que abres desde Inicio o la IA se abre en una pestaña nueva (Inicio no se reemplaza). El **+** de las pestañas abre una nota nueva (en el espacio que indica), una reunión o Inicio. **Ctrl+T** abre una nueva, **Ctrl+W** la cierra, **Ctrl+Tab** pasa a la siguiente (con Shift, a la anterior) y **Ctrl+1…9** salta a una. **Ctrl+clic** (o clic con la rueda) en una nota la abre en otra pestaña. Se recuerdan al cerrar la app.
- **Escribir:** se escribe directo en el editor y se guarda solo.
- **Cada línea es una nota** y lleva su número a la izquierda. Con **Tab** la línea pasa a ser parte de la nota de arriba; con **Tab Tab**, un ítem de lista de esa nota (Tab Tab Tab, un subítem). **Shift+Tab** quita un nivel y **Enter** sigue la lista (en un ítem vacío, sale de ella). En el archivo queda como Markdown normal: `  texto`, `  - ítem`, `    - subítem`.
- **Borrar:** al pasar el mouse por una nota o un espacio de la barra lateral aparece un tacho (también con clic derecho). La nota va a la carpeta `.papelera` y se puede deshacer desde la barra inferior; un espacio pide confirmación y también se puede deshacer.
- **Etiquetas:** `#palabra` es una etiqueta. Se ve como una píldora de color, sin el `#`, y cada etiqueta tiene siempre el mismo color. Haz clic en ella en la barra lateral para ver todas sus líneas. En la línea donde está el cursor se ve el texto tal cual, para poder editarlo.
- **Tareas en la nota:** `- [ ] tarea` se ve con una casilla. Un clic la marca como hecha, también en Tareas y en Google Calendar. `due:2026-09-26` se ve como una fecha ("mañana", "vie 26") y en rojo si venció.
- **Rutas y webs:** las direcciones web y las rutas de carpetas se abren con un clic. Una ruta escrita a mano, como `/workspace/proyectos/consorcio/04 Trincheras`, se busca dentro de Dropbox aunque tenga mayúsculas distintas o un error de tipeo.
- **La ventana de la IA (✦ o Ctrl+K):** todo lo de la IA en un solo lugar, con tres secciones. Arriba muestra qué modelo usa, si está organizando algo y cuántas notas faltan por organizar (con **Organizar ahora**).
  - **Conversar:** pregúntale a la IA sobre todas tus notas, por ejemplo "¿cómo eran las notas de la reunión de la semana pasada con el CIC?", "resumen de las notas del proyecto LaVet" o "¿cuáles son todas las tareas que me faltan?". Responde citando cada dato con el número de su nota (un clic la abre en esa línea), las tareas de la respuesta se pueden marcar ahí mismo, y puedes seguir preguntando sobre lo mismo. La respuesta se puede guardar como nota o copiar.
  - **Preguntas:** lo que la IA no supo con seguridad y las sugerencias de espacios nuevos, para responder ahí mismo (el ✦ muestra cuántas hay y al hacer clic abre esta sección). También **Buscar notas repetidas** y lo que aprendió de tus respuestas.
  - **Lo que hizo:** cada cambio de la IA, por día y hora: qué nota organizó y a dónde llevó cada cosa, qué etiquetas, tareas y eventos agregó, qué preguntas dejó, qué correos anotó, qué respuestas aplicó y qué notas repetidas unió. Cada uno con un enlace a su nota; el último se puede deshacer ahí mismo. Se guarda en `.nodex/actividad.json`.
- **La IA pregunta cuando duda:** si no está segura de algo (a qué proyecto va una línea, si una línea es detalle de otra, qué fecha es "la próxima semana"), no adivina: deja una pregunta con opciones arriba de la nota y en la ventana de la IA (el ícono ✦ muestra cuántas hay e Inicio avisa). Tu respuesta se aplica al tiro (mover, unir, fecha, etiquetas) y se puede deshacer. También puedes escribir tu propia respuesta, o ignorar la pregunta. Lo que conviene recordar queda en `aprendido.txt` (en tu carpeta de notas; puedes editarlo), y la IA lo usa en cada análisis para no volver a preguntar lo mismo.
- **Duplicados:** si anotas algo que ya estaba (en la misma nota o en otra), la app lo detecta comparando las palabras importantes, sin acentos ni palabras de relleno y sin usar la IA, y pregunta "¿es lo mismo que…?". **Unir** deja una sola línea con las etiquetas y los detalles de ambas; si las dos eran tareas, queda una sola, con la fecha que hubiera. **Son distintas** hace que no vuelva a preguntar por ese par. Se revisa sola después de cada análisis, y el botón **Buscar notas repetidas** de la ventana de la IA revisa todas las notas. Unir se puede deshacer.
- **Espacios nuevos:** la IA va notando qué notas tratan de un proyecto o tema concreto que no tiene espacio (por ejemplo "LaVet"). Cuando junta 3 notas del mismo tema, la ventana de la IA sugiere crearlo: **Crear y mover** crea el espacio y lleva ahí esas notas con sus tareas (se puede deshacer); **Solo crear**; **Ahora no** (vuelve a sugerirlo cuando haya 3 notas más); **No, gracias** (no lo vuelve a sugerir, y la IA lo sabe).
- **Revisión semanal:** desde el lunes, Inicio te ofrece revisar la semana (también con el enlace *Revisión semanal*). Muestra cuántas notas escribiste, tareas hiciste y reuniones tuviste; lo atrasado, lo que viene en los próximos 7 días, las tareas sin fecha y lo hecho, todo marcable ahí mismo. Con **Hacer el resumen**, la IA lee solo las notas de la semana y arma un resumen por proyecto (qué avanzó, qué se decidió, qué quedó pendiente) con 3 prioridades para la próxima, citando sus fuentes; se puede guardar como nota.
- **La nota de hoy (Ctrl+D):** cada espacio tiene una nota por día para ir anotando lo que surja; en la barra lateral está siempre arriba como **Hoy**, y las de días anteriores quedan agrupadas en **Diario** («Ayer», «Vie 25 sep»…). En una carpeta nueva, la primera nota es una **Bienvenida** que explica cómo se usa.
- **Espacios de trabajo:** cada espacio es una carpeta; cada nota es un archivo `.md`.
- **Reuniones:** con **Ctrl+R**, cada línea lleva su hora y **Esc** agrega `## fin · hora`. La hora de cada línea, el inicio y el fin se ven como etiquetas («10:02», «Reunión · jue 24 sep · 10:00», «Fin · 10:40») y los títulos sin sus `#` (se ven al editar esa línea). También se cierra sola tras 30 minutos sin escribir, al abrir otra reunión o al cerrar la app. Al cerrarla, la IA agrega un resumen con los asistentes, las decisiones y los acuerdos: cada acuerdo queda como casilla con su responsable y su fecha (`- [ ] Juan: enviar planos due:…`). Los tuyos van a Tareas; los de otros también, marcados con `@Juan`, para saber qué esperas de quién. En una nota de reunión, **Correo de seguimiento** hace que la IA redacte el borrador del correo, que puedes editar, copiar o abrir en tu programa de correo.
- **Correo:** en Configuración → Correo agrega tu cuenta (Gmail, Outlook, iCloud u otra con IMAP) con una **contraseña de aplicación**: una contraseña especial que se crea en tu cuenta (requiere la verificación en dos pasos) y sirve solo para esta app. La app revisa el correo **al llegar uno nuevo** (IMAP IDLE: el servidor avisa al instante), **una vez al día** a la hora que elijas (por defecto 07:00) y cuando presionas **Revisar ahora**; las dos primeras se pueden apagar en Configuración → Correo. Lee la bandeja de entrada y los enviados (la primera vez, los de la última semana) sin marcarlos como leídos. La IA resume cada uno y anota los importantes (los que piden algo, fijan una fecha o traen un compromiso) como una línea en la nota de hoy de su espacio, por ejemplo «Correo de María Soto (26 sep): Visita a obra. Debo enviar la cubicación antes del martes 29 de septiembre…». Desde ahí el organizador los trata como cualquier nota: los lleva a su nota, les pone etiquetas, deja sus tareas con casilla y fecha, manda las citas a la Agenda y suma para sugerir espacios nuevos (con Deshacer). Si un correo parece cumplir algo pendiente (por ejemplo, llegan los planos que Juan prometió), la vista **Correos** te pregunta si marcarlo hecho. Los boletines y avisos automáticos se ignoran. Preguntar también conoce tus correos recientes. La contraseña queda en `config.toml` y los correos en `correos.json`, ambos solo en tu equipo; para entenderlos, su texto se envía al modelo de IA configurado.
- **Tus calendarios:** en la Agenda (o en Configuración → Calendar), **+ Agregar calendario** y pega el enlace ICS de tu calendario. Google Calendar lo da en su configuración como «Dirección secreta en formato iCal», Outlook en «Publicar un calendario → ICS», e iCloud como enlace público (webcal://). Puedes agregar varios, cada uno con su color. Sus eventos aparecen en Agenda, Inicio y Semana, y Preguntar también los conoce. Se leen al abrir la app y cada 15 minutos (solo se leen; la app no los cambia). En un evento de hoy, **Tomar notas** abre una reunión con su nombre.
- **Tareas y Agenda:** las tareas tienen la misma casilla en la nota, en Tareas, en Inicio, en la Semana y en la Agenda; un clic la marca como hecha en todas partes. Al agregar una tarea puedes escribir la fecha como la dirías: «Enviar planos el viernes», «Llamar a Pedro mañana», «Informe 30 sep» o «antes del 3 de octubre». La agenda muestra eventos y tareas con fecha. Las tareas de otros se leen «Juan Pérez: enviar planos».
- **Microsoft To Do:** en Configuración → Tareas (o en Tareas), **Conectar** abre el navegador para entrar con tu cuenta Microsoft (personal o del trabajo, si tu organización lo permite) y aceptar el permiso; no hay que configurar nada más. Tus tareas quedan en una lista **«Notas»** de To Do, en los dos sentidos: lo que la IA saca de tus notas llega a To Do con su fecha; lo que marcas hecho (o le cambias la fecha) en un lado se refleja en el otro, también en la casilla de la nota; y lo que agregas a esa lista desde To Do (por ejemplo, desde el celular) aparece en Tareas. Borrar una tarea en Notas la borra en To Do; borrarla en To Do la deja en Notas. Se sincroniza al cambiar algo y cada 2 minutos. El permiso queda solo en este equipo (`microsoft_token.json`, junto a `config.toml`), y `.nodex/todo.json` recuerda qué tarea es cuál.

| Atajo | Acción |
|---|---|
| Ctrl+T / Ctrl+W | Nueva pestaña / cerrar pestaña |
| Ctrl+Tab / Ctrl+1…9 | Pestaña siguiente / ir a una pestaña |
| Ctrl+N | Nueva nota |
| Ctrl+D | Nota de hoy |
| Ctrl+H | Inicio: tu día y un resumen de todo |
| Ctrl+K | La IA: conversar con tus notas |
| Ctrl+R | Nueva reunión |
| Ctrl+F | Buscar en todas las notas (sin importar tildes ni mayúsculas) |
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

Cuando la IA cambia algo por su cuenta (ordena una nota, anota correos), aparece un aviso arriba a la derecha con lo que hizo y a dónde llevó cada cosa, con **Ver lo que hizo** y **Deshacer**; dura unos segundos y no se va mientras tengas el mouse encima. Cada cambio se puede deshacer desde el aviso, la barra inferior o **Lo que hizo** en la ventana de la IA, donde también está **Organizar ahora** para las notas antiguas pendientes.

**Conversar** envía a la IA la pregunta junto con tus tareas, la agenda y tus notas, con sus líneas numeradas para que pueda citarlas. Si tienes pocas notas (hasta unos 80.000 caracteres) van todas. Si son más, la app primero se queda con las 120 más relevantes para la pregunta, sin usar la IA: mira las palabras de la pregunta (con más peso si están en el título o en el nombre del espacio, y a las palabras poco comunes), las fechas que menciona («ayer», «la semana pasada»), si habla de reuniones y lo más reciente; una pregunta de seguimiento hereda el tema de la anterior. Si esas aún no caben, la IA elige cuáles leer a partir de un índice (espacio, título, fecha, etiquetas y el comienzo de cada una) y después responde leyendo solo esas. Así funciona igual con 100 notas que con 50.000, y cuesta lo mismo. Si la respuesta no está en tus notas, lo dice. La última pregunta queda en `ia-pregunta.txt` (junto a `config.toml`).

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

## Google Calendar (avanzado)

Para *ver* tus calendarios basta con agregarlos con su enlace (arriba). Esto es para lo contrario: *enviar* la agenda de tus notas a Google Calendar.

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

Si un archivo cambia por fuera (por ejemplo, Dropbox lo sincroniza desde otro equipo), la app lo recarga al instante: el sistema operativo le avisa qué archivos cambiaron, sin revisar toda la carpeta (una revisión completa se hace igual cada 10 minutos, por seguridad). Si justo lo estabas editando, la app junta las dos versiones línea por línea (como hace git), usando como base lo último que leyó o guardó: los cambios en líneas distintas se juntan solos, lo que un lado borró o movió no reaparece, y si los dos cambiaron la misma línea quedan las dos versiones y se avisa. Antes de guardar también revisa si el archivo cambió por fuera, así que nunca pisa lo que llegó de otro equipo.

Si Dropbox deja una «copia en conflicto» (pasa cuando dos equipos cambian el mismo archivo sin conexión), la app la reconoce por su nombre, espera unos segundos a que Dropbox termine, la junta con el archivo original sin perder ninguna línea y manda la copia a la papelera. En `tareas.txt` junta por tarea: lo que quedó hecho en un equipo queda hecho. Queda anotado en «Lo que hizo», con Deshacer.

Para abrir rápido aunque haya miles de notas, la app guarda una copia de todas en un solo archivo local (`AppData\Local\nodex-notes` en Windows, la carpeta de caché del sistema en Mac y Linux; nunca en Dropbox). Al abrir lee esa copia y solo relee del disco las notas que cambiaron desde la última vez. Si la copia falta o está dañada, simplemente lee todas las notas.

## Código

| Archivo | Qué hace |
|---|---|
| `src/main.rs` | Punto de entrada: lee la configuración y abre la ventana. |
| `src/app.rs` | Interfaz y comportamiento: barra de íconos, barra lateral, reuniones, lo que hace la IA (y deshacer), vistas Tareas y Agenda. |
| `src/app/editor.rs` | El editor: números, píldoras de etiquetas, sangrías y listas, casillas, fechas, enlaces; Tab, Enter y Retroceso. |
| `src/app/home.rs` | Inicio: el resumen de todo, con anotar y preguntar rápido. |
| `src/app/tabs.rs` | Pestañas: notas y vistas, atajos y cómo se recuerdan. |
| `src/app/followup.rs` | Correo de seguimiento de una reunión. |
| `src/app/week.rs` | Revisión semanal: números, listas de la semana y resumen de la IA. |
| `src/todo.rs`, `src/app/todo_ui.rs` | Microsoft To Do: conexión, sincronización en los dos sentidos y el panel en Tareas. |
| `src/app/today.rs` | «Tu día» en Inicio (atrasado, hoy, mañana y la semana). |
| `src/app/ai_view.rs` | Ventana de la IA: estado, Conversar, Preguntas y Lo que hizo. |
| `src/activity.rs` | Lo que hizo la IA: historial por día en `.nodex/actividad.json`. |
| `src/app/ask_view.rs` | Conversar: conversación, citas clicables, casillas de tareas, guardar y copiar. |
| `src/ask.rs` | Preguntar: elige qué notas leer, arma la pregunta con las líneas numeradas y separa la respuesta en párrafos, listas y citas. |
| `src/app/settings.rs` | Ventana de Configuración (General, IA, Calendar, Atajos, Acerca de). |
| `src/ai.rs` | Conexión con la IA (genai) en un hilo aparte y el prompt con las reglas. |
| `src/lines.rs` | Estructura de una nota: niveles de sangría, casillas, `due:`/`^id` y qué líneas forman cada nota. |
| `src/organize.rs` | Aplica al texto lo que respondió la IA: etiquetas por línea, tareas con casilla, detalles unidos y a dónde va cada nota. |
| `src/doubts.rs` | Preguntas de la IA cuando duda (`.nodex/dudas.json`) y lo aprendido (`aprendido.txt`). |
| `src/spaces.rs` | Temas sin espacio que la IA va encontrando (`.nodex/espacios.json`) y cuándo sugerir crearlos. |
| `src/app/spaces_ui.rs` | La tarjeta de sugerencia y crear el espacio moviendo sus notas. |
| `src/dups.rs` | Duplicados: compara las palabras importantes de cada línea y une dos notas en una. |
| `src/app/doubts_ui.rs` | La tarjeta de cada pregunta y lo que pasa al responder. |
| `src/capture.rs` | Qué notas son de captura (la del día y las "Sin título"). |
| `src/links.rs` | Encuentra rutas (tolerando errores de tipeo) y direcciones web en las líneas. |
| `src/mail.rs` | Correo por IMAP (TLS): cuentas, lectura de entrada y enviados sin marcarlos, y los correos guardados. |
| `src/mail_ai.rs` | Lo que la IA saca de los correos: resumen, compromisos, fechas y qué tareas cumplen. |
| `src/app/mail_ui.rs` | Vista Correos, revisión cada 15 minutos, verificación de compromisos y cuentas en Configuración. |
| `src/ics.rs` | Lee calendarios ICS: eventos, repeticiones, excepciones y horas UTC. |
| `src/calendars.rs` | Calendarios agregados con su enlace: descarga cada 15 minutos y copia para verlos sin internet. |
| `src/app/calendars_ui.rs` | La lista de calendarios con «+ Agregar calendario». |
| `src/gcal.rs` | Google Calendar: OAuth con PKCE, calendario "Notas" y sincronización de eventos. |
| `src/agenda.rs` | `tareas.txt` (todo.txt), `agenda.txt` y `agenda.ics`. |
| `src/conflicts.rs`, `src/app/conflicts_ui.rs` | Reconoce las «copias en conflicto» de Dropbox y las junta con su original. |
| `src/merge.rs` | Junta dos versiones de una nota línea por línea (con base común, o conservando todo si no hay base). |
| `src/vault.rs` | Carpeta de notas: espacios, notas `.md`, cambios en disco, papelera. |
| `src/config.rs` | `config.toml` y `estado.toml` (última nota abierta). |
| `src/theme.rs` | Colores del tema claro, fuentes e íconos. |
| `src/tags.rs` | Reconoce las etiquetas `#palabra`. |
| `src/app/rendimiento.rs` | Banco de pruebas de rendimiento: genera carpetas de 1.000 a 50.000 notas y mide la app sin ventana. |
| `tools/medir-ventana.ps1` | Mide la app real con ventana (Windows), con las mismas carpetas. |
| `.github/workflows/release.yml` | Compila y publica los ejecutables al subir una etiqueta `vX.Y.Z`. |

## Compilar

```bash
cargo build --release
```

En Linux se necesitan `libxkbcommon-dev libgl1-mesa-dev libwayland-dev libx11-dev`.

Para publicar una versión, sube una etiqueta `vX.Y.Z`: GitHub Actions compila para los tres sistemas y adjunta los archivos al Release.

## Rendimiento

El banco de pruebas genera carpetas de notas realistas (1.000, 5.000, 10.000 y 50.000 notas, siempre las mismas, en la carpeta temporal `nodex-bench/`) y mide la app sin abrir ventana: cargar, dibujar Inicio y una nota, la revisión de la carpeta que se hace cada segundo, buscar y la RAM. Cada tamaño se mide en un proceso aparte. El resultado se imprime y se agrega a `target/rendimiento.md`.

```bash
cargo test --release rendimiento -- --ignored --nocapture
```

Para medir solo algunos tamaños: `NODEX_BENCH=1000,10000`. Con las carpetas ya generadas, `tools/medir-ventana.ps1` mide en Windows la app real con ventana (tiempo hasta el primer cuadro, RAM y procesador con la app quieta). La primera lectura de una carpeta es «en frío» (los archivos no están en la memoria de Windows) y puede ser mucho más lenta: conviene medir dos veces.

Para ver si algo repinta de más con la app quieta, la variable `NODEX_CUADROS=1` hace que la app escriba cada 5 segundos cuántos cuadros dibujó por segundo y quién los pidió. Lo normal es 1 por segundo (la revisión periódica). El cursor de texto parpadea mientras se usa la app y queda fijo tras 10 segundos sin tocar nada, justamente para no repintar de más.

## Licencia

MIT
