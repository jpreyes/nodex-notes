//! Historial de versiones de cada nota.
//!
//! Cada vez que una nota cambia (la escribiste, la IA movió algo, llegó de otro equipo), se
//! guarda cómo estaba antes, en `.nodex/historial/<Espacio>/<Nota>/AAAA-MM-DD HH.MM.SS huella.md`.
//! Así viaja con la carpeta a los otros equipos y se puede leer con cualquier editor.
//!
//! - Como mucho una versión cada 10 minutos mientras se escribe, pero siempre antes de un
//!   cambio grande (se fueron varias líneas de una vez).
//! - Una versión con el mismo texto que otra no se repite (la huella va en el nombre).
//! - Se ralean con el tiempo: todas las de los últimos 2 días, una por día hasta los 30 días y
//!   una por mes de ahí en adelante.

use chrono::{DateTime, Datelike, Local, NaiveDateTime, TimeZone};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Dentro de `.nodex/`.
const DIR: &str = "historial";
const NAME_FORMAT: &str = "%Y-%m-%d %H.%M.%S";
/// Mientras se escribe, como mucho una versión cada tanto.
pub const EVERY: Duration = Duration::from_secs(10 * 60);
/// Un cambio grande guarda versión aunque haya una reciente (pero no más de una cada 30 s).
const BIG_CHANGE_GAP: Duration = Duration::from_secs(30);

/// Una versión guardada de una nota.
#[derive(Debug, Clone, PartialEq)]
pub struct Version {
    pub file: PathBuf,
    pub when: DateTime<Local>,
    pub hash: String,
}

/// La carpeta del historial de una nota («Obra Talca/Muro sur», sin «.md»), o de un espacio.
pub fn dir_of(root: &Path, rel: &str) -> PathBuf {
    let mut d = root.join(".nodex").join(DIR);
    for part in rel.split('/').filter(|p| !p.is_empty()) {
        d.push(part);
    }
    d
}

/// La nota «Obra Talca/Muro sur» de una ruta (sin «.md»).
pub fn rel_of(root: &Path, note: &Path) -> String {
    note.strip_prefix(root).unwrap_or(note).with_extension("").to_string_lossy().replace('\\', "/")
}

fn hash(text: &str) -> String {
    format!("{:08x}", crate::ai::fnv(text) as u32)
}

fn parse_name(file: &Path) -> Option<(DateTime<Local>, String)> {
    let stem = file.file_stem()?.to_string_lossy().into_owned();
    let (when, hash) = stem.rsplit_once(' ')?;
    let naive = NaiveDateTime::parse_from_str(when, NAME_FORMAT).ok()?;
    let when = Local.from_local_datetime(&naive).earliest()?;
    Some((when, hash.to_string()))
}

/// Las versiones de una nota, la más reciente primero (sin repetir textos iguales: de dos con la
/// misma huella queda la más antigua, que es desde cuándo estaba así).
pub fn list(root: &Path, rel: &str) -> Vec<Version> {
    let mut v: Vec<Version> = fs::read_dir(dir_of(root, rel))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .filter_map(|file| {
            let (when, hash) = parse_name(&file)?;
            Some(Version { file, when, hash })
        })
        .collect();
    v.sort_by_key(|x| x.when);
    let mut seen = HashSet::new();
    v.retain(|x| seen.insert(x.hash.clone()));
    v.reverse();
    v
}

pub fn read(v: &Version) -> String {
    crate::vault::read_text(&v.file).unwrap_or_default()
}

/// Cuándo se guardó la última versión de una nota.
pub fn last_kept(root: &Path, rel: &str) -> Option<DateTime<Local>> {
    list(root, rel).first().map(|v| v.when)
}

/// ¿Hay que guardar `old` antes de que la nota pase a `new`? `since` = cuánto pasó desde la
/// última versión guardada (`None` si no hay ninguna).
pub fn should_keep(old: &str, new: &str, since: Option<Duration>) -> bool {
    if old.trim().is_empty() || old == new {
        return false;
    }
    let Some(since) = since else { return true };
    if since >= EVERY {
        return true;
    }
    // Un cambio grande: se fueron varias líneas de una vez.
    let before: Vec<&str> = old.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    let after: HashSet<&str> = new.lines().map(str::trim_end).collect();
    let lost = before.iter().filter(|l| !after.contains(*l)).count();
    since >= BIG_CHANGE_GAP && lost >= 2 && lost * 10 >= before.len() * 3
}

/// Guarda `text` como versión de la nota (si ya hay una con el mismo texto, no). Devuelve si
/// guardó.
pub fn keep(root: &Path, rel: &str, text: &str, now: DateTime<Local>) -> io::Result<bool> {
    if text.trim().is_empty() {
        return Ok(false);
    }
    let h = hash(text);
    let dir = dir_of(root, rel);
    if list(root, rel).iter().any(|v| v.hash == h) {
        return Ok(false);
    }
    fs::create_dir_all(&dir)?;
    fs::write(dir.join(format!("{} {h}.md", now.format(NAME_FORMAT))), text)?;
    prune(root, rel, now);
    Ok(true)
}

