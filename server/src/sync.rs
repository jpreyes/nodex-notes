//! Sincronización de las notas de cada cuenta (la «sincronización propia»).
//!
//! Funciona como un Dropbox pequeño: cada archivo de la carpeta de notas (por su ruta relativa,
//! «Obra Talca/Muro.md») tiene una versión, un número (`seq`) que sube con cada cambio de la cuenta.
//! Un equipo sube un cambio diciendo de qué versión partió (`base`); si no es la vigente, el
//! servidor responde 409 con la vigente y el equipo junta y vuelve a subir. Nada se pisa.
//!
//! - Contenido: `sync/<cuenta>/blobs/ab/abcdef….gz` por su huella SHA-256, comprimido (no se borra:
//!   es el historial). Si comprimirlo no ahorra nada (una foto, un PDF) queda sin `.gz`, tal cual.
//! - Índice: en SQLite (`notas.db`, junto a los demás datos), para miles de cuentas sin tenerlas
//!   en memoria: `archivos` (la versión vigente de cada archivo), `cambios` (cada cambio, en orden:
//!   el historial), `contenidos` (qué contenido guarda cada cuenta, para medir su espacio) y
//!   `cuentas_sync` (el último número y el espacio usado). Lo que había en
//!   `sync/<cuenta>/diario.jsonl` (antes de SQLite) se pasa solo al arrancar.
//! - Respaldo: cada 6 horas, una copia consistente en `respaldo/notas.db` (la recoge el respaldo
//!   diario del servidor).
//! - Avisos al instante: `GET /v1/sync/esperar?desde=N` responde apenas hay algo más nuevo que N
//!   (o a los 25 s).

use super::{AppState, error};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, OwnedMutexGuard, watch};

/// Tamaño máximo de un archivo.
pub const MAX_FILE: usize = 50 * 1024 * 1024;
/// Cuánto espera `esperar` antes de responder sin cambios.
const WAIT: Duration = Duration::from_secs(25);
/// El archivo de la base de datos (en la carpeta de datos).
pub const DB_FILE: &str = "notas.db";
/// Cada cuánto se deja una copia consistente de la base para el respaldo.
pub const BACKUP_EVERY: Duration = Duration::from_secs(6 * 3600);

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

