//! Ventana principal: barra de herramientas, barra lateral, editor, reuniones, IA, tareas y agenda.

use crate::agenda::{self, Agenda};
use crate::capture;
use crate::ai::{self, Ai, Analysis};
use crate::config::{self, Config, Estado};
use crate::doubts;
use crate::gcal::{self, GCal};
use crate::lines;
use crate::organize::{self, MEETING_TAG};
use crate::tags;
use crate::theme::{self, ACCENT, ACCENT_BG, BG_EDITOR, BG_RAIL, BG_SIDE, HOVER, MUTED, SUCCESS, TEXT};
use crate::vault::{self, Vault};
use chrono::{DateTime, Datelike, Local, NaiveDate};
use eframe::egui::{
    self, text::LayoutJob, Align, Align2, Color32, FontId, Frame, Id, Key, KeyboardShortcut, Layout, Margin,
    Modifiers, Response, RichText, Sense, Stroke, TextFormat, Ui, ViewportCommand,
};
use egui_phosphor::regular as icon;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

mod ai_view;
mod ask_view;
mod calendars_ui;
mod chats;
mod conflicts_ui;
mod diary;
mod doubts_ui;
mod editor;
mod manage;
mod followup;
mod home;
mod images;
mod mail_ui;
mod recurring;
#[cfg(test)]
mod rendimiento;
mod settings;
mod spaces_ui;
mod tabs;
mod tasks_sync;
mod trash_view;
#[cfg(test)]
mod two_devices_tests;
mod today;
mod todo_ui;
mod week;
use settings::Section;

/// Espera tras la última tecla antes de guardar.
const AUTOSAVE: Duration = Duration::from_millis(800);
/// Cada cuánto se revisa si algo cambió en disco (p. ej. por Dropbox).
const POLL: Duration = Duration::from_secs(1);
/// Una reunión sin notas nuevas durante este tiempo se cierra sola.
const MEETING_IDLE: Duration = Duration::from_secs(30 * 60);
/// La nota abierta se analiza con IA tras este tiempo sin escribir.
const AI_IDLE: Duration = Duration::from_secs(45);
/// El cursor de texto parpadea mientras se usa la app; tras este tiempo sin tocar nada queda
/// fijo, para que la app quieta no repinte varias veces por segundo solo por el parpadeo.
const CURSOR_REST: Duration = Duration::from_secs(10);
/// Cuántas líneas de resultados se muestran de una vez al buscar.
const FOUND_PAGE: usize = 300;
/// Cuánto vale la marca «este equipo está organizando esta nota» (segundos), y cuánto se
/// espera para volver a intentar una nota que tiene otro equipo.
const CLAIM_TTL: u64 = 180;
const CLAIM_RETRY: Duration = Duration::from_secs(60);
/// Tiempo durante el que se ofrece deshacer lo que hizo la IA.
const UNDO_WINDOW: Duration = Duration::from_secs(120);
/// Sincronización periódica con Google Calendar aunque no haya cambios.
const GCAL_EVERY: Duration = Duration::from_secs(10 * 60);
const EDITOR_SIZE: f32 = 15.5;
const COLUMN_MAX: f32 = 780.0;
const MESES: [&str; 12] = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"];
const DIAS: [&str; 7] = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];
const DIAS_CORTOS: [&str; 7] = ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"];
/// Primera nota de una carpeta nueva: cómo se usa la app.
const WELCOME: &str = "Escribe una idea por línea: cada línea es una nota distinta, y la IA la lleva después a su espacio.\n  Con Tab al comienzo, la línea se une a la nota de arriba, como un detalle\n  - con Tab dos veces se vuelve un ítem de lista\nPon #etiquetas donde quieras: se ven como píldoras de color #ejemplo\nEscribe tareas como las dirías, por ejemplo «enviar el informe el viernes»: la IA les pone casilla y fecha\nCtrl+R empieza una reunión: cada línea lleva su hora y Esc la cierra con un resumen\nCtrl+K abre la IA: pregúntale lo que quieras sobre tus notas\nCuando ya no la necesites, borra esta nota con el tacho que aparece al pasar el mouse en la barra lateral\n";
const RED: Color32 = Color32::from_rgb(198, 40, 40);
/// Avisos que no son errores (p. ej. Dropbox cerrado).
const WARN: Color32 = Color32::from_rgb(176, 104, 0);

struct OpenNote {
    path: PathBuf,
    /// Texto del campo de título (se aplica al archivo al salir del campo).
    title: String,
    text: String,
    dirty: bool,
    last_edit: Instant,
    /// Fecha de modificación y tamaño en disco cuando se leyó o guardó; `None` = aún no existe.
    disk_mtime: Option<SystemTime>,
    disk_len: Option<u64>,
    /// El texto tal como estaba en disco la última vez que se leyó o guardó: la base común
    /// para juntar lo escrito aquí con lo que llegue de otro equipo.
    base: String,
}

impl OpenNote {
    /// ¿Cambió el archivo por fuera desde la última vez que se leyó o guardó? Se mira la fecha
    /// de modificación y también el tamaño: Windows actualiza la fecha con una precisión de
    /// unos 16 ms, y un cambio justo después de guardar puede quedar con la misma fecha.
    fn changed_on_disk(&self) -> bool {
        let Ok(m) = fs::metadata(&self.path) else { return false };
        m.modified().ok() != self.disk_mtime || Some(m.len()) != self.disk_len
    }

    /// Recuerda cómo quedó el archivo en disco (después de leerlo o guardarlo).
    fn remember_disk(&mut self) {
        let m = fs::metadata(&self.path).ok();
        self.disk_mtime = m.as_ref().and_then(|m| m.modified().ok());
        self.disk_len = m.map(|m| m.len());
    }

    fn load(path: PathBuf) -> Self {
        let text = vault::read_text(&path).unwrap_or_default();
        let meta = fs::metadata(&path).ok();
        OpenNote {
            base: text.clone(),
            text,
            disk_mtime: meta.as_ref().and_then(|m| m.modified().ok()),
            disk_len: meta.map(|m| m.len()),
            title: vault::stem(&path),
            path,
            dirty: false,
            last_edit: Instant::now(),
        }
    }
}

struct Meeting {
    path: PathBuf,
    title: String,
    started: DateTime<Local>,
    last_activity: Instant,
    last_time: DateTime<Local>,
}

/// Una nota con líneas que coinciden con la búsqueda (o con la etiqueta).
struct Hit {
    path: PathBuf,
    title: String,
    ws: String,
    /// (posición del cursor al final de la línea, texto de la línea)
    lines: Vec<(usize, String)>,
}

/// Resultados guardados de la última búsqueda.
struct Found {
    /// Lo buscado (normalizado), la etiqueta y el espacio.
    key: String,
    /// Estado de las notas cuando se buscó.
    generation: u64,
    tag: Option<String>,
    hits: Vec<Hit>,
    /// Cuántas líneas se muestran («Mostrar más» agrega otras).
    shown: usize,
}

/// Una nota en la lista de la barra lateral.
struct SideNote {
    path: PathBuf,
    /// Cómo se muestra ("Reunión CIC", o "Vie 25 sep" en el Diario).
    title: String,
    /// La fecha a la derecha ("15:34", "25 sep").
    right: String,
    meeting: bool,
}

/// Lo que muestran la barra lateral e Inicio, calculado una sola vez por cada cambio en las
/// notas (o de espacio, o de día), en vez de en cada repintado.
struct Summary {
    /// (estado de las notas, espacio, día) con que se calculó.
    key: (u64, String, String),
    /// Notas del espacio con título (la más reciente primero) y del Diario (la más nueva primero).
    others: Vec<SideNote>,
    diary: Vec<SideNote>,
    /// ¿Existe ya la nota de hoy?
    today_exists: bool,
    tags: Vec<(String, usize)>,
    /// Inicio: notas recientes (ruta, título, espacio, fecha).
    recent: Vec<(PathBuf, String, String, SystemTime)>,
    /// Inicio: reuniones recientes (ruta, título, fecha).
    meetings: Vec<(PathBuf, String, SystemTime)>,
    /// Inicio: notas por espacio y notas escritas en los últimos 7 días.
    spaces: Vec<(String, usize)>,
    written_week: usize,
}

impl Summary {
    fn build(vault: &Vault, key: (u64, String, String)) -> Summary {
        let (ws, today_s) = (&key.1, &key.2);
        let today_path = vault.diary_path(today_s);
        let today_exists = vault.get(&today_path).is_some();
        let mut others = Vec::new();
        let mut diary = Vec::new();
        // El Diario es uno solo; las notas del día que queden en el espacio (sin juntar) también van ahí.
        for n in vault.notes_in(ws).into_iter().chain(vault.notes_in(vault::DIARY)) {
            if n.path == today_path {
                continue;
            }
            let daily = n.workspace == vault::DIARY || agenda::is_date(&n.title);
            let row = SideNote {
                path: n.path.clone(),
                title: if daily { display_title(&n.title) } else { n.title.clone() },
                right: if daily { String::new() } else { short_date(n.modified) },
                meeting: n.meeting(is_meeting),
            };
            if daily {
                diary.push((n.title.clone(), row));
            } else {
                others.push(row);
            }
        }
        diary.sort_by(|a, b| b.0.cmp(&a.0));
        let all = vault.all_notes();
        let week_ago = (Local::now() - chrono::Duration::days(6)).format("%Y-%m-%d").to_string();
        let mut per_ws: HashMap<&str, usize> = HashMap::new();
        for n in &all {
            *per_ws.entry(n.workspace.as_str()).or_default() += 1;
        }
        Summary {
            others,
            diary: diary.into_iter().map(|(_, r)| r).collect(),
            today_exists,
            tags: vault.tag_counts(ws),
            recent: all.iter().take(8).map(|n| (n.path.clone(), display_title(&n.title), n.workspace.clone(), n.modified)).collect(),
            meetings: all.iter().filter(|n| n.meeting(is_meeting)).take(5).map(|n| (n.path.clone(), n.title.clone(), n.modified)).collect(),
            spaces: vault.workspaces.iter().map(|w| (w.clone(), per_ws.get(w.as_str()).copied().unwrap_or(0))).collect(),
            written_week: all.iter().filter(|n| n.day() >= week_ago.as_str()).count(),
            key,
        }
    }
}

/// Diagnóstico: con la variable NODEX_CUADROS, escribe cada 5 segundos cuántos cuadros se
/// dibujaron (para ver si algo repinta de más con la app quieta).
fn count_frame(ctx: &egui::Context) {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("NODEX_CUADROS").is_some()) {
        return;
    }
    static FRAMES: AtomicU64 = AtomicU64::new(0);
    static SINCE: OnceLock<std::sync::Mutex<Instant>> = OnceLock::new();
    let n = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
    let mut since = SINCE.get_or_init(|| std::sync::Mutex::new(Instant::now())).lock().expect("contador");
    if since.elapsed() >= Duration::from_secs(5) {
        eprintln!("CUADROS {:.1} por segundo; los pidió: {:?}", n as f64 / since.elapsed().as_secs_f64(), ctx.repaint_causes());
        FRAMES.store(0, Ordering::Relaxed);
        *since = Instant::now();
    }
}

/// Alto de una fila de lista (`list_row`).
const ROW_H: f32 = 27.0;

/// Las filas de una lista larga que caen en la parte visible. Reserva de una sola vez el lugar
/// de las de arriba y devuelve cuáles dibujar; después se llama a `skip_rows` con las de abajo.
/// Así no se dibujan (ni se recorren) miles de filas que nadie ve.
fn visible_rows(ui: &mut Ui, n: usize) -> std::ops::Range<usize> {
    let row = ROW_H + ui.spacing().item_spacing.y;
    let top = ui.cursor().top();
    let clip = ui.clip_rect();
    let first = (((clip.top() - top) / row).floor().max(0.0) as usize).min(n);
    let last = ((((clip.bottom() - top) / row).ceil().max(0.0)) as usize + 1).min(n).max(first);
    skip_rows(ui, first);
    first..last
}

/// Reserva el lugar de `k` filas sin dibujarlas.
fn skip_rows(ui: &mut Ui, k: usize) {
    if k > 0 {
        let gap = ui.spacing().item_spacing.y;
        ui.allocate_exact_size(egui::vec2(ui.available_width(), k as f32 * (ROW_H + gap) - gap), Sense::hover());
    }
}

/// Lo necesario para revertir el último cambio automático de la IA.
struct Undo {
    /// Contenido previo de cada archivo tocado (`None` = el archivo no existía).
    files: Vec<(PathBuf, Option<String>)>,
    /// Nota renombrada o movida de espacio: (ruta original, ruta nueva).
    renamed: Option<(PathBuf, PathBuf)>,
    agenda: (String, String),
    at: Instant,
    /// Otras notas movidas completas: (ruta original, ruta nueva).
    moved: Vec<(PathBuf, PathBuf)>,
    /// Carpeta de un espacio creado (se borra al deshacer, si quedó vacía).
    created_dir: Option<PathBuf>,
    /// Notas del día que se habían juntado en el Diario: al deshacer, quedan aparte para siempre.
    apart: Vec<String>,
    /// Rutas que cambiaron (nota o espacio movido: antes, después): al deshacer, todo lo que
    /// apuntaba ahí vuelve a apuntar a la ruta de antes.
    relinks: Vec<(String, String)>,
}

#[derive(PartialEq, Clone, Debug)]
enum View {
    Editor,
    Home,
    Mail,
    Week,
    Ai,
    Tag(String),
    Tasks,
    Agenda,
    Trash,
}

enum Action {
    Open(PathBuf, Option<usize>),
    NewNote,
    Today,
    SelectWorkspace(String),
    CreateWorkspace(String),
    Trash(PathBuf),
    /// Pedir confirmación para mover un espacio a la papelera.
    AskTrashWorkspace(String),
    /// Mover una nota a otro espacio.
    MoveNote(PathBuf, String),
    /// Cambiar el nombre de un espacio o de una etiqueta (antes, nombre nuevo).
    RenameWorkspace(String, String),
    RenameTag(String, String),
    /// Dejar una nota completa como tarea.
    NoteToTask(PathBuf),
    /// La ventana de reuniones y notas recurrentes.
    OpenRecurring,
    /// Devolver algo de la papelera a su lugar.
    Restore(crate::vault::Trashed),
    ShowTag(String),
    Show(View),
    /// La ventana de la IA, en una de sus secciones.
    ShowAi(ai_view::AiTab),
    CloseResults,
    FocusSearch,
    StartMeeting,
    CloseMeeting,
    Organize,
    Undo,
    GoogleConnect,
    GoogleSync,
    GoogleDisconnect,
    /// Microsoft To Do.
    TodoConnect,
    TodoSync,
    TodoDisconnect,
    ToggleTask(String),
    AddTask(String),
    OpenExternal(PathBuf),
    OpenSettings(Section),
    /// Correo.
    AddMailAccount(crate::mail::Account),
    /// Cambiar la conexión de una cuenta (clave vacía = se mantiene la actual).
    UpdateMailAccount(usize, crate::mail::Account),
    RemoveMailAccount(usize),
    TestMailAccount(usize),
    CheckMail,
    MailFulfill(String, usize, bool),
    MailRemoveItems(String),
    MailSaveNote(String),
    OpenMail(String),
    /// Calendarios agregados (enlace ICS).
    AddCalendar(String, String),
    /// Cambiar el nombre o el enlace de un calendario agregado.
    EditCalendar(usize, String, String),
    RemoveCalendar(usize),
    RefreshCalendars,
    /// Tomar notas de un evento: una reunión con ese título.
    StartMeetingNamed(String),
    /// Pestañas.
    OpenNewTab(PathBuf),
    ShowTab(View),
    NewTab,
    CloseTab,
    NextTab(bool),
    ActivateTab(usize),
}

