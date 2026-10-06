//! Preguntas de la IA cuando duda, y lo que aprende de las respuestas.
//!
//! - `.nodex/dudas.json`: preguntas pendientes (y qué notas ya se respondieron, para no repetirlas).
//! - `aprendido.txt`: lo que la persona aclaró, una frase por línea. La IA lo lee en cada análisis.
//!
//! Una pregunta apunta a una nota dentro del archivo por el texto de su primera línea
//! (sin etiquetas, fecha ni identificador), así sigue sirviendo aunque cambien los números de línea.

use crate::lines;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

pub const LEARNED_FILE: &str = "aprendido.txt";
const STORE_FILE: &str = "dudas.json";

/// Una respuesta posible y lo que hace al elegirla.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choice {
    pub label: String,
    /// Mover la nota a este espacio (y, en una nota de captura, a esta nota).
    pub espacio: String,
    pub nota: String,
    /// Unirla, como detalle, a la nota que empieza con este texto.
    pub de: String,
    /// Fecha de su tarea (AAAA-MM-DD).
    pub fecha: String,
    pub etiquetas: Vec<String>,
    /// Lo que conviene recordar para las próximas notas.
    pub dato: String,
    /// Duplicado: la otra nota de adentro (su nota y el texto de su primera línea).
    pub unir_nota: String,
    pub unir: String,
    /// Unir al revés: se queda esta y se quita la otra.
    pub al_reves: bool,
    /// "Son distintas": no volver a ofrecer unirlas.
    pub distintas: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Doubt {
    pub id: String,
    /// Nota (relativa a la carpeta, sin ".md").
    pub note: String,
    /// Texto de la primera línea de la nota de adentro a la que se refiere.
    pub unit: String,
    pub question: String,
    pub choices: Vec<Choice>,
    pub created: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub pending: Vec<Doubt>,
    /// Notas de adentro ya respondidas o ignoradas (hash de su texto): no se vuelve a preguntar.
    pub resolved: Vec<u64>,
    /// Pares que la persona dijo que no son duplicados (ver `dups::pair`).
    pub not_dups: Vec<u64>,
    /// Cómo estaba el archivo la última vez que este equipo lo leyó o escribió (ver `shared`).
    #[serde(skip)]
    pub(crate) base: Option<Box<Store>>,
}

impl Store {
    /// Lo que hay en disco; `None` si el archivo no existe o no se entiende (p. ej. a medio
    /// escribir): en ese caso no se saca ninguna conclusión.
    fn read(root: &Path) -> Option<Store> {
        serde_json::from_str(&crate::vault::read_text(&root.join(".nodex").join(STORE_FILE)).ok()?).ok()
    }

    fn remember(&mut self) {
        self.base = Some(Box::new(Store { pending: self.pending.clone(), resolved: self.resolved.clone(), not_dups: self.not_dups.clone(), base: None }));
    }

    pub fn load(root: &Path) -> Store {
        let mut s = Self::read(root).unwrap_or_default();
        s.remember();
        s
    }

    /// Trae lo que otro equipo cambió en disco desde la última vez (sin escribir nada).
    /// Devuelve si cambió algo aquí.
    pub fn sync(&mut self, root: &Path) -> bool {
        let Some(disk) = Self::read(root) else { return false };
        let base = self.base.take().map(|b| *b).unwrap_or_default();
        let mut changed = crate::shared::merge_list(&mut self.pending, &base.pending, &disk.pending, |d| d.id.clone());
        changed |= crate::shared::merge_list(&mut self.resolved, &base.resolved, &disk.resolved, |h| *h);
        changed |= crate::shared::merge_list(&mut self.not_dups, &base.not_dups, &disk.not_dups, |h| *h);
        changed |= self.drop_resolved();
        self.base = Some(Box::new(disk));
        changed
    }

    /// Una pregunta que ya se respondió (aquí o en otro equipo) no queda pendiente.
    fn drop_resolved(&mut self) -> bool {
        let before = self.pending.len();
        let resolved = &self.resolved;
        self.pending.retain(|d| !resolved.contains(&crate::ai::fnv(&d.unit)));
        self.pending.len() != before
    }

