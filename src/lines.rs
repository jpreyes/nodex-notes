//! Estructura de una nota, línea por línea.
//!
//! - Una línea sin sangría es una nota (lleva número).
//! - Con una sangría ("  texto") es parte de la nota de arriba.
//! - Con dos o más ("  - ítem", "    - subítem") es un ítem de lista de esa nota.
//! - Un bloque "## Título" va junto hasta "## fin", el siguiente "##" o una línea en blanco.
//! - Las tareas llevan casilla ("- [ ] texto", "- [x] hecho"), fecha ("due:2026-09-26")
//!   y un identificador ("^k3f9a") que las une con su línea en tareas.txt.

use crate::tags;

/// Sangría de un nivel en el archivo.
pub const INDENT: &str = "  ";
pub const MAX_LEVEL: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LineInfo {
    /// 0 = nota; 1 = continuación de la de arriba; 2 o más = ítem de lista.
    pub level: u8,
    /// Bytes de la sangría.
    pub indent: usize,
    /// Tiene "- " (o "* ") después de la sangría.
    pub marker: bool,
    /// Casilla de tarea: `Some(false)` = "[ ]", `Some(true)` = "[x]".
    pub check: Option<bool>,
    /// Bytes de sangría + "- " + casilla: donde empieza el texto.
    pub prefix: usize,
}

pub fn parse(line: &str) -> LineInfo {
    let line = line.trim_end_matches(['\n', '\r']);
    let mut width: usize = 0;
    let mut indent = 0;
    for b in line.bytes() {
        match b {
            b' ' => width += 1,
            b'\t' => width += 2,
            _ => break,
        }
        indent += 1;
    }
    let rest = &line[indent..];
    let marker = rest.starts_with("- ") || rest.starts_with("* ");
    let mut prefix = indent + if marker { 2 } else { 0 };
    let after = &line[prefix..];
    let check = if after.starts_with("[ ] ") || after == "[ ]" {
        Some(false)
    } else if after.starts_with("[x] ") || after.starts_with("[X] ") || after == "[x]" || after == "[X]" {
        Some(true)
    } else {
        None
    };
    if check.is_some() {
        prefix += after.len().min(4);
    }
    let level = if indent == 0 {
        0
    } else if marker {
        (1 + width.div_ceil(2)).clamp(2, MAX_LEVEL as usize) as u8
    } else {
        1
    };
    LineInfo { level, indent, marker, check, prefix }
}

/// Lo que va antes del texto para un nivel y una casilla.
pub fn prefix_for(level: u8, check: Option<bool>) -> String {
    let mut s = match level {
        0 => String::new(),
        1 => INDENT.to_string(),
        n => INDENT.repeat(n as usize - 1) + "- ",
    };
    match check {
        Some(done) => {
            if level == 0 {
                s += "- ";
            }
            s += if done { "[x] " } else { "[ ] " };
        }
        None => {}
    }
    s
}

/// La misma línea con otro nivel (conserva la casilla y el texto).
pub fn with_level(line: &str, level: u8) -> String {
    let info = parse(line);
    if info.level == 0 && info.indent == 0 && info.check.is_none() {
        // "- 15:03 algo" (reunión) o texto normal: todo es texto.
        return prefix_for(level, None) + line;
    }
    prefix_for(level.min(MAX_LEVEL), info.check) + &line[info.prefix..]
}

/// Marca o desmarca la casilla de una línea de tarea.
pub fn toggle_check(line: &str) -> String {
    let info = parse(line);
    match info.check {
        Some(done) => {
            let at = info.indent + if info.marker { 2 } else { 0 };
            let mark = if done { "[ ]" } else { "[x]" };
            format!("{}{}{}", &line[..at], mark, &line[at + 3..])
        }
        None => line.to_string(),
    }
}

/// Convierte una línea en tarea ("- [ ] …"), si no lo es ya.
pub fn make_task(line: &str) -> String {
    let info = parse(line);
    if info.check.is_some() {
        return line.to_string();
    }
    if info.level == 0 {
        let text = if info.marker && info.indent == 0 { &line[2..] } else { line };
        return format!("- [ ] {text}");
    }
    format!("{}[ ] {}", &line[..info.prefix], &line[info.prefix..])
}

/// Un enlace escrito en una línea.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Link {
    /// `[[Muro]]`, `[[Obra/Muro]]` o `[[Muro|el muro]]`: la nota a la que apunta.
    Note(String),
    /// `[informe.pdf](../Adjuntos/informe.pdf)` o `[sitio](https://…)`: el destino tal cual.
    Markdown(String),
}

