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
- extrae tareas a `tareas.txt` (formato [todo.txt](https://github.com/todotxt/todo.txt)) y eventos a `agenda.txt`, y genera `agenda.ics` para importar en Google Calendar u Outlook.

Cada cambio se puede deshacer desde la barra inferior. El botón ✦ organiza las notas antiguas pendientes.

## Configuración

Botón ⚙ en la app, o el archivo:

- Windows: `%APPDATA%\nodex-notes\config.toml`
- macOS: `~/Library/Application Support/nodex-notes/config.toml`
- Linux: `~/.config/nodex-notes/config.toml`

```toml
carpeta_notas = 'C:\Users\tu-usuario\Dropbox\Notas'  # por defecto: Dropbox/Notas
proveedor = "anthropic"    # anthropic, openai, gemini u ollama
modelo = "claude-haiku-4-5"
clave_api = ""             # o variable ANTHROPIC_API_KEY / OPENAI_API_KEY / GEMINI_API_KEY
ia_automatica = true
```

Sin clave API la app funciona igual; solo se desactiva la IA. Con `ollama` no hace falta clave.

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

## Compilar

```bash
cargo build --release
```

En Linux se necesitan `libxkbcommon-dev libgl1-mesa-dev libwayland-dev libx11-dev`.

Para publicar una versión, sube una etiqueta `vX.Y.Z`: GitHub Actions compila para los tres sistemas y adjunta los archivos al Release.

## Licencia

MIT
