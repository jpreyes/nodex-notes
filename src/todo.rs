//! Sincronización de las tareas con Microsoft To Do, en un hilo aparte.
//!
//! - Conexión: se abre el navegador, Microsoft redirige a un servidor local y el permiso
//!   queda guardado en este equipo (`microsoft_token.json`, junto a config.toml).
//!   La app está registrada en Microsoft como cliente público: no hay secreto.
//! - Todo pasa en la lista «Notas» de To Do, en los dos sentidos:
//!   las tareas de Notas van a To Do (con su fecha); lo que se marca hecho, se cambia de
//!   fecha o se borra en un lado pasa al otro; lo que se agrega a la lista en To Do llega a Tareas.
//! - `.nodex/todo.json` recuerda qué tarea de To Do corresponde a cada tarea (por su `id:`)
//!   y cómo estaban la última vez, para saber de qué lado vino cada cambio.

use crate::gcal::{enc, open_browser, pkce_challenge, random_token, wait_for_code};
use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// La app "Notas" registrada en Microsoft (cliente público; no es un secreto).
pub(crate) const CLIENT_ID: &str = "0474f4e6-d59e-48f9-a4c1-60ba8d03a484";
const SCOPE: &str = "Tasks.ReadWrite offline_access User.Read";
const LIST_NAME: &str = "Notas";

/// Una tarea de Notas, como se compara con To Do.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalTask {
    pub id: String,
    pub title: String,
    pub due: Option<String>,
    pub done: bool,
    /// Espacio y nota, para el detalle de la tarea en To Do.
    pub body: String,
}

/// Una tarea de la lista «Notas» en To Do.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteTask {
    pub id: String,
    pub title: String,
    pub due: Option<String>,
    pub done: bool,
}

/// Lo que hay que cambiar en Notas por lo que se hizo en To Do.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Done(String, bool),
    Due(String, Option<String>),
    /// Tarea nueva agregada en To Do: (id nuevo para Notas, título, fecha).
    New(String, String, Option<String>),
}

/// Lo que hay que cambiar en To Do.
#[derive(Debug, Clone, PartialEq)]
enum Op {
    Create { ours: String, title: String, due: Option<String>, body: String },
    Update { todo: String, done: Option<bool>, due: Option<Option<String>>, title: Option<String> },
    Delete { todo: String },
}

/// Cómo estaba una tarea la última vez que quedaron iguales en los dos lados.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Link {
    pub todo: String,
    pub done: bool,
    pub title: String,
    pub due: Option<String>,
    /// Se borró en To Do: no se vuelve a crear.
    pub gone: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncState {
    pub list_id: String,
    /// id de la tarea en Notas -> su tarea en To Do
    pub links: BTreeMap<String, Link>,
}

