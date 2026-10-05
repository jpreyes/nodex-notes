//! Cuentas: crear una con correo y clave y entrar con ellos (lo normal); recuperar la clave con un
//! código de 6 dígitos que llega por correo; o entrar con Microsoft. El plan de cada cuenta
//! (prueba de 14 días, pro, fundador, gratis) y la configuración que viaja con la cuenta.
//!
//! La clave se guarda como huella Argon2 (con sal propia), nunca tal cual.
//!
//! **Las cuentas nuevas quedan pendientes** hasta que se aprueban (`nodex-ia aprobar <correo>`):
//! mientras tanto no se puede entrar. Al crearse una, se avisa por correo a `NOTAS_IA_AVISAR`;
//! al aprobarla, a la persona. La prueba de 14 días empieza al aprobarla.
//!
//! La configuración llega **cifrada** desde la app (con una clave que vive en la carpeta de notas
//! de la persona): el servidor la guarda y la entrega, pero no puede leerla.
//!
//! Datos: `cuentas.json` (cuentas y sesiones, estas por la huella de su token) y
//! `config/<cuenta>.json`.

use super::{AppState, error, fingerprint, write_atomic};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Cuánto dura un código enviado por correo, y cuántos intentos permite.
const CODE_TTL: Duration = Duration::from_secs(600);
const CODE_TRIES: u32 = 5;
/// No se manda otro código al mismo correo antes de esto.
const CODE_EVERY: Duration = Duration::from_secs(30);
/// Claves equivocadas permitidas por correo en `FAILS_WINDOW`.
const MAX_FAILS: u32 = 10;
const FAILS_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Largo mínimo de una clave.
const MIN_PASSWORD: usize = 8;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Account {
    pub id: String,
    pub correo: String,
    pub nombre: String,
    /// "prueba" (hasta `prueba_hasta`), "pro", "fundador" o "gratis" (sin IA).
    pub plan: String,
    pub prueba_hasta: String,
    /// Tokens al mes; 0 = el del plan.
    pub limite: u64,
    pub creado: String,
    /// Huella Argon2 de la clave (vacía si entra solo con código o Microsoft).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub clave: String,
    /// "" = activa; "pendiente" (espera aprobación) o "rechazada".
    #[serde(skip_serializing_if = "String::is_empty")]
    pub estado: String,
}

/// Lo que se guarda en `cuentas.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Accounts {
    /// Por identificador.
    pub cuentas: BTreeMap<String, Account>,
    /// Huella del token de sesión -> cuenta.
    pub sesiones: BTreeMap<String, String>,
}

impl Accounts {
    pub fn file(data: &Path) -> PathBuf {
        data.join("cuentas.json")
    }

