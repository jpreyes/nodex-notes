//! Tareas (tareas.txt, formato todo.txt) y eventos (agenda.txt), más su exportación a agenda.ics.
//!
//! ```text
//! tareas.txt:  2026-09-24 Enviar planos +Proyecto_Edificio_A due:2026-09-26 nota:Proyecto%20Edificio%20A/Reunión
//!              x 2026-09-25 2026-09-24 Pedir cotización +Obra_Talca nota:...
//! agenda.txt:  2026-09-26 10:00 Visita del inspector +Proyecto_Edificio_A nota:...
//! ```

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const TASKS_FILE: &str = "tareas.txt";
pub const AGENDA_FILE: &str = "agenda.txt";
pub const ICS_FILE: &str = "agenda.ics";

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub done: bool,
    pub text: String,
    /// Espacio de trabajo (en el archivo, "+Nombre_con_guiones_bajos").
    pub project: String,
    pub due: Option<String>,
    /// Nota de origen, relativa a la carpeta de notas y sin ".md".
    pub note: Option<String>,
    /// Une la tarea con su línea en la nota ("^k3f9a" allá, "id:k3f9a" aquí).
    pub id: Option<String>,
    /// Día en que se marcó como hecha ("x 2026-09-25 …").
    pub done_on: Option<String>,
    /// Día en que se creó (la fecha al comienzo de la línea).
    pub created: Option<String>,
    /// Correo de donde salió ("correo:cuenta:carpeta:uid").
    pub mail: Option<String>,
    /// De dónde llegó, si no salió de una nota ni de un correo ("de:todo" = de Microsoft To Do).
    pub from: Option<String>,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub date: String,
    pub time: Option<String>,
    pub title: String,
    pub project: String,
    pub note: Option<String>,
    pub mail: Option<String>,
}

pub fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

pub fn is_time(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 5 && b[2] == b':' && [0, 1, 3, 4].iter().all(|&i| b[i].is_ascii_digit())
}

/// Minúsculas y sin tildes, para reconocer palabras escritas de cualquier forma.
fn plain_word(w: &str) -> String {
    w.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' => 'a',
            'é' => 'e',
            'í' => 'i',
            'ó' => 'o',
            'ú' | 'ü' => 'u',
            'ñ' => 'n',
            c => c,
        })
        .filter(|c| !matches!(c, ',' | '.' | ';'))
        .collect()
}

const WEEKDAYS: [&str; 7] = ["lunes", "martes", "miercoles", "jueves", "viernes", "sabado", "domingo"];
const MONTHS: [&str; 12] = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"];

/// Un día escrito como se dice: "hoy", "mañana", "pasado mañana", "viernes", "30 sep",
/// "30 de septiembre", "30/9", "30/09/2026", "2026-09-30".
fn parse_day(words: &[String], today: chrono::NaiveDate) -> Option<chrono::NaiveDate> {
    use chrono::{Datelike, Duration, NaiveDate};
    let joined = words.join(" ");
    match joined.as_str() {
        "hoy" => return Some(today),
        "manana" => return Some(today + Duration::days(1)),
        "pasado manana" => return Some(today + Duration::days(2)),
        _ => {}
    }
    if words.len() == 1 {
        let w = &words[0];
        if let Some(i) = WEEKDAYS.iter().position(|d| d == w) {
            let now = today.weekday().num_days_from_monday() as i64;
            let ahead = (i as i64 - now + 7) % 7;
            return Some(today + Duration::days(if ahead == 0 { 7 } else { ahead }));
        }
        if is_date(w) {
            return NaiveDate::parse_from_str(w, "%Y-%m-%d").ok();
        }
        // 30/9, 30-09, 30/09/2026
        let parts: Vec<&str> = w.split(['/', '-']).collect();
        if (2..=3).contains(&parts.len()) && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
            let d: u32 = parts[0].parse().ok()?;
            let m: u32 = parts[1].parse().ok()?;
            let y: i32 = match parts.get(2) {
                Some(y) if y.len() == 2 => 2000 + y.parse::<i32>().ok()?,
                Some(y) => y.parse().ok()?,
                None => today.year(),
            };
            let date = NaiveDate::from_ymd_opt(y, m, d)?;
            return Some(if parts.len() == 2 && date < today { NaiveDate::from_ymd_opt(y + 1, m, d)? } else { date });
        }
        return None;
    }
    // "30 sep", "30 de septiembre"
    let (day, month) = match words {
        [d, m] => (d, m),
        [d, de, m] if de == "de" => (d, m),
        _ => return None,
    };
    let d: u32 = day.parse().ok()?;
    let m = MONTHS.iter().position(|x| month.starts_with(x) && month.len() >= 3)? as u32 + 1;
    let date = NaiveDate::from_ymd_opt(today.year(), m, d)?;
    Some(if date < today { NaiveDate::from_ymd_opt(today.year() + 1, m, d)? } else { date })
}

