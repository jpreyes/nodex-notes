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
