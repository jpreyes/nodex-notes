//! config.toml (se edita desde la ventana de Configuración; se crea si no existe)
//! y estado.toml (última nota abierta).

use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Deserialize)]
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
    /// Calendarios que se ven en la Agenda (enlace ICS de Google, Outlook, iCloud…).
    pub calendarios: Vec<crate::calendars::Subscription>,
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
            calendarios: Vec::new(),
        }
    }
}

pub fn config_path() -> PathBuf {
    // Para pruebas: otra carpeta, así nunca se toca la configuración real.
    if let Some(dir) = std::env::var_os("NODEX_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join("config.toml");
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("nodex-notes")
        .join("config.toml")
}

/// Devuelve la configuración y, si hubo algo que avisar, un mensaje.
pub fn load() -> (Config, Option<String>) {
    let path = config_path();
    let (mut cfg, msg) = match crate::vault::read_text(&path) {
        Ok(s) => match toml::from_str::<Config>(&s) {
            Ok(c) => (c, None),
            Err(e) => (
                Config::default(),
                Some(format!("Error en {}: {}", path.display(), e.message())),
            ),
        },
        Err(_) => {
            let c = Config::default();
            let msg = match save(&c) {
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

fn render(c: &Config) -> String {
    let q = |s: &str| toml::Value::String(s.to_string()).to_string();
    let mut calendars = String::new();
    if !c.calendarios.is_empty() {
        calendars += "\n# Calendarios que se ven en la Agenda (enlace ICS de Google, Outlook, iCloud…)\n";
        for cal in &c.calendarios {
            calendars += &format!("[[calendarios]]\nnombre = {}\nurl = {}\n", q(&cal.nombre), q(&cal.url));
        }
    }
    let main = format!(
        "# Configuración de Notas (se cambia desde la app: botón ⚙ o Ctrl+,)\n\
         \n\
         # Carpeta de notas (una subcarpeta por espacio de trabajo)\n\
         carpeta_notas = {}\n\
         \n\
         # Proveedor de IA: opencode (Zen, pago por uso), opencode-go (plan Go), anthropic, openai, gemini u ollama\n\
         proveedor = {}\n\
         modelo = {}\n\
         # Clave API (no hace falta para ollama). Este archivo queda solo en este equipo.\n\
         clave_api = {}\n\
         \n\
         # Analizar solas las notas al terminar de escribirlas (espacio, etiquetas, tareas, agenda)\n\
         ia_automatica = {}\n\
         \n\
         # Google Calendar: credenciales OAuth \"App de escritorio\" (pasos en el README)\n\
         google_client_id = {}\n\
         google_client_secret = {}\n",
        q(&c.carpeta_notas.to_string_lossy()),
        q(&c.proveedor),
        q(&c.modelo),
        q(&c.clave_api),
        c.ia_automatica,
        q(&c.google_client_id),
        q(&c.google_client_secret),
    );
    main + &calendars
}

/// Escribe config.toml completo, con comentarios, para que siga siendo legible a mano.
pub fn save(c: &Config) -> std::io::Result<()> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, render(c))
}

/// Lo último abierto, para volver ahí al iniciar.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Estado {
    pub espacio: String,
    pub nota: String,
    /// Último día en que se mostró la vista "Hoy" al abrir.
    pub hoy: String,
    /// Última semana (AAAA-Wnn) en que se abrió la revisión semanal.
    pub semana: String,
    /// Pestañas abiertas ("nota:General/x", "vista:hoy") y cuál está activa.
    pub pestanas: Vec<String>,
    pub pestana: usize,
}

fn estado_path() -> PathBuf {
    config_path().with_file_name("estado.toml")
}

pub fn load_estado() -> Estado {
    crate::vault::read_text(&estado_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

fn render_estado(e: &Estado) -> String {
    let q = |s: &str| toml::Value::String(s.to_string()).to_string();
    let tabs = toml::Value::Array(e.pestanas.iter().map(|t| toml::Value::String(t.clone())).collect()).to_string();
    format!(
        "espacio = {}\nnota = {}\nhoy = {}\nsemana = {}\npestanas = {tabs}\npestana = {}\n",
        q(&e.espacio),
        q(&e.nota),
        q(&e.hoy),
        q(&e.semana),
        e.pestana
    )
}

pub fn save_estado(e: &Estado) {
    let path = estado_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(path, render_estado(e));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_estado_keeps_every_field() {
        let e = Estado {
            espacio: "General".into(),
            nota: r"General\Notas generales.md".into(),
            hoy: "2026-09-25".into(),
            semana: "2026-W39".into(),
            pestanas: vec!["vista:inicio".into(), "nota:General/Notas \"raras\"".into()],
            pestana: 1,
        };
        let back: Estado = toml::from_str(&render_estado(&e)).unwrap();
        assert_eq!((back.espacio, back.nota, back.hoy, back.semana), (e.espacio, e.nota, e.hoy, e.semana));
        assert_eq!((back.pestanas, back.pestana), (e.pestanas, e.pestana));
    }

    #[test]
    fn saved_config_parses_back() {
        let c = Config {
            carpeta_notas: PathBuf::from(r#"C:\Users\x\Dropbox\Notas "raras""#),
            proveedor: "opencode-go".into(),
            clave_api: "sk-'abc'\"x".into(),
            ia_automatica: false,
            google_client_secret: "GOCSPX-1".into(),
            calendarios: vec![crate::calendars::Subscription { nombre: "Trabajo \"x\"".into(), url: "webcal://ejemplo.com/a.ics?x=1&y=2".into() }],
            ..Config::default()
        };
        let back: Config = toml::from_str(&render(&c)).unwrap();
        assert_eq!(back, c);
        // Guardado por Notepad con BOM: se lee igual.
        let dir = std::env::temp_dir().join(format!("nodex-bom-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("c.toml"), format!("\u{feff}{}", render(&c))).unwrap();
        let text = crate::vault::read_text(&dir.join("c.toml")).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), c);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