/// Separa una fecha escrita al final de una tarea: "Llamar a Pedro el viernes" ->
/// ("Llamar a Pedro", Some("2026-10-02")). También entiende "due:2026-10-02".
pub fn parse_when(text: &str, today: chrono::NaiveDate) -> (String, Option<String>) {
    let text = text.trim();
    if let Some((t, d)) = text.rsplit_once("due:") {
        if is_date(d.trim()) {
            return (t.trim().to_string(), Some(d.trim().to_string()));
        }
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    for n in (1..=3).rev() {
        if words.len() <= n {
            continue;
        }
        let tail: Vec<String> = words[words.len() - n..].iter().map(|w| plain_word(w)).collect();
        let Some(date) = parse_day(&tail, today) else { continue };
        // Sin las palabras que unen: "para el", "antes del", "este", "próximo"…
        let mut rest = &words[..words.len() - n];
        while let Some(last) = rest.last() {
            if ["para", "el", "antes", "del", "hasta", "este", "proximo", "de", "a", "mas", "tardar"].contains(&plain_word(last).as_str()) && rest.len() > 1 {
                rest = &rest[..rest.len() - 1];
            } else {
                break;
            }
        }
        return (rest.join(" "), Some(date.format("%Y-%m-%d").to_string()));
    }
    (text.to_string(), None)
}

/// Cómo se muestra una tarea: "enviar planos @Juan_Pérez" -> "Juan Pérez: enviar planos".
pub fn display_text(text: &str) -> String {
    let mut who = Vec::new();
    let mut rest = Vec::new();
    for w in text.split_whitespace() {
        match w.strip_prefix('@') {
            Some(name) if !name.is_empty() => who.push(name.replace('_', " ")),
            _ => rest.push(w),
        }
    }
    let body = rest.join(" ");
    let body = if who.is_empty() {
        let mut c = body.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => body,
        }
    } else {
        body
    };
    if who.is_empty() { body } else { format!("{}: {body}", who.join(", ")) }
}

pub fn project_token(ws: &str) -> String {
    format!("+{}", ws.replace(' ', "_"))
}

/// "Proyecto Edificio A/Reunión" -> "Proyecto%20Edificio%20A/Reunión" (sin espacios, como pide todo.txt).
pub fn encode_note(rel: &str) -> String {
    rel.replace('%', "%25").replace(' ', "%20")
}

pub fn decode_note(s: &str) -> String {
    s.replace("%20", " ").replace("%25", "%")
}

struct Meta {
    text: String,
    project: String,
    due: Option<String>,
    note: Option<String>,
    id: Option<String>,
    mail: Option<String>,
    from: Option<String>,
}

