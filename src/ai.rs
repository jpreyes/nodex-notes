//! Análisis de notas con IA (genai) en un hilo aparte, para que la ventana nunca se congele.

use crate::config::Config;
use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest};
use genai::resolver::{AuthData, AuthResolver, Endpoint};
use genai::{Client, ModelIden, ModelSpec, ServiceTarget};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

/// Hash estable del contenido (FNV-1a), para recordar qué ya se analizó.
pub fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AiTask {
    pub texto: String,
    pub fecha: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AiEvent {
    pub titulo: String,
    pub fecha: String,
    pub hora: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Analysis {
    pub es_reunion: bool,
    pub espacio: String,
    pub confianza: String,
    pub titulo: String,
    pub etiquetas: Vec<String>,
    pub tareas: Vec<AiTask>,
    pub eventos: Vec<AiEvent>,
    pub resumen: String,
}

pub struct Job {
    pub path: PathBuf,
    pub hash: u64,
    pub system: String,
    pub user: String,
}

pub struct JobResult {
    pub path: PathBuf,
    pub hash: u64,
    pub result: Result<Analysis, String>,
}

pub struct Ai {
    tx: Sender<Job>,
    pub rx: Receiver<JobResult>,
    pub busy: bool,
    pub label: String,
}

/// Cómo hablar con cada proveedor.
struct Provider {
    kind: AdapterKind,
    /// URL base propia (servicios compatibles con la API de OpenAI).
    endpoint: Option<&'static str>,
    /// Variable de entorno con la clave, si no está en config.toml.
    env: Option<&'static str>,
}

fn provider(proveedor: &str) -> Result<Provider, String> {
    let p = |kind, endpoint, env| Ok(Provider { kind, endpoint, env });
    match proveedor.trim().to_lowercase().as_str() {
        // OpenCode Zen (pago por uso): API compatible con OpenAI (chat/completions).
        "opencode" | "opencode.ai" | "opencode-zen" | "zen" => {
            p(AdapterKind::OpenAI, Some("https://opencode.ai/zen/v1/"), Some("OPENCODE_API_KEY"))
        }
        // OpenCode Go (suscripción mensual): misma API y clave, otra dirección.
        "opencode-go" | "go" => p(AdapterKind::OpenAI, Some("https://opencode.ai/zen/go/v1/"), Some("OPENCODE_API_KEY")),
        "anthropic" | "claude" => p(AdapterKind::Anthropic, None, Some("ANTHROPIC_API_KEY")),
        "openai" | "gpt" => p(AdapterKind::OpenAI, None, Some("OPENAI_API_KEY")),
        "gemini" | "google" => p(AdapterKind::Gemini, None, Some("GEMINI_API_KEY")),
        "ollama" => p(AdapterKind::Ollama, None, None),
        other => Err(format!(
            "Proveedor desconocido «{other}» (usa opencode, opencode-go, anthropic, openai, gemini u ollama)"
        )),
    }
}

/// Proveedores para la ventana de Configuración: (id en config.toml, nombre, modelos sugeridos, dónde sacar la clave).
/// Los modelos de OpenCode son los que su documentación lista para chat/completions.
pub const PROVIDERS: &[(&str, &str, &[&str], Option<&str>)] = &[
    ("opencode", "OpenCode Zen (pago por uso)", &["deepseek-v4.1-flash", "deepseek-v4-flash", "deepseek-v4-pro"], Some("https://opencode.ai/zen")),
    ("opencode-go", "OpenCode Go (suscripción)", &["deepseek-v4.1-flash", "kimi-k3", "glm-5.3-flash"], Some("https://opencode.ai/zen")),
    ("anthropic", "Anthropic (Claude)", &["claude-haiku-4-5", "claude-sonnet-5"], None),
    ("openai", "OpenAI", &[], None),
    ("gemini", "Google Gemini", &[], None),
    ("ollama", "Ollama (en este equipo)", &[], None),
];

/// Identificador de esta sesión de la app (uno por ejecución), para `x-opencode-session`.
fn session_id() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        let mut b = [0u8; 12];
        let _ = getrandom::fill(&mut b);
        format!("nodex-notes-{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
    })
}

