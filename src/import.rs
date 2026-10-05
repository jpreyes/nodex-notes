//! Importar notas Markdown de otra app (Obsidian, Notion exportado, Logseq, Bear…) o de
//! cualquier carpeta de `.md`.
//!
//! - Cada carpeta de primer nivel pasa a ser un espacio; las notas sueltas de la raíz van a un
//!   espacio con el nombre de la carpeta importada. Las subcarpetas más hondas se aplanan.
//! - Notion agrega un código a cada nombre («Reunión 1a2b…f.md»): se quita.
//! - Lo de arriba entre `---` (Obsidian) se quita; sus `tags` pasan a ser #etiquetas al final.
//! - Las imágenes y archivos se copian a `Adjuntos/` y sus enlaces se corrigen; los enlaces a
//!   otras páginas (`[x](Otra%20página.md)`, `![[Otra]]`) quedan como `[[Otra]]`.
//! - Nada se pisa: si ya hay una nota con ese nombre, se agrega «2»; si es igual, se salta.

use crate::lines;
use crate::vault;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Carpetas que no son notas (de Obsidian, git, papeleras…).
const SKIP_DIRS: [&str; 6] = [".obsidian", ".trash", ".git", "node_modules", ".nodex", ".papelera"];
/// Un adjunto más grande que esto no se copia (se avisa).
const MAX_FILE: u64 = 100 * 1024 * 1024;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Report {
    pub notes: usize,
    pub files: usize,
    /// Iguales a una que ya estaba.
    pub skipped: usize,
    pub spaces: Vec<String>,
    /// Las notas creadas (para no mandarlas a la IA si no se pidió).
    pub created: Vec<PathBuf>,
    pub errors: Vec<String>,
}

/// «Reunión de obra 1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d» -> «Reunión de obra» (el código de Notion).
pub fn clean_name(name: &str) -> String {
    let n = name.trim();
    let n = match n.rsplit_once(' ') {
        Some((base, code)) if code.len() == 32 && code.chars().all(|c| c.is_ascii_hexdigit()) => base,
        _ => n,
    };
    vault::sanitize(n)
}

/// El nombre de un espacio que no choca con las carpetas reservadas de la app.
fn space_name(name: &str) -> String {
    let n = clean_name(name);
    if vault::is_reserved_dir(&n) { format!("{n} (importado)") } else { n }
}

/// Todos los archivos de la carpeta, sin las carpetas ocultas o de otras apps.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            if !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str()) {
                walk(&p, out);
            }
        } else if t.is_file() && !name.starts_with('.') {
            out.push(p);
        }
    }
}

/// Cuántas notas y cuántos otros archivos hay para importar.
pub fn count(src: &Path) -> (usize, usize) {
    let mut all = Vec::new();
    walk(src, &mut all);
    let notes = all.iter().filter(|p| is_md(p)).count();
    (notes, all.len() - notes)
}

fn is_md(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
}

/// Quita lo de arriba entre `---` y devuelve sus etiquetas.
fn split_front_matter(text: &str) -> (String, Vec<String>) {
    let Some(rest) = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n")) else { return (text.to_string(), Vec::new()) };
    let Some(end) = rest.find("\n---") else { return (text.to_string(), Vec::new()) };
    let head = &rest[..end];
    let body = rest[end + 4..].trim_start_matches(['\r', '\n']).to_string();
    let mut tags = Vec::new();
    let mut in_list = false;
    for l in head.lines() {
        let t = l.trim();
        if let Some(v) = t.strip_prefix("tags:").or_else(|| t.strip_prefix("Tags:")) {
            let v = v.trim().trim_start_matches('[').trim_end_matches(']');
            tags.extend(v.split(',').map(|x| x.trim().trim_matches(['"', '\'', '#']).to_string()).filter(|x| !x.is_empty()));
            in_list = v.is_empty();
        } else if in_list && t.starts_with("- ") {
            tags.push(t[2..].trim().trim_matches(['"', '\'', '#']).to_string());
        } else {
            in_list = false;
        }
    }
    let tags = tags.into_iter().map(|t| t.replace(' ', "-")).filter(|t| !t.is_empty()).collect();
    (body, tags)
}