/// Enlaces de una línea: (inicio, fin) en bytes de todo el enlace, el rango que se ve
/// (el nombre o el texto) y a dónde apunta. Las imágenes (`![…](…)`) no cuentan.
pub fn links(line: &str) -> Vec<(usize, usize, (usize, usize), Link)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        if let Some(inner) = rest.strip_prefix("[[") {
            if let Some(j) = inner.find("]]") {
                let body = &inner[..j];
                let (target, alias) = match body.split_once('|') {
                    Some((t, a)) => (t, Some(a).filter(|a| !a.trim().is_empty())),
                    None => (body, None),
                };
                if !target.trim().is_empty() && !body.contains('[') && !body.contains(']') {
                    // Se ve el texto después de «|», o si no, el nombre.
                    let (at, s) = match alias {
                        Some(a) => (i + 2 + target.len() + 1, a),
                        None => (i + 2, target),
                    };
                    let lead = s.len() - s.trim_start().len();
                    let end = i + 2 + j + 2;
                    out.push((i, end, (at + lead, at + lead + s.trim().len()), Link::Note(target.trim().to_string())));
                    i = end;
                    continue;
                }
            }
            i += 2;
            continue;
        }
        if rest.starts_with('[') && !line[..i].ends_with('!') {
            if let Some(k) = rest.find("](") {
                let text = &rest[1..k];
                let after = &rest[k + 2..];
                if let Some(m) = after.find(')') {
                    let dest = &after[..m];
                    if !text.trim().is_empty() && !text.contains('[') && !text.contains(']') && !dest.is_empty() && !dest.contains(char::is_whitespace) {
                        let end = i + k + 2 + m + 1;
                        out.push((i, end, (i + 1, i + k), Link::Markdown(dest.to_string())));
                        i = end;
                        continue;
                    }
                }
            }
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    out
}

/// El nombre de un archivo para usarlo en un enlace: sin espacios ni paréntesis
/// ("Informe (final).pdf" -> "Informe%20%28final%29.pdf"), como lo entienden otros editores.
pub fn encode_path(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            ' ' => out += "%20",
            '(' => out += "%28",
            ')' => out += "%29",
            '[' => out += "%5B",
            ']' => out += "%5D",
            '#' => out += "%23",
            '%' => out += "%25",
            c => out.push(c),
        }
    }
    out
}

/// Al revés de `encode_path` (cualquier %XX).
pub fn decode_path(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(Ok(v)) = s.get(i + 1..i + 3).filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit())).map(|h| u8::from_str_radix(h, 16)) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Una línea que es solo una imagen: "![Captura 30 sep](../Adjuntos/captura.png)" -> (texto, ruta).
/// Después pueden venir etiquetas o metadatos (los que agrega la IA), pero no más texto.
pub fn image_of(line: &str) -> Option<(&str, &str)> {
    let t = line.trim().strip_prefix("![")?;
    let (alt, rest) = t.split_once("](")?;
    let (path, after) = rest.split_once(')')?;
    let meta_only = after.split_whitespace().all(|w| w.starts_with('#') || w.starts_with("due:") || (w.starts_with('^') && w.len() > 3));
    (meta_only && !path.trim().is_empty() && !path.contains('(')).then_some((alt, path.trim()))
}

pub fn is_heading(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("# ") || t.starts_with("### ") || t.starts_with("#### ")
}

pub fn is_block_start(line: &str) -> bool {
    let l = line.trim();
    l.starts_with("## ") && !is_block_end(l)
}

pub fn is_block_end(line: &str) -> bool {
    let l = line.trim();
    l.starts_with("##") && !l.starts_with("###") && l.trim_start_matches('#').trim_start().to_lowercase().starts_with("fin")
}

/// Línea hecha solo de etiquetas ("#reunión #vigas").
// ---------- Seguimiento ----------

/// Marca de una línea de seguimiento (lo que se hizo con la línea de arriba):
/// "  ↳ 2026-10-01: Se pidió a Gerdau, llega el lunes".
pub const FOLLOW_MARK: &str = "↳";