/// Separa las palabras especiales (+proyecto, due:, nota:, id:) del texto.
fn split_meta(words: &[&str]) -> Meta {
    let mut m = Meta { text: String::new(), project: String::new(), due: None, note: None, id: None, mail: None, from: None };
    let mut text = Vec::new();
    for w in words {
        if let Some(p) = w.strip_prefix('+').filter(|p| !p.is_empty()) {
            m.project = p.replace('_', " ");
        } else if let Some(d) = w.strip_prefix("due:") {
            m.due = Some(d.to_string());
        } else if let Some(n) = w.strip_prefix("nota:") {
            m.note = Some(decode_note(n));
        } else if let Some(i) = w.strip_prefix("id:").filter(|i| !i.is_empty()) {
            m.id = Some(i.to_string());
        } else if let Some(c) = w.strip_prefix("correo:").filter(|c| !c.is_empty()) {
            m.mail = Some(decode_note(c));
        } else if let Some(f) = w.strip_prefix("de:").filter(|f| !f.is_empty()) {
            m.from = Some(f.to_string());
        } else {
            text.push(*w);
        }
    }
    m.text = text.join(" ");
    m
}

pub fn parse_task(line: &str) -> Option<Task> {
    let mut words: Vec<&str> = line.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let done = words[0] == "x";
    if done {
        words.remove(0);
    }
    // Fechas de término (si está hecha) y de creación al inicio.
    let mut dates = Vec::new();
    while words.first().is_some_and(|w| is_date(w)) {
        dates.push(words.remove(0).to_string());
    }
    let (done_on, created) = if done { (dates.first().cloned(), dates.get(1).cloned()) } else { (None, dates.first().cloned()) };
    let Meta { text, project, due, note, id, mail, from } = split_meta(&words);
    Some(Task { done, text, project, due, note, id, done_on, created, mail, from, raw: line.to_string() })
}

pub fn parse_event(line: &str) -> Option<Event> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let date = words.first().filter(|w| is_date(w))?.to_string();
    let (time, rest) = match words.get(1) {
        Some(t) if is_time(t) => (Some(t.to_string()), &words[2..]),
        _ => (None, &words[1..]),
    };
    let Meta { text: title, project, note, mail, .. } = split_meta(rest);
    Some(Event { date, time, title, project, note, mail })
}

pub fn format_task(created: &str, text: &str, ws: &str, due: Option<&str>, note: &str, id: Option<&str>) -> String {
    let mut s = format!("{created} {} {}", text.trim(), project_token(ws));
    if let Some(d) = due {
        s += &format!(" due:{d}");
    }
    s += &format!(" nota:{}", encode_note(note));
    if let Some(i) = id {
        s += &format!(" id:{i}");
    }
    s
}

pub fn format_event(date: &str, time: Option<&str>, title: &str, ws: &str, note: &str) -> String {
    let time = time.map(|t| format!("{t} ")).unwrap_or_default();
    format!("{date} {time}{} {} nota:{}", title.trim(), project_token(ws), encode_note(note))
}

/// Tareas repetidas, para quitarlas (pasa cuando una misma tarea vuelve por Microsoft To Do, o
/// llega de dos equipos). Solo pendientes y solo las «sueltas»: sin línea en una nota (`lines`:
/// identificador -> texto de su línea), sin correo y sin estar en `keep` (las que se devolvieron
/// con Deshacer). Una suelta sobra si ya hay otra igual: una con línea o correo, o una suelta
/// anterior en la lista. Igual = el mismo texto (sin etiquetas, fecha ni mayúsculas), o uno que
/// empieza con el otro si son largos (To Do agrega las etiquetas como palabras al final).
pub fn repeated(tasks: &[Task], lines: &HashMap<String, String>, keep: &HashSet<String>) -> Vec<String> {
    fn norm(s: &str) -> String {
        display_text(s)
            .split_whitespace()
            .filter(|w| !w.starts_with('#') && !w.starts_with("due:") && !w.starts_with('^'))
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }
    let same = |a: &str, b: &str| a == b || (a.chars().count().min(b.chars().count()) >= 24 && (a.starts_with(b) || b.starts_with(a)));
    let pending: Vec<(&Task, &str)> = tasks.iter().filter(|t| !t.done).filter_map(|t| Some((t, t.id.as_deref()?))).collect();
    let anchored = |t: &Task, id: &str| t.mail.is_some() || lines.contains_key(id);
    let mut seen: Vec<String> = Vec::new();
    for (t, id) in &pending {
        if anchored(t, id) {
            seen.push(norm(&t.text));
            seen.extend(lines.get(*id).map(|l| norm(l)));
        }
    }
    let mut out = Vec::new();
    for (t, id) in pending {
        if anchored(t, id) || keep.contains(id) {
            continue;
        }
        let k = norm(&t.text);
        if k.is_empty() {
            continue;
        }
        if seen.iter().any(|s| same(s, &k)) {
            out.push(id.to_string());
        } else {
            seen.push(k);
        }
    }
    out
}

