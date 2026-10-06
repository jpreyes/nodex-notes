//! Preguntar: responde preguntas sobre todas las notas, tareas y agenda, citando de dónde sale cada dato.
//!
//! Si las notas son pocas se envían todas. Si son muchas, la app primero se queda con las más
//! relevantes para la pregunta (`rank`, sin IA: palabras de la pregunta, título, espacio, fechas,
//! reuniones y lo reciente); si aún no caben, la IA elige cuáles leer a partir de un índice
//! (espacio, título, fecha, etiquetas y un extracto) y después responde leyendo solo esas.
//! Cada nota va con sus líneas numeradas, para que la respuesta cite "[n3:12]" (nota n3, línea 12).

use crate::agenda::{Event, Task};
use crate::config::Config;
use crate::tags;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

/// Hasta este tamaño (en caracteres) se envían todas las notas de una vez.
pub const ALL_LIMIT: usize = 80_000;
/// Con más notas que eso, cuántas preselecciona la app antes de preguntarle a la IA.
pub const MAX_CANDIDATES: usize = 120;
/// Aunque pocas notas coincidan con la pregunta, se agregan recientes hasta llegar a estas.
const MIN_CANDIDATES: usize = 20;

/// Palabras de una pregunta que no ayudan a encontrar notas.
const STOPWORDS: &[&str] = &[
    "que", "como", "cual", "cuales", "cuando", "donde", "quien", "quienes", "cuanto", "cuantos", "cuanta", "cuantas", "para", "por",
    "los", "las", "del", "una", "uno", "unos", "unas", "con", "sin", "sobre", "entre", "desde", "hasta", "esta", "este", "esto", "estas",
    "estos", "esa", "ese", "eso", "fue", "era", "eran", "son", "hay", "mis", "tus", "sus", "nos", "les", "mas", "pero", "tambien", "muy",
    "todo", "toda", "todos", "todas", "algo", "nota", "notas", "resumen", "resume", "dime", "dame", "lista", "tengo", "tiene", "tienen",
    "falta", "faltan", "hacer", "hice", "hizo", "sido", "estar", "haber", "the", "and", "pasada", "pasado", "proximo", "proxima", "semana",
    "mes", "hoy", "ayer", "manana", "dia", "dias",
];

/// Lo que se necesita de una nota para ordenarla por relevancia.
pub struct Candidate<'a> {
    pub workspace: &'a str,
    pub title: &'a str,
    /// Texto en minúsculas y sin tildes (`vault::fold`).
    pub folded: &'a str,
    /// Día de la nota: su título si es una nota del día, o su última edición (AAAA-MM-DD).
    pub day: &'a str,
    pub meeting: bool,
}

/// Palabras útiles de una pregunta: sin tildes, sin palabras de relleno y sin repetir.
pub fn keywords(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in crate::vault::fold(text).split(|c: char| !c.is_alphanumeric()) {
        if w.chars().count() > 2 && !STOPWORDS.contains(&w) && !out.iter().any(|x| x == w) {
            out.push(w.to_string());
        }
    }
    out
}

/// Rango de días al que se refiere la pregunta ("ayer", "la semana pasada", "este mes"), si lo dice.
fn date_window(question: &str, today: &str) -> Option<(String, String)> {
    let q = crate::vault::fold(question);
    let t = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d").ok()?;
    let day = |n: i64| (t - chrono::Duration::days(n)).format("%Y-%m-%d").to_string();
    let has = |w: &str| q.split(|c: char| !c.is_alphanumeric()).any(|x| x == w);
    if q.contains("semana pasada") {
        Some((day(13), day(6)))
    } else if has("ayer") {
        Some((day(1), day(1)))
    } else if has("hoy") {
        Some((day(0), day(0)))
    } else if has("semana") {
        Some((day(7), day(0)))
    } else if q.contains("mes pasado") {
        Some((day(62), day(28)))
    } else if has("mes") {
        Some((day(31), day(0)))
    } else {
        None
    }
}