/// Cabeceras que se envían en cada petición: la app se identifica con su nombre y versión
/// y, con OpenCode, manda un ID de sesión estable (OpenCode Go lo exige para enrutar la petición).
fn request_headers(cfg: &Config) -> Vec<(String, String)> {
    let mut h = vec![("User-Agent".to_string(), format!("nodex-notes/{}", env!("CARGO_PKG_VERSION")))];
    if cfg.proveedor.trim().to_lowercase().starts_with("opencode") || matches!(cfg.proveedor.trim(), "go" | "zen") {
        h.push(("x-opencode-session".to_string(), session_id().to_string()));
    }
    h
}

/// Cliente y destino (URL, clave y modelo) según la configuración.
fn connection(cfg: &Config) -> Result<(Client, ModelSpec), String> {
    let prov = provider(&cfg.proveedor)?;
    let key = Some(cfg.clave_api.trim().to_string())
        .filter(|k| !k.is_empty())
        .or_else(|| prov.env.and_then(|v| std::env::var(v).ok()).filter(|v| !v.is_empty()));
    if key.is_none() && prov.kind != AdapterKind::Ollama {
        return Err("Falta la clave API (Configuración → Inteligencia artificial)".into());
    }
    if cfg.modelo.trim().is_empty() {
        return Err("Falta elegir el modelo".into());
    }
    let iden = ModelIden::new(prov.kind, cfg.modelo.trim().to_string());
    // Con URL propia se usa un destino fijo (URL + clave + modelo); si no, genai resuelve el proveedor.
    let model: ModelSpec = match (prov.endpoint, &key) {
        (Some(url), Some(k)) => ServiceTarget {
            endpoint: Endpoint::from_static(url),
            auth: AuthData::from_single(k.clone()),
            model: iden,
        }
        .into(),
        _ => iden.into(),
    };
    let mut builder = Client::builder();
    if let Some(k) = key {
        builder = builder.with_auth_resolver(AuthResolver::from_resolver_fn(
            move |_: ModelIden| -> Result<Option<AuthData>, genai::resolver::Error> {
                Ok(Some(AuthData::from_single(k.clone())))
            },
        ));
    }
    Ok((builder.build(), model))
}

/// Prueba la conexión con un mensaje mínimo; devuelve los milisegundos que tardó o el motivo del error.
pub fn test_connection(cfg: &Config, ctx: eframe::egui::Context) -> Receiver<Result<u128, String>> {
    let (tx, rx) = mpsc::channel();
    let conn = connection(cfg);
    let headers = request_headers(cfg);
    std::thread::spawn(move || {
        let result = conn.and_then(|(client, model)| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
            let start = std::time::Instant::now();
            let req = ChatRequest::default().append_message(ChatMessage::user("Responde solo: ok"));
            let options = ChatOptions::default().with_max_tokens(20).with_extra_headers(headers);
            rt.block_on(client.exec_chat(model, req, Some(&options))).map_err(|e| friendly_error(&e.to_string()))?;
            Ok(start.elapsed().as_millis())
        });
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    rx
}

impl Ai {
    /// Inicia el hilo de IA. Devuelve un error legible si falta configuración.
    pub fn start(cfg: &Config, ctx: eframe::egui::Context) -> Result<Ai, String> {
        let (client, model) = connection(cfg)?;
        let label = format!("{} · {}", cfg.proveedor, cfg.modelo);
        let headers = request_headers(cfg);
        let (tx, job_rx) = mpsc::channel::<Job>();
        let (res_tx, rx) = mpsc::channel::<JobResult>();

        std::thread::Builder::new()
            .name("ia".into())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                    return;
                };
                let options =
                    ChatOptions::default().with_temperature(0.2).with_max_tokens(2000).with_extra_headers(headers);
                for job in job_rx {
                    let req = ChatRequest::default().with_system(job.system).append_message(ChatMessage::user(job.user));
                    let result = rt
                        .block_on(client.exec_chat(model.clone(), req, Some(&options)))
                        .map_err(|e| friendly_error(&e.to_string()))
                        .and_then(|r| r.into_first_text().ok_or_else(|| "Respuesta vacía".to_string()))
                        .and_then(|text| parse_analysis(&text));
                    let _ = res_tx.send(JobResult { path: job.path, hash: job.hash, result });
                    ctx.request_repaint();
                }
            })
            .map_err(|e| e.to_string())?;

        Ok(Ai { tx, rx, busy: false, label })
    }

    pub fn send(&mut self, job: Job) {
        if self.tx.send(job).is_ok() {
            self.busy = true;
        }
    }
}