pub struct NotesApp {
    cfg: Config,
    ctx: egui::Context,
    /// Ventana de Configuración abierta.
    settings: Option<settings::Settings>,
    vault: Vault,
    agenda: Agenda,
    ws: String,
    note: OpenNote,
    view: View,
    search: String,
    /// Nombre del espacio que se está creando (campo en la barra lateral).
    new_ws: Option<String>,
    /// Espacio al que se le está cambiando el nombre (nombre actual, campo), y lo mismo para una etiqueta.
    renaming_ws: Option<(String, String)>,
    renaming_tag: Option<(String, String)>,
    /// La línea del editor donde se hizo clic derecho (para su menú).
    menu_line: Option<usize>,
    /// Conversar tiene espacio para la lista de conversaciones al lado.
    chats_wide: bool,
    /// Tarjetas de Inicio minimizadas (por su título).
    home_closed: HashSet<String>,
    /// Tareas por fecha límite (si no, lo más reciente primero).
    tasks_by_due: bool,
    /// Imágenes pegadas en las notas, ya cargadas.
    images: images::Images,
    /// IA incluida: cuánto va del mes (lo que dijo el servidor), el pedido en curso, cuándo
    /// se pidió y el mes en que ya se avisó que queda poco.
    ai_usage: Option<Result<ai::Usage, String>>,
    ai_usage_rx: Option<std::sync::mpsc::Receiver<Result<ai::Usage, String>>>,
    ai_usage_at: Instant,
    ai_usage_warned: String,
    /// Último minuto en que se revisaron las recurrentes, y su ventana (abierta si hay formulario).
    recurring_checked: String,
    recurring_form: Option<recurring::Form>,
    /// Borrar para siempre: qué, y si se marcó «Entiendo que no se puede recuperar».
    forever: Option<(trash_view::Forever, bool)>,
    new_task: String,
    message: Option<(String, Instant)>,
    last_poll: Instant,
    window_title: String,
    focus_editor: bool,
    focus_title: bool,
    focus_search: bool,
    /// Posición (en caracteres) donde dejar el cursor al abrir una nota.
    pending_cursor: Option<usize>,
    meeting: Option<Meeting>,
    ai: Result<Ai, String>,
    ai_auto: bool,
    /// Hashes de contenidos ya analizados (se guarda en .nodex/analizadas.txt).
    analyzed: HashSet<u64>,
    /// Cómo estaba ese archivo la última vez que se leyó o escribió, y cuándo se revisaron
    /// los datos internos por cambios de otro equipo.
    analyzed_base: HashSet<u64>,
    stores_at: Instant,
    /// Notas escritas en esta sesión, candidatas al análisis automático.
    touched: HashSet<PathBuf>,
    /// Cola del botón Organizar.
    backlog: VecDeque<PathBuf>,
    backlog_total: usize,
    in_flight: Option<PathBuf>,
    undo: Option<Undo>,
    gcal: Option<GCal>,
    /// Microsoft To Do: el hilo, cuándo se sincronizó y cómo estaba tareas.txt entonces.
    todo: Option<crate::todo::ToDo>,
    todo_last: Instant,
    todo_hash: u64,
    /// La agenda cambió y hay que sincronizar con Google.
    gcal_dirty: bool,
    gcal_last_try: Instant,
    /// Rutas y webs encontradas en las líneas (para no buscarlas en disco en cada cuadro).
    links: editor::LinkCache,
    /// Día en que ya se mostró la vista "Hoy" al abrir.
    today_shown: String,
    /// Conversación de Preguntar.
    ask: ask_view::AskState,
    /// Preguntas de la IA pendientes (.nodex/dudas.json).
    doubts: doubts::Store,
    /// Respuesta escrita que se está redactando: (id de la pregunta, texto).
    doubt_reply: Option<(String, String)>,
    /// Temas sin espacio que la IA fue encontrando (.nodex/espacios.json).
    ideas: crate::spaces::Ideas,
    /// Revisión semanal: resumen de la IA.
    week: week::WeekState,
    /// Última semana en que se abrió la revisión (AAAA-Wnn).
    week_seen: String,
    /// Borrador del correo de seguimiento de una reunión.
    followup: followup::FollowUp,
    tabs: tabs::Tabs,
    /// Calendarios agregados: lo descargado y el formulario para agregar uno.
    cals: crate::calendars::Calendars,
    cal_form: Option<(String, String, Option<usize>)>,
    /// Correo: lo leído, lo que falta que lea la IA y su estado.
    mail: mail_ui::MailState,
    /// Espacio que se quiere mover a la papelera (esperando confirmación).
    confirm_ws: Option<String>,
    /// Campos de Inicio: anotar y preguntar rápido.
    home_capture: String,
    home_question: String,
    /// "Diario" (las notas de días anteriores) abierto en la barra lateral.
    diary_open: bool,
    /// Si la carpeta de notas está en Dropbox: si la app de Dropbox está abierta (para avisar).
    dropbox: Option<crate::dropbox::Watch>,
    /// Estado de las notas la última vez que se emparejaron las tareas con sus líneas, y cuándo.
    tasks_gen: Option<u64>,
    tasks_at: Instant,
    /// Lo mismo para juntar en el Diario las notas del día que quedaron en los espacios.
    days_gen: Option<u64>,
    days_at: Instant,
    /// Identificador de este equipo, y las notas que otro equipo está organizando (se reintentan después).
    machine: String,
    claim_wait: HashMap<PathBuf, Instant>,
    /// Copias en conflicto de Dropbox vistas (y desde cuándo), y cuándo se buscaron.
    conflict_seen: HashMap<PathBuf, Instant>,
    conflicts_gen: u64,
    conflicts_at: Instant,
    /// Última vez que se tocó el teclado o el mouse en la ventana, y si el cursor parpadea.
    last_input: Instant,
    cursor_blinks: bool,
    /// Resultados de la última búsqueda (se reutilizan mientras no cambie nada).
    found: Option<Found>,
    /// Lo que muestran la barra lateral e Inicio (se recalcula solo si algo cambió).
    summary: Option<std::rc::Rc<Summary>>,
    /// Notas sin organizar: ((estado de las notas, notas analizadas), cuántas).
    unorganized_count: Option<((u64, usize), usize)>,
    /// Ventana de la IA: sección abierta, lo que hizo y cuál de sus cambios se puede deshacer.
    ai_tab: ai_view::AiTab,
    activity: crate::activity::Log,
    undo_entry: Option<String>,
    /// Último error de la IA al organizar (se muestra en su ventana).
    ai_error: Option<String>,
    /// Aviso de lo que la IA acaba de hacer sola.
    toast: Option<ai_view::Toast>,
}

/// Un instante "hace mucho" (sin pasar por debajo del arranque del equipo).
fn long_ago() -> Instant {
    let now = Instant::now();
    now.checked_sub(GCAL_EVERY).unwrap_or(now)
}

/// Hashes de las notas que la IA ya analizó (.nodex/analizadas.txt).
fn load_analyzed(root: &Path) -> HashSet<u64> {
    vault::read_text(&root.join(".nodex").join("analizadas.txt"))
        .map(|t| t.lines().filter_map(|l| u64::from_str_radix(l.trim(), 16).ok()).collect())
        .unwrap_or_default()
}

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// El espacio de una nota; `None` para las notas del día (no son de ningún espacio).
fn workspace_of(path: &Path) -> Option<String> {
    if vault::in_diary(path) {
        return None;
    }
    Some(path.parent()?.file_name()?.to_string_lossy().into_owned())
}

fn short_date(t: SystemTime) -> String {
    let d: DateTime<Local> = t.into();
    let now = Local::now();
    if d.date_naive() == now.date_naive() {
        d.format("%H:%M").to_string()
    } else if d.year() == now.year() {
        format!("{} {}", d.day(), MESES[d.month0() as usize])
    } else {
        d.format("%Y-%m-%d").to_string()
    }
}

/// "2026-09-26" -> "viernes 26 sep"
fn long_date(date: &str) -> String {
    match NaiveDate::parse_from_str(date, "%Y-%m-%d") {
        Ok(d) => format!(
            "{} {} {}",
            DIAS[d.weekday().num_days_from_monday() as usize],
            d.day(),
            MESES[d.month0() as usize]
        ),
        Err(_) => date.to_string(),
    }
}

/// Nombre para mostrar de una nota: las del día ("2026-09-27") son "Hoy", "Ayer", "Mañana" o
/// "Vie 25 sep"; las demás, su título.
fn display_title(title: &str) -> String {
    let Some(d) = Some(title).filter(|t| agenda::is_date(t)).and_then(|t| NaiveDate::parse_from_str(t, "%Y-%m-%d").ok()) else {
        return title.to_string();
    };
    let today = Local::now().date_naive();
    match (d - today).num_days() {
        0 => "Hoy".into(),
        -1 => "Ayer".into(),
        1 => "Mañana".into(),
        _ => {
            let year = if d.year() != today.year() { format!(" {}", d.year()) } else { String::new() };
            let day = DIAS_CORTOS[d.weekday().num_days_from_monday() as usize];
            let mut s = format!("{day} {} {}{year}", d.day(), MESES[d.month0() as usize]);
            if let Some(f) = s.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            s
        }
    }
}

/// Título grande de una nota del día: "Hoy, domingo 27 sep"; `None` si no es una nota del día.
fn day_heading(title: &str) -> Option<String> {
    let d = NaiveDate::parse_from_str(title, "%Y-%m-%d").ok().filter(|_| agenda::is_date(title))?;
    let long = format!("{} {} {}", DIAS[d.weekday().num_days_from_monday() as usize], d.day(), MESES[d.month0() as usize]);
    Some(match display_title(title).as_str() {
        s @ ("Hoy" | "Ayer" | "Mañana") => format!("{s}, {long}"),
        _ => {
            let mut s = long;
            if let Some(f) = s.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            if d.year() != Local::now().year() {
                s += &format!(" {}", d.year());
            }
            s
        }
    })
}

fn open_external(path: &Path) {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(path).spawn();
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

/// Línea de reunión sin texto: "- 15:03".
fn is_empty_stamp(line: &str) -> bool {
    line.trim().strip_prefix("- ").is_some_and(|t| agenda::is_time(t.trim()))
}

/// Encabezado "## Título · 2026-09-24 15:00" -> (título, fecha).
fn meeting_header(text: &str) -> Option<(String, String)> {
    let first = text.lines().find(|l| !l.trim().is_empty())?;
    let (title, when) = first.trim().strip_prefix("## ")?.rsplit_once(" · ")?;
    Some((title.trim().to_string(), when.trim().to_string()))
}

fn is_meeting(text: &str) -> bool {
    meeting_header(text).is_some() || text.lines().any(|l| tags::line_tags(l).iter().any(|t| t == MEETING_TAG))
}

fn byte_index(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(b, _)| b)
}

/// Identificador corto para unir una tarea con su línea en la nota ("k3f9a").
fn new_task_id() -> String {
    let mut b = [0u8; 5];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| char::from(b"0123456789abcdefghijklmnopqrstuvwxyz"[(*x % 36) as usize])).collect()
}

impl NotesApp {
    pub fn new(cfg: Config, cfg_msg: Option<String>, ctx: egui::Context) -> Self {
        let mut message = cfg_msg;
        if let Err(e) = fs::create_dir_all(&cfg.carpeta_notas) {
            message = Some(format!("No se pudo crear {}: {e}", cfg.carpeta_notas.display()));
        }
        let ai = Ai::start(&cfg, ctx.clone());
        let gcal = GCal::start(&cfg.google_client_id, &cfg.google_client_secret, cfg.carpeta_notas.clone(), ctx.clone());
        let ai_auto = cfg.ia_automatica;
        let cfg_root = cfg.carpeta_notas.clone();
        let vault = Vault::new(cfg.carpeta_notas.clone());
        let agenda = Agenda::new(&vault.root);
        let estado = config::load_estado();
        let ws = if vault.workspaces.contains(&estado.espacio) {
            estado.espacio
        } else {
            vault.workspaces.first().cloned().unwrap_or_else(|| vault::DEFAULT_WORKSPACE.into())
        };
        let last = vault.root.join(&estado.nota);
        let path = if !estado.nota.is_empty() && last.is_file() { last } else { vault.diary_path(&today()) };
        let ws = workspace_of(&path).unwrap_or(ws);
        let mut analyzed = load_analyzed(&vault.root);
        let mut vault = vault;
        let welcome = if vault.all_notes().is_empty() {
            let path = vault.note_path(vault::DEFAULT_WORKSPACE, "Bienvenida");
            let _ = fs::create_dir_all(path.parent().unwrap_or(&vault.root));
            let ok = fs::write(&path, WELCOME).is_ok();
            analyzed.insert(ai::fnv(WELCOME)); // no hay nada que organizar
            vault.scan();
            ok.then_some(path)
        } else {
            None
        };
        let estado_hoy = estado.hoy.clone();
        let estado_semana = estado.semana.clone();
        let home_closed: HashSet<String> = estado.inicio_cerradas.iter().cloned().collect();
        let tasks_by_due = estado.tareas_por_fecha;
        let estado_tabs = estado.pestanas.clone();
        let estado_tab = estado.pestana;
        let mut app = NotesApp {
            cfg,
            ctx: ctx.clone(),
            settings: None,
            vault,
            agenda,
            ws,
            note: OpenNote::load(path),
            view: View::Editor,
            search: String::new(),
            new_ws: None,
            renaming_ws: None,
            renaming_tag: None,
            menu_line: None,
            chats_wide: true,
            forever: None,
            new_task: String::new(),
            message: message.map(|m| (m, Instant::now())),
            last_poll: Instant::now(),
            window_title: String::new(),
            focus_editor: true,
            focus_title: false,
            focus_search: false,
            pending_cursor: Some(usize::MAX),
            meeting: None,
            ai,
            ai_auto,
            analyzed_base: analyzed.clone(),
            analyzed,
            stores_at: Instant::now(),
            touched: HashSet::new(),
            backlog: VecDeque::new(),
            backlog_total: 0,
            in_flight: None,
            undo: None,
            gcal,
            todo: crate::todo::ToDo::start(cfg_root.clone(), ctx.clone()),
            todo_last: long_ago(),
            todo_hash: 0,
            gcal_dirty: true,
            gcal_last_try: long_ago(),
            links: editor::LinkCache::new(&cfg_root),
            today_shown: estado_hoy,
            ask: ask_view::AskState::default(),
            doubts: doubts::Store::load(&cfg_root),
            doubt_reply: None,
            ideas: crate::spaces::Ideas::load(&cfg_root),
            week: week::WeekState::default(),
            week_seen: estado_semana,
            home_closed,
            tasks_by_due,
            images: images::Images::default(),
            ai_usage: None,
            ai_usage_rx: None,
            ai_usage_at: long_ago(),
            ai_usage_warned: String::new(),
            recurring_checked: String::new(),
            recurring_form: None,
            followup: followup::FollowUp::default(),
            tabs: tabs::Tabs::default(),
            cals: crate::calendars::Calendars::load(),
            cal_form: None,
            mail: mail_ui::MailState::load(),
            confirm_ws: None,
            home_capture: String::new(),
            home_question: String::new(),
            diary_open: false,
            dropbox: crate::dropbox::contains(&cfg_root).then(|| crate::dropbox::Watch::start(ctx.clone())),
            tasks_gen: None,
            tasks_at: long_ago(),
            days_gen: None,
            days_at: long_ago(),
            machine: crate::claims::machine_id(),
            claim_wait: HashMap::new(),
            conflict_seen: HashMap::new(),
            conflicts_gen: u64::MAX,
            conflicts_at: Instant::now(),
            last_input: Instant::now(),
            cursor_blinks: true,
            found: None,
            summary: None,
            unorganized_count: None,
            ai_tab: ai_view::AiTab::default(),
            activity: crate::activity::Log::load(&cfg_root),
            undo_entry: None,
            ai_error: None,
            toast: None,
        };
        // Pestañas de la sesión anterior (o la nota que estaba abierta).
        let decoded: Vec<tabs::Tab> = estado_tabs.iter().filter_map(|t| tabs::decode(t, &app.vault.root)).collect();
        // Sin repetidas (versiones anteriores podían dejar la misma nota en varias pestañas).
        let was_active = decoded.get(estado_tab).cloned();
        let mut saved: Vec<tabs::Tab> = Vec::new();
        for t in decoded {
            if !saved.contains(&t) {
                saved.push(t);
            }
        }
        app.tabs = if saved.is_empty() {
            tabs::Tabs { list: vec![tabs::Tab::View(View::Home), tabs::Tab::Note(app.note.path.clone())], active: 1 }
        } else {
            let active = was_active.and_then(|t| saved.iter().position(|x| *x == t)).unwrap_or(0);
            tabs::Tabs { list: saved, active }
        };
        app.prune_doubts();
        app.prune_ideas();
        // La primera vez de cada día se abre en Inicio (el resumen de todo).
        if app.today_shown != today() {
            app.today_shown = today();
            match app.tabs.list.iter().position(|t| *t == tabs::Tab::View(View::Home)) {
                Some(i) => app.tabs.active = i,
                None => {
                    app.tabs.list.insert(0, tabs::Tab::View(View::Home));
                    app.tabs.active = 0;
                }
            }
        }
        // Primera vez: se abre la bienvenida.
        if let Some(p) = welcome {
            app.save_analyzed();
            app.tabs = tabs::Tabs { list: vec![tabs::Tab::View(View::Home), tabs::Tab::Note(p)], active: 1 };
        }
        let active = app.tabs.active;
        app.activate_tab(active);
        app.focus_editor = app.view == View::Editor;
        // Solo en compilaciones de prueba: abrir Preguntar con una pregunta (para capturas).
        #[cfg(debug_assertions)]
        if let Ok(q) = std::env::var("NODEX_DEMO_ASK") {
            app.view = View::Ai;
            app.ask(q);
        }
        #[cfg(debug_assertions)]
        if let Ok(t) = std::env::var("NODEX_DEMO_AI") {
            app.ai_tab = match t.as_str() {
                "preguntas" => ai_view::AiTab::Asks,
                "hizo" => ai_view::AiTab::Log,
                _ => ai_view::AiTab::Chat,
            };
            app.view = View::Ai;
        }
        #[cfg(debug_assertions)]
        if std::env::var("NODEX_DEMO_SETTINGS").is_ok() {
            app.open_settings(Section::Tasks);
        }
        #[cfg(debug_assertions)]
        if std::env::var("NODEX_DEMO_TOAST").is_ok() {
            let details = vec!["→ Consorcio/Trincheras".to_string(), "Etiquetas: planos".into(), "Tarea: Enviar planos corregidos · martes 29 sep".into()];
            let text = "Organizó «2026-09-27»: 1 etiqueta · 1 nota → Consorcio/Trincheras · 1 tarea".to_string();
            app.log_ai(crate::activity::Kind::Organizar, "Consorcio/Trincheras", text, details, false);
        }
        #[cfg(debug_assertions)]
        if std::env::var("NODEX_DEMO_WEEK").is_ok() {
            app.view = View::Week;
            app.start_week_summary();
        }
        app
    }

