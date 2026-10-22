//! La app del celular: una app web instalable (`/app`, ver `web/`) que lee y escribe directo en
//! los archivos sincronizados de la cuenta, los mismos de los equipos y con los mismos formatos
//! (`nodex_core`). Lo que se hace aquí llega a los equipos al instante, como un equipo más.
//!
//! - `GET /v1/movil/hoy`: lo de hoy (tareas, eventos, lo por agendar, hábitos y la nota del día).
//! - `GET /v1/movil/tareas`, `GET /v1/movil/agenda`: todas las tareas pendientes; la semana.
//! - `POST /v1/movil/anotar`: una línea en la nota del día (la IA la organiza después, en un equipo).
//! - `POST /v1/movil/tarea`, `POST /v1/movil/tarea/hecha`: agregar una tarea; marcarla hecha.
//! - `POST /v1/movil/habito`, `POST /v1/movil/por-agendar`: marcar un hábito; agendar o descartar.
//! - `GET /v1/movil/notas`, `GET /v1/movil/nota`, `POST /v1/movil/nota/linea`: buscar, leer y
//!   agregar a una nota.
//! - `POST /v1/movil/preguntar`: preguntarle a la IA (con lo de hoy y las notas que vienen al caso).
//!
//! Todas piden la sesión de la cuenta (`Authorization: Bearer …`). El día lo dice el celular
//! (`hoy`), para que sea el de su zona horaria.

use super::{AppState, error};
use crate::sync::{self, Entry};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{Datelike, NaiveDate};
use nodex_core::formato::{self as fmt, Event, Task, fold};
use nodex_core::habitos;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

const DIARY: &str = "Diario";
const PROPOSALS: &str = ".nodex/por-agendar.txt";
const EVENT_MARKS: &str = ".nodex/eventos.txt";
/// Carpetas que no son espacios ni tienen notas para leer.
const NOT_NOTES: [&str; 4] = ["Plantillas", "Archivo", "Adjuntos", "Bloc"];
/// Cuántas notas recientes se buscan por dentro (las demás, por el título).
const SEARCH_INSIDE: usize = 2000;
/// Cuánto texto de notas va como mucho en una pregunta.
const ASK_NOTES: usize = 40_000;

/// El día de hoy según el celular (o el del servidor).
fn day_or_today(d: Option<&str>) -> String {
    d.filter(|d| fmt::is_date(d)).map(str::to_string).unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string())
}

fn shift(day: &str, n: i64) -> String {
    NaiveDate::parse_from_str(day, "%Y-%m-%d").map(|d| (d + chrono::Duration::days(n)).format("%Y-%m-%d").to_string()).unwrap_or_default()
}

fn now_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

/// Agrega líneas al final de un texto (con su salto de línea).
fn append(old: Option<&str>, lines: &[String]) -> String {
    let mut text = old.unwrap_or("").to_string();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    for l in lines {
        text += l;
        text.push('\n');
    }
    text
}

fn task_json(t: &Task) -> Value {
    json!({
        "clave": t.raw,
        "texto": fmt::display_text(&t.text),
        "hecha": t.done,
        "fecha": t.due,
        "espacio": t.project,
        "nota": t.note,
        "id": t.id,
    })
}

fn event_json(e: &Event, done: bool) -> Value {
    json!({ "fecha": e.date, "hora": e.time, "titulo": e.title, "espacio": e.project, "nota": e.note, "hecho": done })
}

/// La clave de un evento en «Por agendar» (la misma de la app).
fn proposal_key(e: &Event) -> String {
    format!("{}|{}|{}", e.date, e.time.clone().unwrap_or_default(), fold(e.title.trim()))
}

/// La clave de un evento en sus marcas (hecho, convertido en tarea, quitado).
fn event_mark_key(e: &Event) -> String {
    format!("evento:{}:{}", e.date, fold(e.title.trim()))
}

/// Lo que se lee de la cuenta para armar las respuestas.
struct Data {
    tasks: Vec<Task>,
    events: Vec<Event>,
    marks: HashMap<String, String>,
    /// Por agendar: clave -> línea (las pendientes).
    proposals: Vec<(String, String)>,
    habits: Vec<habitos::Habit>,
}

async fn text_of(state: &AppState, acc: &str, ruta: &str) -> String {
    sync::read_text(state, acc, ruta).await.map(|t| t.0).unwrap_or_default()
}

async fn load(state: &AppState, acc: &str) -> Data {
    let tasks = text_of(state, acc, fmt::TASKS_FILE).await.lines().filter_map(fmt::parse_task).collect();
    let events = text_of(state, acc, fmt::AGENDA_FILE).await.lines().filter_map(fmt::parse_event).collect();
    let mut marks = HashMap::new();
    for l in text_of(state, acc, EVENT_MARKS).await.lines() {
        if let Some((s, k)) = l.split_once('\t') {
            if s == "-" {
                marks.remove(k);
            } else {
                marks.insert(k.to_string(), s.to_string());
            }
        }
    }
    let mut states: Vec<(String, (String, String))> = Vec::new();
    for l in text_of(state, acc, PROPOSALS).await.lines() {
        let mut p = l.splitn(3, '\t');
        if let (Some(s), Some(k), Some(line)) = (p.next(), p.next(), p.next()) {
            states.retain(|(key, _)| key != k);
            states.push((k.to_string(), (s.to_string(), line.to_string())));
        }
    }
    let proposals = states.into_iter().filter(|(_, (s, _))| s == "pendiente").map(|(k, (_, l))| (k, l)).collect();
    let habits = habitos::parse(&text_of(state, acc, &format!(".nodex/{}", habitos::FILE)).await);
    Data { tasks, events, marks, proposals, habits }
}