    /// Guarda, juntando antes con lo que otro equipo haya escrito.
    pub fn save(&mut self, root: &Path) -> io::Result<()> {
        self.sync(root);
        let dir = root.join(".nodex");
        fs::create_dir_all(&dir)?;
        fs::write(dir.join(STORE_FILE), serde_json::to_string_pretty(self).unwrap_or_default())?;
        self.remember();
        Ok(())
    }

    /// Suma lo de una copia en conflicto (sin base: solo se agrega lo que falta).
    pub fn absorb(&mut self, other: &Store) {
        crate::shared::absorb_list(&mut self.pending, &other.pending, |d| d.id.clone());
        crate::shared::absorb_list(&mut self.resolved, &other.resolved, |h| *h);
        crate::shared::absorb_list(&mut self.not_dups, &other.not_dups, |h| *h);
        self.drop_resolved();
    }

    pub fn is_resolved(&self, unit: &str) -> bool {
        self.resolved.contains(&crate::ai::fnv(unit))
    }

    /// Deja la pregunta como respondida y la saca de las pendientes.
    pub fn resolve(&mut self, id: &str) -> Option<Doubt> {
        let i = self.pending.iter().position(|d| d.id == id)?;
        let d = self.pending.remove(i);
        let h = crate::ai::fnv(&d.unit);
        if !self.resolved.contains(&h) {
            self.resolved.push(h);
        }
        Some(d)
    }

    /// Las pendientes de una nota.
    pub fn for_note<'a>(&'a self, note: &'a str) -> impl Iterator<Item = &'a Doubt> + 'a {
        self.pending.iter().filter(move |d| d.note == note)
    }
}

