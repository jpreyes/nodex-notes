//! Análisis de notas con IA (genai) en un hilo aparte, para que la ventana nunca se congele.

use crate::config::Config;
use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest};
use genai::resolver::{AuthData, AuthResolver};
use genai::{Client, ModelIden};
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

fn adapter(proveedor: &str) -> Result<AdapterKind, String> {
    match proveedor.trim().to_lowercase().as_str() {
        "anthropic" | "claude" => Ok(AdapterKind::Anthropic),
        "openai" | "gpt" => Ok(AdapterKind::OpenAI),
        "gemini" | "google" => Ok(AdapterKind::Gemini),
        "ollama" => Ok(AdapterKind::Ollama),
        other => Err(format!("Proveedor desconocido «{other}» (usa anthropic, openai, gemini u ollama)")),
    }
}

fn env_key(kind: AdapterKind) -> Option<String> {
    let var = match kind {
        AdapterKind::Anthropic => "ANTHROPIC_API_KEY",
        AdapterKind::OpenAI => "OPENAI_API_KEY",
        AdapterKind::Gemini => "GEMINI_API_KEY",
        _ => return None,
    };
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

impl Ai {
    /// Inicia el hilo de IA. Devuelve un error legible si falta configuración.
    pub fn start(cfg: &Config, ctx: eframe::egui::Context) -> Result<Ai, String> {
        let kind = adapter(&cfg.proveedor)?;
        let key = Some(cfg.clave_api.trim().to_string()).filter(|k| !k.is_empty()).or_else(|| env_key(kind));
        if key.is_none() && kind != AdapterKind::Ollama {
            return Err("Falta la clave API: agrégala en config.toml (clave_api)".into());
        }
        let model = ModelIden::new(kind, cfg.modelo.trim().to_string());
        let label = format!("{} · {}", cfg.proveedor, cfg.modelo);
        let (tx, job_rx) = mpsc::channel::<Job>();
        let (res_tx, rx) = mpsc::channel::<JobResult>();

        std::thread::Builder::new()
            .name("ia".into())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                    return;
                };
                let mut builder = Client::builder();
                if let Some(k) = key {
                    builder = builder.with_auth_resolver(AuthResolver::from_resolver_fn(
                        move |_: ModelIden| -> Result<Option<AuthData>, genai::resolver::Error> {
                            Ok(Some(AuthData::from_single(k.clone())))
                        },
                    ));
                }
                let client = builder.build();
                let options = ChatOptions::default().with_temperature(0.2).with_max_tokens(2000);
                for job in job_rx {
                    let req = ChatRequest::default().with_system(job.system).append_message(ChatMessage::user(job.user));
                    let result = rt
                        .block_on(client.exec_chat(model.clone(), req, Some(&options)))
                        .map_err(|e| e.to_string())
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
