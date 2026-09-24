//! Lectura de config.toml (se crea con valores por defecto si no existe).

use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub carpeta_notas: PathBuf,
    /// opencode | opencode-go | anthropic | openai | gemini | ollama
    pub proveedor: String,
    pub modelo: String,
    pub clave_api: String,
    /// Analizar solas las notas que se escriben (reunión, espacio, etiquetas, tareas, agenda).
    pub ia_automatica: bool,
    /// Credenciales OAuth "App de escritorio" de Google Cloud (ver README).
    pub google_client_id: String,
    pub google_client_secret: String,
}

impl Default for Config {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Config {
            carpeta_notas: home.join("Dropbox").join("Notas"),
            proveedor: "opencode".into(),
            modelo: "deepseek-v4.1-flash".into(),
            clave_api: String::new(),
            ia_automatica: true,
            google_client_id: String::new(),
            google_client_secret: String::new(),
        }
    }
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("nodex-notes")
        .join("config.toml")
}

/// Devuelve la configuración y, si hubo algo que avisar, un mensaje.
pub fn load() -> (Config, Option<String>) {
    let path = config_path();
    let (mut cfg, msg) = match fs::read_to_string(&path) {
        Ok(s) => match toml::from_str::<Config>(&s) {
            Ok(c) => (c, None),
            Err(e) => (
                Config::default(),
                Some(format!("Error en {}: {}", path.display(), e.message())),
            ),
        },
        Err(_) => {
            let c = Config::default();
            let msg = match write_default(&path, &c) {
                Ok(()) => format!("Configuración creada en {}", path.display()),
                Err(e) => format!("No se pudo crear {}: {e}", path.display()),
            };
            (c, Some(msg))
        }
    };
    // Permite "~/Dropbox/Notas" en el archivo.
    if let Ok(rest) = cfg.carpeta_notas.strip_prefix("~") {
        if let Some(home) = dirs::home_dir() {
            cfg.carpeta_notas = home.join(rest);
        }
    }
    (cfg, msg)
}

/// Lo último abierto, para volver ahí al iniciar.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Estado {
    pub espacio: String,
    pub nota: String,
}

fn estado_path() -> PathBuf {
    config_path().with_file_name("estado.toml")
}

pub fn load_estado() -> Estado {
    fs::read_to_string(estado_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_estado(e: &Estado) {
    let q = |s: &str| toml::Value::String(s.to_string()).to_string();
    let path = estado_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(path, format!("espacio = {}\nnota = {}\n", q(&e.espacio), q(&e.nota)));
}

fn write_default(path: &PathBuf, c: &Config) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let q = |s: &str| toml::Value::String(s.to_string()).to_string();
    let text = format!(
        "# Configuración de Notas\n\
         \n\
         # Carpeta de notas (una subcarpeta por espacio de trabajo)\n\
         carpeta_notas = {}\n\
         \n\
         # Proveedor de IA: opencode (Zen, pago por uso), opencode-go (plan Go), anthropic, openai, gemini u ollama\n\
         proveedor = {}\n\
         modelo = {}\n\
         # Clave API (no hace falta para ollama)\n\
         clave_api = \"\"\n\
         \n\
         # Analizar solas las notas al terminar de escribirlas (espacio, etiquetas, tareas, agenda)\n\
         ia_automatica = true\n\
         \n\
         # Google Calendar: credenciales OAuth \"App de escritorio\" (pasos en el README)\n\
         google_client_id = \"\"\n\
         google_client_secret = \"\"\n",
        q(&c.carpeta_notas.to_string_lossy()),
        q(&c.proveedor),
        q(&c.modelo),
    );
    fs::write(path, text)
}
