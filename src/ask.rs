//! Preguntar: responde preguntas sobre todas las notas, tareas y agenda, citando de dónde sale cada dato.
//!
//! Si las notas son pocas se envían todas. Si son muchas, primero la IA elige cuáles leer a partir
//! de un índice (espacio, título, fecha, etiquetas y un extracto) y después responde leyendo solo esas.
//! Cada nota va con sus líneas numeradas, para que la respuesta cite "[n3:12]" (nota n3, línea 12).

use crate::agenda::{Event, Task};
use crate::config::Config;
use crate::tags;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

/// Hasta este tamaño (en caracteres) se envían todas las notas de una vez.
const ALL_LIMIT: usize = 80_000;
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
            send(Msg::Progress(format!("Buscando entre {} notas…", input.docs.len())));
            let (system, user) = selection_prompt(&input);
            let picked = crate::ai::complete(&cfg, &system, &user).ok().map(|r| parse_selection(&r, &input.docs)).unwrap_or_default();
            if picked.is_empty() { keyword_pick(&input) } else { picked }
        };
        send(Msg::Progress(match chosen.len() {
            0 => "Pensando…".to_string(),
            1 => "Leyendo 1 nota…".to_string(),
            n => format!("Leyendo {n} notas…"),
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
- Si la respuesta no está en lo que recibes, dilo claramente ("No encontré…") y, si puedes, di qué nota se acerca más."#,
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
