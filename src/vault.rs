//! Carpeta de notas: cada subcarpeta es un espacio de trabajo y cada `.md` una nota.
//!
//! ```text
//! Dropbox/Notas/
//!   Proyecto Edificio A/
//!     Cubicaciones losa.md
//!     2026-09-24.md
//!   Docencia/
//!   .papelera/        (notas eliminadas; no se muestra)
//! ```
//!
//! Los cambios en disco (Dropbox, otro programa) llegan por los avisos del sistema operativo
//! (`notify`): cada segundo solo se relee lo que cambió. Una revisión completa de toda la carpeta
//! queda como red de seguridad cada 10 minutos, o cada 30 segundos si no hay avisos.

use crate::tags;
use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime};

pub const DEFAULT_WORKSPACE: &str = "General";
const TRASH: &str = ".papelera";
/// Revisión completa de seguridad, por si se perdió algún aviso.
const FULL_SCAN_EVERY: Duration = Duration::from_secs(600);
/// Sin avisos del sistema (p. ej. en una carpeta de red): revisión completa cada tanto.
const FALLBACK_SCAN_EVERY: Duration = Duration::from_secs(30);

pub struct Note {
    pub path: PathBuf,
    pub workspace: String,
    pub title: String,
    pub modified: SystemTime,
    pub text: String,
}

pub struct Vault {
    pub root: PathBuf,
    pub workspaces: Vec<String>,
    notes: HashMap<PathBuf, Note>,
    /// Avisos del sistema operativo cuando algo cambia en la carpeta.
    watcher: Option<RecommendedWatcher>,
    events: Option<Receiver<notify::Result<notify::Event>>>,
    last_full: Instant,
    /// Cuántas revisiones completas se hicieron (para las pruebas).
    pub full_scans: usize,
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
            root,
            workspaces: Vec::new(),
            notes: HashMap::new(),
            watcher: None,
            events: None,
            last_full: Instant::now(),
            full_scans: 0,
        };
        v.scan();
        v.watch();
        v
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
                continue;
            }
            match parts.len() {
                // Un espacio creado, renombrado o borrado (los archivos sueltos, como tareas.txt, no).
                1 => folders_changed |= structural.contains(p) && (p.is_dir() || self.workspaces.contains(&parts[0])),
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
                self.notes.remove(path);
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
        if ws.is_empty() && fs::create_dir_all(self.root.join(DEFAULT_WORKSPACE)).is_ok() {
            ws.push(DEFAULT_WORKSPACE.to_string());
        }
        ws.sort_by_key(|w| w.to_lowercase());

        let mut seen = HashSet::new();
        for w in &ws {
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
                    self.upsert(path, text, m);
                }
            }
        }
        self.notes.retain(|p, _| seen.contains(p));
        self.workspaces = ws;
    }

    /// Actualiza el índice (tras guardar, sin esperar al próximo escaneo).
    pub fn upsert(&mut self, path: PathBuf, text: String, modified: SystemTime) {
        let workspace = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let title = stem(&path);
        self.notes.insert(path.clone(), Note { path, workspace, title, modified, text });
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

    pub fn all_notes(&self) -> Vec<&Note> {
        let mut v: Vec<&Note> = self.notes.values().collect();
        v.sort_by(|a, b| b.modified.cmp(&a.modified));
        v
    }

    /// Etiquetas del espacio con la cantidad de líneas que las usan.
    pub fn tag_counts(&self, ws: &str) -> Vec<(String, usize)> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for n in self.notes.values().filter(|n| n.workspace == ws) {
            for line in n.text.lines() {
                for t in tags::line_tags(line) {
                    *counts.entry(t).or_default() += 1;
                }
            }
        }
        counts.into_iter().collect()
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
        fs::create_dir_all(self.root.join(&name))?;
        self.scan();
        Ok(name)
    }

    /// Mueve la nota a `.papelera` (no se borra). Devuelve dónde quedó.
    pub fn trash(&mut self, path: &Path) -> io::Result<PathBuf> {
        let dir = self.root.join(TRASH);
        fs::create_dir_all(&dir)?;
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let mut dest = dir.join(&name);
        let mut i = 2;
        while dest.exists() {
            dest = dir.join(format!("{} {i}.md", stem(path)));
            i += 1;
        }
        fs::rename(path, &dest)?;
        self.notes.remove(path);
        Ok(dest)
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

    #[test]
    fn sanitizes_titles() {
        assert_eq!(sanitize("  Reunión 3/4: vigas?  "), "Reunión 3-4- vigas-");
        assert_eq!(sanitize("   "), "Sin título");
        assert_eq!(sanitize("nota."), "nota");
    }
}