    pub fn load(data: &Path) -> Accounts {
        std::fs::read_to_string(Self::file(data)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, data: &Path) -> std::io::Result<()> {
        write_atomic(&Self::file(data), &serde_json::to_string_pretty(self).unwrap_or_default())
    }

    pub fn by_email(&self, correo: &str) -> Option<&Account> {
        self.cuentas.values().find(|a| a.correo.eq_ignore_ascii_case(correo))
    }

    /// Entrar con ese correo: un token de sesión nuevo si la cuenta está activa. Si no existe, se
    /// crea **pendiente** (sin sesión). `Err((cuenta, es_nueva))` si no se puede entrar todavía.
    fn sign_in(&mut self, correo: &str, nombre: &str) -> Result<(String, Account), (Account, bool)> {
        let today = chrono::Local::now().date_naive();
        let (id, new) = match self.by_email(correo) {
            Some(a) => (a.id.clone(), false),
            None => {
                let id = random_hex(8);
                let a = Account {
                    id: id.clone(),
                    correo: correo.to_lowercase(),
                    nombre: nombre.to_string(),
                    plan: "prueba".into(),
                    prueba_hasta: String::new(),
                    limite: 0,
                    creado: today.format("%Y-%m-%d").to_string(),
                    clave: String::new(),
                    estado: "pendiente".into(),
                };
                self.cuentas.insert(id.clone(), a);
                (id, true)
            }
        };
        let acc = self.cuentas[&id].clone();
        if !acc.estado.is_empty() {
            return Err((acc, new));
        }
        let token = format!("ns-{}", random_hex(24));
        self.sesiones.insert(fingerprint(&token), id.clone());
        Ok((token, acc))
    }

    /// Las cuentas que esperan aprobación.
    pub fn pending(&self) -> Vec<&Account> {
        self.cuentas.values().filter(|a| a.estado == "pendiente").collect()
    }

    /// Aprueba una cuenta: queda activa y empieza su prueba (si su plan es «prueba»).
    pub fn approve(&mut self, correo: &str, trial_days: i64) -> Option<Account> {
        let id = self.by_email(correo)?.id.clone();
        let a = self.cuentas.get_mut(&id)?;
        a.estado.clear();
        if a.plan == "prueba" {
            a.prueba_hasta = (chrono::Local::now().date_naive() + chrono::Duration::days(trial_days)).format("%Y-%m-%d").to_string();
        }
        Some(a.clone())
    }

    /// Rechaza una cuenta (no podrá entrar) y cierra sus sesiones.
    pub fn reject(&mut self, correo: &str) -> Option<Account> {
        let id = self.by_email(correo)?.id.clone();
        self.sesiones.retain(|_, v| *v != id);
        let a = self.cuentas.get_mut(&id)?;
        a.estado = "rechazada".into();
        Some(a.clone())
    }
}

/// Manda un correo (Resend). Sin clave configurada, lo deja en el registro. Devuelve si salió.
pub async fn send_mail(settings: &crate::Settings, http: &reqwest::Client, to: &str, subject: &str, text: &str) -> bool {
    if settings.mail_key.is_empty() {
        println!("correo para {to}: {subject}\n{text}");
        return true;
    }
    let sent = http
        .post(&settings.mail_api)
        .bearer_auth(&settings.mail_key)
        .json(&serde_json::json!({ "from": settings.mail_from, "to": [to], "subject": subject, "text": text }))
        .send()
        .await;
    let ok = sent.as_ref().is_ok_and(|r| r.status().is_success());
    if !ok {
        eprintln!("no se pudo enviar el correo a {to}: {:?}", sent.map(|r| r.status()));
    }
    ok
}

/// La respuesta para una cuenta que aún no puede entrar.
fn not_yet(acc: &Account) -> Response {
    if acc.estado == "rechazada" {
        return error(StatusCode::FORBIDDEN, "Tu cuenta de Notas no fue aprobada.");
    }
    let msg = "Tu cuenta quedó pendiente de aprobación. Te avisaremos por correo cuando esté lista.";
    (StatusCode::ACCEPTED, Json(serde_json::json!({ "pendiente": true, "mensaje": msg }))).into_response()
}

/// La huella de una clave (Argon2id, con sal al azar).
pub fn hash_password(clave: &str) -> Option<String> {
    use argon2::password_hash::{PasswordHasher, SaltString};
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).ok()?;
    let salt = SaltString::encode_b64(&salt).ok()?;
    argon2::Argon2::default().hash_password(clave.as_bytes(), &salt).ok().map(|h| h.to_string())
}

/// ¿La clave corresponde a la huella?
pub fn check_password(clave: &str, hash: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    !hash.is_empty() && PasswordHash::new(hash).is_ok_and(|h| argon2::Argon2::default().verify_password(clave.as_bytes(), &h).is_ok())
}

pub fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn valid_email(s: &str) -> bool {
    let s = s.trim();
    s.len() <= 254 && s.split_once('@').is_some_and(|(a, d)| !a.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.')) && !s.contains(char::is_whitespace)
}

/// Lo que la app ve de su cuenta.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccountInfo {
    pub correo: String,
    pub nombre: String,
    pub plan: String,
    pub prueba_hasta: String,
    /// Días que le quedan a la prueba (0 si terminó; no aplica en otros planes).
    pub dias_prueba: i64,
}

