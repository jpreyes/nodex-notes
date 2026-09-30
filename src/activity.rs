//! Lo que hizo la IA: un historial de cada cambio (organizar una nota, anotar correos,
//! responder una pregunta, unir duplicados, crear un espacio), con sus detalles.
//! Se guarda en `.nodex/actividad.json` y se muestra en la ventana de la IA.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

const FILE: &str = "actividad.json";
/// Cuántas entradas se guardan (las más viejas se borran).
const MAX: usize = 400;

/// Qué tipo de cambio fue (para su ícono).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Organizar,
    Correo,
    Respuesta,
    Duplicado,
    Espacio,
    Error,
    /// Se juntaron dos versiones de un archivo (copia en conflicto de Dropbox).
    Sincronizar,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub id: String,
    /// "2026-09-26 14:03"
    pub at: String,
    pub kind: Kind,
    /// Nota a la que se refiere (relativa, sin .md).
    pub note: String,
    pub text: String,
    /// Cada cosa hecha: "Tarea: Enviar informe · vie 2 oct", "→ Consorcio/Trincheras"…
    pub details: Vec<String>,
    /// Se deshizo.
    pub undone: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Log {
    pub entries: Vec<Entry>,
    /// Cómo estaba el archivo la última vez que este equipo lo leyó o escribió (ver `shared`).
    #[serde(skip)]
    base: Vec<Entry>,
}

impl Log {
    /// Lo que hay en disco; `None` si no existe o no se entiende.
    fn read(root: &Path) -> Option<Log> {
        serde_json::from_str(&crate::vault::read_text(&root.join(".nodex").join(FILE)).ok()?).ok()
    }

    pub fn load(root: &Path) -> Log {
        let mut log = Self::read(root).unwrap_or_default();
        log.base = log.entries.clone();
        log
    }

    /// Trae lo que otro equipo anotó desde la última vez. Devuelve si cambió algo aquí.
    pub fn sync(&mut self, root: &Path) -> bool {
        let Some(disk) = Self::read(root) else { return false };
        let base = std::mem::take(&mut self.base);
        let changed = crate::shared::merge_list(&mut self.entries, &base, &disk.entries, |e| e.id.clone());
        if changed {
            self.order();
        }
        self.base = disk.entries;
        changed
    }

    /// Por fecha y hora, y solo las últimas `MAX`.
    fn order(&mut self) {
        self.entries.sort_by(|a, b| a.at.cmp(&b.at));
        if self.entries.len() > MAX {
            let extra = self.entries.len() - MAX;
            self.entries.drain(..extra);
        }
    }

    /// Guarda, juntando antes con lo que otro equipo haya anotado.
    pub fn save(&mut self, root: &Path) -> io::Result<()> {
        self.sync(root);
        let dir = root.join(".nodex");
        fs::create_dir_all(&dir)?;
        fs::write(dir.join(FILE), serde_json::to_string_pretty(self).unwrap_or_default())?;
        self.base = self.entries.clone();
        Ok(())
    }

    /// Suma lo de una copia en conflicto (solo se agrega lo que falta).
    pub fn absorb(&mut self, other: &Log) {
        if crate::shared::absorb_list(&mut self.entries, &other.entries, |e| e.id.clone()) {
            self.order();
        }
    }

    /// Agrega una entrada (la más nueva al final) y devuelve su id.
    pub fn add(&mut self, entry: Entry) -> String {
        let id = entry.id.clone();
        self.entries.push(entry);
        if self.entries.len() > MAX {
            let extra = self.entries.len() - MAX;
            self.entries.drain(..extra);
        }
        id
    }

    pub fn mark_undone(&mut self, id: &str) -> bool {
        match self.entries.iter_mut().find(|e| e.id == id) {
            Some(e) => {
                e.undone = true;
                true
            }
            None => false,
        }
    }

    /// Las entradas por día, de la más nueva a la más vieja: (AAAA-MM-DD, entradas).
    pub fn by_day(&self) -> Vec<(String, Vec<&Entry>)> {
        let mut out: Vec<(String, Vec<&Entry>)> = Vec::new();
        for e in self.entries.iter().rev() {
            let day = e.at.get(..10).unwrap_or("").to_string();
            match out.last_mut() {
                Some((d, v)) if *d == day => v.push(e),
                _ => out.push((day, vec![e])),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, at: &str) -> Entry {
        Entry { id: id.into(), at: at.into(), text: id.into(), ..Entry::default() }
    }

    #[test]
    fn keeps_the_newest_grouped_by_day() {
        let mut log = Log::default();
        log.add(entry("a", "2026-09-25 10:00"));
        log.add(entry("b", "2026-09-26 09:00"));
        log.add(entry("c", "2026-09-26 11:30"));
        let days = log.by_day();
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].0, "2026-09-26");
        assert_eq!(days[0].1.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["c", "b"]);
        assert!(log.mark_undone("b") && log.entries[1].undone);
        for i in 0..MAX {
            log.add(entry(&format!("x{i}"), "2026-09-27 08:00"));
        }
        assert_eq!(log.entries.len(), MAX);
        assert_eq!(log.entries[0].id, "x0");
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("nodex-actividad-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut log = Log::default();
        log.add(Entry { kind: Kind::Correo, details: vec!["uno".into()], ..entry("a", "2026-09-26 10:00") });
        log.save(&dir).unwrap();
        let back = Log::load(&dir);
        assert_eq!(back.entries, log.entries);
        let _ = fs::remove_dir_all(&dir);
    }
}
