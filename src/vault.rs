//! Carpeta de notas: cada subcarpeta es un espacio de trabajo y cada `.md` una nota. Las notas
//! del día van todas en `Diario/` (una sola por día, fuera de los espacios).
//!
//! ```text
//! Dropbox/Notas/
//!   Diario/
//!     2026-09-24.md   (la nota de ese día)
//!   Proyecto Edificio A/
//!     Cubicaciones losa.md
//!   Docencia/
//!   .papelera/        (notas eliminadas; no se muestra)
//! ```
//!
//! Los cambios en disco (Dropbox, otro programa) llegan por los avisos del sistema operativo
//! (`notify`): cada segundo solo se relee lo que cambió. Una revisión completa de toda la carpeta
//! queda como red de seguridad cada 10 minutos, o cada 30 segundos si no hay avisos.
//!
//! Para no abrir miles de archivos al arrancar (lento «en frío», sobre todo con antivirus), una
//! copia de todas las notas se guarda en un solo archivo local, fuera de Dropbox (`cache_file`).
//! Al abrir se lee esa copia y solo se releen las notas cuya fecha cambió desde entonces.

use crate::tags;
use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::io::{BufWriter, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_WORKSPACE: &str = "General";
/// La carpeta de las notas del día. No es un espacio: no se lista entre ellos.
pub const DIARY: &str = "Diario";
/// Las imágenes pegadas en las notas (capturas). Tampoco es un espacio, ni tiene notas.
pub const ATTACHMENTS: &str = "Adjuntos";
/// Las plantillas. No es un espacio: sus notas no se leen con las demás.
pub const TEMPLATES: &str = "Plantillas";
/// Las notas archivadas (fuera de la vista): `Archivo/<espacio>/<nota>.md`.
pub const ARCHIVE: &str = "Archivo";
const TRASH: &str = ".papelera";
/// De dónde vino cada cosa de la papelera (en `.nodex/`): nombre, ruta original y cuándo.
pub const TRASH_LIST: &str = "papelera.txt";

/// Algo que está en la papelera: una nota u otro archivo, o un espacio completo.
#[derive(Debug, Clone, PartialEq)]
pub struct Trashed {
    pub path: PathBuf,
    pub name: String,
    /// Ruta original, relativa a la carpeta de notas ("Obra/Muro.md"); `None` si no se sabe.
    pub from: Option<String>,
    /// Cuándo se mandó a la papelera ("2026-09-30 17:06").
    pub when: String,
    pub is_dir: bool,
}
/// Revisión completa de seguridad, por si se perdió algún aviso.
const FULL_SCAN_EVERY: Duration = Duration::from_secs(600);
/// Sin avisos del sistema (p. ej. en una carpeta de red): revisión completa cada tanto.
const FALLBACK_SCAN_EVERY: Duration = Duration::from_secs(30);
/// Cada cuánto se guarda la copia local de las notas si algo cambió (y siempre al cerrar).
const CACHE_EVERY: Duration = Duration::from_secs(300);
const CACHE_MAGIC: &[u8] = b"NODEX-NOTAS-1\n";

/// Dónde va la copia local de las notas de una carpeta: en la carpeta local del equipo
/// (`AppData\Local\nodex-notes`), una por carpeta de notas. En pruebas, junto a la
/// configuración de prueba o en la carpeta temporal.
pub fn cache_file(root: &Path) -> PathBuf {
    let dir = if cfg!(test) {
        std::env::temp_dir().join("nodex-cache-pruebas")
    } else if let Some(d) = std::env::var_os("NODEX_CONFIG_DIR").filter(|d| !d.is_empty()) {
        PathBuf::from(d)
    } else {
        dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("nodex-notes")
    };
    dir.join(format!("notas-{:016x}.cache", crate::ai::fnv(&root.to_string_lossy())))
}

fn put_u32(out: &mut Vec<u8>, v: usize) {
    out.extend_from_slice(&(v as u32).to_le_bytes());
}

/// La copia en bytes: por nota, su ruta relativa, su fecha y su texto.
fn encode_cache(root: &Path, notes: &HashMap<PathBuf, Note>) -> Vec<u8> {
    let mut out = Vec::with_capacity(notes.values().map(|n| n.text.len() + 64).sum::<usize>() + 32);
    out.extend_from_slice(CACHE_MAGIC);
    put_u32(&mut out, notes.len());
    for n in notes.values() {
        let rel = n.path.strip_prefix(root).unwrap_or(&n.path).to_string_lossy().replace('\\', "/");
        let t = n.modified.duration_since(UNIX_EPOCH).unwrap_or_default();
        put_u32(&mut out, rel.len());
        out.extend_from_slice(rel.as_bytes());
        out.extend_from_slice(&t.as_secs().to_le_bytes());
        out.extend_from_slice(&t.subsec_nanos().to_le_bytes());
        put_u32(&mut out, n.text.len());
        out.extend_from_slice(n.text.as_bytes());
    }
    out
}

/// Lee la copia; `None` si no existe o no se entiende (entonces se lee todo desde las notas).
fn decode_cache(root: &Path, bytes: &[u8]) -> Option<Vec<(PathBuf, SystemTime, String)>> {
    let mut b = bytes.strip_prefix(CACHE_MAGIC)?;
    let mut take = |n: usize| -> Option<&[u8]> {
        let (a, rest) = (b.get(..n)?, b.get(n..)?);
        b = rest;
        Some(a)
    };
    let u32_at = |x: &[u8]| u32::from_le_bytes(x.try_into().expect("4 bytes")) as usize;
    let count = u32_at(take(4)?);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let len = u32_at(take(4)?);
        let rel = std::str::from_utf8(take(len)?).ok()?.to_string();
        let secs = u64::from_le_bytes(take(8)?.try_into().ok()?);
        let nanos = u32::from_le_bytes(take(4)?.try_into().ok()?);
        let len = u32_at(take(4)?);
        let text = std::str::from_utf8(take(len)?).ok()?.to_string();
        let path = rel.split('/').fold(root.to_path_buf(), |p, part| p.join(part));
        out.push((path, UNIX_EPOCH + Duration::new(secs, nanos), text));
    }
    Some(out)
}