/// Corrige los enlaces de una nota: adjuntos a `../Adjuntos/…`, otras páginas a `[[…]]`.
/// `dir`: la carpeta de la nota original; `files`: adjunto original -> nombre en Adjuntos;
/// `by_name`: nombre de archivo (en minúsculas) -> nombre en Adjuntos.
fn fix_links(text: &str, dir: &Path, files: &HashMap<PathBuf, String>, by_name: &HashMap<String, String>) -> String {
    let att = |name: &str| format!("../{}/{}", vault::ATTACHMENTS, lines::encode_path(name));
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    // Obsidian: ![[archivo.png]] / ![[Otra nota]] / [[Otra nota]] (este último se deja igual).
    while let Some(i) = rest.find("![[") {
        out += &rest[..i];
        let after = &rest[i + 3..];
        let Some(j) = after.find("]]") else {
            out += &rest[i..];
            rest = "";
            break;
        };
        let inner = &after[..j];
        let target = inner.split('|').next().unwrap_or(inner).trim();
        match by_name.get(&target.to_lowercase()) {
            Some(n) => out += &format!("![{}]({})", Path::new(n).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), att(n)),
            None => out += &format!("[[{}]]", clean_name(target.trim_end_matches(".md"))),
        }
        rest = &after[j + 2..];
    }
    out += rest;
    // Markdown: [texto](ruta) y ![texto](ruta).
    let text = out;
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(i) = rest.find("](") {
        let Some(j) = rest[i + 2..].find(')') else { break };
        let open = rest[..i].rfind('[');
        let target = &rest[i + 2..i + 2 + j];
        let plain = target.split_whitespace().next().unwrap_or("").trim_matches(['<', '>']);
        let is_web = plain.contains("://") || plain.starts_with("mailto:") || plain.starts_with('#');
        let decoded = lines::decode_path(plain);
        let path = dir.join(&decoded);
        let new = if is_web || plain.is_empty() {
            None
        } else if let Some(n) = files.get(&path).or_else(|| by_name.get(&Path::new(&decoded).file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default())) {
            Some(format!("]({})", att(n)))
        } else if is_md(Path::new(&decoded)) {
            // Un enlace a otra página: [[Página|texto]].
            let page = clean_name(&Path::new(&decoded).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
            if let Some(o) = open {
                let label = &rest[o + 1..i];
                out += &rest[..o];
                out += &if label.trim().is_empty() || clean_name(label) == page { format!("[[{page}]]") } else { format!("[[{page}|{label}]]") };
                rest = &rest[i + 2 + j + 1..];
                continue;
            }
            None
        } else {
            None
        };
        match new {
            Some(n) => {
                out += &rest[..i];
                out += &n;
            }
            None => out += &rest[..i + 2 + j + 1],
        }
        rest = &rest[i + 2 + j + 1..];
    }
    out += rest;
    out
}

/// Dónde va `stem` en `dir` («Nombre.md», «Nombre 2.md»…): si ya hay uno igual (`same`), ese
/// (y `true`); si no, el primer nombre libre.
fn place(dir: &Path, stem: &str, ext: &str, same: impl Fn(&Path) -> bool) -> (PathBuf, bool) {
    let mut p = dir.join(format!("{stem}{ext}"));
    let mut i = 2;
    while p.exists() {
        if same(&p) {
            return (p, true);
        }
        p = dir.join(format!("{stem} {i}{ext}"));
        i += 1;
    }
    (p, false)
}

