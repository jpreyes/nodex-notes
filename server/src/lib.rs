//! Servidor de la «IA incluida» de Notas.
//!
//! La app le habla como a cualquier servicio compatible con la API de OpenAI
//! (`POST /v1/chat/completions`), con el código de la persona como clave. El servidor:
//!
//! - reconoce a la persona por su código (se guarda solo su huella SHA-256, nunca el código);
//! - revisa que no haya llegado a su límite del mes (si llegó, responde 429 con un mensaje claro);
//! - reenvía el pedido a la IA con la clave del servicio y el modelo del servicio;
//! - suma los tokens que usó (entrada y salida), por persona y por mes.
//!
//! `GET /v1/uso` le dice a la app cuánto va del mes. Los datos son archivos JSON en una carpeta
//! (`usuarios.json`, `cuentas.json` y `uso/AAAA-MM.json`), suficiente para la beta.
//!
//! Hay dos formas de tener IA incluida: un código entregado a mano (`nodex-ia nuevo`, la beta) o
//! una cuenta (ver `accounts`): se entra con el correo o con Microsoft y la sesión sirve de clave.

pub mod accounts;
pub mod sync;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Cómo está configurado el servicio (variables de entorno, ver `Settings::from_env`).
#[derive(Debug, Clone)]
pub struct Settings {
    /// URL base de la IA (compatible con OpenAI), terminada en "/".
    pub upstream: String,
    pub key: String,
    pub model: String,
    pub data: PathBuf,
    /// Tokens al mes por persona, si su cuenta no dice otro.
    pub default_limit: u64,
    /// Precio en USD por millón de tokens (entrada, salida), para estimar el costo por persona.
    pub price_in: f64,
    pub price_out: f64,
    /// Prueba de las cuentas nuevas: días y tokens al mes.
    pub trial_days: i64,
    pub trial_limit: u64,
    /// Envío de los códigos por correo (API de Resend); sin clave, el código queda en el registro.
    pub mail_api: String,
    pub mail_key: String,
    pub mail_from: String,
    /// Microsoft Graph (para «Entrar con Microsoft»).
    pub graph: String,
    /// A quién avisar por correo cuando alguien crea una cuenta (para aprobarla).
    pub admin: String,
    /// Espacio para sincronizar las notas de cada cuenta, en bytes.
    pub sync_limit: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            upstream: "https://opencode.ai/zen/v1/".into(),
            key: String::new(),
            model: "deepseek-v4.1-flash".into(),
            data: PathBuf::from("datos"),
            default_limit: 3_000_000,
            price_in: 0.3,
            price_out: 1.2,
            trial_days: 14,
            trial_limit: 1_000_000,
            mail_api: "https://api.resend.com/emails".into(),
            mail_key: String::new(),
            mail_from: String::new(),
            graph: "https://graph.microsoft.com/v1.0".into(),
            admin: String::new(),
            sync_limit: 2048 * 1024 * 1024,
        }
    }
}

impl Settings {
    pub fn from_env() -> Settings {
        let var = |k: &str, d: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| d.to_string());
        let mut upstream = var("NOTAS_IA_UPSTREAM", "https://opencode.ai/zen/v1/");
        if !upstream.ends_with('/') {
            upstream.push('/');
        }
        Settings {
            upstream,
            key: var("NOTAS_IA_KEY", ""),
            model: var("NOTAS_IA_MODEL", "deepseek-v4.1-flash"),
            data: PathBuf::from(var("NOTAS_IA_DATA", "datos")),
            default_limit: var("NOTAS_IA_LIMITE", "3000000").parse().unwrap_or(3_000_000),
            price_in: var("NOTAS_IA_PRECIO_ENTRADA", "0.3").parse().unwrap_or(0.3),
            price_out: var("NOTAS_IA_PRECIO_SALIDA", "1.2").parse().unwrap_or(1.2),
            trial_days: var("NOTAS_IA_PRUEBA_DIAS", "14").parse().unwrap_or(14),
            trial_limit: var("NOTAS_IA_LIMITE_PRUEBA", "1000000").parse().unwrap_or(1_000_000),
            mail_api: var("NOTAS_IA_CORREO_API", "https://api.resend.com/emails"),
            mail_key: var("NOTAS_IA_CORREO_CLAVE", ""),
            mail_from: var("NOTAS_IA_CORREO_DE", "Notas <no-responder@notas.invalid>"),
            graph: var("NOTAS_IA_GRAPH", "https://graph.microsoft.com/v1.0"),
            admin: var("NOTAS_IA_AVISAR", ""),
            sync_limit: var("NOTAS_IA_SYNC_LIMITE", "2048").parse::<u64>().unwrap_or(2048) * 1024 * 1024,
        }
    }
}