    fn msg(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), Instant::now()));
    }

    /// Esc para las vistas, salvo que la ventana de Configuración esté encima.
    fn esc(&self, ui: &Ui) -> bool {
        self.settings.is_none() && ui.input(|i| i.key_pressed(Key::Escape))
    }

    // ---------- Configuración aplicada en vivo ----------

    fn save_config(&mut self) {
        if let Err(e) = config::save(&self.cfg) {
            self.msg(format!("No se pudo guardar la configuración: {e}"));
        }
    }

    fn restart_ai(&mut self) {
        self.ai = Ai::start(&self.cfg, self.ctx.clone());
        self.ai_auto = self.cfg.ia_automatica;
        self.in_flight = None;
        self.backlog.clear();
    }

    fn restart_gcal(&mut self) {
        let (id, secret) = (&self.cfg.google_client_id, &self.cfg.google_client_secret);
        self.gcal = GCal::start(id, secret, self.vault.root.clone(), self.ctx.clone());
        self.gcal_dirty = true;
    }

    /// Cambia la carpeta de notas sin reiniciar la app.
    fn change_folder(&mut self, path: PathBuf) {
        if path == self.vault.root {
            return;
        }
        if let Err(e) = fs::create_dir_all(&path) {
            self.msg(format!("No se pudo usar {}: {e}", path.display()));
            return;
        }
        self.close_meeting(Local::now());
        self.save();
        self.vault.save_cache_now();
        self.cfg.carpeta_notas = path.clone();
        self.save_config();
        self.dropbox = crate::dropbox::contains(&path).then(|| crate::dropbox::Watch::start(self.ctx.clone()));
        self.vault = Vault::new(path);
        self.agenda = Agenda::new(&self.vault.root);
        self.links = editor::LinkCache::new(&self.vault.root);
        self.doubts = doubts::Store::load(&self.vault.root);
        self.ideas = crate::spaces::Ideas::load(&self.vault.root);
        self.activity = crate::activity::Log::load(&self.vault.root);
        self.undo_entry = None;
        self.analyzed = load_analyzed(&self.vault.root);
        self.analyzed_base = self.analyzed.clone();
        self.touched.clear();
        self.backlog.clear();
        self.in_flight = None;
        self.undo = None;
        self.restart_gcal();
        self.todo = crate::todo::ToDo::start(self.vault.root.clone(), self.ctx.clone());
        self.todo_hash = 0;
        let ws = self.vault.workspaces.first().cloned().unwrap_or_else(|| vault::DEFAULT_WORKSPACE.into());
        self.note.dirty = false;
        self.select_workspace(ws);
        self.msg(format!("Carpeta de notas: {}", self.vault.root.display()));
    }

    fn rel(&self, path: &Path) -> String {
        let rel = path.strip_prefix(&self.vault.root).unwrap_or(path).with_extension("");
        rel.to_string_lossy().replace('\\', "/")
    }

    // ---------- Archivos ----------

    fn save(&mut self) {
        let n = &mut self.note;
        if !n.dirty {
            return;
        }
        if n.text.is_empty() && n.disk_mtime.is_none() {
            n.dirty = false; // nota nueva vacía: no se crea el archivo
            return;
        }
        if let Some(dir) = n.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        // Si el archivo cambió por fuera desde la última lectura (otro equipo, Dropbox), no se
        // pisa: se junta lo de aquí con lo de allá, línea por línea.
        let mut merged = None;
        if n.changed_on_disk() {
            if let Ok(disk) = vault::read_text(&n.path) {
                if disk != n.base && disk != n.text {
                    let m = crate::merge::merge3(&n.base, &n.text, &disk);
                    n.text = m.text;
                    merged = Some(m.conflicts);
                }
            }
        }
        match fs::write(&n.path, &n.text) {
            Ok(()) => {
                n.dirty = false;
                n.base = n.text.clone();
                n.remember_disk();
                if let Some(m) = n.disk_mtime {
                    self.vault.upsert(n.path.clone(), n.text.clone(), m);
                }
                self.touched.insert(self.note.path.clone());
                if let Some(conflicts) = merged {
                    self.merged_msg(conflicts);
                }
            }
            Err(e) => {
                n.last_edit = Instant::now(); // reintenta en el próximo ciclo
                self.msg(format!("No se pudo guardar: {e}"));
            }
        }
    }

    /// Aviso de que se juntó lo escrito aquí con lo que llegó de otro equipo.
    fn merged_msg(&mut self, conflicts: usize) {
        self.msg(match conflicts {
            0 => "Se juntaron tus cambios con los que llegaron de otro equipo".to_string(),
            1 => "Se juntaron tus cambios con los de otro equipo; una parte cambió en los dos: quedaron las dos versiones, revísala".to_string(),
            n => format!("Se juntaron tus cambios con los de otro equipo; {n} partes cambiaron en los dos: quedaron las dos versiones, revísalas"),
        });
    }

    fn save_estado(&mut self) {
        let nota = self.note.path.strip_prefix(&self.vault.root).unwrap_or(&self.note.path);
        let r = config::save_estado(&Estado {
            espacio: self.ws.clone(),
            nota: nota.to_string_lossy().into_owned(),
            hoy: self.today_shown.clone(),
            semana: self.week_seen.clone(),
            pestanas: self.tabs.list.iter().map(|t| tabs::encode(t, &self.vault.root)).collect(),
            pestana: self.tabs.active,
            inicio_cerradas: {
                let mut v: Vec<String> = self.home_closed.iter().cloned().collect();
                v.sort();
                v
            },
            tareas_por_fecha: self.tasks_by_due,
        });
        if let Err(e) = r {
            self.msg(format!("No se pudo guardar estado.toml: {e}"));
        }
    }

    /// Trae las notas que otro equipo marcó (o desmarcó) como analizadas desde la última vez.
    fn sync_analyzed(&mut self) {
        let file = self.vault.root.join(".nodex").join("analizadas.txt");
        if !file.is_file() {
            return;
        }
        let disk = load_analyzed(&self.vault.root);
        for h in self.analyzed_base.difference(&disk) {
            self.analyzed.remove(h); // el otro la quitó (para volver a analizarla)
        }
        for h in disk.difference(&self.analyzed_base) {
            self.analyzed.insert(*h); // el otro la analizó
        }
        self.analyzed_base = disk;
    }

    fn save_analyzed(&mut self) {
        self.sync_analyzed();
        let dir = self.vault.root.join(".nodex");
        let _ = fs::create_dir_all(&dir);
        let mut lines: Vec<String> = self.analyzed.iter().map(|h| format!("{h:016x}")).collect();
        lines.sort();
        if fs::write(dir.join("analizadas.txt"), lines.join("\n") + "\n").is_ok() {
            self.analyzed_base = self.analyzed.clone();
        }
    }

    /// Los datos internos que comparten los equipos (`.nodex/`): trae lo que cambió otro.
    fn sync_stores(&mut self) {
        self.stores_at = Instant::now();
        let root = self.vault.root.clone();
        self.doubts.sync(&root);
        self.ideas.sync(&root);
        self.activity.sync(&root);
        self.sync_analyzed();
    }

    fn open(&mut self, path: PathBuf, cursor: Option<usize>) {
        self.save();
        if self.note.path != path {
            self.note = OpenNote::load(path);
        }
        if let Some(ws) = workspace_of(&self.note.path) {
            self.ws = ws;
        }
        self.view = View::Editor;
        self.pending_cursor = cursor;
        self.focus_editor = true;
        self.save_estado();
    }

    /// La nota de hoy: una sola, en `Diario/` (la IA reparte lo que tiene a cada espacio).
    fn today_path(&self) -> PathBuf {
        self.vault.diary_path(&today())
    }

    /// El espacio de lo que no tiene uno claro (una tarea en la nota del día): General, o el primero.
    fn home_ws(&self) -> String {
        let w = &self.vault.workspaces;
        w.iter().find(|w| *w == vault::DEFAULT_WORKSPACE).or(w.first()).cloned().unwrap_or_else(|| vault::DEFAULT_WORKSPACE.into())
    }

    /// El espacio de una nota para sus tareas; las del día, `home_ws`.
    fn space_of(&self, path: &Path) -> String {
        workspace_of(path).unwrap_or_else(|| self.home_ws())
    }

    fn new_note(&mut self) {
        self.save();
        let path = self.vault.unique_path(&self.ws, "Sin título");
        let from_view = self.view != View::Editor;
        if from_view {
            self.sync_tab();
        }
        self.note = OpenNote::load(path.clone());
        if from_view {
            // Desde Inicio o la IA: en una pestaña nueva.
            self.new_tab(tabs::Tab::Note(path));
        }
        self.view = View::Editor;
        self.search.clear();
        self.focus_title = true;
    }

    fn select_workspace(&mut self, ws: String) {
        self.search.clear();
        let path = match self.vault.notes_in(&ws).first() {
            Some(n) => n.path.clone(),
            None => self.vault.unique_path(&ws, "Sin título"),
        };
        self.open_in_tab(path, Some(usize::MAX));
    }

    /// Renombra el archivo según el campo de título.
    fn commit_title(&mut self) {
        let wanted = vault::sanitize(&self.note.title);
        let current = vault::stem(&self.note.path);
        if wanted == current {
            self.note.title = current;
            return;
        }
        let ws = workspace_of(&self.note.path).unwrap_or_else(|| self.ws.clone());
        let new_path = if wanted.to_lowercase() == current.to_lowercase() {
            self.vault.note_path(&ws, &wanted) // solo cambian mayúsculas
        } else {
            self.vault.unique_path(&ws, &wanted)
        };
        if self.note.disk_mtime.is_some() {
            if let Err(e) = fs::rename(&self.note.path, &new_path) {
                self.note.title = current;
                self.msg(format!("No se pudo renombrar: {e}"));
                return;
            }
            self.vault.scan();
        }
        let old_path = std::mem::replace(&mut self.note.path, new_path);
        self.note.title = vault::stem(&self.note.path);
        // El encabezado de una reunión lleva el mismo título.
        if let Some((_, when)) = meeting_header(&self.note.text) {
            if let Some(end) = self.note.text.find('\n') {
                let header = format!("## {} · {when}", self.note.title);
                self.note.text.replace_range(..end, &header);
                self.note.dirty = true;
            }
        }
        if let Some(m) = self.meeting.as_mut().filter(|m| m.path == old_path) {
            m.path = self.note.path.clone();
            m.title = self.note.title.clone();
        }
        self.touched.remove(&old_path);
        self.save_estado();
    }

    fn trash(&mut self, path: PathBuf) {
        let is_open = path == self.note.path;
        if is_open {
            self.save();
        }
        if path.exists() {
            match self.vault.trash(&path) {
                Ok(dest) => {
                    // Se puede deshacer desde la barra inferior.
                    self.undo = Some(Undo {
                        files: Vec::new(),
                        renamed: None,
                        agenda: self.agenda.snapshot(),
                        at: Instant::now(),
                        moved: vec![(path.clone(), dest)],
                        created_dir: None,
                        apart: Vec::new(),
                        relinks: Vec::new(),
                    });
                    self.undo_entry = None;
                    self.msg(format!("«{}» movida a la papelera", vault::stem(&path)));
                }
                Err(e) => {
                    self.msg(format!("No se pudo eliminar: {e}"));
                    return;
                }
            }
        }
        if self.meeting.as_ref().is_some_and(|m| m.path == path) {
            self.meeting = None;
        }
        self.touched.remove(&path);
        if is_open {
            self.note.dirty = false;
            self.note.disk_mtime = None;
            self.note.disk_len = None;
            let ws = self.ws.clone();
            self.select_workspace(ws);
        }
    }

    /// Mueve un espacio completo a la papelera (con Deshacer).
    fn trash_workspace(&mut self, ws: String) {
        self.save();
        if self.meeting.as_ref().is_some_and(|m| workspace_of(&m.path).as_deref() == Some(ws.as_str())) {
            self.close_meeting(Local::now());
        }
        match self.vault.trash_workspace(&ws) {
            Ok(dest) => {
                self.undo = Some(Undo {
                    files: Vec::new(),
                    renamed: None,
                    agenda: self.agenda.snapshot(),
                    at: Instant::now(),
                    moved: vec![(self.vault.root.join(&ws), dest)],
                    created_dir: None,
                    apart: Vec::new(),
                    relinks: Vec::new(),
                });
                self.undo_entry = None;
                self.touched.retain(|p| workspace_of(p).as_deref() != Some(ws.as_str()));
                if workspace_of(&self.note.path).as_deref() == Some(ws.as_str()) || self.ws == ws {
                    self.note.dirty = false;
                    let next = self.vault.workspaces.first().cloned().unwrap_or_else(|| vault::DEFAULT_WORKSPACE.into());
                    self.select_workspace(next);
                }
                self.msg(format!("Espacio «{ws}» movido a la papelera"));
            }
            Err(e) => self.msg(format!("No se pudo mover el espacio: {e}")),
        }
    }

    /// Confirmación para mover un espacio a la papelera.
    fn confirm_window(&mut self, ctx: &egui::Context) {
        let Some(ws) = self.confirm_ws.clone() else { return };
        let n = self.vault.notes_in(&ws).len();
        let mut answer: Option<bool> = None;
        let modal = egui::Modal::new(Id::new("borrar-espacio")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new(format!("¿Mover el espacio «{ws}» a la papelera?")).font(theme::bold(16.0)));
            ui.add_space(4.0);
            let what = if n == 0 { "Está vacío.".to_string() } else { format!("Se va con sus {}.", plural(n, "nota")) };
            ui.label(RichText::new(format!("{what} Queda en la carpeta .papelera y se puede deshacer.")).size(13.0).color(MUTED));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let b = egui::Button::new(RichText::new(format!("{} Mover a la papelera", icon::TRASH)).color(Color32::WHITE)).fill(RED);
                if ui.add(b).clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancelar").clicked() {
                    answer = Some(false);
                }
            });
        });
        if modal.should_close() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                self.confirm_ws = None;
                self.trash_workspace(ws);
            }
            Some(false) => self.confirm_ws = None,
            None => {}
        }
    }

    /// Detecta cambios hechos por fuera (otra app, Dropbox).
    fn poll(&mut self) {
        self.vault.refresh();
        self.vault.maybe_save_cache();
        // Otro equipo cambió los datos internos (o pasó un rato): se traen sus cambios.
        if self.vault.take_internal_changed() || self.stores_at.elapsed() >= Duration::from_secs(60) {
            self.sync_stores();
        }
        self.resolve_conflicts(false);
        self.maybe_merge_days();
        self.maybe_create_recurring();
        self.poll_ai_usage();
        self.maybe_reconcile_tasks();
        let m = vault::modified(&self.note.path);
        if m.is_none() || !self.note.changed_on_disk() {
            return;
        }
        let Ok(disk_text) = vault::read_text(&self.note.path) else { return };
        self.note.remember_disk();
        if disk_text == self.note.text {
            self.note.base = disk_text;
            return;
        }
        if !self.note.dirty {
            self.note.text = disk_text.clone();
            self.note.base = disk_text;
            self.msg("Nota actualizada con cambios de otro equipo");
        } else {
            // Cambió aquí y allá: se juntan línea por línea (la base es lo último que se leyó
            // o guardó). Lo que resulte se guarda como cualquier cambio.
            let merged = crate::merge::merge3(&self.note.base, &self.note.text, &disk_text);
            self.note.text = merged.text;
            self.note.base = disk_text;
            self.note.dirty = true;
            self.note.last_edit = Instant::now();
            self.merged_msg(merged.conflicts);
        }
    }

    // ---------- Reuniones ----------

    fn start_meeting(&mut self) {
        self.start_meeting_as(None);
    }

    /// Nueva reunión; con título (al tomar notas de un evento del calendario) no pide el nombre.
    fn start_meeting_as(&mut self, title: Option<String>) {
        self.close_meeting(Local::now());
        self.save();
        let now = Local::now();
        let named = title.as_deref().map(vault::sanitize).filter(|t| !t.is_empty());
        let file = named.clone().unwrap_or_else(|| format!("Reunión {}", now.format("%Y-%m-%d %H.%M")));
        let path = self.vault.unique_path(&self.ws, &file);
        self.note = OpenNote::load(path.clone());
        let header = named.clone().unwrap_or_else(|| "Reunión".into());
        self.note.text = format!("## {header} · {}\n- {} ", now.format("%Y-%m-%d %H:%M"), now.format("%H:%M"));
        self.note.dirty = true;
        self.save();
        self.meeting = Some(Meeting {
            path,
            title: header,
            started: now,
            last_activity: Instant::now(),
            last_time: now,
        });
        self.view = View::Editor;
        self.search.clear();
        self.pending_cursor = Some(usize::MAX);
        if named.is_some() {
            self.focus_editor = true;
            self.msg("Reunión iniciada: cada línea lleva su hora. Esc la cierra.");
        } else {
            self.focus_title = true;
            self.msg("Reunión iniciada: escribe su nombre y Enter; cada línea lleva su hora. Esc la cierra.");
        }
    }

    /// Agrega "## fin · hora" a la reunión abierta.
    fn close_meeting(&mut self, end: DateTime<Local>) {
        let Some(m) = self.meeting.take() else { return };
        let finish = |text: &str| -> String {
            let mut t = text.trim_end().to_string();
            // Quita una última línea que solo tiene la hora.
            if let Some(pos) = t.rfind('\n') {
                if is_empty_stamp(&t[pos + 1..]) {
                    t.truncate(pos);
                }
            }
            format!("{}\n## fin · {}\n", t.trim_end(), end.format("%H:%M"))
        };
        if self.note.path == m.path {
            self.note.text = finish(&self.note.text);
            self.note.dirty = true;
            self.save();
        } else if let Ok(t) = vault::read_text(&m.path) {
            let t = finish(&t);
            if fs::write(&m.path, &t).is_ok() {
                if let Some(mt) = vault::modified(&m.path) {
                    self.vault.upsert(m.path.clone(), t, mt);
                }
            }
        }
        self.touched.insert(m.path.clone());
        self.msg(format!("Reunión «{}» cerrada a las {}", m.title, end.format("%H:%M")));
    }

    // ---------- IA ----------

    fn start_organize(&mut self) {
        if let Err(e) = &self.ai {
            let e = e.clone();
            self.msg(format!("IA no disponible: {e}"));
            self.open_settings(Section::Ai);
            return;
        }
        self.save();
        self.backlog = self.unorganized().into();
        self.backlog_total = self.backlog.len();
        if self.backlog.is_empty() {
            self.msg("Todo está organizado");
        }
    }

    /// Envía a la IA la siguiente nota pendiente (de a una).
    fn queue_ai(&mut self) {
        let busy = match &self.ai {
            Ok(ai) => ai.busy,
            Err(_) => return,
        };
        if busy {
            return;
        }
        let meeting = self.meeting.as_ref().map(|m| m.path.clone());
        let eligible = |app: &Self, p: &PathBuf, manual: bool| -> Option<u64> {
            if Some(p) == meeting.as_ref() {
                return None;
            }
            if app.claim_wait.get(p).is_some_and(|t| t.elapsed() < CLAIM_RETRY) {
                return None; // la está organizando otro equipo
            }
            if *p == app.note.path && (app.note.dirty || (!manual && app.note.last_edit.elapsed() < AI_IDLE)) {
                return None;
            }
            let n = app.vault.get(p)?;
            if n.text.split_whitespace().count() < 3 {
                return None;
            }
            Some(ai::fnv(&n.text)).filter(|h| !app.analyzed.contains(h))
        };

        let mut pick = None;
        while let Some(p) = self.backlog.pop_front() {
            if let Some(h) = eligible(self, &p, true) {
                pick = Some((p, h));
                break;
            }
        }
        if pick.is_none() && self.ai_auto {
            let mut done = Vec::new();
            for p in &self.touched {
                match eligible(self, p, false) {
                    Some(h) => {
                        pick = Some((p.clone(), h));
                        break;
                    }
                    None if self.vault.get(p).is_none_or(|n| self.analyzed.contains(&ai::fnv(&n.text))) => {
                        done.push(p.clone())
                    }
                    None => {}
                }
            }
            for p in done {
                self.touched.remove(&p);
            }
        }
        let Some((path, hash)) = pick else { return };
        // Un solo equipo organiza cada nota a la vez: si otro la tiene, se reintenta después.
        if !crate::claims::take_note(&self.vault.root, &self.rel(&path), &self.machine, CLAIM_TTL) {
            self.claim_wait.insert(path, Instant::now());
            return;
        }
        let Some(note) = self.vault.get(&path) else { return };

        let infos: Vec<ai::WorkspaceInfo> = self
            .vault
            .workspaces
            .iter()
            .map(|w| {
                let mut tags = self.vault.tag_counts(w);
                tags.sort_by(|a, b| b.1.cmp(&a.1));
                ai::WorkspaceInfo {
                    name: w.clone(),
                    titles: self.vault.notes_in(w).iter().filter(|n| n.path != path).take(15).map(|n| n.title.clone()).collect(),
                    tags: tags.into_iter().take(12).map(|(t, _)| t).collect(),
                }
            })
            .collect();
        let mut all_tags: Vec<String> = infos.iter().flat_map(|i| i.tags.clone()).collect();
        all_tags.sort();
        all_tags.dedup();
        let text: String = note.text.chars().take(12_000).collect();
        // Nota de captura (la del día, "Sin título"): cada línea es una nota; los bloques "##" van juntos.
        let capture = capture::is_capture(&note.title);
        let (system, user) =
            ai::build_prompt(
            &note.title,
            &note.workspace,
            &text,
            &lines::units(&text),
            capture,
            &infos,
            &all_tags,
            &self.ai_facts(),
        );
        if let Ok(ai) = &mut self.ai {
            ai.send(ai::Job { path: path.clone(), hash, system, user });
            self.in_flight = Some(path);
        }
    }

    fn handle_ai_results(&mut self) {
        let results: Vec<ai::JobResult> = match &mut self.ai {
            Ok(ai) => ai.rx.try_iter().collect(),
            Err(_) => return,
        };
        for r in results {
            if let Ok(ai) = &mut self.ai {
                ai.busy = false;
            }
            self.in_flight = None;
            let claimed = self.rel(&r.path);
            let root = self.vault.root.clone();
            match r.result {
                Ok(a) => {
                    self.ai_error = None;
                    self.apply_analysis(r.path, r.hash, a)
                }
                Err(e) => {
                    self.touched.remove(&r.path);
                    self.backlog.clear(); // sin conexión o clave inválida: no insistir
                    let e: String = e.chars().take(160).collect();
                    self.msg(format!("IA: {e}"));
                    if self.ai_error.as_deref() != Some(e.as_str()) {
                        let note = self.rel(&r.path);
                        let what = format!("No pudo organizar «{}»: {e}", vault::stem(&r.path));
                        self.log_ai(crate::activity::Kind::Error, &note, what, Vec::new(), false);
                    }
                    self.ai_error = Some(e);
                }
            }
            crate::claims::release_note(&root, &claimed, &self.machine);
        }
    }

    /// Aplica lo que devolvió la IA: etiquetas en cada línea, tareas con casilla, detalles unidos
    /// a su nota y, según el tipo de nota, a dónde va cada una (captura) o el título y el espacio
    /// de la nota completa (nota con título). También escribe tareas y eventos.
    fn apply_analysis(&mut self, path: PathBuf, hash: u64, a: Analysis) {
        if !path.starts_with(&self.vault.root) {
            return; // la carpeta de notas cambió mientras la IA trabajaba
        }
        let is_open = path == self.note.path;
        if is_open && self.note.dirty {
            return; // se siguió escribiendo; se volverá a analizar
        }
        if is_open && self.note.changed_on_disk() {
            return; // llegó un cambio de otro equipo: primero se junta, después se vuelve a analizar
        }
        if !crate::claims::note_is_mine(&self.vault.root, &self.rel(&path), &self.machine) {
            return; // otro equipo la tomó al mismo tiempo y le toca a él
        }
        let text = if is_open { self.note.text.clone() } else { vault::read_text(&path).unwrap_or_default() };
        if ai::fnv(&text) != hash {
            return;
        }
        let snapshot = self.agenda.snapshot();
        let old_ws = workspace_of(&path).unwrap_or_else(|| self.ws.clone());
        let old_title = vault::stem(&path);
        let is_capture = capture::is_capture(&old_title);

        // A dónde va cada unidad (solo en notas de captura): nota existente del espacio o una nueva.
        let mut chosen: Vec<(String, String, PathBuf)> = Vec::new(); // (espacio, título, ruta)
        let vault_ref = &self.vault;
        let dest = |au: &ai::AiUnit| -> Option<(PathBuf, String)> {
            if !is_capture || au.nota.trim().is_empty() {
                return None;
            }
            let ws = vault_ref.workspaces.iter().find(|w| w.eq_ignore_ascii_case(au.espacio.trim()))?.clone();
            let title = vault::sanitize(au.nota.trim_start_matches('#').trim());
            if let Some((_, _, p)) = chosen.iter().find(|(w, t, _)| *w == ws && t.eq_ignore_ascii_case(&title)) {
                return Some((p.clone(), ws));
            }
            let existing = vault_ref.notes_in(&ws).iter().find(|n| n.title.eq_ignore_ascii_case(&title)).map(|n| n.path.clone());
            let target = existing.unwrap_or_else(|| {
                // Ruta libre que no choque con otra nota nueva de esta misma respuesta.
                let mut p = vault_ref.unique_path(&ws, &title);
                let mut i = 2;
                while chosen.iter().any(|(_, _, c)| *c == p) {
                    p = vault_ref.note_path(&ws, &format!("{title} {i}"));
                    i += 1;
                }
                p
            });
            if target == path {
                return None;
            }
            chosen.push((ws.clone(), title, target.clone()));
            Some((target, ws))
        };
        let plan = organize::plan(&text, &a, dest, new_task_id);
        let mut done: Vec<String> = Vec::new();

        // Notas destino: se agrega al final.
        let mut files: Vec<(PathBuf, Option<String>)> = vec![(path.clone(), Some(text.clone()))];
        let mut written: Vec<(PathBuf, String, String)> = Vec::new(); // (ruta, espacio, texto nuevo)
        for ((target, ws), chunk) in &plan.moves {
            let before = vault::read_text(target).ok();
            let new_t = match before.as_deref().map(str::trim_end).filter(|b| !b.is_empty()) {
                Some(b) if chunk.starts_with("##") => format!("{b}\n\n{chunk}\n"),
                Some(b) => format!("{b}\n{chunk}\n"),
                None => format!("{chunk}\n"),
            };
            if let Some(dir) = target.parent() {
                let _ = fs::create_dir_all(dir);
            }
            if let Err(e) = fs::write(target, &new_t) {
                self.msg(format!("IA: no se pudo escribir {}: {e}", target.display()));
                continue;
            }
            files.push((target.clone(), before));
            written.push((target.clone(), ws.clone(), new_t));
        }
        // Lo que no se pudo escribir se queda en la nota original.
        let failed: Vec<usize> =
            (0..plan.moves.len()).filter(|&i| !written.iter().any(|(p, _, _)| *p == plan.moves[i].0 .0)).collect();
        let mut new_text = plan.source.clone();
        for &i in &failed {
            new_text = format!("{}\n{}\n", new_text.trim_end(), plan.moves[i].1).trim_start().to_string();
        }
        if plan.tags_added > 0 {
            done.push(plural(plan.tags_added, "etiqueta"));
        }
        if plan.grouped > 0 {
            done.push(format!("{} con su nota", plural(plan.grouped, "detalle")));
        }

        // Nota con título: título (si no tenía) y espacio de la nota completa.
        let mut title = old_title.clone();
        let mut ws = old_ws.clone();
        if !is_capture {
            let auto_named = old_title.starts_with("Reunión 20");
            if auto_named && !a.titulo.trim().is_empty() {
                title = vault::sanitize(&a.titulo);
                done.push(format!("«{title}»"));
                if let Some((_, when)) = meeting_header(&new_text) {
                    if let Some(end) = new_text.find('\n') {
                        new_text.replace_range(..end, &format!("## {title} · {when}"));
                    }
                }
            }
            let espacio = a.espacio.trim();
            if a.confianza.trim().eq_ignore_ascii_case("alta")
                && espacio != old_ws
                && self.vault.workspaces.iter().any(|w| w == espacio)
            {
                done.push(format!("→ {espacio}"));
                ws = espacio.to_string();
            }
        }

        // Escribir la nota original (a la papelera si quedó vacía) y moverla si cambió de nombre o espacio.
        let emptied = new_text.trim().is_empty();
        if emptied {
            let _ = self.vault.trash(&path);
        } else if new_text != text {
            if let Err(e) = fs::write(&path, &new_text) {
                self.msg(format!("IA: no se pudo guardar la nota: {e}"));
                return;
            }
        }
        let mut new_path = path.clone();
        if !emptied && (ws != old_ws || title != old_title) {
            let target = self.vault.unique_path(&ws, &title);
            match fs::rename(&path, &target) {
                Ok(()) => new_path = target,
                Err(e) => {
                    ws = old_ws.clone();
                    self.msg(format!("IA: no se pudo mover la nota: {e}"));
                }
            }
        }
        if !written.is_empty() {
            let dests: Vec<String> = written.iter().map(|(p, w, _)| format!("{w}/{}", vault::stem(p))).collect();
            let moved = plan.placed.values().filter(|i| !failed.contains(i)).count();
            done.push(format!("{} → {}", plural(moved, "nota"), dests.join(", ")));
        }

        // Tareas y eventos: cada uno queda asociado a la nota donde terminó su unidad.
        let source_old = self.rel(&path);
        let source_rel = self.rel(&new_path);
        let place = |unidad: &str| -> Option<(String, String)> {
            let i = *plan.placed.get(&unidad.trim().to_uppercase())?;
            let (p, w, _) = written.iter().find(|(p, _, _)| *p == plan.moves[i].0 .0)?;
            Some((self.rel(p), w.clone()))
        };
        let created = today();
        // Lo que queda en la nota del día (que no es de ningún espacio) va al espacio que dijo la IA
        // para esa unidad, o a General.
        let home = self.home_ws();
        let in_diary = vault::in_diary(&path);
        let src_ws = |unidad: &str| -> String {
            if !in_diary {
                return ws.clone();
            }
            let u = unidad.trim();
            a.unidades
                .iter()
                .find(|x| x.id.trim().eq_ignore_ascii_case(u))
                .and_then(|x| self.vault.workspaces.iter().find(|w| w.eq_ignore_ascii_case(x.espacio.trim())))
                .cloned()
                .unwrap_or_else(|| home.clone())
        };
        let (mut src_tasks, mut src_events, mut other_tasks, mut other_events) = (vec![], vec![], vec![], vec![]);
        // Acuerdos de reuniones: los de otros llevan "@Nombre" (lo que se espera de cada uno).
        let meeting_units: HashSet<String> = plan.agreements.iter().map(|g| g.unit.clone()).collect();
        for g in &plan.agreements {
            let text = if g.who.is_empty() { g.what.clone() } else { format!("{} @{}", g.what, g.who.split_whitespace().collect::<Vec<_>>().join("_")) };
            match place(&g.unit) {
                Some((rel, w)) => other_tasks.push(agenda::format_task(&created, &text, &w, g.due.as_deref(), &rel, Some(&g.id))),
                None => src_tasks.push(agenda::format_task(&created, &text, &src_ws(&g.unit), g.due.as_deref(), &source_rel, Some(&g.id))),
            }
        }
        for (ti, t) in a.tareas.iter().enumerate().filter(|(_, t)| !t.texto.trim().is_empty()) {
            // En una reunión con acuerdos, las tareas ya están en los acuerdos.
            if meeting_units.contains(&t.unidad.trim().to_uppercase()) {
                continue;
            }
            let due = Some(t.fecha.trim()).filter(|d| agenda::is_date(d));
            let id = plan.task_ids.get(ti).cloned().flatten();
            match place(&t.unidad) {
                Some((rel, w)) => other_tasks.push(agenda::format_task(&created, &t.texto, &w, due, &rel, id.as_deref())),
                None => src_tasks.push(agenda::format_task(&created, &t.texto, &src_ws(&t.unidad), due, &source_rel, id.as_deref())),
            }
        }
        for e in a.eventos.iter().filter(|e| agenda::is_date(e.fecha.trim()) && !e.titulo.trim().is_empty()) {
            let time = Some(e.hora.trim()).filter(|h| agenda::is_time(h));
            match place(&e.unidad) {
                Some((rel, w)) => other_events.push(agenda::format_event(e.fecha.trim(), time, &e.titulo, &w, &rel)),
                None => src_events.push(agenda::format_event(e.fecha.trim(), time, &e.titulo, &src_ws(&e.unidad), &source_rel)),
            }
        }
        let r1 = self.agenda.replace_for_note(&source_old, &source_rel, &src_tasks, &src_events);
        let r2 = self.agenda.add_lines(&other_tasks, &other_events);
        if let Err(e) = r1.and(r2) {
            self.msg(format!("IA: no se pudo escribir tareas/agenda: {e}"));
        }
        self.gcal_dirty = true;
        let n_tasks = src_tasks.len() + other_tasks.len();
        let n_events = src_events.len() + other_events.len();
        if n_tasks > 0 {
            done.push(plural(n_tasks, "tarea"));
        }
        if n_events > 0 {
            done.push(format!("{} en agenda", plural(n_events, "evento")));
        }

        // Preguntas de la IA sobre lo que no supo con seguridad.
        let found = if emptied { Vec::new() } else { doubts::from_ai(&text, &source_rel, &a, &today(), new_task_id) };
        let questions: Vec<String> = found.iter().map(|d| d.question.clone()).collect();
        let asked = self.add_doubts(&source_old, &source_rel, &new_text, found);
        if asked > 0 {
            done.push(plural(asked, "pregunta"));
        }
        // Temas sin espacio: se juntan y, al reunir varias notas, se sugiere crear el espacio.
        if !emptied {
            let ready_before: Vec<String> = self.ideas.ready().map(|i| i.name.clone()).collect();
            let placed = |id: &str| plan.placed.contains_key(id);
            for r in &mut self.ideas.ideas {
                for x in &mut r.refs {
                    if x.note == source_old {
                        x.note = source_rel.clone();
                    }
                }
            }
            let workspaces = self.vault.workspaces.clone();
            for (name, r) in crate::spaces::refs_from_ai(&new_text, &source_rel, &a, is_capture, &placed) {
                self.ideas.add(&name, r, &workspaces);
            }
            let _ = self.ideas.save(&self.vault.root);
            if let Some(i) = self.ideas.ready().find(|i| !ready_before.contains(&i.name)) {
                done.push(format!("sugerencia: espacio «{}»", i.name));
            }
        }

        // Estado de la app: nada de esto se vuelve a analizar.
        self.analyzed.insert(ai::fnv(&text));
        self.analyzed.insert(ai::fnv(&new_text));
        for (_, _, t) in &written {
            self.analyzed.insert(ai::fnv(t));
        }
        self.save_analyzed();
        self.touched.remove(&path);
        self.vault.scan();
        // Duplicados de lo recién escrito, en todas las notas.
        let mut fresh: Vec<(String, String)> = written.iter().map(|(p, _, t)| (self.rel(p), t.clone())).collect();
        if !emptied {
            fresh.push((source_rel.clone(), new_text.clone()));
        }
        let dups = self.detect_duplicates(fresh);
        if dups > 0 {
            done.push(format!("{} posible{} duplicado{}", dups, if dups == 1 { "" } else { "s" }, if dups == 1 { "" } else { "s" }));
        }
        if is_open {
            // Si la nota quedó vacía, sigue abierta como nota nueva para seguir anotando.
            self.note = OpenNote::load(new_path.clone());
            self.ws = workspace_of(&new_path).unwrap_or_else(|| ws.clone());
            self.save_estado();
        } else if written.iter().any(|(p, _, _)| *p == self.note.path) && !self.note.dirty {
            self.note = OpenNote::load(self.note.path.clone());
        }
        if done.is_empty() {
            return;
        }
        // El detalle, para "Lo que hizo" en la ventana de la IA.
        let mut details: Vec<String> = Vec::new();
        if title != old_title {
            details.push(format!("Título: «{title}»"));
        }
        if ws != old_ws {
            details.push(format!("Movida al espacio {ws}"));
        }
        for (p, w, _) in &written {
            details.push(format!("→ {w}/{}", vault::stem(p)));
        }
        if plan.tags_added > 0 {
            let mut tags: Vec<String> = a.unidades.iter().flat_map(|u| u.etiquetas.iter()).map(|t| organize::clean_tag(t)).filter(|t| !t.is_empty()).collect();
            tags.sort();
            tags.dedup();
            if !tags.is_empty() {
                details.push(format!("Etiquetas: {}", tags.join(", ")));
            }
        }
        let when = |d: Option<&str>| d.filter(|d| agenda::is_date(d)).map(|d| format!(" · {}", long_date(d))).unwrap_or_default();
        for g in &plan.agreements {
            let who = if g.who.is_empty() { String::new() } else { format!("{}: ", g.who) };
            details.push(format!("Acuerdo: {who}{}{}", g.what, when(g.due.as_deref())));
        }
        for t in a.tareas.iter().filter(|t| !t.texto.trim().is_empty() && !meeting_units.contains(&t.unidad.trim().to_uppercase())) {
            details.push(format!("Tarea: {}{}", t.texto.trim(), when(Some(t.fecha.trim()))));
        }
        for e in a.eventos.iter().filter(|e| agenda::is_date(e.fecha.trim()) && !e.titulo.trim().is_empty()) {
            let hour = Some(e.hora.trim()).filter(|h| agenda::is_time(h)).map(|h| format!(" {h}")).unwrap_or_default();
            details.push(format!("Evento: {}{}{hour}", e.titulo.trim(), when(Some(e.fecha.trim()))));
        }
        for q in &questions {
            details.push(format!("Pregunta: {q}"));
        }
        self.undo = Some(Undo {
            files,
            renamed: (new_path != path).then(|| (path.clone(), new_path.clone())),
            agenda: snapshot,
            at: Instant::now(),
            moved: Vec::new(),
            created_dir: None,
            apart: Vec::new(),
            relinks: Vec::new(),
        });
        let name = if emptied { String::new() } else { format!(" {}:", vault::stem(&new_path)) };
        self.msg(format!("IA ·{name} {}", done.join(" · ")));
        let note = if emptied { source_old.clone() } else { self.rel(&new_path) };
        let what = format!("Organizó «{}»: {}", vault::stem(&path), done.join(" · "));
        self.log_ai(crate::activity::Kind::Organizar, &note, what, details, true);
    }

    /// Deja la casilla de la línea "^id" de una nota como hecha o pendiente.
    fn sync_task_line(&mut self, note_rel: &str, id: &str, done: bool) {
        self.edit_task_line(note_rel, id, |line| lines::parse(line).check.is_some_and(|d| d != done).then(|| lines::toggle_check(line)));
    }

    /// Cambia la línea "^id" de una nota (`edit` devuelve la línea nueva, o `None` si no cambia).
    fn edit_task_line(&mut self, note_rel: &str, id: &str, edit: impl Fn(&str) -> Option<String>) {
        let path = self.vault.root.join(format!("{note_rel}.md"));
        let fix = |text: &str| -> Option<String> {
            let (idx, line) = text.split('\n').enumerate().find(|(_, l)| lines::id_of(l).as_deref() == Some(id))?;
            let line = line.trim_end_matches('\r');
            edit(line).map(|new| editor::replace_line(text, idx, &new))
        };
        let open = path == self.note.path;
        let old = if open { self.note.text.clone() } else { vault::read_text(&path).unwrap_or_default() };
        let Some(new) = fix(&old) else { return };
        // Marcar una casilla no cambia el contenido: no hace falta volver a analizarla.
        if self.analyzed.contains(&ai::fnv(&old)) {
            self.analyzed.insert(ai::fnv(&new));
            self.save_analyzed();
        }
        if open {
            self.note.text = new;
            self.note.dirty = true;
            self.save();
        } else if fs::write(&path, &new).is_ok() {
            if let Some(m) = vault::modified(&path) {
                self.vault.upsert(path, new, m);
            }
        }
    }

    fn undo_ai(&mut self) {
        let Some(u) = self.undo.take() else { return };
        self.toast = None;
        if let Some(id) = self.undo_entry.take() {
            if self.activity.mark_undone(&id) {
                let _ = self.activity.save(&self.vault.root);
            }
        }
        for (orig, new) in u.moved.iter().rev() {
            let _ = fs::rename(new, orig);
            if self.note.path == *new {
                self.note.path = orig.clone();
            }
        }
        for (before, after) in u.relinks.iter().rev() {
            self.relink(after, before);
        }
        if let Some((orig, new)) = &u.renamed {
            if let Some(dir) = orig.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::rename(new, orig);
        }
        for (p, before) in &u.files {
            match before {
                Some(t) => {
                    if let Some(dir) = p.parent() {
                        let _ = fs::create_dir_all(dir);
                    }
                    let _ = fs::write(p, t);
                    self.analyzed.insert(ai::fnv(t));
                }
                // Nota creada por la IA: se quita.
                None => {
                    let _ = fs::remove_file(p);
                }
            }
            self.touched.remove(p);
        }
        if let Some(dir) = &u.created_dir {
            let _ = fs::remove_dir(dir); // solo si quedó vacía
        }
        if !u.apart.is_empty() {
            self.keep_apart(&u.apart);
        }
        let _ = self.agenda.restore(&u.agenda);
        self.ask.list = None; // pudo volver una conversación borrada
        self.save_analyzed();
        self.vault.scan();
        let current = self.note.path.clone();
        let reopen = match &u.renamed {
            Some((orig, new)) if *new == current => Some(orig.clone()),
            _ => (u.files.iter().any(|(p, _)| *p == current) || u.moved.iter().any(|(o, _)| *o == current)).then(|| current.clone()),
        };
        if !self.vault.workspaces.contains(&self.ws) {
            self.ws = workspace_of(&current).unwrap_or_else(|| vault::DEFAULT_WORKSPACE.into());
        }
        if let Some(p) = reopen {
            self.note = OpenNote::load(p.clone());
            if let Some(ws) = workspace_of(&p) {
                self.ws = ws;
            }
        }
        self.gcal_dirty = true;
        self.msg("Deshecho");
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Open(p, c) => {
                self.search.clear();
                self.open_in_tab(p, c)
            }
            Action::NewNote => self.new_note(),
            Action::Today => {
                self.search.clear();
                let p = self.today_path();
                self.open_in_tab(p, Some(usize::MAX));
            }
            Action::SelectWorkspace(ws) => self.select_workspace(ws),
            Action::CreateWorkspace(name) => match self.vault.create_workspace(&name) {
                Ok(ws) => self.select_workspace(ws),
                Err(e) => self.msg(format!("No se pudo crear el espacio: {e}")),
            },
            Action::Trash(p) => self.trash(p),
            Action::AskTrashWorkspace(ws) => self.confirm_ws = Some(ws),
            Action::MoveNote(p, ws) => self.move_note(p, ws),
            Action::RenameWorkspace(old, name) => self.rename_space(old, &name),
            Action::RenameTag(old, name) => self.rename_tag(old, &name),
            Action::Restore(t) => self.restore(t),
            Action::NoteToTask(p) => self.note_to_task(p),
            Action::ShowTag(t) => {
                self.save();
                self.search.clear();
                self.view = if self.view == View::Tag(t.clone()) { View::Editor } else { View::Tag(t) };
                self.focus_editor = self.view == View::Editor;
            }
            Action::Show(v) => {
                self.save();
                self.search.clear();
                if self.view == v {
                    self.view = View::Editor;
                    self.focus_editor = true;
                } else {
                    self.show_in_tab(v);
                }
            }
            Action::ShowAi(t) => {
                self.search.clear();
                self.ai_tab = t;
                self.show_in_tab(View::Ai);
                self.ask.focus = t == ai_view::AiTab::Chat;
            }
            Action::CloseResults => {
                self.search.clear();
                self.view = View::Editor;
                self.focus_editor = true;
            }
            Action::FocusSearch => self.focus_search = true,
            Action::StartMeeting => self.start_meeting(),
            Action::CloseMeeting => self.close_meeting(Local::now()),
            Action::Organize => self.start_organize(),
            Action::Undo => self.undo_ai(),
            Action::GoogleConnect => match &mut self.gcal {
                Some(g) => {
                    g.connect();
                    self.msg("Se abrió el navegador: elige tu cuenta y permite el acceso al calendario");
                }
                None => self.msg("Falta google_client_id en config.toml (pasos en el README)"),
            },
            Action::GoogleSync => {
                self.gcal_dirty = true;
                self.gcal_last_try = long_ago();
            }
            Action::GoogleDisconnect => {
                if let Some(g) = &mut self.gcal {
                    g.disconnect();
                }
            }
            Action::TodoConnect => {
                if let Some(t) = &mut self.todo {
                    t.connect();
                    self.msg("Se abrió el navegador: entra con tu cuenta Microsoft y acepta el permiso");
                }
            }
            Action::TodoSync => self.todo_last = long_ago(),
            Action::TodoDisconnect => {
                if let Some(t) = &mut self.todo {
                    t.disconnect();
                }
            }
            Action::ToggleTask(raw) => {
                self.gcal_dirty = true;
                if let Err(e) = self.agenda.toggle_task(&raw, &today()) {
                    self.msg(format!("No se pudo actualizar tareas.txt: {e}"));
                }
                // La casilla de su línea en la nota también.
                if let Some(t) = agenda::parse_task(&raw) {
                    if let (Some(id), Some(note)) = (t.id, t.note) {
                        self.sync_task_line(&note, &id, !t.done);
                    }
                }
            }
            Action::AddTask(text) => {
                self.gcal_dirty = true;
                let (text, due) = agenda::parse_when(&text, Local::now().date_naive());
                let line = agenda::format_task(&today(), &text, &self.ws, due.as_deref(), &self.rel(&self.note.path), None);
                if let Err(e) = self.agenda.add_task(line) {
                    self.msg(format!("No se pudo escribir tareas.txt: {e}"));
                }
            }
            Action::OpenExternal(p) => open_external(&p),
            Action::OpenSettings(section) => self.open_settings(section),
            Action::AddMailAccount(a) => {
                let email = a.correo.clone();
                self.cfg.correos.retain(|x| !x.correo.eq_ignore_ascii_case(&email));
                self.cfg.correos.push(a);
                self.save_config();
                let i = self.cfg.correos.len() - 1;
                self.test_mail_account(i);
                self.msg(format!("Correo {email} agregado; probando la conexión…"));
            }
            Action::RemoveMailAccount(i) => {
                if i < self.cfg.correos.len() {
                    let a = self.cfg.correos.remove(i);
                    self.save_config();
                    self.mail.errors.remove(&a.correo);
                    self.msg(format!("Correo {} quitado", a.correo));
                }
            }
            Action::UpdateMailAccount(i, mut a) => {
                if let Some(old) = self.cfg.correos.get(i).cloned() {
                    if a.clave.trim().is_empty() {
                        a.clave = old.clave.clone();
                    }
                    self.mail.errors.remove(&old.correo);
                    self.mail.tests.remove(&old.correo);
                    let email = a.correo.clone();
                    self.cfg.correos[i] = a;
                    self.save_config();
                    self.test_mail_account(i);
                    self.msg(format!("Correo {email} actualizado; probando la conexión…"));
                }
            }
            Action::TestMailAccount(i) => self.test_mail_account(i),
            Action::CheckMail => self.mail.request(),
            Action::MailFulfill(id, i, done) => self.mail_fulfill(&id, i, done),
            Action::MailRemoveItems(id) => self.mail_remove_items(&id),
            Action::MailSaveNote(id) => self.mail_save_note(&id),
            Action::OpenMail(id) => self.open_mail(id),
            Action::AddCalendar(name, url) => self.add_calendar(name, url),
            Action::RemoveCalendar(i) => self.remove_calendar(i),
            Action::EditCalendar(i, name, url) => self.edit_calendar(i, name, url),
            Action::RefreshCalendars => self.cals.last = None,
            Action::StartMeetingNamed(title) => {
                if !self.start_recurring(&title) {
                    self.start_meeting_as(Some(title));
                }
            }
            Action::OpenRecurring => self.open_recurring(),
            Action::OpenNewTab(p) => self.new_tab(tabs::Tab::Note(p)),
            Action::ShowTab(v) => self.show_in_tab(v),
            Action::NewTab => self.new_tab(tabs::Tab::View(View::Home)),
            Action::CloseTab => {
                let i = self.tabs.active;
                self.close_tab(i);
            }
            Action::NextTab(forward) => {
                self.sync_tab();
                let n = self.tabs.list.len();
                let i = if forward { (self.tabs.active + 1) % n } else { (self.tabs.active + n - 1) % n };
                self.activate_tab(i);
            }
            Action::ActivateTab(i) => {
                self.sync_tab();
                let i = i.min(self.tabs.list.len() - 1);
                self.activate_tab(i);
            }
        }
    }

    // ---------- Interfaz ----------

    fn shortcuts(&mut self, ctx: &egui::Context) -> Option<Action> {
        let pressed = |k| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, k)));
        // Pestañas: Ctrl+T, Ctrl+W, Ctrl+Tab / Ctrl+Shift+Tab, Ctrl+1…9.
        if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab))) {
            return Some(Action::NextTab(false));
        }
        if pressed(Key::Tab) {
            return Some(Action::NextTab(true));
        }
        if pressed(Key::T) {
            return Some(Action::NewTab);
        }
        if pressed(Key::W) {
            return Some(Action::CloseTab);
        }
        let numbers = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
        for (i, k) in numbers.into_iter().enumerate() {
            if pressed(k) {
                return Some(Action::ActivateTab(if i == 8 { usize::MAX } else { i }));
            }
        }
        if pressed(Key::N) {
            return Some(Action::NewNote);
        }
        if pressed(Key::D) {
            return Some(Action::Today);
        }
        if pressed(Key::F) {
            return Some(Action::FocusSearch);
        }
        if pressed(Key::H) {
            return Some(Action::ShowTab(View::Home));
        }
        if pressed(Key::K) {
            return Some(Action::ShowAi(ai_view::AiTab::Chat));
        }
        if pressed(Key::R) {
            return Some(Action::StartMeeting);
        }
        if pressed(Key::Comma) {
            return Some(Action::OpenSettings(Section::General));
        }
        if pressed(Key::S) {
            self.note.dirty = true;
            self.save();
        }
        // Esc cierra la reunión (si no hay una búsqueda o vista abierta que cerrar primero).
        let esc = ctx.input(|i| i.key_pressed(Key::Escape));
        if esc && self.settings.is_none() && self.meeting.is_some() && self.search.is_empty() && self.view == View::Editor && self.new_ws.is_none() {
            return Some(Action::CloseMeeting);
        }
        None
    }

    fn rail(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            if rail_item(ui, icon::HOUSE, "Inicio", "Inicio: tu día y un resumen de todo (Ctrl+H)", self.view == View::Home, TEXT).clicked() {
                action = Some(Action::ShowTab(View::Home));
            }
            let (tip, color) = match &self.meeting {
                Some(m) => (format!("Cerrar reunión «{}» (Esc)", m.title), SUCCESS),
                None => ("Nueva reunión (Ctrl+R)".to_string(), TEXT),
            };
            if rail_item(ui, icon::USERS, "Reunión", &tip, self.meeting.is_some(), color).clicked() {
                action = Some(if self.meeting.is_some() { Action::CloseMeeting } else { Action::StartMeeting });
            }
            if rail_item(ui, icon::CHECK_SQUARE, "Tareas", "Todas las tareas", self.view == View::Tasks, TEXT).clicked() {
                action = Some(Action::ShowTab(View::Tasks));
            }
            if rail_item(ui, icon::CALENDAR_BLANK, "Agenda", "Tus calendarios, eventos y tareas con fecha", self.view == View::Agenda, TEXT).clicked() {
                action = Some(Action::ShowTab(View::Agenda));
            }
            let checks = self.mail.open_checks().len();
            let color = if self.mail.busy() { ACCENT } else { TEXT };
            let r = rail_item(ui, icon::ENVELOPE_SIMPLE, "Correo", "Compromisos y fechas de tu correo", self.view == View::Mail, color);
            if r.clicked() {
                action = Some(Action::ShowTab(View::Mail));
            }
            if checks > 0 {
                rail_badge(ui, &r, checks, SUCCESS);
            }
            // La IA: conversar, sus preguntas y lo que hizo, en una sola ventana.
            let asks = self.pending_asks();
            let working = self.in_flight.is_some() || self.ask.busy();
            let color = if self.ai.is_err() { MUTED } else if working { ACCENT } else { TEXT };
            let tip = match &self.ai {
                Ok(_) if asks > 0 => format!("IA: tiene {} para ti · conversar y lo que hizo (Ctrl+K)", plural(asks, "pregunta")),
                Ok(_) => "IA: conversar con tus notas, sus preguntas y lo que hizo (Ctrl+K)".to_string(),
                Err(e) => format!("IA no disponible: {e}"),
            };
            let r = rail_item(ui, icon::SPARKLE, "IA", &tip, self.view == View::Ai, color);
            if r.clicked() {
                let tab = if asks > 0 && self.view != View::Ai { ai_view::AiTab::Asks } else { self.ai_tab };
                action = Some(Action::ShowAi(tab));
            }
            if asks > 0 {
                rail_badge(ui, &r, asks, ACCENT);
            }
            ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                if rail_item(ui, icon::GEAR, "Ajustes", "Configuración (Ctrl+,)", self.settings.is_some(), TEXT).clicked() {
                    action = Some(Action::OpenSettings(Section::General));
                }
                if rail_item(ui, icon::TRASH, "Papelera", "Lo que borraste: restaurar o borrar para siempre", self.view == View::Trash, TEXT).clicked() {
                    action = Some(Action::ShowTab(View::Trash));
                }
                if rail_item(ui, icon::FOLDER_OPEN, "Carpeta", "Abrir la carpeta de notas", false, TEXT).clicked() {
                    action = Some(Action::OpenExternal(self.vault.root.clone()));
                }
            });
        });
        action
    }

    fn sidebar(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;

        let search = ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text(format!("{}  Buscar en todas las notas", icon::MAGNIFYING_GLASS))
                .desired_width(f32::INFINITY)
                .margin(Margin::symmetric(8, 5)),
        );
        if std::mem::take(&mut self.focus_search) {
            search.request_focus();
        }
        if search.has_focus() && self.esc(ui) {
            action = Some(Action::CloseResults);
        }
        ui.add_space(10.0);

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let summary = self.summary();
            let editing = self.view == View::Editor;
            // Clic abre; Ctrl+clic o la rueda, en otra pestaña; el tacho la manda a la papelera.
            // Una nota se puede arrastrar a un espacio, o moverla con el clic derecho.
            let spaces = self.vault.workspaces.clone();
            let row_action = |ui: &Ui, r: &Response, path: &PathBuf| -> Option<Action> {
                if row_trash_button(ui, r, "Mover a la papelera (se puede deshacer)") {
                    return Some(Action::Trash(path.clone()));
                }
                let mut act = None;
                if r.middle_clicked() || (r.clicked() && ui.input(|i| i.modifiers.command)) {
                    act = Some(Action::OpenNewTab(path.clone()));
                } else if r.clicked() {
                    act = Some(Action::Open(path.clone(), None));
                }
                let movable = !vault::in_diary(path);
                if movable {
                    r.dnd_set_drag_payload(path.clone());
                }
                r.context_menu(|ui| {
                    if movable {
                        let here = workspace_of(path);
                        ui.menu_button(format!("{}  Mover a", icon::FOLDER_SIMPLE), |ui| {
                            for w in spaces.iter().filter(|w| Some(*w) != here.as_ref()) {
                                if ui.button(w).clicked() {
                                    act = Some(Action::MoveNote(path.clone(), w.clone()));
                                    ui.close();
                                }
                            }
                        });
                    }
                    if ui.button(format!("{}  Convertir en tarea", icon::CHECK_SQUARE)).clicked() {
                        act = Some(Action::NoteToTask(path.clone()));
                        ui.close();
                    }
                    if ui.button(format!("{}  Mover a la papelera", icon::TRASH)).clicked() {
                        act = Some(Action::Trash(path.clone()));
                        ui.close();
                    }
                });
                act
            };

            // Hoy y el Diario, arriba y fuera de los espacios: una sola nota por día (se crea al
            // escribir); la IA reparte lo que tiene a cada espacio.
            let today_path = self.today_path();
            let today_exists = summary.today_exists;
            let r = list_row(ui, icon::SUN, "Hoy", if today_exists { "" } else { "vacía" }, today_path == self.note.path && editing);
            let r = r.on_hover_text("La nota de hoy: escribe aquí lo que vaya surgiendo; la IA reparte cada cosa a su espacio (Ctrl+D)");
            if today_exists {
                if let Some(a) = row_action(ui, &r, &today_path) {
                    action = Some(a);
                }
            } else if r.clicked() {
                action = Some(Action::Today);
            }

            // Diario: las notas de días anteriores, de la más nueva a la más vieja.
            let diary = &summary.diary;
            if !diary.is_empty() {
                let open = self.diary_open || diary.iter().any(|n| n.path == self.note.path);
                let caret = if open { icon::CARET_DOWN } else { icon::CARET_RIGHT };
                let r = list_row(ui, caret, "Diario", &diary.len().to_string(), false)
                    .on_hover_text("Las notas de días anteriores");
                if r.clicked() {
                    self.diary_open = !open;
                }
                if open {
                    let rows = visible_rows(ui, diary.len());
                    let below = diary.len() - rows.end;
                    for n in &diary[rows] {
                        let glyph = if n.meeting { icon::USERS } else { icon::CALENDAR_BLANK };
                        let r = ui.horizontal(|ui| {
                            ui.add_space(14.0);
                            list_row(ui, glyph, &n.title, "", n.path == self.note.path && editing)
                        });
                        if let Some(a) = row_action(ui, &r.inner, &n.path) {
                            action = Some(a);
                        }
                    }
                    skip_rows(ui, below);
                }
            }

            ui.add_space(14.0);

            // Espacios
            if section(ui, "Espacios", Some("Nuevo espacio")) {
                self.new_ws = Some(String::new());
            }
            for ws in self.vault.workspaces.clone() {
                // Cambiando el nombre: un campo en vez de la fila (Enter guarda, Esc o clic afuera cancela).
                if let Some((_, name)) = self.renaming_ws.as_mut().filter(|(old, _)| *old == ws) {
                    let r = ui.add(egui::TextEdit::singleline(name).hint_text("Nombre del espacio").desired_width(f32::INFINITY));
                    if !r.has_focus() && !r.lost_focus() {
                        r.request_focus();
                    }
                    if r.lost_focus() {
                        let entered = ui.input(|i| i.key_pressed(Key::Enter));
                        if let Some((old, name)) = self.renaming_ws.take().filter(|_| entered) {
                            action = Some(Action::RenameWorkspace(old, name));
                        }
                    }
                    continue;
                }
                let r = list_row(ui, icon::FOLDER_SIMPLE, &ws, "", ws == self.ws);
                // Soltar aquí una nota arrastrada la mueve a este espacio.
                let dragged = r.dnd_hover_payload::<PathBuf>().filter(|p| workspace_of(p).as_deref() != Some(ws.as_str()));
                if dragged.is_some() {
                    ui.painter().rect_stroke(r.rect, 6, Stroke::new(1.5, ACCENT), egui::StrokeKind::Inside);
                }
                if let Some(p) = r.dnd_release_payload::<PathBuf>() {
                    action = Some(Action::MoveNote((*p).clone(), ws.clone()));
                } else if row_trash_button(ui, &r, "Mover el espacio a la papelera") {
                    action = Some(Action::AskTrashWorkspace(ws.clone()));
                } else if r.clicked() {
                    action = Some(Action::SelectWorkspace(ws.clone()));
                }
                r.context_menu(|ui| {
                    if ui.button(format!("{}  Cambiar nombre", icon::PENCIL_SIMPLE)).clicked() {
                        self.renaming_ws = Some((ws.clone(), ws.clone()));
                        ui.close();
                    }
                    if ui.button(format!("{}  Mover el espacio a la papelera", icon::TRASH)).clicked() {
                        action = Some(Action::AskTrashWorkspace(ws.clone()));
                        ui.close();
                    }
                });
            }
            if let Some(name) = &mut self.new_ws {
                let r = ui.add(
                    egui::TextEdit::singleline(name).hint_text("Nombre del espacio").desired_width(f32::INFINITY),
                );
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                if r.lost_focus() {
                    let entered = ui.input(|i| i.key_pressed(Key::Enter));
                    let name = std::mem::take(name);
                    self.new_ws = None;
                    if entered && !name.trim().is_empty() {
                        action = Some(Action::CreateWorkspace(name));
                    }
                }
            }
            ui.add_space(14.0);

            // Notas del espacio (las reuniones con su ícono)
            if section(ui, "Notas", Some(&format!("Nueva nota en {} (Ctrl+N)", self.ws))) {
                action = Some(Action::NewNote);
            }
            let open_is_new = self.note.disk_mtime.is_none();
            if open_is_new && self.note.path != today_path && workspace_of(&self.note.path).as_deref() == Some(self.ws.as_str()) {
                let r = list_row(ui, icon::FILE_TEXT, &self.note.title, "nueva", editing);
                if row_trash_button(ui, &r, "Descartar la nota nueva") {
                    action = Some(Action::Trash(self.note.path.clone()));
                }
            }
            let live = self.meeting.as_ref().map(|m| m.path.clone());
            let rows = visible_rows(ui, summary.others.len());
            let below = summary.others.len() - rows.end;
            for n in &summary.others[rows] {
                let glyph = if n.meeting { icon::USERS } else { icon::FILE_TEXT };
                let right = if live.as_ref() == Some(&n.path) { "en curso" } else { n.right.as_str() };
                let r = list_row(ui, glyph, &n.title, right, n.path == self.note.path && editing);
                if let Some(a) = row_action(ui, &r, &n.path) {
                    action = Some(a);
                }
            }
            skip_rows(ui, below);
            ui.add_space(14.0);

            // Etiquetas del espacio
            let tags = summary.tags.clone();
            section(ui, "Etiquetas", None);
            if tags.is_empty() {
                ui.label(RichText::new("Escribe #palabra en una nota.").color(MUTED).size(12.5));
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(5.0, 6.0);
                for (tag, count) in tags {
                    let selected = self.view == View::Tag(tag.clone());
                    let r = tag_pill(ui, &tag, count, selected).on_hover_text(format!("Ver las líneas con #{tag} (clic derecho: cambiar nombre)"));
                    r.context_menu(|ui| {
                        if ui.button(format!("{}  Cambiar nombre", icon::PENCIL_SIMPLE)).clicked() {
                            self.renaming_tag = Some((tag.clone(), tag.clone()));
                            ui.close();
                        }
                    });
                    if r.clicked() {
                        action = Some(Action::ShowTag(tag));
                    }
                }
            });
        });
        // La nota que se está arrastrando, junto al puntero.
        if let Some(p) = egui::DragAndDrop::payload::<PathBuf>(ui.ctx()) {
            if let Some(pos) = ui.ctx().pointer_interact_pos() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                egui::Area::new(Id::new("nota-arrastrada"))
                    .fixed_pos(pos + egui::vec2(14.0, 10.0))
                    .order(egui::Order::Tooltip)
                    .interactable(false)
                    .show(ui.ctx(), |ui| {
                        Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(RichText::new(format!("{} {} → suéltala en un espacio", icon::FILE_TEXT, vault::stem(&p))).size(13.0));
                        });
                    });
            }
        }
        action
    }

    fn status_bar(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        ui.horizontal_centered(|ui| {
            // Reunión en curso (siempre visible).
            if let Some(m) = &self.meeting {
                let mins = (Local::now() - m.started).num_minutes();
                let text = RichText::new(format!("{} {} · {mins} min · Esc para cerrar", icon::RECORD, m.title))
                    .size(12.5)
                    .color(SUCCESS);
                let r = ui.add(egui::Label::new(text).sense(Sense::click())).on_hover_text("Ir a la reunión");
                if r.clicked() {
                    action = Some(Action::Open(m.path.clone(), Some(usize::MAX)));
                }
                ui.add_space(12.0);
            }
            // Dropbox cerrado: se puede seguir trabajando, pero no se sincroniza.
            if self.dropbox.as_ref().is_some_and(|d| d.closed()) {
                let text = RichText::new(format!("{} Dropbox no está abierto", icon::CLOUD_SLASH)).size(12.5).color(WARN);
                ui.label(text).on_hover_text(
                    "Tus notas están en una carpeta de Dropbox, pero la app de Dropbox no está abierta. Puedes seguir trabajando: los cambios quedan en este equipo y se sincronizarán cuando la abras.",
                );
                ui.add_space(12.0);
            }
            // Copias en conflicto de Dropbox que se están por juntar (se juntan solas).
            if !self.conflict_seen.is_empty() {
                let n = self.conflict_seen.len();
                let text = RichText::new(format!("{} Juntando {}", icon::ARROWS_MERGE, plural(n, "copia en conflicto"))).size(12.5).color(MUTED);
                ui.label(text).on_hover_text("Dropbox dejó dos versiones de un archivo; en unos segundos se juntan solas, sin perder nada");
                ui.add_space(12.0);
            }
            let (glyph, text, color) = if self.note.dirty {
                (icon::CIRCLE_NOTCH, "Guardando…", MUTED)
            } else if self.note.disk_mtime.is_none() {
                (icon::FILE_TEXT, "Nota nueva — se guarda al escribir", MUTED)
            } else {
                (icon::CHECK_CIRCLE, "Guardado", SUCCESS)
            };
            ui.label(RichText::new(format!("{glyph} {text}")).size(12.5).color(color));
            let words = self.note.text.split_whitespace().count();
            ui.label(RichText::new(format!("   {words} palabras")).size(12.5).color(MUTED));
            // IA trabajando.
            if let Some(p) = &self.in_flight {
                let progress = if self.backlog_total > 0 {
                    format!(" ({}/{})", self.backlog_total - self.backlog.len(), self.backlog_total)
                } else {
                    String::new()
                };
                ui.label(
                    RichText::new(format!("   {} Analizando «{}»{progress}", icon::SPARKLE, vault::stem(p)))
                        .size(12.5)
                        .color(ACCENT),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.undo.as_ref().is_some_and(|u| u.at.elapsed() < UNDO_WINDOW) {
                    let b = egui::Button::new(RichText::new(format!("{} Deshacer", icon::ARROW_COUNTER_CLOCKWISE)).size(12.5));
                    if ui.add(b).clicked() {
                        action = Some(Action::Undo);
                    }
                }
                let msg = self.message.as_ref().filter(|(_, t)| t.elapsed() < Duration::from_secs(10));
                if let Some((m, _)) = msg {
                    ui.add(egui::Label::new(RichText::new(m).color(TEXT).size(12.5)).truncate());
                }
            });
        });
        action
    }

    /// Columna centrada con desplazamiento, común a todas las vistas.
    fn column<R>(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui, f32) -> R) -> R {
        Self::column_at(ui, id, 26.0, add)
    }

    /// Igual que `column`, con otro margen arriba.
    fn column_at<R>(ui: &mut Ui, id: &str, top: f32, add: impl FnOnce(&mut Ui, f32) -> R) -> R {
        egui::ScrollArea::vertical()
            .id_salt(id)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let w = ui.available_width();
                let col_w = (w - 64.0).clamp(200.0, COLUMN_MAX);
                ui.horizontal_top(|ui| {
                    ui.add_space((w - col_w) / 2.0);
                    ui.vertical(|ui| {
                        ui.set_width(col_w);
                        ui.add_space(top);
                        let r = add(ui, col_w);
                        ui.add_space(40.0);
                        r
                    })
                    .inner
                })
                .inner
            })
            .inner
    }

    /// Resultados de la búsqueda o de una etiqueta. Se calculan una sola vez por texto buscado
    /// y se vuelven a calcular solo si cambia lo buscado, el espacio o alguna nota.
    fn found(&mut self) -> &Found {
        let query = vault::fold(self.search.trim());
        let tag = match &self.view {
            View::Tag(t) if query.is_empty() => Some(t.clone()),
            _ => None,
        };
        let key = format!("{query}\u{1}{tag:?}\u{1}{}", self.ws);
        let fresh = self.found.as_ref().is_some_and(|f| f.key == key && f.generation == self.vault.generation);
        if !fresh {
            let notes = match &tag {
                Some(_) => self.vault.notes_in(&self.ws),
                None => self.vault.all_notes(),
            };
            let mut hits = Vec::new();
            for n in notes {
                let title_match = tag.is_none() && vault::fold(&n.title).contains(&query);
                // Descarte rápido: la nota completa no tiene lo buscado.
                let may = match &tag {
                    Some(_) => n.text.contains('#'),
                    None => n.folded().contains(&query),
                };
                if !may && !title_match {
                    continue;
                }
                let mut lines = Vec::new();
                if may {
                    let mut offset = 0; // en caracteres
                    for (line, folded) in n.text.split_inclusive('\n').zip(n.folded().split_inclusive('\n')) {
                        let content = line.trim_end();
                        let matches = match &tag {
                            Some(t) => content.contains('#') && tags::line_tags(content).contains(t),
                            None => folded.contains(&query),
                        };
                        if matches {
                            // Cursor al final de la línea, listo para seguir escribiendo.
                            lines.push((offset + content.chars().count(), content.trim().to_string()));
                        }
                        offset += line.chars().count();
                    }
                }
                if !lines.is_empty() || title_match {
                    hits.push(Hit { path: n.path.clone(), title: display_title(&n.title), ws: n.workspace.clone(), lines });
                }
            }
            self.found = Some(Found { key, generation: self.vault.generation, tag, hits, shown: FOUND_PAGE });
        }
        self.found.as_ref().expect("recién calculado")
    }

    /// El resumen de la barra lateral e Inicio, calculado de nuevo solo si cambió algo.
    fn summary(&mut self) -> std::rc::Rc<Summary> {
        let key = (self.vault.generation, self.ws.clone(), today());
        if let Some(s) = self.summary.as_ref().filter(|s| s.key == key) {
            return s.clone();
        }
        let s = std::rc::Rc::new(Summary::build(&self.vault, key));
        self.summary = Some(s.clone());
        s
    }

    /// Lista de líneas que tienen una etiqueta o coinciden con la búsqueda.
    fn results(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let (tag, total, notes) = {
            let found = self.found();
            (found.tag.clone(), found.hits.iter().map(|h| h.lines.len()).sum::<usize>(), found.hits.len())
        };
        let heading = match &tag {
            Some(t) => t.clone(),
            None => format!("Buscar «{}»", self.search.trim()),
        };
        let scope = if tag.is_some() { format!("en {}", self.ws) } else { "en todos los espacios".into() };
        let subtitle = format!("{} en {}, {scope}", plural(total, "línea"), plural(notes, "nota"));
        let Some(found) = self.found.take() else { return None };
        let mut more = false;
        Self::column(ui, "results", |ui, _| {
            view_header(ui, &heading, &subtitle);
            if found.hits.is_empty() {
                ui.label(RichText::new("Sin resultados.").color(MUTED));
            }
            // Solo las primeras líneas (dibujar miles es lento); el resto con «Mostrar más».
            let mut drawn = 0;
            for h in &found.hits {
                if drawn >= found.shown {
                    break;
                }
                let r = ui.add(
                    egui::Label::new(RichText::new(&h.title).font(theme::bold(15.0)).color(TEXT)).sense(Sense::click()),
                );
                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    action = Some(Action::Open(h.path.clone(), None));
                }
                if tag.is_none() {
                    ui.label(RichText::new(&h.ws).size(12.5).color(MUTED));
                }
                for (offset, line) in &h.lines {
                    if clickable_line(ui, highlight_line(&display_line(line), EDITOR_SIZE - 1.0)).clicked() {
                        action = Some(Action::Open(h.path.clone(), Some(*offset)));
                    }
                }
                drawn += h.lines.len().max(1);
                ui.add_space(14.0);
            }
            if drawn < total {
                ui.add_space(4.0);
                let rest = total.saturating_sub(drawn);
                if ui.button(format!("Mostrar más ({})", plural(rest, "línea"))).clicked() {
                    more = true;
                }
            }
        });
        let mut found = found;
        if more {
            found.shown += FOUND_PAGE;
        }
        self.found = Some(found);
        if self.esc(ui) {
            action = Some(Action::CloseResults);
        }
        action
    }

    fn tasks_view(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let today = today();
        let tasks = sort_tasks(self.agenda.tasks(), self.tasks_by_due, &today);
        let (pending, mut done): (Vec<_>, Vec<_>) = tasks.into_iter().partition(|t| !t.done);
        // Las hechas: la última que se terminó, primero.
        done.reverse();
        done.sort_by(|a, b| b.done_on.cmp(&a.done_on));
        let subtitle = format!("{} · marca la casilla cuando la termines", plural(pending.len(), "pendiente"));
        let root = self.vault.root.clone();
        let mut typing = false;
        Self::column(ui, "tasks", |ui, _| {
            view_header(ui, "Tareas", &subtitle);
            if let Some(a) = self.todo_panel(ui) {
                action = Some(a);
            }
            ui.add_space(10.0);
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.new_task)
                    .hint_text(format!("{}  Nueva tarea en {}: «Enviar planos el viernes», «Llamar a Pedro mañana»…", icon::PLUS, self.ws))
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(8, 6)),
            );
            typing = r.has_focus();
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !self.new_task.trim().is_empty() {
                action = Some(Action::AddTask(std::mem::take(&mut self.new_task)));
                r.request_focus();
            }
            ui.add_space(8.0);
            // Orden: lo más reciente primero (las atrasadas, arriba) o por fecha límite.
            ui.horizontal(|ui| {
                let before = self.tasks_by_due;
                ui.selectable_value(&mut self.tasks_by_due, false, RichText::new("Recientes primero").size(12.5));
                ui.selectable_value(&mut self.tasks_by_due, true, RichText::new("Por fecha").size(12.5));
                if self.tasks_by_due != before {
                    self.save_estado();
                }
            });
            ui.add_space(8.0);
            if pending.is_empty() {
                ui.label(RichText::new("No hay tareas pendientes. La IA las encuentra en tus notas, o agrégalas arriba.").color(MUTED));
            }
            for t in &pending {
                if let Some(a) = task_row(ui, t, &today, &root) {
                    action = Some(a);
                }
            }
            if !done.is_empty() {
                ui.add_space(16.0);
                egui::CollapsingHeader::new(RichText::new(format!("Hechas ({})", done.len())).color(MUTED))
                    .default_open(false)
                    .show(ui, |ui| {
                        for t in done.iter().take(50) {
                            if let Some(a) = task_row(ui, t, &today, &root) {
                                action = Some(a);
                            }
                        }
                    });
            }
        });
        if !typing && self.esc(ui) {
            action = Some(Action::CloseResults);
        }
        action
    }

    /// Estado y botones de la sincronización con Google Calendar (al pie de la Agenda).
    fn google_panel(&self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{} Google Calendar", icon::GOOGLE_LOGO)).font(theme::bold(14.0)));
            match &self.gcal {
                None => {
                    ui.label(RichText::new("Si quieres, tus tareas y eventos también pueden aparecer en Google Calendar.").size(13.0).color(MUTED));
                    if ui.link(RichText::new("Cómo activarlo").size(13.0)).clicked() {
                        action = Some(Action::OpenSettings(Section::Calendar));
                    }
                }
                Some(g) if g.connecting => {
                    ui.label(RichText::new("Esperando tu permiso en el navegador…").size(12.5).color(ACCENT));
                }
                Some(g) if !g.connected => {
                    if ui.button(format!("{} Conectar Google Calendar", icon::LINK)).clicked() {
                        action = Some(Action::GoogleConnect);
                    }
                    if let Some(e) = &g.last_error {
                        ui.label(RichText::new(e).size(12.5).color(RED));
                    }
                }
                Some(g) => {
                    let status = if g.busy {
                        "sincronizando…".to_string()
                    } else if let Some(e) = &g.last_error {
                        format!("error: {e}")
                    } else if let Some(t) = g.last_sync {
                        format!("sincronizado a las {}", t.format("%H:%M"))
                    } else {
                        "conectado".to_string()
                    };
                    let color = if g.last_error.is_some() { RED } else { SUCCESS };
                    ui.label(RichText::new(format!("calendario «Notas» · {status}")).size(12.5).color(color));
                    if ui.link(RichText::new("Sincronizar ahora").size(12.5)).clicked() {
                        action = Some(Action::GoogleSync);
                    }
                    if ui.link(RichText::new("Desconectar").size(12.5)).clicked() {
                        action = Some(Action::GoogleDisconnect);
                    }
                }
            }
        });
        action
    }

    /// Respuestas del hilo de Google y sincronización cuando cambió la agenda (o cada 10 min).
    fn handle_gcal(&mut self) {
        let Some(g) = &mut self.gcal else { return };
        let was_connected = g.connected;
        let msgs = g.poll();
        if g.connected && !was_connected {
            self.gcal_dirty = true;
        }
        let since = self.gcal_last_try.elapsed();
        if g.connected && !g.busy && since >= Duration::from_secs(3) && (self.gcal_dirty || since >= GCAL_EVERY) {
            g.sync(gcal::desired_items(&self.agenda));
            self.gcal_dirty = false;
            self.gcal_last_try = Instant::now();
        }
        for m in msgs {
            self.msg(m);
        }
    }

    fn agenda_view(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let today = today();
        let tomorrow = (Local::now() + chrono::Duration::days(1)).format("%Y-%m-%d").to_string();

        // (fecha, hora, texto, espacio o calendario, nota, tarea (su línea), es de un calendario agregado)
        let mut items: Vec<(String, String, String, String, Option<String>, Option<String>, bool)> = self
            .all_events()
            .into_iter()
            .map(|e| {
                let ext = self.is_external(&e);
                (e.date, e.time.unwrap_or_default(), e.title, e.project, e.note, None, ext)
            })
            .collect();
        let tasks = self.agenda.tasks();
        let overdue: Vec<_> = tasks
            .iter()
            .filter(|t| !t.done && t.due.as_deref().is_some_and(|d| agenda::is_date(d) && *d < *today))
            .cloned()
            .collect();
        items.extend(tasks.into_iter().filter(|t| !t.done).filter_map(|t| {
            let d = t.due.filter(|d| agenda::is_date(d))?;
            Some((d, String::new(), agenda::display_text(&t.text), t.project, t.note, Some(t.raw), false))
        }));
        items.retain(|i| i.0 >= today);
        items.sort();

        let root = self.vault.root.clone();
        Self::column(ui, "agenda", |ui, _| {
            let subtitle = "Tus calendarios, los eventos de tus notas y las tareas con fecha".to_string();
            view_header(ui, "Agenda", &subtitle);
            let subs = self.cfg.calendarios.clone();
            if let Some(a) = calendars_ui::calendars_panel(ui, &subs, &self.cals, &mut self.cal_form) {
                action = Some(a);
            }
            ui.add_space(4.0);
            let n = self.recurring().len();
            let label = if n == 0 { "Reuniones y notas que se repiten…".to_string() } else { format!("Reuniones y notas que se repiten ({n})") };
            if ui.link(RichText::new(format!("{} {label}", icon::ARROWS_CLOCKWISE)).size(12.5)).clicked() {
                action = Some(Action::OpenRecurring);
            }
            ui.add_space(14.0);
            if !overdue.is_empty() {
                ui.label(RichText::new("Atrasadas").font(theme::bold(15.0)).color(RED));
                ui.add_space(4.0);
                for t in &overdue {
                    if let Some(a) = task_row(ui, t, &today, &root) {
                        action = Some(a);
                    }
                }
                ui.add_space(14.0);
            }
            if items.is_empty() {
                ui.label(RichText::new("Nada agendado. La IA agrega aquí las fechas que menciones en tus notas.").color(MUTED));
            }
            let mut current = String::new();
            for (date, time, text, project, note, task, external) in &items {
                if *date != current {
                    current = date.clone();
                    let label = if *date == today {
                        format!("Hoy · {}", long_date(date))
                    } else if *date == tomorrow {
                        format!("Mañana · {}", long_date(date))
                    } else {
                        long_date(date)
                    };
                    ui.add_space(8.0);
                    ui.label(RichText::new(label).font(theme::bold(15.0)).color(if *date == today { ACCENT } else { TEXT }));
                    ui.add_space(2.0);
                }
                let mut job = LayoutJob::default();
                if *external {
                    job.append("● ", 0.0, fmt(FontId::proportional(13.0), theme::tag_colors(project).dot));
                }
                if task.is_none() {
                    let lead = if time.is_empty() { format!("{}  ", icon::CALENDAR_BLANK) } else { format!("{time}  ") };
                    job.append(&lead, 0.0, fmt(FontId::proportional(14.0), MUTED));
                }
                job.append(text, 0.0, fmt(FontId::proportional(14.5), TEXT));
                if !project.is_empty() {
                    job.append(&format!("   {project}"), 0.0, fmt(FontId::proportional(12.5), MUTED));
                }
                ui.horizontal(|ui| {
                    if let Some(raw) = task {
                        let b = egui::Button::new(RichText::new(icon::SQUARE).size(18.0).color(MUTED)).frame(false);
                        if ui.add(b).on_hover_text("Marcar hecha").clicked() {
                            action = Some(Action::ToggleTask(raw.clone()));
                        }
                    }
                    let r = clickable_line(ui, job);
                    if let Some(n) = note {
                        if r.clicked() {
                            action = Some(Action::Open(root.join(format!("{n}.md")), None));
                        }
                    }
                    // Tomar notas de un evento de hoy: abre una reunión con su nombre.
                    if *external && *date == today {
                        ui.add_space(8.0);
                        if ui.link(RichText::new(format!("{} Tomar notas", icon::NOTE_PENCIL)).size(12.5)).clicked() {
                            action = Some(Action::StartMeetingNamed(text.clone()));
                        }
                    }
                });
            }
            ui.add_space(24.0);
            ui.separator();
            ui.add_space(8.0);
            if let Some(a) = self.google_panel(ui) {
                action = Some(a);
            }
        });
        if self.esc(ui) {
            action = Some(Action::CloseResults);
        }
        action
    }
}