/// Escribe la copia sin dejarla a medias (archivo temporal y luego cambio de nombre).
fn write_cache(file: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut w = BufWriter::new(fs::File::create(&tmp)?);
        w.write_all(bytes)?;
        w.flush()?;
    }
    fs::rename(&tmp, file)
}

pub struct Note {
    pub path: PathBuf,
    pub workspace: String,
    pub title: String,
    pub modified: SystemTime,
    pub text: String,
    /// El texto en minúsculas y sin tildes, para buscar (se prepara la primera vez que se busca).
    folded: std::cell::OnceCell<String>,
    /// Datos que se calculan una sola vez por versión de la nota (la nota se reemplaza al cambiar).
    meeting: std::cell::OnceCell<bool>,
    tags: std::cell::OnceCell<Vec<(String, usize)>>,
    day: std::cell::OnceCell<String>,
    hash: std::cell::OnceCell<u64>,
    task_lines: std::cell::OnceCell<Vec<TaskLine>>,
}

/// Una línea de tarea de una nota: "- [ ] texto due:2026-10-02 ^id".
#[derive(Debug, Clone, PartialEq)]
pub struct TaskLine {
    pub id: String,
    pub done: bool,
    pub due: Option<String>,
    /// El texto de la línea, sin casilla, etiquetas, fecha ni identificador.
    pub text: String,
}

impl Note {
    /// El texto como se compara al buscar (ver `fold`). Tiene las mismas líneas que `text`.
    pub fn folded(&self) -> &str {
        self.folded.get_or_init(|| fold(&self.text))
    }

    /// ¿Es una reunión? (`test` decide, con el texto; el resultado se guarda).
    pub fn meeting(&self, test: impl FnOnce(&str) -> bool) -> bool {
        *self.meeting.get_or_init(|| test(&self.text))
    }

    /// Etiquetas de la nota con cuántas líneas las usan.
    pub fn tags(&self) -> &[(String, usize)] {
        self.tags.get_or_init(|| {
            let mut counts: BTreeMap<String, usize> = BTreeMap::new();
            for line in self.text.lines().filter(|l| l.contains('#')) {
                for t in tags::line_tags(line) {
                    *counts.entry(t).or_default() += 1;
                }
            }
            counts.into_iter().collect()
        })
    }

    /// Día en que se editó por última vez ("2026-09-30", hora de aquí).
    pub fn day(&self) -> &str {
        self.day.get_or_init(|| chrono::DateTime::<chrono::Local>::from(self.modified).format("%Y-%m-%d").to_string())
    }

    /// Las líneas con casilla e identificador de la nota (las tareas que viven en ella).
    pub fn task_lines(&self) -> &[TaskLine] {
        self.task_lines.get_or_init(|| {
            self.text
                .lines()
                .filter(|l| l.contains('^'))
                .filter_map(|l| {
                    let done = crate::lines::parse(l).check?;
                    let id = crate::lines::id_of(l)?;
                    let due = crate::lines::due_of(l).filter(|d| crate::agenda::is_date(d));
                    Some(TaskLine { id, done, due, text: crate::doubts::core(l) })
                })
                .collect()
        })
    }