/// El texto que identifica una línea: sin sangría, casilla, etiquetas, fecha ni identificador.
pub fn core(line: &str) -> String {
    let info = lines::parse(line);
    let rest = line.get(info.prefix..).unwrap_or(line);
    rest.split_whitespace()
        .filter(|w| !(w.starts_with('#') && crate::tags::tag_spans(w).first().is_some_and(|s| s.0 == 0)))
        .filter(|w| !w.starts_with("due:") && !(w.starts_with('^') && w.len() > 3))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Todas las notas de adentro cuya primera línea tiene ese texto (en orden).
pub fn find_units(text: &str, unit: &str) -> Vec<lines::Unit> {
    let all: Vec<&str> = text.lines().collect();
    lines::units(text).into_iter().filter(|u| all.get(u.first).is_some_and(|l| core(l) == unit)).collect()
}

/// La nota de adentro cuya primera línea tiene ese texto.
pub fn find_unit(text: &str, unit: &str) -> Option<lines::Unit> {
    let all: Vec<&str> = text.lines().collect();
    lines::units(text).into_iter().find(|u| all.get(u.first).is_some_and(|l| core(l) == unit))
}

/// Lo aprendido, para el prompt (las últimas 80 frases).
pub fn learned(root: &Path) -> Vec<String> {
    let text = crate::vault::read_text(&root.join(LEARNED_FILE)).unwrap_or_default();
    let facts: Vec<String> = text
        .lines()
        .map(|l| l.trim().trim_start_matches("- ").trim().to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    facts[facts.len().saturating_sub(80)..].to_vec()
}

pub fn learn(root: &Path, fact: &str) -> io::Result<()> {
    let fact = fact.trim();
    if fact.is_empty() || learned(root).iter().any(|f| f.eq_ignore_ascii_case(fact)) {
        return Ok(());
    }
    let path = root.join(LEARNED_FILE);
    let mut text = crate::vault::read_text(&path).unwrap_or_default();
    if text.is_empty() {
        text = "# Lo que la IA aprendió de tus respuestas (puedes editarlo o borrar líneas)\n".into();
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text += &format!("- {fact}\n");
    fs::write(path, text)
}

/// Convierte las dudas de la IA ("unidad": "L4", "de": "L1") en preguntas guardables.
pub fn from_ai(text: &str, note: &str, a: &crate::ai::Analysis, today: &str, mut new_id: impl FnMut() -> String) -> Vec<Doubt> {
    let lines_v: Vec<&str> = text.lines().collect();
    let units = lines::units(text);
    let head = |id: &str| -> Option<String> {
        let u = units.iter().find(|u| u.id.eq_ignore_ascii_case(id.trim()))?;
        Some(core(lines_v.get(u.first)?))
    };
    let mut out = Vec::new();
    for d in a.dudas.iter().take(3) {
        let Some(unit) = head(&d.unidad) else { continue };
        if d.pregunta.trim().is_empty() || unit.is_empty() {
            continue;
        }
        let choices: Vec<Choice> = d
            .opciones
            .iter()
            .filter_map(|o| {
                let c = match o {
                    serde_json::Value::String(s) => Choice { label: s.clone(), ..Choice::default() },
                    serde_json::Value::Object(m) => {
                        let s = |k: &str| m.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                        Choice {
                            label: s("texto"),
                            espacio: s("espacio"),
                            nota: s("nota"),
                            de: head(&s("de")).unwrap_or_default(),
                            fecha: Some(s("fecha")).filter(|f| crate::agenda::is_date(f)).unwrap_or_default(),
                            etiquetas: m
                                .get("etiquetas")
                                .and_then(|v| v.as_array())
                                .map(|a| a.iter().filter_map(|t| t.as_str()).map(crate::organize::clean_tag).filter(|t| !t.is_empty()).collect())
                                .unwrap_or_default(),
                            dato: s("dato"),
                            ..Choice::default()
                        }
                    }
                    _ => return None,
                };
                (!c.label.is_empty()).then_some(c)
            })
            .take(4)
            .collect();
        if choices.is_empty() {
            continue;
        }
        out.push(Doubt { id: new_id(), note: note.to_string(), unit, question: d.pregunta.trim().to_string(), choices, created: today.to_string() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_text_ignores_marks() {
        assert_eq!(core("- [ ] Debo entregar el LaVet #lavet due:2026-10-02 ^k3f9a"), "Debo entregar el LaVet");
        assert_eq!(core("  - ítem con C# y #tag"), "ítem con C# y");
        let text = "Uno\n- [ ] Debo entregar el LaVet #lavet\n  detalle";
        let u = find_unit(text, "Debo entregar el LaVet").unwrap();
        assert_eq!((u.first, u.last), (1, 2));
    }

    #[test]
    fn converts_ai_doubts_and_learns() {
        let a: crate::ai::Analysis = serde_json::from_str(
            r#"{"dudas": [
                {"unidad": "L2", "pregunta": "¿De qué proyecto es el LaVet?",
                 "opciones": [{"texto": "Docencia", "espacio": "Docencia", "nota": "LaVet", "dato": "LaVet es de Docencia"},
                              {"texto": "Es detalle de la línea 1", "de": "L1"}, "No sé"]},
                {"unidad": "L9", "pregunta": "¿?", "opciones": ["x"]}]}"#,
        )
        .unwrap();
        let text = "Informe trincheras\nDebo entregar el LaVet #lavet\n";
        let d = from_ai(text, "General/2026-09-25", &a, "2026-09-25", || "d1".into());
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].unit, "Debo entregar el LaVet");
        assert_eq!(d[0].choices[0].espacio, "Docencia");
        assert_eq!(d[0].choices[1].de, "Informe trincheras");
        assert_eq!(d[0].choices[2].label, "No sé");

        let dir = std::env::temp_dir().join(format!("nodex-dudas-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        learn(&dir, "LaVet es de Docencia").unwrap();
        learn(&dir, "lavet es de docencia").unwrap();
        assert_eq!(learned(&dir), vec!["LaVet es de Docencia"]);
        let mut s = Store { pending: d, ..Store::default() };
        s.save(&dir).unwrap();
        let mut s = Store::load(&dir);
        assert_eq!(s.resolve("d1").unwrap().unit, "Debo entregar el LaVet");
        assert!(s.is_resolved("Debo entregar el LaVet") && s.pending.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