impl eframe::App for NotesApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.close_meeting(Local::now());
        self.save();
        self.save_estado();
        self.vault.save_cache_now();
    }
}

impl NotesApp {
    /// Un cuadro completo de la ventana (también lo usa el banco de pruebas de rendimiento).
    fn frame(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        count_frame(&ctx);
        // El cursor parpadea solo mientras se usa la app (ver CURSOR_REST).
        if ctx.input(|i| !i.events.is_empty()) {
            self.last_input = Instant::now();
        }
        let blinks = self.last_input.elapsed() < CURSOR_REST;
        if blinks != self.cursor_blinks {
            self.cursor_blinks = blinks;
            ctx.all_styles_mut(|s| s.visuals.text_cursor.blink = blinks);
        }
        let mut actions: Vec<Action> = Vec::new();
        actions.extend(self.shortcuts(&ctx));

        if self.last_poll.elapsed() >= POLL {
            self.poll();
            self.last_poll = Instant::now();
            if let Some(m) = &self.meeting {
                if m.last_activity.elapsed() >= MEETING_IDLE {
                    let end = m.last_time;
                    self.close_meeting(end);
                }
            }
            self.queue_ai();
            self.maybe_sync_todo();
        }
        self.handle_ai_results();
        self.handle_gcal();
        self.handle_todo();
        self.handle_calendars();
        self.handle_mail();
        self.poll_ask();

        egui::Panel::bottom("status")
            .exact_size(26.0)
            .frame(Frame::new().fill(BG_SIDE).inner_margin(Margin::symmetric(12, 0)))
            .show(ui, |ui| actions.extend(self.status_bar(ui)));
        egui::Panel::left("rail")
            .exact_size(64.0)
            .resizable(false)
            .frame(Frame::new().fill(BG_RAIL).inner_margin(Margin::symmetric(0, 12)))
            .show(ui, |ui| actions.extend(self.rail(ui)));
        egui::Panel::left("sidebar")
            .default_size(250.0)
            .size_range(180.0..=420.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(BG_SIDE).inner_margin(Margin::same(12)))
            .show(ui, |ui| {
                // Línea divisoria propia (la de egui queda oscura en paneles redimensionables).
                let r = ui.max_rect().expand2(egui::vec2(12.0, 12.0));
                ui.painter().vline(r.right() - 0.5, r.y_range(), Stroke::new(1.0, theme::BORDER));
                actions.extend(self.sidebar(ui))
            });
        egui::Panel::top("pestanas")
            .exact_size(36.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(BG_SIDE).inner_margin(Margin { left: 6, right: 6, top: 5, bottom: 0 }))
            .show(ui, |ui| self.tab_bar(ui));
        egui::CentralPanel::default().frame(Frame::new().fill(BG_EDITOR)).show(ui, |ui| {
            if !self.search.trim().is_empty() {
                actions.extend(self.results(ui));
            } else {
                match self.view.clone() {
                    View::Editor => self.editor(ui),
                    View::Home => actions.extend(self.home_view(ui)),
                    View::Mail => actions.extend(self.mail_view(ui)),
                    View::Week => actions.extend(self.week_view(ui)),
                    View::Ai => actions.extend(self.ai_view(ui)),
                    View::Tag(_) => actions.extend(self.results(ui)),
                    View::Tasks => actions.extend(self.tasks_view(ui)),
                    View::Agenda => actions.extend(self.agenda_view(ui)),
                    View::Trash => actions.extend(self.trash_view(ui)),
                }
            }
        });
        actions.extend(self.toast_ui(&ctx));

        for a in actions {
            self.apply(a);
        }
        self.sync_tab();
        self.settings_window(&ctx);
        self.followup_window(&ctx);
        self.confirm_window(&ctx);
        self.forever_window(&ctx);
        self.recurring_window(&ctx);
        if let Some(a) = self.rename_tag_window(&ctx) {
            self.apply(a);
        }

        if self.note.dirty && self.note.last_edit.elapsed() >= AUTOSAVE {
            self.save();
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            self.close_meeting(Local::now());
            self.save();
            self.save_estado();
        }

        let title = format!("{} — Notas", display_title(&self.note.title));
        if title != self.window_title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
        ctx.request_repaint_after(if self.note.dirty { AUTOSAVE } else { POLL });
    }
}

