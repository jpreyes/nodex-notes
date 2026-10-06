//! Sincronización propia: la carpeta de notas con el servidor de Notas (tu cuenta), sin Dropbox.
//!
//! Funciona como un Dropbox pequeño dentro de la app: un hilo aparte sube lo que cambia aquí y
//! baja lo que cambia en otros equipos (el servidor avisa al instante). La app no se entera de
//! cómo llegó un cambio: lo ve en disco como si lo hubiera escrito Dropbox, y todo lo de M2 sirve
//! igual (la nota abierta se junta, los datos internos se juntan desde sus copias).
//!
//! Cada archivo recuerda la versión de la que partió (`Base`: número y huella). Al subir, el
//! servidor acepta solo si nadie lo cambió desde esa versión; si no, en la vuelta siguiente se
//! baja la vigente y se junta:
//! - notas y texto (`.md`, `.txt`): línea por línea con la versión base (`merge::merge3`);
//! - lo demás (datos internos, tareas, imágenes): la otra versión queda como «copia en conflicto»
//!   al lado, y la app la junta sola (o quedan las dos, si es una imagen).
//!
//! Lo que recuerda este equipo va en `sincronizacion-<carpeta>.json` junto a la configuración.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Tamaño máximo de un archivo que se sincroniza.
pub const MAX_FILE: u64 = 50 * 1024 * 1024;
/// Cada cuánto se revisa la carpeta aunque nada avise.
const RESCAN: Duration = Duration::from_secs(15);
/// La clave de la configuración de la cuenta (ver `account::key`).
const KEY_FILE: &str = ".nodex/cuenta.clave";
/// La marca de las copias que deja la sincronización (la reconoce `conflicts::original_of`).
const COPY_MARK: &str = "copia en conflicto de Notas";

/// La versión de la que partió un archivo de este equipo.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Base {
    pub seq: u64,
    /// Vacía si la última versión conocida es «borrado».
    pub hash: String,
    /// Fecha (ms) y tamaño del archivo local cuando tenía esa huella (para no recalcularla).
    pub mtime: u64,
    pub tam: u64,
}

/// Lo que recuerda este equipo.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct State {
    pub seq: u64,
    pub archivos: BTreeMap<String, Base>,
}

/// Una versión en el servidor.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Entry {
    pub ruta: String,
    pub seq: u64,
    pub hash: String,
    pub tam: u64,
    pub borrado: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Changes {
    seq: u64,
    cambios: Vec<Entry>,
    usado: u64,
    limite: u64,
}

/// Cómo va (para mostrarlo en la app).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    /// Está subiendo o bajando.
    pub busy: bool,
    /// Archivos que faltan por subir o bajar.
    pub pending: usize,
    /// Última vez que quedó todo al día.
    pub synced_at: Option<chrono::DateTime<chrono::Local>>,
    /// Sin conexión o un error (se reintenta solo).
    pub error: Option<String>,
    pub used: u64,
    pub limit: u64,
}

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn mtime_ms(m: &fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64)
}

/// ¿Se sincroniza este archivo? (ruta relativa con «/»)
pub fn syncable(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let first = rel.split('/').next().unwrap_or(rel);
    if first.starts_with('.') && first != ".nodex" && first != ".papelera" {
        return false;
    }
    let lower = name.to_lowercase();
    if lower.ends_with(".tmp") || lower.ends_with(".part") || lower.starts_with("~$") || lower.starts_with(".~") {
        return false;
    }
    if matches!(lower.as_str(), ".ds_store" | "thumbs.db" | "desktop.ini") {
        return false;
    }
    // Las copias en conflicto se juntan en este equipo; no se suben.
    crate::conflicts::original_of(name).is_none()
}

/// Los archivos de la carpeta que se sincronizan: ruta relativa → (fecha ms, tamaño).
pub fn scan(root: &Path) -> BTreeMap<String, (u64, u64)> {
    fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, (u64, u64)>) {
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                if rel.is_empty() && name.starts_with('.') && name != ".nodex" && name != ".papelera" {
                    continue;
                }
                walk(&e.path(), &r, out);
            } else if t.is_file() && syncable(&r) {
                if let Ok(m) = e.metadata() {
                    if m.len() <= MAX_FILE {
                        out.insert(r, (mtime_ms(&m), m.len()));
                    }
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out);
    out
}

/// El archivo local de una ruta.
fn local(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, part| p.join(part))
}

/// Escribe sin dejar el archivo a medias.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    let tmp = path.with_file_name(format!("{}.nodex-sync.tmp", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

