//! ¿Tu carpeta de notas está en Dropbox, y está abierta la app de Dropbox?
//!
//! Solo sirve para avisar («Dropbox no está abierto: tus cambios quedan en este equipo»): la app
//! funciona igual con o sin Dropbox. Se revisa en un hilo aparte cada 30 segundos, sin abrir
//! ventanas.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

const EVERY: Duration = Duration::from_secs(30);

/// Las carpetas de Dropbox que dice su configuración (`info.json`: personal y de empresa).
fn roots_from_info(json: &str) -> Vec<PathBuf> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    v.as_object()
        .into_iter()
        .flat_map(|o| o.values())
        .filter_map(|acct| acct["path"].as_str())
        .map(PathBuf::from)
        .collect()
}

fn info_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(d) = dirs::data_local_dir() {
        out.push(d.join("Dropbox").join("info.json"));
    }
    if let Some(d) = dirs::config_dir() {
        out.push(d.join("Dropbox").join("info.json"));
    }
    if let Some(h) = dirs::home_dir() {
        out.push(h.join(".dropbox").join("info.json"));
    }
    out
}

/// ¿La carpeta está dentro de Dropbox? (según `info.json`, o si su ruta pasa por una carpeta
/// llamada «Dropbox»).
pub fn contains(notes: &Path) -> bool {
    let roots: Vec<PathBuf> = info_files().iter().filter_map(|f| std::fs::read_to_string(f).ok()).flat_map(|j| roots_from_info(&j)).collect();
    inside(notes, &roots)
}

fn inside(notes: &Path, roots: &[PathBuf]) -> bool {
    let lower = |p: &Path| p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase();
    let n = lower(notes);
    if roots.iter().any(|r| {
        let r = lower(r);
        !r.is_empty() && (n == r || n.starts_with(&format!("{r}/")))
    }) {
        return true;
    }
    notes.components().any(|c| {
        let s = c.as_os_str().to_string_lossy().to_lowercase();
        s == "dropbox" || s.starts_with("dropbox (")
    })
}

/// ¿Está corriendo la app de Dropbox? `None` si no se pudo saber.
pub fn is_running() -> Option<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let out = std::process::Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq Dropbox.exe", "/NH", "/FO", "CSV"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&out.stdout).to_lowercase().contains("dropbox.exe"))
    }
    #[cfg(not(windows))]
    {
        let name = if cfg!(target_os = "macos") { "Dropbox" } else { "dropbox" };
        let out = std::process::Command::new("pgrep").args(["-x", name]).output().ok()?;
        Some(out.status.success())
    }
}

/// Estado de Dropbox, revisado en segundo plano.
pub struct Watch {
    /// 0 = aún no se sabe, 1 = abierto, 2 = cerrado.
    state: Arc<AtomicU8>,
}

impl Watch {
    pub fn start(ctx: eframe::egui::Context) -> Watch {
        let state = Arc::new(AtomicU8::new(0));
        let s = state.clone();
        let _ = std::thread::Builder::new().name("dropbox".into()).spawn(move || {
            // Si la app se cerró, el hilo termina solo (nadie más tiene el estado).
            while Arc::strong_count(&s) > 1 {
                let now = match is_running() {
                    Some(true) => 1,
                    Some(false) => 2,
                    None => 0,
                };
                if s.swap(now, Ordering::Relaxed) != now {
                    ctx.request_repaint();
                }
                std::thread::sleep(EVERY);
            }
        });
        Watch { state }
    }

    /// `true` si se sabe que Dropbox está cerrado.
    pub fn closed(&self) -> bool {
        self.state.load(Ordering::Relaxed) == 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_whether_notes_are_in_dropbox() {
        let info = r#"{"personal": {"path": "C:\\Users\\jp\\Dropbox", "host": 1}, "business": {"path": "D:\\Dropbox (Empresa)"}}"#;
        let roots = roots_from_info(info);
        assert_eq!(roots.len(), 2);
        assert!(inside(Path::new(r"C:\Users\jp\Dropbox\Notas"), &roots));
        assert!(inside(Path::new(r"d:\dropbox (empresa)\Notas"), &roots));
        assert!(!inside(Path::new(r"C:\Users\jp\Documentos\Notas"), &roots));
        assert!(!inside(Path::new(r"C:\Users\jp\DropboxViejo\Notas"), &roots));
        // Sin info.json: por el nombre de la carpeta.
        assert!(inside(Path::new("/home/jp/Dropbox/Notas"), &[]));
        assert!(!inside(Path::new("/home/jp/OneDrive/Notas"), &[]));
        assert!(roots_from_info("no es json").is_empty());
    }
}