// ---------- Widgets ----------

/// Un acceso de la barra izquierda: ícono con su nombre debajo.
fn rail_item(ui: &mut Ui, glyph: &str, label: &str, tip: &str, selected: bool, color: Color32) -> Response {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(56.0, 50.0), Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 8, ACCENT_BG);
    } else if r.hovered() {
        p.rect_filled(rect, 8, HOVER);
    }
    let color = if selected && color == TEXT { ACCENT } else { color };
    p.text(egui::pos2(rect.center().x, rect.top() + 18.0), Align2::CENTER_CENTER, glyph, FontId::proportional(20.0), color);
    let label_color = if selected { ACCENT } else { MUTED };
    p.text(egui::pos2(rect.center().x, rect.bottom() - 10.0), Align2::CENTER_CENTER, label, FontId::proportional(11.0), label_color);
    r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip)
}

/// Número sobre un acceso de la barra izquierda.
fn rail_badge(ui: &Ui, r: &Response, n: usize, color: Color32) {
    let c = egui::pos2(r.rect.center().x + 13.0, r.rect.top() + 9.0);
    ui.painter().circle_filled(c, 7.5, color);
    ui.painter().text(c, Align2::CENTER_CENTER, n.min(9).to_string(), FontId::proportional(10.5), Color32::WHITE);
}