/// Resume un error de genai: "401 Unauthorized: Invalid API key." en vez del texto técnico completo.
pub fn friendly_error(e: &str) -> String {
    let between = |start: &str, end: char| {
        let i = e.find(start)? + start.len();
        e[i..].find(end).map(|j| e[i..i + j].to_string())
    };
    match (between("status code '", '\x27'), between("\"message\":\"", '"')) {
        (Some(status), Some(msg)) => format!("{status}: {msg}"),
        (Some(status), None) => status,
        _ => e.lines().next().unwrap_or(e).to_string(),
    }
}

/// Extrae el objeto JSON aunque venga con texto o ``` alrededor.
pub fn parse_analysis(text: &str) -> Result<Analysis, String> {
    let (Some(a), Some(b)) = (text.find('{'), text.rfind('}')) else {
        return Err("La IA no devolvió JSON".into());
    };
    serde_json::from_str(&text[a..=b]).map_err(|e| format!("JSON inválido de la IA: {e}"))
}

pub struct WorkspaceInfo {
    pub name: String,
    pub titles: Vec<String>,
    pub tags: Vec<String>,
}

const DIAS: [&str; 7] = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];

pub fn build_prompt(
    title: &str,
    workspace: &str,
    text: &str,
    workspaces: &[WorkspaceInfo],
    all_tags: &[String],
) -> (String, String) {
    use chrono::Datelike;
    let now = chrono::Local::now();
    let today = format!("{} {}", DIAS[now.weekday().num_days_from_monday() as usize], now.format("%Y-%m-%d"));
    let system = format!(
        r#"Organizas las notas de una persona que escribe en español. Devuelve SOLO un objeto JSON válido, sin texto adicional, con esta forma exacta:
{{"es_reunion": false, "espacio": "", "confianza": "baja", "titulo": "", "etiquetas": [], "tareas": [{{"texto": "", "fecha": ""}}], "eventos": [{{"titulo": "", "fecha": "", "hora": ""}}], "resumen": ""}}

Reglas:
- es_reunion: true si la nota registra una reunión, llamada o conversación con otras personas.
- espacio: el espacio de trabajo al que pertenece TODA la nota, escrito exactamente como en la lista. confianza "alta" solo si es evidente; si la nota mezcla temas de varios espacios o no está claro, deja el espacio actual con confianza "baja".
- titulo: título breve (máximo 6 palabras) que describa la nota, sin fecha.
- etiquetas: de 1 a 4, en minúsculas, una palabra cada una (usa guiones si hace falta), sin '#'. Prefiere etiquetas que ya existen. No repitas las que la nota ya tiene.
- tareas: acciones pendientes concretas que la persona debe hacer, empezando con verbo en infinitivo. Ignora las marcadas como hechas ([x]). "fecha" (AAAA-MM-DD) solo si la nota la indica o se deduce (por ejemplo "el viernes" = la fecha real de ese viernes); si no, "".
- eventos: citas, visitas, entregas o reuniones futuras con fecha concreta (AAAA-MM-DD) y hora (HH:MM) si se indica. No incluyas la reunión que la propia nota registra.
- resumen: si es una reunión, 2 o 3 frases con decisiones y acuerdos; si no, "".
Hoy es {today}."#
    );
    let mut user = String::from("Espacios de trabajo disponibles:\n");
    for w in workspaces {
        user += &format!("- {}", w.name);
        if !w.titles.is_empty() {
            user += &format!(" (notas: {})", w.titles.join(", "));
        }
        if !w.tags.is_empty() {
            user += &format!(" (etiquetas: {})", w.tags.join(", "));
        }
        user.push('\n');
    }
    if !all_tags.is_empty() {
        user += &format!("\nEtiquetas existentes: {}\n", all_tags.join(", "));
    }
    user += &format!("\nEspacio actual: {workspace}\nTítulo actual: {title}\n\nNota:\n<<<\n{text}\n>>>");
    (system, user)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_json() {
        let a = parse_analysis("```json\n{\"es_reunion\": true, \"etiquetas\": [\"vigas\"], \"tareas\": [{\"texto\": \"Enviar planos\", \"fecha\": \"2026-09-26\"}]}\n```").unwrap();
        assert!(a.es_reunion);
        assert_eq!(a.etiquetas, vec!["vigas"]);
        assert_eq!(a.tareas[0].fecha, "2026-09-26");
        assert!(parse_analysis("nada").is_err());
    }

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv(""), 0xcbf29ce484222325);
        assert_ne!(fnv("a"), fnv("b"));
    }
}

