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

mod ask_view;
mod doubts_ui;
mod editor;
mod followup;
mod home;
mod settings;
mod spaces_ui;
mod tabs;
mod today;
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
/// Tiempo durante el que se ofrece deshacer lo que hizo la IA.
const UNDO_WINDOW: Duration = Duration::from_secs(120);
/// Sincronización periódica con Google Calendar aunque no haya cambios.
const GCAL_EVERY: Duration = Duration::from_secs(10 * 60);
const EDITOR_SIZE: f32 = 15.5;
const COLUMN_MAX: f32 = 780.0;
const MESES: [&str; 12] = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"];
const DIAS: [&str; 7] = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];
const RED: Color32 = Color32::from_rgb(198, 40, 40);

struct OpenNote {
    path: PathBuf,
    /// Texto del campo de título (se aplica al archivo al salir del campo).
    title: String,
    text: String,
    dirty: bool,
    last_edit: Instant,
    /// Fecha de modificación en disco cuando se leyó o guardó; `None` = aún no existe.
    disk_mtime: Option<SystemTime>,
}

impl OpenNote {
    fn load(path: PathBuf) -> Self {
        OpenNote {
            text: vault::read_text(&path).unwrap_or_default(),
            disk_mtime: vault::modified(&path),
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
}

#[derive(PartialEq, Clone, Debug)]
enum View {
    Editor,
    Home,
    Today,
    Week,
    Ask,
    Tag(String),
    Tasks,
    Agenda,
}

enum Action {
    Open(PathBuf, Option<usize>),
    NewNote,
    Today,
    SelectWorkspace(String),
    CreateWorkspace(String),
    Trash(PathBuf),
    ShowTag(String),
    Show(View),
    CloseResults,
    FocusSearch,
    StartMeeting,
    CloseMeeting,
    Organize,
    Undo,
    GoogleConnect,
    GoogleSync,
    GoogleDisconnect,
    ToggleTask(String),
    AddTask(String),
    OpenExternal(PathBuf),
    OpenSettings(Section),
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
    /// Notas escritas en esta sesión, candidatas al análisis automático.
    touched: HashSet<PathBuf>,
    /// Cola del botón Organizar.
    backlog: VecDeque<PathBuf>,
    backlog_total: usize,
    in_flight: Option<PathBuf>,
    undo: Option<Undo>,
    gcal: Option<GCal>,
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
    /// Campos de Inicio: anotar y preguntar rápido.
    home_capture: String,
    home_question: String,
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

fn workspace_of(path: &Path) -> Option<String> {
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
        let path = if !estado.nota.is_empty() && last.is_file() { last } else { vault.note_path(&ws, &today()) };
        let ws = workspace_of(&path).unwrap_or(ws);
        let analyzed = load_analyzed(&vault.root);
        let estado_hoy = estado.hoy.clone();
        let estado_semana = estado.semana.clone();
        let estado_tabs = estado.pestanas.clone();
        let estado_tab = estado.pestana;
        let mut app = NotesApp {
            cfg,
            ctx,
            settings: None,
            vault,
            agenda,
            ws,
            note: OpenNote::load(path),
            view: View::Editor,
            search: String::new(),
            new_ws: None,
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
            analyzed,
            touched: HashSet::new(),
            backlog: VecDeque::new(),
            backlog_total: 0,
            in_flight: None,
            undo: None,
            gcal,
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
            followup: followup::FollowUp::default(),
            tabs: tabs::Tabs::default(),
            home_capture: String::new(),
            home_question: String::new(),
        };
        // Pestañas de la sesión anterior (o la nota que estaba abierta).
        let saved: Vec<tabs::Tab> = estado_tabs.iter().filter_map(|t| tabs::decode(t, &app.vault.root)).collect();
        app.tabs = if saved.is_empty() {
            tabs::Tabs { list: vec![tabs::Tab::View(View::Home), tabs::Tab::Note(app.note.path.clone())], active: 1 }
        } else {
            let active = estado_tab.min(saved.len() - 1);
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
        let active = app.tabs.active;
        app.activate_tab(active);
        app.focus_editor = app.view == View::Editor;
        // Solo en compilaciones de prueba: abrir Preguntar con una pregunta (para capturas).
        #[cfg(debug_assertions)]
        if let Ok(q) = std::env::var("NODEX_DEMO_ASK") {
            app.view = View::Ask;
            app.ask(q);
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
        self.cfg.carpeta_notas = path.clone();
        self.save_config();
        self.vault = Vault::new(path);
        self.agenda = Agenda::new(&self.vault.root);
        self.links = editor::LinkCache::new(&self.vault.root);
        self.doubts = doubts::Store::load(&self.vault.root);
        self.ideas = crate::spaces::Ideas::load(&self.vault.root);
        self.analyzed = load_analyzed(&self.vault.root);
        self.touched.clear();
        self.backlog.clear();
        self.in_flight = None;
        self.undo = None;
        self.restart_gcal();
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
        match fs::write(&n.path, &n.text) {
            Ok(()) => {
                n.dirty = false;
                n.disk_mtime = vault::modified(&n.path);
                if let Some(m) = n.disk_mtime {
                    self.vault.upsert(n.path.clone(), n.text.clone(), m);
                }
                self.touched.insert(self.note.path.clone());
            }
            Err(e) => {
                n.last_edit = Instant::now(); // reintenta en el próximo ciclo
                self.msg(format!("No se pudo guardar: {e}"));
            }
        }
    }

    fn save_estado(&self) {
        let nota = self.note.path.strip_prefix(&self.vault.root).unwrap_or(&self.note.path);
        config::save_estado(&Estado {
            espacio: self.ws.clone(),
            nota: nota.to_string_lossy().into_owned(),
            hoy: self.today_shown.clone(),
            semana: self.week_seen.clone(),
            pestanas: self.tabs.list.iter().map(|t| tabs::encode(t, &self.vault.root)).collect(),
            pestana: self.tabs.active,
        });
    }

    fn save_analyzed(&self) {
        let dir = self.vault.root.join(".nodex");
        let _ = fs::create_dir_all(&dir);
        let mut lines: Vec<String> = self.analyzed.iter().map(|h| format!("{h:016x}")).collect();
        lines.sort();
        let _ = fs::write(dir.join("analizadas.txt"), lines.join("\n") + "\n");
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

    fn new_note(&mut self) {
        self.save();
        let path = self.vault.unique_path(&self.ws, "Sin título");
        self.note = OpenNote::load(path);
        self.view = View::Editor;
        self.search.clear();
        self.focus_title = true;
    }

    fn select_workspace(&mut self, ws: String) {
        self.search.clear();
        let path = match self.vault.notes_in(&ws).first() {
            Some(n) => n.path.clone(),
            None => self.vault.note_path(&ws, &today()),
        };
        self.open(path, Some(usize::MAX));
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
        if path.exists() {
            if let Err(e) = self.vault.trash(&path) {
                self.msg(format!("No se pudo eliminar: {e}"));
                return;
            }
            self.msg("Nota movida a .papelera");
        }
        if self.meeting.as_ref().is_some_and(|m| m.path == path) {
            self.meeting = None;
        }
        self.touched.remove(&path);
        if is_open {
            self.note.dirty = false;
            self.note.disk_mtime = None;
            let ws = self.ws.clone();
            self.select_workspace(ws);
        }
    }

    /// Detecta cambios hechos por fuera (otra app, Dropbox).
    fn poll(&mut self) {
        self.vault.scan();
        let m = vault::modified(&self.note.path);
        if m.is_none() || m == self.note.disk_mtime {
            return;
        }
        let Ok(disk_text) = vault::read_text(&self.note.path) else { return };
        self.note.disk_mtime = m;
        if disk_text == self.note.text {
            return;
        }
        if !self.note.dirty {
            self.note.text = disk_text;
            self.msg("Nota actualizada con cambios de otro equipo");
        } else {
            // Ambos cambiaron: se conserva lo escrito aquí y la otra versión queda como copia.
            let ws = workspace_of(&self.note.path).unwrap_or_else(|| self.ws.clone());
            let name = format!("{} (conflicto {})", self.note.title, Local::now().format("%H.%M"));
            let copy = self.vault.unique_path(&ws, &name);
            let _ = fs::write(&copy, disk_text);
            self.msg(format!("La nota cambió en otro equipo; esa versión quedó en «{}»", vault::stem(&copy)));
        }
    }

    // ---------- Reuniones ----------

    fn start_meeting(&mut self) {
        self.close_meeting(Local::now());
        self.save();
        let now = Local::now();
        let path = self.vault.unique_path(&self.ws, &format!("Reunión {}", now.format("%Y-%m-%d %H.%M")));
        self.note = OpenNote::load(path.clone());
        self.note.text = format!("## Reunión · {}\n- {} ", now.format("%Y-%m-%d %H:%M"), now.format("%H:%M"));
        self.note.dirty = true;
        self.save();
        self.meeting = Some(Meeting {
            path,
            title: "Reunión".into(),
            started: now,
            last_activity: Instant::now(),
            last_time: now,
        });
        self.view = View::Editor;
        self.search.clear();
        self.focus_title = true;
        self.pending_cursor = Some(usize::MAX);
        self.msg("Reunión iniciada: escribe su nombre y Enter; cada línea lleva su hora. Esc la cierra.");
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
        let meeting = self.meeting.as_ref().map(|m| m.path.clone());
        self.backlog = self
            .vault
            .all_notes()
            .into_iter()
            .filter(|n| Some(&n.path) != meeting.as_ref() && !self.analyzed.contains(&ai::fnv(&n.text)))
            .filter(|n| n.text.split_whitespace().count() >= 3)
            .map(|n| n.path.clone())
            .collect();
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
            match r.result {
                Ok(a) => self.apply_analysis(r.path, r.hash, a),
                Err(e) => {
                    self.touched.remove(&r.path);
                    self.backlog.clear(); // sin conexión o clave inválida: no insistir
                    self.msg(format!("IA: {}", e.chars().take(160).collect::<String>()));
                }
            }
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
        let (mut src_tasks, mut src_events, mut other_tasks, mut other_events) = (vec![], vec![], vec![], vec![]);
        // Acuerdos de reuniones: los de otros llevan "@Nombre" (lo que se espera de cada uno).
        let meeting_units: HashSet<String> = plan.agreements.iter().map(|g| g.unit.clone()).collect();
        for g in &plan.agreements {
            let text = if g.who.is_empty() { g.what.clone() } else { format!("{} @{}", g.what, g.who.split_whitespace().collect::<Vec<_>>().join("_")) };
            match place(&g.unit) {
                Some((rel, w)) => other_tasks.push(agenda::format_task(&created, &text, &w, g.due.as_deref(), &rel, Some(&g.id))),
                None => src_tasks.push(agenda::format_task(&created, &text, &ws, g.due.as_deref(), &source_rel, Some(&g.id))),
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
                None => src_tasks.push(agenda::format_task(&created, &t.texto, &ws, due, &source_rel, id.as_deref())),
            }
        }
        for e in a.eventos.iter().filter(|e| agenda::is_date(e.fecha.trim()) && !e.titulo.trim().is_empty()) {
            let time = Some(e.hora.trim()).filter(|h| agenda::is_time(h));
            match place(&e.unidad) {
                Some((rel, w)) => other_events.push(agenda::format_event(e.fecha.trim(), time, &e.titulo, &w, &rel)),
                None => src_events.push(agenda::format_event(e.fecha.trim(), time, &e.titulo, &ws, &source_rel)),
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
            self.ws = workspace_of(&new_path).unwrap_or(ws);
            self.save_estado();
        } else if written.iter().any(|(p, _, _)| *p == self.note.path) && !self.note.dirty {
            self.note = OpenNote::load(self.note.path.clone());
        }
        if done.is_empty() {
            return;
        }
        self.undo = Some(Undo {
            files,
            renamed: (new_path != path).then(|| (path.clone(), new_path.clone())),
            agenda: snapshot,
            at: Instant::now(),
            moved: Vec::new(),
            created_dir: None,
        });
        let name = if emptied { String::new() } else { format!(" {}:", vault::stem(&new_path)) };
        self.msg(format!("IA ·{name} {}", done.join(" · ")));
    }

    /// Deja la casilla de la línea "^id" de una nota como hecha o pendiente.
    fn sync_task_line(&mut self, note_rel: &str, id: &str, done: bool) {
        let path = self.vault.root.join(format!("{note_rel}.md"));
        let fix = |text: &str| -> Option<String> {
            let (idx, line) = text.split('\n').enumerate().find(|(_, l)| lines::id_of(l).as_deref() == Some(id))?;
            let line = line.trim_end_matches('\r');
            lines::parse(line)
                .check
                .is_some_and(|d| d != done)
                .then(|| editor::replace_line(text, idx, &lines::toggle_check(line)))
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
        for (orig, new) in u.moved.iter().rev() {
            let _ = fs::rename(new, orig);
            if self.note.path == *new {
                self.note.path = orig.clone();
            }
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
        let _ = self.agenda.restore(&u.agenda);
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
        self.msg("Se deshizo lo que hizo la IA");
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Open(p, c) => {
                self.search.clear();
                self.open(p, c)
            }
            Action::NewNote => self.new_note(),
            Action::Today => {
                self.search.clear();
                let p = self.vault.note_path(&self.ws, &today());
                self.open(p, Some(usize::MAX));
            }
            Action::SelectWorkspace(ws) => self.select_workspace(ws),
            Action::CreateWorkspace(name) => match self.vault.create_workspace(&name) {
                Ok(ws) => self.select_workspace(ws),
                Err(e) => self.msg(format!("No se pudo crear el espacio: {e}")),
            },
            Action::Trash(p) => self.trash(p),
            Action::ShowTag(t) => {
                self.save();
                self.search.clear();
                self.view = if self.view == View::Tag(t.clone()) { View::Editor } else { View::Tag(t) };
                self.focus_editor = self.view == View::Editor;
            }
            Action::Show(v) => {
                self.save();
                self.search.clear();
                self.view = if self.view == v { View::Editor } else { v };
                self.focus_editor = self.view == View::Editor;
                self.ask.focus = self.view == View::Ask;
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
                let (text, due) = match text.split_once("due:") {
                    Some((t, d)) if agenda::is_date(d.trim()) => (t.trim().to_string(), Some(d.trim().to_string())),
                    _ => (text.trim().to_string(), None),
                };
                let line = agenda::format_task(&today(), &text, &self.ws, due.as_deref(), &self.rel(&self.note.path), None);
                if let Err(e) = self.agenda.add_task(line) {
                    self.msg(format!("No se pudo escribir tareas.txt: {e}"));
                }
            }
            Action::OpenExternal(p) => open_external(&p),
            Action::OpenSettings(section) => self.open_settings(section),
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
            return Some(Action::ShowTab(View::Today));
        }
        if pressed(Key::K) {
            return Some(Action::ShowTab(View::Ask));
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
        let is_today = vault::stem(&self.note.path) == today() && self.view == View::Editor;
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            if rail_button(ui, icon::HOUSE, "Inicio: un resumen de todo", self.view == View::Home, TEXT).clicked() {
                action = Some(Action::ShowTab(View::Home));
            }
            if rail_button(ui, icon::NOTE_PENCIL, "Nueva nota (Ctrl+N)", false, TEXT).clicked() {
                action = Some(Action::NewNote);
            }
            if rail_button(ui, icon::SUN, "Nota de hoy (Ctrl+D)", is_today, TEXT).clicked() {
                action = Some(Action::Today);
            }
            if rail_button(ui, icon::MAGNIFYING_GLASS, "Buscar (Ctrl+F)", false, TEXT).clicked() {
                action = Some(Action::FocusSearch);
            }
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
            let (tip, color) = match &self.meeting {
                Some(m) => (format!("Cerrar reunión «{}» (Esc)", m.title), SUCCESS),
                None => ("Nueva reunión (Ctrl+R)".to_string(), TEXT),
            };
            if rail_button(ui, icon::USERS, &tip, self.meeting.is_some(), color).clicked() {
                action = Some(if self.meeting.is_some() { Action::CloseMeeting } else { Action::StartMeeting });
            }
            let tip = if self.ask.busy() { "Preguntar: buscando la respuesta…" } else { "Preguntar a tus notas (Ctrl+K)" };
            if rail_button(ui, icon::CHAT_CIRCLE_TEXT, tip, self.view == View::Ask, if self.ask.busy() { ACCENT } else { TEXT }).clicked() {
                action = Some(Action::ShowTab(View::Ask));
            }
            if rail_button(ui, icon::TRAY, "Hoy: atrasado, preguntas de la IA y la semana (Ctrl+H)", self.view == View::Today, TEXT).clicked() {
                action = Some(Action::ShowTab(View::Today));
            }
            if rail_button(ui, icon::CHECK_SQUARE, "Tareas", self.view == View::Tasks, TEXT).clicked() {
                action = Some(Action::ShowTab(View::Tasks));
            }
            if rail_button(ui, icon::CALENDAR_BLANK, "Agenda", self.view == View::Agenda, TEXT).clicked() {
                action = Some(Action::Show(View::Agenda));
            }
            let (tip, color) = match &self.ai {
                Ok(ai) => (format!("Organizar con IA las notas pendientes ({})", ai.label), TEXT),
                Err(e) => (format!("IA no disponible: {e}"), MUTED),
            };
            let r = rail_button(ui, icon::SPARKLE, &tip, !self.backlog.is_empty(), color);
            if r.clicked() {
                action = Some(Action::Organize);
            }
            // Cuántas preguntas y sugerencias de la IA esperan respuesta (en Hoy o en su nota).
            let asks = self.doubts.pending.len() + self.ideas.ready().count();
            if asks > 0 {
                let c = r.rect.right_top() + egui::vec2(-7.0, 7.0);
                ui.painter().circle_filled(c, 7.5, ACCENT);
                ui.painter().text(c, Align2::CENTER_CENTER, asks.min(9).to_string(), FontId::proportional(10.5), Color32::WHITE);
                r.on_hover_text(format!("La IA tiene {} (en Hoy)", plural(asks, "pregunta")));
            }
            ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                if rail_button(ui, icon::GEAR, "Configuración (Ctrl+,)", self.settings.is_some(), TEXT).clicked() {
                    action = Some(Action::OpenSettings(Section::General));
                }
                if rail_button(ui, icon::FOLDER_OPEN, "Abrir carpeta de notas", false, TEXT).clicked() {
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
            // Espacios
            if section(ui, "Espacios", Some("Nuevo espacio")) {
                self.new_ws = Some(String::new());
            }
            for ws in self.vault.workspaces.clone() {
                if list_row(ui, icon::FOLDER_SIMPLE, &ws, "", ws == self.ws).clicked() {
                    action = Some(Action::SelectWorkspace(ws));
                }
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
            if section(ui, "Notas", Some("Nueva nota (Ctrl+N)")) {
                action = Some(Action::NewNote);
            }
            let notes: Vec<(PathBuf, String, SystemTime, bool)> = self
                .vault
                .notes_in(&self.ws)
                .iter()
                .map(|n| (n.path.clone(), n.title.clone(), n.modified, is_meeting(&n.text)))
                .collect();
            let open_is_new = self.note.disk_mtime.is_none();
            if open_is_new && workspace_of(&self.note.path).as_deref() == Some(self.ws.as_str()) {
                list_row(ui, icon::FILE_TEXT, &self.note.title, "nueva", self.view == View::Editor);
            }
            let live = self.meeting.as_ref().map(|m| m.path.clone());
            for (path, title, modified, meeting) in notes {
                let selected = path == self.note.path && self.view == View::Editor;
                let glyph = if meeting { icon::USERS } else { icon::FILE_TEXT };
                let right = if live.as_ref() == Some(&path) { "en curso".to_string() } else { short_date(modified) };
                let r = list_row(ui, glyph, &title, &right, selected).on_hover_text("Ctrl+clic: abrir en otra pestaña");
                if r.middle_clicked() || (r.clicked() && ui.input(|i| i.modifiers.command)) {
                    action = Some(Action::OpenNewTab(path.clone()));
                } else if r.clicked() {
                    action = Some(Action::Open(path.clone(), None));
                }
                r.context_menu(|ui| {
                    if ui.button(format!("{}  Mover a la papelera", icon::TRASH)).clicked() {
                        action = Some(Action::Trash(path.clone()));
                        ui.close();
                    }
                });
            }
            if self.vault.notes_in(&self.ws).is_empty() && !open_is_new {
                ui.label(RichText::new("Escribe algo para crear la primera nota.").color(MUTED).size(12.5));
            }
            ui.add_space(14.0);

            // Etiquetas del espacio
            let tags = self.vault.tag_counts(&self.ws);
            section(ui, "Etiquetas", None);
            if tags.is_empty() {
                ui.label(RichText::new("Escribe #palabra en una nota.").color(MUTED).size(12.5));
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(5.0, 6.0);
                for (tag, count) in tags {
                    let selected = self.view == View::Tag(tag.clone());
                    if tag_pill(ui, &tag, count, selected).on_hover_text(format!("Ver las líneas con #{tag}")).clicked() {
                        action = Some(Action::ShowTag(tag));
                    }
                }
            });
        });
        action
    }

    fn status_bar(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        ui.horizontal_centered(|ui| {
            // Reunión en curso (siempre visible).
            if let Some(m) = &self.meeting {
                let mins = (Local::now() - m.started).num_minutes();
                let text = RichText::new(format!("{} {} · {mins} min · Esc para cerrar", icon::RECORD, m.title))
                    .size(12.0)
                    .color(SUCCESS);
                let r = ui.add(egui::Label::new(text).sense(Sense::click())).on_hover_text("Ir a la reunión");
                if r.clicked() {
                    action = Some(Action::Open(m.path.clone(), Some(usize::MAX)));
                }
                ui.add_space(12.0);
            }
            let (glyph, text, color) = if self.note.dirty {
                (icon::CIRCLE_NOTCH, "Guardando…", MUTED)
            } else if self.note.disk_mtime.is_none() {
                (icon::FILE_TEXT, "Nota nueva — se guarda al escribir", MUTED)
            } else {
                (icon::CHECK_CIRCLE, "Guardado", SUCCESS)
            };
            ui.label(RichText::new(format!("{glyph} {text}")).size(12.0).color(color));
            let words = self.note.text.split_whitespace().count();
            ui.label(RichText::new(format!("   {words} palabras")).size(12.0).color(MUTED));
            // IA trabajando.
            if let Some(p) = &self.in_flight {
                let progress = if self.backlog_total > 0 {
                    format!(" ({}/{})", self.backlog_total - self.backlog.len(), self.backlog_total)
                } else {
                    String::new()
                };
                ui.label(
                    RichText::new(format!("   {} Analizando «{}»{progress}", icon::SPARKLE, vault::stem(p)))
                        .size(12.0)
                        .color(ACCENT),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.undo.as_ref().is_some_and(|u| u.at.elapsed() < UNDO_WINDOW) {
                    let b = egui::Button::new(RichText::new(format!("{} Deshacer", icon::ARROW_COUNTER_CLOCKWISE)).size(12.0));
                    if ui.add(b).clicked() {
                        action = Some(Action::Undo);
                    }
                }
                let msg = self.message.as_ref().filter(|(_, t)| t.elapsed() < Duration::from_secs(10));
                let text = match msg {
                    Some((m, _)) => RichText::new(m).color(TEXT),
                    None => RichText::new(self.note.path.display().to_string()).color(MUTED),
                };
                ui.add(egui::Label::new(text.size(12.0)).truncate());
            });
        });
        action
    }

    /// Columna centrada con desplazamiento, común a todas las vistas.
    fn column<R>(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui, f32) -> R) -> R {
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
                        ui.add_space(26.0);
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

    /// Lista de líneas que tienen una etiqueta o coinciden con la búsqueda.
    fn results(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let query = self.search.trim().to_lowercase();
        let tag = match &self.view {
            View::Tag(t) if query.is_empty() => Some(t.clone()),
            _ => None,
        };

        struct Hit {
            path: PathBuf,
            title: String,
            ws: String,
            lines: Vec<(usize, String)>,
        }
        let notes = match &tag {
            Some(_) => self.vault.notes_in(&self.ws),
            None => self.vault.all_notes(),
        };
        let mut hits = Vec::new();
        for n in notes {
            let mut lines = Vec::new();
            let mut offset = 0; // en caracteres
            for line in n.text.split_inclusive('\n') {
                let content = line.trim_end();
                let matches = match &tag {
                    Some(t) => tags::line_tags(content).contains(t),
                    None => content.to_lowercase().contains(&query),
                };
                if matches {
                    // Cursor al final de la línea, listo para seguir escribiendo.
                    lines.push((offset + content.chars().count(), content.trim().to_string()));
                }
                offset += line.chars().count();
            }
            let title_match = tag.is_none() && n.title.to_lowercase().contains(&query);
            if !lines.is_empty() || title_match {
                hits.push(Hit { path: n.path.clone(), title: n.title.clone(), ws: n.workspace.clone(), lines });
            }
        }

        let heading = match &tag {
            Some(t) => t.clone(),
            None => format!("Buscar «{}»", self.search.trim()),
        };
        let total: usize = hits.iter().map(|h| h.lines.len()).sum();
        let scope = if tag.is_some() { format!("en {}", self.ws) } else { "en todos los espacios".into() };
        let subtitle = format!("{} en {}, {scope}", plural(total, "línea"), plural(hits.len(), "nota"));
        Self::column(ui, "results", |ui, _| {
            if view_header(ui, &heading, &subtitle) {
                action = Some(Action::CloseResults);
            }
            if hits.is_empty() {
                ui.label(RichText::new("Sin resultados.").color(MUTED));
            }
            for h in &hits {
                let r = ui.add(
                    egui::Label::new(RichText::new(&h.title).font(theme::bold(15.0)).color(TEXT)).sense(Sense::click()),
                );
                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    action = Some(Action::Open(h.path.clone(), None));
                }
                if tag.is_none() {
                    ui.label(RichText::new(&h.ws).size(12.0).color(MUTED));
                }
                for (offset, line) in &h.lines {
                    if clickable_line(ui, highlight_line(&display_line(line), EDITOR_SIZE - 1.0)).clicked() {
                        action = Some(Action::Open(h.path.clone(), Some(*offset)));
                    }
                }
                ui.add_space(14.0);
            }
        });
        if self.esc(ui) {
            action = Some(Action::CloseResults);
        }
        action
    }

    fn tasks_view(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let mut tasks = self.agenda.tasks();
        let today = today();
        tasks.sort_by(|a, b| {
            (a.due.is_none(), a.due.clone(), a.text.to_lowercase()).cmp(&(b.due.is_none(), b.due.clone(), b.text.to_lowercase()))
        });
        let (pending, done): (Vec<_>, Vec<_>) = tasks.into_iter().partition(|t| !t.done);
        let subtitle = format!("{} · doble clic para marcar como hecha · tareas.txt", plural(pending.len(), "pendiente"));
        let root = self.vault.root.clone();
        let mut typing = false;
        Self::column(ui, "tasks", |ui, _| {
            if view_header(ui, "Tareas", &subtitle) {
                action = Some(Action::CloseResults);
            }
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.new_task)
                    .hint_text(format!("{}  Nueva tarea en {} (agrega due:AAAA-MM-DD para fecha)", icon::PLUS, self.ws))
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(8, 6)),
            );
            typing = r.has_focus();
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !self.new_task.trim().is_empty() {
                action = Some(Action::AddTask(std::mem::take(&mut self.new_task)));
                r.request_focus();
            }
            ui.add_space(12.0);
            if pending.is_empty() {
                ui.label(RichText::new("No hay tareas pendientes. La IA las detecta en tus notas, o agrégalas arriba.").color(MUTED));
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
                        for t in done.iter().rev().take(50) {
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
                    ui.label(
                        RichText::new("Para sincronizar, agrega google_client_id y google_client_secret en config.toml (pasos en el README).")
                            .size(12.5)
                            .color(MUTED),
                    );
                    if ui.link(RichText::new("Configurar Calendar").size(12.5)).clicked() {
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

        // (fecha, hora, texto, espacio, nota, es_tarea)
        let mut items: Vec<(String, String, String, String, Option<String>, bool)> = self
            .agenda
            .events()
            .into_iter()
            .map(|e| (e.date, e.time.unwrap_or_default(), e.title, e.project, e.note, false))
            .collect();
        let tasks = self.agenda.tasks();
        let overdue: Vec<_> = tasks
            .iter()
            .filter(|t| !t.done && t.due.as_deref().is_some_and(|d| agenda::is_date(d) && *d < *today))
            .cloned()
            .collect();
        items.extend(tasks.into_iter().filter(|t| !t.done).filter_map(|t| {
            let d = t.due.filter(|d| agenda::is_date(d))?;
            Some((d, String::new(), t.text, t.project, t.note, true))
        }));
        items.retain(|i| i.0 >= today);
        items.sort();

        let root = self.vault.root.clone();
        Self::column(ui, "agenda", |ui, _| {
            let subtitle = format!("Eventos y tareas con fecha · {} se actualiza solo", agenda::ICS_FILE);
            if view_header(ui, "Agenda", &subtitle) {
                action = Some(Action::CloseResults);
            }
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
            for (date, time, text, project, note, is_task) in &items {
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
                let lead = if *is_task { format!("{}  ", icon::CHECK_SQUARE) } else if time.is_empty() { format!("{}  ", icon::CALENDAR_BLANK) } else { format!("{time}  ") };
                job.append(&lead, 0.0, fmt(FontId::proportional(14.0), MUTED));
                job.append(text, 0.0, fmt(FontId::proportional(14.5), TEXT));
                if !project.is_empty() {
                    job.append(&format!("   {project}"), 0.0, fmt(FontId::proportional(12.5), MUTED));
                }
                let r = clickable_line(ui, job);
                if let Some(n) = note {
                    if r.clicked() {
                        action = Some(Action::Open(root.join(format!("{n}.md")), None));
                    }
                }
            }
            ui.add_space(24.0);
            ui.separator();
            ui.add_space(8.0);
            if let Some(a) = self.google_panel(ui) {
                action = Some(a);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("Para Outlook u otro calendario, importa {}.", agenda::ICS_FILE))
                        .size(12.5)
                        .color(MUTED),
                );
                if ui.link(RichText::new("Abrir carpeta").size(12.5)).clicked() {
                    action = Some(Action::OpenExternal(root.clone()));
                }
            });
        });
        if self.esc(ui) {
            action = Some(Action::CloseResults);
        }
        action
    }
}

impl eframe::App for NotesApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
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
        }
        self.handle_ai_results();
        self.handle_gcal();
        self.poll_ask();

        egui::Panel::bottom("status")
            .exact_size(26.0)
            .frame(Frame::new().fill(BG_SIDE).inner_margin(Margin::symmetric(12, 0)))
            .show(ui, |ui| actions.extend(self.status_bar(ui)));
        egui::Panel::left("rail")
            .exact_size(52.0)
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
                    View::Today => actions.extend(self.today_view(ui)),
                    View::Week => actions.extend(self.week_view(ui)),
                    View::Ask => actions.extend(self.ask_view(ui)),
                    View::Tag(_) => actions.extend(self.results(ui)),
                    View::Tasks => actions.extend(self.tasks_view(ui)),
                    View::Agenda => actions.extend(self.agenda_view(ui)),
                }
            }
        });

        for a in actions {
            self.apply(a);
        }
        self.sync_tab();
        self.settings_window(&ctx);
        self.followup_window(&ctx);

        if self.note.dirty && self.note.last_edit.elapsed() >= AUTOSAVE {
            self.save();
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            self.close_meeting(Local::now());
            self.save();
            self.save_estado();
        }

        let title = format!("{} — Notas", self.note.title);
        if title != self.window_title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
        ctx.request_repaint_after(if self.note.dirty { AUTOSAVE } else { POLL });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.close_meeting(Local::now());
        self.save();
        self.save_estado();
    }
}