/// Si el texto de una línea (sin su sangría) es un seguimiento: su fecha ("" si no tiene) y
/// dónde empieza lo escrito (en bytes).
pub fn follow_up(content: &str) -> Option<(&str, usize)> {
    let rest = content.strip_prefix(FOLLOW_MARK)?;
    let rest = rest.trim_start();
    let mut at = content.len() - rest.len();
    let date = rest.get(..10).filter(|d| crate::agenda::is_date(d)).unwrap_or("");
    if !date.is_empty() {
        let after = &rest[10..];
        let after = after.strip_prefix(':').unwrap_or(after);
        at = content.len() - after.trim_start().len();
    }
    Some((date, at))
}

/// ¿Es una línea de seguimiento?
pub fn is_follow_up(line: &str) -> bool {
    follow_up(&line[parse(line).prefix..]).is_some()
}

/// Los seguimientos que tiene la línea `idx` (las líneas «↳» con más sangría justo debajo):
/// (fecha, texto).
pub fn follow_ups_after(text: &str, idx: usize) -> Vec<(String, String)> {
    let ls: Vec<&str> = text.lines().collect();
    let Some(base) = ls.get(idx).map(|l| parse(l).level) else { return Vec::new() };
    let mut out = Vec::new();
    for l in ls.iter().skip(idx + 1) {
        let info = parse(l);
        if info.level <= base {
            break;
        }
        if let Some((date, at)) = follow_up(&l[info.prefix..]) {
            out.push((date.to_string(), l[info.prefix + at..].trim().to_string()));
        }
    }
    out
}

/// `text` con un seguimiento nuevo bajo la línea `idx` (después de los que ya tenga). Devuelve el
/// texto y en qué línea quedó.
pub fn add_follow_up(text: &str, idx: usize, date: &str, what: &str) -> (String, usize) {
    let mut ls: Vec<&str> = text.split('\n').collect();
    let base = ls.get(idx).map_or(0, |l| parse(l.trim_end_matches('\r')).level);
    let mut at = idx + 1;
    while at < ls.len() {
        let l = ls[at].trim_end_matches('\r');
        if l.trim().is_empty() || parse(l).level <= base || !is_follow_up(l) {
            break;
        }
        at += 1;
    }
    let cr = if ls.get(idx).is_some_and(|l| l.ends_with('\r')) { "\r" } else { "" };
    let line = format!("{}{FOLLOW_MARK} {date}: {}{cr}", prefix_for((base + 1).min(MAX_LEVEL), None), what.trim());
    ls.insert(at.min(ls.len()), &line);
    let mut out = ls.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    (out, at)
}

// ---------- Tablas ----------

/// ¿Es una fila de una tabla? ("| Material | Cantidad |")
pub fn is_table_row(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t[1..].contains('|')
}

/// ¿Es la fila que separa el encabezado del resto? ("|---|:---:|")
pub fn is_table_rule(line: &str) -> bool {
    is_table_row(line)
        && table_cells(line).1.iter().all(|&(a, b)| {
            let c = &line[a..b];
            c.contains('-') && c.chars().all(|ch| ch == '-' || ch == ':')
        })
}

/// Una fila de tabla: dónde están sus «|» (en bytes) y el texto de cada celda, sin los espacios
/// de alrededor (inicio, fin). La última «|» puede faltar.
pub fn table_cells(line: &str) -> (Vec<usize>, Vec<(usize, usize)>) {
    let pipes: Vec<usize> = line.char_indices().filter(|&(i, c)| c == '|' && !line[..i].ends_with('\\')).map(|(i, _)| i).collect();
    let mut cells = Vec::new();
    for (k, &p) in pipes.iter().enumerate() {
        let end = pipes.get(k + 1).copied().unwrap_or(line.len());
        let raw = &line[p + 1..end];
        if pipes.get(k + 1).is_none() && raw.trim().is_empty() {
            break; // después de la última «|»
        }
        let lead = raw.len() - raw.trim_start().len();
        let a = p + 1 + lead;
        cells.push((a, a + raw.trim().len()));
    }
    (pipes, cells)
}

/// Una fila vacía de `n` columnas.
pub fn empty_table_row(n: usize) -> String {
    format!("|{}", "  |".repeat(n.max(1)))
}

/// Una tabla nueva, para insertar.
pub const NEW_TABLE: [&str; 3] = ["| Columna 1 | Columna 2 | Columna 3 |", "|---|---|---|", "|  |  |  |"];

pub fn is_tag_only(line: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    !words.is_empty() && words.iter().all(|w| tags::tag_spans(w).first() == Some(&(0, w.len())))
}

// ---------- Palabras especiales dentro de una línea ----------

