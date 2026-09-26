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
    /// En una nota de captura: la unidad de donde sale ("L3", "B5").
    pub unidad: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AiEvent {
    pub titulo: String,
    pub fecha: String,
    pub hora: String,
    pub unidad: String,
}

/// Una nota dentro del archivo (línea con sus sangrías, o bloque "##"): sus etiquetas,
/// de qué otra es detalle y, en una nota de captura, a qué nota (y espacio) va.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AiUnit {
    pub id: String,
    pub etiquetas: Vec<String>,
    /// Id de la unidad que esta completa ("L1"); vacío si es independiente.
    pub de: String,
    pub espacio: String,
    pub nota: String,
    pub es_reunion: bool,
    pub resumen: String,
    /// Proyecto o tema concreto que no tiene espacio todavía ("LaVet").
    pub espacio_nuevo: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Analysis {
    // Nota con título: se analiza completa.
    pub es_reunion: bool,
    pub espacio: String,
    pub confianza: String,
    pub titulo: String,
    pub etiquetas: Vec<String>,
    pub resumen: String,
    // Nota de captura: cada unidad por separado.
    pub unidades: Vec<AiUnit>,
    pub tareas: Vec<AiTask>,
    pub eventos: Vec<AiEvent>,
    /// Lo que la IA no sabe con seguridad: una pregunta con opciones.
    pub dudas: Vec<AiDoubt>,
    /// Nota con título: proyecto o tema concreto de toda la nota que no tiene espacio todavía.
    pub espacio_nuevo: String,
}

/// Una pregunta de la IA sobre una unidad. Las opciones pueden venir como texto o como objeto
/// ({"texto", "espacio", "nota", "de", "fecha", "etiquetas", "dato"}); ver `doubts::from_ai`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AiDoubt {
    pub unidad: String,
    pub pregunta: String,
    pub opciones: Vec<serde_json::Value>,
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
    // Para pruebas: otro servidor compatible (nunca se usa en la app normal).
    let test_url = std::env::var("NODEX_AI_ENDPOINT").ok().filter(|u| !u.is_empty() && prov.endpoint.is_some());
    let model: ModelSpec = match (prov.endpoint, &key) {
        (Some(_), Some(k)) if test_url.is_some() => ServiceTarget {
            endpoint: Endpoint::from_owned(test_url.unwrap_or_default()),
            auth: AuthData::from_single(k.clone()),
            model: iden,
        }
        .into(),
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
            let options = ChatOptions::default().with_extra_headers(headers);
            rt.block_on(client.exec_chat(model, req, Some(&options))).map_err(|e| friendly_error(&e.to_string()))?;
            Ok(start.elapsed().as_millis())
        });
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    rx
}

/// Una consulta de texto libre (la usa Preguntar). Bloquea: se llama desde un hilo aparte.
/// Devuelve el texto de la respuesta, o el motivo si vino vacía o falló.
pub fn complete(cfg: &Config, system: &str, user: &str) -> Result<String, String> {
    let (client, model) = connection(cfg)?;
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    let options = ChatOptions::default()
        .with_temperature(0.2)
        .with_normalize_reasoning_content(true)
        .with_extra_headers(request_headers(cfg));
    let req = ChatRequest::default().with_system(system).append_message(ChatMessage::user(user));
    let r = rt.block_on(client.exec_chat(model, req, Some(&options))).map_err(|e| friendly_error(&e.to_string()))?;
    let text = r.first_text().unwrap_or("").trim().to_string();
    if text.is_empty() {
        let stop = r.stop_reason.as_ref().map(|s| format!("{s:?}")).unwrap_or_else(|| "?".into());
        let why = if stop.contains("MaxTokens") { "se cortó por el límite de tokens del modelo".to_string() } else { format!("motivo: {stop}") };
        return Err(format!("el modelo devolvió una respuesta vacía ({why})"));
    }
    Ok(text)
}

