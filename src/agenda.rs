//! Tareas (tareas.txt, formato todo.txt) y eventos (agenda.txt), más su exportación a agenda.ics.
//!
//! ```text
//! tareas.txt:  2026-09-24 Enviar planos +Proyecto_Edificio_A due:2026-09-26 nota:Proyecto%20Edificio%20A/Reunión
//!              x 2026-09-25 2026-09-24 Pedir cotización +Obra_Talca nota:...
//! agenda.txt:  2026-09-26 10:00 Visita del inspector +Proyecto_Edificio_A nota:...
//! ```

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
    /// Correo de donde salió ("correo:cuenta:carpeta:uid").
    pub mail: Option<String>,
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
}

/// Separa las palabras especiales (+proyecto, due:, nota:, id:) del texto.
fn split_meta(words: &[&str]) -> Meta {
    let mut m = Meta { text: String::new(), project: String::new(), due: None, note: None, id: None, mail: None };
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
    let done_on = words.first().filter(|w| done && is_date(w)).map(|w| w.to_string());
    // Fechas de término y de creación al inicio.
    while words.first().is_some_and(|w| is_date(w)) {
        words.remove(0);
    }
    let Meta { text, project, due, note, id, mail } = split_meta(&words);
    Some(Task { done, text, project, due, note, id, done_on, mail, raw: line.to_string() })
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

/// Tarea que salió de un correo (sin nota; "correo:" apunta al correo).
pub fn format_mail_task(created: &str, text: &str, ws: &str, due: Option<&str>, mail: &str, id: &str) -> String {
    let mut s = format!("{created} {}", text.trim());
    if !ws.trim().is_empty() {
        s += &format!(" {}", project_token(ws));
    }
    if let Some(d) = due {
        s += &format!(" due:{d}");
    }
    s + &format!(" correo:{} id:{id}", encode_note(mail))
}

pub fn format_mail_event(date: &str, time: Option<&str>, title: &str, ws: &str, mail: &str) -> String {
    let time = time.map(|t| format!("{t} ")).unwrap_or_default();
    let ws = if ws.trim().is_empty() { String::new() } else { format!(" {}", project_token(ws)) };
    format!("{date} {time}{}{ws} correo:{}", title.trim(), encode_note(mail))
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
    /// pasan a la nota `note` del espacio `ws`.
    pub fn retarget(&self, from: Option<&str>, ids: &[String], note: &str, ws: &str) -> io::Result<()> {
        let fix = |l: &str| -> String {
            l.split_whitespace()
                .map(|w| {
                    if w.starts_with("nota:") {
                        format!("nota:{}", encode_note(note))
                    } else if w.starts_with('+') && w.len() > 1 {
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
        let m = parse_task(&format_mail_task("2026-09-26", "Enviar cubicación", "", Some("2026-09-29"), "jp@gmail.com:INBOX:3", "abc12")).unwrap();
        assert_eq!((m.text.as_str(), m.mail.as_deref(), m.project.as_str()), ("Enviar cubicación", Some("jp@gmail.com:INBOX:3"), ""));
        let e = parse_event(&format_mail_event("2026-10-01", Some("10:00"), "Visita a obra", "Consorcio", "jp@gmail.com:Sent Items:4")).unwrap();
        assert_eq!(e.mail.as_deref(), Some("jp@gmail.com:Sent Items:4"));
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