/// Quita los eventos con esas líneas exactas.
impl Agenda {
    pub fn remove_events(&self, lines: &[String]) -> io::Result<()> {
        let keep: Vec<String> = self.read_lines(AGENDA_FILE).into_iter().filter(|l| !lines.contains(l)).collect();
        self.write_lines(AGENDA_FILE, &keep)?;
        self.write_ics()
    }
}

pub struct Agenda {
    root: PathBuf,
}

impl Agenda {
    pub fn new(root: &Path) -> Self {
        Agenda { root: root.to_path_buf() }
    }

    fn read_lines(&self, name: &str) -> Vec<String> {
        crate::vault::read_text(&self.root.join(name))
            .map(|t| t.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn write_lines(&self, name: &str, lines: &[String]) -> io::Result<()> {
        let mut text = lines.join("\n");
        if !text.is_empty() {
            text.push('\n');
        }
        fs::write(self.root.join(name), text)
    }

    pub fn tasks(&self) -> Vec<Task> {
        self.read_lines(TASKS_FILE).iter().filter_map(|l| parse_task(l)).collect()
    }

    pub fn events(&self) -> Vec<Event> {
        self.read_lines(AGENDA_FILE).iter().filter_map(|l| parse_event(l)).collect()
    }

    /// Contenido actual de ambos archivos (para poder deshacer).
    pub fn snapshot(&self) -> (String, String) {
        let r = |n| crate::vault::read_text(&self.root.join(n)).unwrap_or_default();
        (r(TASKS_FILE), r(AGENDA_FILE))
    }

    pub fn restore(&self, snap: &(String, String)) -> io::Result<()> {
        fs::write(self.root.join(TASKS_FILE), &snap.0)?;
        fs::write(self.root.join(AGENDA_FILE), &snap.1)?;
        self.write_ics()
    }

    /// Reemplaza las tareas pendientes y los eventos que vienen de una nota
    /// (las tareas ya hechas se conservan). `old_note` cubre una nota renombrada o movida.
    pub fn replace_for_note(
        &self,
        old_note: &str,
        note: &str,
        tasks: &[String],
        events: &[String],
    ) -> io::Result<()> {
        let from = |t: &Option<String>| t.as_deref().is_some_and(|n| n == old_note || n == note);
        let mut task_lines: Vec<String> = self
            .read_lines(TASKS_FILE)
            .into_iter()
            .filter(|l| parse_task(l).is_none_or(|t| t.done || !from(&t.note)))
            .collect();
        // No duplicar tareas que ya estaban hechas.
        let done: Vec<String> = task_lines
            .iter()
            .filter_map(|l| parse_task(l))
            .filter(|t| t.done && from(&t.note))
            .map(|t| t.text.to_lowercase())
            .collect();
        for t in tasks {
            if parse_task(t).is_some_and(|p| !done.contains(&p.text.to_lowercase())) {
                task_lines.push(t.clone());
            }
        }
        let mut event_lines: Vec<String> = self
            .read_lines(AGENDA_FILE)
            .into_iter()
            .filter(|l| parse_event(l).is_none_or(|e| !from(&e.note)))
            .collect();
        event_lines.extend(events.iter().cloned());
        event_lines.sort();
        self.write_lines(TASKS_FILE, &task_lines)?;
        self.write_lines(AGENDA_FILE, &event_lines)?;
        self.write_ics()
    }

    /// Agrega tareas y eventos sin tocar los que ya existen (omite los repetidos).
    pub fn add_lines(&self, tasks: &[String], events: &[String]) -> io::Result<()> {
        if tasks.is_empty() && events.is_empty() {
            return Ok(());
        }
        let key_t = |l: &str| parse_task(l).map(|t| (t.text.to_lowercase(), t.note));
        let mut task_lines = self.read_lines(TASKS_FILE);
        for t in tasks {
            if !task_lines.iter().any(|l| key_t(l) == key_t(t)) {
                task_lines.push(t.clone());
            }
        }
        let key_e = |l: &str| parse_event(l).map(|e| (e.date, e.time, e.title.to_lowercase()));
        let mut event_lines = self.read_lines(AGENDA_FILE);
        for e in events {
            if !event_lines.iter().any(|l| key_e(l) == key_e(e)) {
                event_lines.push(e.clone());
            }
        }
        event_lines.sort();
        self.write_lines(TASKS_FILE, &task_lines)?;
        self.write_lines(AGENDA_FILE, &event_lines)?;
        self.write_ics()
    }

    pub fn add_task(&self, line: String) -> io::Result<()> {
        let mut lines = self.read_lines(TASKS_FILE);
        lines.push(line);
        self.write_lines(TASKS_FILE, &lines)?;
        self.write_ics()
    }

    /// Marca o desmarca como hecha la tarea con esa línea exacta.
    pub fn toggle_task(&self, raw: &str, today: &str) -> io::Result<()> {
        let lines: Vec<String> = self
            .read_lines(TASKS_FILE)
            .into_iter()
            .map(|l| {
                if l != raw {
                    return l;
                }
                match l.strip_prefix("x ") {
                    // Quita "x " y la fecha de término.
                    Some(rest) => match rest.split_once(' ') {
                        Some((d, r)) if is_date(d) => r.to_string(),
                        _ => rest.to_string(),
                    },
                    None => format!("x {today} {l}"),
                }
            })
            .collect();
        self.write_lines(TASKS_FILE, &lines)?;
        self.write_ics()
    }

    /// Marca como hecha (o pendiente) la tarea con ese identificador. Devuelve si la encontró.
    pub fn set_done_by_id(&self, id: &str, done: bool, today: &str) -> io::Result<bool> {
        let Some(t) = self.tasks().into_iter().find(|t| t.id.as_deref() == Some(id)) else {
            return Ok(false);
        };
        if t.done != done {
            self.toggle_task(&t.raw, today)?;
        }
        Ok(true)
    }

    /// Borra la tarea con ese identificador (al unir dos tareas duplicadas).
    pub fn remove_by_id(&self, id: &str) -> io::Result<()> {
        let lines: Vec<String> = self
            .read_lines(TASKS_FILE)
            .into_iter()
            .filter(|l| parse_task(l).and_then(|t| t.id).as_deref() != Some(id))
            .collect();
        self.write_lines(TASKS_FILE, &lines)?;
        self.write_ics()
    }

    /// Cambia la tarea con esa línea exacta (None = la borra).
    pub fn replace_task(&self, raw: &str, new: Option<String>) -> io::Result<()> {
        let mut lines = Vec::new();
        for l in self.read_lines(TASKS_FILE) {
            if l != raw {
                lines.push(l);
            } else if let Some(n) = &new {
                lines.push(n.clone());
            }
        }
        self.write_lines(TASKS_FILE, &lines)?;
        self.write_ics()
    }

    /// Quita un evento de la agenda.
    pub fn remove_event(&self, e: &Event) -> io::Result<()> {
        let keep: Vec<String> = self.read_lines(AGENDA_FILE).into_iter().filter(|l| parse_event(l).as_ref() != Some(e)).collect();
        self.write_lines(AGENDA_FILE, &keep)?;
        self.write_ics()
    }

    /// Borra las tareas con esos identificadores.
    pub fn remove_ids(&self, ids: &[String]) -> io::Result<()> {
        let lines: Vec<String> =
            self.read_lines(TASKS_FILE).into_iter().filter(|l| parse_task(l).and_then(|t| t.id).is_none_or(|i| !ids.contains(&i))).collect();
        self.write_lines(TASKS_FILE, &lines)?;
        self.write_ics()
    }

    /// Da un identificador a cada tarea pendiente que no tenga (para reconocerla en To Do).
    /// Devuelve si cambió algo.
    pub fn ensure_ids(&self, mut next: impl FnMut() -> String) -> io::Result<bool> {
        let mut changed = false;
        let lines: Vec<String> = self
            .read_lines(TASKS_FILE)
            .into_iter()
            .map(|l| match parse_task(&l) {
                Some(t) if !t.done && t.id.is_none() => {
                    changed = true;
                    format!("{l} id:{}", next())
                }
                _ => l,
            })
            .collect();
        if changed {
            self.write_lines(TASKS_FILE, &lines)?;
        }
        Ok(changed)
    }

    /// Quita la fecha de la tarea con ese identificador.
    pub fn clear_due_by_id(&self, id: &str) -> io::Result<()> {
        let lines: Vec<String> = self
            .read_lines(TASKS_FILE)
            .into_iter()
            .map(|l| {
                if parse_task(&l).and_then(|t| t.id).as_deref() != Some(id) {
                    return l;
                }
                l.split_whitespace().filter(|w| !w.starts_with("due:")).collect::<Vec<_>>().join(" ")
            })
            .collect();
        self.write_lines(TASKS_FILE, &lines)?;
        self.write_ics()
    }

    /// Cambia (o pone) la fecha de la tarea con ese identificador. Devuelve si la encontró.
    pub fn set_due_by_id(&self, id: &str, date: &str) -> io::Result<bool> {
        let mut found = false;
        let lines: Vec<String> = self
            .read_lines(TASKS_FILE)
            .into_iter()
            .map(|l| {
                if parse_task(&l).and_then(|t| t.id).as_deref() != Some(id) {
                    return l;
                }
                found = true;
                let mut words: Vec<String> = l.split_whitespace().filter(|w| !w.starts_with("due:")).map(str::to_string).collect();
                words.push(format!("due:{date}"));
                words.join(" ")
            })
            .collect();
        if found {
            self.write_lines(TASKS_FILE, &lines)?;
            self.write_ics()?;
        }
        Ok(found)
    }

    /// Las tareas y eventos que venían de `from` (o las tareas con esos identificadores)
    /// pasan a la nota `note` del espacio `ws` (con `ws` vacío, cada una conserva su espacio).
    pub fn retarget(&self, from: Option<&str>, ids: &[String], note: &str, ws: &str) -> io::Result<()> {
        let fix = |l: &str| -> String {
            l.split_whitespace()
                .map(|w| {
                    if w.starts_with("nota:") {
                        format!("nota:{}", encode_note(note))
                    } else if w.starts_with('+') && w.len() > 1 && !ws.is_empty() {
                        project_token(ws)
                    } else {
                        w.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        let tasks: Vec<String> = self
            .read_lines(TASKS_FILE)
            .into_iter()
            .map(|l| match parse_task(&l) {
                Some(t) if t.id.as_ref().is_some_and(|i| ids.contains(i)) || (from.is_some() && t.note.as_deref() == from) => fix(&l),
                _ => l,
            })
            .collect();
        let events: Vec<String> = self
            .read_lines(AGENDA_FILE)
            .into_iter()
            .map(|l| match parse_event(&l) {
                Some(e) if from.is_some() && e.note.as_deref() == from => fix(&l),
                _ => l,
            })
            .collect();
        self.write_lines(TASKS_FILE, &tasks)?;
        self.write_lines(AGENDA_FILE, &events)?;
        self.write_ics()
    }

    /// Un espacio cambió de nombre: sus tareas y eventos pasan al nombre nuevo (el espacio y la
    /// nota de donde vienen). Devuelve si cambió algo.
    pub fn rename_space(&self, old: &str, new: &str) -> io::Result<bool> {
        let fix = |l: &str| -> String {
            l.split_whitespace()
                .map(|w| {
                    if w.strip_prefix('+').is_some_and(|p| p.replace('_', " ") == old) {
                        project_token(new)
                    } else if let Some(rest) = w.strip_prefix("nota:").map(decode_note).and_then(|n| n.strip_prefix(old).filter(|r| r.starts_with('/')).map(str::to_string)) {
                        format!("nota:{}", encode_note(&format!("{new}{rest}")))
                    } else {
                        w.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut changed = false;
        for name in [TASKS_FILE, AGENDA_FILE] {
            let before = self.read_lines(name);
            let after: Vec<String> = before.iter().map(|l| fix(l)).collect();
            // Solo se reescriben las líneas que cambian (las demás quedan tal cual, con sus espacios).
            let after: Vec<String> = before.iter().zip(after).map(|(b, a)| if a.split_whitespace().eq(b.split_whitespace()) { b.clone() } else { a }).collect();
            if after != before {
                self.write_lines(name, &after)?;
                changed = true;
            }
        }
        if changed {
            self.write_ics()?;
        }
        Ok(changed)
    }

    /// agenda.ics: eventos y tareas pendientes con fecha, para importar en un calendario.
    pub fn write_ics(&self) -> io::Result<()> {
        let mut out = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//nodex-notes//ES\r\nCALSCALE:GREGORIAN\r\n");
        let esc = |s: &str| s.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,");
        let compact = |d: &str| d.replace('-', "");
        let mut push = |uid: String, date: &str, time: Option<&str>, summary: String, desc: String| {
            out += "BEGIN:VEVENT\r\n";
            out += &format!("UID:{uid}@nodex-notes\r\n");
            match time {
                Some(t) => {
                    out += &format!("DTSTART:{}T{}00\r\nDURATION:PT1H\r\n", compact(date), t.replace(':', ""))
                }
                None => out += &format!("DTSTART;VALUE=DATE:{}\r\n", compact(date)),
            }
            out += &format!("SUMMARY:{}\r\nDESCRIPTION:{}\r\nEND:VEVENT\r\n", esc(&summary), esc(&desc));
        };
        for e in self.events() {
            let uid = format!("{:x}", crate::ai::fnv(&format!("{}{:?}{}", e.date, e.time, e.title)));
            push(uid, &e.date, e.time.as_deref(), e.title.clone(), e.project.clone());
        }
        for t in self.tasks().into_iter().filter(|t| !t.done) {
            if let Some(d) = t.due.as_deref().filter(|d| is_date(d)) {
                let uid = format!("{:x}", crate::ai::fnv(&t.raw));
                push(uid, d, None, format!("☐ {}", t.text), t.project.clone());
            }
        }
        out += "END:VCALENDAR\r\n";
        fs::write(self.root.join(ICS_FILE), out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_repeated_tasks() {
        let tasks: Vec<Task> = [
            "2026-09-26 Hacer el cálculo del galpón +General nota:General/Galpón id:g1 due:2026-09-26",
            "2026-10-01 Hacer el cálculo del galpón +General id:g2 due:2026-09-25",
            "2026-10-01 hacer el  cálculo del galpón +General id:g3 due:2026-09-24",
            "2026-09-30 Entregar planos a Marcor +General id:p1",
            "2026-10-01 Entregar planos a Marcor +General id:p2",
            "2026-10-01 Me gustaría que se sincronice con la nube +General id:n1",
            "2026-10-01 Me gustaría que se sincronice con la nube interfaz nube dropbox +General id:n2",
            "2026-09-29 Comentar a José el cierre +General nota:General/Paloma id:x9",
            "2026-10-01 Correo de José (29 sep): cerrar el contrato de Paloma +General id:c1",
            "2026-09-28 comentar el informe @Gisele_Muñoz +General correo:x:INBOX:1 id:m1",
            "2026-10-01 Gisele Muñoz: comentar el informe +General id:m2",
            "x 2026-09-29 2026-09-26 Llamar a Juan +General id:j1",
            "2026-10-01 Llamar a Juan +General id:j2",
            "2026-10-01 Llamar +General id:k1",
            "2026-10-01 Llamar más tarde a la oficina +General id:k2",
            "2026-10-01 Revisar el muro +General id:r1",
            "2026-10-01 Revisar el muro +General id:r2",
        ]
        .iter()
        .filter_map(|l| parse_task(l))
        .collect();
        let lines = HashMap::from([
            ("g1".to_string(), "Hacer el cálculo del galpón".to_string()),
            ("x9".to_string(), "Correo de José (29 sep): cerrar el contrato de Paloma".to_string()),
        ]);
        let keep = HashSet::from(["r2".to_string()]);
        let out = repeated(&tasks, &lines, &keep);
        // Se queda la que tiene línea (o correo, o la primera suelta); las demás sobran. La hecha
        // no cuenta, las cortas no se comparan por el comienzo y la que se devolvió no se toca.
        assert_eq!(out, ["g2", "g3", "p2", "n2", "c1", "m2"]);
    }

    #[test]
    fn dates_written_as_said() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 9, 27).unwrap(); // domingo
        let w = |t: &str| parse_when(t, today);
        assert_eq!(w("Llamar a Pedro mañana"), ("Llamar a Pedro".into(), Some("2026-09-28".into())));
        assert_eq!(w("Llamar a Pedro pasado mañana"), ("Llamar a Pedro".into(), Some("2026-09-29".into())));
        assert_eq!(w("Enviar planos para el viernes"), ("Enviar planos".into(), Some("2026-10-02".into())));
        assert_eq!(w("Clase el domingo"), ("Clase".into(), Some("2026-10-04".into())), "hoy es domingo: el próximo");
        assert_eq!(w("Informe 30 sep"), ("Informe".into(), Some("2026-09-30".into())));
        assert_eq!(w("Informe antes del 3 de octubre"), ("Informe".into(), Some("2026-10-03".into())));
        assert_eq!(w("Informe 5/1"), ("Informe".into(), Some("2027-01-05".into())), "ya pasó: el próximo año");
        assert_eq!(w("Informe due:2026-10-09"), ("Informe".into(), Some("2026-10-09".into())));
        assert_eq!(w("Comprar pan"), ("Comprar pan".into(), None));
        assert_eq!(w("mañana"), ("mañana".into(), None), "sin texto no se separa");
        assert_eq!(display_text("enviar planos corregidos @Juan_Pérez"), "Juan Pérez: enviar planos corregidos");
        assert_eq!(display_text("revisar cubicación"), "Revisar cubicación");
    }

    #[test]
    fn roundtrip_task() {
        let l = format_task("2026-09-24", "Enviar planos", "Proyecto A", Some("2026-09-26"), "Proyecto A/Reunión 1", Some("k3f9a"));
        assert_eq!(l, "2026-09-24 Enviar planos +Proyecto_A due:2026-09-26 nota:Proyecto%20A/Reunión%201 id:k3f9a");
        let t = parse_task(&l).unwrap();
        assert!(!t.done);
        assert_eq!(t.text, "Enviar planos");
        assert_eq!(t.project, "Proyecto A");
        assert_eq!(t.due.as_deref(), Some("2026-09-26"));
        assert_eq!(t.note.as_deref(), Some("Proyecto A/Reunión 1"));
        assert_eq!(t.id.as_deref(), Some("k3f9a"));
        let m = parse_task("2026-09-26 Enviar cubicación due:2026-09-29 correo:jp@gmail.com:INBOX:3 id:abc12").unwrap();
        assert_eq!((m.text.as_str(), m.mail.as_deref()), ("Enviar cubicación", Some("jp@gmail.com:INBOX:3")));
        let d = parse_task(&format!("x 2026-09-25 {l}")).unwrap();
        assert!(d.done && d.text == "Enviar planos");
    }

    #[test]
    fn parses_event() {
        let e = parse_event("2026-09-26 10:00 Visita inspector +Obra nota:Obra/x").unwrap();
        assert_eq!((e.date.as_str(), e.time.as_deref(), e.title.as_str()), ("2026-09-26", Some("10:00"), "Visita inspector"));
        assert!(parse_event("sin fecha").is_none());
    }
}