// ---------- Widgets ----------

fn rail_button(ui: &mut Ui, glyph: &str, tip: &str, selected: bool, color: Color32) -> Response {
    let b = egui::Button::selectable(selected, RichText::new(glyph).size(20.0).color(color))
        .min_size(egui::vec2(36.0, 36.0))
        .corner_radius(8);
    ui.add(b).on_hover_text(tip)
}

/// Encabezado de sección; devuelve true si se presionó su botón "+".
fn section(ui: &mut Ui, title: &str, add_tip: Option<&str>) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(12.0).color(MUTED));
        if let Some(tip) = add_tip {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let b = egui::Button::new(RichText::new(icon::PLUS).size(14.0).color(MUTED)).frame(false);
                clicked = ui.add(b).on_hover_text(tip).clicked();
            });
        }
    });
    clicked
}

/// Título grande de una vista con botón de cerrar; devuelve true si se cerró.
fn view_header(ui: &mut Ui, title: &str, subtitle: &str) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).font(theme::bold(26.0)));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let b = egui::Button::new(RichText::new(icon::X).size(18.0)).frame(false);
            close = ui.add(b).on_hover_text("Volver a la nota (Esc)").clicked();
        });
    });
    ui.label(RichText::new(subtitle).size(12.5).color(MUTED));
    ui.add_space(14.0);
    close
}

