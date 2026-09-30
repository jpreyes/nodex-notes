//! Cuentas: entrar con el correo (un código de 6 dígitos que llega por correo) o con Microsoft,
//! el plan de cada cuenta (prueba de 14 días, pro, fundador, gratis) y la configuración que
//! viaja con la cuenta.
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

    /// La cuenta de ese correo (la crea, con la prueba, si no existe) y un token de sesión nuevo.
    fn sign_in(&mut self, correo: &str, nombre: &str, trial_days: i64) -> (String, Account) {
        let today = chrono::Local::now().date_naive();
        let id = match self.by_email(correo) {
            Some(a) => a.id.clone(),
            None => {
                let id = random_hex(8);
                let a = Account {
                    id: id.clone(),
                    correo: correo.to_lowercase(),
                    nombre: nombre.to_string(),
                    plan: "prueba".into(),
                    prueba_hasta: (today + chrono::Duration::days(trial_days)).format("%Y-%m-%d").to_string(),
                    limite: 0,
                    creado: today.format("%Y-%m-%d").to_string(),
                };
                self.cuentas.insert(id.clone(), a);
                id
            }
        };
        let token = format!("ns-{}", random_hex(24));
        self.sesiones.insert(fingerprint(&token), id.clone());
        let acc = self.cuentas[&id].clone();
        (token, acc)
    }
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

/// Códigos enviados por correo que aún no se usan (solo en memoria).
#[derive(Default)]
pub struct Pending {
    codes: BTreeMap<String, (String, Instant, u32)>,
}

pub(crate) fn bearer(headers: &HeaderMap) -> String {
    headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// La cuenta de la sesión que viene en `Authorization`.
async fn account_of(state: &AppState, headers: &HeaderMap) -> Result<Account, Response> {
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
    let s = &state.settings;
    if s.mail_key.is_empty() {
        // Sin servicio de correo configurado (desarrollo): el código queda en el registro.
        println!("código para {correo}: {code}");
    } else {
        let sent = state
            .http
            .post(&s.mail_api)
            .bearer_auth(&s.mail_key)
            .json(&serde_json::json!({ "from": s.mail_from, "to": [correo], "subject": format!("Tu código de Notas: {code}"), "text": text }))
            .send()
            .await;
        if !sent.as_ref().is_ok_and(|r| r.status().is_success()) {
            eprintln!("no se pudo enviar el código a {correo}: {:?}", sent.map(|r| r.status()));
            return error(StatusCode::BAD_GATEWAY, "No se pudo enviar el correo con el código; inténtalo de nuevo en un rato.");
        }
    }
    Json(serde_json::json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
pub struct CodeLogin {
    correo: String,
    codigo: String,
}

pub async fn sign_in_code(State(state): State<Arc<AppState>>, Json(r): Json<CodeLogin>) -> Response {
    let correo = r.correo.trim().to_lowercase();
    {
        let mut p = state.pending.lock().await;
        let Some((fp, at, tries)) = p.codes.get_mut(&correo) else {
            return error(StatusCode::UNAUTHORIZED, "Primero pide un código para ese correo.");
        };
        if at.elapsed() > CODE_TTL || *tries >= CODE_TRIES {
            p.codes.remove(&correo);
            return error(StatusCode::UNAUTHORIZED, "Ese código venció; pide otro.");
        }
        if *fp != fingerprint(r.codigo.trim()) {
            *tries += 1;
            return error(StatusCode::UNAUTHORIZED, "El código no coincide; revísalo.");
        }
        p.codes.remove(&correo);
    }
    finish_sign_in(&state, &correo, "").await
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
    let mut store = state.store.lock().await;
    store.refresh_accounts(&state.settings.data);
    let (token, acc) = store.accounts.sign_in(correo, nombre, state.settings.trial_days);
    if let Err(e) = store.accounts.save(&state.settings.data) {
        eprintln!("no se pudo guardar cuentas.json: {e}");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "No se pudo guardar tu cuenta; inténtalo de nuevo.");
    }
    store.accounts_mtime = std::fs::metadata(Accounts::file(&state.settings.data)).and_then(|m| m.modified()).ok();
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
            ..Default::default()
        };
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

        // Microsoft: la misma cuenta si el correo coincide; otra si no.
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
