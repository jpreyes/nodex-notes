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

use crate::tags;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const DEFAULT_WORKSPACE: &str = "General";
const TRASH: &str = ".papelera";

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
        let mut v = Vault { root, workspaces: Vec::new(), notes: HashMap::new() };
        v.scan();
        v
    }

    /// Relee la estructura de carpetas; solo vuelve a leer las notas cuya fecha cambió.
    pub fn scan(&mut self) {
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
                let is_md = path.extension().is_some_and(|x| x.eq_ignore_ascii_case("md"));
                if !is_md || !e.file_type().is_ok_and(|t| t.is_file()) {
                    continue;
                }
                seen.insert(path.clone());
                let Some(m) = modified(&path) else { continue };
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

    /// Mueve la nota a `.papelera` (no se borra).
    pub fn trash(&mut self, path: &Path) -> io::Result<()> {
        let dir = self.root.join(TRASH);
        fs::create_dir_all(&dir)?;
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let mut dest = dir.join(&name);
        let mut i = 2;
        while dest.exists() {
            dest = dir.join(format!("{} {i}.md", stem(path)));
            i += 1;
        }
        fs::rename(path, dest)?;
        self.notes.remove(path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_titles() {
        assert_eq!(sanitize("  Reunión 3/4: vigas?  "), "Reunión 3-4- vigas-");
        assert_eq!(sanitize("   "), "Sin título");
        assert_eq!(sanitize("nota."), "nota");
    }
}
