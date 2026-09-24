//! Sincronización de la agenda con Google Calendar, en un hilo aparte.
//!
//! - Conexión OAuth 2.0 de escritorio: se abre el navegador, Google redirige a un
//!   servidor local (127.0.0.1) y el permiso queda guardado en este equipo
//!   (`google_token.json`, junto a config.toml; nunca en la carpeta de Dropbox).
//! - La app crea su propio calendario "Notas" y solo toca ese
//!   (permiso `calendar.app.created`: no ve ni cambia tus otros calendarios).
//! - Sincronización en un sentido: eventos de agenda.txt y tareas pendientes con fecha.
//!   `.nodex/google.json` recuerda qué evento de Google corresponde a cada elemento.

use crate::agenda::{self, Agenda};
use base64::Engine;
use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

const SCOPE: &str = "https://www.googleapis.com/auth/calendar.app.created";
const CALENDAR_NAME: &str = "Notas";
/// Tiempo máximo para dar el permiso en el navegador.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(300);

/// Un elemento de la agenda tal como irá a Google.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Identifica el elemento; si cambia algo (fecha, título…) es otro elemento.
    pub key: String,
    pub date: String,
    pub time: Option<String>,
    pub title: String,
    pub description: String,
}

/// Lo que debería estar en Google: eventos y tareas pendientes con fecha.
pub fn desired_items(agenda: &Agenda) -> Vec<Item> {
    let mut items = Vec::new();
    for e in agenda.events() {
        let note = e.note.clone().unwrap_or_default();
        let key = format!("e{:016x}", crate::ai::fnv(&format!("{}|{:?}|{}|{}|{note}", e.date, e.time, e.title, e.project)));
        let description = description(&e.project, &note);
        items.push(Item { key, date: e.date, time: e.time, title: e.title, description });
    }
    for t in agenda.tasks().into_iter().filter(|t| !t.done) {
        let Some(due) = t.due.filter(|d| agenda::is_date(d)) else { continue };
        let note = t.note.clone().unwrap_or_default();
        let key = format!("t{:016x}", crate::ai::fnv(&format!("{due}|{}|{}|{note}", t.text, t.project)));
        let description = description(&t.project, &note);
        items.push(Item { key, date: due, time: None, title: format!("☐ {}", t.text), description });
    }
    items
}

fn description(project: &str, note: &str) -> String {
    let mut d = String::new();
    if !project.is_empty() {
        d += &format!("Espacio: {project}\n");
    }
    if !note.is_empty() {
        d += &format!("Nota: {note}\n");
    }
    d + "Creado por la app Notas."
}

/// Cuerpo JSON del evento para la API de Google Calendar.
pub fn event_body(it: &Item) -> Option<Value> {
    let (start, end) = match &it.time {
        Some(t) => {
            let ndt = NaiveDateTime::parse_from_str(&format!("{} {t}", it.date), "%Y-%m-%d %H:%M").ok()?;
            let start = Local.from_local_datetime(&ndt).earliest()?;
            let end = start + chrono::Duration::hours(1);
            (json!({ "dateTime": start.to_rfc3339() }), json!({ "dateTime": end.to_rfc3339() }))
        }
        None => {
            let d = NaiveDate::parse_from_str(&it.date, "%Y-%m-%d").ok()?;
            let next = d.succ_opt()?;
            (json!({ "date": d.to_string() }), json!({ "date": next.to_string() }))
        }
    };
    Some(json!({
        "summary": it.title,
        "description": it.description,
        "start": start,
        "end": end,
        "extendedProperties": { "private": { "nodex": it.key } },
    }))
}

// ---------- Utilidades de URL ----------

/// Codifica para URL todo lo que no sea letra, número o `-_.~`.
pub fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn dec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parámetros de "GET /?code=...&state=... HTTP/1.1".
pub fn query_params(request_line: &str) -> BTreeMap<String, String> {
    let path = request_line.split_whitespace().nth(1).unwrap_or("");
    let query = path.split_once('?').map_or("", |(_, q)| q);
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (dec(k), dec(v)))
        .collect()
}

fn random_token(bytes: usize) -> Result<String, String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| e.to_string())?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

fn pkce_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

// ---------- Estado guardado ----------

fn token_path() -> PathBuf {
    crate::config::config_path().with_file_name("google_token.json")
}

#[derive(Default, Serialize, Deserialize)]
struct Token {
    refresh_token: String,
}

#[derive(Default, Serialize, Deserialize)]
struct SyncState {
    calendar_id: String,
    /// clave del elemento -> id del evento en Google
    events: BTreeMap<String, String>,
}