#[cfg(test)]
mod net_tests {
    use super::*;

    /// Llama al servidor real de OpenCode Zen con una clave falsa: debe responder
    /// "clave inválida" (y no "ruta o modelo inexistente"). `cargo test -- --ignored opencode`
    #[test]
    #[ignore]
    fn opencode_endpoint_reachable() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        for name in ["opencode", "opencode-go"] {
            let prov = provider(name).unwrap();
            let target = ServiceTarget {
                endpoint: Endpoint::from_static(prov.endpoint.unwrap()),
                auth: AuthData::from_single("clave-falsa"),
                model: ModelIden::new(prov.kind, "deepseek-v4.1-flash"),
            };
            let req = ChatRequest::default().append_message(ChatMessage::user("hola"));
            let err = rt.block_on(Client::default().exec_chat(target, req, None)).unwrap_err().to_string();
            println!("{name}: {}", friendly_error(&err));
            assert_eq!(friendly_error(&err), "401 Unauthorized: Invalid API key.", "{name}: {err}");
        }
    }
}

#[cfg(test)]
mod header_tests {
    use super::*;
    use std::io::{Read, Write};

    /// Un servidor local captura la petición real: debe llevar User-Agent propio y x-opencode-session.
    #[test]
    fn opencode_requests_carry_session_and_user_agent() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = vec![0u8; 16384];
            let n = s.read(&mut buf).unwrap();
            let body = r#"{"id":"x","object":"chat.completion","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
            let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            String::from_utf8_lossy(&buf[..n]).to_lowercase()
        });

        let cfg = Config { proveedor: "opencode-go".into(), ..Config::default() };
        let target = ServiceTarget {
            endpoint: Endpoint::from_owned(format!("http://127.0.0.1:{port}/")),
            auth: AuthData::from_single("k"),
            model: ModelIden::new(AdapterKind::OpenAI, "deepseek-v4.1-flash"),
        };
        let options = ChatOptions::default().with_extra_headers(request_headers(&cfg));
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let req = ChatRequest::default().append_message(ChatMessage::user("hola"));
        let res = rt.block_on(Client::default().exec_chat(target, req, Some(&options))).unwrap();
        assert_eq!(res.first_text(), Some("ok"));

        let raw = server.join().unwrap();
        assert!(raw.contains(&format!("x-opencode-session: {}", session_id())), "{raw}");
        assert!(raw.contains(&format!("user-agent: nodex-notes/{}", env!("CARGO_PKG_VERSION"))), "{raw}");
    }
}