/// Encabezado de sección; devuelve true si se presionó su botón "+".
fn section(ui: &mut Ui, title: &str, add_tip: Option<&str>) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(12.5).color(MUTED));
        if let Some(tip) = add_tip {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let b = egui::Button::new(RichText::new(icon::PLUS).size(14.0).color(MUTED)).frame(false);
                clicked = ui.add(b).on_hover_text(tip).clicked();
            });
        }
    });
    clicked
}

/// Título grande de una vista (se cierra con la ✕ de su pestaña o con Esc).
fn view_header(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).font(theme::bold(26.0)));
    ui.label(RichText::new(subtitle).size(13.0).color(MUTED));
    ui.add_space(14.0);
}

/// Línea de texto clicable con borde al pasar el mouse.
fn clickable_line(ui: &mut Ui, job: LayoutJob) -> Response {
    let r = ui.add(egui::Label::new(job).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand);
    if r.hovered() {
        ui.painter().rect_stroke(r.rect.expand(3.0), 4, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Outside);
    }
    r
}

/// Orden de Tareas. Por fecha: la fecha límite (sin fecha al final). Recientes primero: las
/// atrasadas arriba (para que no se pierdan) y después lo último que se agregó.
fn sort_tasks(tasks: Vec<agenda::Task>, by_due: bool, today: &str) -> Vec<agenda::Task> {
    // La posición en tareas.txt desempata: lo que está más abajo se agregó después.
    let mut v: Vec<(usize, agenda::Task)> = tasks.into_iter().enumerate().collect();
    if by_due {
        v.sort_by(|(_, a), (_, b)| (a.due.is_none(), a.due.clone(), a.text.to_lowercase()).cmp(&(b.due.is_none(), b.due.clone(), b.text.to_lowercase())));
    } else {
        let late = |t: &agenda::Task| !t.done && t.due.as_deref().is_some_and(|d| agenda::is_date(d) && d < today);
        v.sort_by(|(ia, a), (ib, b)| {
            late(b).cmp(&late(a)).then_with(|| if late(a) { a.due.cmp(&b.due) } else { std::cmp::Ordering::Equal }).then_with(|| b.created.cmp(&a.created)).then_with(|| ib.cmp(ia))
        });
    }
    v.into_iter().map(|(_, t)| t).collect()
}