    /// Huella del texto (para saber si la IA ya lo analizó).
    pub fn hash(&self) -> u64 {
        *self.hash.get_or_init(|| crate::ai::fnv(&self.text))
    }
}

/// Minúsculas y sin tildes, para buscar: «Cubicación» y «cubicacion» son lo mismo.
/// La ñ se mantiene (no es una tilde). Los saltos de línea quedan donde estaban.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'á' | 'à' | 'ä' | 'â' | 'Á' | 'À' | 'Ä' | 'Â' => out.push('a'),
            'é' | 'è' | 'ë' | 'ê' | 'É' | 'È' | 'Ë' | 'Ê' => out.push('e'),
            'í' | 'ì' | 'ï' | 'î' | 'Í' | 'Ì' | 'Ï' | 'Î' => out.push('i'),
            'ó' | 'ò' | 'ö' | 'ô' | 'Ó' | 'Ò' | 'Ö' | 'Ô' => out.push('o'),
            'ú' | 'ù' | 'ü' | 'û' | 'Ú' | 'Ù' | 'Ü' | 'Û' => out.push('u'),
            'Ñ' => out.push('ñ'),
            c if c.is_ascii() => out.push(c.to_ascii_lowercase()),
            c => out.extend(c.to_lowercase()),
        }
    }
    out
}

pub struct Vault {
    pub root: PathBuf,
    pub workspaces: Vec<String>,
    /// La carpeta de las notas del día (como esté escrita en disco; `Diario` si aún no existe).
    diary_dir: PathBuf,
    notes: HashMap<PathBuf, Note>,
    /// Avisos del sistema operativo cuando algo cambia en la carpeta.
    watcher: Option<RecommendedWatcher>,
    events: Option<Receiver<notify::Result<notify::Event>>>,
    last_full: Instant,
    /// Cuántas revisiones completas se hicieron y cuántas notas se leyeron del disco (pruebas).
    pub full_scans: usize,
    /// Sube cada vez que cambia alguna nota (para saber si hay que volver a buscar).
    pub generation: u64,
    /// Cambió algo en `.nodex/` (datos que comparten los equipos) desde la última consulta.
    internal_changed: bool,
    pub reads: usize,
    /// La copia local cambió desde que se guardó, y cuándo se guardó por última vez.
    cache_dirty: bool,
    cache_saved: Instant,
    cache_busy: Arc<AtomicBool>,
    /// Cuándo se guardó la última versión de cada nota en el historial (ver `history`).
    history_last: HashMap<PathBuf, chrono::DateTime<chrono::Local>>,
}

fn is_md(path: &Path) -> bool {
    path.extension().is_some_and(|x| x.eq_ignore_ascii_case("md"))
}

pub fn read_text(path: &Path) -> io::Result<String> {
    let bytes = fs::read(path)?;
    let text = String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    Ok(text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text))
}

pub fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// ¿Es el nombre de la carpeta de las notas del día? (Sin distinguir mayúsculas: en Windows y
/// Mac «diario» es la misma carpeta).
pub fn is_diary_dir(name: &str) -> bool {
    name.eq_ignore_ascii_case(DIARY)
}

/// ¿Es el nombre de la carpeta de las imágenes?
pub fn is_attachments_dir(name: &str) -> bool {
    name.eq_ignore_ascii_case(ATTACHMENTS)
}

/// ¿Es un nombre que no puede tener un espacio? (Diario, Adjuntos, Plantillas, Bloc, Archivo)
pub fn is_reserved_dir(name: &str) -> bool {
    is_diary_dir(name) || is_attachments_dir(name) || name.eq_ignore_ascii_case(TEMPLATES) || name.eq_ignore_ascii_case("Bloc") || name.eq_ignore_ascii_case(ARCHIVE)
}

/// ¿Es una plantilla?
pub fn in_templates(path: &Path) -> bool {
    path.parent().and_then(|p| p.file_name()).is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(TEMPLATES))
}

/// ¿Está la nota en la carpeta de las notas del día?
pub fn in_diary(path: &Path) -> bool {
    path.parent().and_then(|p| p.file_name()).is_some_and(|n| is_diary_dir(&n.to_string_lossy()))
}

