//! Marcas para que dos equipos con la app abierta sobre la misma carpeta no hagan lo mismo a la vez.
//!
//! - Notas: antes de mandar una nota a la IA, el equipo deja una marca en
//!   `.nodex/organizando/<huella de la nota>.txt` (su identificador y hasta cuándo vale). Si otro
//!   equipo ya la tiene, espera. Al aplicar el resultado se vuelve a mirar: si dos la tomaron a
//!   la vez (Dropbox deja una copia en conflicto de la marca), gana el de identificador menor.
//! - Correos: `.nodex/correos-anotados.txt` lleva qué equipo tomó cada correo ("clave<TAB>equipo");
//!   si lo tomaron dos, también gana el de identificador menor.
//!
//! Es un mejor esfuerzo: las marcas viajan por Dropbox y tardan unos segundos en llegar.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const NOTES_DIR: &str = "organizando";
const MAILS_FILE: &str = "correos-anotados.txt";
/// Cuántos correos recuerda la lista compartida.
const MAX_MAILS: usize = 5000;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Identificador de este equipo (se crea la primera vez, junto a la configuración).
pub fn machine_id() -> String {
    let path = crate::config::config_path().with_file_name("equipo.txt");
    if let Some(id) = fs::read_to_string(&path).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        return id;
    }
    let mut b = [0u8; 8];
    let _ = getrandom::fill(&mut b);
    let id: String = b.iter().map(|x| char::from(b"0123456789abcdefghijklmnopqrstuvwxyz"[(*x % 36) as usize])).collect();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(&path, &id);
    id
}

fn note_stem(rel: &str) -> String {
    format!("{:016x}", crate::ai::fnv(rel))
}

/// Las marcas vigentes de una nota: (archivo, equipo). Incluye las copias en conflicto de la
/// marca. De paso borra las vencidas.
fn holders(root: &Path, rel: &str) -> Vec<(PathBuf, String)> {
    let stem = note_stem(rel);
    let mut out = Vec::new();
    for e in fs::read_dir(root.join(".nodex").join(NOTES_DIR)).into_iter().flatten().flatten() {
        if !e.file_name().to_string_lossy().starts_with(&stem) {
            continue;
        }
        let text = fs::read_to_string(e.path()).unwrap_or_default();
        let mut lines = text.lines();
        let (who, until) = (lines.next().unwrap_or("").trim().to_string(), lines.next().and_then(|l| l.trim().parse::<u64>().ok()).unwrap_or(0));
        if until > now() && !who.is_empty() {
            out.push((e.path(), who));
        } else {
            let _ = fs::remove_file(e.path());
        }
    }
    out
}

/// Toma una nota para organizarla durante `ttl` segundos. `false` si otro equipo la tiene.
pub fn take_note(root: &Path, rel: &str, me: &str, ttl: u64) -> bool {
    if holders(root, rel).iter().any(|(_, who)| who != me) {
        return false;
    }
    let dir = root.join(".nodex").join(NOTES_DIR);
    if fs::create_dir_all(&dir).is_err() {
        return true; // sin marcas no se bloquea el trabajo
    }
    let _ = fs::write(dir.join(format!("{}.txt", note_stem(rel))), format!("{me}\n{}\n{rel}\n", now() + ttl));
    true
}

/// Al aplicar: ¿la nota sigue siendo de este equipo? Si otro también la tomó, gana el de
/// identificador menor.
pub fn note_is_mine(root: &Path, rel: &str, me: &str) -> bool {
    !holders(root, rel).iter().any(|(_, who)| who.as_str() < me)
}

/// Suelta la nota (borra las marcas de este equipo).
pub fn release_note(root: &Path, rel: &str, me: &str) {
    for (path, who) in holders(root, rel) {
        if who == me {
            let _ = fs::remove_file(path);
        }
    }
}

/// Qué equipo tomó cada correo (si lo tomaron dos, el de identificador menor).
pub fn mail_owners(root: &Path) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    let text = crate::vault::read_text(&root.join(".nodex").join(MAILS_FILE)).unwrap_or_default();
    for line in text.lines() {
        let Some((key, who)) = line.rsplit_once('\t') else { continue };
        match out.get_mut(key) {
            Some(w) if who < w.as_str() => *w = who.to_string(),
            Some(_) => {}
            None => {
                out.insert(key.to_string(), who.to_string());
            }
        }
    }
    out
}

/// Anota que este equipo tomó esos correos (los que nadie había tomado).
pub fn claim_mails(root: &Path, keys: &[String], me: &str) {
    let owners = mail_owners(root);
    let new: Vec<&String> = keys.iter().filter(|k| !owners.contains_key(*k)).collect();
    if new.is_empty() {
        return;
    }
    let dir = root.join(".nodex");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(MAILS_FILE);
    let old = crate::vault::read_text(&path).unwrap_or_default();
    let mut lines: Vec<String> = old.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect();
    lines.extend(new.iter().map(|k| format!("{}\t{me}", k.replace(['\t', '\n'], " "))));
    let from = lines.len().saturating_sub(MAX_MAILS);
    let _ = fs::write(path, lines[from..].join("\n") + "\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_device_holds_a_note() {
        let root = std::env::temp_dir().join(format!("nodex-marcas-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let note = "General/2026-09-30";
        assert!(take_note(&root, note, "pc-b", 60));
        assert!(!take_note(&root, note, "pc-a", 60), "otro equipo la tiene");
        assert!(take_note(&root, note, "pc-b", 60), "el mismo equipo puede renovarla");
        assert!(take_note(&root, "General/Otra", "pc-a", 60), "otra nota está libre");
        assert!(note_is_mine(&root, note, "pc-b"));
        // Los dos la tomaron a la vez (Dropbox dejó una copia en conflicto de la marca): gana el menor.
        let dir = root.join(".nodex").join(NOTES_DIR);
        fs::write(dir.join(format!("{} (copia en conflicto de A 2026-09-30).txt", note_stem(note))), format!("pc-a\n{}\n{note}\n", now() + 60)).unwrap();
        assert!(!note_is_mine(&root, note, "pc-b"));
        assert!(note_is_mine(&root, note, "pc-a"));
        // Al soltarla, o al vencer, queda libre.
        release_note(&root, note, "pc-a");
        release_note(&root, note, "pc-b");
        assert!(take_note(&root, note, "pc-a", 0), "una marca que vence al tiro");
        assert!(take_note(&root, note, "pc-b", 60), "la vencida no cuenta");

        // Correos: el primero que lo toma es el dueño; si lo tomaron dos, el de identificador menor.
        claim_mails(&root, &["<m1@x>".into(), "<m2@x>".into()], "pc-b");
        claim_mails(&root, &["<m2@x>".into(), "<m3@x>".into()], "pc-a");
        let owners = mail_owners(&root);
        assert_eq!((owners["<m1@x>"].as_str(), owners["<m2@x>"].as_str(), owners["<m3@x>"].as_str()), ("pc-b", "pc-b", "pc-a"));
        let path = root.join(".nodex").join(MAILS_FILE);
        fs::write(&path, fs::read_to_string(&path).unwrap() + "<m1@x>\tpc-a\n").unwrap();
        assert_eq!(mail_owners(&root)["<m1@x>"], "pc-a");
        let _ = fs::remove_dir_all(&root);
    }
}