fn habit_json(h: &habitos::Habit, today: &str) -> Value {
    let (unit, week) = match h.freq {
        habitos::Freq::Weekly(n) => ("semanas", Some(json!({ "hechas": h.week_count(today), "meta": n }))),
        habitos::Freq::Days(_) => ("veces", None),
        habitos::Freq::Daily => ("dias", None),
    };
    // Los últimos 35 días (para la cuadrícula).
    let first = shift(today, -34);
    let done: Vec<&String> = h.done.range(first..).collect();
    json!({
        "id": h.id,
        "nombre": h.name,
        "frecuencia": h.freq.encode(),
        "toca_hoy": h.due_on(today),
        "hecho_hoy": h.is_done(today),
        "racha": h.streak(today),
        "mejor": h.best(today),
        "unidad": unit,
        "semana": week,
        "desde": h.since,
        "hechos": done,
    })
}

async fn account(state: &AppState, headers: &HeaderMap) -> Result<String, Response> {
    crate::accounts::account_of(state, headers).await.map(|a| a.id)
}

fn failed(e: String) -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, &format!("No se pudo guardar: {e}"))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Day {
    hoy: Option<String>,
}

/// Lo de hoy: tareas (atrasadas, hoy, mañana y la semana), eventos, lo por agendar, hábitos y la
/// nota del día.
pub async fn today(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<Day>) -> Response {
    let acc = match account(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let today = day_or_today(q.hoy.as_deref());
    let week = shift(&today, 7);
    let d = load(&state, &acc).await;
    let mut tasks: Vec<&Task> = d.tasks.iter().filter(|t| !t.done && t.due.as_deref().is_some_and(|f| fmt::is_date(f) && f <= week.as_str())).collect();
    tasks.sort_by(|a, b| a.due.cmp(&b.due));
    let undated = d.tasks.iter().filter(|t| !t.done && t.due.as_deref().is_none_or(|f| !fmt::is_date(f))).count();
    let mut events: Vec<&Event> = d
        .events
        .iter()
        .filter(|e| e.date >= today && e.date <= week)
        .filter(|e| d.marks.get(&event_mark_key(e)).is_none_or(|m| m != "tarea" && m != "quitado"))
        .collect();
    events.sort_by(|a, b| (&a.date, &a.time).cmp(&(&b.date, &b.time)));
    // El mismo evento escrito de otra forma, una vez; por fecha y hora.
    let mut pending: Vec<(&String, Event)> = d.proposals.iter().filter_map(|(k, l)| Some((k, fmt::parse_event(l)?))).filter(|(_, e)| e.date >= today).collect();
    pending.sort_by(|a, b| (&a.1.date, &a.1.time, &a.1.title).cmp(&(&b.1.date, &b.1.time, &b.1.title)));
    let mut shown: Vec<(&String, Event)> = Vec::new();
    for (k, e) in pending {
        if !shown.iter().any(|(_, x)| fmt::same_event(x, &e)) {
            shown.push((k, e));
        }
    }
    let proposals: Vec<Value> = shown.iter().map(|(k, e)| json!({ "clave": k, "evento": event_json(e, false) })).collect();
    let due_habits: Vec<Value> = d.habits.iter().filter(|h| h.due_on(&today) || matches!(h.freq, habitos::Freq::Weekly(_))).map(|h| habit_json(h, &today)).collect();
    let (done, total) = habitos::today_count(&d.habits, &today);
    let note = text_of(&state, &acc, &format!("{DIARY}/{today}.md")).await;
    Json(json!({
        "hoy": today,
        "tareas": tasks.iter().map(|t| task_json(t)).collect::<Vec<_>>(),
        "sin_fecha": undated,
        "eventos": events.iter().map(|e| event_json(e, d.marks.get(&event_mark_key(e)).is_some_and(|m| m == "hecho"))).collect::<Vec<_>>(),
        "por_agendar": proposals,
        "habitos": due_habits,
        "todos_habitos": d.habits.iter().map(|h| habit_json(h, &today)).collect::<Vec<_>>(),
        "meta": { "hechos": done, "total": total, "cumplida": habitos::goal_met(&d.habits, &today), "racha": habitos::goal_streak(&d.habits, &today) },
        "nota_hoy": note.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>(),
    }))
    .into_response()
}

/// Todas las tareas pendientes, y las hechas hace poco.
pub async fn tasks(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let acc = match account(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let d = load(&state, &acc).await;
    let mut pending: Vec<&Task> = d.tasks.iter().filter(|t| !t.done).collect();
    pending.sort_by(|a, b| match (&a.due, &b.due) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    let mut done: Vec<&Task> = d.tasks.iter().filter(|t| t.done).collect();
    done.sort_by(|a, b| b.done_on.cmp(&a.done_on));
    Json(json!({
        "pendientes": pending.iter().map(|t| task_json(t)).collect::<Vec<_>>(),
        "hechas": done.iter().take(30).map(|t| task_json(t)).collect::<Vec<_>>(),
    }))
    .into_response()
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Range {
    desde: Option<String>,
    dias: Option<i64>,
}

/// Los eventos y las tareas con fecha de unos días (por omisión, desde hoy y 14 días).
pub async fn agenda(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<Range>) -> Response {
    let acc = match account(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let from = day_or_today(q.desde.as_deref());
    let to = shift(&from, q.dias.unwrap_or(14).clamp(1, 62) - 1);
    let d = load(&state, &acc).await;
    let mut events: Vec<&Event> = d
        .events
        .iter()
        .filter(|e| e.date >= from && e.date <= to)
        .filter(|e| d.marks.get(&event_mark_key(e)).is_none_or(|m| m != "tarea" && m != "quitado"))
        .collect();
    events.sort_by(|a, b| (&a.date, &a.time).cmp(&(&b.date, &b.time)));
    let tasks: Vec<&Task> = d.tasks.iter().filter(|t| !t.done && t.due.as_deref().is_some_and(|f| f >= from.as_str() && f <= to.as_str())).collect();
    Json(json!({
        "desde": from,
        "hasta": to,
        "eventos": events.iter().map(|e| event_json(e, d.marks.get(&event_mark_key(e)).is_some_and(|m| m == "hecho"))).collect::<Vec<_>>(),
        "tareas": tasks.iter().map(|t| task_json(t)).collect::<Vec<_>>(),
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct Note {
    #[serde(default)]
    hoy: Option<String>,
    texto: String,
}

/// Anota en la nota del día (una línea por renglón).
pub async fn jot(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(r): Json<Note>) -> Response {
    let acc = match account(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let today = day_or_today(r.hoy.as_deref());
    let lines: Vec<String> = r.texto.lines().map(|l| l.trim_end().to_string()).filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return error(StatusCode::BAD_REQUEST, "No hay nada que anotar.");
    }
    match sync::modify(&state, &acc, &format!("{DIARY}/{today}.md"), |old| Some(append(old, &lines))).await {
        Ok(_) => Json(json!({ "ok": true })).into_response(),
        Err(e) => failed(e),
    }
}

/// El espacio de lo que no tiene uno: General, o el primero (como en la app).
fn home_space(files: &[Entry]) -> String {
    let spaces: BTreeSet<&str> = files
        .iter()
        .filter(|e| e.ruta.ends_with(".md"))
        .filter_map(|e| e.ruta.split_once('/').map(|(w, _)| w))
        .filter(|w| !w.starts_with('.') && !w.eq_ignore_ascii_case(DIARY) && !NOT_NOTES.iter().any(|n| w.eq_ignore_ascii_case(n)))
        .collect();
    if spaces.contains("General") || spaces.is_empty() { "General".into() } else { spaces.iter().next().map(|s| s.to_string()).unwrap_or_default() }
}

#[derive(Deserialize)]
pub struct NewTask {
    #[serde(default)]
    hoy: Option<String>,
    texto: String,
    /// La fecha elegida (si no, la que diga el texto: «el viernes»).
    #[serde(default)]
    fecha: Option<String>,
}

/// Una tarea nueva: queda en la nota del día («- [ ] … ^id») y en tareas.txt, unidas (como las
/// que se escriben en la app).
pub async fn add_task(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(r): Json<NewTask>) -> Response {
    let acc = match account(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let today = day_or_today(r.hoy.as_deref());
    let Ok(day) = NaiveDate::parse_from_str(&today, "%Y-%m-%d") else { return error(StatusCode::BAD_REQUEST, "Fecha inválida.") };
    let one_line = r.texto.split_whitespace().collect::<Vec<_>>().join(" ");
    let (text, said) = fmt::parse_when(&one_line, day);
    let due = r.fecha.filter(|f| fmt::is_date(f)).or(said);
    if text.trim().is_empty() {
        return error(StatusCode::BAD_REQUEST, "Escribe la tarea.");
    }
    let id = fmt::new_id(now_ms() as u64 ^ fmt::fold(&text).len() as u64);
    let note = format!("{DIARY}/{today}");
    let space = home_space(&sync::files(&state, &acc).await);
    let line = match &due {
        Some(d) => format!("- [ ] {text} due:{d} ^{id}"),
        None => format!("- [ ] {text} ^{id}"),
    };
    if let Err(e) = sync::modify(&state, &acc, &format!("{note}.md"), |old| Some(append(old, &[line]))).await {
        return failed(e);
    }
    let task = fmt::format_task(&today, &text, &space, due.as_deref(), &note, Some(&id));
    match sync::modify(&state, &acc, fmt::TASKS_FILE, |old| Some(append(old, std::slice::from_ref(&task)))).await {
        Ok(_) => Json(json!({ "ok": true, "tarea": fmt::parse_task(&task).map(|t| task_json(&t)) })).into_response(),
        Err(e) => failed(e),
    }
}

#[derive(Deserialize)]
pub struct Done {
    #[serde(default)]
    hoy: Option<String>,
    /// La línea de la tarea en tareas.txt (como vino).
    clave: String,
    hecha: bool,
}

/// Marca una tarea hecha o pendiente: en tareas.txt y en su línea de la nota.
pub async fn task_done(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(r): Json<Done>) -> Response {
    let acc = match account(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let today = day_or_today(r.hoy.as_deref());
    let Some(task) = fmt::parse_task(&r.clave) else { return error(StatusCode::BAD_REQUEST, "Tarea inválida.") };
    // Se busca por su identificador (si cambió desde que se leyó) o por la línea tal cual.
    let same = |l: &str| l == r.clave || (task.id.is_some() && fmt::parse_task(l).is_some_and(|t| t.id == task.id));
    let mut found = false;
    let res = sync::modify(&state, &acc, fmt::TASKS_FILE, |old| {
        let old = old?;
        let mut out = String::with_capacity(old.len() + 16);
        for l in old.lines() {
            if !found && same(l) {
                found = true;
                out += &fmt::set_task_done(l, r.hecha, &today);
            } else {
                out += l;
            }
            out.push('\n');
        }
        Some(out)
    })
    .await;
    if let Err(e) = res {
        return failed(e);
    }
    if !found {
        return error(StatusCode::NOT_FOUND, "Esa tarea ya no está (quizás cambió en otro equipo): actualiza.");
    }
    if let (Some(id), Some(note)) = (&task.id, &task.note) {
        let mark = format!("^{id}");
        let _ = sync::modify(&state, &acc, &format!("{note}.md"), |old| {
            let old = old?;
            let mut changed = false;
            let lines: Vec<String> = old
                .split('\n')
                .map(|l| match (changed, l.split_whitespace().any(|w| w == mark)) {
                    (false, true) => match fmt::set_box(l, r.hecha) {
                        Some(n) => {
                            changed = true;
                            n
                        }
                        None => l.to_string(),
                    },
                    _ => l.to_string(),
                })
                .collect();
            changed.then(|| lines.join("\n"))
        })
        .await;
    }
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
pub struct HabitMark {
    id: String,
    dia: String,
}

/// Marca o desmarca un hábito en un día (agrega una línea, como la app).
pub async fn habit(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(r): Json<HabitMark>) -> Response {
    let acc = match account(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !fmt::is_date(&r.dia) || r.id.contains(['\t', '\n']) {
        return error(StatusCode::BAD_REQUEST, "Día inválido.");
    }
    let path = format!(".nodex/{}", habitos::FILE);
    let mut now = None;
    let res = sync::modify(&state, &acc, &path, |old| {
        let hs = habitos::parse(old.unwrap_or(""));
        let h = hs.iter().find(|h| h.id == r.id)?;
        let on = !h.is_done(&r.dia);
        now = Some(on);
        Some(append(old, &[format!("{}\t{}\t{}\t{}", now_ms(), if on { "si" } else { "no" }, r.id, r.dia)]))
    })
    .await;
    match (res, now) {
        (Err(e), _) => failed(e),
        (_, None) => error(StatusCode::NOT_FOUND, "Ese hábito ya no está."),
        (Ok(_), Some(on)) => Json(json!({ "ok": true, "hecho": on })).into_response(),
    }
}

#[derive(Deserialize)]
pub struct Decide {
    clave: String,
    agendar: bool,
}

/// Agendar o descartar algo que llegó por correo.
pub async fn decide(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(r): Json<Decide>) -> Response {
    let acc = match account(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let d = load(&state, &acc).await;
    let Some((key, line)) = d.proposals.iter().find(|(k, _)| *k == r.clave).cloned() else {
        return error(StatusCode::NOT_FOUND, "Eso ya se decidió (quizás en otro equipo).");
    };
    // Las otras formas del mismo evento se descartan (si no, quedaría dos veces).
    let event = fmt::parse_event(&line);
    let mut marks = vec![format!("{}\t{key}\t{line}", if r.agendar { "si" } else { "no" })];
    for (k, l) in d.proposals.iter().filter(|(k, _)| *k != key) {
        if let (Some(a), Some(b)) = (&event, fmt::parse_event(l)) {
            if fmt::same_event(a, &b) {
                marks.push(format!("no\t{k}\t{l}"));
            }
        }
    }
    if let Err(e) = sync::modify(&state, &acc, PROPOSALS, |old| Some(append(old, &marks))).await {
        return failed(e);
    }
    if r.agendar {
        let res = sync::modify(&state, &acc, fmt::AGENDA_FILE, |old| {
            let has = old.unwrap_or("").lines().filter_map(fmt::parse_event).any(|e| proposal_key(&e) == key || event.as_ref().is_some_and(|x| fmt::same_event(x, &e)));
            (!has).then(|| append(old, std::slice::from_ref(&line)))
        })
        .await;
        if let Err(e) = res {
            return failed(e);
        }
    }
    Json(json!({ "ok": true })).into_response()
}

/// Las notas que se pueden leer (sin plantillas, archivadas ni datos internos), la más reciente primero.
fn note_entries(files: Vec<Entry>) -> Vec<Entry> {
    let mut v: Vec<Entry> = files
        .into_iter()
        .filter(|e| e.ruta.ends_with(".md"))
        .filter(|e| !e.ruta.split('/').any(|p| p.starts_with('.')))
        .filter(|e| e.ruta.split_once('/').is_none_or(|(w, _)| !NOT_NOTES.iter().any(|n| w.eq_ignore_ascii_case(n))))
        .collect();
    v.sort_by(|a, b| b.seq.cmp(&a.seq));
    v
}

fn title_of(ruta: &str) -> (String, String) {
    let stem = ruta.strip_suffix(".md").unwrap_or(ruta);
    match stem.rsplit_once('/') {
        Some((ws, t)) => (t.to_string(), ws.to_string()),
        None => (stem.to_string(), String::new()),
    }
}

/// Las primeras palabras de una nota, sin marcas.
fn preview(text: &str) -> String {
    let mut out = String::new();
    for l in text.lines() {
        let t = l.trim().trim_start_matches(['#', '-', '*', '>', ' ']).trim_start_matches("[ ] ").trim_start_matches("[x] ");
        if t.is_empty() || t.starts_with("![") {
            continue;
        }
        // Sin los identificadores ni las fechas escritas para la app.
        let t: Vec<&str> = t.split_whitespace().filter(|w| !(w.starts_with('^') && w.len() > 3) && !w.starts_with("due:")).collect();
        if t.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str(" · ");
        }
        out += &t.join(" ");
        if out.chars().count() > 140 {
            break;
        }
    }
    out.chars().take(140).collect()
}

/// Palabras que valen para buscar (sin las cortas).
fn words(q: &str) -> Vec<String> {
    fold(q).split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 3).map(str::to_string).collect()
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Search {
    q: String,
}

/// Buscar notas (por el título en todas; por dentro, en las más recientes). Sin `q`, las recientes.
pub async fn notes(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<Search>) -> Response {
    let acc = match account(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let all = note_entries(sync::files(&state, &acc).await);
    let want = words(&q.q);
    let mut hits: Vec<(usize, &Entry, Option<String>)> = Vec::new();
    for (i, e) in all.iter().enumerate() {
        if want.is_empty() {
            hits.push((0, e, None));
        } else {
            let title = fold(&e.ruta);
            let mut score = want.iter().filter(|w| title.contains(w.as_str())).count() * 3;
            let mut text = None;
            if i < SEARCH_INSIDE {
                if let Some(t) = sync::blob_text(&state, &acc, &e.hash) {
                    let f = fold(&t);
                    score += want.iter().filter(|w| f.contains(w.as_str())).count();
                    text = Some(t);
                }
            }
            if score > 0 {
                hits.push((score, e, text));
            }
        }
        if want.is_empty() && hits.len() >= 60 {
            break;
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.seq.cmp(&a.1.seq)));
    let out: Vec<Value> = hits
        .into_iter()
        .take(60)
        .map(|(_, e, text)| {
            let (title, ws) = title_of(&e.ruta);
            let text = text.or_else(|| sync::blob_text(&state, &acc, &e.hash)).unwrap_or_default();
            json!({ "ruta": e.ruta, "titulo": title, "espacio": ws, "vista": preview(&text) })
        })
        .collect();
    Json(json!({ "notas": out, "total": all.len() })).into_response()
}

#[derive(Deserialize)]
pub struct NotePath {
    ruta: String,
}

/// Una nota, para leerla.
pub async fn note(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<NotePath>) -> Response {
    let acc = match account(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !q.ruta.ends_with(".md") || !sync::valid_path(&q.ruta) {
        return error(StatusCode::BAD_REQUEST, "Nota inválida.");
    }
    match sync::read_text(&state, &acc, &q.ruta).await {
        Some((text, seq)) => {
            let (title, ws) = title_of(&q.ruta);
            Json(json!({ "ruta": q.ruta, "titulo": title, "espacio": ws, "texto": text, "version": seq })).into_response()
        }
        None => error(StatusCode::NOT_FOUND, "Esa nota ya no está."),
    }
}

#[derive(Deserialize)]
pub struct NoteLine {
    ruta: String,
    texto: String,
}

/// Agrega líneas al final de una nota.
pub async fn note_line(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(r): Json<NoteLine>) -> Response {
    let acc = match account(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !r.ruta.ends_with(".md") || !sync::valid_path(&r.ruta) {
        return error(StatusCode::BAD_REQUEST, "Nota inválida.");
    }
    let lines: Vec<String> = r.texto.lines().map(|l| l.trim_end().to_string()).filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return error(StatusCode::BAD_REQUEST, "No hay nada que agregar.");
    }
    match sync::modify(&state, &acc, &r.ruta, |old| old.map(|o| append(Some(o), &lines))).await {
        Ok(Some(_)) => Json(json!({ "ok": true })).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "Esa nota ya no está."),
        Err(e) => failed(e),
    }
}

const WEEKDAYS: [&str; 7] = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];

/// Lo que sabe la IA para responder desde el celular: lo de hoy y las notas que vienen al caso.
async fn ask_context(state: &AppState, acc: &str, today: &str, question: &str) -> String {
    let d = load(state, acc).await;
    let weekday = NaiveDate::parse_from_str(today, "%Y-%m-%d").map(|x| WEEKDAYS[x.weekday().num_days_from_monday() as usize]).unwrap_or("");
    let mut out = format!("Hoy es {weekday} {today}.\n\n## Tareas pendientes\n");
    let mut pending: Vec<&Task> = d.tasks.iter().filter(|t| !t.done).collect();
    pending.sort_by(|a, b| a.due.cmp(&b.due));
    for t in pending.iter().take(120) {
        out += &format!("- {}{}{}\n", fmt::display_text(&t.text), t.due.as_ref().map(|f| format!(" (para el {f})")).unwrap_or_default(), if t.project.is_empty() { String::new() } else { format!(" [{}]", t.project) });
    }
    out += "\n## Agenda (próximas dos semanas)\n";
    let until = shift(today, 14);
    for e in d.events.iter().filter(|e| e.date.as_str() >= today && e.date <= until) {
        out += &format!("- {} {}{}\n", e.date, e.time.as_ref().map(|t| format!("{t} ")).unwrap_or_default(), e.title);
    }
    if !d.habits.is_empty() {
        out += "\n## Hábitos\n";
        for h in &d.habits {
            out += &format!("- {}: {} hoy; racha {}\n", h.name, if h.is_done(today) { "hecho" } else { "no hecho" }, h.streak(today));
        }
    }
    // Las notas: la de hoy, y las que más tienen las palabras de la pregunta.
    let all = note_entries(sync::files(state, acc).await);
    let want = words(question);
    let mut scored: Vec<(usize, &Entry, String)> = Vec::new();
    for (i, e) in all.iter().enumerate() {
        let is_today = e.ruta == format!("{DIARY}/{today}.md");
        if !is_today && (want.is_empty() || i >= SEARCH_INSIDE) {
            continue;
        }
        let Some(t) = sync::blob_text(state, acc, &e.hash) else { continue };
        let f = fold(&t) + " " + &fold(&e.ruta);
        let score = want.iter().filter(|w| f.contains(w.as_str())).count() + if is_today { 100 } else { 0 };
        if score > 0 {
            scored.push((score, e, t));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.seq.cmp(&a.1.seq)));
    let mut used = 0;
    for (_, e, t) in scored.into_iter().take(12) {
        if used > ASK_NOTES {
            break;
        }
        let t: String = t.chars().take(8000).collect();
        used += t.len();
        out += &format!("\n## Nota «{}»\n{t}\n", e.ruta.trim_end_matches(".md"));
    }
    out
}

#[derive(Deserialize)]
pub struct Ask {
    #[serde(default)]
    hoy: Option<String>,
    pregunta: String,
    /// Lo conversado antes: [{"role": "user"|"assistant", "content": "…"}].
    #[serde(default)]
    historia: Vec<Value>,
    #[serde(default)]
    idioma: String,
}

/// Preguntarle a la IA desde el celular (con el mismo límite de uso de la cuenta).
pub async fn ask(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(r): Json<Ask>) -> Response {
    let acc = match account(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let today = day_or_today(r.hoy.as_deref());
    let context = ask_context(&state, &acc, &today, &r.pregunta).await;
    let lang = if r.idioma == "en" { "Answer in English." } else { "Responde en español." };
    let system = format!(
        "Eres la IA de Notas, la app de notas de esta persona, y te pregunta desde su celular. Responde corto y directo (es una pantalla chica), con lo que dicen sus tareas, su agenda, sus hábitos y sus notas, que van abajo. Si no está ahí, dilo; no inventes. {lang}\n\n{context}"
    );
    let mut messages = vec![json!({ "role": "system", "content": system })];
    for m in r.historia.into_iter().rev().take(12).rev() {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("");
        if let (true, Some(c)) = (role == "user" || role == "assistant", m.get("content").and_then(Value::as_str)) {
            messages.push(json!({ "role": role, "content": c }));
        }
    }
    messages.push(json!({ "role": "user", "content": r.pregunta }));
    let res = crate::chat(State(state.clone()), headers, Json(json!({ "messages": messages }))).await;
    if !res.status().is_success() {
        return res;
    }
    let body = axum::body::to_bytes(res.into_body(), 4 * 1024 * 1024).await.unwrap_or_default();
    let v: Value = serde_json::from_slice(&body).unwrap_or_default();
    let answer = v.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if answer.is_empty() {
        return error(StatusCode::BAD_GATEWAY, "La IA no respondió; inténtalo de nuevo.");
    }
    Json(json!({ "respuesta": answer })).into_response()
}

// La app web, guardada dentro del programa.

const INDEX: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const APP_CSS: &str = include_str!("../web/app.css");
const MANIFEST: &str = include_str!("../web/manifest.webmanifest");
const SW: &str = include_str!("../web/sw.js");
const ICON_192: &[u8] = include_bytes!("../web/icon-192.png");
const ICON_512: &[u8] = include_bytes!("../web/icon-512.png");

/// Los archivos de la app web (`/app/…`).
pub async fn web(axum::extract::Path(file): axum::extract::Path<String>) -> Response {
    let (kind, body): (&str, &[u8]) = match file.as_str() {
        "" | "index.html" => ("text/html; charset=utf-8", INDEX.as_bytes()),
        "app.js" => ("text/javascript; charset=utf-8", APP_JS.as_bytes()),
        "app.css" => ("text/css; charset=utf-8", APP_CSS.as_bytes()),
        "manifest.webmanifest" => ("application/manifest+json", MANIFEST.as_bytes()),
        "sw.js" => ("text/javascript; charset=utf-8", SW.as_bytes()),
        "icon-192.png" => ("image/png", ICON_192),
        "icon-512.png" => ("image/png", ICON_512),
        _ => return error(StatusCode::NOT_FOUND, "No existe."),
    };
    // La página y el código se revisan siempre (para que las actualizaciones lleguen); los íconos se guardan.
    let cache = if kind.starts_with("image/") { "public, max-age=604800" } else { "no-cache" };
    ([("content-type", kind), ("cache-control", cache)], body).into_response()
}

pub async fn web_index() -> Response {
    web(axum::extract::Path(String::new())).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{Account, Accounts, SignedIn};
    use crate::{Settings, router, state};
    use axum::Router;
    use axum::routing::post;

    /// El celular sobre los archivos de la cuenta: lee lo de hoy, anota, agrega y marca tareas
    /// (en tareas.txt y en su línea), marca hábitos, agenda lo del correo, busca y lee notas, y
    /// le pregunta a la IA. Lo que escribe queda en el formato de la app.
    #[tokio::test]
    async fn the_phone_works_on_the_synced_files() {
        let mails: Arc<tokio::sync::Mutex<Vec<Value>>> = Arc::default();
        let asked: Arc<tokio::sync::Mutex<Vec<Value>>> = Arc::default();
        let (m2, a2) = (mails.clone(), asked.clone());
        let fake = Router::new()
            .route("/correo", post(move |Json(v): Json<Value>| {
                let m = m2.clone();
                async move {
                    m.lock().await.push(v);
                    Json(json!({"id": "1"}))
                }
            }))
            .route("/v1/chat/completions", post(move |Json(v): Json<Value>| {
                let a = a2.clone();
                async move {
                    a.lock().await.push(v);
                    Json(json!({"choices": [{"index": 0, "message": {"role": "assistant", "content": "Mañana tienes la visita a obra."}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 5, "completion_tokens": 5}}))
                }
            }));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fake_url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, fake).await.unwrap() });
        let data = std::env::temp_dir().join(format!("nodex-ia-movil-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let settings = Settings {
            upstream: format!("{fake_url}/v1/"),
            key: "k".into(),
            data: data.clone(),
            mail_api: format!("{fake_url}/correo"),
            mail_key: "clave-correo".into(),
            mail_from: "Notas <hola@notas.cl>".into(),
            ..Default::default()
        };
        let mut seed = Accounts::default();
        seed.cuentas.insert("j1".into(), Account { id: "j1".into(), correo: "juan@obra.cl".into(), plan: "prueba".into(), estado: "pendiente".into(), ..Default::default() });
        seed.approve("juan@obra.cl", 14).unwrap();
        seed.save(&data).unwrap();
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, router(state(settings))).await.unwrap() });
        let http = reqwest::Client::new();
        http.post(format!("{base}/v1/cuenta/codigo")).json(&json!({"correo": "juan@obra.cl"})).send().await.unwrap();
        let text = mails.lock().await[0]["text"].as_str().unwrap().to_string();
        let code: String = text.chars().filter(|c| c.is_ascii_digit()).take(6).collect();
        let ok: SignedIn = http.post(format!("{base}/v1/cuenta/entrar")).json(&json!({"correo": "juan@obra.cl", "codigo": code})).send().await.unwrap().json().await.unwrap();
        let tok = ok.token.clone();

        // Lo que ya subió un equipo.
        let today = "2026-10-12"; // lunes
        let put = |ruta: &str, body: &str| http.put(format!("{base}/v1/sync/archivo?ruta={ruta}&base=0")).bearer_auth(&tok).body(body.to_string()).send();
        put("tareas.txt", "2026-10-10 Enviar planos +Obra due:2026-10-12 nota:Obra/Muro id:pl001\n2026-10-01 Pagar cuentas +General\n").await.unwrap();
        put("Obra/Muro.md", "Revisar el muro del eje 3\n- [ ] Enviar planos due:2026-10-12 ^pl001\n").await.unwrap();
        put("General/Ideas.md", "Comprar un taladro\n").await.unwrap();
        put("agenda.txt", "2026-10-13 09:00 Visita a obra +Obra nota:Obra/Muro\n2026-10-12 Clase IOCC +General nota:General/Ideas\n").await.unwrap();
        put(".nodex/eventos.txt", "quitado\tevento:2026-10-12:clase iocc\n").await.unwrap();
        put(".nodex/habitos.txt", "1000\thabito\th1\tLeer\n1001\thabito\th2\tGimnasio\n1002\tfrecuencia\th2\tsemana:2\n2000\tsi\th1\t2026-10-11\n").await.unwrap();
        put(".nodex/por-agendar.txt", "pendiente\t2026-10-15|10:00|webinar glaciar\t2026-10-15 10:00 Webinar Glaciar +General nota:Diario/2026-10-12\n").await.unwrap();
        let get = |path: &str| {
            let r = http.get(format!("{base}/v1/movil/{path}")).bearer_auth(&tok).send();
            async move { r.await.unwrap().json::<Value>().await.unwrap() }
        };
        let post = |path: &str, body: Value| {
            let r = http.post(format!("{base}/v1/movil/{path}")).bearer_auth(&tok).json(&body).send();
            let path = path.to_string();
            async move {
                let r = r.await.unwrap();
                assert!(r.status().is_success(), "{path}: {}", r.status());
                r.json::<Value>().await.unwrap()
            }
        };
        let file = |ruta: &'static str| {
            let r = http.get(format!("{base}/v1/sync/cambios")).bearer_auth(&tok).send();
            let tok = tok.clone();
            let (http, base) = (http.clone(), base.clone());
            async move {
                let c: Value = r.await.unwrap().json().await.unwrap();
                let hash = c["cambios"].as_array().unwrap().iter().find(|e| e["ruta"] == ruta).map(|e| e["hash"].as_str().unwrap().to_string()).unwrap();
                http.get(format!("{base}/v1/sync/blob/{hash}")).bearer_auth(&tok).send().await.unwrap().text().await.unwrap()
            }
        };

        // Lo de hoy.
        let hoy = get(&format!("hoy?hoy={today}")).await;
        assert_eq!(hoy["tareas"][0]["texto"], "Enviar planos");
        assert_eq!(hoy["sin_fecha"], 1);
        let eventos: Vec<&str> = hoy["eventos"].as_array().unwrap().iter().map(|e| e["titulo"].as_str().unwrap()).collect();
        assert_eq!(eventos, vec!["Visita a obra"], "la clase quitada no aparece");
        assert_eq!(hoy["por_agendar"][0]["evento"]["titulo"], "Webinar Glaciar");
        assert_eq!(hoy["habitos"].as_array().unwrap().len(), 2);
        assert_eq!(hoy["habitos"][0]["racha"], 1, "Leer ayer: la racha sigue");
        assert_eq!(hoy["meta"]["total"], 1);
        let anon = http.get(format!("{base}/v1/movil/hoy")).send().await.unwrap();
        assert_eq!(anon.status(), 401);

        // Anotar y agregar una tarea («el viernes» es su fecha).
        post("anotar", json!({"hoy": today, "texto": "Llamar al inspector\n\nPedir el libro de obra"})).await;
        let added = post("tarea", json!({"hoy": today, "texto": "Revisar cubicación el viernes"})).await;
        assert_eq!(added["tarea"]["fecha"], "2026-10-16");
        let diary = file("Diario/2026-10-12.md").await;
        let lines: Vec<&str> = diary.lines().collect();
        assert_eq!(&lines[..2], &["Llamar al inspector", "Pedir el libro de obra"]);
        let id = lines[2].rsplit('^').next().unwrap();
        assert_eq!(lines[2], format!("- [ ] Revisar cubicación due:2026-10-16 ^{id}"));
        let tasks = file("tareas.txt").await;
        let t = tasks.lines().filter_map(fmt::parse_task).find(|t| t.text == "Revisar cubicación").unwrap();
        assert_eq!((t.id.as_deref(), t.note.as_deref(), t.project.as_str(), t.due.as_deref()), (Some(id), Some("Diario/2026-10-12"), "General", Some("2026-10-16")));

        // Marcar hecha: en tareas.txt y en la línea de la nota; y volver a pendiente.
        let clave = hoy["tareas"][0]["clave"].as_str().unwrap().to_string();
        post("tarea/hecha", json!({"hoy": today, "clave": clave, "hecha": true})).await;
        assert!(file("tareas.txt").await.contains("x 2026-10-12 2026-10-10 Enviar planos"));
        assert!(file("Obra/Muro.md").await.contains("- [x] Enviar planos due:2026-10-12 ^pl001"));
        let done = get("tareas").await["hechas"][0]["clave"].as_str().unwrap().to_string();
        post("tarea/hecha", json!({"hoy": today, "clave": done, "hecha": false})).await;
        assert!(file("tareas.txt").await.starts_with("2026-10-10 Enviar planos"));
        assert!(file("Obra/Muro.md").await.contains("- [ ] Enviar planos"));
        let gone = http.post(format!("{base}/v1/movil/tarea/hecha")).bearer_auth(&tok).json(&json!({"clave": "2026-01-01 No existe", "hecha": true})).send().await.unwrap();
        assert_eq!(gone.status(), 404);

        // Hábitos: marcar Leer hoy (y que se lea igual que en la app).
        assert_eq!(post("habito", json!({"id": "h1", "dia": today})).await["hecho"], true);
        let hs = habitos::parse(&file(".nodex/habitos.txt").await);
        assert!(hs[0].is_done(today) && hs[0].streak(today) == 2);
        assert_eq!(get(&format!("hoy?hoy={today}")).await["meta"]["cumplida"], true);

        // Agendar lo del correo.
        post("por-agendar", json!({"clave": "2026-10-15|10:00|webinar glaciar", "agendar": true})).await;
        assert!(file("agenda.txt").await.contains("2026-10-15 10:00 Webinar Glaciar"));
        assert!(file(".nodex/por-agendar.txt").await.ends_with("si\t2026-10-15|10:00|webinar glaciar\t2026-10-15 10:00 Webinar Glaciar +General nota:Diario/2026-10-12\n"));
        assert!(get(&format!("hoy?hoy={today}")).await["por_agendar"].as_array().unwrap().is_empty());
        let agenda = get("agenda?desde=2026-10-12&dias=7").await;
        assert_eq!(agenda["eventos"].as_array().unwrap().len(), 2, "visita y webinar");

        // Notas: buscar (por dentro), leer y agregar una línea.
        let found = get("notas?q=taladro").await;
        assert_eq!(found["notas"][0]["ruta"], "General/Ideas.md");
        assert_eq!(found["notas"][0]["vista"], "Comprar un taladro");
        assert_eq!(get("notas").await["total"], 3);
        let n = get("nota?ruta=Obra/Muro.md").await;
        assert_eq!(n["titulo"], "Muro");
        post("nota/linea", json!({"ruta": "General/Ideas.md", "texto": "Y brocas"})).await;
        assert_eq!(file("General/Ideas.md").await, "Comprar un taladro\nY brocas\n");

        // Preguntar: la IA recibe lo de hoy y la nota que viene al caso.
        let r = post("preguntar", json!({"hoy": today, "pregunta": "¿Qué dice la nota del muro?", "historia": [{"role": "user", "content": "hola"}, {"role": "system", "content": "no"}]})).await;
        assert_eq!(r["respuesta"], "Mañana tienes la visita a obra.");
        let sent = asked.lock().await[0].clone();
        let msgs = sent["messages"].as_array().unwrap();
        let system = msgs[0]["content"].as_str().unwrap();
        assert!(system.contains("Hoy es lunes 2026-10-12") && system.contains("Enviar planos") && system.contains("Nota «Obra/Muro»"), "{system}");
        assert_eq!(msgs.len(), 3, "solo lo de la persona y la IA, más la pregunta");

        // La app web.
        let page = http.get(format!("{base}/app/")).send().await.unwrap();
        assert_eq!(page.headers()["content-type"], "text/html; charset=utf-8");
        assert!(page.text().await.unwrap().contains("manifest.webmanifest"));
        let redirect = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap().get(format!("{base}/app")).send().await.unwrap();
        assert_eq!(redirect.headers()["location"], "/app/");
        assert_eq!(http.get(format!("{base}/app/icon-192.png")).send().await.unwrap().bytes().await.unwrap()[..4], b"\x89PNG"[..]);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn pieces() {
        assert_eq!(append(None, &["a".into()]), "a\n");
        assert_eq!(append(Some("x"), &["a".into(), "b".into()]), "x\na\nb\n");
        assert_eq!(title_of("Obra Talca/Muro.md"), ("Muro".into(), "Obra Talca".into()));
        assert_eq!(preview("# Título\n\n- [ ] Enviar planos due:2026-10-12 ^pl001\n![x](a.png)\nMás texto"), "Título · Enviar planos · Más texto");
        assert_eq!(words("¿Qué dijo Pérez del muro?"), vec!["que", "dijo", "perez", "del", "muro"]);
        let files = |r: &[&str]| r.iter().map(|r| Entry { ruta: r.to_string(), ..Entry::default() }).collect::<Vec<_>>();
        assert_eq!(home_space(&files(&["Obra/Muro.md", "General/x.md", "Diario/2026-10-13.md"])), "General");
        assert_eq!(home_space(&files(&["Obra/Muro.md", "Diario/2026-10-13.md", "Plantillas/a.md"])), "Obra");
        let notes = note_entries(files(&["Obra/Muro.md", "Plantillas/a.md", ".nodex/x.md", "Archivo/Obra/v.md", "tareas.txt", "Diario/2026-10-13.md"]));
        assert_eq!(notes.iter().map(|e| e.ruta.as_str()).collect::<Vec<_>>(), vec!["Obra/Muro.md", "Diario/2026-10-13.md"]);
    }
}