fn task_row(ui: &mut Ui, t: &agenda::Task, today: &str, root: &Path) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        let (glyph, color) = if t.done { (icon::CHECK_SQUARE, ACCENT) } else { (icon::SQUARE, MUTED) };
        let check = egui::Button::new(RichText::new(glyph).size(18.0).color(color)).frame(false);
        if ui.add(check).on_hover_text(if t.done { "Marcar pendiente" } else { "Marcar hecha" }).clicked() {
            action = Some(Action::ToggleTask(t.raw.clone()));
        }
        let mut job = LayoutJob::default();
        let mut body = fmt(FontId::proportional(14.5), if t.done { MUTED } else { TEXT });
        if t.done {
            body.strikethrough = Stroke::new(1.0, MUTED);
        }
        job.append(&agenda::display_text(&t.text), 0.0, body);
        if let Some(d) = &t.due {
            let color = if !t.done && d.as_str() < today { RED } else if d == today { ACCENT } else { MUTED };
            let label = if d == today { "hoy".to_string() } else { long_date(d) };
            job.append(&format!("   {} {label}", icon::CALENDAR_BLANK), 0.0, fmt(FontId::proportional(12.5), color));
        }
        if !t.project.is_empty() {
            job.append(&format!("   {}", t.project), 0.0, fmt(FontId::proportional(12.5), MUTED));
        }
        let r = ui.add(egui::Label::new(job).wrap().sense(Sense::click()));
        if r.double_clicked() {
            action = Some(Action::ToggleTask(t.raw.clone()));
        }
        if let Some(n) = &t.note {
            let b = egui::Button::new(RichText::new(icon::ARROW_SQUARE_OUT).size(14.0).color(MUTED)).frame(false);
            if ui.add(b).on_hover_text(format!("Abrir nota «{n}»")).clicked() {
                action = Some(Action::Open(root.join(format!("{n}.md")), None));
            }
        }
        if let Some(m) = &t.mail {
            let b = egui::Button::new(RichText::new(icon::ENVELOPE_SIMPLE).size(14.0).color(MUTED)).frame(false);
            if ui.add(b).on_hover_text("Salió de un correo: verlo").clicked() {
                action = Some(Action::OpenMail(m.clone()));
            }
        }
    });
    action
}

/// Etiqueta como píldora de color (sin '#'), con su cantidad.
fn tag_pill(ui: &mut Ui, tag: &str, count: usize, selected: bool) -> Response {
    let c = theme::tag_colors(tag);
    let name = ui.painter().layout_no_wrap(tag.to_string(), FontId::proportional(12.5), c.text);
    let num = ui.painter().layout_no_wrap(count.to_string(), FontId::proportional(11.0), c.text.gamma_multiply(0.65));
    let w = 17.0 + name.size().x + 6.0 + num.size().x + 9.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 22.0), Sense::click());
    let p = ui.painter();
    let fill = if selected { c.border } else { c.bg };
    p.rect_filled(rect, 11.0, fill);
    let stroke = if selected || resp.hovered() { Stroke::new(1.5, c.dot) } else { Stroke::new(1.0, c.border) };
    p.rect_stroke(rect, 11.0, stroke, egui::StrokeKind::Inside);
    let cy = rect.center().y;
    p.circle_filled(egui::pos2(rect.left() + 9.5, cy), 2.8, c.dot);
    let x = rect.left() + 17.0;
    p.galley(egui::pos2(x, cy - name.size().y / 2.0), name.clone(), c.text);
    p.galley(egui::pos2(x + name.size().x + 6.0, cy - num.size().y / 2.0), num, c.text);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Al pasar el mouse por una fila, un tacho a la derecha; devuelve true si se hizo clic en él.
fn row_trash_button(ui: &Ui, r: &Response, tip: &str) -> bool {
    if !r.hovered() {
        return false;
    }
    let rect = egui::Rect::from_min_size(egui::pos2(r.rect.right() - 28.0, r.rect.top()), egui::vec2(28.0, r.rect.height()));
    let over = ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p));
    let p = ui.painter();
    p.rect_filled(rect.shrink(2.0), 5, if over { Color32::from_rgb(250, 225, 225) } else { HOVER });
    p.text(rect.center(), Align2::CENTER_CENTER, icon::TRASH, FontId::proportional(14.0), if over { RED } else { MUTED });
    if over {
        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), Id::new("tacho"), egui::PopupAnchor::Pointer).show(|ui| ui.label(tip));
    }
    over && r.clicked()
}