/// Las notas más relevantes para la pregunta, la mejor primero (como mucho `max`). Sin IA:
/// cuenta las palabras de la pregunta (más peso en el título y el espacio, y a las palabras
/// poco comunes), las fechas que menciona, si habla de reuniones y lo reciente.
/// `notes` viene de la más reciente a la más antigua.
pub fn rank(question: &str, history: &[(String, String)], notes: &[Candidate], today: &str, max: usize) -> Vec<usize> {
    // Una pregunta de seguimiento ("¿y cuándo se entrega?") hereda el tema de la anterior.
    let mut terms = keywords(question);
    if let Some((prev, _)) = history.last() {
        for w in keywords(prev) {
            if !terms.contains(&w) {
                terms.push(w);
            }
        }
    }
    let window = date_window(question, today);
    let about_meetings = crate::vault::fold(question).contains("reunion");
    let n = notes.len().max(1) as f64;
    // Cuántas veces aparece cada palabra en cada nota, y en cuántas notas aparece.
    let counts: Vec<Vec<usize>> = terms.iter().map(|t| notes.iter().map(|c| c.folded.matches(t.as_str()).take(5).count()).collect()).collect();
    let weight: Vec<f64> = counts.iter().map(|c| (1.0 + n / (1 + c.iter().filter(|x| **x > 0).count()) as f64).ln()).collect();
    let recent = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d").ok().map(|t| (t - chrono::Duration::days(7)).format("%Y-%m-%d").to_string());

    let mut scored: Vec<(f64, usize)> = Vec::new();
    for (i, c) in notes.iter().enumerate() {
        let (title, ws) = (crate::vault::fold(c.title), crate::vault::fold(c.workspace));
        let mut score = 0.0;
        for (k, t) in terms.iter().enumerate() {
            score += counts[k][i] as f64 * weight[k];
            if title.contains(t.as_str()) {
                score += 8.0 * weight[k];
            }
            if ws.contains(t.as_str()) {
                score += 4.0 * weight[k];
            }
        }
        if let Some((from, to)) = &window {
            if c.day >= from.as_str() && c.day <= to.as_str() {
                score += 12.0;
            }
        }
        if about_meetings && c.meeting {
            score += 6.0;
        }
        if score > 0.0 {
            // Entre dos parecidas, la más reciente.
            if recent.as_deref().is_some_and(|r| c.day >= r) {
                score += 1.0;
            }
            scored.push((score, i));
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    let mut out: Vec<usize> = scored.into_iter().take(max).map(|(_, i)| i).collect();
    // Pocas coincidencias (o una pregunta general): se completa con lo más reciente.
    let mut i = 0;
    while out.len() < MIN_CANDIDATES.min(max) && i < notes.len() {
        if !out.contains(&i) {
            out.push(i);
        }
        i += 1;
    }
    out
}
/// Tamaño máximo de las notas elegidas, y de cada una.
const SELECTED_LIMIT: usize = 60_000;
const NOTE_LIMIT: usize = 12_000;
const MAX_SELECTED: usize = 12;

/// Una nota, con su clave para las citas ("n3").
#[derive(Clone)]
pub struct Doc {
    pub key: String,
    pub path: PathBuf,
    pub workspace: String,
    pub title: String,
    /// Fecha de la última edición (AAAA-MM-DD).
    pub date: String,
    pub text: String,
}

impl Doc {
    pub fn label(&self) -> String {
        format!("{}/{}", self.workspace, self.title)
    }
}

pub struct Input {
    pub question: String,
    /// Preguntas y respuestas anteriores de esta conversación.
    pub history: Vec<(String, String)>,
    pub docs: Vec<Doc>,
    /// Tareas con su clave ("t4").
    pub tasks: Vec<(String, Task)>,
    pub events: Vec<Event>,
    /// Correos recientes, uno por línea (fecha · de/para · asunto · resumen).
    pub mails: Vec<String>,
    pub today: String,
}

pub enum Msg {
    Progress(String),
    Done(Result<String, String>),
}

/// Pregunta en un hilo aparte; los avances y la respuesta llegan por el canal.
pub fn start(cfg: &Config, input: Input, ctx: eframe::egui::Context) -> Receiver<Msg> {
    let (tx, rx) = mpsc::channel();
    let cfg = cfg.clone();
    std::thread::spawn(move || {
        let send = |m: Msg| {
            let _ = tx.send(m);
            ctx.request_repaint();
        };
        let total: usize = input.docs.iter().map(|d| d.text.len()).sum();
        let chosen: Vec<usize> = if total <= ALL_LIMIT {
            (0..input.docs.len()).collect()
        } else {
            send(Msg::Progress(tf!("Buscando entre {n} notas…", n = input.docs.len())));
            let (system, user) = selection_prompt(&input);
            let picked = crate::ai::complete(&cfg, &system, &user).ok().map(|r| parse_selection(&r, &input.docs)).unwrap_or_default();
            if picked.is_empty() { keyword_pick(&input) } else { picked }
        };
        send(Msg::Progress(match chosen.len() {
            0 => t!("Pensando…").to_string(),
            1 => t!("Leyendo 1 nota…").to_string(),
            n => tf!("Leyendo {n} notas…", n = n),
        }));
        let (system, user) = answer_prompt(&input, &chosen);
        let result = crate::ai::complete(&cfg, &system, &user);
        log(&cfg, &user, &result);
        send(Msg::Done(result));
    });
    rx
}

fn log(cfg: &Config, user: &str, result: &Result<String, String>) {
    let out = match result {
        Ok(a) => a.clone(),
        Err(e) => format!("ERROR: {e}"),
    };
    let text = format!(
        "Fecha: {}\nModelo: {} · {}\n\n== Respuesta ==\n{out}\n\n== Enviado ==\n{}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        cfg.proveedor,
        cfg.modelo,
        user.chars().take(40_000).collect::<String>()
    );
    let _ = std::fs::write(crate::config::config_path().with_file_name("ia-pregunta.txt"), text);
}

fn today_long(today: &str) -> String {
    use chrono::Datelike;
    const DIAS: [&str; 7] = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];
    match chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d") {
        Ok(d) => format!("{} {today}", DIAS[d.weekday().num_days_from_monday() as usize]),
        Err(_) => today.to_string(),
    }
}