/// Una persona con IA incluida.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct User {
    pub nombre: String,
    /// Tokens al mes; 0 = el del servicio.
    pub limite: u64,
    pub activo: bool,
    pub creado: String,
}

/// Lo que usó una persona en un mes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Usage {
    pub entrada: u64,
    pub salida: u64,
    pub pedidos: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.entrada + self.salida
    }
}

/// Personas (por la huella de su código) y el uso del mes en curso.
#[derive(Debug, Default)]
pub struct Store {
    pub users: BTreeMap<String, User>,
    /// Cuándo cambió `usuarios.json` la última vez que se leyó (para ver altas sin reiniciar).
    users_mtime: Option<std::time::SystemTime>,
    pub accounts: accounts::Accounts,
    /// Fecha y tamaño de cuentas.json la última vez que se leyó (el tamaño, por si dos cambios
    /// caen en la misma fecha).
    accounts_mtime: Option<(std::time::SystemTime, u64)>,
    pub month: String,
    pub usage: BTreeMap<String, Usage>,
}

pub fn fingerprint(code: &str) -> String {
    Sha256::digest(code.trim().as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn this_month() -> String {
    chrono::Local::now().format("%Y-%m").to_string()
}

/// "2026-10" -> "2026-11-01"
fn next_month(month: &str) -> String {
    let (y, m) = month.split_once('-').and_then(|(y, m)| Some((y.parse::<i32>().ok()?, m.parse::<u32>().ok()?))).unwrap_or((1970, 1));
    if m >= 12 { format!("{}-01-01", y + 1) } else { format!("{y}-{:02}-01", m + 1) }
}

/// Escribe un archivo sin dejarlo a medias (primero uno temporal, después se renombra).
pub(crate) fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}

/// Fecha y tamaño de un archivo (para saber si alguien lo cambió).
pub(crate) fn file_stamp(p: &Path) -> Option<(std::time::SystemTime, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

impl Store {
    fn users_file(data: &Path) -> PathBuf {
        data.join("usuarios.json")
    }

    fn usage_file(data: &Path, month: &str) -> PathBuf {
        data.join("uso").join(format!("{month}.json"))
    }

    pub fn load(data: &Path) -> Store {
        let read = |p: PathBuf| std::fs::read_to_string(p).ok();
        let users = read(Self::users_file(data)).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let month = this_month();
        let usage = read(Self::usage_file(data, &month)).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let users_mtime = std::fs::metadata(Self::users_file(data)).and_then(|m| m.modified()).ok();
        let accounts = accounts::Accounts::load(data);
        let accounts_mtime = file_stamp(&accounts::Accounts::file(data));
        Store { users, users_mtime, accounts, accounts_mtime, month, usage }
    }

    /// Relee las cuentas si alguien cambió el archivo (`nodex-ia plan` con el servidor andando).
    fn refresh_accounts(&mut self, data: &Path) {
        let now = file_stamp(&accounts::Accounts::file(data));
        if now != self.accounts_mtime {
            self.accounts = accounts::Accounts::load(data);
            self.accounts_mtime = now;
        }
    }

    /// Relee las personas si alguien cambió el archivo (`nodex-ia nuevo` con el servidor andando).
    fn refresh_users(&mut self, data: &Path) {
        let now = std::fs::metadata(Self::users_file(data)).and_then(|m| m.modified()).ok();
        if now != self.users_mtime {
            if let Some(u) = std::fs::read_to_string(Self::users_file(data)).ok().and_then(|t| serde_json::from_str(&t).ok()) {
                self.users = u;
            }
            self.users_mtime = now;
        }
    }

    pub fn save_users(&self, data: &Path) -> std::io::Result<()> {
        write_atomic(&Self::users_file(data), &serde_json::to_string_pretty(&self.users).unwrap_or_default())
    }

    fn save_usage(&self, data: &Path) -> std::io::Result<()> {
        write_atomic(&Self::usage_file(data, &self.month), &serde_json::to_string_pretty(&self.usage).unwrap_or_default())
    }

    /// Al cambiar de mes, el uso vuelve a cero.
    fn roll(&mut self) {
        let now = this_month();
        if now != self.month {
            self.month = now;
            self.usage.clear();
        }
    }

    /// Crea una persona y devuelve su código (se muestra una sola vez).
    pub fn add_user(&mut self, nombre: &str, limite: u64) -> String {
        let mut b = [0u8; 16];
        let _ = getrandom::fill(&mut b);
        let code = format!("nx-{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>());
        let user = User { nombre: nombre.to_string(), limite, activo: true, creado: chrono::Local::now().format("%Y-%m-%d").to_string() };
        self.users.insert(fingerprint(&code), user);
        code
    }

    fn limit_of(&self, u: &User, default: u64) -> u64 {
        if u.limite > 0 { u.limite } else { default }
    }
}

pub struct AppState {
    pub settings: Settings,
    pub store: Mutex<Store>,
    pub pending: Mutex<accounts::Pending>,
    pub http: reqwest::Client,
    /// La sincronización de las notas de cada cuenta.
    pub sync: sync::Hub,
}

pub(crate) fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": { "message": message, "type": "notas" } }))).into_response()
}

/// Quién pide (por su código o su sesión): con qué clave se cuenta su uso y cuál es su límite.
struct Who {
    key: String,
    limit: u64,
}

/// La persona del código o la sesión que viene en `Authorization: Bearer …`.
async fn who(state: &AppState, headers: &HeaderMap) -> Result<Who, Response> {
    let fp = fingerprint(&accounts::bearer(headers));
    let s = &state.settings;
    let mut store = state.store.lock().await;
    store.refresh_users(&s.data);
    store.refresh_accounts(&s.data);
    if let Some(u) = store.users.get(&fp) {
        return if u.activo {
            Ok(Who { key: fp.clone(), limit: store.limit_of(u, s.default_limit) })
        } else {
            Err(error(StatusCode::FORBIDDEN, "Este código de IA incluida está desactivado."))
        };
    }
    let Some(acc) = store.accounts.sesiones.get(&fp).and_then(|id| store.accounts.cuentas.get(id)) else {
        return Err(error(StatusCode::UNAUTHORIZED, "Código de IA incluida no válido: revísalo en Configuración → Inteligencia artificial."));
    };
    let key = format!("cuenta:{}", acc.id);
    let own = |d: u64| if acc.limite > 0 { acc.limite } else { d };
    match acc.plan.as_str() {
        "pro" | "fundador" => Ok(Who { key, limit: own(s.default_limit) }),
        "prueba" if acc.prueba_hasta >= chrono::Local::now().format("%Y-%m-%d").to_string() => {
            Ok(Who { key, limit: own(s.trial_limit) })
        }
        "prueba" => Err(error(
            StatusCode::PAYMENT_REQUIRED,
            &format!("Tu prueba de IA incluida terminó el {}. Para seguir, activa tu plan en Notas; tus notas siguen igual.", acc.prueba_hasta),
        )),
        _ => Err(error(StatusCode::PAYMENT_REQUIRED, "Tu plan no incluye IA incluida. Puedes activarlo en Notas, o usar tu propia clave de IA.")),
    }
}

/// Lo que se le muestra a la persona en la app.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct UsageReply {
    pub mes: String,
    pub usado: u64,
    pub limite: u64,
    pub pedidos: u64,
    /// Día en que vuelve a cero ("2026-11-01").
    pub renueva: String,
}

async fn usage(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let who = match who(&state, &headers).await {
        Ok(x) => x,
        Err(r) => return r,
    };
    let mut store = state.store.lock().await;
    store.roll();
    let u = store.usage.get(&who.key).cloned().unwrap_or_default();
    let reply = UsageReply {
        limite: who.limit,
        usado: u.total(),
        pedidos: u.pedidos,
        renueva: next_month(&store.month),
        mes: store.month.clone(),
    };
    Json(reply).into_response()
}

async fn chat(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(mut body): Json<serde_json::Value>) -> Response {
    let who = match who(&state, &headers).await {
        Ok(x) => x,
        Err(r) => return r,
    };
    let fp = who.key.clone();
    {
        let mut store = state.store.lock().await;
        store.roll();
        let used = store.usage.get(&fp).map(Usage::total).unwrap_or(0);
        if used >= who.limit {
            let when = next_month(&store.month);
            return error(
                StatusCode::TOO_MANY_REQUESTS,
                &format!("Llegaste al límite de IA incluida de este mes; se renueva el {when}. Mientras, tus notas se guardan igual (la IA las organiza después)."),
            );
        }
    }
    // El modelo lo decide el servicio; sin respuestas en partes (la app no las usa).
    if let Some(o) = body.as_object_mut() {
        o.insert("model".into(), serde_json::Value::String(state.settings.model.clone()));
        o.remove("stream");
    }
    let session = format!("notas-{}", fingerprint(&fp).get(..16).unwrap_or(""));
    let sent = state
        .http
        .post(format!("{}chat/completions", state.settings.upstream))
        .bearer_auth(&state.settings.key)
        .header("x-opencode-session", session)
        .json(&body)
        .send()
        .await;
    let resp = match sent {
        Ok(r) => r,
        Err(e) => return error(StatusCode::BAD_GATEWAY, &format!("No se pudo llegar a la IA: {e}")),
    };
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let text = resp.text().await.unwrap_or_default();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    if status.is_success() {
        let n = |k: &str| json.pointer(&format!("/usage/{k}")).and_then(|v| v.as_u64()).unwrap_or(0);
        let mut store = state.store.lock().await;
        store.roll();
        let u = store.usage.entry(fp).or_default();
        u.entrada += n("prompt_tokens");
        u.salida += n("completion_tokens");
        u.pedidos += 1;
        if let Err(e) = store.save_usage(&state.settings.data) {
            eprintln!("no se pudo guardar el uso: {e}");
        }
    }
    (status, [("content-type", "application/json")], text).into_response()
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(chat))
        .route("/v1/uso", get(usage))
        .route("/v1/cuenta/codigo", post(accounts::send_code))
        .route("/v1/cuenta/entrar", post(accounts::sign_in_code))
        .route("/v1/cuenta/registro", post(accounts::register))
        .route("/v1/cuenta/entrar-clave", post(accounts::sign_in_password))
        .route("/v1/cuenta/clave-nueva", post(accounts::reset_password))
        .route("/v1/cuenta/microsoft", post(accounts::sign_in_microsoft))
        .route("/v1/cuenta", get(accounts::me))
        .route("/v1/cuenta/salir", post(accounts::sign_out))
        .route("/v1/cuenta/config", get(accounts::get_config).put(accounts::put_config))
        .route("/v1/sync/cambios", get(sync::changes))
        .route("/v1/sync/esperar", get(sync::wait))
        .route("/v1/sync/blob/{hash}", get(sync::blob))
        .route(
            "/v1/sync/archivo",
            axum::routing::put(sync::put).delete(sync::delete).layer(axum::extract::DefaultBodyLimit::max(sync::MAX_FILE + 1024)),
        )
        .route("/salud", get(|| async { "ok" }))
        .with_state(state)
}