/// Fila de lista a todo el ancho: ícono + texto recortado a la izquierda, dato a la derecha.
fn list_row(ui: &mut Ui, glyph: &str, text: &str, right: &str, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), Sense::click_and_drag());
    let bg = if selected {
        ACCENT_BG
    } else if resp.hovered() {
        HOVER
    } else {
        Color32::TRANSPARENT
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 6, bg);
    let color = if selected { ACCENT } else { TEXT };
    let x = rect.left() + 30.0;
    painter.text(egui::pos2(rect.left() + 8.0, rect.center().y), Align2::LEFT_CENTER, glyph, FontId::proportional(15.0), if selected { ACCENT } else { MUTED });
    let mut right_w = 0.0;
    if !right.is_empty() {
        let g = painter.layout_no_wrap(right.to_string(), FontId::proportional(12.0), MUTED);
        right_w = g.size().x + 10.0;
        painter.galley(egui::pos2(rect.right() - 8.0 - g.size().x, rect.center().y - g.size().y / 2.0), g, MUTED);
    }
    let mut job = LayoutJob::simple_singleline(text.to_string(), FontId::proportional(14.0), color);
    job.wrap = egui::text::TextWrapping {
        max_width: (rect.right() - 8.0 - right_w - x).max(10.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let g = painter.layout_job(job);
    painter.galley(egui::pos2(x, rect.center().y - g.size().y / 2.0), g, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------- Resaltado del editor ----------

fn fmt(font: FontId, color: Color32) -> TextFormat {
    let line_height = Some(font.size * 1.55);
    TextFormat { font_id: font, color, line_height, ..Default::default() }
}

/// Aplica formato a una línea: títulos, hora de reunión, etiquetas, tareas hechas.
fn append_line(job: &mut LayoutJob, line: &str, size: f32) {
    let content = line.trim_end_matches(['\n', '\r']);
    let trimmed = content.trim_start();
    let heading = [("### ", 17.0), ("## ", 19.0), ("# ", 23.0)].into_iter().find(|(p, _)| trimmed.starts_with(p));
    if let Some((_, hsize)) = heading {
        job.append(line, 0.0, fmt(theme::bold(hsize), TEXT));
        return;
    }
    let done = trimmed.starts_with("- [x]") || trimmed.starts_with("[x]");
    let mut body = fmt(FontId::proportional(size), if done { MUTED } else { TEXT });
    if done {
        body.strikethrough = Stroke::new(1.0, MUTED);
    }
    let mut pos = 0;
    // "- 15:03 " de las reuniones, en gris.
    if content.len() >= 8 && content.starts_with("- ") && agenda::is_time(&content[2..7]) {
        job.append(&line[..8.min(line.len())], 0.0, fmt(FontId::proportional(size), MUTED));
        pos = 8.min(line.len());
    }
    // Etiquetas: con su color y sin el '#'.
    for (a, b) in tags::tag_spans(content) {
        if a < pos {
            continue;
        }
        let c = theme::tag_colors(&content[a + 1..b]);
        let mut tag = fmt(FontId::proportional(size - 1.0), c.text);
        tag.background = c.bg;
        tag.expand_bg = 3.0;
        job.append(&line[pos..a], 0.0, body.clone());
        job.append(&line[a + 1..b], 4.0, tag);
        pos = b;
    }
    job.append(&line[pos..], 0.0, body);
}

/// Una línea para mostrar fuera del editor: sin "^id", con la fecha legible y la casilla como ícono.
fn display_line(line: &str) -> String {
    let info = lines::parse(line);
    let mut out = match info.check {
        Some(true) => format!("{} ", icon::CHECK_SQUARE),
        Some(false) => format!("{} ", icon::SQUARE),
        None => String::new(),
    };
    let start = if info.check.is_some() { info.prefix } else { 0 };
    let words: Vec<String> = line[start..]
        .split(' ')
        .filter(|w| !(w.starts_with('^') && lines::id_of(w).is_some()))
        .map(|w| match w.strip_prefix("due:").filter(|d| agenda::is_date(d)) {
            Some(d) => format!("{} {}", icon::CALENDAR_BLANK, long_date(d)),
            None => w.to_string(),
        })
        .collect();
    out += &words.join(" ");
    out
}

fn highlight_line(line: &str, size: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    append_line(&mut job, line, size);
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recientes primero: las atrasadas arriba y después lo último agregado; o por fecha límite.
    #[test]
    fn tasks_sort_recent_first_or_by_due() {
        let lines = [
            "2026-09-20 Vieja con fecha lejana due:2026-12-01",
            "2026-09-21 Atrasada due:2026-09-25",
            "2026-09-29 Nueva sin fecha",
            "2026-09-29 Nueva con fecha due:2026-10-10",
        ];
        let tasks: Vec<agenda::Task> = lines.iter().filter_map(|l| agenda::parse_task(l)).collect();
        let texts = |v: Vec<agenda::Task>| v.into_iter().map(|t| t.text).collect::<Vec<_>>();
        assert_eq!(texts(sort_tasks(tasks.clone(), false, "2026-09-30")), ["Atrasada", "Nueva con fecha", "Nueva sin fecha", "Vieja con fecha lejana"]);
        assert_eq!(texts(sort_tasks(tasks, true, "2026-09-30")), ["Atrasada", "Nueva con fecha", "Vieja con fecha lejana", "Nueva sin fecha"]);
        assert_eq!(agenda::parse_task("x 2026-09-30 2026-09-20 Hecha").map(|t| (t.done_on, t.created)), Some((Some("2026-09-30".into()), Some("2026-09-20".into()))));
    }

    /// Lo que llega de otro equipo mientras se escribe aquí se junta línea por línea: nada se
    /// pisa, nada se pierde y no quedan copias "(conflicto)".
    #[test]
    fn changes_from_another_device_are_merged() {
        let dir = std::env::temp_dir().join(format!("nodex-juntar-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("General")).unwrap();
        let path = dir.join("General").join("Obra.md");
        fs::write(&path, "Llamar a Pedro\nRevisar planos\nComprar pan\n").unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.open(path.clone(), None);
        let outside = |text: &str| {
            std::thread::sleep(Duration::from_millis(30)); // otra fecha de modificación
            fs::write(&path, text).unwrap();
        };

        // 1. Aquí se agrega una línea; en otro equipo la IA se lleva dos. La app lo nota al revisar.
        app.note.text = "Llamar a Pedro\nRevisar planos\nComprar pan\nIdea nueva\n".into();
        app.note.dirty = true;
        outside("Comprar pan\n");
        app.poll();
        assert_eq!(app.note.text, "Comprar pan\nIdea nueva\n");
        assert!(app.note.dirty, "lo juntado todavía hay que guardarlo");
        app.save();
        assert_eq!(fs::read_to_string(&path).unwrap(), "Comprar pan\nIdea nueva\n");

        // 2. El cambio de afuera llega justo antes del autoguardado (sin que la app alcance a revisar).
        app.note.text = "Comprar pan\nIdea nueva\nOtra mía\n".into();
        app.note.dirty = true;
        outside("Arriba, del otro equipo\nComprar pan\nIdea nueva\n");
        app.save();
        assert_eq!(fs::read_to_string(&path).unwrap(), "Arriba, del otro equipo\nComprar pan\nIdea nueva\nOtra mía\n");
        assert_eq!(app.note.text, "Arriba, del otro equipo\nComprar pan\nIdea nueva\nOtra mía\n");

        // 3. La misma línea cambia en los dos: quedan las dos versiones y se avisa.
        app.note.text = app.note.text.replace("Comprar pan", "Comprar pan integral");
        app.note.dirty = true;
        outside("Arriba, del otro equipo\nComprar pan y leche\nIdea nueva\nOtra mía\n");
        app.save();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("Comprar pan integral\nComprar pan y leche\n"), "{text}");
        assert!(app.message.as_ref().is_some_and(|(m, _)| m.contains("quedaron las dos versiones")), "{:?}", app.message);

        // 4. Sin cambios aquí, simplemente se toma lo de afuera.
        outside("Todo nuevo\n");
        app.poll();
        assert_eq!((app.note.text.as_str(), app.note.dirty), ("Todo nuevo\n", false));
        // Nunca queda una copia "(conflicto)".
        let names: Vec<String> = fs::read_dir(dir.join("General")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["Obra.md"]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// El cursor deja de parpadear tras un rato sin tocar nada y vuelve a parpadear al usar la app.
    #[test]
    fn cursor_rests_when_idle() {
        let dir = std::env::temp_dir().join(format!("nodex-cursor-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("General")).unwrap();
        fs::write(dir.join("General").join("A.md"), "hola").unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let ctx = egui::Context::default();
        theme::setup(&ctx);
        let mut app = NotesApp::new(cfg, None, ctx.clone());
        let frame = |app: &mut NotesApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0))),
                ..Default::default()
            };
            ctx.run_ui(input, |ui| app.frame(ui)).drop_without_applying_deltas();
        };
        frame(&mut app, vec![]);
        assert!(ctx.global_style().visuals.text_cursor.blink, "recién abierta, parpadea");
        app.last_input = Instant::now().checked_sub(CURSOR_REST + Duration::from_secs(1)).expect("reloj");
        frame(&mut app, vec![]);
        assert!(!ctx.global_style().visuals.text_cursor.blink, "quieta: cursor fijo");
        frame(&mut app, vec![egui::Event::PointerMoved(egui::pos2(300.0, 300.0))]);
        assert!(ctx.global_style().visuals.text_cursor.blink, "al usarla, vuelve a parpadear");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Buscar: sin tildes ni mayúsculas, una sola vez por texto, y de nuevo si cambia una nota.
    #[test]
    fn search_is_cached_and_ignores_accents() {
        let dir = std::env::temp_dir().join(format!("nodex-buscar-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("General")).unwrap();
        fs::create_dir_all(dir.join("Obra")).unwrap();
        fs::write(dir.join("General").join("Muro.md"), "Revisar la Cubicación del muro #obra\notra cosa\n").unwrap();
        fs::write(dir.join("Obra").join("Losa.md"), "cubicacion de la losa\n").unwrap();
        fs::write(dir.join("Obra").join("Cubicaciones.md"), "sin la palabra\n").unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.search = "CUBICACIÓN".into();
        let f = app.found();
        let lines: usize = f.hits.iter().map(|h| h.lines.len()).sum();
        assert_eq!((f.hits.len(), lines), (3, 2), "dos líneas, más la nota que solo coincide en el título");
        let muro = f.hits.iter().find(|h| h.title == "Muro").unwrap();
        assert_eq!(muro.lines[0], ("Revisar la Cubicación del muro #obra".chars().count(), "Revisar la Cubicación del muro #obra".to_string()));
        // Otra vez lo mismo: se reutiliza.
        let generation = app.found().generation;
        assert_eq!(app.found().generation, generation);
        // Cambia una nota: se vuelve a buscar.
        let path = dir.join("General").join("Otra.md");
        fs::write(&path, "más cubicaciones").unwrap();
        app.vault.upsert(path, "más cubicaciones".into(), SystemTime::now());
        assert_eq!(app.found().hits.len(), 4);
        // El resumen de la barra lateral e Inicio se reutiliza hasta que cambia una nota.
        app.ws = "Obra".into();
        let a = app.summary();
        assert!(std::rc::Rc::ptr_eq(&a, &app.summary()));
        assert_eq!(a.others.iter().map(|n| n.title.as_str()).collect::<HashSet<_>>(), HashSet::from(["Losa", "Cubicaciones"]));
        let path = dir.join("Obra").join("Nueva.md");
        fs::write(&path, "hola #obra").unwrap();
        app.vault.upsert(path, "hola #obra".into(), SystemTime::now());
        let b = app.summary();
        assert!(!std::rc::Rc::ptr_eq(&a, &b));
        assert_eq!((b.others.len(), b.tags.clone()), (3, vec![("obra".to_string(), 1)]));
        // Etiqueta: solo en el espacio actual.
        app.search.clear();
        app.ws = "General".into();
        app.view = View::Tag("obra".into());
        let f = app.found();
        assert_eq!((f.hits.len(), f.hits[0].lines.len()), (1, 1));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn daily_notes_show_friendly_names() {
        let day = |n: i64| (Local::now() + chrono::Duration::days(n)).format("%Y-%m-%d").to_string();
        assert_eq!(display_title(&day(0)), "Hoy");
        assert_eq!(display_title(&day(-1)), "Ayer");
        assert_eq!(display_title("Reunión CIC"), "Reunión CIC");
        assert!(day_heading(&day(0)).unwrap().starts_with("Hoy, "));
        assert!(day_heading("Sin título").is_none());
        let old = day(-10);
        let d = NaiveDate::parse_from_str(&old, "%Y-%m-%d").unwrap();
        assert!(display_title(&old).contains(&format!("{} {}", d.day(), MESES[d.month0() as usize])), "{}", display_title(&old));
    }

    /// Una carpeta vacía recibe la nota de bienvenida, abierta, y la IA no la organiza.
    #[test]
    fn empty_folder_gets_a_welcome_note() {
        let dir = std::env::temp_dir().join(format!("nodex-bienvenida-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let app = NotesApp::new(cfg.clone(), None, egui::Context::default());
        let path = dir.join(vault::DEFAULT_WORKSPACE).join("Bienvenida.md");
        assert_eq!(fs::read_to_string(&path).unwrap(), WELCOME);
        assert_eq!((app.note.path.clone(), app.view.clone()), (path.clone(), View::Editor));
        assert!(app.unorganized().is_empty());
        // La segunda vez ya no (la carpeta tiene notas).
        fs::remove_file(&path).unwrap();
        fs::write(dir.join(vault::DEFAULT_WORKSPACE).join("Otra.md"), "hola").unwrap();
        let _ = NotesApp::new(cfg, None, egui::Context::default());
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn meeting_helpers() {
        assert_eq!(
            meeting_header("## Estructuras · 2026-09-24 15:00\n- 15:03 x"),
            Some(("Estructuras".into(), "2026-09-24 15:00".into()))
        );
        assert!(is_empty_stamp("- 15:03 "));
        assert!(!is_empty_stamp("- 15:03 revisar"));
        assert!(is_meeting("Llamada con Juan\n#reunión"));
    }

    /// Nota del día: cada nota (con sus detalles) va a su nota, con sus etiquetas y su casilla;
    /// lo no atribuido se queda; las casillas se sincronizan con tareas.txt; Deshacer lo revierte.
    #[test]
    fn capture_units_move_to_their_notes_and_undo() {
        let dir = std::env::temp_dir().join(format!("nodex-captura-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // Configuración y estado de prueba fuera de la carpeta real del usuario.
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", dir.join(".config")) };
        for ws in ["General", "Consorcio", "Docencia", "Diario"] {
            fs::create_dir_all(dir.join(ws)).unwrap();
        }
        let daily = dir.join("Diario").join("2026-09-24.md");
        let original = "Debo entregar el informe de revisión de las trincheras\n\
            Las trincheras enstán en la carpeta dropbox /workspace/proeyctos/activos/consorcio/04 Trincheras\n\
            Debo entregar mañana el informe a la UTalca\n\
            Debo entregar la proxima semana el LaVet\n\
            \n\
            ## Reunión Estructuras · 2026-09-24 15:00\n\
            - 15:03 revisar vigas eje 3\n\
            ## fin · 15:42\n";
        fs::write(&daily, original).unwrap();
        let trincheras = dir.join("Consorcio").join("Trincheras.md");
        fs::write(&trincheras, "# Trincheras\n\nRevisión de taludes #trincheras\n").unwrap();

        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        let a: Analysis = serde_json::from_str(
            r#"{"unidades": [
                {"id": "L1", "espacio": "Consorcio", "nota": "Trincheras", "etiquetas": ["informe"]},
                {"id": "L2", "de": "L1", "etiquetas": ["trincheras"]},
                {"id": "L3", "espacio": "Docencia", "nota": "Informe UTalca", "etiquetas": ["utalca"]},
                {"id": "L4", "espacio": "Docencia", "etiquetas": ["lavet"]},
                {"id": "B6", "espacio": "Consorcio", "nota": "Reunión Estructuras", "es_reunion": true, "resumen": "Se revisaron las vigas del eje 3."}],
              "tareas": [
                {"texto": "Entregar el informe a la UTalca", "fecha": "2026-09-25", "unidad": "L3"},
                {"texto": "Entregar el LaVet", "fecha": "2026-10-02", "unidad": "L4"}]}"#,
        )
        .unwrap();
        app.apply_analysis(daily.clone(), ai::fnv(original), a);

        // La línea 1 va a la nota existente "Trincheras" y la 2 se va con ella, como detalle.
        let t = fs::read_to_string(&trincheras).unwrap();
        assert_eq!(
            t,
            "# Trincheras\n\nRevisión de taludes #trincheras\n\
             Debo entregar el informe de revisión de las trincheras #informe\n\
             \x20 Las trincheras enstán en la carpeta dropbox /workspace/proeyctos/activos/consorcio/04 Trincheras #trincheras\n"
        );
        // La línea 3 crea una nota nueva en Docencia, como tarea con fecha e identificador.
        let u = fs::read_to_string(dir.join("Docencia").join("Informe UTalca.md")).unwrap();
        assert!(u.starts_with("- [ ] Debo entregar mañana el informe a la UTalca #utalca due:2026-09-25 ^"), "{u}");
        let utalca_id = lines::id_of(u.trim_end()).unwrap();
        // El bloque de reunión se mueve entero, con resumen y #reunión.
        let r = fs::read_to_string(dir.join("Consorcio").join("Reunión Estructuras.md")).unwrap();
        assert_eq!(r, "## Reunión Estructuras · 2026-09-24 15:00\n- 15:03 revisar vigas eje 3\n## fin · 15:42\n### Resumen\nSe revisaron las vigas del eje 3.\n#reunión\n");
        // La línea 4 no tiene nota: se queda en la nota del día, con su etiqueta y su casilla (y su
        // tarea, en el espacio que dijo la IA).
        let d = fs::read_to_string(&daily).unwrap();
        assert!(d.starts_with("- [ ] Debo entregar la proxima semana el LaVet #lavet due:2026-10-02 ^"), "{d}");
        assert_eq!(d.lines().count(), 1);
        let lavet_id = lines::id_of(d.trim_end()).unwrap();
        // Tareas: cada una apunta a su nota y a su línea.
        let tasks = fs::read_to_string(dir.join("tareas.txt")).unwrap();
        assert!(tasks.contains(&format!("Entregar el informe a la UTalca +Docencia due:2026-09-25 nota:Docencia/Informe%20UTalca id:{utalca_id}")), "{tasks}");
        assert!(tasks.contains(&format!("Entregar el LaVet +Docencia due:2026-10-02 nota:Diario/2026-09-24 id:{lavet_id}")), "{tasks}");

        // Marcar la casilla en la nota marca la tarea; desmarcarla en Tareas desmarca la línea.
        app.open(daily.clone(), None);
        app.toggle_line_check(0);
        assert!(fs::read_to_string(&daily).unwrap().starts_with("- [x] Debo entregar"));
        let raw = app.agenda.tasks().into_iter().find(|t| t.id.as_deref() == Some(lavet_id.as_str())).unwrap();
        assert!(raw.done, "{}", raw.raw);
        app.apply(Action::ToggleTask(raw.raw));
        assert!(fs::read_to_string(&daily).unwrap().starts_with("- [ ] Debo entregar"));
        assert!(app.agenda.tasks().iter().all(|t| !t.done));

        // Deshacer deja todo como antes.
        app.undo_ai();
        assert_eq!(fs::read_to_string(&daily).unwrap(), original);
        assert_eq!(fs::read_to_string(&trincheras).unwrap(), "# Trincheras\n\nRevisión de taludes #trincheras\n");
        assert!(!dir.join("Docencia").join("Informe UTalca.md").exists());
        assert!(!dir.join("Consorcio").join("Reunión Estructuras.md").exists());
        assert_eq!(fs::read_to_string(dir.join("tareas.txt")).unwrap_or_default(), "");
        let _ = fs::remove_dir_all(&dir);
    }
}