/// Primeras palabras de una nota, sin identificadores internos.
fn snippet(text: &str, n: usize) -> String {
    let s: String = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
        .split_whitespace()
        .filter(|w| !(w.starts_with('^') && w.len() > 3))
        .collect::<Vec<_>>()
        .join(" ");
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

fn doc_tags(text: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for t in text.lines().flat_map(tags::line_tags) {
        if !seen.contains(&t) {
            seen.push(t);
        }
    }
    seen.truncate(8);
    seen
}

// ---------- Paso 1: elegir qué notas leer ----------

fn selection_prompt(input: &Input) -> (String, String) {
    let system = format!(
        r#"Eliges qué notas hay que leer para responder una pregunta sobre las notas de una persona (en español). Recibes un índice: clave, espacio/título, fecha de edición, etiquetas y el comienzo de cada nota. Devuelve SOLO un objeto JSON válido, sin texto adicional: {{"notas": ["n3", "n7"]}} con hasta {MAX_SELECTED} claves, las más útiles primero. Ten en cuenta las fechas ("la semana pasada", "ayer"), los espacios (proyectos), los títulos, las etiquetas y si es una reunión. Hoy es {}."#,
        today_long(&input.today)
    );
    let mut index = String::new();
    for d in &input.docs {
        let tags = doc_tags(&d.text).iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ");
        index += &format!("[{}] {} · {} · {tags} · {}\n", d.key, d.label(), d.date, snippet(&d.text, 140));
    }
    let history = history_text(input);
    let user = format!("{history}Pregunta: {}\n\nÍndice de notas:\n{index}", input.question);
    (system, user)
}

fn parse_selection(reply: &str, docs: &[Doc]) -> Vec<usize> {
    #[derive(serde::Deserialize)]
    struct Sel {
        #[serde(default)]
        notas: Vec<String>,
    }
    let (Some(a), Some(b)) = (reply.find('{'), reply.rfind('}')) else { return Vec::new() };
    let Ok(sel) = serde_json::from_str::<Sel>(&reply[a..=b]) else { return Vec::new() };
    let mut out = Vec::new();
    for k in sel.notas {
        let k = k.trim().trim_matches(['[', ']']).to_lowercase();
        if let Some(i) = docs.iter().position(|d| d.key == k) {
            if !out.contains(&i) {
                out.push(i);
            }
        }
    }
    out.truncate(MAX_SELECTED);
    out
}

/// Plan B si la IA no eligió: las notas que más palabras de la pregunta contienen.
fn keyword_pick(input: &Input) -> Vec<usize> {
    let words: Vec<String> = input
        .question
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() > 3)
        .map(str::to_string)
        .collect();
    let mut scored: Vec<(usize, usize)> = input
        .docs
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let hay = format!("{} {}", d.label(), d.text).to_lowercase();
            (words.iter().map(|w| hay.matches(w.as_str()).count()).sum(), i)
        })
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut out: Vec<usize> = scored.into_iter().take(MAX_SELECTED).map(|(_, i)| i).collect();
    if out.is_empty() {
        out = (0..input.docs.len().min(MAX_SELECTED)).collect(); // las más recientes
    }
    out
}

