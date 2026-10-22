//! Los formatos de las tareas (`tareas.txt`, como todo.txt) y de los eventos (`agenda.txt`):
//! leerlos y escribirlos. Los usan la app y el servidor (la app del celular), para que los dos
//! escriban exactamente igual. Ver FORMATO.md.
//!
//! ```text
//! tareas.txt:  2026-09-24 Enviar planos +Proyecto_Edificio_A due:2026-09-26 nota:Proyecto%20Edificio%20A/Reunión
//!              x 2026-09-25 2026-09-24 Pedir cotización +Obra_Talca nota:...
//! agenda.txt:  2026-09-26 10:00 Visita del inspector +Proyecto_Edificio_A nota:...
//! ```

/// Minúsculas y sin tildes (la «ñ» queda), para comparar textos escritos de cualquier forma.
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

pub const TASKS_FILE: &str = "tareas.txt";
pub const AGENDA_FILE: &str = "agenda.txt";

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
pub fn plain_word(w: &str) -> String {
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

pub const WEEKDAYS: [&str; 7] = ["lunes", "martes", "miercoles", "jueves", "viernes", "sabado", "domingo"];
pub const MONTHS: [&str; 12] = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"];

/// Un día escrito como se dice: "hoy", "mañana", "pasado mañana", "viernes", "30 sep",
/// "30 de septiembre", "30/9", "30/09/2026", "2026-09-30".
pub fn parse_day(words: &[String], today: chrono::NaiveDate) -> Option<chrono::NaiveDate> {
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

pub struct Meta {
    pub text: String,
    pub project: String,
    pub due: Option<String>,
    pub note: Option<String>,
    pub id: Option<String>,
    pub mail: Option<String>,
    pub from: Option<String>,
}

/// Separa las palabras especiales (+proyecto, due:, nota:, id:) del texto.
pub fn split_meta(words: &[&str]) -> Meta {
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

/// La misma línea de tareas.txt, hecha (con la fecha de hoy) o pendiente.
pub fn set_task_done(line: &str, done: bool, today: &str) -> String {
    match (line.strip_prefix("x "), done) {
        (Some(rest), false) => match rest.split_once(' ') {
            Some((d, r)) if is_date(d) => r.to_string(),
            _ => rest.to_string(),
        },
        (None, true) => format!("x {today} {line}"),
        _ => line.to_string(),
    }
}

/// La casilla de una línea de nota («- [ ] …», «  [x] …») marcada o no; None si no tiene.
pub fn set_box(line: &str, done: bool) -> Option<String> {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    let at = indent + if rest.starts_with("- ") || rest.starts_with("* ") { 2 } else { 0 };
    let b = line.get(at..at + 3)?;
    if !(b == "[ ]" || b == "[x]" || b == "[X]") {
        return None;
    }
    Some(format!("{}{}{}", &line[..at], if done { "[x]" } else { "[ ]" }, &line[at + 3..]))
}

/// Un identificador nuevo para unir una tarea con su línea («^k3f9a»): cinco letras y números.
pub fn new_id(seed: u64) -> String {
    const ABC: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
    let mut x = seed ^ 0x9e37_79b9_7f4a_7c15;
    (0..5)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            ABC[(x % ABC.len() as u64) as usize] as char
        })
        .collect()
}

/// ¿Es el mismo evento escrito de otra forma? Mismo día y hora, y casi las mismas palabras
/// («Jornada de Construcción Sustentable… 2026» y «Jornada Construcción Sustentable… (Inacap)»):
/// pasa cuando la IA saca lo mismo de un correo en dos vueltas.
pub fn same_event(a: &Event, b: &Event) -> bool {
    if a.date != b.date || a.time != b.time {
        return false;
    }
    let words = |s: &str| -> std::collections::BTreeSet<String> { fold(s).split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 4).map(str::to_string).collect() };
    let (x, y) = (words(&a.title), words(&b.title));
    if x.is_empty() || y.is_empty() {
        return fold(a.title.trim()) == fold(b.title.trim());
    }
    let shared = x.intersection(&y).count();
    shared * 10 >= x.len().min(y.len()) * 7
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_event_written_differently() {
        let ev = |date: &str, time: Option<&str>, title: &str| Event { date: date.into(), time: time.map(str::to_string), title: title.into(), project: String::new(), note: None, mail: None };
        let a = ev("2026-10-23", Some("10:00"), "Jornada Construcción Sustentable y Cambio Climático Región de Los Ríos 2026 (Inacap Valdivia)");
        let b = ev("2026-10-23", Some("10:00"), "Jornada de Construcción Sustentable y Cambio Climático Región de Los Ríos 2026");
        let c = ev("2026-10-23", Some("10:00"), "Jornada de Construcción Sustentable y Cambio Climático de Los Ríos 2026");
        assert!(same_event(&a, &b) && same_event(&a, &c) && same_event(&b, &c));
        assert!(!same_event(&a, &ev("2026-10-23", Some("10:00"), "Reunión con Pedro por los planos")));
        assert!(!same_event(&a, &ev("2026-10-24", Some("10:00"), &a.title)), "otro día");
        assert!(!same_event(&a, &ev("2026-10-23", Some("11:00"), &a.title)), "otra hora");
        assert!(same_event(&ev("2026-10-23", None, "Ley Karin"), &ev("2026-10-23", None, "ley karin")));
    }
}
