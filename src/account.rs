//! Tu cuenta de Notas: entrar (con un código que llega por correo, o con Microsoft), el plan y la
//! prueba, y la configuración que viaja con la cuenta.
//!
//! La configuración compartida (calendarios, cuentas de correo, Google, To Do, organizar sola…)
//! se guarda en el servidor **cifrada** con una clave que vive en la carpeta de notas
//! (`.nodex/cuenta.clave`): el servidor solo no puede leerla, y la carpeta sola tampoco.
//! Cada dato lleva la hora de su último cambio y, al juntar, gana el más reciente.
//! La carpeta de notas y la sesión no viajan: son de cada equipo.

use crate::config::Config;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

/// El servidor de Notas, si la versión se compiló con uno (`NODEX_SERVIDOR_IA`).
pub const BUILT_IN_SERVER: Option<&str> = option_env!("NODEX_SERVIDOR_IA");

/// La dirección del servidor: la de config.toml o la que trae la app.
pub fn server(cfg: &Config) -> Option<String> {
    let s = cfg.servidor_ia.trim();
    let s = if s.is_empty() { BUILT_IN_SERVER.unwrap_or("").trim() } else { s };
    let s = s.trim_end_matches('/').trim_end_matches("/v1");
    if s.is_empty() {
        return None;
    }
    Some(if s.starts_with("http://") || s.starts_with("https://") { s.to_string() } else { format!("https://{s}") })
}

/// Lo que dice el servidor de la cuenta.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Info {
    pub correo: String,
    pub nombre: String,
    pub plan: String,
    pub prueba_hasta: String,
    pub dias_prueba: i64,
}

impl Info {
    /// "Prueba · quedan 9 días", "Pro", "Prueba terminada"
    pub fn plan_label(&self) -> String {
        match self.plan.as_str() {
            "prueba" if self.dias_prueba > 1 => format!("Prueba gratis · quedan {} días", self.dias_prueba),
            "prueba" if self.dias_prueba == 1 => "Prueba gratis · queda 1 día".into(),
            "prueba" if self.prueba_hasta >= chrono::Local::now().format("%Y-%m-%d").to_string() => "Prueba gratis · termina hoy".into(),
            "prueba" => "Prueba terminada".into(),
            "pro" => "Pro".into(),
            "fundador" => "Fundador".into(),
            "gratis" => "Gratis (sin IA incluida)".into(),
            p => p.to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SignedIn {
    pub token: String,
    pub cuenta: Info,
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().user_agent(format!("nodex-notes/{}", env!("CARGO_PKG_VERSION"))).build().unwrap_or_default()
}

/// El mensaje de error del servidor (`{"error": {"message": …}}`), o uno genérico.
async fn check(r: reqwest::Response) -> Result<String, String> {
    let status = r.status();
    let text = r.text().await.map_err(|e| e.to_string())?;
    if status.is_success() {
        return Ok(text);
    }
    let msg = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.pointer("/error/message").and_then(|m| m.as_str()).map(str::to_string));
    Err(msg.unwrap_or_else(|| format!("el servidor respondió {status}")))
}

/// Marca de «tu cuenta espera aprobación» al comienzo del mensaje (no es un error).
pub const PENDING: &str = "pendiente:";

/// Lee la respuesta de entrar: la sesión, o que la cuenta espera aprobación.
fn signed_in(text: &str) -> Result<SignedIn, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if v["pendiente"].as_bool() == Some(true) {
        return Err(format!("{PENDING}{}", v["mensaje"].as_str().unwrap_or("Tu cuenta espera aprobación.")));
    }
    serde_json::from_value(v).map_err(|e| e.to_string())
}

/// Hace algo con el servidor en otro hilo y avisa a la ventana al terminar.
fn spawn<T: Send + 'static>(ctx: eframe::egui::Context, job: impl FnOnce() -> Result<T, String> + Send + 'static) -> Receiver<Result<T, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(job());
        ctx.request_repaint();
    });
    rx
}