// ---------- Paso 2: responder ----------

fn history_text(input: &Input) -> String {
    if input.history.is_empty() {
        return String::new();
    }
    let mut s = String::from("Conversación anterior:\n");
    for (q, a) in input.history.iter().rev().take(3).rev() {
        s += &format!("P: {q}\nR: {}\n\n", a.chars().take(2000).collect::<String>());
    }
    s
}

fn answer_prompt(input: &Input, chosen: &[usize]) -> (String, String) {
    let system = format!(
        r#"Respondes preguntas sobre las notas personales de una persona que escribe en español. Usa SOLO la información de las notas, tareas y agenda que recibes; no inventes. Hoy es {}.

Reglas:
- Responde directo y breve, en español. Usa listas con "- " cuando enumeres y **negrita** solo para lo clave. Sin tablas ni títulos.
- Después de cada dato, cita de dónde sale: [n3:12] = nota n3, línea 12 (o [n3] si es la nota completa). Una tarea de la lista de tareas se cita con su clave: [t4]. Puedes citar varias: [n3:12][n5:2].
- Para listar tareas pendientes usa "- [ ] texto [t4]" (y "- [x] texto [t4]" para las hechas).
- Interpreta las fechas relativas ("la semana pasada", "ayer", "el lunes") con la fecha de hoy y las fechas de las notas (el título de las notas del día es su fecha).
- Si la respuesta no está en lo que recibes, dilo claramente ("No encontré…") y, si puedes, di qué nota se acerca más.
- Puedes HACER cosas cuando la persona lo pide o lo da por hecho («ya subí la presentación», «créame un evento…», «anota que…»): marcar una tarea hecha, crear una tarea, crear un evento en la agenda, anotar algo en una nota, anotar un seguimiento de una tarea (lo que se hizo) o cambiar la fecha de una tarea. Para eso, al final de la respuesta agrega un bloque así (solo las acciones que corresponden):
```acciones
{{"acciones": [{{"tipo": "hecha", "tarea": "t4"}}, {{"tipo": "tarea", "texto": "Enviar planos", "fecha": "AAAA-MM-DD o vacío", "espacio": ""}}, {{"tipo": "evento", "titulo": "Congreso", "fecha": "AAAA-MM-DD", "hora": "HH:MM o vacío", "lugar": "", "espacio": ""}}, {{"tipo": "anotar", "nota": "n3 o hoy", "texto": "…"}}, {{"tipo": "seguimiento", "tarea": "t4", "texto": "lo que se hizo"}}, {{"tipo": "fecha", "tarea": "t4", "fecha": "AAAA-MM-DD"}}]}}
```
  En el texto di en pasado lo que hiciste («Listo: marqué hecha…, creé el evento…»); nunca digas que no puedes hacerlo. Un evento sin hora es de todo el día: no pidas la hora. Si falta algo imprescindible (por ejemplo, la fecha de un evento), pregúntalo y no agregues esa acción; no inventes datos. "espacio" es uno de los espacios de la persona si se deduce de las notas; si no, vacío. Si no pidió hacer nada, no agregues el bloque."#,
        today_long(&input.today)
    );
    let mut user = history_text(input);
    user += &format!("Pregunta: {}\n\n", input.question);

    let pending: Vec<&(String, Task)> = input.tasks.iter().filter(|(_, t)| !t.done).collect();
    if !pending.is_empty() {
        user += "Tareas pendientes (tareas.txt):\n";
        for (k, t) in pending {
            user += &format!("[{k}] {}", t.text);
            if let Some(d) = &t.due {
                user += &format!(" · vence {d}");
            }
            if !t.project.is_empty() {
                user += &format!(" · {}", t.project);
            }
            if let Some(n) = &t.note {
                user += &format!(" · nota {n}");
            }
            user.push('\n');
        }
        user.push('\n');
    }
    let done: Vec<&(String, Task)> = input.tasks.iter().filter(|(_, t)| t.done).collect();
    if !done.is_empty() {
        user += "Tareas hechas (las más recientes):\n";
        for (k, t) in done {
            user += &format!("[{k}] {}\n", t.text);
        }
        user.push('\n');
    }
    if !input.events.is_empty() {
        user += "Agenda:\n";
        for e in &input.events {
            let time = e.time.as_deref().map(|t| format!(" {t}")).unwrap_or_default();
            user += &format!("- {}{time} {} · {}\n", e.date, e.title, e.project);
        }
        user.push('\n');
    }

    if !input.mails.is_empty() {
        user += "Correos recientes (cítalos en texto, por ejemplo: correo de Juan del 26 sep):\n";
        for m in &input.mails {
            user += &format!("- {m}\n");
        }
        user.push('\n');
    }

    user += "Notas:\n";
    let mut used = 0;
    for &i in chosen {
        let d = &input.docs[i];
        if used >= SELECTED_LIMIT && chosen.len() > 1 {
            break;
        }
        let body: String = d.text.chars().take(NOTE_LIMIT).collect();
        used += body.len();
        user += &format!("[{}] {} · editada {}\n<<<\n", d.key, d.label(), d.date);
        for (n, line) in body.lines().enumerate() {
            user += &format!("{}│ {line}\n", n + 1);
        }
        user += ">>>\n\n";
    }
    if chosen.is_empty() {
        user += "(no hay notas)\n";
    }
    (system, user)
}

