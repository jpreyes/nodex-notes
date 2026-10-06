//! Idioma de la interfaz: español (el original) o inglés.
//!
//! Cada texto de la interfaz se escribe en español dentro de la macro `t!` (o `tf!`, con nombres
//! entre llaves y sus valores, si lleva datos). En inglés, se busca su traducción en `src/i18n/en/*.txt`; si no está, se ve en
//! español (nunca queda en blanco). Una prueba revisa que todo `t!`/`tf!` tenga su traducción.
//!
//! Formato de los diccionarios (`src/i18n/en/*.txt`), un par por bloque, separados por una línea
//! en blanco; `\n` dentro del texto es un salto de línea:
//!
//! ```text
//! Tareas
//! = Tasks
//!
//! Quitó {n} tareas repetidas
//! = Removed {n} repeated tasks
//!
//! plural:nota
//! = note|notes
//! ```

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

static ENGLISH: AtomicBool = AtomicBool::new(false);

/// Los diccionarios, uno por parte de la app.
const FILES: &[&str] = &[
    include_str!("i18n/en/app.txt"),
    include_str!("i18n/en/views.txt"),
    include_str!("i18n/en/ai.txt"),
    include_str!("i18n/en/editor.txt"),
    include_str!("i18n/en/settings.txt"),
    include_str!("i18n/en/tasks.txt"),
    include_str!("i18n/en/mail.txt"),
    include_str!("i18n/en/sync.txt"),
    include_str!("i18n/en/core.txt"),
];

pub fn set_english(on: bool) {
    ENGLISH.store(on, Ordering::Relaxed);
}

pub fn is_english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

/// ¿El sistema está en inglés? (para elegir el idioma la primera vez).
pub fn system_is_english() -> bool {
    sys_locale::get_locale().is_some_and(|l| l.to_lowercase().starts_with("en"))
}

/// Lee los diccionarios: español -> inglés.
pub fn parse(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut es: Option<String> = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with("# ") {
            es = None;
            continue;
        }
        match (line.strip_prefix("= "), &es) {
            (Some(en), Some(k)) => {
                out.push((k.replace("\\n", "\n"), en.replace("\\n", "\n")));
                es = None;
            }
            _ => es = Some(line.to_string()),
        }
    }
    out
}

fn dict() -> &'static HashMap<String, &'static str> {
    static D: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    D.get_or_init(|| {
        FILES
            .iter()
            .flat_map(|f| parse(f))
            .map(|(es, en)| (es, &*Box::leak(en.into_boxed_str())))
            .collect()
    })
}

/// La traducción de un texto (o el mismo texto, en español o si falta).
pub fn tr(es: &'static str) -> &'static str {
    if !is_english() {
        return es;
    }
    dict().get(es).copied().unwrap_or(es)
}

/// Igual que `tr`, para un texto que no es fijo (devuelve una copia).
pub fn tr_owned(es: &str) -> String {
    if !is_english() {
        return es.to_string();
    }
    dict().get(es).map(|s| s.to_string()).unwrap_or_else(|| es.to_string())
}

/// Pone los datos en un texto: «Quitó {n} tareas» con n = 3. Un `{}` sin nombre toma los datos
/// en orden.
pub fn fill(template: &str, args: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    let mut next = 0;
    while let Some(i) = rest.find('{') {
        out += &rest[..i];
        let after = &rest[i + 1..];
        if let Some(stripped) = after.strip_prefix('{') {
            out.push('{');
            rest = stripped;
            continue;
        }
        let Some(j) = after.find('}') else {
            out += &rest[i..];
            rest = "";
            break;
        };
        let name = &after[..j];
        let value = if name.is_empty() {
            let v = args.get(next).map(|a| a.1.clone());
            next += 1;
            v
        } else {
            args.iter().find(|a| a.0 == name).map(|a| a.1.clone())
        };
        match value {
            Some(v) => out += &v,
            None => out += &rest[i..i + j + 2],
        }
        rest = &after[j + 1..];
    }
    out += &rest.replace("}}", "}");
    out
}

/// «3 notas» / «3 notes»: la palabra en español, en singular.
pub fn plural(n: usize, es_word: &str) -> String {
    if is_english() {
        let key = format!("plural:{es_word}");
        if let Some(forms) = dict().get(key.as_str()) {
            let (one, many) = forms.split_once('|').unwrap_or((forms, forms));
            return if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
        }
    }
    if n == 1 { format!("1 {es_word}") } else { format!("{n} {es_word}s") }
}

const MESES_ES: [&str; 12] = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"];
const MESES_EN: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const DIAS_ES: [&str; 7] = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];
const DIAS_EN: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const DIAS_CORTOS_ES: [&str; 7] = ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"];
const DIAS_CORTOS_EN: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS_ES: [&str; 12] = ["Enero", "Febrero", "Marzo", "Abril", "Mayo", "Junio", "Julio", "Agosto", "Septiembre", "Octubre", "Noviembre", "Diciembre"];
const MONTHS_EN: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// Meses cortos («oct» / «Oct»).
pub fn meses() -> &'static [&'static str; 12] {
    if is_english() { &MESES_EN } else { &MESES_ES }
}