pub fn send_code(base: String, email: String, ctx: eframe::egui::Context) -> Receiver<Result<(), String>> {
    spawn(ctx, move || {
        runtime()?.block_on(async {
            let r = http().post(format!("{base}/v1/cuenta/codigo")).json(&serde_json::json!({ "correo": email.trim() })).send().await;
            check(r.map_err(|e| format!("sin conexión con el servidor ({e})"))?).await.map(|_| ())
        })
    })
}

/// Pide al servidor algo que termina en una sesión (crear cuenta, entrar, clave nueva).
fn session(base: String, path: &'static str, body: Value, ctx: eframe::egui::Context) -> Receiver<Result<SignedIn, String>> {
    spawn(ctx, move || {
        runtime()?.block_on(async {
            let r = http().post(format!("{base}{path}")).json(&body).send().await;
            let text = check(r.map_err(|e| format!("sin conexión con el servidor ({e})"))?).await?;
            signed_in(&text)
        })
    })
}

/// Crear una cuenta con correo y clave.
pub fn register(base: String, email: String, password: String, ctx: eframe::egui::Context) -> Receiver<Result<SignedIn, String>> {
    session(base, "/v1/cuenta/registro", serde_json::json!({ "correo": email.trim(), "clave": password }), ctx)
}

/// Entrar con correo y clave.
pub fn sign_in_password(base: String, email: String, password: String, ctx: eframe::egui::Context) -> Receiver<Result<SignedIn, String>> {
    session(base, "/v1/cuenta/entrar-clave", serde_json::json!({ "correo": email.trim(), "clave": password }), ctx)
}

/// «¿Olvidaste tu clave?»: con el código del correo, una clave nueva (y adentro).
pub fn reset_password(base: String, email: String, code: String, password: String, ctx: eframe::egui::Context) -> Receiver<Result<SignedIn, String>> {
    session(base, "/v1/cuenta/clave-nueva", serde_json::json!({ "correo": email.trim(), "codigo": code.trim(), "clave": password }), ctx)
}

/// Entrar con Microsoft: se pide permiso para leer el perfil (navegador) y el servidor pregunta
/// a Microsoft de quién es.
pub fn sign_in_microsoft(base: String, ctx: eframe::egui::Context) -> Receiver<Result<SignedIn, String>> {
    spawn(ctx, move || {
        let token = microsoft_profile_token()?;
        runtime()?.block_on(async {
            let r = http().post(format!("{base}/v1/cuenta/microsoft")).json(&serde_json::json!({ "token": token })).send().await;
            let text = check(r.map_err(|e| format!("sin conexión con el servidor ({e})"))?).await?;
            signed_in(&text)
        })
    })
}

/// Permiso de Microsoft para leer tu perfil (correo y nombre), con el navegador.
fn microsoft_profile_token() -> Result<String, String> {
    use crate::gcal::{enc, open_browser, pkce_challenge, random_token, wait_for_code};
    let v4 = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = v4.local_addr().map_err(|e| e.to_string())?.port();
    let mut listeners = vec![v4];
    if let Ok(v6) = std::net::TcpListener::bind(("::1", port)) {
        listeners.push(v6);
    }
    let redirect = format!("http://localhost:{port}");
    let (verifier, state) = (random_token(48)?, random_token(18)?);
    let scope = "User.Read";
    open_browser(&format!(
        "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id={}&response_type=code&redirect_uri={}&response_mode=query&scope={}&code_challenge={}&code_challenge_method=S256&prompt=select_account&state={state}",
        crate::todo::CLIENT_ID,
        enc(&redirect),
        enc(scope),
        pkce_challenge(&verifier)
    ));
    let code = wait_for_code(&listeners, &state, "Notas")?;
    runtime()?.block_on(async {
        let form = [
            ("client_id", crate::todo::CLIENT_ID),
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("code_verifier", verifier.as_str()),
            ("scope", scope),
        ];
        let r = http().post("https://login.microsoftonline.com/common/oauth2/v2.0/token").form(&form).send().await.map_err(|e| e.to_string())?;
        let v: Value = r.json().await.map_err(|e| e.to_string())?;
        v["access_token"].as_str().map(str::to_string).ok_or_else(|| "Microsoft no entregó el permiso".to_string())
    })
}

