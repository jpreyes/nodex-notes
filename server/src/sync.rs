//! Sincronización de las notas de cada cuenta (la «sincronización propia»).
//!
//! Funciona como un Dropbox pequeño: cada archivo de la carpeta de notas (por su ruta relativa,
//! «Obra Talca/Muro.md») tiene una versión, un número (`seq`) que sube con cada cambio de la cuenta.
//! Un equipo sube un cambio diciendo de qué versión partió (`base`); si no es la vigente, el
//! servidor responde 409 con la vigente y el equipo junta y vuelve a subir. Nada se pisa.
//!
//! - Contenido: `sync/<cuenta>/blobs/ab/abcdef….gz` por su huella SHA-256, comprimido (no se borra:
//!   es el historial). Si comprimirlo no ahorra nada (una foto, un PDF) queda sin `.gz`, tal cual.
//! - Índice: `sync/<cuenta>/diario.jsonl`, una línea por cambio; al arrancar se rehace leyéndolo.
//! - Avisos al instante: `GET /v1/sync/esperar?desde=N` responde apenas hay algo más nuevo que N
//!   (o a los 25 s).

use super::{AppState, error};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, watch};

/// Tamaño máximo de un archivo.
pub const MAX_FILE: usize = 50 * 1024 * 1024;
/// Cuánto espera `esperar` antes de responder sin cambios.
const WAIT: Duration = Duration::from_secs(25);

/// La versión vigente de un archivo.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Entry {
    pub ruta: String,
    pub seq: u64,
    pub hash: String,
    pub tam: u64,
    pub borrado: bool,
}

/// El índice de una cuenta.
#[derive(Debug, Default)]
pub struct Index {
    pub seq: u64,
    pub files: BTreeMap<String, Entry>,
    /// Contenido guardado (huella → tamaño), para medir el espacio usado.
    blobs: HashMap<String, u64>,
}

impl Index {
    pub fn used(&self) -> u64 {
        self.blobs.values().sum()
    }
}

/// Índices abiertos y avisos, por cuenta.
#[derive(Default)]
pub struct Hub {
    indexes: Mutex<HashMap<String, Arc<Mutex<Index>>>>,
    notify: Mutex<HashMap<String, watch::Sender<u64>>>,
}

fn dir(data: &Path, account: &str) -> PathBuf {
    data.join("sync").join(account)
}

fn blob_path(data: &Path, account: &str, hash: &str) -> PathBuf {
    dir(data, account).join("blobs").join(&hash[..2]).join(hash)
}

/// El contenido de una versión (comprimido o no).
fn read_blob(file: &Path) -> std::io::Result<Vec<u8>> {
    match std::fs::File::open(file.with_extension("gz")) {
        Ok(f) => {
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(f), &mut out)?;
            Ok(out)
        }
        Err(_) => std::fs::read(file),
    }
}

fn has_blob(file: &Path) -> bool {
    file.exists() || file.with_extension("gz").exists()
}

/// Guarda el contenido de una versión: comprimido si así ocupa menos.
fn write_blob(file: &Path, body: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    enc.write_all(body)?;
    let gz = enc.finish()?;
    let (dest, bytes) = if gz.len() < body.len() { (file.with_extension("gz"), gz.as_slice()) } else { (file.to_path_buf(), body) };
    let tmp = file.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &dest)
}

/// Comprime lo que se guardó antes sin comprimir (al arrancar el servidor). Devuelve cuántos bytes ahorró.
pub fn compress_old(data: &Path) -> u64 {
    let mut saved = 0;
    for acc in std::fs::read_dir(data.join("sync")).into_iter().flatten().flatten() {
        for sub in std::fs::read_dir(acc.path().join("blobs")).into_iter().flatten().flatten() {
            for f in std::fs::read_dir(sub.path()).into_iter().flatten().flatten() {
                let p = f.path();
                let name = f.file_name().to_string_lossy().into_owned();
                if !valid_hash(&name) || p.with_extension("gz").exists() {
                    continue;
                }
                let Ok(body) = std::fs::read(&p) else { continue };
                if write_blob(&p, &body).is_ok() && p.with_extension("gz").exists() {
                    let gz = std::fs::metadata(p.with_extension("gz")).map(|m| m.len()).unwrap_or(0);
                    if read_blob(&p).is_ok_and(|b| b == body) && std::fs::remove_file(&p).is_ok() {
                        saved += (body.len() as u64).saturating_sub(gz);
                    }
                }
            }
        }
    }
    saved
}