impl Ai {
    /// Inicia el hilo de IA. Devuelve un error legible si falta configuración.
    pub fn start(cfg: &Config, ctx: eframe::egui::Context) -> Result<Ai, String> {
        let (client, model) = connection(cfg)?;
        let label = format!("{} · {}", cfg.proveedor, cfg.modelo);
        let headers = request_headers(cfg);
        let (tx, job_rx) = mpsc::channel::<Job>();
        let (res_tx, rx) = mpsc::channel::<JobResult>();
        let log_label = label.clone();

        std::thread::Builder::new()
            .name("ia".into())
            .spawn(move || {
                let label = log_label;
                let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                    return;
                };
                // Sin límite de tokens propio: cada modelo usa los que necesite.
                let options = ChatOptions::default()
                    .with_temperature(0.2)
                    .with_normalize_reasoning_content(true)
                    .with_extra_headers(headers);
                for job in job_rx {
                    let req = ChatRequest::default().with_system(&job.system).append_message(ChatMessage::user(&job.user));
                    let result = match rt.block_on(client.exec_chat(model.clone(), req, Some(&options))) {
                        Ok(r) => {
                            let text = r.first_text().unwrap_or("").trim().to_string();
                            let reasoning = r.reasoning_content.clone().unwrap_or_default();
                            let stop = r.stop_reason.as_ref().map(|s| format!("{s:?}")).unwrap_or_else(|| "?".into());
                            let result = interpret(&text, &reasoning, &stop);
                            log_exchange(&label, &job, &format!("{:?}", r.usage), &stop, &text, &reasoning, &result);
                            result
                        }
                        Err(e) => {
                            let e = friendly_error(&e.to_string());
                            log_exchange(&label, &job, "", "error", "", "", &Err(e.clone()));
                            Err(e)
                        }
                    };
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

/// Saca el análisis de la respuesta; si el texto viene vacío, lo busca en el razonamiento del modelo.
pub fn interpret(text: &str, reasoning: &str, stop: &str) -> Result<Analysis, String> {
    if !text.is_empty() {
        return parse_analysis(text).or_else(|e| parse_analysis(reasoning).map_err(|_| e));
    }
    if let Ok(a) = parse_analysis(reasoning) {
        return Ok(a);
    }
    let why = if stop.contains("MaxTokens") {
        "se cortó por el límite de tokens del modelo".to_string()
    } else if !reasoning.is_empty() {
        format!("solo razonó ({} caracteres) y no respondió", reasoning.chars().count())
    } else {
        format!("motivo: {stop}")
    };
    Err(format!("el modelo devolvió una respuesta vacía ({why}). Detalle en ia-ultima.txt"))
}

/// Guarda el último intercambio con la IA (junto a config.toml, solo en este equipo) para diagnosticar.
fn log_exchange(
    label: &str,
    job: &Job,
    usage: &str,
    stop: &str,
    text: &str,
    reasoning: &str,
    result: &Result<Analysis, String>,
) {
    let cut = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    let outcome = match result {
        Ok(a) => format!("OK: {a:?}"),
        Err(e) => format!("ERROR: {e}"),
    };
    let log = format!(
        "Fecha: {}\nModelo: {label}\nNota: {}\nMotivo de término: {stop}\nUso: {usage}\n\n== Resultado ==\n{outcome}\n\n== Respuesta ==\n{}\n\n== Razonamiento ==\n{}\n\n== Nota enviada ==\n{}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        job.path.display(),
        cut(text, 20_000),
        cut(reasoning, 20_000),
        cut(&job.user, 20_000),
    );
    let _ = std::fs::write(crate::config::config_path().with_file_name("ia-ultima.txt"), log);
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


/// Contexto común: espacios de trabajo, sus notas y etiquetas, y lo que la persona ya aclaró.
fn context(workspaces: &[WorkspaceInfo], all_tags: &[String], learned: &[String]) -> String {
    let mut s = String::from("Espacios de trabajo disponibles:\n");
    for w in workspaces {
        s += &format!("- {}", w.name);
        if !w.titles.is_empty() {
            s += &format!(" (notas: {})", w.titles.join(", "));
        }
        if !w.tags.is_empty() {
            s += &format!(" (etiquetas: {})", w.tags.join(", "));
        }
        s.push('\n');
    }
    if !all_tags.is_empty() {
        s += &format!("\nEtiquetas existentes: {}\n", all_tags.join(", "));
    }
    if !learned.is_empty() {
        s += "\nLo que la persona ya aclaró (úsalo y no vuelvas a preguntarlo):\n";
        for f in learned {
            s += &format!("- {f}\n");
        }
    }
    s
}

fn today() -> String {
    use chrono::Datelike;
    let now = chrono::Local::now();
    format!("{} {}", DIAS[now.weekday().num_days_from_monday() as usize], now.format("%Y-%m-%d"))
}

const RULES_TASKS: &str = r#"- tareas: acciones pendientes concretas que la persona debe hacer ("debo…", "hay que…", "tengo que…"), redactadas con verbo en infinitivo, cada una con el id de la unidad de donde sale. Las líneas "- [ ] …" ya son tareas pendientes: inclúyelas igual (con su unidad). Ignora las hechas ("- [x] …"). "fecha" (AAAA-MM-DD) si se indica o se deduce: "mañana" = el día siguiente a hoy; "el viernes" = la fecha de ese viernes; un plazo vago como "la próxima semana" = el viernes de la próxima semana; "due:AAAA-MM-DD" en la línea es su fecha. Si no hay plazo, "".
- eventos: citas, visitas o reuniones futuras con fecha (AAAA-MM-DD) y hora (HH:MM) si se indica, con el id de su unidad. Las entregas con plazo son tareas, no eventos. No incluyas una reunión que el propio texto está registrando.
- Ignora los "^abc12" del final de algunas líneas: son identificadores internos."#;

const RULES_UNITS: &str = r#"- etiquetas: de 1 a 3 por unidad, sobre el tema de ESA unidad (no de toda la nota). En minúsculas, una palabra (guiones si hace falta), sin '#'. Prefiere etiquetas existentes y no repitas las que la unidad ya tiene.
- de: si una unidad es un detalle de otra anterior (habla de lo mismo y la completa: dónde está, un dato más, una aclaración), el id de esa otra unidad; si es independiente, "". Úsalo solo cuando sea evidente."#;

const RULES_DOUBTS: &str = r#"- dudas: si NO estás seguro de algo que importa, no adivines: no lo apliques y pregunta. Por ejemplo, a qué espacio o nota pertenece una unidad cuando hay dos posibles, si una unidad es detalle de otra, o a qué fecha se refiere un plazo ambiguo. Como máximo 2 preguntas por nota, breves y concretas, cada una con el id de su unidad y de 2 a 4 opciones. Cada opción tiene "texto" (lo que se ve en el botón, corto) y lo que hace al elegirla: "espacio" y "nota" (dónde guardar la unidad), "de" (id de la unidad que completa), "fecha" (AAAA-MM-DD de su tarea), "etiquetas", y "dato": una frase corta con lo que conviene recordar para las próximas notas (por ejemplo "LaVet es un proyecto del espacio Docencia"). No preguntes lo evidente ni lo que la persona ya aclaró. Si no hay dudas, "dudas": []."#;

const RULE_NEW_SPACE: &str = r#"espacio_nuevo: si trata de un proyecto, cliente, obra o tema concreto y recurrente que NO tiene espacio en la lista (por ejemplo, el nombre de un proyecto), ese nombre corto, tal como se llamaría el espacio ("LaVet"); si calza con un espacio existente o es algo general (informes, reuniones, compras, ideas), "". No propongas los que la persona descartó."#;

const DOUBTS_SHAPE: &str = r#""dudas": [{"unidad": "L1", "pregunta": "", "opciones": [{"texto": "", "espacio": "", "nota": "", "de": "", "fecha": "", "etiquetas": [], "dato": ""}]}]"#;

/// Una nota se envía dividida en unidades: cada línea sin sangría (con sus líneas con sangría)
/// y cada bloque "##".
fn list_units(text: &str, units: &[crate::lines::Unit]) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut listed = String::new();
    for u in units {
        if u.block {
            listed += &format!("[{}] (bloque, líneas {} a {})\n", u.id, u.first + 1, u.last + 1);
            for l in &lines[u.first..=u.last] {
                listed += &format!("    {l}\n");
            }
        } else {
            listed += &format!("[{}] {}\n", u.id, lines[u.first]);
            for l in &lines[u.first + 1..=u.last] {
                listed += &format!("    {l}\n");
            }
        }
    }
    listed
}

/// El prompt para una nota. En una nota de captura (la del día, "Sin título") cada unidad
/// puede irse a otra nota; en una con título propio, la nota se analiza completa y sus
/// unidades se quedan donde están.
pub fn build_prompt(
    title: &str,
    workspace: &str,
    text: &str,
    units: &[crate::lines::Unit],
    capture: bool,
    workspaces: &[WorkspaceInfo],
    all_tags: &[String],
    learned: &[String],
) -> (String, String) {
    let intro = r###"Organizas las notas de una persona que escribe en español. Cada línea sin sangría es una nota distinta; las líneas con sangría (y sus ítems "- ") son parte de la nota de arriba; un bloque que empieza con "##" (una reunión o un tema) va junto. Recibes la nota dividida en esas unidades, cada una con su id ("L3" = la que empieza en la línea 3; "B5" = bloque que empieza en la línea 5). Devuelve SOLO un objeto JSON válido, sin texto adicional, con esta forma exacta:"###;
    let system = if capture {
        format!(
            r###"{intro}
{{"unidades": [{{"id": "L1", "etiquetas": [], "de": "", "espacio": "", "nota": "", "espacio_nuevo": "", "es_reunion": false, "resumen": ""}}], "tareas": [{{"texto": "", "fecha": "", "unidad": "L1"}}], "eventos": [{{"titulo": "", "fecha": "", "hora": "", "unidad": "L1"}}], {DOUBTS_SHAPE}}}

Son apuntes rápidos: cada unidad puede guardarse en la nota que le corresponde.
Reglas:
- unidades: incluye cada unidad a la que le pongas etiquetas o le des un lugar.
{RULES_UNITS}
- espacio y nota: dónde guardarla, si claramente pertenece a un tema. "espacio": un espacio de la lista, escrito exactamente igual. "nota": el título exacto de una nota existente de ese espacio si alguna corresponde; si no, un título nuevo y breve (máximo 5 palabras). Las unidades del mismo tema van a la misma nota. En un bloque "##" que no calza con una nota existente, usa como nota el título del bloque (sin fecha). Si no se puede atribuir con seguridad, deja espacio y nota vacíos: se queda donde está. Una unidad con "de" se va junto con la otra.
- {RULE_NEW_SPACE} Si la unidad no tiene espacio adecuado, déjala sin espacio ni nota y usa espacio_nuevo.
- es_reunion y resumen solo para bloques que registran una reunión: 2 o 3 frases con decisiones y acuerdos.
{RULES_TASKS}
{RULES_DOUBTS}
Hoy es {}."###,
            today()
        )
    } else {
        format!(
            r###"{intro}
{{"es_reunion": false, "espacio": "", "confianza": "baja", "espacio_nuevo": "", "titulo": "", "resumen": "", "unidades": [{{"id": "L1", "etiquetas": [], "de": ""}}], "tareas": [{{"texto": "", "fecha": "", "unidad": "L1"}}], "eventos": [{{"titulo": "", "fecha": "", "hora": "", "unidad": "L1"}}], {DOUBTS_SHAPE}}}

Reglas:
- es_reunion: true si la nota completa registra una reunión, llamada o conversación con otras personas.
- espacio: el espacio de trabajo al que pertenece la nota completa, exactamente como en la lista. confianza "alta" solo si es evidente; si no está claro, deja el espacio actual con confianza "baja".
- {RULE_NEW_SPACE} Aplica a la nota completa: solo si toda la nota trata de eso y no calza con ningún espacio.
- titulo: título breve para la nota (máximo 6 palabras), sin fecha.
- resumen: si es una reunión, 2 o 3 frases con decisiones y acuerdos; si no, "".
- unidades: incluye cada unidad a la que le pongas etiquetas.
{RULES_UNITS}
{RULES_TASKS}
{RULES_DOUBTS} En esta nota las unidades no se mueven: en una opción, "espacio" (sin "nota") mueve la nota completa a ese espacio.
Hoy es {}."###,
            today()
        )
    };
    let place = if capture { String::new() } else { format!("Espacio actual: {workspace}\nTítulo actual: {title}\n") };
    let user = format!("{}\n{place}\nUnidades:\n<<<\n{}>>>", context(workspaces, all_tags, learned), list_units(text, units));
    (system, user)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_answers_are_explained_or_rescued() {
        // Texto vacío pero el JSON quedó en el razonamiento: se rescata.
        let a = interpret("", "pensando… {\"unidades\": [{\"id\": \"L2\", \"espacio\": \"X\", \"nota\": \"Y\"}]}", "Completed").unwrap();
        assert_eq!((a.unidades[0].id.as_str(), a.unidades[0].nota.as_str()), ("L2", "Y"));
        // Sin nada: el error dice por qué.
        let e = interpret("", "mucho razonamiento sin respuesta", "MaxTokens(\"length\")").unwrap_err();
        assert!(e.contains("límite"), "{e}");
        let e = interpret("", "solo pensé", "Completed(\"stop\")").unwrap_err();
        assert!(e.contains("solo razonó"), "{e}");
    }

    #[test]
    fn capture_prompt_lists_units() {
        let text = "uno\n\n## Reunión X · hoy\n- 10:00 algo\n## fin · 10:30\ncuatro";
        let (_, user) = build_prompt("", "", text, &crate::lines::units(text), true, &[], &[], &[]);
        assert!(user.contains("[L1] uno\n[B3] (bloque, líneas 3 a 5)\n    ## Reunión X · hoy\n"), "{user}");
        assert!(user.contains("[L6] cuatro\n"), "{user}");
        // Nota con título: sus líneas con sangría van con su nota; no se pregunta a dónde moverlas.
        let text = "Comprar pan\n  integral\n";
        let (system, user) = build_prompt("Compras", "General", text, &crate::lines::units(text), false, &[], &[], &["LaVet es de Docencia".to_string()]);
        assert!(user.contains("Título actual: Compras\n") && user.contains("[L1] Comprar pan\n      integral\n"), "{user}");
        assert!(system.contains("\"confianza\"") && system.contains("\"dudas\""), "{system}");
        assert!(user.contains("ya aclaró (úsalo y no vuelvas a preguntarlo):\n- LaVet es de Docencia\n"), "{user}");
    }

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