pub fn fetch_info(base: String, token: String, ctx: eframe::egui::Context) -> Receiver<Result<Info, String>> {
    spawn(ctx, move || {
        runtime()?.block_on(async {
            let r = http().get(format!("{base}/v1/cuenta")).bearer_auth(&token).send().await;
            let text = check(r.map_err(|e| format!("sin conexión con el servidor ({e})"))?).await?;
            serde_json::from_str(&text).map_err(|e| e.to_string())
        })
    })
}

pub fn sign_out(base: String, token: String) {
    std::thread::spawn(move || {
        if let Ok(rt) = runtime() {
            let _ = rt.block_on(http().post(format!("{base}/v1/cuenta/salir")).bearer_auth(&token).send());
        }
    });
}

// ---------- La configuración que viaja con la cuenta ----------

/// Un dato con la hora de su último cambio ("2026-09-30T17:06:01.123").
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Stamped {
    pub t: String,
    pub v: Value,
}

pub type Shared = BTreeMap<String, Stamped>;

/// Lo que este equipo recuerda de la última vez que juntó: la hora y la huella de cada dato.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncState {
    pub campos: BTreeMap<String, (String, u64)>,
}

fn state_path() -> PathBuf {
    crate::config::config_path().with_file_name("cuenta-sincronizada.json")
}