/// Qué versiones se quedan: todas las de los últimos 2 días, la última de cada día hasta los 30
/// días y la última de cada mes de ahí en adelante.
pub fn to_prune(versions: &[Version], now: DateTime<Local>) -> Vec<PathBuf> {
    let mut kept_slots = HashSet::new();
    let mut out = Vec::new();
    // De la más reciente a la más antigua: la primera de cada «casilla» es la última de ese tramo.
    let mut sorted: Vec<&Version> = versions.iter().collect();
    sorted.sort_by(|a, b| b.when.cmp(&a.when));
    for v in sorted {
        let age = now.signed_duration_since(v.when);
        let slot = if age < chrono::Duration::days(2) {
            continue;
        } else if age < chrono::Duration::days(30) {
            format!("d{}", v.when.date_naive())
        } else {
            format!("m{}-{}", v.when.year(), v.when.month())
        };
        if !kept_slots.insert(slot) {
            out.push(v.file.clone());
        }
    }
    out
}

fn prune(root: &Path, rel: &str, now: DateTime<Local>) {
    // Se miran todas (también las repetidas: se borran igual).
    let all: Vec<Version> = fs::read_dir(dir_of(root, rel))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter_map(|file| {
            let (when, hash) = parse_name(&file)?;
            Some(Version { file, when, hash })
        })
        .collect();
    for f in to_prune(&all, now) {
        let _ = fs::remove_file(f);
    }
}

/// Una nota (o un espacio) se movió o cambió de nombre: su historial la sigue.
pub fn relink(root: &Path, old: &str, new: &str) {
    let (from, to) = (dir_of(root, old), dir_of(root, new));
    if from == to || !from.is_dir() {
        return;
    }
    if let Some(parent) = to.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if !to.exists() && fs::rename(&from, &to).is_ok() {
        return;
    }
    // Ya había historial con ese nombre: se juntan (archivo por archivo, carpetas incluidas).
    fn merge(from: &Path, to: &Path) {
        let _ = fs::create_dir_all(to);
        for e in fs::read_dir(from).into_iter().flatten().flatten() {
            let target = to.join(e.file_name());
            if e.path().is_dir() {
                merge(&e.path(), &target);
            } else if !target.exists() {
                let _ = fs::rename(e.path(), &target);
            }
        }
        let _ = fs::remove_dir_all(from);
    }
    merge(&from, &to);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nodex-historial-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn when_to_keep_a_version() {
        let old = "Uno\nDos\nTres\nCuatro\n";
        assert!(should_keep(old, "Uno\nDos\n", None), "la primera vez, siempre");
        assert!(!should_keep(old, old, None));
        assert!(!should_keep("", "algo", None));
        // Escribiendo: una cada 10 minutos.
        assert!(!should_keep(old, "Uno\nDos\nTres\nCuatro y más\n", Some(Duration::from_secs(60))));
        assert!(should_keep(old, "Uno\nDos\nTres\nCuatro y más\n", Some(EVERY)));
        // Se fueron dos líneas de cuatro: cambio grande.
        assert!(should_keep(old, "Uno\nDos\n", Some(Duration::from_secs(60))));
        assert!(!should_keep(old, "Uno\nDos\n", Some(Duration::from_secs(5))), "no más de una cada 30 s");
    }

    #[test]
    fn keeps_lists_and_follows_the_note() {
        let root = temp("guardar");
        let t0 = Local.with_ymd_and_hms(2026, 10, 1, 10, 0, 0).unwrap();
        assert!(keep(&root, "Obra/Muro", "versión 1\n", t0).unwrap());
        assert!(keep(&root, "Obra/Muro", "versión 2\n", t0 + chrono::Duration::minutes(15)).unwrap());
        assert!(!keep(&root, "Obra/Muro", "versión 1\n", t0 + chrono::Duration::minutes(30)).unwrap(), "el mismo texto no se repite");
        let v = list(&root, "Obra/Muro");
        assert_eq!(v.len(), 2);
        assert_eq!(read(&v[0]), "versión 2\n");
        assert_eq!(v[1].when, t0);
        // Se movió a otro espacio, y después cambió de nombre el espacio.
        relink(&root, "Obra/Muro", "Talca/Muro");
        assert_eq!(list(&root, "Talca/Muro").len(), 2);
        assert!(list(&root, "Obra/Muro").is_empty());
        relink(&root, "Talca", "Obra Talca");
        assert_eq!(list(&root, "Obra Talca/Muro").len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn old_versions_are_thinned_out() {
        let now = Local.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap();
        let v = |days: i64, hours: i64, name: &str| Version { file: PathBuf::from(name), when: now - chrono::Duration::days(days) - chrono::Duration::hours(hours), hash: name.into() };
        let versions = vec![
            v(0, 1, "hoy-a"),
            v(0, 2, "hoy-b"),
            v(5, 1, "hace5-tarde"),
            v(5, 3, "hace5-temprano"),
            v(60, 0, "hace60-a"),
            v(61, 0, "hace61-b"),
        ];
        let mut gone = to_prune(&versions, now);
        gone.sort();
        assert_eq!(gone, vec![PathBuf::from("hace5-temprano"), PathBuf::from("hace61-b")]);
    }
}
