//! Etiquetas "#palabra" dentro del texto de las notas.

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Rangos en bytes (incluyendo el '#') de cada etiqueta de una línea.
pub fn tag_spans(line: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut prev: Option<char> = None;
    for (i, c) in line.char_indices() {
        // '#' al inicio o tras un separador (no "C#", "url#ancla" ni "##").
        if c == '#' && prev.is_none_or(|p| !p.is_alphanumeric() && p != '#') {
            let rest = &line[i + 1..];
            let len: usize = rest.chars().take_while(|&c| is_tag_char(c)).map(char::len_utf8).sum();
            let len = rest[..len].trim_end_matches('-').len();
            if len > 0 {
                out.push((i, i + 1 + len));
            }
        }
        prev = Some(c);
    }
    out
}

/// Etiquetas de una línea, en minúsculas, sin '#' y sin repetir.
pub fn line_tags(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (a, b) in tag_spans(line) {
        let t = line[a + 1..b].to_lowercase();
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_tags() {
        assert_eq!(line_tags("revisar #Vigas, y #eje-3. (#obra) #vigas"), vec!["vigas", "eje-3", "obra"]);
        assert!(line_tags("C# y url#ancla y ## Título y # suelto").is_empty());
        assert_eq!(line_tags("#diseño_final-"), vec!["diseño_final"]);
        assert_eq!(tag_spans("a #bé c"), vec![(2, 6)]);
    }
}
