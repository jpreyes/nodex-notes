//! Ventana principal: barra de herramientas, barra lateral, editor, reuniones, IA, tareas y agenda.

use crate::agenda::{self, Agenda};
use crate::ai::{self, Ai, Analysis};
use crate::config::{self, Config, Estado};
use crate::gcal::{self, GCal};
use crate::tags;
use crate::theme::{self, ACCENT, ACCENT_BG, BG_EDITOR, BG_RAIL, BG_SIDE, HOVER, MUTED, SUCCESS, TEXT};
use crate::vault::{self, Vault};
use chrono::{DateTime, Datelike, Local, NaiveDate};
use eframe::egui::{
    self, text::LayoutJob, Align, Align2, Color32, FontId, Frame, Id, Key, KeyboardShortcut, Layout, Margin,
    Modifiers, Response, RichText, Sense, Stroke, TextFormat, Ui, ViewportCommand,
};
use egui_phosphor::regular as icon;
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

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
const MEETING_TAG: &str = "reunión";
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
    orig_path: PathBuf,
    orig_text: String,
    new_path: PathBuf,
    agenda: (String, String),
    at: Instant,
}

#[derive(PartialEq, Clone)]
enum View {
    Editor,
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
}

pub struct NotesApp {
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
}

/// Un instante "hace mucho" (sin pasar por debajo del arranque del equipo).
fn long_ago() -> Instant {
    let now = Instant::now();
    now.checked_sub(GCAL_EVERY).unwrap_or(now)
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

/// Deja una etiqueta como "palabra-compuesta" (sin '#', minúsculas).
fn clean_tag(t: &str) -> String {
    let t = t.trim().trim_start_matches('#').to_lowercase().replace(' ', "-");
    t.chars().filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect::<String>().trim_matches('-').to_string()
}

/// Agrega etiquetas al final: en la última línea si ya es solo de etiquetas, o en una línea nueva.
fn append_tags(text: &str, tags: &[String]) -> String {
    let body = text.trim_end();
    let add = tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ");
    let last = body.rsplit('\n').next().unwrap_or("");
    let only_tags = !last.trim().is_empty()
        && last.split_whitespace().all(|w| tags::tag_spans(w).first() == Some(&(0, w.len())));
    if only_tags { format!("{body} {add}\n") } else { format!("{body}\n\n{add}\n") }
}

impl NotesApp {
    pub fn new(cfg: Config, cfg_msg: Option<String>, ctx: egui::Context) -> Self {
        let mut message = cfg_msg;
        if let Err(e) = fs::create_dir_all(&cfg.carpeta_notas) {
            message = Some(format!("No se pudo crear {}: {e}", cfg.carpeta_notas.display()));
        }
        let ai = Ai::start(&cfg, ctx.clone());
        let gcal = GCal::start(&cfg.google_client_id, &cfg.google_client_secret, cfg.carpeta_notas.clone(), ctx);
        let ai_auto = cfg.ia_automatica;
        let vault = Vault::new(cfg.carpeta_notas);
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
        let analyzed = vault::read_text(&vault.root.join(".nodex").join("analizadas.txt"))
            .map(|t| t.lines().filter_map(|l| u64::from_str_radix(l.trim(), 16).ok()).collect())
            .unwrap_or_default();
        NotesApp {
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
        }
    }

    fn msg(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), Instant::now()));
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
        config::save_estado(&Estado { espacio: self.ws.clone(), nota: nota.to_string_lossy().into_owned() });
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
                    titles: self.vault.notes_in(w).iter().filter(|n| n.path != path).take(8).map(|n| n.title.clone()).collect(),
                    tags: tags.into_iter().take(12).map(|(t, _)| t).collect(),
                }
            })
            .collect();
        let mut all_tags: Vec<String> = infos.iter().flat_map(|i| i.tags.clone()).collect();
        all_tags.sort();
        all_tags.dedup();
        let text: String = note.text.chars().take(12_000).collect();
        let (system, user) = ai::build_prompt(&note.title, &note.workspace, &text, &infos, &all_tags);
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

    /// Aplica lo que devolvió la IA: etiquetas, resumen, título, espacio, tareas y eventos.
    fn apply_analysis(&mut self, path: PathBuf, hash: u64, a: Analysis) {
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
        let mut new_text = text.clone();
        let mut done: Vec<String> = Vec::new();

        // Resumen de reunión (ya cerrada).
        if a.es_reunion && !a.resumen.trim().is_empty() && text.contains("## fin") && !text.contains("### Resumen") {
            new_text = format!("{}\n\n### Resumen\n{}\n", new_text.trim_end(), a.resumen.trim());
            done.push("resumen".into());
        }
        // Etiquetas nuevas (y #reunión si corresponde).
        let existing: HashSet<String> = text.lines().flat_map(tags::line_tags).collect();
        let mut add: Vec<String> = Vec::new();
        if a.es_reunion {
            add.push(MEETING_TAG.into());
        }
        add.extend(a.etiquetas.iter().map(|t| clean_tag(t)));
        let mut seen = HashSet::new();
        add.retain(|t| !t.is_empty() && !existing.contains(t) && seen.insert(t.clone()));
        add.truncate(5);
        if !add.is_empty() {
            new_text = append_tags(&new_text, &add);
            done.push(add.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" "));
        }
        // Título para notas sin nombre.
        let auto_named = old_title.starts_with("Sin título") || old_title.starts_with("Reunión 20");
        let title = if auto_named && !a.titulo.trim().is_empty() {
            let t = vault::sanitize(&a.titulo);
            done.push(format!("«{t}»"));
            t
        } else {
            old_title.clone()
        };
        if let Some((_, when)) = meeting_header(&new_text).filter(|_| title != old_title) {
            if let Some(end) = new_text.find('\n') {
                new_text.replace_range(..end, &format!("## {title} · {when}"));
            }
        }
        // Espacio de trabajo.
        let espacio = a.espacio.trim();
        let ws = if a.confianza.trim().eq_ignore_ascii_case("alta")
            && espacio != old_ws
            && self.vault.workspaces.iter().any(|w| w == espacio)
        {
            done.push(format!("→ {espacio}"));
            espacio.to_string()
        } else {
            old_ws.clone()
        };

        // Escribir la nota y moverla si cambió de nombre o espacio.
        if new_text != text {
            if let Err(e) = fs::write(&path, &new_text) {
                self.msg(format!("IA: no se pudo guardar la nota: {e}"));
                return;
            }
        }
        let mut new_path = path.clone();
        if ws != old_ws || title != old_title {
            let target = self.vault.unique_path(&ws, &title);
            match fs::rename(&path, &target) {
                Ok(()) => new_path = target,
                Err(e) => self.msg(format!("IA: no se pudo mover la nota: {e}")),
            }
        }

        // Tareas y eventos.
        let note_rel = self.rel(&new_path);
        let created = today();
        let final_ws = workspace_of(&new_path).unwrap_or(ws);
        let tasks: Vec<String> = a
            .tareas
            .iter()
            .filter(|t| !t.texto.trim().is_empty())
            .map(|t| {
                let due = Some(t.fecha.trim()).filter(|d| agenda::is_date(d));
                agenda::format_task(&created, &t.texto, &final_ws, due, &note_rel)
            })
            .collect();
        let events: Vec<String> = a
            .eventos
            .iter()
            .filter(|e| agenda::is_date(e.fecha.trim()) && !e.titulo.trim().is_empty())
            .map(|e| {
                let time = Some(e.hora.trim()).filter(|h| agenda::is_time(h));
                agenda::format_event(e.fecha.trim(), time, &e.titulo, &final_ws, &note_rel)
            })
            .collect();
        if let Err(e) = self.agenda.replace_for_note(&self.rel(&path), &note_rel, &tasks, &events) {
            self.msg(format!("IA: no se pudo escribir tareas/agenda: {e}"));
        }
        self.gcal_dirty = true;
        if !tasks.is_empty() {
            done.push(plural(tasks.len(), "tarea"));
        }
        if !events.is_empty() {
            done.push(format!("{} en agenda", plural(events.len(), "evento")));
        }

        // Estado de la app.
        self.analyzed.insert(ai::fnv(&new_text));
        self.save_analyzed();
        self.touched.remove(&path);
        if let Some(m) = vault::modified(&new_path) {
            self.vault.upsert(new_path.clone(), new_text.clone(), m);
        }
        self.vault.scan();
        if is_open {
            self.note.text = new_text;
            self.note.path = new_path.clone();
            self.note.title = vault::stem(&new_path);
            self.note.disk_mtime = vault::modified(&new_path);
            self.ws = final_ws;
            self.save_estado();
        }
        if done.is_empty() {
            return;
        }
        self.undo = Some(Undo {
            orig_path: path,
            orig_text: text,
            new_path: new_path.clone(),
            agenda: snapshot,
            at: Instant::now(),
        });
        self.msg(format!("IA · {}: {}", vault::stem(&new_path), done.join(" · ")));
    }

    fn undo_ai(&mut self) {
        let Some(u) = self.undo.take() else { return };
        if u.new_path != u.orig_path {
            if let Some(dir) = u.orig_path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::rename(&u.new_path, &u.orig_path);
        }
        let _ = fs::write(&u.orig_path, &u.orig_text);
        let _ = self.agenda.restore(&u.agenda);
        self.analyzed.insert(ai::fnv(&u.orig_text));
        self.save_analyzed();
        self.touched.remove(&u.orig_path);
        self.vault.scan();
        if self.note.path == u.new_path || self.note.path == u.orig_path {
            self.note = OpenNote::load(u.orig_path.clone());
            if let Some(ws) = workspace_of(&u.orig_path) {
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
            }
            Action::AddTask(text) => {
                self.gcal_dirty = true;
                let (text, due) = match text.split_once("due:") {
                    Some((t, d)) if agenda::is_date(d.trim()) => (t.trim().to_string(), Some(d.trim().to_string())),
                    _ => (text.trim().to_string(), None),
                };
                let line = agenda::format_task(&today(), &text, &self.ws, due.as_deref(), &self.rel(&self.note.path));
                if let Err(e) = self.agenda.add_task(line) {
                    self.msg(format!("No se pudo escribir tareas.txt: {e}"));
                }
            }
            Action::OpenExternal(p) => open_external(&p),
        }
    }

    // ---------- Interfaz ----------

    fn shortcuts(&mut self, ctx: &egui::Context) -> Option<Action> {
        let pressed = |k| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, k)));
        if pressed(Key::N) {
            return Some(Action::NewNote);
        }
        if pressed(Key::D) {
            return Some(Action::Today);
        }
        if pressed(Key::F) {
            return Some(Action::FocusSearch);
        }
        if pressed(Key::R) {
            return Some(Action::StartMeeting);
        }
        if pressed(Key::S) {
            self.note.dirty = true;
            self.save();
        }
        // Esc cierra la reunión (si no hay una búsqueda o vista abierta que cerrar primero).
        let esc = ctx.input(|i| i.key_pressed(Key::Escape));
        if esc && self.meeting.is_some() && self.search.is_empty() && self.view == View::Editor && self.new_ws.is_none() {
            return Some(Action::CloseMeeting);
        }
        None
    }

    fn rail(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let is_today = vault::stem(&self.note.path) == today() && self.view == View::Editor;
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
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
            if rail_button(ui, icon::CHECK_SQUARE, "Tareas", self.view == View::Tasks, TEXT).clicked() {
                action = Some(Action::Show(View::Tasks));
            }
            if rail_button(ui, icon::CALENDAR_BLANK, "Agenda", self.view == View::Agenda, TEXT).clicked() {
                action = Some(Action::Show(View::Agenda));
            }
            let (tip, color) = match &self.ai {
                Ok(ai) => (format!("Organizar con IA las notas pendientes ({})", ai.label), TEXT),
                Err(e) => (format!("IA no disponible: {e}"), MUTED),
            };
            if rail_button(ui, icon::SPARKLE, &tip, !self.backlog.is_empty(), color).clicked() {
                action = Some(Action::Organize);
            }
            ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                if rail_button(ui, icon::GEAR, "Configuración (config.toml)", false, TEXT).clicked() {
                    action = Some(Action::OpenExternal(config::config_path()));
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
        if search.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
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
                let r = list_row(ui, glyph, &title, &right, selected);
                if r.clicked() {
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
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                for (tag, count) in tags {
                    let selected = self.view == View::Tag(tag.clone());
                    let text = RichText::new(format!("#{tag}  {count}")).size(12.5);
                    let chip = egui::Button::selectable(selected, text).corner_radius(10);
                    if ui.add(chip).clicked() {
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

    fn editor(&mut self, ui: &mut Ui) {
        let viewport_h = ui.available_height();
        let ctx = ui.ctx().clone();
        Self::column(ui, "editor", |ui, col_w| {
            // Título = nombre del archivo
            let title = ui.add(
                egui::TextEdit::singleline(&mut self.note.title)
                    .id(Id::new("title"))
                    .font(theme::bold(26.0))
                    .frame(Frame::NONE)
                    .hint_text("Sin título")
                    .desired_width(f32::INFINITY),
            );
            if std::mem::take(&mut self.focus_title) {
                title.request_focus();
                if let Some(mut st) = egui::text_edit::TextEditState::load(&ctx, title.id) {
                    let n = self.note.title.chars().count();
                    st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(n),
                    )));
                    st.store(&ctx, title.id);
                }
            }
            if title.lost_focus() {
                self.commit_title();
                if ui.input(|i| i.key_pressed(Key::Enter)) {
                    self.focus_editor = true;
                }
            }

            let in_meeting = self.meeting.as_ref().is_some_and(|m| m.path == self.note.path);
            let when = match self.note.disk_mtime {
                Some(m) => {
                    let d: DateTime<Local> = m.into();
                    format!("editada {} {} {}", d.day(), MESES[d.month0() as usize], d.format("%H:%M"))
                }
                None => "sin guardar".into(),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} {}   ·   {when}", icon::FOLDER_SIMPLE, self.ws)).size(12.5).color(MUTED));
                if in_meeting {
                    ui.label(
                        RichText::new(format!("  {} Reunión en curso", icon::RECORD)).size(12.5).color(SUCCESS),
                    );
                }
            });
            ui.add_space(14.0);

            // Texto
            let id = Id::new(("editor", &self.note.path));
            if let Some(c) = self.pending_cursor.take() {
                let n = self.note.text.chars().count();
                let mut st = egui::text_edit::TextEditState::load(&ctx, id).unwrap_or_default();
                st.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(c.min(n)))));
                st.store(&ctx, id);
            }
            let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
                let mut job = highlight(buf.as_str());
                job.wrap.max_width = wrap;
                ui.fonts_mut(|f| f.layout_job(job))
            };
            let hint = if in_meeting {
                "Escribe lo que se va diciendo; cada Enter agrega la hora…"
            } else {
                "Empieza a escribir…  #etiqueta para clasificar"
            };
            let out = egui::TextEdit::multiline(&mut self.note.text)
                .id(id)
                .frame(Frame::NONE)
                .hint_text(hint)
                .desired_width(f32::INFINITY)
                .min_size(egui::vec2(col_w, (viewport_h - 120.0).max(120.0)))
                .lock_focus(true)
                .layouter(&mut layouter)
                .show(ui);
            if out.response.changed() {
                self.note.dirty = true;
                self.note.last_edit = Instant::now();
                if in_meeting {
                    if let Some(m) = &mut self.meeting {
                        m.last_activity = Instant::now();
                        m.last_time = Local::now();
                    }
                    if ui.input(|i| i.key_pressed(Key::Enter)) {
                        if let Some(cr) = out.cursor_range {
                            self.stamp_new_line(&ctx, id, out.state.clone(), cr.primary.index.0);
                        }
                    }
                }
            }
            if std::mem::take(&mut self.focus_editor) {
                out.response.request_focus();
            }
        });
    }

    /// En una reunión, cada línea nueva empieza con la hora ("- 15:03 ").
    /// Un Enter sobre una línea que solo tiene la hora la deja en blanco.
    fn stamp_new_line(&mut self, ctx: &egui::Context, id: Id, mut st: egui::text_edit::TextEditState, ci: usize) {
        let text = &mut self.note.text;
        let bi = byte_index(text, ci);
        if bi == 0 || !text[..bi].ends_with('\n') {
            return;
        }
        let prev_end = bi - 1;
        let prev_start = text[..prev_end].rfind('\n').map_or(0, |p| p + 1);
        let new_ci = if is_empty_stamp(&text[prev_start..prev_end]) {
            let removed = text[prev_start..prev_end].chars().count();
            text.replace_range(prev_start..prev_end, "");
            ci - removed
        } else {
            let stamp = format!("- {} ", Local::now().format("%H:%M"));
            text.insert_str(bi, &stamp);
            ci + stamp.chars().count()
        };
        st.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(new_ci))));
        st.store(ctx, id);
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
            Some(t) => format!("#{t}"),
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
                    if clickable_line(ui, highlight_line(line, EDITOR_SIZE - 1.0)).clicked() {
                        action = Some(Action::Open(h.path.clone(), Some(*offset)));
                    }
                }
                ui.add_space(14.0);
            }
        });
        if ui.input(|i| i.key_pressed(Key::Escape)) {
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
        if !typing && ui.input(|i| i.key_pressed(Key::Escape)) {
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
                    if ui.link(RichText::new("Abrir configuración").size(12.5)).clicked() {
                        action = Some(Action::OpenExternal(config::config_path()));
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
        if ui.input(|i| i.key_pressed(Key::Escape)) {
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
        egui::CentralPanel::default().frame(Frame::new().fill(BG_EDITOR)).show(ui, |ui| {
            if !self.search.trim().is_empty() {
                actions.extend(self.results(ui));
            } else {
                match self.view.clone() {
                    View::Editor => self.editor(ui),
                    View::Tag(_) => actions.extend(self.results(ui)),
                    View::Tasks => actions.extend(self.tasks_view(ui)),
                    View::Agenda => actions.extend(self.agenda_view(ui)),
                }
            }
        });

        for a in actions {
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
    let mut tag = fmt(FontId::proportional(size), ACCENT);
    tag.background = ACCENT_BG;
    let mut pos = 0;
    // "- 15:03 " de las reuniones, en gris.
    if content.len() >= 8 && content.starts_with("- ") && agenda::is_time(&content[2..7]) {
        job.append(&line[..8.min(line.len())], 0.0, fmt(FontId::proportional(size), MUTED));
        pos = 8.min(line.len());
    }
    for (a, b) in tags::tag_spans(content) {
        if a < pos {
            continue;
        }
        job.append(&line[pos..a], 0.0, body.clone());
        job.append(&line[a..b], 0.0, tag.clone());
        pos = b;
    }
    job.append(&line[pos..], 0.0, body);
}

fn highlight(text: &str) -> LayoutJob {
    let mut job = LayoutJob::default();
    for line in text.split_inclusive('\n') {
        append_line(&mut job, line, EDITOR_SIZE);
    }
    if text.is_empty() || text.ends_with('\n') {
        job.append("", 0.0, fmt(FontId::proportional(EDITOR_SIZE), TEXT));
    }
    job
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

    #[test]
    fn tags_are_appended() {
        assert_eq!(append_tags("texto\n", &["a".into(), "b".into()]), "texto\n\n#a #b\n");
        assert_eq!(append_tags("texto\n\n#a\n", &["b".into()]), "texto\n\n#a #b\n");
        assert_eq!(clean_tag("#Muro Contención"), "muro-contención");
    }
}