impl AccountInfo {
    pub fn of(a: &Account) -> AccountInfo {
        let left = chrono::NaiveDate::parse_from_str(&a.prueba_hasta, "%Y-%m-%d")
            .map(|d| (d - chrono::Local::now().date_naive()).num_days().max(0))
            .unwrap_or(0);
        AccountInfo { correo: a.correo.clone(), nombre: a.nombre.clone(), plan: a.plan.clone(), prueba_hasta: a.prueba_hasta.clone(), dias_prueba: left }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignedIn {
    pub token: String,
    pub cuenta: AccountInfo,
}

/// Códigos enviados por correo que aún no se usan, y claves equivocadas (solo en memoria).
#[derive(Default)]
pub struct Pending {
    codes: BTreeMap<String, (String, Instant, u32)>,
    fails: BTreeMap<String, (u32, Instant)>,
}

pub(crate) fn bearer(headers: &HeaderMap) -> String {
    headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// La cuenta de la sesión que viene en `Authorization`.
pub(crate) async fn account_of(state: &AppState, headers: &HeaderMap) -> Result<Account, Response> {
    let fp = fingerprint(&bearer(headers));
    let mut store = state.store.lock().await;
    store.refresh_accounts(&state.settings.data);
    store
        .accounts
        .sesiones
        .get(&fp)
        .and_then(|id| store.accounts.cuentas.get(id))
        .cloned()
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Tu sesión de Notas terminó: vuelve a entrar en Configuración → Tu cuenta."))
}

#[derive(Deserialize)]
pub struct CodeRequest {
    correo: String,
}

/// Manda un código de 6 dígitos al correo.
pub async fn send_code(State(state): State<Arc<AppState>>, Json(r): Json<CodeRequest>) -> Response {
    let correo = r.correo.trim().to_lowercase();
    if !valid_email(&correo) {
        return error(StatusCode::BAD_REQUEST, "Ese correo no parece válido.");
    }
    let code = {
        let mut p = state.pending.lock().await;
        if p.codes.get(&correo).is_some_and(|(_, at, _)| at.elapsed() < CODE_EVERY) {
            return error(StatusCode::TOO_MANY_REQUESTS, "Ya te enviamos un código hace un momento; revisa tu correo (y la carpeta de spam).");
        }
        let mut b = [0u8; 4];
        let _ = getrandom::fill(&mut b);
        let code = format!("{:06}", u32::from_le_bytes(b) % 1_000_000);
        p.codes.insert(correo.clone(), (fingerprint(&code), Instant::now(), 0));
        code
    };
    let text = format!("Tu código para entrar a Notas es {code}\n\nVence en 10 minutos. Si no lo pediste, ignora este correo.");
    // (Sin servicio de correo configurado, el código queda en el registro.)
    if !send_mail(&state.settings, &state.http, &correo, &format!("Tu código de Notas: {code}"), &text).await {
        return error(StatusCode::BAD_GATEWAY, "No se pudo enviar el correo con el código; inténtalo de nuevo en un rato.");
    }
    Json(serde_json::json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
pub struct CodeLogin {
    correo: String,
    codigo: String,
}

/// Revisa (y gasta) el código enviado a ese correo.
async fn use_code(state: &AppState, correo: &str, codigo: &str) -> Result<(), Response> {
    let mut p = state.pending.lock().await;
    let Some((fp, at, tries)) = p.codes.get_mut(correo) else {
        return Err(error(StatusCode::UNAUTHORIZED, "Primero pide un código para ese correo."));
    };
    if at.elapsed() > CODE_TTL || *tries >= CODE_TRIES {
        p.codes.remove(correo);
        return Err(error(StatusCode::UNAUTHORIZED, "Ese código venció; pide otro."));
    }
    if *fp != fingerprint(codigo.trim()) {
        *tries += 1;
        return Err(error(StatusCode::UNAUTHORIZED, "El código no coincide; revísalo."));
    }
    p.codes.remove(correo);
    Ok(())
}

pub async fn sign_in_code(State(state): State<Arc<AppState>>, Json(r): Json<CodeLogin>) -> Response {
    let correo = r.correo.trim().to_lowercase();
    if let Err(e) = use_code(&state, &correo, &r.codigo).await {
        return e;
    }
    finish_sign_in(&state, &correo, "").await
}

#[derive(Deserialize)]
pub struct PasswordLogin {
    correo: String,
    clave: String,
    #[serde(default)]
    nombre: String,
}

fn short_password(clave: &str) -> Option<Response> {
    (clave.chars().count() < MIN_PASSWORD).then(|| error(StatusCode::BAD_REQUEST, &format!("La clave debe tener al menos {MIN_PASSWORD} caracteres.")))
}

/// Crear una cuenta con correo y clave (con la prueba de 14 días) y entrar.
pub async fn register(State(state): State<Arc<AppState>>, Json(r): Json<PasswordLogin>) -> Response {
    let correo = r.correo.trim().to_lowercase();
    if !valid_email(&correo) {
        return error(StatusCode::BAD_REQUEST, "Ese correo no parece válido.");
    }
    if let Some(e) = short_password(&r.clave) {
        return e;
    }
    {
        let mut store = state.store.lock().await;
        store.refresh_accounts(&state.settings.data);
        if let Some(a) = store.accounts.by_email(&correo) {
            return error(
                StatusCode::CONFLICT,
                if a.clave.is_empty() {
                    "Ya hay una cuenta con ese correo (entraste antes con un código o con Microsoft): usa «¿Olvidaste tu clave?» para ponerle una."
                } else {
                    "Ya hay una cuenta con ese correo: inicia sesión."
                },
            );
        }
    }
    let Some(hash) = hash_password(&r.clave) else { return error(StatusCode::INTERNAL_SERVER_ERROR, "No se pudo guardar la clave; inténtalo de nuevo.") };
    finish_sign_in_with(&state, &correo, r.nombre.trim(), Some(hash)).await
}

/// Entrar con correo y clave.
pub async fn sign_in_password(State(state): State<Arc<AppState>>, Json(r): Json<PasswordLogin>) -> Response {
    let correo = r.correo.trim().to_lowercase();
    {
        let p = state.pending.lock().await;
        if p.fails.get(&correo).is_some_and(|(n, at)| *n >= MAX_FAILS && at.elapsed() < FAILS_WINDOW) {
            return error(StatusCode::TOO_MANY_REQUESTS, "Demasiados intentos con esa cuenta; espera unos minutos o usa «¿Olvidaste tu clave?».");
        }
    }
    let hash = {
        let mut store = state.store.lock().await;
        store.refresh_accounts(&state.settings.data);
        store.accounts.by_email(&correo).map(|a| a.clave.clone()).unwrap_or_default()
    };
    if !check_password(&r.clave, &hash) {
        let mut p = state.pending.lock().await;
        let e = p.fails.entry(correo).or_insert((0, Instant::now()));
        if e.1.elapsed() >= FAILS_WINDOW {
            *e = (0, Instant::now());
        }
        e.0 += 1;
        return error(StatusCode::UNAUTHORIZED, "Correo o clave incorrectos.");
    }
    state.pending.lock().await.fails.remove(&correo);
    finish_sign_in(&state, &correo, "").await
}

#[derive(Deserialize)]
pub struct PasswordReset {
    correo: String,
    codigo: String,
    clave: String,
}

/// «¿Olvidaste tu clave?»: con el código que llegó al correo, una clave nueva (si no había
/// cuenta, se crea) y adentro.
pub async fn reset_password(State(state): State<Arc<AppState>>, Json(r): Json<PasswordReset>) -> Response {
    let correo = r.correo.trim().to_lowercase();
    if let Some(e) = short_password(&r.clave) {
        return e;
    }
    if let Err(e) = use_code(&state, &correo, &r.codigo).await {
        return e;
    }
    let Some(hash) = hash_password(&r.clave) else { return error(StatusCode::INTERNAL_SERVER_ERROR, "No se pudo guardar la clave; inténtalo de nuevo.") };
    state.pending.lock().await.fails.remove(&correo);
    finish_sign_in_with(&state, &correo, "", Some(hash)).await
}

#[derive(Deserialize)]
pub struct MicrosoftLogin {
    token: String,
}

/// Entrar con Microsoft: la app entrega un permiso de Microsoft Graph y aquí se pregunta de quién es.
pub async fn sign_in_microsoft(State(state): State<Arc<AppState>>, Json(r): Json<MicrosoftLogin>) -> Response {
    let me = state.http.get(format!("{}/me", state.settings.graph)).bearer_auth(r.token.trim()).send().await;
    let v: serde_json::Value = match me {
        Ok(resp) if resp.status().is_success() => resp.json().await.unwrap_or_default(),
        _ => return error(StatusCode::UNAUTHORIZED, "Microsoft no confirmó tu cuenta; inténtalo de nuevo."),
    };
    let correo = v["mail"].as_str().filter(|m| !m.is_empty()).or(v["userPrincipalName"].as_str()).unwrap_or("").to_lowercase();
    if !valid_email(&correo) {
        return error(StatusCode::UNAUTHORIZED, "Tu cuenta de Microsoft no tiene un correo; entra con tu correo.");
    }
    finish_sign_in(&state, &correo, v["displayName"].as_str().unwrap_or("")).await
}

async fn finish_sign_in(state: &AppState, correo: &str, nombre: &str) -> Response {
    finish_sign_in_with(state, correo, nombre, None).await
}

/// Entra (creando la cuenta si no existe) y, si viene, le pone esa clave.
async fn finish_sign_in_with(state: &AppState, correo: &str, nombre: &str, clave: Option<String>) -> Response {
    let mut store = state.store.lock().await;
    store.refresh_accounts(&state.settings.data);
    let result = store.accounts.sign_in(correo, nombre);
    let id = match &result {
        Ok((_, a)) | Err((a, _)) => a.id.clone(),
    };
    if let Some(hash) = clave {
        if let Some(a) = store.accounts.cuentas.get_mut(&id) {
            a.clave = hash;
        }
    }
    let (token, acc) = match result {
        Ok(ok) => ok,
        Err((acc, new)) => {
            if let Err(e) = store.accounts.save(&state.settings.data) {
                eprintln!("no se pudo guardar cuentas.json: {e}");
            }
            store.accounts_mtime = crate::file_stamp(&Accounts::file(&state.settings.data));
            drop(store);
            // Una cuenta nueva: aviso para aprobarla.
            if new && !state.settings.admin.is_empty() {
                let text = format!("{correo} creó una cuenta en Notas.\n\nPara aprobarla, en el servidor:\n  nodex-ia aprobar {correo}\n\n(o: nodex-ia rechazar {correo})");
                send_mail(&state.settings, &state.http, &state.settings.admin, &format!("Notas: cuenta nueva por aprobar ({correo})"), &text).await;
            }
            return not_yet(&acc);
        }
    };
    if let Err(e) = store.accounts.save(&state.settings.data) {
        eprintln!("no se pudo guardar cuentas.json: {e}");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "No se pudo guardar tu cuenta; inténtalo de nuevo.");
    }
    store.accounts_mtime = crate::file_stamp(&Accounts::file(&state.settings.data));
    Json(SignedIn { token, cuenta: AccountInfo::of(&acc) }).into_response()
}

pub async fn me(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    match account_of(&state, &headers).await {
        Ok(a) => Json(AccountInfo::of(&a)).into_response(),
        Err(r) => r,
    }
}

/// Cierra la sesión de este equipo.
pub async fn sign_out(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let fp = fingerprint(&bearer(&headers));
    let mut store = state.store.lock().await;
    if store.accounts.sesiones.remove(&fp).is_some() {
        let _ = store.accounts.save(&state.settings.data);
    }
    Json(serde_json::json!({ "ok": true })).into_response()
}

/// La configuración cifrada de la cuenta: `version` sube con cada cambio.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Blob {
    pub version: u64,
    pub datos: String,
}

fn blob_file(data: &Path, id: &str) -> PathBuf {
    data.join("config").join(format!("{id}.json"))
}

pub async fn get_config(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let acc = match account_of(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let blob: Blob = std::fs::read_to_string(blob_file(&state.settings.data, &acc.id)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    Json(blob).into_response()
}

/// Guarda la configuración si nadie la cambió desde la versión que leyó la app (si no, 409 y la
/// app vuelve a leer, junta y reintenta).
pub async fn put_config(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(b): Json<Blob>) -> Response {
    let acc = match account_of(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if b.datos.len() > 1_000_000 {
        return error(StatusCode::PAYLOAD_TOO_LARGE, "La configuración es demasiado grande.");
    }
    let _guard = state.store.lock().await; // una escritura a la vez
    let file = blob_file(&state.settings.data, &acc.id);
    let current: Blob = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if current.version != b.version {
        return (StatusCode::CONFLICT, Json(current)).into_response();
    }
    let new = Blob { version: current.version + 1, datos: b.datos };
    match write_atomic(&file, &serde_json::to_string(&new).unwrap_or_default()) {
        Ok(()) => Json(Blob { version: new.version, datos: String::new() }).into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &format!("No se pudo guardar: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Settings, router, state};
    use axum::Router;
    use axum::routing::{get, post};
    use tokio::sync::Mutex;

    /// Entrar con un código por correo (y con Microsoft), la prueba de 14 días con IA incluida, y
    /// la configuración cifrada que se guarda sin pisar la de otro equipo.
    #[tokio::test]
    async fn sign_in_trial_and_config() {
        // Correo, Microsoft Graph e IA, falsos.
        let mails: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
        let m2 = mails.clone();
        let fake = Router::new()
            .route("/correo", post(move |Json(v): Json<serde_json::Value>| {
                let m = m2.clone();
                async move {
                    m.lock().await.push(v);
                    Json(serde_json::json!({"id": "1"}))
                }
            }))
            .route("/graph/me", get(|headers: HeaderMap| async move {
                if bearer(&headers) == "permiso-ms" {
                    Json(serde_json::json!({"displayName": "Ana Pérez", "mail": null, "userPrincipalName": "Ana@Outlook.com"})).into_response()
                } else {
                    StatusCode::UNAUTHORIZED.into_response()
                }
            }))
            .route("/v1/chat/completions", post(|| async {
                Json(serde_json::json!({"choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 5, "completion_tokens": 5}}))
            }));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fake_url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, fake).await.unwrap() });

        let data = std::env::temp_dir().join(format!("nodex-ia-cuentas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let settings = Settings {
            upstream: format!("{fake_url}/v1/"),
            key: "k".into(),
            data: data.clone(),
            mail_api: format!("{fake_url}/correo"),
            mail_key: "clave-correo".into(),
            mail_from: "Notas <hola@notas.cl>".into(),
            graph: format!("{fake_url}/graph"),
            admin: "jp@notas.cl".into(),
            ..Default::default()
        };
        // Juan ya tiene una cuenta aprobada (la de Ana, por Microsoft, será nueva).
        let mut seed = Accounts::default();
        seed.cuentas.insert("j1".into(), Account { id: "j1".into(), correo: "juan@obra.cl".into(), plan: "prueba".into(), estado: "pendiente".into(), ..Default::default() });
        seed.approve("juan@obra.cl", 14).unwrap();
        seed.save(&data).unwrap();
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, router(state(settings))).await.unwrap() });
        let http = reqwest::Client::new();

        // Correo: pedir el código, equivocarse y entrar.
        let r = http.post(format!("{base}/v1/cuenta/codigo")).json(&serde_json::json!({"correo": "juan@obra.cl"})).send().await.unwrap();
        assert_eq!(r.status(), 200);
        let text = mails.lock().await[0]["text"].as_str().unwrap().to_string();
        let code: String = text.chars().filter(|c| c.is_ascii_digit()).take(6).collect();
        assert_eq!(mails.lock().await[0]["to"][0], "juan@obra.cl");
        let again = http.post(format!("{base}/v1/cuenta/codigo")).json(&serde_json::json!({"correo": "juan@obra.cl"})).send().await.unwrap();
        assert_eq!(again.status(), 429, "no se manda otro código enseguida");
        let bad = http.post(format!("{base}/v1/cuenta/entrar")).json(&serde_json::json!({"correo": "juan@obra.cl", "codigo": "000000x"})).send().await.unwrap();
        assert_eq!(bad.status(), 401);
        let ok: SignedIn = http
            .post(format!("{base}/v1/cuenta/entrar"))
            .json(&serde_json::json!({"correo": "Juan@Obra.cl", "codigo": code}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!((ok.cuenta.plan.as_str(), ok.cuenta.dias_prueba), ("prueba", 14));
        // El código ya se usó.
        let reuse = http.post(format!("{base}/v1/cuenta/entrar")).json(&serde_json::json!({"correo": "juan@obra.cl", "codigo": code})).send().await.unwrap();
        assert_eq!(reuse.status(), 401);

        // La sesión sirve para la IA incluida (la prueba la incluye).
        let chat = http
            .post(format!("{base}/v1/chat/completions"))
            .bearer_auth(&ok.token)
            .json(&serde_json::json!({"model": "x", "messages": []}))
            .send()
            .await
            .unwrap();
        assert_eq!(chat.status(), 200);
        let me: AccountInfo = http.get(format!("{base}/v1/cuenta")).bearer_auth(&ok.token).send().await.unwrap().json().await.unwrap();
        assert_eq!(me.correo, "juan@obra.cl");

        // Configuración: la primera escritura (versión 0), otra que llega tarde choca (409).
        let put = |v: u64, d: &str| http.put(format!("{base}/v1/cuenta/config")).bearer_auth(&ok.token).json(&Blob { version: v, datos: d.into() }).send();
        assert_eq!(put(0, "cifrado-1").await.unwrap().json::<Blob>().await.unwrap().version, 1);
        let late = put(0, "cifrado-viejo").await.unwrap();
        assert_eq!(late.status(), 409);
        assert_eq!(late.json::<Blob>().await.unwrap(), Blob { version: 1, datos: "cifrado-1".into() });
        let got: Blob = http.get(format!("{base}/v1/cuenta/config")).bearer_auth(&ok.token).send().await.unwrap().json().await.unwrap();
        assert_eq!(got.datos, "cifrado-1");

        // Microsoft con un correo nuevo: la cuenta queda pendiente y se avisa para aprobarla.
        let first = http.post(format!("{base}/v1/cuenta/microsoft")).json(&serde_json::json!({"token": "permiso-ms"})).send().await.unwrap();
        assert_eq!(first.status(), 202);
        assert_eq!(first.json::<serde_json::Value>().await.unwrap()["pendiente"], true);
        assert!(mails.lock().await.iter().any(|m| m["to"][0] == "jp@notas.cl" && m["text"].as_str().unwrap().contains("nodex-ia aprobar ana@outlook.com")));
        let mut acc = Accounts::load(&data);
        assert_eq!(acc.pending().len(), 1);
        acc.approve("ana@outlook.com", 14).unwrap();
        acc.save(&data).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Aprobada: entra.
        let ms: SignedIn = http.post(format!("{base}/v1/cuenta/microsoft")).json(&serde_json::json!({"token": "permiso-ms"})).send().await.unwrap().json().await.unwrap();
        assert_eq!((ms.cuenta.correo.as_str(), ms.cuenta.nombre.as_str()), ("ana@outlook.com", "Ana Pérez"));
        let bad_ms = http.post(format!("{base}/v1/cuenta/microsoft")).json(&serde_json::json!({"token": "otro"})).send().await.unwrap();
        assert_eq!(bad_ms.status(), 401);

        // Prueba vencida: la IA ya no, con un mensaje claro; con el plan pro, sí.
        let mut acc = Accounts::load(&data);
        let id = acc.by_email("juan@obra.cl").unwrap().id.clone();
        acc.cuentas.get_mut(&id).unwrap().prueba_hasta = "2020-01-01".into();
        acc.save(&data).unwrap();
        // (el servidor relee cuentas.json al cambiar; se espera un poco por la precisión de la fecha del archivo)
        tokio::time::sleep(Duration::from_millis(50)).await;
        let chat = || http.post(format!("{base}/v1/chat/completions")).bearer_auth(&ok.token).json(&serde_json::json!({"messages": []})).send();
        let r = chat().await.unwrap();
        assert_eq!(r.status(), 402);
        assert!(r.text().await.unwrap().contains("Tu prueba de IA incluida terminó"));
        acc.cuentas.get_mut(&id).unwrap().plan = "pro".into();
        acc.save(&data).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(chat().await.unwrap().status(), 200);

        // Salir: la sesión deja de servir.
        http.post(format!("{base}/v1/cuenta/salir")).bearer_auth(&ok.token).send().await.unwrap();
        assert_eq!(http.get(format!("{base}/v1/cuenta")).bearer_auth(&ok.token).send().await.unwrap().status(), 401);
        let _ = std::fs::remove_dir_all(&data);
    }
}