pub fn is_connected() -> bool {
    token_path().is_file()
}

// ---------- Hilo de trabajo ----------

enum Job {
    Connect,
    Sync(Vec<Item>),
    Disconnect,
}

enum Reply {
    Connected(Result<(), String>),
    Synced(Result<(usize, usize), String>),
    Disconnected,
}

pub struct GCal {
    tx: Sender<Job>,
    rx: Receiver<Reply>,
    pub connected: bool,
    pub busy: bool,
    pub connecting: bool,
    pub last_sync: Option<chrono::DateTime<Local>>,
    pub last_error: Option<String>,
}

struct Endpoints {
    auth: String,
    token: String,
    api: String,
    /// Pruebas (variable NODEX_GOOGLE_API): no abre el navegador, imprime la URL.
    test: bool,
}

fn endpoints() -> Endpoints {
    match std::env::var("NODEX_GOOGLE_API") {
        Ok(base) if !base.is_empty() => {
            let base = base.trim_end_matches('/').to_string();
            Endpoints { auth: format!("{base}/auth"), token: format!("{base}/token"), api: base, test: true }
        }
        _ => Endpoints {
            auth: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token: "https://oauth2.googleapis.com/token".into(),
            api: "https://www.googleapis.com".into(),
            test: false,
        },
    }
}

impl GCal {
    /// `None` si no hay credenciales OAuth en config.toml.
    pub fn start(client_id: &str, client_secret: &str, root: PathBuf, ctx: eframe::egui::Context) -> Option<GCal> {
        if client_id.trim().is_empty() {
            return None;
        }
        let (tx, job_rx) = mpsc::channel::<Job>();
        let (reply_tx, rx) = mpsc::channel::<Reply>();
        let id = client_id.trim().to_string();
        let secret = client_secret.trim().to_string();
        std::thread::Builder::new()
            .name("google-calendar".into())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
                let mut w = Worker {
                    rt,
                    http: reqwest::Client::new(),
                    id,
                    secret,
                    ep: endpoints(),
                    state_path: root.join(".nodex").join("google.json"),
                    access: None,
                };
                for job in job_rx {
                    let reply = match job {
                        Job::Connect => Reply::Connected(w.connect()),
                        Job::Sync(items) => Reply::Synced(w.sync(&items)),
                        Job::Disconnect => {
                            let _ = std::fs::remove_file(token_path());
                            w.access = None;
                            Reply::Disconnected
                        }
                    };
                    let _ = reply_tx.send(reply);
                    ctx.request_repaint();
                }
            })
            .ok()?;
        Some(GCal {
            tx,
            rx,
            connected: is_connected(),
            busy: false,
            connecting: false,
            last_sync: None,
            last_error: None,
        })
    }

    pub fn connect(&mut self) {
        if !self.busy && self.tx.send(Job::Connect).is_ok() {
            self.busy = true;
            self.connecting = true;
        }
    }

    pub fn sync(&mut self, items: Vec<Item>) {
        if !self.busy && self.connected && self.tx.send(Job::Sync(items)).is_ok() {
            self.busy = true;
        }
    }

    pub fn disconnect(&mut self) {
        if self.tx.send(Job::Disconnect).is_ok() {
            self.busy = true;
        }
    }

    /// Procesa las respuestas del hilo; devuelve mensajes para mostrar.
    pub fn poll(&mut self) -> Vec<String> {
        let mut msgs = Vec::new();
        while let Ok(r) = self.rx.try_recv() {
            self.busy = false;
            match r {
                Reply::Connected(Ok(())) => {
                    self.connecting = false;
                    self.connected = true;
                    self.last_error = None;
                    msgs.push("Google Calendar conectado: se creó el calendario «Notas»".into());
                }
                Reply::Connected(Err(e)) => {
                    self.connecting = false;
                    self.last_error = Some(e.clone());
                    msgs.push(format!("Google Calendar: {e}"));
                }
                Reply::Synced(Ok((created, deleted))) => {
                    self.last_sync = Some(Local::now());
                    self.last_error = None;
                    if created + deleted > 0 {
                        msgs.push(format!("Google Calendar: {created} agregados, {deleted} quitados"));
                    }
                }
                Reply::Synced(Err(e)) => {
                    self.connected = is_connected();
                    self.last_error = Some(e.clone());
                    msgs.push(format!("Google Calendar: {e}"));
                }
                Reply::Disconnected => {
                    self.connected = false;
                    self.last_sync = None;
                    msgs.push("Google Calendar desconectado".into());
                }
            }
        }
        msgs
    }
}