pub fn load_state() -> SyncState {
    crate::vault::read_text(&state_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_state(s: &SyncState) {
    let _ = std::fs::write(state_path(), serde_json::to_string_pretty(s).unwrap_or_default());
}

fn now_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string()
}

fn file_value(p: &Path) -> Value {
    crate::vault::read_text(p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}

/// Los datos de este equipo que viajan con la cuenta.
pub fn local_values(cfg: &Config) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    m.insert("calendarios".into(), serde_json::to_value(&cfg.calendarios).unwrap_or(Value::Null));
    m.insert("correos".into(), serde_json::to_value(&cfg.correos).unwrap_or(Value::Null));
    m.insert("correo_al_llegar".into(), Value::Bool(cfg.correo_al_llegar));
    m.insert("correo_diario".into(), Value::String(cfg.correo_diario.clone()));
    m.insert("ia_automatica".into(), Value::Bool(cfg.ia_automatica));
    m.insert("google_client".into(), serde_json::json!([cfg.google_client_id, cfg.google_client_secret]));
    // De las conexiones con Google y To Do viaja el permiso; cambia solo al conectar o desconectar
    // (no cada vez que se renueva), por eso su huella es «está o no está».
    m.insert("google_token".into(), file_value(&crate::gcal::token_path()));
    m.insert("microsoft_token".into(), file_value(&crate::todo::token_path()));
    m
}

/// La huella con que se nota un cambio local.
fn fingerprint(name: &str, v: &Value) -> u64 {
    match name {
        "google_token" | "microsoft_token" => u64::from(!v.is_null()),
        _ => crate::ai::fnv(&v.to_string()),
    }
}

/// Aplica en este equipo un dato que llegó de la cuenta. Devuelve si cambió la configuración
/// (hay que guardarla) y qué hay que reiniciar.
pub fn apply(cfg: &mut Config, name: &str, v: &Value) {
    let write_file = |p: PathBuf, v: &Value| {
        if v.is_null() {
            let _ = std::fs::remove_file(p);
        } else {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(p, v.to_string());
        }
    };
    match name {
        "calendarios" => cfg.calendarios = serde_json::from_value(v.clone()).unwrap_or_default(),
        "correos" => cfg.correos = serde_json::from_value(v.clone()).unwrap_or_default(),
        "correo_al_llegar" => cfg.correo_al_llegar = v.as_bool().unwrap_or(cfg.correo_al_llegar),
        "correo_diario" => cfg.correo_diario = v.as_str().unwrap_or(&cfg.correo_diario).to_string(),
        "ia_automatica" => cfg.ia_automatica = v.as_bool().unwrap_or(cfg.ia_automatica),
        "google_client" => {
            cfg.google_client_id = v[0].as_str().unwrap_or("").to_string();
            cfg.google_client_secret = v[1].as_str().unwrap_or("").to_string();
        }
        "google_token" => write_file(crate::gcal::token_path(), v),
        "microsoft_token" => write_file(crate::todo::token_path(), v),
        _ => {}
    }
}

/// Anota los cambios hechos en este equipo desde la última vez (les pone la hora de ahora).
pub fn note_local_changes(state: &mut SyncState, local: &BTreeMap<String, Value>) {
    for (name, v) in local {
        let fp = fingerprint(name, v);
        match state.campos.get(name) {
            Some((_, old)) if *old == fp => {}
            // La primera vez en este equipo, un dato vacío no le gana a lo de la cuenta.
            None if v.is_null() || v == &Value::Array(vec![]) => {
                state.campos.insert(name.clone(), (String::new(), fp));
            }
            _ => {
                state.campos.insert(name.clone(), (now_stamp(), fp));
            }
        }
    }
}

/// Junta lo de este equipo con lo de la cuenta: por cada dato, el cambio más reciente.
/// Devuelve lo juntado (para subir) y lo que hay que aplicar aquí.
pub fn merge(state: &mut SyncState, local: &BTreeMap<String, Value>, remote: &Shared) -> (Shared, Vec<(String, Value)>) {
    let mut out = Shared::new();
    let mut apply = Vec::new();
    let names: std::collections::BTreeSet<&String> = local.keys().chain(remote.keys()).collect();
    for name in names {
        let mine = state.campos.get(name).cloned().unwrap_or_default();
        match remote.get(name) {
            Some(r) if r.t > mine.0 => {
                if local.get(name) != Some(&r.v) {
                    apply.push((name.clone(), r.v.clone()));
                }
                state.campos.insert(name.clone(), (r.t.clone(), fingerprint(name, &r.v)));
                out.insert(name.clone(), r.clone());
            }
            _ => {
                if let Some(v) = local.get(name) {
                    out.insert(name.clone(), Stamped { t: mine.0.clone(), v: v.clone() });
                }
            }
        }
    }
    (out, apply)
}

// ---------- Cifrado ----------

fn key_path(root: &Path) -> PathBuf {
    root.join(".nodex").join("cuenta.clave")
}

/// La clave de la configuración (en la carpeta de notas); se crea si no hay.
pub fn key(root: &Path, create: bool) -> Option<[u8; 32]> {
    if let Some(k) = crate::vault::read_text(&key_path(root)).ok().and_then(|t| B64.decode(t.trim()).ok()).filter(|k| k.len() == 32) {
        return k.try_into().ok();
    }
    if !create {
        return None;
    }
    let mut k = [0u8; 32];
    getrandom::fill(&mut k).ok()?;
    let _ = std::fs::create_dir_all(root.join(".nodex"));
    std::fs::write(key_path(root), B64.encode(k)).ok()?;
    Some(k)
}

/// La clave como texto (para copiarla a un equipo que no comparte la carpeta de notas).
pub fn key_text(root: &Path) -> Option<String> {
    key(root, false).map(|k| B64.encode(k))
}

pub fn set_key_text(root: &Path, text: &str) -> Result<(), String> {
    let k = B64.decode(text.trim()).map_err(|_| "Esa clave no es válida".to_string())?;
    if k.len() != 32 {
        return Err("Esa clave no es válida".into());
    }
    let _ = std::fs::create_dir_all(root.join(".nodex"));
    std::fs::write(key_path(root), B64.encode(k)).map_err(|e| e.to_string())
}

pub fn encrypt(key: &[u8; 32], data: &Shared) -> Result<String, String> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce).map_err(|e| e.to_string())?;
    let plain = serde_json::to_vec(data).map_err(|e| e.to_string())?;
    let sealed = cipher.encrypt(XNonce::from_slice(&nonce), plain.as_slice()).map_err(|_| "no se pudo cifrar".to_string())?;
    Ok(B64.encode([nonce.as_slice(), sealed.as_slice()].concat()))
}