pub fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Convierte un título en un nombre de archivo válido en Windows, Mac y Linux.
pub fn sanitize(title: &str) -> String {
    let t: String = title
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { '-' } else { c })
        .collect();
    let t = t.trim().trim_end_matches('.').trim();
    if t.is_empty() { "Sin título".to_string() } else { t.to_string() }
}

impl Vault {
    pub fn new(root: PathBuf) -> Self {
        let mut v = Vault {
            diary_dir: root.join(DIARY),
            root,
            workspaces: Vec::new(),
            notes: HashMap::new(),
            watcher: None,
            events: None,
            last_full: Instant::now(),
            full_scans: 0,
            generation: 0,
            internal_changed: false,
            reads: 0,
            cache_dirty: false,
            cache_saved: Instant::now(),
            cache_busy: Arc::new(AtomicBool::new(false)),
            history_last: HashMap::new(),
        };
        // Primero la copia local (un solo archivo); después solo se relee lo que cambió.
        let cached = v.load_cache();
        v.scan();
        v.watch();
        if !cached || v.cache_dirty {
            v.save_cache();
        }
        v
    }

    /// Carga la copia local de las notas. Devuelve si había una.
    fn load_cache(&mut self) -> bool {
        let file = cache_file(&self.root);
        let Ok(mut f) = fs::File::open(&file) else { return false };
        let mut bytes = Vec::new();
        if f.read_to_end(&mut bytes).is_err() {
            return false;
        }
        let Some(notes) = decode_cache(&self.root, &bytes) else { return false };
        for (path, modified, text) in notes {
            self.upsert(path, text, modified);
        }
        self.cache_dirty = false;
        true
    }

    /// Guarda la copia local en segundo plano (la app no se detiene mientras se escribe).
    pub fn save_cache(&mut self) {
        if self.cache_busy.swap(true, Ordering::SeqCst) {
            return; // ya se está guardando; se hará en la próxima vuelta
        }
        let bytes = encode_cache(&self.root, &self.notes);
        let file = cache_file(&self.root);
        let busy = self.cache_busy.clone();
        self.cache_dirty = false;
        self.cache_saved = Instant::now();
        std::thread::spawn(move || {
            let _ = write_cache(&file, &bytes);
            busy.store(false, Ordering::SeqCst);
        });
    }

    /// Guarda la copia ahora, esperando a que termine (al cerrar la app).
    pub fn save_cache_now(&mut self) {
        while self.cache_busy.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.cache_dirty {
            let _ = write_cache(&cache_file(&self.root), &encode_cache(&self.root, &self.notes));
            self.cache_dirty = false;
        }
    }

    /// Guarda la copia si cambió algo y ya pasó un rato (se llama cada segundo).
    pub fn maybe_save_cache(&mut self) {
        if self.cache_dirty && self.cache_saved.elapsed() >= CACHE_EVERY {
            self.save_cache();
        }
    }

    /// Empieza a recibir los avisos del sistema (si no se puede, se revisa cada 30 segundos).
    fn watch(&mut self) {
        let (tx, rx) = mpsc::channel();
        let Ok(mut w) = notify::recommended_watcher(move |r| {
            let _ = tx.send(r);
        }) else {
            return;
        };
        if w.watch(&self.root, RecursiveMode::Recursive).is_ok() {
            self.watcher = Some(w);
            self.events = Some(rx);
        }
    }

    /// ¿Cambió algo en `.nodex/` desde la última vez que se preguntó?
    pub fn take_internal_changed(&mut self) -> bool {
        std::mem::take(&mut self.internal_changed)
    }

    /// ¿Llegan los avisos del sistema?
    pub fn watching(&self) -> bool {
        self.watcher.is_some()
    }