/// Línea de texto clicable con borde al pasar el mouse.
fn clickable_line(ui: &mut Ui, job: LayoutJob) -> Response {
    let r = ui.add(egui::Label::new(job).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand);
    if r.hovered() {
        ui.painter().rect_stroke(r.rect.expand(3.0), 4, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Outside);
    }
    r
}

fn task_row(ui: &mut Ui, t: &agenda::Task, today: &str, root: &Path) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        let (glyph, color) = if t.done { (icon::CHECK_CIRCLE, SUCCESS) } else { (icon::CIRCLE, MUTED) };
        let check = egui::Button::new(RichText::new(glyph).size(18.0).color(color)).frame(false);
        if ui.add(check).on_hover_text(if t.done { "Marcar pendiente" } else { "Marcar hecha" }).clicked() {
            action = Some(Action::ToggleTask(t.raw.clone()));
        }
        let mut job = LayoutJob::default();
        let mut body = fmt(FontId::proportional(14.5), if t.done { MUTED } else { TEXT });
        if t.done {
            body.strikethrough = Stroke::new(1.0, MUTED);
        }
        job.append(&t.text, 0.0, body);
        if let Some(d) = &t.due {
            let color = if !t.done && d.as_str() < today { RED } else if d == today { ACCENT } else { MUTED };
            let label = if d == today { "hoy".to_string() } else { long_date(d) };
            job.append(&format!("   {} {label}", icon::CALENDAR_BLANK), 0.0, fmt(FontId::proportional(12.5), color));
        }
        if !t.project.is_empty() {
            job.append(&format!("   {}", t.project), 0.0, fmt(FontId::proportional(12.5), MUTED));
        }
        let r = ui.add(egui::Label::new(job).sense(Sense::click()));
        if r.double_clicked() {
            action = Some(Action::ToggleTask(t.raw.clone()));
        }
        if let Some(n) = &t.note {
            let b = egui::Button::new(RichText::new(icon::ARROW_SQUARE_OUT).size(14.0).color(MUTED)).frame(false);
            if ui.add(b).on_hover_text(format!("Abrir nota «{n}»")).clicked() {
                action = Some(Action::Open(root.join(format!("{n}.md")), None));
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

/// Fila de lista a todo el ancho: ícono + texto recortado a la izquierda, dato a la derecha.
fn list_row(ui: &mut Ui, glyph: &str, text: &str, right: &str, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 27.0), Sense::click());
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
        for ws in ["General", "Consorcio", "Docencia"] {
            fs::create_dir_all(dir.join(ws)).unwrap();
        }
        let daily = dir.join("General").join("2026-09-24.md");
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
                {"id": "L4", "etiquetas": ["lavet"]},
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
        // La línea 4 no se atribuyó: se queda en la nota del día, con su etiqueta y su casilla.
        let d = fs::read_to_string(&daily).unwrap();
        assert!(d.starts_with("- [ ] Debo entregar la proxima semana el LaVet #lavet due:2026-10-02 ^"), "{d}");
        assert_eq!(d.lines().count(), 1);
        let lavet_id = lines::id_of(d.trim_end()).unwrap();
        // Tareas: cada una apunta a su nota y a su línea.
        let tasks = fs::read_to_string(dir.join("tareas.txt")).unwrap();
        assert!(tasks.contains(&format!("Entregar el informe a la UTalca +Docencia due:2026-09-25 nota:Docencia/Informe%20UTalca id:{utalca_id}")), "{tasks}");
        assert!(tasks.contains(&format!("Entregar el LaVet +General due:2026-10-02 nota:General/2026-09-24 id:{lavet_id}")), "{tasks}");

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