/// «Muro.md» → «Muro (copia en conflicto de Notas).md» (o con número si ya existe).
fn copy_name(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    let mut p = path.with_file_name(format!("{stem} ({COPY_MARK}){ext}"));
    let mut i = 2;
    while p.exists() {
        p = path.with_file_name(format!("{stem} ({COPY_MARK}) ({i}){ext}"));
        i += 1;
    }
    p
}

/// ¿Se junta línea por línea? (las notas y el texto; las listas de la app se juntan desde copias)
fn mergeable(rel: &str) -> bool {
    let lower = rel.to_lowercase();
    (lower.ends_with(".md") || lower.ends_with(".txt")) && !rel.starts_with(".nodex/") && !matches!(rel, "tareas.txt" | "agenda.txt" | "aprendido.txt")
}

/// La conexión con el servidor.
pub struct Remote {
    pub base: String,
    pub token: String,
    rt: tokio::runtime::Runtime,
    http: reqwest::Client,
}

impl Remote {
    pub fn new(base: &str, token: &str) -> Result<Remote, String> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
        let http = reqwest::Client::builder()
            .user_agent(concat!("nodex-notes/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Remote { base: base.trim_end_matches('/').to_string(), token: token.to_string(), rt, http })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn fail(e: reqwest::Error) -> String {
        format!("sin conexión con el servidor ({e})")
    }

    async fn message(r: reqwest::Response) -> String {
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.pointer("/error/message").and_then(|m| m.as_str()).map(str::to_string))
            .unwrap_or_else(|| tf!("el servidor respondió {status}", status = status))
    }

    fn changes(&self, since: u64) -> Result<Changes, String> {
        self.rt.block_on(async {
            let r = self.http.get(self.url("/v1/sync/cambios")).query(&[("desde", since)]).bearer_auth(&self.token).send().await.map_err(Self::fail)?;
            if !r.status().is_success() {
                return Err(Self::message(r).await);
            }
            r.json().await.map_err(|e| e.to_string())
        })
    }

    fn blob(&self, hash: &str) -> Result<Vec<u8>, String> {
        self.rt.block_on(async {
            let r = self.http.get(self.url(&format!("/v1/sync/blob/{hash}"))).bearer_auth(&self.token).send().await.map_err(Self::fail)?;
            if !r.status().is_success() {
                return Err(Self::message(r).await);
            }
            r.bytes().await.map(|b| b.to_vec()).map_err(|e| e.to_string())
        })
    }

    /// Sube un archivo. `Ok(Some(seq))` si quedó; `Ok(None)` si había una versión más nueva.
    fn put(&self, rel: &str, base: u64, bytes: Vec<u8>) -> Result<Option<u64>, String> {
        self.rt.block_on(async {
            let r = self
                .http
                .put(self.url("/v1/sync/archivo"))
                .query(&[("ruta", rel), ("base", &base.to_string())])
                .bearer_auth(&self.token)
                .body(bytes)
                .send()
                .await
                .map_err(Self::fail)?;
            match r.status().as_u16() {
                200 => Ok(r.json::<serde_json::Value>().await.ok().and_then(|v| v["seq"].as_u64())),
                409 => Ok(None),
                _ => Err(Self::message(r).await),
            }
        })
    }

    fn delete(&self, rel: &str, base: u64) -> Result<Option<u64>, String> {
        self.rt.block_on(async {
            let r = self
                .http
                .delete(self.url("/v1/sync/archivo"))
                .query(&[("ruta", rel), ("base", &base.to_string())])
                .bearer_auth(&self.token)
                .send()
                .await
                .map_err(Self::fail)?;
            match r.status().as_u16() {
                200 => Ok(r.json::<serde_json::Value>().await.ok().and_then(|v| v["seq"].as_u64())),
                409 => Ok(None),
                _ => Err(Self::message(r).await),
            }
        })
    }

    /// Espera (hasta ~25 s) a que haya algo más nuevo que `since`. Devuelve el número vigente.
    fn wait(&self, since: u64) -> Result<u64, String> {
        self.rt.block_on(async {
            let r = self
                .http
                .get(self.url("/v1/sync/esperar"))
                .query(&[("desde", since)])
                .bearer_auth(&self.token)
                .timeout(Duration::from_secs(40))
                .send()
                .await
                .map_err(Self::fail)?;
            if !r.status().is_success() {
                return Err(Self::message(r).await);
            }
            Ok(r.json::<serde_json::Value>().await.ok().and_then(|v| v["seq"].as_u64()).unwrap_or(since))
        })
    }
}