// ---------- La respuesta, lista para mostrar ----------

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    /// "[n3:12]" -> ("n3", Some(12)); "[t4]" -> ("t4", None).
    Cite(String, Option<usize>),
}

/// Algo que la persona pidió hacer (lo trae la respuesta en un bloque ```acciones).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Accion {
    /// hecha | tarea | evento | anotar | seguimiento | fecha
    pub tipo: String,
    /// Clave de la tarea («t4»).
    pub tarea: String,
    pub texto: String,
    pub titulo: String,
    pub fecha: String,
    pub hora: String,
    pub lugar: String,
    pub espacio: String,
    /// Clave de la nota («n3») o «hoy».
    pub nota: String,
}

/// Separa de la respuesta el bloque de acciones (si lo trae): el texto sin él y las acciones.
pub fn split_actions(answer: &str) -> (String, Vec<Accion>) {
    #[derive(serde::Deserialize)]
    struct Wrap {
        acciones: Vec<Accion>,
    }
    let mut rest = answer;
    let mut text = String::new();
    let mut actions = Vec::new();
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        let Some(end) = after.find("```") else { break };
        let block = &after[..end];
        // El contenido: después de la primera línea («acciones», «json» o nada).
        let body = block.split_once('\n').map_or(block, |(first, b)| if first.trim().starts_with('{') { block } else { b });
        match serde_json::from_str::<Wrap>(body.trim()) {
            Ok(w) => {
                text += &rest[..start];
                actions.extend(w.acciones.into_iter().filter(|a| !a.tipo.trim().is_empty()));
            }
            Err(_) => text += &rest[..start + 3 + end + 3],
        }
        rest = &after[end + 3..];
    }
    text += rest;
    (text.trim_end().to_string(), actions)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Para(Vec<Inline>),
    Item { level: u8, check: Option<bool>, parts: Vec<Inline> },
}