pub fn decrypt(key: &[u8; 32], text: &str) -> Result<Shared, String> {
    let bytes = B64.decode(text.trim()).map_err(|e| e.to_string())?;
    if bytes.len() < 24 {
        return Err("datos incompletos".into());
    }
    let cipher = XChaCha20Poly1305::new(key.into());
    let plain = cipher
        .decrypt(XNonce::from_slice(&bytes[..24]), &bytes[24..])
        .map_err(|_| "La configuración de tu cuenta está cifrada con otra clave: abre la misma carpeta de notas que en tu otro equipo, o pega su clave en Tu cuenta.".to_string())?;
    serde_json::from_slice(&plain).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Blob {
    version: u64,
    datos: String,
}

/// El resultado de juntar la configuración: lo que hay que aplicar en este equipo.
#[derive(Debug)]
pub struct Synced {
    pub apply: Vec<(String, Value)>,
    pub state: SyncState,
}

/// Junta la configuración de este equipo con la de la cuenta y sube el resultado (en otro hilo).
/// Si otro equipo la cambió al mismo tiempo, vuelve a leer y juntar.
pub fn sync(base: String, token: String, root: PathBuf, local: BTreeMap<String, Value>, mut state: SyncState, ctx: eframe::egui::Context) -> Receiver<Result<Synced, String>> {
    spawn(ctx, move || {
        note_local_changes(&mut state, &local);
        runtime()?.block_on(async {
            let http = http();
            for _ in 0..3 {
                let r = http.get(format!("{base}/v1/cuenta/config")).bearer_auth(&token).send().await;
                let blob: Blob = serde_json::from_str(&check(r.map_err(|e| format!("sin conexión con el servidor ({e})"))?).await?).map_err(|e| e.to_string())?;
                let key = key(&root, blob.datos.is_empty()).ok_or(
                    "La configuración de tu cuenta está cifrada con la clave de tu carpeta de notas: abre la misma carpeta que en tu otro equipo, o pega su clave en Tu cuenta.",
                )?;
                let remote = if blob.datos.is_empty() { Shared::new() } else { decrypt(&key, &blob.datos)? };
                let mut st = state.clone();
                let (merged, apply) = merge(&mut st, &local, &remote);
                if merged == remote {
                    return Ok(Synced { apply, state: st });
                }
                let put = http
                    .put(format!("{base}/v1/cuenta/config"))
                    .bearer_auth(&token)
                    .json(&Blob { version: blob.version, datos: encrypt(&key, &merged)? })
                    .send()
                    .await
                    .map_err(|e| format!("sin conexión con el servidor ({e})"))?;
                if put.status() == reqwest::StatusCode::CONFLICT {
                    continue; // otro equipo la cambió: se vuelve a leer y juntar
                }
                check(put).await?;
                return Ok(Synced { apply, state: st });
            }
            Err("Otro equipo está cambiando la configuración; se reintentará en un rato.".to_string())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_change_wins_per_field() {
        let mut a = SyncState::default();
        let local: BTreeMap<String, Value> = [("correo_diario".to_string(), Value::String("07:00".into())), ("ia_automatica".to_string(), Value::Bool(true))].into();
        note_local_changes(&mut a, &local);
        // En la cuenta: ia_automatica cambió después; correo_diario es más viejo.
        let remote: Shared = [
            ("ia_automatica".to_string(), Stamped { t: "2999-01-01T00:00:00.000".into(), v: Value::Bool(false) }),
            ("correo_diario".to_string(), Stamped { t: "2000-01-01T00:00:00.000".into(), v: Value::String("09:00".into()) }),
        ]
        .into();
        let (merged, apply) = merge(&mut a, &local, &remote);
        assert_eq!(apply, vec![("ia_automatica".to_string(), Value::Bool(false))]);
        assert_eq!(merged["correo_diario"].v, Value::String("07:00".into()));
        assert_eq!(merged["ia_automatica"].v, Value::Bool(false));
        // Sin cambios nuevos, juntar otra vez no cambia nada.
        let local2: BTreeMap<String, Value> = [("correo_diario".to_string(), Value::String("07:00".into())), ("ia_automatica".to_string(), Value::Bool(false))].into();
        note_local_changes(&mut a, &local2);
        let (again, apply2) = merge(&mut a, &local2, &merged);
        assert!(apply2.is_empty());
        assert_eq!(again, merged);
    }

    #[test]
    fn encrypts_and_rejects_another_key() {
        let root = std::env::temp_dir().join(format!("nodex-clave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert!(key(&root, false).is_none());
        let k = key(&root, true).unwrap();
        assert_eq!(key(&root, false), Some(k));
        let data: Shared = [("x".to_string(), Stamped { t: "1".into(), v: Value::Bool(true) })].into();
        let sealed = encrypt(&k, &data).unwrap();
        assert!(!sealed.contains("true"));
        assert_eq!(decrypt(&k, &sealed).unwrap(), data);
        assert!(decrypt(&[7u8; 32], &sealed).unwrap_err().contains("otra clave"));
        // La clave como texto se puede pegar en otro equipo.
        let other = root.join("otro");
        set_key_text(&other, &key_text(&root).unwrap()).unwrap();
        assert_eq!(key(&other, false), Some(k));
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod two_devices {
    use super::*;
    use axum::{Json, Router, routing::post};
    use std::sync::{Arc, Mutex};

    /// Dos equipos con la misma cuenta y la misma carpeta de notas: lo que se configura en uno
    /// llega al otro, y cada dato queda con su cambio más reciente.
    #[test]
    fn configuration_travels_between_devices() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let data = std::env::temp_dir().join(format!("nodex-cuenta-{}", std::process::id()));
        let root = std::env::temp_dir().join(format!("nodex-cuenta-notas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&root);
        let mails: Arc<Mutex<Vec<String>>> = Arc::default();
        let m2 = mails.clone();
        let data2 = data.clone();
        let base = rt.block_on(async move {
            let data = data2;
            let fake = Router::new().route(
                "/correo",
                post(move |Json(v): Json<Value>| {
                    m2.lock().unwrap().push(v["text"].as_str().unwrap_or("").to_string());
                    async { Json(serde_json::json!({"id": "1"})) }
                }),
            );
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let fake_url = format!("http://{}", l.local_addr().unwrap());
            tokio::spawn(async move { axum::serve(l, fake).await.unwrap() });
            let settings = nodex_ia::Settings { key: "k".into(), data: data.clone(), mail_api: format!("{fake_url}/correo"), mail_key: "x".into(), ..Default::default() };
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", l.local_addr().unwrap());
            tokio::spawn(async move { axum::serve(l, nodex_ia::router(nodex_ia::state(settings))).await.unwrap() });
            base
        });
        let ctx = eframe::egui::Context::default();
        // Crear la cuenta: queda pendiente hasta que se apruebe (no se puede entrar).
        let pending = register(base.clone(), "ana@obra.cl".into(), "clave-segura".into(), ctx.clone()).recv().unwrap().unwrap_err();
        assert!(pending.starts_with(PENDING), "{pending}");
        let again = sign_in_password(base.clone(), "ana@obra.cl".into(), "clave-segura".into(), ctx.clone()).recv().unwrap().unwrap_err();
        assert!(again.starts_with(PENDING), "{again}");
        let short = register(base.clone(), "otra@obra.cl".into(), "corta".into(), ctx.clone()).recv().unwrap().unwrap_err();
        assert!(short.contains("8 caracteres"), "{short}");
        // Se aprueba (nodex-ia aprobar): empieza la prueba.
        let mut accounts = nodex_ia::accounts::Accounts::load(&data);
        assert_eq!(accounts.pending().len(), 1);
        assert!(!std::fs::read_to_string(nodex_ia::accounts::Accounts::file(&data)).unwrap().contains("clave-segura"), "la clave no se guarda tal cual");
        accounts.approve("ana@obra.cl", 14).unwrap();
        accounts.save(&data).unwrap();
        let wrong = sign_in_password(base.clone(), "ana@obra.cl".into(), "otra-clave".into(), ctx.clone()).recv().unwrap().unwrap_err();
        assert_eq!(wrong, "Correo o clave incorrectos.");
        let s = sign_in_password(base.clone(), "ana@obra.cl".into(), "clave-segura".into(), ctx.clone()).recv().unwrap().unwrap();
        assert_eq!(s.cuenta.plan_label(), "Prueba gratis · quedan 14 días");
        // «¿Olvidaste tu clave?»: con el código del correo, una clave nueva.
        send_code(base.clone(), "ana@obra.cl".into(), ctx.clone()).recv().unwrap().unwrap();
        let code: String = mails.lock().unwrap().last().unwrap().chars().filter(|c| c.is_ascii_digit()).take(6).collect();
        reset_password(base.clone(), "ana@obra.cl".into(), code, "clave-nueva-1".into(), ctx.clone()).recv().unwrap().unwrap();
        let s = sign_in_password(base.clone(), "ana@obra.cl".into(), "clave-nueva-1".into(), ctx.clone()).recv().unwrap().unwrap();
        let token = s.token;

        let cal = serde_json::json!([{"nombre": "Trabajo", "url": "https://x.cl/a.ics"}]);
        // Equipo A: tiene un calendario y la revisión del correo a las 07:00.
        let mut a_local: BTreeMap<String, Value> = [("calendarios".to_string(), cal.clone()), ("correo_diario".to_string(), Value::String("07:00".into()))].into();
        let a = sync(base.clone(), token.clone(), root.clone(), a_local.clone(), SyncState::default(), ctx.clone()).recv().unwrap().unwrap();
        assert!(a.apply.is_empty());
        assert!(key(&root, false).is_some(), "la clave quedó en la carpeta de notas");
        // Equipo B (recién instalado): recibe el calendario; su valor por defecto del correo no le gana a lo de A.
        let b_local: BTreeMap<String, Value> = [("calendarios".to_string(), Value::Array(vec![])), ("correo_diario".to_string(), Value::String("07:00".into()))].into();
        let b = sync(base.clone(), token.clone(), root.clone(), b_local, SyncState::default(), ctx.clone()).recv().unwrap().unwrap();
        assert_eq!(b.apply, vec![("calendarios".to_string(), cal.clone())]);
        // B cambia la hora del correo; A la recibe y conserva su calendario.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b_local2: BTreeMap<String, Value> = [("calendarios".to_string(), cal.clone()), ("correo_diario".to_string(), Value::String("09:30".into()))].into();
        let b2 = sync(base.clone(), token.clone(), root.clone(), b_local2, b.state, ctx.clone()).recv().unwrap().unwrap();
        assert!(b2.apply.is_empty());
        let a2 = sync(base.clone(), token.clone(), root.clone(), a_local.clone(), a.state, ctx.clone()).recv().unwrap().unwrap();
        assert_eq!(a2.apply, vec![("correo_diario".to_string(), Value::String("09:30".into()))]);
        a_local.insert("correo_diario".into(), Value::String("09:30".into()));
        // En el servidor, la configuración no se puede leer.
        let stored = std::fs::read_dir(data.join("config")).unwrap().flatten().next().unwrap().path();
        let text = std::fs::read_to_string(stored).unwrap();
        assert!(!text.contains("Trabajo") && !text.contains("09:30"), "va cifrada");
        // Un equipo con otra carpeta (sin la clave) no puede leerla, y lo dice.
        let other = root.join("otra");
        let err = sync(base.clone(), token.clone(), other, a_local, SyncState::default(), ctx).recv().unwrap().unwrap_err();
        assert!(err.contains("clave"), "{err}");
        drop(rt);
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&root);
    }
}
