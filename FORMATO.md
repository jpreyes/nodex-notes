# Formato de los archivos de Notas

Todo lo que escribes en Notas son archivos de texto en una carpeta tuya. Este documento explica
qué es cada archivo, para que puedas leerlos con otro programa, respaldarlos o llevártelos.
Ninguno está en un formato cerrado.

## La carpeta de notas

```text
Notas/
  Diario/
    2026-09-30.md          la nota de ese día («Hoy»)
  Adjuntos/
    captura-2026-09-30-171200.png   capturas pegadas en las notas
  Obra Talca/              un espacio de trabajo = una carpeta
    Muro.md                una nota = un archivo Markdown
  General/
  tareas.txt               las tareas
  agenda.txt               los eventos que salieron de las notas
  agenda.ics               lo mismo, para importar en un calendario
  aprendido.txt            lo que la IA aprendió de tus respuestas
  .papelera/               lo borrado (se puede restaurar)
  .nodex/                  datos internos de la app (ver abajo)
```

- Cada subcarpeta es un **espacio**, salvo `Diario` y `Adjuntos`, que están reservadas.
- Cada `.md` es una **nota**; su nombre de archivo es su título. Las notas del día se llaman con
  su fecha (`AAAA-MM-DD.md`).
- Los archivos están en UTF-8. Si se editan con otro programa, la app lo nota al instante.

## Dentro de una nota

Es Markdown, con unas pocas convenciones:

| Escrito así | Qué es |
|---|---|
| `Una línea sin sangría` | Una nota de adentro (la app las numera). |
| `␣␣sigue la de arriba` (2 espacios) | Detalle de la línea de arriba. |
| `␣␣- ítem`, `␣␣␣␣- subítem` | Ítems de lista (cada nivel, 2 espacios más). |
| `#palabra` | Etiqueta. |
| `- [ ] Enviar planos` / `- [x] …` | Tarea pendiente / hecha. |
| `due:2026-10-02` | Fecha de la tarea. |
| `^k3f9a` | Identificador que une la línea con su tarea en `tareas.txt`. |
| `## Reunión de obra · 2026-09-30 10:00` … `## fin · 10:42` | Una reunión; en medio, cada línea `- 10:05 texto` lleva su hora. Al cerrarla, la IA agrega `### Resumen`, decisiones y acuerdos. |
| `↻ Juan: enviar planos · vie 2 oct` | Acuerdo pendiente de la reunión anterior (en las reuniones que se repiten). |
| `![Captura 30 sep 17:12](../Adjuntos/captura-….png)` | Una imagen; la ruta es relativa a la nota. |
| `# Título`, `### Subtítulo` | Títulos. |

Un bloque que empieza con `## Título` va junto hasta `## fin`, el siguiente `##` o una línea en
blanco.

## tareas.txt

Una tarea por línea, en el formato [todo.txt](https://github.com/todotxt/todo.txt):

```text
2026-09-28 Enviar planos @Juan +Obra_Talca due:2026-10-02 nota:Obra%20Talca/Muro id:k3f9a
x 2026-09-30 2026-09-28 Pedir acero +Obra_Talca nota:Obra%20Talca/Muro id:ac002
```

- `x FECHA` al comienzo: hecha (y cuándo). La siguiente fecha es el día en que se creó.
- `+Espacio`: su espacio (los espacios del nombre van como `_`).
- `due:`: su fecha. `@Nombre`: de quién se espera (un acuerdo de otra persona).
- `nota:`: la nota de donde salió (relativa, sin `.md`; los espacios como `%20`).
- `id:`: el identificador de su línea en la nota (`^id`).
- `correo:`: el correo del que salió, si vino de uno.

## agenda.txt y agenda.ics

`agenda.txt` tiene un evento por línea: `2026-10-01 10:00 Visita a obra +Obra_Talca nota:Obra%20Talca/Muro`
(la hora es opcional). `agenda.ics` se genera de nuevo con cada cambio, con los eventos y las
tareas pendientes con fecha, para importarlo en Outlook, Google Calendar u otro.

## aprendido.txt

Una frase por línea (`- …`) con lo que la IA aprendió de tus respuestas y de tus «No, gracias».
La IA las lee en cada análisis. Puedes editarlas o borrarlas.

## .nodex/ (datos internos que comparten tus equipos)

| Archivo | Qué guarda |
|---|---|
| `analizadas.txt` | Huellas (FNV-1a, en hexadecimal) de los textos que la IA ya organizó, para no repetir. |
| `actividad.json` | «Lo que hizo» la IA: cada cambio, con sus detalles, si se deshizo o si no lo quisiste. |
| `dudas.json` | Las preguntas de la IA que esperan respuesta, y las ya respondidas. |
| `espacios.json` | Los temas sin espacio que la IA va juntando para sugerir uno nuevo. |
| `sugerencias.json` | Con «sugerir antes de aplicar»: lo que propone la IA y espera tu visto bueno. |
| `conversaciones/*.json` | Las conversaciones con la IA (una por archivo). |
| `recurrentes.json` | Las reuniones y notas que se repiten. |
| `papelera.txt` | De dónde vino cada cosa de la papelera (`nombre⇥ruta original⇥fecha`). |
| `diario-aparte.txt` | Notas del día que no se juntan en el Diario (se deshizo cuando se juntaron). |
| `correos-anotados.txt` | Qué equipo anotó cada correo (`mensaje⇥equipo`), para no repetirlo. |
| `organizando/` | Marcas de qué equipo está organizando cada nota (duran unos minutos). |
| `todo.json`, `google.json` | Qué tarea de Microsoft To Do o evento de Google corresponde a cada cosa. |
| `cuenta.clave` | La clave con que se cifra la configuración que viaja con tu cuenta. |

Si Dropbox deja una «copia en conflicto» de alguno de estos archivos, la app la junta sola
(salvo en `conversaciones/`, donde la copia queda como otra conversación).

## En cada equipo (fuera de la carpeta de notas)

En `%APPDATA%\nodex-notes\` (Windows), `~/Library/Application Support/nodex-notes/` (Mac) o
`~/.config/nodex-notes/` (Linux):

| Archivo | Qué guarda |
|---|---|
| `config.toml` | La configuración (carpeta, IA, correo, calendarios). Se puede editar a mano. |
| `estado.toml` | Lo último abierto: pestañas, espacio, tarjetas minimizadas. |
| `correos.json` | Los correos leídos y lo que la IA sacó de ellos. |
| `google_token.json`, `microsoft_token.json` | Los permisos de Google Calendar y Microsoft To Do. |
| `cuenta-sincronizada.json` | Cuándo cambió cada dato de la configuración que viaja con la cuenta. |
| `equipo.txt` | El identificador de este equipo. |
| `ia-ultima.txt`, `ia-pregunta.txt` | El último intercambio con la IA, para diagnosticar. |
| `actualizando.txt`, `novedades.txt` | Solo durante una actualización: la versión nueva espera a que la anterior termine de guardar, y después muestra qué trae. |

Además, una copia rápida de las notas para abrir al instante (`notas-….cache`) en la carpeta
local del sistema (`%LOCALAPPDATA%\nodex-notes\` en Windows). Se puede borrar: se vuelve a crear.
Ahí también, en `actualizacion/`, se baja la versión nueva hasta instalarla.

## Llevarse las notas

Basta con copiar la carpeta de notas: las notas son Markdown y se abren en cualquier editor
(Obsidian, VS Code, Typora…). Las tareas están en `tareas.txt`, en un formato que leen muchas
aplicaciones de tareas. Lo de `.nodex/` solo sirve a esta app.
