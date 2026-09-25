//! Rutas de carpetas y archivos, y direcciones web, dentro de las notas.
//!
//! Una ruta escrita a mano se busca tal cual y, si no existe, carpeta por carpeta,
//! tolerando mayúsculas y errores de tipeo: "/workspace/proeyctos/activos" escrito en
//! una nota encuentra `Dropbox/Workspace/proyectos/activos`. Las rutas que empiezan con "/"
//! se buscan primero dentro de Dropbox.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Url(String),
    Path(PathBuf),
}

/// Carpetas donde buscar una ruta que empieza con "/": Dropbox y la carpeta personal.
pub fn bases(notes_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(d) = notes_root.ancestors().find(|a| a.file_name().is_some_and(|n| n.eq_ignore_ascii_case("dropbox"))) {
        out.push(d.to_path_buf());
    }
    if let Some(home) = dirs::home_dir() {
        out.push(home.join("Dropbox"));
        out.push(home);
    }
    out.dedup();
    out.retain(|p| p.is_dir());
    out
}

fn is_special(word: &str) -> bool {
    word.starts_with('#') || word.starts_with("due:") || word.starts_with('^')
}

fn trim_punct(s: &str) -> &str {
    s.trim_end_matches(['.', ',', ';', ':', ')', ']', '"', '\'', '»'])
}

fn looks_like_path(word: &str) -> bool {
    let b = word.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    let rooted = b.len() >= 2 && (b[0] == b'/' || b[0] == b'\\') && (b[1] as char).is_alphanumeric();
    drive || rooted || word.starts_with("~/") || word.starts_with("~\\")
}

/// Enlaces de una línea: (inicio, fin) en bytes y destino. Solo las rutas que existen.
pub fn find(line: &str, bases: &[PathBuf]) -> Vec<(usize, usize, Target)> {
    let words = words(line);
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let (a, w) = words[i];
        if w.starts_with("http://") || w.starts_with("https://") || w.starts_with("www.") {
            let url = trim_punct(w);
            let full = if url.starts_with("www.") { format!("https://{url}") } else { url.to_string() };
            out.push((a, a + url.len(), Target::Url(full)));
            i += 1;
            continue;
        }
        if looks_like_path(w) {
            // Las carpetas pueden tener espacios: se prueba desde la más larga.
            let mut last = i;
            while last + 1 < words.len() && !is_special(words[last + 1].1) {
                last += 1;
            }
            let mut found = None;
            for j in (i..=last).rev() {
                let end = words[j].0 + words[j].1.len();
                let cand = trim_punct(&line[a..end]);
                if let Some(p) = resolve(cand, bases) {
                    found = Some((a + cand.len(), p, j));
                    break;
                }
            }
            if let Some((end, p, j)) = found {
                out.push((a, end, Target::Path(p)));
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn words(s: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in s.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(a)) => {
                out.push((a, &s[a..i]));
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    if let Some(a) = start {
        out.push((a, &s[a..]));
    }
    out
}

/// Encuentra la ruta en disco, tolerando mayúsculas y errores de tipeo en cada carpeta.
pub fn resolve(s: &str, bases: &[PathBuf]) -> Option<PathBuf> {
    let direct = Path::new(s);
    if direct.is_absolute() && direct.exists() {
        return Some(direct.to_path_buf());
    }
    let parts: Vec<&str> = s.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' {
        let root = PathBuf::from(format!("{}\\", &s[..2]));
        return walk(&root, &parts[1..]);
    }
    if s.starts_with('~') {
        return walk(&dirs::home_dir()?, &parts[1..]);
    }
    let mut roots: Vec<PathBuf> = bases.to_vec();
    if cfg!(not(windows)) {
        roots.push(PathBuf::from("/"));
    }
    roots.iter().find_map(|r| walk(r, &parts))
}

fn walk(root: &Path, parts: &[&str]) -> Option<PathBuf> {
    if parts.is_empty() {
        return None;
    }
    let mut cur = root.to_path_buf();
    for part in parts {
        let exact = cur.join(part);
        if exact.exists() {
            cur = exact;
            continue;
        }
        let want = part.to_lowercase();
        let mut best: Option<(usize, PathBuf)> = None;
        for e in std::fs::read_dir(&cur).ok()?.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let d = if name == want { 0 } else { distance(&want, &name) };
            if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                best = Some((d, e.path()));
            }
        }
        let allowed = (want.chars().count() / 4).max(1);
        match best {
            Some((d, p)) if d <= allowed => cur = p,
            _ => return None,
        }
    }
    Some(cur)
}

/// Distancia de edición con transposiciones ("proeyctos" -> "proyectos" = 1).
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > 3 {
        return usize::MAX;
    }
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=b.len() {
        d[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

pub fn open(target: &Target) {
    match target {
        Target::Url(u) => crate::gcal::open_browser(u),
        Target::Path(p) => {
            let program = if cfg!(windows) {
                "explorer"
            } else if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            let _ = std::process::Command::new(program).arg(p).spawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_paths_with_typos_and_spaces() {
        let root = std::env::temp_dir().join(format!("nodex-links-{}", std::process::id()));
        let target = root.join("Workspace").join("proyectos").join("activos").join("Consorcio").join("04 Trincheras");
        std::fs::create_dir_all(&target).unwrap();
        let line = "Las trincheras están en la carpeta dropbox /workspace/proeyctos/activos/consorcio/04 Trincheras #trincheras";
        let found = find(line, &[root.clone()]);
        assert_eq!(found.len(), 1, "{found:?}");
        let (a, b, t) = &found[0];
        assert_eq!(&line[*a..*b], "/workspace/proeyctos/activos/consorcio/04 Trincheras");
        let Target::Path(p) = t else { panic!("{t:?}") };
        assert_eq!(p.to_string_lossy().to_lowercase(), target.to_string_lossy().to_lowercase());
        // Una ruta que no existe no es enlace; una web sí.
        assert!(find("ver /no/existe/nada", &[root.clone()]).is_empty());
        let web = find("ver https://opencode.ai/zen. y listo", &[]);
        assert_eq!(web[0].2, Target::Url("https://opencode.ai/zen".into()));
        assert_eq!(distance("proeyctos", "proyectos"), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