/// Meses cortos siempre en español, para lo que se escribe en las notas y se vuelve a buscar
/// (por ejemplo «Correo de Ana (26 sep)»).
pub fn meses_es() -> &'static [&'static str; 12] {
    &MESES_ES
}

/// Días de la semana, de lunes a domingo.
pub fn dias() -> &'static [&'static str; 7] {
    if is_english() { &DIAS_EN } else { &DIAS_ES }
}

pub fn dias_cortos() -> &'static [&'static str; 7] {
    if is_english() { &DIAS_CORTOS_EN } else { &DIAS_CORTOS_ES }
}

/// Meses completos («Octubre» / «October»).
pub fn months() -> &'static [&'static str; 12] {
    if is_english() { &MONTHS_EN } else { &MONTHS_ES }
}

/// Para las indicaciones a la IA: en qué idioma escribir lo que genera.
pub fn ai_language_rule() -> &'static str {
    if is_english() {
        "\n\nIMPORTANTE: la persona usa la app en inglés. Escribe en inglés todo lo que generes para mostrar (respuestas, títulos, resúmenes, tareas, preguntas, etiquetas), salvo que su nota esté en otro idioma: en ese caso, usa el idioma de la nota."
    } else {
        ""
    }
}

/// Un texto de la interfaz, traducido.
#[macro_export]
macro_rules! t {
    ($s:literal) => {
        $crate::i18n::tr($s)
    };
}

/// Un texto con datos: el texto en español con `{n}` y después `n = 3`.
#[macro_export]
macro_rules! tf {
    ($s:literal $(, $k:ident = $v:expr)* $(,)?) => {
        $crate::i18n::fill($crate::i18n::tr($s), &[$((stringify!($k), ($v).to_string())),*])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_and_reads() {
        assert_eq!(fill("Quitó {n} de {total}", &[("n", "2".into()), ("total", "5".into())]), "Quitó 2 de 5");
        assert_eq!(fill("{} y {}", &[("a", "x".into()), ("b", "y".into())]), "x y y");
        assert_eq!(fill("llaves {{así}}", &[]), "llaves {así}");
        let d = parse("Tareas\n= Tasks\n\n# comentario\nUna\\nlínea\n= One\\nline\n");
        assert_eq!(d, vec![("Tareas".into(), "Tasks".into()), ("Una\nlínea".into(), "One\nline".into())]);
    }

    /// Cada texto con `t!` o `tf!` del código tiene su traducción, con los mismos datos `{…}`.
    #[test]
    fn every_text_is_translated() {
        let dict: HashMap<String, String> = FILES.iter().flat_map(|f| parse(f)).collect();
        let mut missing = Vec::new();
        let mut wrong = Vec::new();
        let names = |s: &str| {
            let mut v: Vec<String> = Vec::new();
            let mut rest = s;
            while let Some(i) = rest.find('{') {
                let after = &rest[i + 1..];
                if let Some(a) = after.strip_prefix('{') {
                    rest = a;
                    continue;
                }
                let Some(j) = after.find('}') else { break };
                v.push(after[..j].to_string());
                rest = &after[j + 1..];
            }
            v.sort();
            v
        };
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")), &mut files);
        for f in files {
            let src = std::fs::read_to_string(&f).unwrap_or_default();
            for mac in ["t!(\"", "tf!(\""] {
                let mut rest = src.as_str();
                while let Some(i) = rest.find(mac) {
                    // Que no sea parte de otra palabra (p. ej. «assert!(»).
                    let before = rest[..i].chars().last();
                    let start = i + mac.len();
                    rest = &rest[start..];
                    if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    // El texto hasta la comilla de cierre (respetando \").
                    let mut lit = String::new();
                    let mut chars = rest.chars();
                    let mut esc = false;
                    for ch in chars.by_ref() {
                        if esc {
                            lit.push(match ch {
                                'n' => '\n',
                                't' => '\t',
                                other => other,
                            });
                            esc = false;
                        } else if ch == '\\' {
                            esc = true;
                        } else if ch == '"' {
                            break;
                        } else {
                            lit.push(ch);
                        }
                    }
                    match dict.get(&lit) {
                        None => missing.push(format!("{}: {lit}", f.file_name().unwrap_or_default().to_string_lossy())),
                        Some(en) if names(en) != names(&lit) => wrong.push(format!("{lit} -> {en}")),
                        _ => {}
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "Faltan {} traducciones:\n{}", missing.len(), missing.join("\n"));
        assert!(wrong.is_empty(), "Traducciones con otros datos {{…}}:\n{}", wrong.join("\n"));
    }
}
