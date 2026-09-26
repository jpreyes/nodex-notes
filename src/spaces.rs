//! Sugerencias de espacios nuevos.
//!
//! En cada análisis la IA marca las notas que tratan de un proyecto o tema concreto que no
//! tiene espacio ("espacio_nuevo"). Aquí se van juntando; cuando un mismo nombre reúne
//! 3 notas, se sugiere crear el espacio y mover esas notas ahí. Se guarda en `.nodex/espacios.json`.

use crate::ai::Analysis;
use crate::{doubts, lines, vault};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

const FILE: &str = "espacios.json";
/// Notas necesarias para sugerir (y cuántas más, después de "Ahora no").
pub const MIN_NOTES: usize = 3;

/// Una nota que pertenece al tema: una nota de adentro (`unit` = texto de su primera línea)
/// o una nota completa (`unit` vacío).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ref {
    pub note: String,
    pub unit: String,
    /// Nota donde guardarla dentro del espacio nuevo (la que propuso la IA), si hay.
    pub title: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Idea {
    pub name: String,
    pub refs: Vec<Ref>,
    /// Cantidad de notas cuando se respondió "Ahora no" (0 = nunca se preguntó).
    pub snoozed_at: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Ideas {
    pub ideas: Vec<Idea>,
    /// Nombres descartados ("No"): no se vuelven a sugerir.
    pub rejected: Vec<String>,
}

fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

impl Ideas {
    pub fn load(root: &Path) -> Ideas {
        vault::read_text(&root.join(".nodex").join(FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, root: &Path) -> io::Result<()> {
        let dir = root.join(".nodex");
        fs::create_dir_all(&dir)?;
        fs::write(dir.join(FILE), serde_json::to_string_pretty(self).unwrap_or_default())
    }

    /// Suma una nota a un tema (salvo que ya exista ese espacio o se haya descartado).
    pub fn add(&mut self, name: &str, r: Ref, workspaces: &[String]) {
        let name = vault::sanitize(name.trim().trim_matches(['"', '«', '»']));
        if name == "Sin título" || workspaces.iter().any(|w| key(w) == key(&name)) || self.rejected.iter().any(|x| key(x) == key(&name)) {
            return;
        }
        match self.ideas.iter_mut().find(|i| key(&i.name) == key(&name)) {
            Some(i) => {
                if !i.refs.iter().any(|x| x.note == r.note && x.unit == r.unit) {
                    i.refs.push(r);
                }
            }
            None => self.ideas.push(Idea { name, refs: vec![r], snoozed_at: 0 }),
        }
    }

    /// Las ideas que ya reúnen suficientes notas para sugerirlas.
    pub fn ready(&self) -> impl Iterator<Item = &Idea> {
        self.ideas.iter().filter(|i| i.refs.len() >= MIN_NOTES && i.refs.len() >= i.snoozed_at + MIN_NOTES)
    }

    pub fn snooze(&mut self, name: &str) {
        if let Some(i) = self.ideas.iter_mut().find(|i| i.name == name) {
            i.snoozed_at = i.refs.len();
        }
    }

    pub fn reject(&mut self, name: &str) {
        self.ideas.retain(|i| i.name != name);
        if !self.rejected.iter().any(|x| key(x) == key(name)) {
            self.rejected.push(name.to_string());
        }
    }

    pub fn take(&mut self, name: &str) -> Option<Idea> {
        let i = self.ideas.iter().position(|i| i.name == name)?;
        Some(self.ideas.remove(i))
    }

    /// Quita las notas que ya no existen (`exists(ref)`), y los temas que quedan vacíos.
    pub fn prune(&mut self, mut exists: impl FnMut(&Ref) -> bool) -> bool {
        let before: usize = self.ideas.iter().map(|i| i.refs.len()).sum();
        for i in &mut self.ideas {
            i.refs.retain(&mut exists);
        }
        self.ideas.retain(|i| !i.refs.is_empty());
        before != self.ideas.iter().map(|i| i.refs.len()).sum::<usize>()
    }
}

/// Las notas que la IA marcó con un espacio nuevo. En una nota de captura, cada nota de
/// adentro que se quedó donde estaba; en una con título, la nota completa.
pub fn refs_from_ai(text: &str, note: &str, a: &Analysis, capture: bool, moved: &dyn Fn(&str) -> bool) -> Vec<(String, Ref)> {
    let mut out = Vec::new();
    if !capture {
        if !a.espacio_nuevo.trim().is_empty() {
            out.push((a.espacio_nuevo.trim().to_string(), Ref { note: note.to_string(), unit: String::new(), title: String::new() }));
        }
        return out;
    }
    let ls: Vec<&str> = text.lines().collect();
    let units = lines::units(text);
    for au in &a.unidades {
        if au.espacio_nuevo.trim().is_empty() || moved(&au.id.trim().to_uppercase()) {
            continue;
        }
        let Some(u) = units.iter().find(|u| u.id.eq_ignore_ascii_case(au.id.trim())) else { continue };
        let Some(head) = ls.get(u.first) else { continue };
        let core = doubts::core(head);
        if !core.is_empty() {
            out.push((au.espacio_nuevo.trim().to_string(), Ref { note: note.to_string(), unit: core, title: au.nota.trim().to_string() }));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ideas_gather_and_become_ready() {
        let a: Analysis = serde_json::from_str(
            r#"{"unidades": [{"id": "L1", "espacio_nuevo": "LaVet", "nota": "Entregas"}, {"id": "L2", "espacio_nuevo": "LaVet"}, {"id": "L3", "espacio_nuevo": "LaVet"}, {"id": "L4", "espacio": "Docencia", "nota": "X"}]}"#,
        )
        .unwrap();
        let text = "- [ ] Entregar el LaVet #lavet ^abc12\nRevisar planos LaVet\nLlamar al LaVet\nClase lunes\n";
        let refs = refs_from_ai(text, "General/2026-09-25", &a, true, &|id| id == "L3");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].1, Ref { note: "General/2026-09-25".into(), unit: "Entregar el LaVet".into(), title: "Entregas".into() });

        let mut ideas = Ideas::default();
        let ws = vec!["General".to_string(), "Docencia".to_string()];
        for (n, r) in refs {
            ideas.add(&n, r, &ws);
        }
        assert_eq!(ideas.ready().count(), 0);
        ideas.add("lavet", Ref { note: "General/Notas".into(), ..Ref::default() }, &ws);
        ideas.add("Docencia", Ref { note: "x".into(), ..Ref::default() }, &ws); // ya existe: no
        assert_eq!(ideas.ideas.len(), 1);
        assert_eq!(ideas.ready().next().unwrap().refs.len(), 3);
        ideas.snooze("LaVet");
        assert_eq!(ideas.ready().count(), 0);
        ideas.reject("LaVet");
        ideas.add("LAVET", Ref { note: "y".into(), ..Ref::default() }, &ws);
        assert!(ideas.ideas.is_empty());
    }
}