/// Separa el texto y las citas ("[n3:12]", "[t4]", "[n3, n5]").
pub fn inlines(s: &str) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    let mut text = String::new();
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']').map(|c| open + c) else { break };
        let inner = &rest[open + 1..close];
        let cites: Option<Vec<Inline>> = inner
            .split([',', ';'])
            .map(|p| {
                let p = p.trim().to_lowercase();
                let (key, line) = match p.split_once(':') {
                    Some((k, l)) => (k.trim().to_string(), l.trim().trim_start_matches('l').split('-').next()?.trim().parse().ok()),
                    None => (p.clone(), None),
                };
                let valid = key.len() >= 2
                    && (key.starts_with('n') || key.starts_with('t'))
                    && key[1..].chars().all(|c| c.is_ascii_digit());
                valid.then_some(Inline::Cite(key, line))
            })
            .collect();
        match cites {
            Some(c) if !c.is_empty() => {
                text += &rest[..open];
                if !text.is_empty() {
                    out.push(Inline::Text(std::mem::take(&mut text)));
                }
                out.extend(c);
            }
            _ => text += &rest[..=close],
        }
        rest = &rest[close + 1..];
    }
    text += rest;
    // Sin espacios colgando antes de una cita ("dato [n1]" -> "dato¹").
    if !text.is_empty() {
        out.push(Inline::Text(text));
    }
    for i in 0..out.len() {
        if matches!(out.get(i + 1), Some(Inline::Cite(..))) {
            if let Inline::Text(t) = &mut out[i] {
                let trimmed = t.trim_end().to_string();
                *t = trimmed;
            }
        }
    }
    out.retain(|p| !matches!(p, Inline::Text(t) if t.is_empty()));
    out
}