/// Compara los dos lados con cómo estaban la última vez y decide qué cambiar en cada uno.
/// Deja `st` como quedará cuando todo se aplique (salvo las tareas por crear en To Do).
fn reconcile(st: &mut SyncState, locals: &[LocalTask], remote: &[RemoteTask], mut new_id: impl FnMut() -> String) -> (Vec<Op>, Vec<Change>) {
    let mut ops = Vec::new();
    let mut changes = Vec::new();
    let local_by: BTreeMap<&str, &LocalTask> = locals.iter().map(|l| (l.id.as_str(), l)).collect();
    let remote_by: BTreeMap<&str, &RemoteTask> = remote.iter().map(|r| (r.id.as_str(), r)).collect();

    let keys: Vec<String> = st.links.keys().cloned().collect();
    for key in keys {
        let link = st.links.get_mut(&key).expect("existe");
        if link.gone {
            if !local_by.contains_key(key.as_str()) {
                st.links.remove(&key);
            }
            continue;
        }
        match (local_by.get(key.as_str()), remote_by.get(link.todo.as_str())) {
            // Se borró en Notas: también en To Do.
            (None, Some(_)) => {
                ops.push(Op::Delete { todo: link.todo.clone() });
                st.links.remove(&key);
            }
            (None, None) => {
                st.links.remove(&key);
            }
            // Se borró en To Do: queda en Notas, pero no se vuelve a crear allá.
            (Some(_), None) => link.gone = true,
            (Some(l), Some(r)) => {
                let mut done = None;
                if l.done != link.done {
                    done = Some(l.done);
                    link.done = l.done;
                } else if r.done != link.done {
                    changes.push(Change::Done(key.clone(), r.done));
                    link.done = r.done;
                }
                let mut due = None;
                if l.due != link.due {
                    due = Some(l.due.clone());
                    link.due = l.due.clone();
                } else if r.due != link.due {
                    changes.push(Change::Due(key.clone(), r.due.clone()));
                    link.due = r.due.clone();
                }
                // El título va de Notas a To Do (allá se puede ver distinto, no se trae).
                let title = (l.title != link.title).then(|| {
                    link.title = l.title.clone();
                    l.title.clone()
                });
                if done.is_some() || due.is_some() || title.is_some() {
                    ops.push(Op::Update { todo: link.todo.clone(), done, due, title });
                }
            }
        }
    }

    // Lo que está en To Do sin pareja.
    let mut linked: Vec<String> = st.links.values().map(|l| l.todo.clone()).collect();
    linked.extend(ops.iter().filter_map(|o| match o {
        Op::Delete { todo } => Some(todo.clone()),
        _ => None,
    }));
    let mut orphans: Vec<&RemoteTask> = remote.iter().filter(|r| !linked.contains(&r.id)).collect();
    // Tareas de Notas sin pareja: si en To Do hay una con el mismo título, se unen; si no, se crean.
    let unlinked: Vec<&LocalTask> = locals.iter().filter(|l| !l.done && !st.links.contains_key(&l.id)).collect();
    for l in unlinked {
        if let Some(i) = orphans.iter().position(|r| r.title.eq_ignore_ascii_case(&l.title)) {
            let r = orphans.remove(i);
            st.links.insert(l.id.clone(), Link { todo: r.id.clone(), done: r.done, title: l.title.clone(), due: r.due.clone(), gone: false });
            if r.due != l.due {
                // Manda la de Notas.
                ops.push(Op::Update { todo: r.id.clone(), done: None, due: Some(l.due.clone()), title: None });
                st.links.get_mut(&l.id).expect("recién puesta").due = l.due.clone();
            }
            continue;
        }
        ops.push(Op::Create { ours: l.id.clone(), title: l.title.clone(), due: l.due.clone(), body: l.body.clone() });
    }
    // Agregadas en To Do (pendientes): llegan a Notas.
    for r in orphans.into_iter().filter(|r| !r.done) {
        let id = new_id();
        st.links.insert(id.clone(), Link { todo: r.id.clone(), done: false, title: r.title.clone(), due: r.due.clone(), gone: false });
        changes.push(Change::New(id, r.title.clone(), r.due.clone()));
    }
    (ops, changes)
}

/// La fecha de una tarea en To Do: medianoche de ese día, aquí, dicha en UTC.
fn due_to_remote(date: &str) -> Option<Value> {
    let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let local = Local.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).earliest()?;
    let utc = local.with_timezone(&Utc).naive_utc();
    Some(json!({ "dateTime": utc.format("%Y-%m-%dT%H:%M:%S").to_string(), "timeZone": "UTC" }))
}

/// El día de una fecha de To Do ("2026-09-30T03:00:00.0000000", "UTC") en la hora de aquí.
fn due_from_remote(v: &Value) -> Option<String> {
    let raw = v["dateTime"].as_str()?;
    let base = raw.split('.').next().unwrap_or(raw);
    let ndt = NaiveDateTime::parse_from_str(base, "%Y-%m-%dT%H:%M:%S").ok()?;
    let tz = v["timeZone"].as_str().unwrap_or("UTC");
    let date = if tz.eq_ignore_ascii_case("UTC") {
        Utc.from_utc_datetime(&ndt).with_timezone(&Local).date_naive()
    } else {
        ndt.date()
    };
    Some(date.format("%Y-%m-%d").to_string())
}

pub(crate) fn token_path() -> PathBuf {
    crate::config::config_path().with_file_name("microsoft_token.json")
}

#[derive(Default, Serialize, Deserialize)]
struct Token {
    refresh_token: String,
}

pub fn is_connected() -> bool {
    token_path().is_file()
}

// ---------- Hilo de trabajo ----------

enum Job {
    Connect,
    Sync(Vec<LocalTask>),
    Disconnect,
}