/// La base de datos, los turnos por cuenta (un cambio a la vez en cada una) y los avisos.
pub struct Hub {
    db: std::sync::Mutex<Connection>,
    turns: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    notify: Mutex<HashMap<String, watch::Sender<u64>>>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS archivos (
    cuenta TEXT NOT NULL, ruta TEXT NOT NULL, seq INTEGER NOT NULL, hash TEXT NOT NULL,
    tam INTEGER NOT NULL, borrado INTEGER NOT NULL, PRIMARY KEY (cuenta, ruta)
);
CREATE INDEX IF NOT EXISTS archivos_seq ON archivos (cuenta, seq);
CREATE TABLE IF NOT EXISTS cambios (
    cuenta TEXT NOT NULL, seq INTEGER NOT NULL, ruta TEXT NOT NULL, hash TEXT NOT NULL,
    tam INTEGER NOT NULL, borrado INTEGER NOT NULL, en INTEGER NOT NULL, PRIMARY KEY (cuenta, seq)
);
CREATE TABLE IF NOT EXISTS contenidos (
    cuenta TEXT NOT NULL, hash TEXT NOT NULL, tam INTEGER NOT NULL, PRIMARY KEY (cuenta, hash)
);
CREATE TABLE IF NOT EXISTS cuentas_sync (
    cuenta TEXT PRIMARY KEY, seq INTEGER NOT NULL, usado INTEGER NOT NULL
);
";

fn entry_of(r: &rusqlite::Row) -> rusqlite::Result<Entry> {
    Ok(Entry { ruta: r.get(0)?, seq: r.get::<_, i64>(1)? as u64, hash: r.get(2)?, tam: r.get::<_, i64>(3)? as u64, borrado: r.get::<_, i64>(4)? != 0 })
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Anota un cambio (dentro de una transacción): sube el número de la cuenta, lo guarda en el
/// historial, deja la versión vigente y suma el contenido nuevo al espacio usado.
fn record_in(tx: &rusqlite::Transaction, account: &str, e: &Entry, at: i64) -> rusqlite::Result<u64> {
    let seq: i64 = tx.query_row("SELECT seq FROM cuentas_sync WHERE cuenta = ?1", [account], |r| r.get(0)).optional()?.unwrap_or(0);
    let seq = if e.seq > 0 { (e.seq as i64).max(seq) } else { seq + 1 };
    let target = if e.seq > 0 { e.seq as i64 } else { seq };
    tx.execute(
        "INSERT OR REPLACE INTO cambios (cuenta, seq, ruta, hash, tam, borrado, en) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![account, target, e.ruta, e.hash, e.tam as i64, e.borrado as i64, at],
    )?;
    tx.execute(
        "INSERT INTO archivos (cuenta, ruta, seq, hash, tam, borrado) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (cuenta, ruta) DO UPDATE SET seq = excluded.seq, hash = excluded.hash, tam = excluded.tam, borrado = excluded.borrado
         WHERE excluded.seq >= archivos.seq",
        params![account, e.ruta, target, e.hash, e.tam as i64, e.borrado as i64],
    )?;
    let mut added = 0i64;
    if !e.borrado {
        added = tx.execute("INSERT OR IGNORE INTO contenidos (cuenta, hash, tam) VALUES (?1, ?2, ?3)", params![account, e.hash, e.tam as i64])? as i64 * e.tam as i64;
    }
    tx.execute(
        "INSERT INTO cuentas_sync (cuenta, seq, usado) VALUES (?1, ?2, ?3)
         ON CONFLICT (cuenta) DO UPDATE SET seq = max(cuentas_sync.seq, excluded.seq), usado = cuentas_sync.usado + ?3",
        params![account, seq, added],
    )?;
    Ok(target as u64)
}

/// Pasa a la base lo que había en `sync/<cuenta>/diario.jsonl` (antes de SQLite). Una vez por
/// cuenta: el diario queda como `diario.jsonl.migrado`. Devuelve cuántas cuentas pasó.
fn migrate(conn: &mut Connection, data: &Path) -> rusqlite::Result<usize> {
    let mut done = 0;
    for acc in std::fs::read_dir(data.join("sync")).into_iter().flatten().flatten() {
        let journal = acc.path().join("diario.jsonl");
        let Ok(text) = std::fs::read_to_string(&journal) else { continue };
        let account = acc.file_name().to_string_lossy().into_owned();
        let tx = conn.transaction()?;
        let already: bool = tx.query_row("SELECT 1 FROM cuentas_sync WHERE cuenta = ?1", [&account], |_| Ok(true)).optional()?.unwrap_or(false);
        if !already {
            for line in text.lines() {
                if let Ok(e) = serde_json::from_str::<Entry>(line) {
                    if e.seq > 0 {
                        record_in(&tx, &account, &e, 0)?;
                    }
                }
            }
        }
        tx.commit()?;
        let _ = std::fs::rename(&journal, journal.with_extension("jsonl.migrado"));
        done += 1;
    }
    Ok(done)
}

impl Hub {
    /// Abre (o crea) la base en la carpeta de datos y pasa lo que hubiera de antes.
    pub fn open(data: &Path) -> Hub {
        let _ = std::fs::create_dir_all(data);
        let mut conn = Connection::open(data.join(DB_FILE)).unwrap_or_else(|e| panic!("no se pudo abrir {}: {e}", data.join(DB_FILE).display()));
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");
        let _ = conn.busy_timeout(Duration::from_secs(5));
        conn.execute_batch(SCHEMA).expect("no se pudo preparar la base de datos");
        match migrate(&mut conn, data) {
            Ok(0) => {}
            Ok(n) => println!("sincronización: {n} cuentas pasadas a SQLite"),
            Err(e) => eprintln!("no se pudo pasar la sincronización a SQLite: {e}"),
        }
        Hub { db: std::sync::Mutex::new(conn), turns: Mutex::default(), notify: Mutex::default() }
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// El turno de una cuenta: los cambios de una misma cuenta van de a uno.
    async fn turn(&self, account: &str) -> OwnedMutexGuard<()> {
        let m = self.turns.lock().await.entry(account.to_string()).or_default().clone();
        m.lock_owned().await
    }

    async fn sender(&self, account: &str) -> watch::Sender<u64> {
        let mut map = self.notify.lock().await;
        map.entry(account.to_string()).or_insert_with(|| watch::channel(0).0).clone()
    }

    fn seq(&self, account: &str) -> u64 {
        self.db().query_row("SELECT seq FROM cuentas_sync WHERE cuenta = ?1", [account], |r| r.get::<_, i64>(0)).optional().ok().flatten().unwrap_or(0) as u64
    }

    fn used(&self, account: &str) -> u64 {
        self.db().query_row("SELECT usado FROM cuentas_sync WHERE cuenta = ?1", [account], |r| r.get::<_, i64>(0)).optional().ok().flatten().unwrap_or(0) as u64
    }

    fn has_content(&self, account: &str, hash: &str) -> bool {
        self.db().query_row("SELECT 1 FROM contenidos WHERE cuenta = ?1 AND hash = ?2", [account, hash], |_| Ok(())).optional().ok().flatten().is_some()
    }

    fn current(&self, account: &str, ruta: &str) -> Option<Entry> {
        self.db()
            .query_row("SELECT ruta, seq, hash, tam, borrado FROM archivos WHERE cuenta = ?1 AND ruta = ?2", [account, ruta], entry_of)
            .optional()
            .ok()
            .flatten()
    }

    /// Lo que cambió desde `desde` (con 0, todo lo que existe), en orden.
    fn since(&self, account: &str, desde: u64) -> Vec<Entry> {
        let db = self.db();
        let sql = if desde == 0 {
            "SELECT ruta, seq, hash, tam, borrado FROM archivos WHERE cuenta = ?1 AND borrado = 0 ORDER BY ruta"
        } else {
            "SELECT ruta, seq, hash, tam, borrado FROM archivos WHERE cuenta = ?1 AND seq > ?2 ORDER BY seq"
        };
        let Ok(mut st) = db.prepare_cached(sql) else { return Vec::new() };
        let rows = if desde == 0 { st.query_map(params![account], entry_of) } else { st.query_map(params![account, desde as i64], entry_of) };
        rows.map(|r| r.flatten().collect()).unwrap_or_default()
    }

    /// Anota un cambio y avisa a los equipos que esperan. Devuelve su número.
    async fn record(&self, account: &str, e: Entry) -> rusqlite::Result<u64> {
        let seq = {
            let mut db = self.db();
            let tx = db.transaction()?;
            let seq = record_in(&tx, account, &Entry { seq: 0, ..e }, now_secs())?;
            tx.commit()?;
            seq
        };
        let _ = self.sender(account).await.send(seq);
        Ok(seq)
    }

    /// Una copia consistente de la base (aunque se esté usando), para el respaldo.
    pub fn backup(&self, dest: &Path) -> rusqlite::Result<()> {
        if let Some(dir) = dest.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = dest.with_extension("tmp");
        let _ = std::fs::remove_file(&tmp);
        self.db().execute("VACUUM INTO ?1", [tmp.to_string_lossy()])?;
        std::fs::rename(&tmp, dest).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    }

    /// Cuántas cuentas sincronizan, cuántos archivos y cuánto espacio.
    pub fn totals(&self) -> (u64, u64, u64) {
        totals_of(&self.db())
    }
}

fn totals_of(db: &Connection) -> (u64, u64, u64) {
    let n = |sql: &str| db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0) as u64;
    (n("SELECT count(*) FROM cuentas_sync"), n("SELECT count(*) FROM archivos WHERE borrado = 0"), n("SELECT coalesce(sum(usado), 0) FROM cuentas_sync"))
}

/// Lo mismo, solo leyendo la base (para `nodex-ia lista`, con el servidor andando).
pub fn totals_at(data: &Path) -> Option<(u64, u64, u64)> {
    let path = data.join(DB_FILE);
    path.exists().then_some(())?;
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    Some(totals_of(&db))
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
    // El número primero: si algo cambia mientras se lee, la vuelta siguiente lo trae.
    let seq = state.sync.seq(&acc.id);
    let cambios = state.sync.since(&acc.id, q.desde);
    let seq = cambios.iter().map(|e| e.seq).max().unwrap_or(0).max(seq);
    Json(Changes { seq, cambios, usado: state.sync.used(&acc.id), limite: state.settings.sync_limit }).into_response()
}

/// Espera hasta que haya algo más nuevo que `desde` (o 25 s).
pub async fn wait(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<Since>) -> Response {
    let acc = match crate::accounts::account_of(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let mut rx = state.sync.sender(&acc.id).await.subscribe();
    let current = state.sync.seq(&acc.id);
    if current > q.desde {
        return Json(serde_json::json!({ "seq": current })).into_response();
    }
    let _ = tokio::time::timeout(WAIT, async {
        while rx.changed().await.is_ok() {
            if *rx.borrow() > q.desde {
                break;
            }
        }
    })
    .await;
    Json(serde_json::json!({ "seq": state.sync.seq(&acc.id) })).into_response()
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

/// Guarda un contenido nuevo de una cuenta (con su turno tomado). Ok(None) = es el mismo que ya
/// estaba; Err = la respuesta de error.
async fn store(state: &AppState, account: &str, ruta: &str, body: &[u8]) -> Result<Option<(u64, String)>, Response> {
    let hash = sha256(body);
    let cur = state.sync.current(account, ruta);
    if cur.as_ref().is_some_and(|e| !e.borrado && e.hash == hash) {
        return Ok(None);
    }
    let fresh = !state.sync.has_content(account, &hash);
    if fresh && state.sync.used(account) + body.len() as u64 > state.settings.sync_limit {
        return Err(error(StatusCode::INSUFFICIENT_STORAGE, "Se llenó el espacio de tu cuenta para sincronizar."));
    }
    let file = blob_path(&state.settings.data, account, &hash);
    if !has_blob(&file) && write_blob(&file, body).is_err() {
        return Err(error(StatusCode::INTERNAL_SERVER_ERROR, "No se pudo guardar el archivo; inténtalo de nuevo."));
    }
    let e = Entry { ruta: ruta.to_string(), seq: 0, hash: hash.clone(), tam: body.len() as u64, borrado: false };
    match state.sync.record(account, e).await {
        Ok(seq) => Ok(Some((seq, hash))),
        Err(_) => Err(error(StatusCode::INTERNAL_SERVER_ERROR, "No se pudo guardar el cambio; inténtalo de nuevo.")),
    }
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
    let _turn = state.sync.turn(&acc.id).await;
    let cur = state.sync.current(&acc.id, &t.ruta);
    let cur_seq = cur.as_ref().map_or(0, |e| e.seq);
    // Un archivo borrado se puede volver a crear sin conocer el borrado (base 0).
    let recreate = t.base == 0 && cur.as_ref().is_some_and(|e| e.borrado);
    if cur_seq != t.base && !recreate {
        return conflict(cur.as_ref(), cur_seq);
    }
    match store(&state, &acc.id, &t.ruta, &body).await {
        Ok(Some((seq, hash))) => Json(serde_json::json!({ "seq": seq, "hash": hash })).into_response(),
        // Lo mismo que ya está: no es un cambio.
        Ok(None) => Json(serde_json::json!({ "seq": cur_seq, "hash": sha256(&body) })).into_response(),
        Err(r) => r,
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
    let _turn = state.sync.turn(&acc.id).await;
    let cur = state.sync.current(&acc.id, &t.ruta);
    let cur_seq = cur.as_ref().map_or(0, |e| e.seq);
    if cur.as_ref().is_none_or(|e| e.borrado) {
        return Json(serde_json::json!({ "seq": cur_seq })).into_response();
    }
    if cur_seq != t.base {
        return conflict(cur.as_ref(), cur_seq);
    }
    let e = Entry { ruta: t.ruta.clone(), seq: 0, hash: String::new(), tam: 0, borrado: true };
    match state.sync.record(&acc.id, e).await {
        Ok(seq) => Json(serde_json::json!({ "seq": seq })).into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "No se pudo guardar el cambio; inténtalo de nuevo."),
    }
}

// Para la app del celular (`movil`): leer y cambiar archivos de la cuenta directo, como si los
// subiera un equipo más. Los equipos lo bajan al instante y, si tenían un cambio propio en el
// mismo archivo, lo juntan (como entre dos equipos).

/// Los archivos vigentes de una cuenta.
pub(crate) async fn files(state: &AppState, account: &str) -> Vec<Entry> {
    state.sync.since(account, 0)
}

/// El texto de una versión, por su huella.
pub(crate) fn blob_text(state: &AppState, account: &str, hash: &str) -> Option<String> {
    valid_hash(hash).then_some(())?;
    read_blob(&blob_path(&state.settings.data, account, hash)).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// El texto vigente de un archivo y su versión.
pub(crate) async fn read_text(state: &AppState, account: &str, ruta: &str) -> Option<(String, u64)> {
    let e = state.sync.current(account, ruta).filter(|e| !e.borrado)?;
    Some((blob_text(state, account, &e.hash)?, e.seq))
}

/// Cambia un archivo de texto: `f` recibe el texto vigente (None si no existe) y devuelve el
/// nuevo (None = no cambiarlo). Todo con el turno de la cuenta tomado, para que dos cambios
/// seguidos no se pisen. Devuelve la versión nueva (None si no cambió).
pub(crate) async fn modify(state: &AppState, account: &str, ruta: &str, f: impl FnOnce(Option<&str>) -> Option<String>) -> Result<Option<u64>, String> {
    if !valid_path(ruta) {
        return Err("Ruta inválida.".into());
    }
    let _turn = state.sync.turn(account).await;
    let cur = state.sync.current(account, ruta).filter(|e| !e.borrado);
    let old = match &cur {
        Some(e) => Some(read_blob(&blob_path(&state.settings.data, account, &e.hash)).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| e.to_string())?),
        None => None,
    };
    let Some(new) = f(old.as_deref()) else { return Ok(None) };
    if old.as_deref() == Some(new.as_str()) {
        return Ok(None);
    }
    if new.len() > MAX_FILE {
        return Err("El archivo es demasiado grande.".into());
    }
    match store(state, account, ruta, new.as_bytes()).await {
        Ok(r) => Ok(r.map(|(seq, _)| seq)),
        Err(_) => Err("No se pudo guardar el cambio.".into()),
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

    /// Lo que había en diario.jsonl (antes de SQLite) pasa a la base una sola vez, con sus números,
    /// y la sincronización sigue desde ahí.
    #[tokio::test]
    async fn moves_the_old_journal_to_sqlite() {
        let data = std::env::temp_dir().join(format!("nodex-ia-sqlite-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let d = dir(&data, "c1");
        std::fs::create_dir_all(&d).unwrap();
        let lines = [
            Entry { ruta: "Obra/Muro.md".into(), seq: 1, hash: "a".repeat(64), tam: 10, borrado: false },
            Entry { ruta: "tareas.txt".into(), seq: 2, hash: "b".repeat(64), tam: 20, borrado: false },
            Entry { ruta: "Obra/Muro.md".into(), seq: 3, hash: "c".repeat(64), tam: 30, borrado: false },
            Entry { ruta: "tareas.txt".into(), seq: 4, hash: String::new(), tam: 0, borrado: true },
        ];
        let text: String = lines.iter().map(|e| serde_json::to_string(e).unwrap() + "
").collect();
        std::fs::write(d.join("diario.jsonl"), text).unwrap();
        let hub = Hub::open(&data);
        assert!(!d.join("diario.jsonl").exists() && d.join("diario.jsonl.migrado").exists());
        assert_eq!(hub.seq("c1"), 4);
        assert_eq!(hub.used("c1"), 60, "todo lo guardado cuenta");
        let all = hub.since("c1", 0);
        assert_eq!(all.len(), 1, "tareas.txt se borró");
        assert_eq!((all[0].ruta.as_str(), all[0].seq, all[0].tam), ("Obra/Muro.md", 3, 30));
        assert_eq!(hub.since("c1", 3).iter().map(|e| (e.ruta.as_str(), e.borrado)).collect::<Vec<_>>(), vec![("tareas.txt", true)]);
        // Lo nuevo sigue la numeración.
        let seq = hub.record("c1", Entry { ruta: "Obra/Losa.md".into(), seq: 0, hash: "d".repeat(64), tam: 5, borrado: false }).await.unwrap();
        assert_eq!(seq, 5);
        assert_eq!(hub.used("c1"), 65);
        // Volver a abrir no repite nada; la copia de respaldo se puede abrir.
        drop(hub);
        let hub = Hub::open(&data);
        assert_eq!((hub.seq("c1"), hub.since("c1", 0).len()), (5, 2));
        hub.backup(&data.join("respaldo").join(DB_FILE)).unwrap();
        let copy = Connection::open(data.join("respaldo").join(DB_FILE)).unwrap();
        let n: i64 = copy.query_row("SELECT count(*) FROM cambios WHERE cuenta = 'c1'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 5);
        assert_eq!(hub.totals(), (1, 2, 65));
        drop(hub);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn paths() {
        assert!(valid_path("Obra Talca/Muro.md") && valid_path(".nodex/dudas.json"));
        assert!(!valid_path("../x") && !valid_path("/etc/passwd") && !valid_path("a//b") && !valid_path("C:/x") && !valid_path("a\\b"));
    }
}