pub fn parse_answer(s: &str) -> Vec<Block> {
    let mut out = Vec::new();
    for raw in s.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        let t = raw.trim();
        let t = t.trim_start_matches('#').trim_start();
        let bullet = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("• "));
        let numbered = t.split_once(". ").filter(|(n, _)| !n.is_empty() && n.len() <= 3 && n.chars().all(|c| c.is_ascii_digit()));
        match (bullet, numbered) {
            (Some(b), _) => {
                let (check, text) = if let Some(x) = b.strip_prefix("[ ] ") {
                    (Some(false), x)
                } else if let Some(x) = b.strip_prefix("[x] ").or_else(|| b.strip_prefix("[X] ")) {
                    (Some(true), x)
                } else {
                    (None, b)
                };
                out.push(Block::Item { level: (indent / 2).min(3) as u8, check, parts: inlines(text) });
            }
            (None, Some((n, text))) => {
                let mut parts = vec![Inline::Text(format!("{n}. "))];
                parts.extend(inlines(text));
                out.push(Block::Item { level: (indent / 2).min(3) as u8, check: None, parts });
            }
            _ => out.push(Block::Para(inlines(t))),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(key: &str, title: &str, text: &str) -> Doc {
        Doc { key: key.into(), path: PathBuf::from(format!("{title}.md")), workspace: "Consorcio".into(), title: title.into(), date: "2026-09-24".into(), text: text.into() }
    }

    #[test]
    fn actions_come_apart_from_the_text() {
        let a = "Listo: marqué hecha la presentación [t2] y creé el evento.\n\n```acciones\n{\"acciones\": [{\"tipo\": \"hecha\", \"tarea\": \"t2\"}, {\"tipo\": \"evento\", \"titulo\": \"18° Congreso AICE\", \"fecha\": \"2026-10-02\", \"lugar\": \"Villarrica\"}]}\n```";
        let (text, acts) = split_actions(a);
        assert_eq!(text, "Listo: marqué hecha la presentación [t2] y creé el evento.");
        assert_eq!(acts.len(), 2);
        assert_eq!((acts[0].tipo.as_str(), acts[0].tarea.as_str()), ("hecha", "t2"));
        assert_eq!((acts[1].fecha.as_str(), acts[1].hora.as_str(), acts[1].lugar.as_str()), ("2026-10-02", "", "Villarrica"));
        // Otros bloques de código se quedan en el texto.
        let (text, acts) = split_actions("Usa:\n```\ncargo build\n```\nfin");
        assert_eq!(text, "Usa:\n```\ncargo build\n```\nfin");
        assert!(acts.is_empty());
    }

    #[test]
    fn prompt_numbers_lines_and_lists_tasks() {
        let task = crate::agenda::parse_task("2026-09-24 Entregar informe +Docencia due:2026-09-26 nota:Docencia/Informe id:k3f9a").unwrap();
        let input = Input {
            question: "¿Qué me falta?".into(),
            history: vec![("hola".into(), "hola [n1]".into())],
            docs: vec![doc("n1", "Trincheras", "Revisión de taludes #trincheras\n  están en Dropbox ^k3f9a")],
            tasks: vec![("t1".into(), task)],
            events: vec![],
            mails: vec![],
            today: "2026-09-25".into(),
        };
        let (system, user) = answer_prompt(&input, &[0]);
        assert!(system.contains("viernes 2026-09-25"), "{system}");
        assert!(user.contains("[t1] Entregar informe · vence 2026-09-26 · Docencia · nota Docencia/Informe\n"), "{user}");
        assert!(user.contains("[n1] Consorcio/Trincheras · editada 2026-09-24\n<<<\n1│ Revisión de taludes #trincheras\n2│   están en Dropbox ^k3f9a\n>>>"), "{user}");
        assert!(user.starts_with("Conversación anterior:\nP: hola\n"), "{user}");
        let (_, index) = selection_prompt(&input);
        assert!(index.contains("[n1] Consorcio/Trincheras · 2026-09-24 · #trincheras · Revisión de taludes #trincheras · están en Dropbox\n"), "{index}");
        assert_eq!(parse_selection("```json\n{\"notas\": [\"n1\", \"n9\", \"[N1]\"]}\n```", &input.docs), vec![0]);
        assert_eq!(keyword_pick(&Input { question: "¿dónde están los taludes?".into(), ..input }), vec![0]);
    }

    /// Con muchas notas, la app elige sola las que tienen que ver con la pregunta.
    #[test]
    fn ranks_notes_for_a_question() {
        let texts = [
            ("General", "2026-09-29", "comprar pan\nllamar al medico", "2026-09-29", false),
            ("Obra Talca", "Muro eje 3", "revisar la cubicacion del muro\nfalta el acero", "2026-08-02", false),
            ("Consorcio", "Reunión CIC", "## reunion cic · 2026-09-24 10:00\n- 10:02 se hablo del presupuesto", "2026-09-24", true),
            ("Docencia", "Clases", "preparar la clase de hormigon", "2026-05-10", false),
            ("General", "2026-09-22", "pedir la cubicacion a juan", "2026-09-22", false),
        ];
        let notes: Vec<Candidate> = texts.iter().map(|(ws, title, folded, day, meeting)| Candidate { workspace: ws, title, folded, day, meeting: *meeting }).collect();
        let today = "2026-09-30";
        assert_eq!(keywords("¿Cómo va la cubicación del muro en Obra Talca?"), vec!["cubicacion", "muro", "obra", "talca"]);
        // Palabras de la pregunta: primero la que las tiene en el título y el espacio.
        let r = rank("¿Cómo va la cubicación del muro en Obra Talca?", &[], &notes, today, 3);
        assert_eq!(r[..2], [1, 4], "{r:?}");
        // Reuniones y fechas.
        assert_eq!(rank("¿Qué se habló en la reunión de la semana pasada?", &[], &notes, today, 1), vec![2]);
        assert_eq!(rank("¿Qué anoté ayer?", &[], &notes, today, 1), vec![0]);
        // Una pregunta de seguimiento hereda el tema de la anterior.
        let history = vec![("¿Cómo va la cubicación del muro?".to_string(), "Falta el acero".to_string())];
        assert_eq!(rank("¿Y quién la tiene?", &history, &notes, today, 1), vec![1]);
        // Pregunta general: lo más reciente.
        assert_eq!(rank("¿Qué tengo?", &[], &notes, today, 3), vec![0, 1, 2]);
    }

    #[test]
    fn parses_answers_with_citations() {
        assert_eq!(
            inlines("Está en Dropbox [n3:12][n5], ver [t4] y [nota]."),
            vec![
                Inline::Text("Está en Dropbox".into()),
                Inline::Cite("n3".into(), Some(12)),
                Inline::Cite("n5".into(), None),
                Inline::Text(", ver".into()),
                Inline::Cite("t4".into(), None),
                Inline::Text(" y [nota].".into()),
            ]
        );
        assert_eq!(inlines("a [n1, n2:L4-6]"), vec![Inline::Text("a".into()), Inline::Cite("n1".into(), None), Inline::Cite("n2".into(), Some(4))]);
        let b = parse_answer("Quedan **3** pendientes:\n\n- [ ] Entregar informe [t1]\n  - detalle [n2:3]\n1. uno\n## Título");
        assert_eq!(b.len(), 5);
        assert!(matches!(&b[1], Block::Item { level: 0, check: Some(false), .. }));
        assert!(matches!(&b[2], Block::Item { level: 1, check: None, .. }));
        assert_eq!(b[4], Block::Para(vec![Inline::Text("Título".into())]));
    }

    /// De punta a punta con un servidor falso compatible con OpenAI: la pregunta viaja con
    /// las notas numeradas y la respuesta vuelve por el canal.
    #[test]
    fn asks_a_fake_server() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 65536];
            // Lee cabeceras y cuerpo completos (Content-Length).
            loop {
                let n = s.read(&mut buf).unwrap();
                req.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&req).to_string();
                if let Some(h) = text.find("\r\n\r\n") {
                    let len = text[..h].lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                    if req.len() >= h + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let content = "Te falta **1** tarea:\\n- [ ] Entregar informe [t1]\\nLas trincheras están en Dropbox [n1:2].";
            let body = format!(r#"{{"id":"x","object":"chat.completion","model":"m","choices":[{{"index":0,"message":{{"role":"assistant","content":"{content}"}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}}}"#);
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(format!("{head}{body}").as_bytes());
            String::from_utf8_lossy(&req).to_string()
        });
        unsafe {
            std::env::set_var("NODEX_AI_ENDPOINT", format!("http://127.0.0.1:{port}/"));
            std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id())));
        }
        let cfg = Config { proveedor: "opencode".into(), clave_api: "k".into(), ..Config::default() };
        let task = crate::agenda::parse_task("2026-09-24 Entregar informe +General nota:General/x id:abc12").unwrap();
        let input = Input {
            question: "¿Qué me falta?".into(),
            history: vec![],
            docs: vec![doc("n1", "Trincheras", "Revisar taludes
  están en Dropbox")],
            tasks: vec![("t1".into(), task)],
            events: vec![],
            mails: vec![],
            today: "2026-09-25".into(),
        };
        let rx = start(&cfg, input, eframe::egui::Context::default());
        let answer = loop {
            match rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap() {
                Msg::Progress(_) => continue,
                Msg::Done(r) => break r.unwrap(),
            }
        };
        let request = server.join().unwrap();
        assert!(request.contains("2│   están en Dropbox"), "{request}");
        assert!(request.contains("[t1] Entregar informe"), "{request}");
        let blocks = parse_answer(&answer);
        assert_eq!(blocks.len(), 3, "{answer}");
        assert!(matches!(&blocks[2], Block::Para(p) if p.contains(&Inline::Cite("n1".into(), Some(2)))));
    }
}