/// Resultado de una sincronización: cambios para Notas, cuántas se mandaron a To Do y
/// un aviso si algo no alcanzó a crearse.
pub struct Synced {
    pub changes: Vec<Change>,
    pub sent: usize,
    pub warning: Option<String>,
}

enum Reply {
    Connected(Result<(), String>),
    Synced(Result<Synced, String>),
    Disconnected,
}

pub struct ToDo {
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
    /// Pruebas (variable NODEX_MS_API): no abre el navegador, imprime la URL.
    test: bool,
}

fn endpoints() -> Endpoints {
    match std::env::var("NODEX_MS_API") {
        Ok(base) if !base.is_empty() => {
            let base = base.trim_end_matches('/').to_string();
            Endpoints { auth: format!("{base}/authorize"), token: format!("{base}/token"), api: base, test: true }
        }
        _ => Endpoints {
            auth: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".into(),
            token: "https://login.microsoftonline.com/common/oauth2/v2.0/token".into(),
            api: "https://graph.microsoft.com/v1.0".into(),
            test: false,
        },
    }
}

impl ToDo {
    pub fn start(root: PathBuf, ctx: eframe::egui::Context) -> Option<ToDo> {
        let (tx, job_rx) = mpsc::channel::<Job>();
        let (reply_tx, rx) = mpsc::channel::<Reply>();
        let ep = endpoints();
        std::thread::Builder::new()
            .name("microsoft-todo".into())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
                let mut w = Worker { rt, http: reqwest::Client::new(), ep, state_path: root.join(".nodex").join("todo.json"), access: None };
                for job in job_rx {
                    let reply = match job {
                        Job::Connect => Reply::Connected(w.connect()),
                        Job::Sync(tasks) => Reply::Synced(w.sync(&tasks)),
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
        Some(ToDo { tx, rx, connected: is_connected(), busy: false, connecting: false, last_sync: None, last_error: None })
    }

    pub fn connect(&mut self) {
        if !self.busy && self.tx.send(Job::Connect).is_ok() {
            self.busy = true;
            self.connecting = true;
        }
    }

    pub fn sync(&mut self, tasks: Vec<LocalTask>) {
        if !self.busy && self.connected && self.tx.send(Job::Sync(tasks)).is_ok() {
            self.busy = true;
        }
    }

    pub fn disconnect(&mut self) {
        if self.tx.send(Job::Disconnect).is_ok() {
            self.busy = true;
        }
    }

    /// Respuestas del hilo: (cambios para Notas, mensajes para mostrar).
    pub fn poll(&mut self) -> (Vec<Change>, Vec<String>) {
        let mut changes = Vec::new();
        let mut msgs = Vec::new();
        while let Ok(r) = self.rx.try_recv() {
            self.busy = false;
            match r {
                Reply::Connected(Ok(())) => {
                    self.connecting = false;
                    self.connected = true;
                    self.last_error = None;
                    msgs.push("Microsoft To Do conectado: tus tareas van a la lista «Notas»".into());
                }
                Reply::Connected(Err(e)) => {
                    self.connecting = false;
                    self.last_error = Some(e.clone());
                    msgs.push(format!("Microsoft To Do: {e}"));
                }
                Reply::Synced(Ok(s)) => {
                    self.last_sync = Some(Local::now());
                    self.last_error = s.warning.clone();
                    let got = s.changes.len();
                    if s.sent + got > 0 {
                        msgs.push(format!("Microsoft To Do: {} enviadas, {} recibidas", s.sent, got));
                    }
                    if let Some(w) = s.warning {
                        msgs.push(format!("Microsoft To Do: {w}"));
                    }
                    changes.extend(s.changes);
                }
                Reply::Synced(Err(e)) => {
                    self.connected = is_connected();
                    self.last_error = Some(e.clone());
                    msgs.push(format!("Microsoft To Do: {e}"));
                }
                Reply::Disconnected => {
                    self.connected = false;
                    self.last_sync = None;
                    msgs.push("Microsoft To Do desconectado".into());
                }
            }
        }
        (changes, msgs)
    }
}

struct Worker {
    rt: tokio::runtime::Runtime,
    http: reqwest::Client,
    ep: Endpoints,
    state_path: PathBuf,
    access: Option<(String, Instant)>,
}

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

    /// Navegador -> inicio de sesión de Microsoft -> servidor local -> token.
    fn connect(&mut self) -> Result<(), String> {
        let v4 = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = v4.local_addr().map_err(|e| e.to_string())?.port();
        // "localhost" puede resolver a IPv6: se escucha también ahí si se puede.
        let mut listeners = vec![v4];
        if let Ok(v6) = TcpListener::bind(("::1", port)) {
            listeners.push(v6);
        }
        let redirect = format!("http://localhost:{port}");
        let verifier = random_token(48)?;
        let state = random_token(18)?;
        let url = format!(
            "{}?client_id={}&response_type=code&redirect_uri={}&response_mode=query&scope={}&code_challenge={}&code_challenge_method=S256&prompt=select_account&state={}",
            self.ep.auth,
            CLIENT_ID,
            enc(&redirect),
            enc(SCOPE),
            pkce_challenge(&verifier),
            state
        );
        if self.ep.test {
            // Pruebas: el "navegador" vuelve solo con un código.
            let back = format!("GET /?code=prueba&state={state} HTTP/1.1\r\nHost: localhost\r\n\r\n");
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                if let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) {
                    use std::io::{Read, Write};
                    let _ = s.write_all(back.as_bytes());
                    let _ = s.read(&mut [0u8; 512]);
                }
            });
        } else {
            open_browser(&url);
        }
        let code = wait_for_code(&listeners, &state, "Microsoft To Do")?;
        let form = [
            ("client_id", CLIENT_ID),
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("code_verifier", verifier.as_str()),
            ("scope", SCOPE),
        ];
        let v = self.send(self.http.post(&self.ep.token).form(&form)).map_err(|e| e.message)?;
        self.save_token(&v)?;
        self.remember_access(&v);
        // Crea (o encuentra) la lista de inmediato para confirmar que todo funciona.
        let mut st = self.load_state();
        self.ensure_list(&mut st)?;
        self.save_state(&st);
        Ok(())
    }

    /// Microsoft entrega un token de renovación nuevo cada vez: se guarda el último.
    fn save_token(&self, v: &Value) -> Result<(), String> {
        let refresh = v["refresh_token"].as_str().ok_or("Microsoft no entregó un permiso permanente")?;
        let token = serde_json::to_string(&Token { refresh_token: refresh.to_string() }).map_err(|e| e.to_string())?;
        if let Some(dir) = token_path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(token_path(), token).map_err(|e| e.to_string())
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
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", tok.refresh_token.as_str()),
            ("scope", SCOPE),
        ];
        match self.send(self.http.post(&self.ep.token).form(&form)) {
            Ok(v) => {
                let _ = self.save_token(&v);
                self.remember_access(&v);
                self.access.as_ref().map(|(t, _)| t.clone()).ok_or_else(|| "respuesta sin token".into())
            }
            Err(e) if e.status == 400 || e.status == 401 => {
                let _ = std::fs::remove_file(token_path());
                Err("Microsoft retiró el permiso; vuelve a conectar desde Tareas".into())
            }
            Err(e) => Err(e.message),
        }
    }

    fn load_state(&self) -> SyncState {
        std::fs::read_to_string(&self.state_path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    fn save_state(&self, st: &SyncState) {
        if let Some(dir) = self.state_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string_pretty(st) {
            let _ = std::fs::write(&self.state_path, s);
        }
    }

    /// La lista «Notas»: la que ya existe con ese nombre, o una nueva.
    fn ensure_list(&mut self, st: &mut SyncState) -> Result<(), String> {
        if !st.list_id.is_empty() {
            return Ok(());
        }
        let token = self.access_token()?;
        let v = self.send(self.http.get(format!("{}/me/todo/lists", self.ep.api)).bearer_auth(&token)).map_err(|e| e.message)?;
        let found = v["value"].as_array().and_then(|a| a.iter().find(|l| l["displayName"].as_str() == Some(LIST_NAME)).and_then(|l| l["id"].as_str()));
        st.list_id = match found {
            Some(id) => id.to_string(),
            None => {
                let body = json!({ "displayName": LIST_NAME });
                let v = self.send(self.http.post(format!("{}/me/todo/lists", self.ep.api)).bearer_auth(&token).json(&body)).map_err(|e| e.message)?;
                v["id"].as_str().ok_or("Microsoft no devolvió la lista")?.to_string()
            }
        };
        st.links.clear();
        Ok(())
    }

    fn tasks_url(&self, st: &SyncState) -> String {
        format!("{}/me/todo/lists/{}/tasks", self.ep.api, enc(&st.list_id))
    }

    fn fetch(&mut self, st: &SyncState) -> Result<Vec<RemoteTask>, HttpError> {
        let token = self.access_token().map_err(|message| HttpError { status: 0, message })?;
        let mut out = Vec::new();
        let mut url = format!("{}?$top=100", self.tasks_url(st));
        loop {
            let v = self.send(self.http.get(&url).bearer_auth(&token))?;
            for t in v["value"].as_array().into_iter().flatten() {
                let Some(id) = t["id"].as_str() else { continue };
                out.push(RemoteTask {
                    id: id.to_string(),
                    title: t["title"].as_str().unwrap_or("").trim().to_string(),
                    due: due_from_remote(&t["dueDateTime"]),
                    done: t["status"].as_str() == Some("completed"),
                });
            }
            match v["@odata.nextLink"].as_str() {
                Some(next) => url = next.to_string(),
                None => break,
            }
        }
        Ok(out)
    }

    fn sync(&mut self, locals: &[LocalTask]) -> Result<Synced, String> {
        let mut st = self.load_state();
        self.ensure_list(&mut st)?;
        let remote = match self.fetch(&st) {
            Ok(r) => r,
            Err(e) if e.status == 404 => {
                // Borraron la lista «Notas» en To Do: se crea de nuevo en la próxima vuelta.
                st.list_id.clear();
                st.links.clear();
                self.save_state(&st);
                return Err("la lista «Notas» ya no existía en To Do; se volverá a crear".into());
            }
            Err(e) => return Err(e.message),
        };
        let mut next_id = || {
            let mut b = [0u8; 6];
            let _ = getrandom::fill(&mut b);
            b.iter().map(|x| char::from(b"0123456789abcdefghijklmnopqrstuvwxyz"[(*x % 36) as usize])).collect::<String>()
        };
        let (ops, changes) = reconcile(&mut st, locals, &remote, &mut next_id);
        let token = self.access_token()?;
        let url = self.tasks_url(&st);
        let mut sent = 0;
        // Primero cambios y borrados (si algo falla, no se guarda nada y se reintenta después).
        for op in ops.iter().filter(|o| !matches!(o, Op::Create { .. })) {
            match op {
                Op::Update { todo, done, due, title } => {
                    let mut body = serde_json::Map::new();
                    if let Some(d) = done {
                        body.insert("status".into(), json!(if *d { "completed" } else { "notStarted" }));
                    }
                    if let Some(due) = due {
                        body.insert("dueDateTime".into(), due.as_deref().and_then(due_to_remote).unwrap_or(Value::Null));
                    }
                    if let Some(t) = title {
                        body.insert("title".into(), json!(t));
                    }
                    match self.send(self.http.patch(format!("{url}/{}", enc(todo))).bearer_auth(&token).json(&Value::Object(body))) {
                        Ok(_) => sent += 1,
                        Err(e) if e.status == 404 => {}
                        Err(e) => return Err(e.message),
                    }
                }
                Op::Delete { todo } => match self.send(self.http.delete(format!("{url}/{}", enc(todo))).bearer_auth(&token)) {
                    Ok(_) => sent += 1,
                    Err(e) if e.status == 404 => {}
                    Err(e) => return Err(e.message),
                },
                Op::Create { .. } => {}
            }
        }
        self.save_state(&st);
        // Después las nuevas, guardando cada una apenas se crea (para no duplicarlas).
        let mut warning = None;
        for op in ops {
            let Op::Create { ours, title, due, body } = op else { continue };
            let mut v = json!({ "title": title, "body": { "content": body, "contentType": "text" } });
            if let Some(d) = due.as_deref().and_then(due_to_remote) {
                v["dueDateTime"] = d;
            }
            match self.send(self.http.post(&url).bearer_auth(&token).json(&v)) {
                Ok(r) => {
                    if let Some(id) = r["id"].as_str() {
                        st.links.insert(ours, Link { todo: id.to_string(), done: false, title, due, gone: false });
                        self.save_state(&st);
                        sent += 1;
                    }
                }
                Err(e) => {
                    warning = Some(format!("no se pudieron enviar todas ({})", e.message));
                    break;
                }
            }
        }
        Ok(Synced { changes, sent, warning })
    }
}

/// Para las pruebas de la app: el día de una fecha de To Do.
#[cfg(test)]
pub fn tests_due(v: &Value) -> Option<String> {
    due_from_remote(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(id: &str, title: &str, due: Option<&str>, done: bool) -> LocalTask {
        LocalTask { id: id.into(), title: title.into(), due: due.map(Into::into), done, body: String::new() }
    }

    fn remote(id: &str, title: &str, due: Option<&str>, done: bool) -> RemoteTask {
        RemoteTask { id: id.into(), title: title.into(), due: due.map(Into::into), done }
    }

    #[test]
    fn two_way_sync_decides_each_side() {
        let mut st = SyncState::default();
        let mut n = 0;
        let mut next_id = || {
            n += 1;
            format!("nuevo{n}")
        };
        // Primera vez: lo pendiente de Notas se crea; lo hecho no; lo que había en To Do llega a Notas.
        let locals = vec![local("a1", "Enviar planos", Some("2026-09-30"), false), local("b2", "Vieja", None, true)];
        let remotes = vec![remote("R9", "Comprar pan", Some("2026-09-28"), false)];
        let (ops, changes) = reconcile(&mut st, &locals, &remotes, &mut next_id);
        assert_eq!(ops, vec![Op::Create { ours: "a1".into(), title: "Enviar planos".into(), due: Some("2026-09-30".into()), body: String::new() }]);
        assert_eq!(changes, vec![Change::New("nuevo1".into(), "Comprar pan".into(), Some("2026-09-28".into()))]);
        // (el hilo guarda la pareja al crearla)
        st.links.insert("a1".into(), Link { todo: "R1".into(), title: "Enviar planos".into(), due: Some("2026-09-30".into()), ..Link::default() });

        // Se marca hecha en To Do y se cambia la fecha en Notas.
        let locals = vec![local("a1", "Enviar planos", Some("2026-10-02"), false), local("nuevo1", "Comprar pan", Some("2026-09-28"), false)];
        let remotes = vec![remote("R1", "Enviar planos", Some("2026-09-30"), true), remote("R9", "Comprar pan", Some("2026-09-28"), false)];
        let (ops, changes) = reconcile(&mut st, &locals, &remotes, &mut next_id);
        assert_eq!(changes, vec![Change::Done("a1".into(), true)]);
        assert_eq!(ops, vec![Op::Update { todo: "R1".into(), done: None, due: Some(Some("2026-10-02".into())), title: None }]);

        // Se borra en Notas: se borra en To Do. Se borra en To Do: queda en Notas y no se recrea.
        let locals = vec![local("a1", "Enviar planos", Some("2026-10-02"), true)];
        let remotes = vec![remote("R9", "Comprar pan", Some("2026-09-28"), false)];
        let (ops, changes) = reconcile(&mut st, &locals, &remotes, &mut next_id);
        assert_eq!(ops, vec![Op::Delete { todo: "R9".into() }]);
        assert!(changes.is_empty());
        assert!(st.links["a1"].gone);
        let (ops, _) = reconcile(&mut st, &locals, &remotes[..0], &mut next_id);
        assert!(ops.is_empty(), "no se vuelve a crear");
    }

    #[test]
    fn same_title_links_instead_of_duplicating() {
        let mut st = SyncState::default();
        let locals = vec![local("a1", "Enviar planos", Some("2026-09-30"), false)];
        let remotes = vec![remote("R1", "enviar planos", None, false)];
        let (ops, changes) = reconcile(&mut st, &locals, &remotes, || "x".into());
        assert!(changes.is_empty());
        assert_eq!(ops, vec![Op::Update { todo: "R1".into(), done: None, due: Some(Some("2026-09-30".into())), title: None }]);
        assert_eq!(st.links["a1"].todo, "R1");
    }

    #[test]
    fn dates_round_trip_through_utc() {
        let v = due_to_remote("2026-09-30").unwrap();
        assert_eq!(v["timeZone"], "UTC");
        assert_eq!(due_from_remote(&v).as_deref(), Some("2026-09-30"));
        let v = json!({ "dateTime": "2026-10-05T00:00:00.0000000", "timeZone": "Pacific SA Standard Time" });
        assert_eq!(due_from_remote(&v).as_deref(), Some("2026-10-05"));
    }
}