struct Worker {
    rt: tokio::runtime::Runtime,
    http: reqwest::Client,
    id: String,
    secret: String,
    ep: Endpoints,
    state_path: PathBuf,
    access: Option<(String, Instant)>,
}

/// Error HTTP con su código, para distinguir "no existe" de otros fallos.
struct HttpError {
    status: u16,
    message: String,
}

impl Worker {
    fn send(&self, req: reqwest::RequestBuilder) -> Result<Value, HttpError> {
        self.rt.block_on(async {
            let resp = req.send().await.map_err(|e| HttpError { status: 0, message: format!("sin conexión ({e})") })?;
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            if (200..300).contains(&status) {
                Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
            } else {
                let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                let message = v["error_description"]
                    .as_str()
                    .or(v["error"]["message"].as_str())
                    .or(v["error"].as_str())
                    .unwrap_or(&text)
                    .chars()
                    .take(200)
                    .collect();
                Err(HttpError { status, message })
            }
        })
    }

    /// Flujo OAuth: navegador -> servidor local -> intercambio del código por un token.
    fn connect(&mut self) -> Result<(), String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let redirect = format!("http://127.0.0.1:{port}");
        let verifier = random_token(48)?;
        let state = random_token(18)?;
        let url = format!(
            "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={}&code_challenge_method=S256&access_type=offline&prompt=consent&state={}",
            self.ep.auth,
            enc(&self.id),
            enc(&redirect),
            enc(SCOPE),
            pkce_challenge(&verifier),
            state
        );
        if self.ep.test {
            eprintln!("AUTH_URL {url}");
        } else {
            open_browser(&url);
        }
        let code = wait_for_code(&listener, &state)?;

