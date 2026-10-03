//! Actualizar la app desde la misma app.
//!
//! Busca la última versión publicada en GitHub, baja el archivo que corresponde a cómo está
//! instalada esta copia (.msi, ejecutable suelto, Notas.app o .deb), revisa que llegó entero
//! (la huella SHA-256 que publica GitHub) y la instala: la app se cierra y se abre la nueva.
//! La nueva espera a que la anterior termine de guardar (ver `wait_for_previous`).

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

const LATEST: &str = "https://api.github.com/repos/jpreyes/nodex-notes/releases/latest";
/// La página de la última versión (para bajarla a mano).
pub const RELEASES_PAGE: &str = "https://github.com/jpreyes/nodex-notes/releases/latest";

/// Para pruebas: otro servidor con la misma respuesta que GitHub (NODEX_ACTUALIZAR_API).
fn test_api() -> Option<String> {
    std::env::var("NODEX_ACTUALIZAR_API").ok().filter(|s| !s.is_empty())
}

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    /// Qué hay de nuevo (el texto de la versión en GitHub).
    pub notes: String,
    pub assets: Vec<Asset>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Huella SHA-256 en hexadecimal, si GitHub la publica.
    pub sha256: Option<String>,
}

impl Release {
    pub fn version(&self) -> &str {
        self.tag.trim_start_matches('v')
    }

    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

pub fn parse_release(v: &Value) -> Option<Release> {
    let tag = v["tag_name"].as_str()?.to_string();
    let notes = v["body"].as_str().unwrap_or("").trim().to_string();
    let assets = v["assets"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    Some(Asset {
                        name: a["name"].as_str()?.to_string(),
                        url: a["browser_download_url"].as_str()?.to_string(),
                        size: a["size"].as_u64().unwrap_or(0),
                        sha256: a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(|h| h.to_ascii_lowercase()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Release { tag, notes, assets })
}

fn version_parts(s: &str) -> Vec<u32> {
    s.trim().trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// ¿La versión `tag` es más nueva que `than`?
pub fn is_newer(tag: &str, than: &str) -> bool {
    version_parts(tag) > version_parts(than)
}

/// Las novedades para mostrar: sin líneas vacías de más ni la firma de los commits.
pub fn notes_lines(notes: &str) -> Vec<String> {
    notes
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty() && !l.starts_with("Co-Authored-By"))
        .map(|l| l.to_string())
        .collect()
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("nodex-notes/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .unwrap_or_default()
}

/// La última versión publicada.
pub fn fetch_latest() -> Result<Release, String> {
    let url = test_api().unwrap_or_else(|| LATEST.into());
    runtime()?.block_on(async {
        let r = client().get(&url).send().await.map_err(|e| format!("sin conexión ({e})"))?;
        if !r.status().is_success() {
            return Err(format!("GitHub respondió {}", r.status()));
        }
        let v: Value = r.json().await.map_err(|e| e.to_string())?;
        parse_release(&v).ok_or_else(|| "GitHub no respondió la versión".to_string())
    })
}

// ---------- Cómo está instalada ----------

/// Cómo está instalada esta copia de la app: de eso depende qué se baja y cómo se instala.
#[derive(Clone, Debug, PartialEq)]
pub enum Install {
    /// Windows, con el instalador .msi (en Archivos de programa).
    Msi { exe: PathBuf },
    /// Un ejecutable suelto en una carpeta donde se puede escribir (Windows .exe, Linux .tar.gz).
    Binary { exe: PathBuf },
    /// macOS: Notas.app.
    MacApp { bundle: PathBuf },
    /// Linux, con el paquete .deb.
    Deb { exe: PathBuf },
    /// La actualiza la Microsoft Store.
    Store,
    /// No se puede actualizar sola (por qué): se avisa y se baja a mano.
    Manual(String),
}

impl Install {
    /// El archivo de la versión que sirve para esta instalación.
    pub fn asset_name(&self) -> Option<&'static str> {
        match self {
            Install::Msi { .. } => Some("Notas-windows-x64.msi"),
            Install::Binary { .. } if cfg!(windows) => Some("Notas-windows-x64.exe"),
            Install::Binary { .. } => Some("Notas-linux-x64.tar.gz"),
            Install::MacApp { .. } if cfg!(target_arch = "aarch64") => Some("Notas-macos-apple-silicon.zip"),
            Install::MacApp { .. } => Some("Notas-macos-intel.zip"),
            Install::Deb { .. } => Some("Notas-linux-x64.deb"),
            Install::Store | Install::Manual(_) => None,
        }
    }

    /// Lo que hay que saber, para Configuración → Acerca de (nada, si se actualiza sola).
    pub fn describe(&self) -> Option<String> {
        match self {
            Install::Msi { .. } => Some("Windows te pedirá permiso para instalarla.".into()),
            Install::Deb { .. } => Some("Se te pedirá tu contraseña para instalarla.".into()),
            Install::Store => Some("La actualiza la Microsoft Store.".into()),
            Install::Manual(why) => Some(format!("Esta copia no se puede actualizar sola ({why}): baja la nueva desde GitHub.")),
            Install::Binary { .. } | Install::MacApp { .. } => None,
        }
    }
}

/// Cómo está instalada esta copia (mira dónde está el ejecutable y si se puede escribir ahí).
pub fn detect() -> Install {
    let Ok(exe) = std::env::current_exe() else { return Install::Manual("no se sabe dónde está".into()) };
    #[cfg(unix)]
    let exe = exe.canonicalize().unwrap_or(exe);
    // Una copia compilada en el equipo (cargo build) no se reemplaza, salvo en las pruebas.
    let comps: Vec<String> = exe.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect();
    let dev = comps.windows(2).any(|w| w[0] == "target" && (w[1] == "debug" || w[1] == "release"))
        || comps.windows(3).any(|w| w[0] == "target" && (w[2] == "debug" || w[2] == "release"));
    if dev && test_api().is_none() {
        return Install::Manual("es una copia de desarrollo".into());
    }
    if !cfg!(target_arch = "x86_64") && !cfg!(target_os = "macos") {
        return Install::Manual("no hay versión para este procesador".into());
    }
    let from_deb = cfg!(target_os = "linux")
        && exe.starts_with("/usr")
        && Command::new("dpkg").arg("-S").arg(&exe).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    classify(&exe, std::env::consts::OS, writable, from_deb)
}

/// La parte de `detect` que no mira el equipo (para probarla).
pub fn classify(exe: &Path, os: &str, writable: impl Fn(&Path) -> bool, from_deb: bool) -> Install {
    let dir = exe.parent().unwrap_or(Path::new("."));
    let not_writable = || Install::Manual("está en una carpeta donde no se puede escribir".into());
    match os {
        "windows" => {
            let lower = exe.to_string_lossy().to_lowercase();
            if lower.contains("\\windowsapps\\") {
                return Install::Store;
            }
            let name = |p: Option<&Path>| p.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            if name(Some(dir)) == "notas" && name(dir.parent()).starts_with("program files") {
                return Install::Msi { exe: exe.to_path_buf() };
            }
            if writable(dir) { Install::Binary { exe: exe.to_path_buf() } } else { not_writable() }
        }
        "macos" => {
            // …/Notas.app/Contents/MacOS/nodex-notes
            let bundle = dir.parent().and_then(|c| c.parent()).filter(|b| b.extension().is_some_and(|e| e == "app"));
            match bundle {
                Some(b) if writable(b.parent().unwrap_or(Path::new("/"))) => Install::MacApp { bundle: b.to_path_buf() },
                Some(_) => not_writable(),
                None => Install::Manual("no está dentro de Notas.app".into()),
            }
        }
        _ => {
            if from_deb {
                Install::Deb { exe: exe.to_path_buf() }
            } else if writable(dir) {
                Install::Binary { exe: exe.to_path_buf() }
            } else {
                not_writable()
            }
        }
    }
}

/// ¿Se puede escribir en la carpeta? (Se prueba de verdad: los permisos engañan en Windows.)
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".nodex-prueba-{}", std::process::id()));
    let ok = fs::write(&probe, b"").is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

// ---------- Bajar ----------

/// Dónde se bajan las versiones nuevas.
fn downloads_dir() -> PathBuf {
    if std::env::var_os("NODEX_CONFIG_DIR").is_some_and(|d| !d.is_empty()) {
        return crate::config::config_path().with_file_name("actualizacion");
    }
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("nodex-notes").join("actualizacion")
}

pub fn download_dir(tag: &str) -> PathBuf {
    downloads_dir().join(tag)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// ¿El archivo es el publicado? (tamaño y huella)
fn verify(path: &Path, asset: &Asset) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if asset.size > 0 && bytes.len() as u64 != asset.size {
        return Err("la versión nueva llegó incompleta".into());
    }
    if let Some(want) = &asset.sha256 {
        if hex(&Sha256::digest(&bytes)) != *want {
            return Err("la versión nueva llegó dañada (su huella no coincide)".into());
        }
    }
    Ok(())
}

/// Baja `asset` a `dir` (si ya estaba bajado y entero, no lo baja de nuevo). `progress` va
/// contando los bytes.
pub fn download(asset: &Asset, dir: &Path, progress: &AtomicU64) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let dest = dir.join(&asset.name);
    if dest.is_file() && verify(&dest, asset).is_ok() {
        progress.store(asset.size, Ordering::Relaxed);
        return Ok(dest);
    }
    let part = dir.join(format!("{}.part", asset.name));
    runtime()?.block_on(async {
        let mut r = client().get(&asset.url).send().await.map_err(|e| format!("sin conexión ({e})"))?;
        if !r.status().is_success() {
            return Err(format!("GitHub respondió {}", r.status()));
        }
        let mut f = fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut n = 0u64;
        while let Some(chunk) = r.chunk().await.map_err(|e| format!("se cortó la descarga ({e})"))? {
            f.write_all(&chunk).map_err(|e| e.to_string())?;
            n += chunk.len() as u64;
            progress.store(n, Ordering::Relaxed);
        }
        f.sync_all().map_err(|e| e.to_string())
    })?;
    if let Err(e) = verify(&part, asset) {
        let _ = fs::remove_file(&part);
        return Err(e);
    }
    let _ = fs::remove_file(&dest);
    fs::rename(&part, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

// ---------- Instalar ----------

/// Carpeta junto a Notas.app donde se descomprime la nueva (en el mismo disco, para moverla).
fn mac_staging(bundle: &Path) -> PathBuf {
    bundle.parent().unwrap_or(Path::new("/")).join(".notas-actualizacion")
}

/// Lo lento antes de reiniciar (descomprimir). Devuelve el archivo o la carpeta con la versión nueva.
pub fn prepare(install: &Install, file: &Path) -> Result<PathBuf, String> {
    let run = |c: &mut Command, what: &str| -> Result<(), String> {
        let ok = c.stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
        if ok { Ok(()) } else { Err(format!("no se pudo {what}")) }
    };
    match install {
        Install::Binary { .. } if file.to_string_lossy().ends_with(".tar.gz") => {
            let dir = file.with_file_name("nueva");
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            run(Command::new("tar").arg("-xzf").arg(file).arg("-C").arg(&dir), "descomprimir la versión nueva")?;
            let bin = dir.join("nodex-notes");
            if bin.is_file() { Ok(bin) } else { Err("la versión nueva no trae el programa".into()) }
        }
        Install::MacApp { bundle } => {
            let staging = mac_staging(bundle);
            let _ = fs::remove_dir_all(&staging);
            fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
            run(Command::new("ditto").args(["-x", "-k"]).arg(file).arg(&staging), "descomprimir la versión nueva")?;
            let app = staging.join("Notas.app");
            if app.is_dir() { Ok(app) } else { Err("la versión nueva no trae Notas.app".into()) }
        }
        _ => Ok(file.to_path_buf()),
    }
}

/// El ejecutable anterior queda al lado con este nombre hasta el próximo inicio (en Windows no se
/// puede borrar un programa mientras corre, pero sí cambiarle el nombre).
fn old_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".anterior");
    exe.with_file_name(name)
}

/// Pone `new` en el lugar de `exe` (el anterior queda como «….anterior»; si algo falla, vuelve).
pub fn replace_file(exe: &Path, new: &Path) -> Result<(), String> {
    let old = old_path(exe);
    let _ = fs::remove_file(&old);
    fs::rename(exe, &old).map_err(|e| format!("no se pudo reemplazar la app ({e})"))?;
    if let Err(e) = fs::copy(new, exe) {
        let _ = fs::remove_file(exe);
        let _ = fs::rename(&old, exe);
        return Err(format!("no se pudo copiar la versión nueva ({e})"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(exe, fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

/// Marca «se está actualizando»: la app nueva espera a que la anterior la borre al cerrarse.
fn marker() -> PathBuf {
    crate::config::config_path().with_file_name("actualizando.txt")
}

/// La anterior ya guardó todo y se cierra.
pub fn finished_closing() {
    let _ = fs::remove_file(marker());
}

/// Al abrir: si la versión anterior se está cerrando (recién actualizada), espera a que termine
/// de guardar (como mucho 20 s). Después borra lo que quedó de la versión anterior.
/// Devuelve si se acaba de actualizar.
pub fn wait_for_previous() -> bool {
    let m = marker();
    let recent = fs::metadata(&m)
        .and_then(|md| md.modified())
        .is_ok_and(|t| SystemTime::now().duration_since(t).is_ok_and(|d| d < Duration::from_secs(120)));
    if recent {
        let until = Instant::now() + Duration::from_secs(20);
        while m.exists() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let _ = fs::remove_file(&m);
    cleanup();
    recent
}

/// Borra el ejecutable anterior, la Notas.app anterior y las descargas ya usadas.
fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        // En Windows el anterior queda bloqueado unos segundos más, hasta que su proceso termina.
        let old = old_path(&exe);
        if old.exists() {
            std::thread::spawn(move || {
                for _ in 0..60 {
                    if fs::remove_file(&old).is_ok() || !old.exists() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
            });
        }
        if let Install::MacApp { bundle } = classify(&exe, std::env::consts::OS, |_| true, false) {
            let _ = fs::remove_dir_all(mac_staging(&bundle));
        }
    }
    // Descargas de versiones que ya no son nuevas.
    if let Ok(list) = fs::read_dir(downloads_dir()) {
        for e in list.flatten() {
            if !is_newer(&e.file_name().to_string_lossy(), current()) {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
}

/// Abre un programa que sigue vivo cuando esta app se cierra.
fn spawn(c: &mut Command) -> Result<(), String> {
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    c.spawn().map(|_| ()).map_err(|e| format!("no se pudo abrir la versión nueva ({e})"))
}

/// Instala la versión nueva (ya preparada) y abre la nueva, que espera a que esta se cierre.
/// Si sale bien, la app tiene que cerrarse enseguida.
pub fn apply(install: &Install, new: &Path) -> Result<(), String> {
    fs::write(marker(), std::process::id().to_string()).map_err(|e| e.to_string())?;
    let result = install_new(install, new);
    if result.is_err() {
        finished_closing();
    }
    result
}

fn install_new(install: &Install, new: &Path) -> Result<(), String> {
    match install {
        Install::Binary { exe } => replace_file(exe, new).and_then(|()| spawn(&mut Command::new(exe))),
        Install::MacApp { bundle } => {
            let old = mac_staging(bundle).join("anterior.app");
            let _ = fs::remove_dir_all(&old);
            fs::rename(bundle, &old).map_err(|e| format!("no se pudo reemplazar la app ({e})"))?;
            if let Err(e) = fs::rename(new, bundle) {
                let _ = fs::rename(&old, bundle);
                return Err(format!("no se pudo poner la versión nueva ({e})"));
            }
            spawn(Command::new("open").arg("-n").arg(bundle))
        }
        Install::Deb { exe } => {
            // Pide la contraseña con la ventana del sistema; si se cancela, no se cierra nada.
            let ok = Command::new("pkexec").args(["dpkg", "-i"]).arg(new).status().is_ok_and(|s| s.success());
            if ok { spawn(&mut Command::new(exe)) } else { Err("no se instaló (¿se canceló la contraseña?)".into()) }
        }
        Install::Msi { exe } => run_msi(new, exe),
        Install::Store | Install::Manual(_) => Err("esta copia no se puede actualizar sola".into()),
    }
}

/// Windows: un ayudante (PowerShell, sin ventana) espera a que la app se cierre, corre el
/// instalador (Windows pide permiso) y vuelve a abrir la app.
#[cfg(windows)]
fn run_msi(msi: &Path, exe: &Path) -> Result<(), String> {
    use base64::Engine;
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let q = |p: &Path| p.display().to_string().replace('\'', "''");
    let script = format!(
        "Wait-Process -Id {pid} -Timeout 60 -ErrorAction SilentlyContinue; \
         Start-Process msiexec.exe -ArgumentList '/i \"{msi}\" /passive /norestart /l*v \"{log}\"' -Wait; \
         Start-Process -FilePath '{exe}'",
        pid = std::process::id(),
        msi = q(msi),
        log = q(&msi.with_extension("log")),
        exe = q(exe),
    );
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    spawn(
        Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-EncodedCommand"])
            .arg(encoded)
            .creation_flags(CREATE_NO_WINDOW),
    )
}

#[cfg(not(windows))]
fn run_msi(_msi: &Path, _exe: &Path) -> Result<(), String> {
    Err("el instalador .msi es solo para Windows".into())
}

// ---------- Qué hay de nuevo ----------

/// Se instaló (o se intentó instalar) una versión: para mostrarlo una vez al abrir.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub tag: String,
    pub notes: String,
    /// ¿Quedó instalada? (Si no, por ejemplo, se canceló el permiso de Windows.)
    pub installed: bool,
}

fn pending_path() -> PathBuf {
    crate::config::config_path().with_file_name("novedades.txt")
}

/// Antes de reiniciar: qué versión se instala y qué trae.
pub fn remember_pending(release: &Release) {
    let _ = fs::write(pending_path(), format!("{}\n{}", release.tag, release.notes));
}

/// Al abrir: la versión que se acaba de instalar (una sola vez).
pub fn take_pending() -> Option<Pending> {
    let path = pending_path();
    let text = crate::vault::read_text(&path).ok()?;
    let _ = fs::remove_file(&path);
    let (tag, notes) = text.split_once('\n').unwrap_or((text.as_str(), ""));
    let tag = tag.trim().to_string();
    (!tag.is_empty()).then(|| Pending { installed: !is_newer(&tag, current()), tag, notes: notes.trim().to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn newer_versions() {
        assert!(is_newer("v0.27.0", "0.26.1"));
        assert!(is_newer("v0.26.10", "0.26.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("v0.26.1", "0.26.1"));
        assert!(!is_newer("v0.25.9", "0.26.1"));
    }

    #[test]
    fn reads_github_release() {
        let v = serde_json::json!({
            "tag_name": "v0.27.0",
            "body": "- Actualizar desde la app\n\nCo-Authored-By: alguien",
            "assets": [
                {"name": "Notas-windows-x64.msi", "browser_download_url": "https://x/a.msi", "size": 10, "digest": "sha256:ABCD"},
                {"name": "Notas-linux-x64.deb", "browser_download_url": "https://x/a.deb", "size": 5, "digest": null}
            ]
        });
        let r = parse_release(&v).unwrap();
        assert_eq!(r.version(), "0.27.0");
        assert_eq!(r.asset("Notas-windows-x64.msi").unwrap().sha256.as_deref(), Some("abcd"));
        assert_eq!(r.asset("Notas-linux-x64.deb").unwrap().sha256, None);
        assert_eq!(notes_lines(&r.notes), vec!["- Actualizar desde la app"]);
    }

    #[cfg(windows)]
    #[test]
    fn how_it_was_installed_on_windows() {
        let yes = |_: &Path| true;
        let no = |_: &Path| false;
        let msi = Path::new(r"C:\Program Files\Notas\Notas.exe");
        assert_eq!(classify(msi, "windows", no, false), Install::Msi { exe: msi.into() });
        let store = Path::new(r"C:\Program Files\WindowsApps\Notas_1.0_x64\Notas.exe");
        assert_eq!(classify(store, "windows", no, false), Install::Store);
        let loose = Path::new(r"C:\Users\ana\Descargas\Notas-windows-x64.exe");
        assert_eq!(classify(loose, "windows", yes, false), Install::Binary { exe: loose.into() });
        assert!(matches!(classify(loose, "windows", no, false), Install::Manual(_)));
        assert_eq!(Install::Msi { exe: msi.into() }.asset_name(), Some("Notas-windows-x64.msi"));
        assert_eq!(Install::Binary { exe: loose.into() }.asset_name(), Some("Notas-windows-x64.exe"));
    }

    #[cfg(unix)]
    #[test]
    fn how_it_was_installed_on_mac_and_linux() {
        let yes = |_: &Path| true;
        let app = Path::new("/Applications/Notas.app/Contents/MacOS/nodex-notes");
        assert_eq!(classify(app, "macos", yes, false), Install::MacApp { bundle: "/Applications/Notas.app".into() });
        let deb = Path::new("/usr/bin/nodex-notes");
        assert_eq!(classify(deb, "linux", |_| false, true), Install::Deb { exe: deb.into() });
        let tar = Path::new("/home/ana/bin/nodex-notes");
        assert_eq!(classify(tar, "linux", yes, false), Install::Binary { exe: tar.into() });
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nodex-actualizar-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn replaces_the_program_and_keeps_the_old_one_aside() {
        let dir = temp("reemplazar");
        let exe = dir.join("Notas.exe");
        let new = dir.join("nueva.exe");
        fs::write(&exe, "vieja").unwrap();
        fs::write(&new, "nueva").unwrap();
        replace_file(&exe, &new).unwrap();
        assert_eq!(fs::read_to_string(&exe).unwrap(), "nueva");
        assert_eq!(fs::read_to_string(old_path(&exe)).unwrap(), "vieja");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Baja de un servidor falso: revisa la huella y cuenta el avance.
    #[test]
    fn downloads_and_checks_the_fingerprint() {
        use axum::{Router, routing::get};
        let body = b"programa nuevo".repeat(1000);
        let sha = hex(&Sha256::digest(&body));
        let served = body.clone();
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route("/nueva", get(move || async move { served }));
        rt.spawn(async move { axum::serve(listener, app).await.unwrap() });

        let dir = temp("bajar");
        let good = Asset { name: "Notas.exe".into(), url: format!("http://{addr}/nueva"), size: body.len() as u64, sha256: Some(sha) };
        let progress = Arc::new(AtomicU64::new(0));
        let file = download(&good, &dir, &progress).unwrap();
        assert_eq!(fs::read(&file).unwrap(), body);
        assert_eq!(progress.load(Ordering::Relaxed), body.len() as u64);

        // Huella distinta: no queda nada a medio bajar ni se da por buena.
        let bad = Asset { name: "Otra.exe".into(), sha256: Some("00".repeat(32)), ..good.clone() };
        let err = download(&bad, &dir, &progress).unwrap_err();
        assert!(err.contains("dañada"), "{err}");
        assert!(!dir.join("Otra.exe").exists() && !dir.join("Otra.exe.part").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