/// "due:2026-09-26" -> fecha.
pub fn due_of(line: &str) -> Option<String> {
    line.split_whitespace()
        .find_map(|w| w.strip_prefix("due:"))
        .filter(|d| crate::agenda::is_date(d))
        .map(str::to_string)
}

/// "^k3f9a" al final de la línea: el identificador de su tarea.
pub fn id_of(line: &str) -> Option<String> {
    let last = line.split_whitespace().last()?;
    let id = last.strip_prefix('^')?;
    (id.len() >= 3 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).then(|| id.to_string())
}

fn is_meta_word(w: &str) -> bool {
    w.strip_prefix("due:").is_some_and(crate::agenda::is_date)
        || w.strip_prefix('^').is_some_and(|id| id.len() >= 3 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// Agrega palabras al final del texto de una línea, antes de "due:" y "^id".
pub fn insert_words(line: &str, add: &str) -> String {
    if add.trim().is_empty() {
        return line.to_string();
    }
    let body = line.trim_end();
    let mut cut = body.len();
    for (start, word) in words_with_pos(body).into_iter().rev() {
        if is_meta_word(word) {
            cut = start;
        } else {
            break;
        }
    }
    let (head, tail) = body.split_at(cut);
    let head = head.trim_end();
    if tail.is_empty() { format!("{head} {add}") } else { format!("{head} {add} {}", tail.trim()) }
}

/// Agrega (o reemplaza) la fecha y el identificador al final de una línea.
pub fn set_meta(line: &str, due: Option<&str>, id: Option<&str>) -> String {
    let mut words: Vec<&str> = line.split_whitespace().collect();
    let keep_due = due_of(line);
    let keep_id = id_of(line);
    words.retain(|w| !is_meta_word(w));
    let lead = &line[..line.len() - line.trim_start().len()];
    let info = parse(line);
    let mut s = String::from(lead);
    // Conserva el prefijo tal cual (sangría, "- ", casilla).
    let prefix_words = line[info.indent..info.prefix].split_whitespace().count();
    s += &line[info.indent..info.prefix];
    s += &words[prefix_words..].join(" ");
    if let Some(d) = due.map(str::to_string).or(keep_due) {
        s += &format!(" due:{d}");
    }
    if let Some(i) = id.map(str::to_string).or(keep_id) {
        s += &format!(" ^{i}");
    }
    s
}

fn words_with_pos(s: &str) -> Vec<(usize, &str)> {
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

/// Qué es cada trozo especial de una línea (en bytes).
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Tag(String),
    Due(String),
    Id,
}

pub fn tokens(line: &str) -> Vec<(usize, usize, Token)> {
    let mut out: Vec<(usize, usize, Token)> = tags::tag_spans(line)
        .into_iter()
        .map(|(a, b)| (a, b, Token::Tag(line[a + 1..b].to_lowercase())))
        .collect();
    for (a, w) in words_with_pos(line) {
        if let Some(d) = w.strip_prefix("due:").filter(|d| crate::agenda::is_date(d)) {
            out.push((a, a + w.len(), Token::Due(d.to_string())));
        } else if w.starts_with('^') && is_meta_word(w) {
            out.push((a, a + w.len(), Token::Id));
        }
    }
    out.sort_by_key(|t| t.0);
    out
}

// ---------- Unidades: qué líneas forman cada nota ----------

/// Una nota dentro del archivo: una línea con sus líneas con sangría, o un bloque "##".
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    /// "L3" = nota que empieza en la línea 3; "B5" = bloque que empieza en la línea 5.
    pub id: String,
    /// Primera y última línea (desde 0, inclusive).
    pub first: usize,
    pub last: usize,
    pub block: bool,
}

pub fn units(text: &str) -> Vec<Unit> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<Unit> = Vec::new();
    // Unidad que puede seguir creciendo, y en qué modo.
    #[derive(PartialEq)]
    enum Open {
        None,
        Note,
        /// Bloque "##" sin cerrar: todo es parte de él.
        Block,
        /// Tras "## fin": siguen las sangrías, las etiquetas y un "### Resumen".
        Closed,
        /// Tras "### Título": su texto, hasta una línea en blanco o una de etiquetas.
        Section,
        /// Las filas de una tabla.
        Table,
    }
    let mut open = Open::None;
    for (i, raw) in lines.iter().enumerate() {
        let l = raw.trim();
        if l.is_empty() {
            open = Open::None;
            continue;
        }
        if is_block_start(l) {
            out.push(Unit { id: format!("B{}", i + 1), first: i, last: i, block: true });
            open = Open::Block;
            continue;
        }
        let extend = |out: &mut Vec<Unit>| {
            if let Some(u) = out.last_mut() {
                u.last = i;
            }
        };
        match open {
            Open::Block => {
                extend(&mut out);
                if is_block_end(l) {
                    open = Open::Closed;
                }
                continue;
            }
            Open::Section if !l.starts_with("# ") && !is_block_end(l) => {
                extend(&mut out);
                if is_tag_only(l) {
                    open = Open::Closed;
                }
                continue;
            }
            _ => {}
        }
        if l.starts_with("# ") || is_block_end(l) {
            open = Open::None;
            continue;
        }
        // Una tabla es una sola nota (y va dentro de la nota de arriba si tiene sangría).
        if is_table_row(l) {
            if open == Open::Table || (open != Open::None && raw.starts_with(' ')) {
                extend(&mut out);
            } else {
                out.push(Unit { id: format!("L{}", i + 1), first: i, last: i, block: false });
            }
            open = Open::Table;
            continue;
        }
        let info = parse(raw);
        let attaches = info.level >= 1 || is_tag_only(l) || l.starts_with("### ");
        if attaches && open != Open::None {
            extend(&mut out);
            if l.starts_with("### ") {
                open = Open::Section;
            }
            continue;
        }
        if attaches && (is_tag_only(l) || l.starts_with("### ")) {
            continue; // etiquetas o subtítulo sueltos: no son una nota
        }
        out.push(Unit { id: format!("L{}", i + 1), first: i, last: i, block: false });
        open = Open::Note;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_ups() {
        assert_eq!(follow_up("↳ 2026-10-01: Se pidió a Gerdau"), Some(("2026-10-01", 16)));
        assert_eq!(follow_up("↳ sin fecha"), Some(("", 4)));
        assert_eq!(follow_up("otra cosa"), None);
        let t = "- [x] Pedir acero ^ab12c\n  ↳ 2026-09-30: Cotizado\nOtra\n";
        assert_eq!(follow_ups_after(t, 0), vec![("2026-09-30".to_string(), "Cotizado".to_string())]);
        // El nuevo va después de los que ya hay, con un nivel más de sangría.
        let (t2, at) = add_follow_up(t, 0, "2026-10-01", "Pedido a Gerdau");
        assert_eq!(t2, "- [x] Pedir acero ^ab12c\n  ↳ 2026-09-30: Cotizado\n  ↳ 2026-10-01: Pedido a Gerdau\nOtra\n");
        assert_eq!(at, 2);
        assert!(is_follow_up("  ↳ 2026-10-01: x"));
        // Es parte de la nota de arriba.
        assert_eq!(units(&t2).iter().map(|u| (u.first, u.last)).collect::<Vec<_>>(), vec![(0, 2), (3, 3)]);
        // Bajo un detalle (nivel 1), va como ítem.
        let (t3, _) = add_follow_up("Nota\n  detalle\n", 1, "2026-10-01", "listo");
        assert_eq!(t3, "Nota\n  detalle\n  - ↳ 2026-10-01: listo\n");
    }

    #[test]
    fn tables() {
        let row = "| Acero A63 | 120 kg |";
        assert!(is_table_row(row) && !is_table_rule(row));
        assert!(is_table_rule("|---|:--:|") && is_table_rule("| --- | --- |"));
        assert!(!is_table_row("| solo una"));
        let (pipes, cells) = table_cells(row);
        assert_eq!(pipes, vec![0, 12, 21]);
        let texts: Vec<&str> = cells.iter().map(|&(a, b)| &row[a..b]).collect();
        assert_eq!(texts, vec!["Acero A63", "120 kg"]);
        // Sin la última «|» y con celdas vacías.
        let (_, cells) = table_cells("| a |  | c");
        assert_eq!(cells.len(), 3);
        assert_eq!(empty_table_row(2), "|  |  |");
        // Toda la tabla es una nota.
        let u = units("Materiales\n| a | b |\n|---|---|\n| 1 | 2 |\nOtra\n");
        assert_eq!(u.iter().map(|u| (u.first, u.last)).collect::<Vec<_>>(), vec![(0, 0), (1, 3), (4, 4)]);
    }

    #[test]
    fn note_and_file_links() {
        let l = "Ver [[Muro]] y [[Obra/Planos|los planos]] o [informe.pdf](../Adjuntos/informe%20final.pdf) ![x](a.png)";
        let found = links(l);
        assert_eq!(found.len(), 3);
        let (a, b, shown, link) = &found[0];
        assert_eq!((&l[*a..*b], &l[shown.0..shown.1], link), ("[[Muro]]", "Muro", &Link::Note("Muro".into())));
        let (_, _, shown, link) = &found[1];
        assert_eq!((&l[shown.0..shown.1], link), ("los planos", &Link::Note("Obra/Planos".into())));
        let (a, b, shown, link) = &found[2];
        assert_eq!(&l[*a..*b], "[informe.pdf](../Adjuntos/informe%20final.pdf)");
        assert_eq!(&l[shown.0..shown.1], "informe.pdf");
        assert_eq!(link, &Link::Markdown("../Adjuntos/informe%20final.pdf".into()));
        assert!(links("[[ ]] [sin destino]() [[a]b]]").is_empty());
        assert_eq!(decode_path("informe%20final.pdf"), "informe final.pdf");
        assert_eq!(decode_path(&encode_path("Acta (1) #2 100%.pdf")), "Acta (1) #2 100%.pdf");
        assert_eq!(encode_path("Acta técnica.pdf"), "Acta%20técnica.pdf");
    }

    #[test]
    fn levels_and_prefixes() {
        assert_eq!(parse("texto").level, 0);
        assert_eq!(parse("  sigue").level, 1);
        assert_eq!(parse("  - ítem").level, 2);
        assert_eq!(parse("    - sub").level, 3);
        assert_eq!(parse("\t- ítem").level, 2);
        assert_eq!(parse("- 15:03 reunión").level, 0);
        let t = parse("  - [x] hecho");
        assert_eq!((t.level, t.check, t.prefix), (2, Some(true), 8));
        assert_eq!(parse("- [ ] tarea").check, Some(false));
        for level in 0..=MAX_LEVEL {
            let line = prefix_for(level, None) + "hola";
            assert_eq!(parse(&line).level, level, "{line:?}");
            assert_eq!(&line[parse(&line).prefix..], "hola");
        }
        assert_eq!(with_level("hola", 1), "  hola");
        assert_eq!(with_level("  hola", 2), "  - hola");
        assert_eq!(with_level("  - hola", 3), "    - hola");
        assert_eq!(with_level("  - hola", 1), "  hola");
        assert_eq!(with_level("  hola", 0), "hola");
        assert_eq!(with_level("- [ ] tarea", 2), "  - [ ] tarea");
    }

    #[test]
    fn tasks_and_meta() {
        assert_eq!(make_task("Debo entregar"), "- [ ] Debo entregar");
        assert_eq!(make_task("  - ítem"), "  - [ ] ítem");
        assert_eq!(toggle_check("- [ ] a"), "- [x] a");
        assert_eq!(toggle_check("  - [x] a"), "  - [ ] a");
        assert_eq!(toggle_check("[ ] a"), "[x] a");
        let l = set_meta("- [ ] Entregar #utalca", Some("2026-09-26"), Some("k3f9a"));
        assert_eq!(l, "- [ ] Entregar #utalca due:2026-09-26 ^k3f9a");
        assert_eq!(due_of(&l).as_deref(), Some("2026-09-26"));
        assert_eq!(id_of(&l).as_deref(), Some("k3f9a"));
        assert_eq!(insert_words(&l, "#informe"), "- [ ] Entregar #utalca #informe due:2026-09-26 ^k3f9a");
        assert_eq!(set_meta(&l, None, None), l);
        let toks: Vec<Token> = tokens(&l).into_iter().map(|t| t.2).collect();
        assert_eq!(toks, vec![Token::Tag("utalca".into()), Token::Due("2026-09-26".into()), Token::Id]);
    }

    #[test]
    fn units_group_indented_lines_and_blocks() {
        let text = "Debo entregar el informe\n  Están en Dropbox\n  - revisar perfiles\n    - T-3\nOtra nota #x\n\n  suelta\n## Reunión Estructuras · 2026-09-24 15:00\n- 15:03 revisar vigas\n## fin · 15:42\n### Resumen\nSe revisaron.\n#reunión\nComprar pan\n# Título\n#a #b";
        let u = units(text);
        let ids: Vec<&str> = u.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, vec!["L1", "L5", "L7", "B8", "L14"]);
        assert_eq!((u[0].first, u[0].last), (0, 3));
        assert_eq!((u[3].first, u[3].last), (7, 12)); // reunión + resumen + etiquetas
    }
}