    /// Aplica lo que cambió en disco (se llama cada segundo). Solo relee las notas avisadas;
    /// revisa todo si se perdieron avisos, si cambió una carpeta o cada tanto como seguridad.
    pub fn refresh(&mut self) {
        let every = if self.watching() { FULL_SCAN_EVERY } else { FALLBACK_SCAN_EVERY };
        if self.last_full.elapsed() >= every {
            self.scan();
            return;
        }
        let Some(rx) = &self.events else { return };
        let mut paths: HashSet<PathBuf> = HashSet::new();
        // Creados, borrados o renombrados (a una carpeta que solo "cambió" no le pasa nada:
        // Windows lo avisa cada vez que cambia algo adentro).
        let mut structural: HashSet<PathBuf> = HashSet::new();
        let mut full = false;
        for r in rx.try_iter() {
            match r {
                Ok(ev) => {
                    full |= ev.need_rescan();
                    let moved = matches!(ev.kind, EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_)));
                    for p in ev.paths {
                        if moved {
                            structural.insert(p.clone());
                        }
                        paths.insert(p);
                    }
                }
                Err(_) => full = true,
            }
        }
        let mut folders_changed = false;
        for p in &paths {
            let Ok(rel) = p.strip_prefix(&self.root) else { continue };
            let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            // .nodex, .papelera y otros ocultos no son notas.
            if parts.first().is_none_or(|c| c.starts_with('.')) {
                // (El historial de versiones no es un dato que haya que releer.)
                self.internal_changed |= parts.first().is_some_and(|c| c == ".nodex") && parts.get(1).is_none_or(|c| c != "historial");
                continue;
            }
            match parts.len() {
                // Un espacio creado, renombrado o borrado (los archivos sueltos, como tareas.txt, no).
                1 => folders_changed |= structural.contains(p) && (p.is_dir() || self.workspaces.contains(&parts[0]) || is_diary_dir(&parts[0])),
                _ if is_attachments_dir(&parts[0]) => {}
                2 if is_md(p) => self.refresh_note(p),
                _ => {}
            }
        }
        if full || folders_changed {
            self.scan();
        }
    }

    /// Relee una nota avisada (o la quita si ya no existe). Siempre se relee: con el primer aviso
    /// pudo leerse a medio escribir, y la fecha de modificación puede no haber cambiado aún.
    fn refresh_note(&mut self, path: &Path) {
        match fs::metadata(path) {
            Ok(m) if m.is_file() => {
                let Ok(mt) = m.modified() else { return };
                if let Ok(text) = read_text(path) {
                    self.upsert(path.to_path_buf(), text, mt);
                }
            }
            _ => {
                if self.notes.remove(path).is_some() {
                    self.cache_dirty = true;
                    self.generation += 1;
                }
            }
        }
    }

    /// Relee la estructura de carpetas; solo vuelve a leer las notas cuya fecha cambió.
    /// La fecha sale del listado de la carpeta, sin abrir cada archivo.
    pub fn scan(&mut self) {
        self.last_full = Instant::now();
        self.full_scans += 1;
        let mut ws: Vec<String> = fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .collect();
        // Las notas del día se leen como las demás, pero su carpeta no es un espacio.
        let diary: Vec<String> = ws.iter().filter(|w| is_diary_dir(w)).cloned().collect();
        ws.retain(|w| !is_reserved_dir(w));
        if ws.is_empty() && fs::create_dir_all(self.root.join(DEFAULT_WORKSPACE)).is_ok() {
            ws.push(DEFAULT_WORKSPACE.to_string());
        }
        ws.sort_by_key(|w| w.to_lowercase());

        self.diary_dir = self.root.join(diary.first().map_or(DIARY, |d| d.as_str()));

        let mut seen = HashSet::new();
        for w in ws.iter().chain(&diary) {
            for e in fs::read_dir(self.root.join(w)).into_iter().flatten().flatten() {
                let path = e.path();
                if !is_md(&path) || !e.file_type().is_ok_and(|t| t.is_file()) {
                    continue;
                }
                seen.insert(path.clone());
                let Some(m) = e.metadata().and_then(|m| m.modified()).ok() else { continue };
                if self.notes.get(&path).is_some_and(|n| n.modified == m) {
                    continue;
                }
                if let Ok(text) = read_text(&path) {
                    self.reads += 1;
                    self.upsert(path, text, m);
                }
            }
        }
        let before = self.notes.len();
        self.notes.retain(|p, _| seen.contains(p));
        if self.notes.len() != before {
            self.cache_dirty = true;
            self.generation += 1;
        }
        self.workspaces = ws;
    }

    /// Actualiza el índice (tras guardar, sin esperar al próximo escaneo).
    pub fn upsert(&mut self, path: PathBuf, text: String, modified: SystemTime) {
        let workspace = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .map(|w| if is_diary_dir(&w) { DIARY.to_string() } else { w })
            .unwrap_or_default();
        let title = stem(&path);
        // Antes de reemplazarla: cómo estaba, al historial (si corresponde).
        if let Some(old) = self.notes.get(&path).filter(|n| n.text != text) {
            let rel = crate::history::rel_of(&self.root, &path);
            let now = chrono::Local::now();
            let last = self.history_last.get(&path).copied().or_else(|| crate::history::last_kept(&self.root, &rel));
            let since = last.map(|t| (now - t).to_std().unwrap_or_default());
            if crate::history::should_keep(&old.text, &text, since) {
                if crate::history::keep(&self.root, &rel, &old.text, now).is_ok() {
                    self.history_last.insert(path.clone(), now);
                }
            } else if let Some(t) = last {
                self.history_last.insert(path.clone(), t);
            }
        }
        self.cache_dirty = true;
        self.generation += 1;
        use std::cell::OnceCell;
        self.notes.insert(
            path.clone(),
            Note { path, workspace, title, modified, text, folded: OnceCell::new(), meeting: OnceCell::new(), tags: OnceCell::new(), day: OnceCell::new(), hash: OnceCell::new(), task_lines: OnceCell::new() },
        );
    }

    /// Notas de un espacio, la más reciente primero.
    pub fn notes_in(&self, ws: &str) -> Vec<&Note> {
        let mut v: Vec<&Note> = self.notes.values().filter(|n| n.workspace == ws).collect();
        v.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.title.cmp(&b.title)));
        v
    }

    pub fn get(&self, path: &Path) -> Option<&Note> {
        self.notes.get(path)
    }

    /// Las notas, sin ordenar.
    pub fn notes_iter(&self) -> impl Iterator<Item = &Note> + Clone {
        self.notes.values()
    }

    pub fn all_notes(&self) -> Vec<&Note> {
        let mut v: Vec<&Note> = self.notes.values().collect();
        v.sort_by(|a, b| b.modified.cmp(&a.modified));
        v
    }

    /// Etiquetas del espacio con la cantidad de líneas que las usan.
    pub fn tag_counts(&self, ws: &str) -> Vec<(String, usize)> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for n in self.notes.values().filter(|n| n.workspace == ws) {
            for (t, c) in n.tags() {
                *counts.entry(t.clone()).or_default() += c;
            }
        }
        counts.into_iter().collect()
    }

    /// La nota de un día ("2026-09-30"), en `Diario/` (se usa la carpeta como esté escrita).
    pub fn diary_path(&self, day: &str) -> PathBuf {
        self.diary_dir.join(format!("{}.md", sanitize(day)))
    }

    pub fn note_path(&self, ws: &str, title: &str) -> PathBuf {
        self.root.join(ws).join(format!("{}.md", sanitize(title)))
    }

    /// Ruta libre para un título ("Sin título", "Sin título 2", ...).
    pub fn unique_path(&self, ws: &str, title: &str) -> PathBuf {
        let base = sanitize(title);
        let mut path = self.note_path(ws, &base);
        let mut i = 2;
        while path.exists() {
            path = self.note_path(ws, &format!("{base} {i}"));
            i += 1;
        }
        path
    }

    pub fn create_workspace(&mut self, name: &str) -> io::Result<String> {
        let name = sanitize(name);
        if is_reserved_dir(&name) {
            return Err(io::Error::other(tf!("«{name}» es una carpeta reservada (Diario, Adjuntos, Plantillas, Bloc o Archivo)", name = name)));
        }
        fs::create_dir_all(self.root.join(&name))?;
        self.scan();
        Ok(name)
    }

    /// Mueve la nota a `.papelera` (no se borra). Devuelve dónde quedó.
    pub fn trash(&mut self, path: &Path) -> io::Result<PathBuf> {
        let dir = self.root.join(TRASH);
        fs::create_dir_all(&dir)?;
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let mut dest = dir.join(&name);
        let mut i = 2;
        while dest.exists() {
            dest = dir.join(format!("{} {i}{ext}", stem(path)));
            i += 1;
        }
        fs::rename(path, &dest)?;
        self.remember_trashed(&dest, path);
        if self.notes.remove(path).is_some() {
            self.cache_dirty = true;
            self.generation += 1;
        }
        Ok(dest)
    }

    fn trash_list(&self) -> PathBuf {
        self.root.join(".nodex").join(TRASH_LIST)
    }

    /// Anota de dónde vino lo que se mandó a la papelera (para restaurarlo a su lugar).
    fn remember_trashed(&self, dest: &Path, original: &Path) {
        let (Some(name), Ok(rel)) = (dest.file_name(), original.strip_prefix(&self.root)) else { return };
        let when = chrono::Local::now().format("%Y-%m-%d %H:%M");
        let line = format!("{}\t{}\t{when}\n", name.to_string_lossy(), rel.to_string_lossy().replace('\\', "/"));
        let file = self.trash_list();
        let _ = fs::create_dir_all(self.root.join(".nodex"));
        let _ = fs::OpenOptions::new().create(true).append(true).open(&file).and_then(|mut f| f.write_all(line.as_bytes()));
    }

    /// Quita de la lista lo que ya no está en la papelera.
    fn forget_trashed(&self, name: &str) {
        let Ok(text) = read_text(&self.trash_list()) else { return };
        let keep: Vec<&str> = text.lines().filter(|l| l.split('\t').next() != Some(name) && !l.trim().is_empty()).collect();
        let _ = fs::write(self.trash_list(), keep.iter().map(|l| format!("{l}\n")).collect::<String>());
    }

    /// Lo que hay en la papelera, lo más reciente primero.
    pub fn trashed(&self) -> Vec<Trashed> {
        let mut known: HashMap<String, (String, String)> = HashMap::new();
        for l in read_text(&self.trash_list()).unwrap_or_default().lines() {
            let mut parts = l.split('\t');
            if let (Some(name), Some(from), Some(when)) = (parts.next(), parts.next(), parts.next()) {
                known.insert(name.to_string(), (from.to_string(), when.to_string()));
            }
        }
        let mut out: Vec<Trashed> = fs::read_dir(self.root.join(TRASH))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                let (from, when) = match known.get(&name) {
                    Some((f, w)) => (Some(f.clone()), w.clone()),
                    None => {
                        let m: Option<chrono::DateTime<chrono::Local>> = e.metadata().and_then(|m| m.modified()).ok().map(Into::into);
                        (None, m.map(|m| m.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default())
                    }
                };
                Trashed { path: e.path(), name, from, when, is_dir }
            })
            .collect();
        out.sort_by(|a, b| b.when.cmp(&a.when).then_with(|| a.name.cmp(&b.name)));
        out
    }

    /// Devuelve algo de la papelera a su lugar (si ya hay algo con ese nombre, queda con otro).
    /// Si no se sabe de dónde vino: una nota del día al Diario, otra nota a `ws`, un espacio
    /// como espacio. Devuelve dónde quedó.
    pub fn restore(&mut self, t: &Trashed, ws: &str) -> io::Result<PathBuf> {
        let rel = match &t.from {
            Some(f) => f.clone(),
            None if t.is_dir || !is_md(&t.path) => t.name.clone(),
            None if crate::agenda::is_date(stem(&t.path).split(' ').next().unwrap_or_default()) => format!("{DIARY}/{}", t.name),
            None => format!("{ws}/{}", t.name),
        };
        let wanted = self.root.join(&rel);
        let ext = wanted.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let base = if t.is_dir { wanted.file_name().unwrap_or_default().to_string_lossy().into_owned() } else { stem(&wanted) };
        let mut target = wanted.clone();
        let mut i = 2;
        while target.exists() {
            target = wanted.with_file_name(format!("{base} {i}{ext}"));
            i += 1;
        }
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::rename(&t.path, &target)?;
        self.forget_trashed(&t.name);
        self.scan();
        Ok(target)
    }

    /// Borra para siempre algo de la papelera (no se puede deshacer).
    pub fn delete_forever(&mut self, t: &Trashed) -> io::Result<()> {
        if !t.path.starts_with(self.root.join(TRASH)) {
            return Err(io::Error::other(t!("solo se borra lo que está en la papelera")));
        }
        // En Windows, el antivirus o el indexador pueden tener el archivo abierto un momento.
        let mut tries = 0;
        loop {
            let r = if t.is_dir { fs::remove_dir_all(&t.path) } else { fs::remove_file(&t.path) };
            match r {
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied && tries < 10 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(100));
                }
                other => break other?,
            }
        }
        self.forget_trashed(&t.name);
        Ok(())
    }

    /// Mueve un espacio completo (con sus notas) a `.papelera`. Devuelve dónde quedó.
    pub fn trash_workspace(&mut self, ws: &str) -> io::Result<PathBuf> {
        let dir = self.root.join(TRASH);
        fs::create_dir_all(&dir)?;
        let mut dest = dir.join(ws);
        let mut i = 2;
        while dest.exists() {
            dest = dir.join(format!("{ws} {i}"));
            i += 1;
        }
        fs::rename(self.root.join(ws), &dest)?;
        self.remember_trashed(&dest, &self.root.join(ws));
        self.scan();
        Ok(dest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_keeps_notes_and_workspaces() {
        let root = std::env::temp_dir().join(format!("nodex-papelera-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("Obra")).unwrap();
        fs::create_dir_all(root.join("General")).unwrap();
        fs::write(root.join("Obra").join("A.md"), "a").unwrap();
        fs::write(root.join("General").join("B.md"), "b").unwrap();
        let mut v = Vault::new(root.clone());
        let dest = v.trash(&root.join("General").join("B.md")).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "b");
        let dest = v.trash_workspace("Obra").unwrap();
        assert!(dest.join("A.md").exists() && !v.workspaces.contains(&"Obra".to_string()));
        let _ = fs::remove_dir_all(&root);
    }

    /// Lo que cambia por fuera (otro programa, Dropbox) llega sin revisar toda la carpeta.
    #[test]
    fn notices_changes_without_full_scans() {
        let root = std::env::temp_dir().join(format!("nodex-avisos-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("General")).unwrap();
        fs::write(root.join("General").join("A.md"), "a").unwrap();
        let mut v = Vault::new(root.clone());
        assert!(v.watching(), "los avisos del sistema deberían funcionar en una carpeta local");
        let wait = |v: &mut Vault, what: &str, ok: &dyn Fn(&Vault) -> bool| {
            let start = Instant::now();
            loop {
                v.refresh();
                if ok(v) {
                    break;
                }
                assert!(start.elapsed() < Duration::from_secs(10), "no llegó el aviso: {what}");
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        let scans = v.full_scans;
        fs::write(root.join("General").join("B.md"), "nueva").unwrap();
        fs::write(root.join("General").join("A.md"), "cambiada").unwrap();
        fs::write(root.join("tareas.txt"), "2026-09-30 algo").unwrap();
        wait(&mut v, "nota nueva y cambiada", &|v| {
            v.get(&v.root.join("General").join("B.md")).is_some()
                && v.get(&v.root.join("General").join("A.md")).is_some_and(|n| n.text == "cambiada")
        });
        assert_eq!(v.full_scans, scans, "una nota cambiada no revisa toda la carpeta");
        fs::remove_file(root.join("General").join("B.md")).unwrap();
        wait(&mut v, "nota borrada", &|v| v.get(&v.root.join("General").join("B.md")).is_none());
        // Un espacio nuevo sí revisa la estructura.
        fs::create_dir_all(root.join("Obra")).unwrap();
        fs::write(root.join("Obra").join("C.md"), "c").unwrap();
        wait(&mut v, "espacio nuevo", &|v| v.workspaces.contains(&"Obra".to_string()) && v.get(&v.root.join("Obra").join("C.md")).is_some());
        let _ = fs::remove_dir_all(&root);
    }

    /// Al abrir de nuevo, las notas salen de la copia local: solo se lee del disco lo que cambió.
    #[test]
    fn startup_reads_only_what_changed() {
        let root = std::env::temp_dir().join(format!("nodex-copia-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(cache_file(&root));
        fs::create_dir_all(root.join("General")).unwrap();
        fs::create_dir_all(root.join("Obra")).unwrap();
        for i in 0..20 {
            fs::write(root.join("General").join(format!("N{i}.md")), format!("nota {i} #tag")).unwrap();
        }
        fs::write(root.join("Obra").join("Ñandú.md"), "\u{feff}con acentos: año").unwrap();
        let mut v = Vault::new(root.clone());
        assert_eq!(v.reads, 21, "la primera vez se lee todo");
        v.save_cache_now();
        while v.cache_busy.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(v);

        // Cambia una, se borra otra y aparece una nueva.
        std::thread::sleep(Duration::from_millis(20));
        fs::write(root.join("General").join("N3.md"), "cambiada").unwrap();
        fs::remove_file(root.join("General").join("N4.md")).unwrap();
        fs::write(root.join("Obra").join("Nueva.md"), "nueva").unwrap();
        let v = Vault::new(root.clone());
        assert_eq!(v.reads, 2, "solo la cambiada y la nueva");
        assert_eq!(v.get(&root.join("General").join("N3.md")).unwrap().text, "cambiada");
        assert!(v.get(&root.join("General").join("N4.md")).is_none());
        assert_eq!(v.get(&root.join("Obra").join("Ñandú.md")).unwrap().text, "con acentos: año");
        assert_eq!(v.all_notes().len(), 21);

        // Una copia dañada no rompe nada: se lee todo.
        fs::write(cache_file(&root), b"basura").unwrap();
        let v = Vault::new(root.clone());
        assert_eq!(v.reads, 21);
        let _ = fs::remove_file(cache_file(&root));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folds_for_search() {
        assert_eq!(fold("Cubicación del MURO\nÑandú Über"), "cubicacion del muro\nñandu uber");
        assert_eq!(fold("a\nb\r\nc").split('\n').count(), 3);
    }

    #[test]
    fn sanitizes_titles() {
        assert_eq!(sanitize("  Reunión 3/4: vigas?  "), "Reunión 3-4- vigas-");
        assert_eq!(sanitize("   "), "Sin título");
        assert_eq!(sanitize("nota."), "nota");
    }
}