/// Importa la carpeta `src` a la carpeta de notas `root`. `progress(hechos, total)`.
pub fn run(src: &Path, root: &Path, mut progress: impl FnMut(usize, usize)) -> Report {
    let mut report = Report::default();
    let mut all = Vec::new();
    walk(src, &mut all);
    let (mds, others): (Vec<PathBuf>, Vec<PathBuf>) = all.into_iter().partition(|p| is_md(p));
    let total = mds.len() + others.len();
    let base = space_name(&src.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "Importadas".into()));
    let mut done = 0;

    // Adjuntos primero, para corregir los enlaces.
    let att_dir = root.join(vault::ATTACHMENTS);
    let mut files: HashMap<PathBuf, String> = HashMap::new();
    let mut by_name: HashMap<String, String> = HashMap::new();
    for p in &others {
        done += 1;
        progress(done, total);
        let size = fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        if size > MAX_FILE {
            report.errors.push(format!("{} es muy grande ({} MB): no se copió", p.display(), size / 1024 / 1024));
            continue;
        }
        let name = p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let stem = Path::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = Path::new(&name).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let bytes = fs::read(p).unwrap_or_default();
        let (dest, existed) = place(&att_dir, &vault::sanitize(&stem), &ext, |q| fs::read(q).is_ok_and(|b| b == bytes));
        let copied = if existed { Ok(0) } else { fs::create_dir_all(&att_dir).and_then(|_| fs::copy(p, &dest)) };
        match copied {
            Ok(_) => {
                let n = dest.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                files.insert(p.clone(), n.clone());
                by_name.entry(name.to_lowercase()).or_insert(n);
                if !existed {
                    report.files += 1;
                }
            }
            Err(e) => report.errors.push(format!("{}: {e}", p.display())),
        }
    }

    for p in &mds {
        done += 1;
        progress(done, total);
        let rel = p.strip_prefix(src).unwrap_or(p);
        let space = match rel.components().count() {
            0 | 1 => base.clone(),
            _ => space_name(&rel.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default()),
        };
        let Ok(bytes) = fs::read(p) else {
            report.errors.push(format!("{}: no se pudo leer", p.display()));
            continue;
        };
        let raw = String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').replace("\r\n", "\n");
        let (body, tags) = split_front_matter(&raw);
        let mut text = fix_links(&body, p.parent().unwrap_or(src), &files, &by_name);
        if !tags.is_empty() {
            let line = tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ");
            text = format!("{}\n\n{line}\n", text.trim_end());
        }
        let title = clean_name(&p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        let dir = root.join(&space);
        // La misma nota ya importada: no se repite.
        let (dest, existed) = place(&dir, &title, ".md", |q| fs::read_to_string(q).is_ok_and(|t| t.trim() == text.trim()));
        if existed {
            report.skipped += 1;
            continue;
        }
        match fs::create_dir_all(&dir).and_then(|_| fs::write(&dest, &text)) {
            Ok(()) => {
                report.notes += 1;
                report.created.push(dest);
                if !report.spaces.contains(&space) {
                    report.spaces.push(space);
                }
            }
            Err(e) => report.errors.push(format!("{}: {e}", p.display())),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_obsidian_and_notion_folders() {
        let base = std::env::temp_dir().join(format!("nodex-importar-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let src = base.join("Mi bóveda");
        let root = base.join("Notas");
        fs::create_dir_all(src.join("Obra").join("Fotos")).unwrap();
        fs::create_dir_all(src.join("Proyectos 0123456789abcdef0123456789abcdef")).unwrap();
        fs::create_dir_all(src.join(".obsidian")).unwrap();
        fs::create_dir_all(root.join("Obra")).unwrap();
        fs::write(src.join(".obsidian").join("app.json"), "{}").unwrap();
        fs::write(src.join("Obra").join("Fotos").join("muro 1.jpg"), "JPG").unwrap();
        fs::write(src.join("Obra").join("Acta.pdf"), "PDF").unwrap();
        // Obsidian: propiedades arriba, una imagen incrustada y un enlace a otra nota.
        fs::write(
            src.join("Obra").join("Visita.md"),
            "---\ntags: [obra, inspección técnica]\nfecha: 2026-10-01\n---\n# Visita\nVer ![[muro 1.jpg]] y [[Acta de entrega]].\n[El acta](Acta.pdf)\n",
        )
        .unwrap();
        // Ya había una «Visita» distinta en Obra: la importada queda como «Visita 2».
        fs::write(root.join("Obra").join("Visita.md"), "Otra cosa\n").unwrap();
        // Notion: códigos en los nombres y enlaces a otras páginas por su archivo.
        fs::write(
            src.join("Proyectos 0123456789abcdef0123456789abcdef").join("Puente fedcba9876543210fedcba9876543210.md"),
            "# Puente\nVer [la visita](../Obra/Visita.md) y [Google](https://google.com).\n",
        )
        .unwrap();
        fs::write(src.join("Suelta.md"), "Una nota en la raíz\n").unwrap();
        fs::create_dir_all(src.join("Diario")).unwrap();
        fs::write(src.join("Diario").join("2026-10-01.md"), "Del diario de Obsidian\n").unwrap();

        let r = run(&src, &root, |_, _| {});
        assert_eq!((r.notes, r.files, r.skipped), (4, 2, 0), "{r:?}");
        let visita = fs::read_to_string(root.join("Obra").join("Visita 2.md")).unwrap();
        assert_eq!(
            visita,
            "# Visita\nVer ![muro 1](../Adjuntos/muro%201.jpg) y [[Acta de entrega]].\n[El acta](../Adjuntos/Acta.pdf)\n\n#obra #inspección-técnica\n"
        );
        assert_eq!(fs::read_to_string(root.join("Obra").join("Visita.md")).unwrap(), "Otra cosa\n", "no se pisa");
        let puente = fs::read_to_string(root.join("Proyectos").join("Puente.md")).unwrap();
        assert_eq!(puente, "# Puente\nVer [[Visita|la visita]] y [Google](https://google.com).\n");
        assert!(root.join("Mi bóveda").join("Suelta.md").exists(), "lo de la raíz, a un espacio con el nombre de la carpeta");
        assert!(root.join("Diario (importado)").join("2026-10-01.md").exists(), "Diario es de la app: otro nombre");
        assert!(root.join("Adjuntos").join("muro 1.jpg").exists() && !root.join("Adjuntos").join("app.json").exists());

        // Importar otra vez la misma carpeta no repite nada.
        let again = run(&src, &root, |_, _| {});
        assert_eq!((again.notes, again.files, again.skipped), (0, 0, 4), "{again:?}");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn notion_codes_are_removed() {
        assert_eq!(clean_name("Reunión de obra 1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d"), "Reunión de obra");
        assert_eq!(clean_name("Informe 2026"), "Informe 2026");
    }
}