pub fn state(settings: Settings) -> Arc<AppState> {
    let store = Store::load(&settings.data);
    Arc::new(AppState { settings, store: Mutex::new(store), pending: Mutex::new(accounts::Pending::default()), http: reqwest::Client::new(), sync: sync::Hub::default() })
}

/// El uso del mes de cada persona, con su costo estimado (para `nodex-ia lista`).
pub fn report(settings: &Settings) -> String {
    let store = Store::load(&settings.data);
    let mut out = format!("Mes {} · límite por defecto {} tokens\n", store.month, settings.default_limit);
    let mut total = 0.0;
    for (fp, u) in &store.users {
        let usage = store.usage.get(fp).cloned().unwrap_or_default();
        let cost = usage.entrada as f64 / 1e6 * settings.price_in + usage.salida as f64 / 1e6 * settings.price_out;
        total += cost;
        let limit = store.limit_of(u, settings.default_limit);
        out += &format!(
            "{} {:<24} {:>10} / {:<10} tokens  {:>5} pedidos  ~{cost:.2} USD{}\n",
            &fp[..8],
            u.nombre,
            usage.total(),
            limit,
            usage.pedidos,
            if u.activo { "" } else { "  (desactivado)" }
        );
    }
    for a in store.accounts.cuentas.values() {
        let usage = store.usage.get(&format!("cuenta:{}", a.id)).cloned().unwrap_or_default();
        let cost = usage.entrada as f64 / 1e6 * settings.price_in + usage.salida as f64 / 1e6 * settings.price_out;
        total += cost;
        let plan = if a.plan == "prueba" { format!("prueba hasta {}", a.prueba_hasta) } else { a.plan.clone() };
        out += &format!("{:<32} {:<22} {:>10} tokens  {:>5} pedidos  ~{cost:.2} USD\n", a.correo, plan, usage.total(), usage.pedidos);
    }
    out += &format!("Costo estimado del mes: ~{total:.2} USD\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months() {
        assert_eq!(next_month("2026-10"), "2026-11-01");
        assert_eq!(next_month("2026-12"), "2027-01-01");
    }

    /// Un servidor de IA falso, el servidor de Notas delante y la app como cliente: mide el uso,
    /// rechaza códigos malos y corta al llegar al límite.
    #[tokio::test]
    async fn meters_usage_and_enforces_the_limit() {
        // IA falsa: devuelve siempre 100 tokens de entrada y 20 de salida, y anota qué recibió.
        let seen: Arc<Mutex<Vec<(String, serde_json::Value)>>> = Arc::default();
        let seen2 = seen.clone();
        let fake = Router::new().route(
            "/v1/chat/completions",
            post(move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                let seen = seen2.clone();
                async move {
                    let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                    seen.lock().await.push((auth, body));
                    Json(serde_json::json!({
                        "id": "x", "object": "chat.completion", "created": 0, "model": "m",
                        "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
                        "usage": {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120}
                    }))
                }
            }),
        );
        let fake_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fake_url = format!("http://{}/v1/", fake_listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(fake_listener, fake).await.unwrap() });

        let data = std::env::temp_dir().join(format!("nodex-ia-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let settings = Settings { upstream: fake_url, key: "clave-del-servicio".into(), data: data.clone(), default_limit: 300, ..Default::default() };
        let code = {
            let mut s = Store::load(&data);
            let code = s.add_user("Ana", 0);
            s.save_users(&data).unwrap();
            code
        };
        let st = state(settings.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router(st)).await.unwrap() });

        let http = reqwest::Client::new();
        let ask = |key: &str| {
            http.post(format!("{base}/v1/chat/completions"))
                .bearer_auth(key)
                .json(&serde_json::json!({"model": "incluida", "messages": [{"role": "user", "content": "hola"}], "stream": true}))
                .send()
        };
        // Código malo.
        assert_eq!(ask("nx-malo").await.unwrap().status(), 401);
        // Dos pedidos: 240 tokens de 300.
        for _ in 0..2 {
            let r = ask(&code).await.unwrap();
            assert_eq!(r.status(), 200);
            assert_eq!(r.json::<serde_json::Value>().await.unwrap()["choices"][0]["message"]["content"], "ok");
        }
        let (auth, body) = seen.lock().await[0].clone();
        assert_eq!(auth, "Bearer clave-del-servicio", "a la IA va la clave del servicio, no el código");
        assert_eq!((body["model"].as_str(), body.get("stream")), (Some("deepseek-v4.1-flash"), None));
        let u: UsageReply = http.get(format!("{base}/v1/uso")).bearer_auth(&code).send().await.unwrap().json().await.unwrap();
        assert_eq!((u.usado, u.limite, u.pedidos), (240, 300, 2));
        // El tercero pasa (aún no llega a 300) y el cuarto ya no.
        assert_eq!(ask(&code).await.unwrap().status(), 200);
        let r = ask(&code).await.unwrap();
        assert_eq!(r.status(), 429);
        assert!(r.text().await.unwrap().contains("Llegaste al límite"));
        assert_eq!(seen.lock().await.len(), 3);
        // El uso quedó en disco y el reporte lo muestra.
        assert!(report(&settings).contains("Ana"));
        assert!(std::fs::read_to_string(data.join("uso").join(format!("{}.json", this_month()))).unwrap().contains("\"pedidos\": 3"));
        let _ = std::fs::remove_dir_all(&data);
    }
}