        let form = [
            ("code", code.as_str()),
            ("client_id", self.id.as_str()),
            ("client_secret", self.secret.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier.as_str()),
        ];
        let v = self.send(self.http.post(&self.ep.token).form(&form)).map_err(|e| e.message)?;
        let refresh = v["refresh_token"].as_str().ok_or("Google no entregó un token de acceso permanente")?;
        self.remember_access(&v);
        let token = serde_json::to_string(&Token { refresh_token: refresh.to_string() }).map_err(|e| e.to_string())?;
        if let Some(dir) = token_path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(token_path(), token).map_err(|e| e.to_string())?;
        // Crea (o encuentra) el calendario de inmediato para confirmar que todo funciona.
        let mut st = self.load_state();
        self.ensure_calendar(&mut st)?;
        self.save_state(&st);
        Ok(())
    }

    fn remember_access(&mut self, v: &Value) {
        if let Some(t) = v["access_token"].as_str() {
            let secs = v["expires_in"].as_u64().unwrap_or(3600).saturating_sub(60);
            self.access = Some((t.to_string(), Instant::now() + Duration::from_secs(secs)));
        }
    }

    fn access_token(&mut self) -> Result<String, String> {
        if let Some((t, exp)) = &self.access {
            if Instant::now() < *exp {
                return Ok(t.clone());
            }
        }
        let raw = std::fs::read_to_string(token_path()).map_err(|_| "no conectado".to_string())?;
        let tok: Token = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let form = [
            ("client_id", self.id.as_str()),
            ("client_secret", self.secret.as_str()),
            ("refresh_token", tok.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ];
        match self.send(self.http.post(&self.ep.token).form(&form)) {
            Ok(v) => {
                self.remember_access(&v);
                self.access.as_ref().map(|(t, _)| t.clone()).ok_or_else(|| "respuesta sin token".into())
            }
            Err(e) if e.status == 400 || e.status == 401 => {
                let _ = std::fs::remove_file(token_path());
                Err("Google retiró el permiso; vuelve a conectar desde Agenda".into())
            }
            Err(e) => Err(e.message),
        }
    }

    fn load_state(&self) -> SyncState {
        std::fs::read_to_string(&self.state_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save_state(&self, st: &SyncState) {
        if let Some(dir) = self.state_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string_pretty(st) {
            let _ = std::fs::write(&self.state_path, s);
        }
    }

    fn ensure_calendar(&mut self, st: &mut SyncState) -> Result<(), String> {
        if !st.calendar_id.is_empty() {
            return Ok(());
        }
        let token = self.access_token()?;
        let body = json!({ "summary": CALENDAR_NAME, "description": "Eventos y tareas de la app Notas (nodex-notes)" });
        let url = format!("{}/calendar/v3/calendars", self.ep.api);
        let v = self.send(self.http.post(url).bearer_auth(token).json(&body)).map_err(|e| e.message)?;
        st.calendar_id = v["id"].as_str().ok_or("Google no devolvió el calendario")?.to_string();
        st.events.clear();
        Ok(())
    }

    /// Crea en Google lo que falta y quita lo que ya no está. Devuelve (agregados, quitados).
    fn sync(&mut self, items: &[Item]) -> Result<(usize, usize), String> {
        let mut st = self.load_state();
        self.ensure_calendar(&mut st)?;
        let token = self.access_token()?;
        let wanted: BTreeMap<&str, &Item> = items.iter().map(|i| (i.key.as_str(), i)).collect();
        let (mut created, mut deleted) = (0, 0);

        let stale: Vec<(String, String)> =
            st.events.iter().filter(|(k, _)| !wanted.contains_key(k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
        for (key, event_id) in stale {
            let url = format!("{}/calendar/v3/calendars/{}/events/{}", self.ep.api, enc(&st.calendar_id), enc(&event_id));
            match self.send(self.http.delete(url).bearer_auth(&token)) {
                Ok(_) => deleted += 1,
                Err(e) if e.status == 404 || e.status == 410 => {} // ya no estaba
                Err(e) => return Err(e.message),
            }
            st.events.remove(&key);
            self.save_state(&st);
        }

        for (key, item) in wanted {
            if st.events.contains_key(key) {
                continue;
            }
            let Some(body) = event_body(item) else { continue };
            let url = format!("{}/calendar/v3/calendars/{}/events", self.ep.api, enc(&st.calendar_id));
            match self.send(self.http.post(url).bearer_auth(&token).json(&body)) {
                Ok(v) => {
                    if let Some(id) = v["id"].as_str() {
                        st.events.insert(key.to_string(), id.to_string());
                        created += 1;
                        self.save_state(&st);
                    }
                }
                Err(e) if e.status == 404 => {
                    // Borraron el calendario "Notas" en Google: se crea de nuevo en la próxima vuelta.
                    st.calendar_id.clear();
                    st.events.clear();
                    self.save_state(&st);
                    return Err("el calendario «Notas» ya no existía; se volverá a crear".into());
                }
                Err(e) => return Err(e.message),
            }
        }
        Ok((created, deleted))
    }
}

fn open_browser(url: &str) {
    let result = if cfg!(windows) {
        // "start" interpreta los '&'; rundll32 abre la URL tal cual en el navegador por defecto.
        std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    let _ = result;
}

/// Espera la redirección de Google en el servidor local y devuelve el código.
fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let params = query_params(request.lines().next().unwrap_or(""));
                let (ok, page) = match (params.get("code"), params.get("error")) {
                    (Some(_), _) if params.get("state").map(String::as_str) == Some(state) => {
                        (true, "Listo: Notas quedó conectada a tu Google Calendar. Ya puedes cerrar esta pestaña.")
                    }
                    (_, Some(_)) => (false, "No se dio el permiso. Puedes cerrar esta pestaña e intentarlo de nuevo."),
                    _ => {
                        // Otra petición del navegador (p. ej. favicon): se ignora.
                        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                        continue;
                    }
                };
                let html = format!(
                    "<!doctype html><meta charset=utf-8><title>Notas</title><body style=\"font-family:sans-serif;padding:3em\"><h2>{page}</h2></body>"
                );
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                        html.len()
                    )
                    .as_bytes(),
                );
                return if ok {
                    Ok(params["code"].clone())
                } else {
                    Err(format!("permiso rechazado ({})", params.get("error").cloned().unwrap_or_default()))
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    return Err("se acabó el tiempo para dar el permiso en el navegador".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_helpers() {
        assert_eq!(enc("a b/ñ"), "a%20b%2F%C3%B1");
        let p = query_params("GET /?code=4%2F0Ab-x&state=xyz&scope=a+b HTTP/1.1");
        assert_eq!(p["code"], "4/0Ab-x");
        assert_eq!(p["state"], "xyz");
        assert_eq!(p["scope"], "a b");
        // Ejemplo del RFC 7636
        assert_eq!(pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn builds_events() {
        let timed = Item { key: "e1".into(), date: "2026-09-28".into(), time: Some("10:00".into()), title: "Visita".into(), description: String::new() };
        let v = event_body(&timed).unwrap();
        assert!(v["start"]["dateTime"].as_str().unwrap().starts_with("2026-09-28T10:00:00"));
        assert!(v["end"]["dateTime"].as_str().unwrap().starts_with("2026-09-28T11:00:00"));
        let day = Item { time: None, ..timed };
        let v = event_body(&day).unwrap();
        assert_eq!((v["start"]["date"].as_str(), v["end"]["date"].as_str()), (Some("2026-09-28"), Some("2026-09-29")));
    }
}