fn valid_hash(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// ¿Es una ruta aceptable? (relativa, con «/», sin «..» ni partes vacías)
pub fn valid_path(ruta: &str) -> bool {
    !ruta.is_empty()
        && ruta.len() <= 1024
        && !ruta.starts_with('/')
        && !ruta.contains('\\')
        && !ruta.contains(':')
        && ruta.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
}

/// Lee el diario de una cuenta.
fn load(data: &Path, account: &str) -> Index {
    let mut ix = Index::default();
    let text = std::fs::read_to_string(dir(data, account).join("diario.jsonl")).unwrap_or_default();
    for line in text.lines() {
        let Ok(e) = serde_json::from_str::<Entry>(line) else { continue };
        ix.seq = ix.seq.max(e.seq);
        if !e.borrado {
            ix.blobs.insert(e.hash.clone(), e.tam);
        }
        ix.files.insert(e.ruta.clone(), e);
    }
    ix
}

impl Hub {
    async fn index(&self, data: &Path, account: &str) -> Arc<Mutex<Index>> {
        let mut map = self.indexes.lock().await;
        map.entry(account.to_string()).or_insert_with(|| Arc::new(Mutex::new(load(data, account)))).clone()
    }

    async fn sender(&self, account: &str) -> watch::Sender<u64> {
        let mut map = self.notify.lock().await;
        map.entry(account.to_string()).or_insert_with(|| watch::channel(0).0).clone()
    }
}

/// Anota un cambio en el diario y en el índice, y avisa a los equipos que esperan.
async fn record(state: &AppState, account: &str, ix: &mut Index, mut e: Entry) -> std::io::Result<u64> {
    ix.seq += 1;
    e.seq = ix.seq;
    let d = dir(&state.settings.data, account);
    std::fs::create_dir_all(&d)?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(d.join("diario.jsonl"))?;
    writeln!(f, "{}", serde_json::to_string(&e).unwrap_or_default())?;
    if !e.borrado {
        ix.blobs.insert(e.hash.clone(), e.tam);
    }
    ix.files.insert(e.ruta.clone(), e);
    let _ = state.sync.sender(account).await.send(ix.seq);
    Ok(ix.seq)
}

#[derive(Deserialize)]
pub struct Since {
    #[serde(default)]
    desde: u64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Changes {
    pub seq: u64,
    pub cambios: Vec<Entry>,
    pub usado: u64,
    pub limite: u64,
}

/// Lo que cambió desde `desde` (con 0, todo lo que existe).
pub async fn changes(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<Since>) -> Response {
    let acc = match crate::accounts::account_of(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let ix = state.sync.index(&state.settings.data, &acc.id).await;
    let ix = ix.lock().await;
    let cambios: Vec<Entry> = ix.files.values().filter(|e| e.seq > q.desde && !(q.desde == 0 && e.borrado)).cloned().collect();
    Json(Changes { seq: ix.seq, cambios, usado: ix.used(), limite: state.settings.sync_limit }).into_response()
}

/// Espera hasta que haya algo más nuevo que `desde` (o 25 s).
pub async fn wait(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<Since>) -> Response {
    let acc = match crate::accounts::account_of(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let current = state.sync.index(&state.settings.data, &acc.id).await.lock().await.seq;
    if current > q.desde {
        return Json(serde_json::json!({ "seq": current })).into_response();
    }
    let mut rx = state.sync.sender(&acc.id).await.subscribe();
    let _ = tokio::time::timeout(WAIT, async {
        while rx.changed().await.is_ok() {
            if *rx.borrow() > q.desde {
                break;
            }
        }
    })
    .await;
    let seq = state.sync.index(&state.settings.data, &acc.id).await.lock().await.seq;
    Json(serde_json::json!({ "seq": seq })).into_response()
}

/// El contenido de una versión, por su huella.
pub async fn blob(State(state): State<Arc<AppState>>, headers: HeaderMap, UrlPath(hash): UrlPath<String>) -> Response {
    let acc = match crate::accounts::account_of(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !valid_hash(&hash) {
        return error(StatusCode::BAD_REQUEST, "Huella inválida.");
    }
    match read_blob(&blob_path(&state.settings.data, &acc.id, &hash)) {
        Ok(b) => ([("content-type", "application/octet-stream")], b).into_response(),
        Err(_) => error(StatusCode::NOT_FOUND, "No existe esa versión."),
    }
}

#[derive(Deserialize)]
pub struct Target {
    ruta: String,
    #[serde(default)]
    base: u64,
}

/// La respuesta 409: la versión vigente, para que el equipo la junte con la suya.
fn conflict(e: Option<&Entry>, seq: u64) -> Response {
    let cur = e.cloned().unwrap_or(Entry { seq, ..Entry::default() });
    (StatusCode::CONFLICT, Json(cur)).into_response()
}

/// Subir un archivo (el cuerpo es su contenido) partiendo de la versión `base` (0 = nuevo).
pub async fn put(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(t): Query<Target>, body: Bytes) -> Response {
    let acc = match crate::accounts::account_of(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !valid_path(&t.ruta) {
        return error(StatusCode::BAD_REQUEST, "Ruta inválida.");
    }
    if body.len() > MAX_FILE {
        return error(StatusCode::PAYLOAD_TOO_LARGE, "El archivo es demasiado grande para sincronizarlo (máximo 50 MB).");
    }
    let hash = sha256(&body);
    let data = state.settings.data.clone();
    let ix = state.sync.index(&data, &acc.id).await;
    let mut ix = ix.lock().await;
    let cur = ix.files.get(&t.ruta);
    let cur_seq = cur.map_or(0, |e| e.seq);
    if cur_seq != t.base {
        return conflict(cur, cur_seq);
    }
    // Lo mismo que ya está: no es un cambio.
    if cur.is_some_and(|e| !e.borrado && e.hash == hash) {
        return Json(serde_json::json!({ "seq": cur_seq, "hash": hash })).into_response();
    }
    let new_blob = !ix.blobs.contains_key(&hash);
    if new_blob && ix.used() + body.len() as u64 > state.settings.sync_limit {
        return error(StatusCode::INSUFFICIENT_STORAGE, "Se llenó el espacio de tu cuenta para sincronizar.");
    }
    let file = blob_path(&data, &acc.id, &hash);
    if !has_blob(&file) && write_blob(&file, &body).is_err() {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "No se pudo guardar el archivo; inténtalo de nuevo.");
    }
    let e = Entry { ruta: t.ruta.clone(), seq: 0, hash: hash.clone(), tam: body.len() as u64, borrado: false };
    match record(&state, &acc.id, &mut ix, e).await {
        Ok(seq) => Json(serde_json::json!({ "seq": seq, "hash": hash })).into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "No se pudo guardar el cambio; inténtalo de nuevo."),
    }
}

/// Borrar un archivo partiendo de la versión `base` (su contenido queda en el historial).
pub async fn delete(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(t): Query<Target>) -> Response {
    let acc = match crate::accounts::account_of(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !valid_path(&t.ruta) {
        return error(StatusCode::BAD_REQUEST, "Ruta inválida.");
    }
    let ix = state.sync.index(&state.settings.data, &acc.id).await;
    let mut ix = ix.lock().await;
    let cur = ix.files.get(&t.ruta);
    let cur_seq = cur.map_or(0, |e| e.seq);
    if cur.is_none_or(|e| e.borrado) {
        return Json(serde_json::json!({ "seq": cur_seq })).into_response();
    }
    if cur_seq != t.base {
        return conflict(cur, cur_seq);
    }
    let e = Entry { ruta: t.ruta.clone(), seq: 0, hash: String::new(), tam: 0, borrado: true };
    match record(&state, &acc.id, &mut ix, e).await {
        Ok(seq) => Json(serde_json::json!({ "seq": seq })).into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "No se pudo guardar el cambio; inténtalo de nuevo."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_compressed() {
        let data = std::env::temp_dir().join(format!("nodex-ia-blobs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let text = "Revisar el muro del eje 3 y enviar los planos.
".repeat(200).into_bytes();
        let f = blob_path(&data, "c1", &sha256(&text));
        write_blob(&f, &text).unwrap();
        assert!(f.with_extension("gz").exists() && !f.exists(), "el texto queda comprimido");
        assert!(std::fs::metadata(f.with_extension("gz")).unwrap().len() < text.len() as u64 / 10);
        assert_eq!(read_blob(&f).unwrap(), text);
        // Lo que no se achica (ya comprimido) queda tal cual.
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let noise: Vec<u8> = (0..4000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        let g = blob_path(&data, "c1", &sha256(&noise));
        write_blob(&g, &noise).unwrap();
        assert!(g.exists() && !g.with_extension("gz").exists());
        assert_eq!(read_blob(&g).unwrap(), noise);
        // Lo guardado antes sin comprimir se comprime al arrancar.
        let old = b"una nota vieja, una nota vieja, una nota vieja, una nota vieja".to_vec();
        let h = blob_path(&data, "c2", &sha256(&old));
        std::fs::create_dir_all(h.parent().unwrap()).unwrap();
        std::fs::write(&h, &old).unwrap();
        assert!(compress_old(&data) > 0);
        assert!(!h.exists() && has_blob(&h));
        assert_eq!(read_blob(&h).unwrap(), old);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn paths() {
        assert!(valid_path("Obra Talca/Muro.md") && valid_path(".nodex/dudas.json"));
        assert!(!valid_path("../x") && !valid_path("/etc/passwd") && !valid_path("a//b") && !valid_path("C:/x") && !valid_path("a\\b"));
    }
}