/// Un equipo sincronizando una carpeta.
pub struct Engine {
    pub root: PathBuf,
    pub state: State,
    state_file: PathBuf,
    pub remote: Remote,
    pub status: Status,
}

/// Lo que dejó una vuelta.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Report {
    pub downloaded: usize,
    pub uploaded: usize,
    pub deleted: usize,
    /// Quedaron dos versiones (una como copia, para juntarla) o se juntaron línea por línea.
    pub merged: usize,
}

/// Dónde guarda este equipo lo que recuerda de esa carpeta.
pub fn state_file(root: &Path) -> PathBuf {
    let key = format!("{:x}", crate::ai::fnv(&root.to_string_lossy()));
    crate::config::config_path().with_file_name(format!("sincronizacion-{key}.json"))
}

impl Engine {
    pub fn new(root: PathBuf, remote: Remote, state_file: PathBuf) -> Engine {
        let state = crate::vault::read_text(&state_file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Engine { root, state, state_file, remote, status: Status::default() }
    }

    fn save(&self) {
        if let Some(d) = self.state_file.parent() {
            let _ = fs::create_dir_all(d);
        }
        let _ = fs::write(&self.state_file, serde_json::to_string(&self.state).unwrap_or_default());
    }

    /// La huella del archivo local (la de su versión base si no cambió desde entonces).
    fn local_hash(&self, rel: &str, meta: (u64, u64)) -> Option<String> {
        if let Some(b) = self.state.archivos.get(rel).filter(|b| !b.hash.is_empty() && b.mtime == meta.0 && b.tam == meta.1) {
            return Some(b.hash.clone());
        }
        fs::read(local(&self.root, rel)).ok().map(|b| sha256(&b))
    }

    fn remember(&mut self, rel: &str, seq: u64, hash: &str) {
        let (mtime, tam) = fs::metadata(local(&self.root, rel)).map(|m| (mtime_ms(&m), m.len())).unwrap_or((0, 0));
        self.state.archivos.insert(rel.to_string(), Base { seq, hash: hash.to_string(), mtime, tam });
    }

    /// Una vuelta: bajar lo nuevo del servidor (juntando si hace falta) y subir lo de aquí.
    pub fn cycle(&mut self) -> Result<Report, String> {
        let mut report = Report::default();
        self.pull(&mut report)?;
        self.push(&mut report)?;
        self.save();
        Ok(report)
    }

    fn pull(&mut self, report: &mut Report) -> Result<(), String> {
        let ch = self.remote.changes(self.state.seq)?;
        self.status.used = ch.usado;
        self.status.limit = ch.limite;
        let files = scan(&self.root);
        for c in ch.cambios {
            if !crate::sync::syncable(&c.ruta) {
                continue;
            }
            let base = self.state.archivos.get(&c.ruta).cloned();
            if base.as_ref().is_some_and(|b| b.seq == c.seq) {
                continue; // ya la tenemos (por ejemplo, la subimos nosotros)
            }
            let path = local(&self.root, &c.ruta);
            let here = files.get(&c.ruta).and_then(|m| self.local_hash(&c.ruta, *m));
            let changed_here = match (&base, &here) {
                (Some(b), Some(h)) => *h != b.hash,
                (Some(b), None) => !b.hash.is_empty(),
                (None, Some(_)) => true,
                (None, None) => false,
            };
            // La clave que cifra la configuración de la cuenta: gana la que ya estaba en el
            // servidor (si cada equipo usara la suya, no podrían leer la configuración del otro).
            let changed_here = changed_here && c.ruta != KEY_FILE;
            if c.borrado {
                if !changed_here && here.is_some() {
                    let _ = fs::remove_file(&path);
                    report.deleted += 1;
                }
                if changed_here {
                    // Lo cambiaste aquí: se queda y se vuelve a subir.
                    self.state.archivos.insert(c.ruta.clone(), Base { seq: c.seq, ..Base::default() });
                } else {
                    self.state.archivos.remove(&c.ruta);
                }
                continue;
            }
            if !changed_here || here.as_deref() == Some(c.hash.as_str()) {
                let bytes = self.remote.blob(&c.hash)?;
                write_atomic(&path, &bytes).map_err(|e| tf!("no se pudo escribir {ruta}: {e}", ruta = c.ruta, e = e))?;
                self.remember(&c.ruta, c.seq, &c.hash);
                report.downloaded += 1;
                continue;
            }
            // Cambió aquí y allá: se juntan.
            let remote = self.remote.blob(&c.hash)?;
            let mine = fs::read(&path).unwrap_or_default();
            if mergeable(&c.ruta) {
                let theirs = String::from_utf8_lossy(&remote).into_owned();
                let ours = String::from_utf8_lossy(&mine).into_owned();
                let merged = match base.as_ref().filter(|b| !b.hash.is_empty()) {
                    Some(b) => {
                        let old = String::from_utf8_lossy(&self.remote.blob(&b.hash)?).into_owned();
                        crate::merge::merge3(&old, &ours, &theirs).text
                    }
                    None => crate::merge::union(&ours, &theirs),
                };
                write_atomic(&path, merged.as_bytes()).map_err(|e| tf!("no se pudo escribir {ruta}: {e}", ruta = c.ruta, e = e))?;
            } else {
                // La otra versión queda al lado; la app la junta (o quedan las dos).
                write_atomic(&copy_name(&path), &remote).map_err(|e| tf!("no se pudo escribir {ruta}: {e}", ruta = c.ruta, e = e))?;
            }
            // La base pasa a ser la del servidor (lo de aquí, ya juntado, se sube después).
            let tam = c.tam;
            self.state.archivos.insert(c.ruta.clone(), Base { seq: c.seq, hash: c.hash.clone(), mtime: 0, tam });
            report.merged += 1;
        }
        self.state.seq = self.state.seq.max(ch.seq);
        Ok(())
    }

    fn push(&mut self, report: &mut Report) -> Result<(), String> {
        let files = scan(&self.root);
        let mut todo: Vec<(String, Option<(u64, u64)>)> = Vec::new();
        for (rel, meta) in &files {
            let base = self.state.archivos.get(rel);
            let hash = self.local_hash(rel, *meta);
            if base.is_none() || hash.as_deref() != base.map(|b| b.hash.as_str()) {
                todo.push((rel.clone(), Some(*meta)));
            } else if let Some(b) = base.filter(|b| b.mtime != meta.0 || b.tam != meta.1) {
                // Mismo contenido, otra fecha: solo se recuerda la fecha nueva.
                let (seq, h) = (b.seq, b.hash.clone());
                self.state.archivos.insert(rel.clone(), Base { seq, hash: h, mtime: meta.0, tam: meta.1 });
            }
        }
        for (rel, b) in &self.state.archivos {
            if !b.hash.is_empty() && !files.contains_key(rel) {
                todo.push((rel.clone(), None));
            }
        }
        self.status.pending = todo.len();
        for (rel, meta) in todo {
            let base = self.state.archivos.get(&rel).map_or(0, |b| b.seq);
            match meta {
                Some(_) => {
                    let Ok(bytes) = fs::read(local(&self.root, &rel)) else { continue };
                    let hash = sha256(&bytes);
                    if let Some(seq) = self.remote.put(&rel, base, bytes)? {
                        self.remember(&rel, seq, &hash);
                        report.uploaded += 1;
                    }
                }
                None => {
                    if let Some(seq) = self.remote.delete(&rel, base)? {
                        self.state.archivos.insert(rel.clone(), Base { seq, ..Base::default() });
                        report.deleted += 1;
                    }
                }
            }
            self.status.pending = self.status.pending.saturating_sub(1);
        }
        Ok(())
    }
}

/// La sincronización corriendo en segundo plano.
pub struct Handle {
    pub status: Arc<Mutex<Status>>,
    kick: Sender<()>,
    stop: Arc<AtomicBool>,
}

impl Handle {
    /// Algo cambió aquí: revisar ya.
    pub fn kick(&self) {
        let _ = self.kick.send(());
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.kick.send(());
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Empieza a sincronizar `root` con la cuenta. `on_change` se llama cuando llegó algo (para
/// repintar la ventana).
pub fn start(root: PathBuf, base: String, token: String, on_change: impl Fn() + Send + Sync + 'static) -> Handle {
    let status = Arc::new(Mutex::new(Status::default()));
    let stop = Arc::new(AtomicBool::new(false));
    let (kick, rx): (Sender<()>, Receiver<()>) = mpsc::channel();
    let known = Arc::new(AtomicU64::new(0));
    let on_change = Arc::new(on_change);

    // Escucha al servidor: cuando otro equipo cambia algo, despierta la vuelta.
    {
        let (base, token, stop, known, kick) = (base.clone(), token.clone(), stop.clone(), known.clone(), kick.clone());
        std::thread::spawn(move || {
            let Ok(remote) = Remote::new(&base, &token) else { return };
            while !stop.load(Ordering::Relaxed) {
                let since = known.load(Ordering::Relaxed);
                match remote.wait(since) {
                    Ok(seq) if seq > since => {
                        let _ = kick.send(());
                        std::thread::sleep(Duration::from_millis(300));
                    }
                    Ok(_) => {}
                    Err(_) => std::thread::sleep(Duration::from_secs(10)),
                }
            }
        });
    }

    let st = status.clone();
    let stop2 = stop.clone();
    std::thread::spawn(move || {
        let Ok(remote) = Remote::new(&base, &token) else { return };
        let mut engine = Engine::new(root.clone(), remote, state_file(&root));
        let mut backoff = Duration::from_secs(5);
        while !stop2.load(Ordering::Relaxed) {
            if let Ok(mut s) = st.lock() {
                s.busy = true;
            }
            on_change();
            let r = engine.cycle();
            known.store(engine.state.seq, Ordering::Relaxed);
            let wait = match &r {
                Ok(_) => {
                    backoff = Duration::from_secs(5);
                    RESCAN
                }
                Err(_) => {
                    backoff = (backoff * 2).min(Duration::from_secs(120));
                    backoff
                }
            };
            if let Ok(mut s) = st.lock() {
                let mut new = engine.status.clone();
                new.busy = false;
                match r {
                    Ok(_) => {
                        new.synced_at = Some(chrono::Local::now());
                        new.error = None;
                        new.pending = 0;
                    }
                    Err(e) => {
                        new.synced_at = s.synced_at;
                        new.error = Some(e);
                    }
                }
                *s = new;
            }
            on_change();
            // Esperar: un aviso (de aquí o del servidor) o el próximo repaso.
            let until = Instant::now() + wait;
            let _ = rx.recv_timeout(wait);
            // Juntar varios avisos seguidos en una sola vuelta.
            std::thread::sleep(Duration::from_millis(400).min(until.saturating_duration_since(Instant::now())));
            while rx.try_recv().is_ok() {}
        }
    });
    Handle { status, kick, stop }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un servidor de verdad (en esta prueba) con una cuenta aprobada; devuelve su dirección y la sesión.
    fn server(name: &str) -> (tokio::runtime::Runtime, String, String, PathBuf) {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let data = std::env::temp_dir().join(format!("nodex-sync-srv-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&data);
        let mut acc = nodex_ia::accounts::Accounts::default();
        acc.cuentas.insert(
            "a1".into(),
            nodex_ia::accounts::Account { id: "a1".into(), correo: "ana@obra.cl".into(), plan: "pro".into(), clave: nodex_ia::accounts::hash_password("clave-segura").unwrap(), ..Default::default() },
        );
        acc.save(&data).unwrap();
        let d2 = data.clone();
        let base = rt.block_on(async move {
            let settings = nodex_ia::Settings { key: "k".into(), data: d2, ..Default::default() };
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", l.local_addr().unwrap());
            tokio::spawn(async move { axum::serve(l, nodex_ia::router(nodex_ia::state(settings))).await.unwrap() });
            base
        });
        let ctx = eframe::egui::Context::default();
        let s = crate::account::sign_in_password(base.clone(), "ana@obra.cl".into(), "clave-segura".into(), ctx).recv().unwrap().unwrap();
        (rt, base, s.token, data)
    }

    fn device(name: &str, base: &str, token: &str) -> Engine {
        let root = std::env::temp_dir().join(format!("nodex-sync-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let state = root.with_extension("estado.json");
        let _ = fs::remove_file(&state);
        Engine::new(root, Remote::new(base, token).unwrap(), state)
    }

    fn write(e: &Engine, rel: &str, text: &str) {
        let p = local(&e.root, rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        // (otra fecha de modificación, como al escribir de verdad)
        std::thread::sleep(Duration::from_millis(20));
        fs::write(p, text).unwrap();
    }

    fn read(e: &Engine, rel: &str) -> Option<String> {
        fs::read_to_string(local(&e.root, rel)).ok()
    }

    /// Dos equipos de la misma cuenta, sin Dropbox: lo que se escribe en uno llega al otro, lo
    /// que cambian los dos a la vez se junta, y nada se pierde.
    #[test]
    fn two_devices_stay_in_sync() {
        let (_rt, base, token, data) = server("dos");
        let mut a = device("a", &base, &token);
        let mut b = device("b", &base, &token);

        // A escribe; B lo recibe (y la imagen, igual).
        write(&a, "Obra/Muro.md", "Revisar armado\nPedir acero\nLlamar a Juan\n");
        fs::create_dir_all(local(&a.root, "Adjuntos")).unwrap();
        fs::write(local(&a.root, "Adjuntos/foto.png"), [137u8, 80, 78, 71, 0, 1, 2, 3]).unwrap();
        assert_eq!(a.cycle().unwrap().uploaded, 2);
        assert_eq!(b.cycle().unwrap().downloaded, 2);
        assert_eq!(read(&b, "Obra/Muro.md").as_deref(), Some("Revisar armado\nPedir acero\nLlamar a Juan\n"));
        assert_eq!(fs::read(local(&b.root, "Adjuntos/foto.png")).unwrap(), vec![137u8, 80, 78, 71, 0, 1, 2, 3]);
        // Una vuelta sin cambios no sube ni baja nada.
        assert_eq!(a.cycle().unwrap(), Report { downloaded: 0, uploaded: 0, deleted: 0, merged: 0 });

        // Los dos cambian líneas distintas de la misma nota (sin conexión entre medio).
        write(&a, "Obra/Muro.md", "Revisar armado del muro sur\nPedir acero\nLlamar a Juan\n");
        write(&b, "Obra/Muro.md", "Revisar armado\nPedir acero\nLlamar a Juan el lunes\n");
        a.cycle().unwrap();
        let r = b.cycle().unwrap();
        assert_eq!(r.merged, 1);
        let joined = "Revisar armado del muro sur\nPedir acero\nLlamar a Juan el lunes\n";
        assert_eq!(read(&b, "Obra/Muro.md").as_deref(), Some(joined), "se juntan las dos");
        a.cycle().unwrap();
        assert_eq!(read(&a, "Obra/Muro.md").as_deref(), Some(joined));

        // Un dato interno cambiado en los dos: la otra versión queda como copia para juntarla.
        write(&a, ".nodex/dudas.json", "{\"de\": \"a\"}");
        a.cycle().unwrap();
        b.cycle().unwrap();
        write(&a, ".nodex/dudas.json", "{\"de\": \"a2\"}");
        write(&b, ".nodex/dudas.json", "{\"de\": \"b2\"}");
        a.cycle().unwrap();
        b.cycle().unwrap();
        assert_eq!(read(&b, ".nodex/dudas (copia en conflicto de Notas).json").as_deref(), Some("{\"de\": \"a2\"}"));
        assert_eq!(read(&b, ".nodex/dudas.json").as_deref(), Some("{\"de\": \"b2\"}"));

        // Borrar en A lo borra en B (si B no lo había cambiado).
        fs::remove_file(local(&a.root, "Adjuntos/foto.png")).unwrap();
        assert_eq!(a.cycle().unwrap().deleted, 1);
        assert_eq!(b.cycle().unwrap().deleted, 1);
        assert!(!local(&b.root, "Adjuntos/foto.png").exists());

        // Un equipo nuevo con la carpeta vacía recibe todo.
        let mut c = device("c", &base, &token);
        c.cycle().unwrap();
        assert_eq!(read(&c, "Obra/Muro.md").as_deref(), Some(joined));
        assert!(!local(&c.root, "Adjuntos/foto.png").exists());
        assert!(c.status.used > 0);

        for e in [&a, &b, &c] {
            let _ = fs::remove_dir_all(&e.root);
        }
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn what_is_synced() {
        assert!(syncable("Obra Talca/Muro.md") && syncable(".nodex/dudas.json") && syncable(".papelera/x.md") && syncable("Adjuntos/foto.jpg"));
        assert!(!syncable(".git/config") && !syncable("Obra/Muro.md.nodex-sync.tmp") && !syncable("Obra/Thumbs.db"));
        assert!(!syncable("Obra/Muro (copia en conflicto de Notas).md"), "las copias se juntan aquí");
        assert_eq!(copy_name(Path::new("C:/x/Muro.md")).file_name().unwrap().to_string_lossy(), "Muro (copia en conflicto de Notas).md");
        assert_eq!(crate::conflicts::original_of("dudas (copia en conflicto de Notas).json").as_deref(), Some("dudas.json"));
        assert!(mergeable("Obra/Muro.md") && mergeable("Bloc.md") && !mergeable("tareas.txt") && !mergeable(".nodex/x.json") && !mergeable("Adjuntos/a.png"));
    }
}
