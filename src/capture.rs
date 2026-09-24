//! Notas de captura (la nota del día y las "Sin título"): cada línea es una nota distinta,
//! salvo los bloques que empiezan con "##" (una reunión o un tema), que van juntos.
//! Un bloque termina con "## fin …", con el siguiente "##" o con una línea en blanco.

/// Una unidad de una nota de captura: una línea suelta o un bloque "##".
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    /// "L3" = línea 3; "B5" = bloque que empieza en la línea 5.
    pub id: String,
    /// Primera y última línea (desde 0, inclusive).
    pub first: usize,
    pub last: usize,
    pub block: bool,
}

/// ¿Es una nota de captura? La nota del día (título "AAAA-MM-DD") y las "Sin título".
pub fn is_capture(title: &str) -> bool {
    title.starts_with("Sin título") || crate::agenda::is_date(title.trim())
}

fn is_block_start(l: &str) -> bool {
    l.starts_with("## ") && !is_block_end(l)
}

fn is_block_end(l: &str) -> bool {
    l.trim_start_matches('#').trim_start().to_lowercase().starts_with("fin")
        && l.starts_with("##")
}

pub fn units(text: &str) -> Vec<Unit> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i].trim();
        // Vacías, títulos "# …" y "## fin" sueltos no son notas.
        if l.is_empty() || l.starts_with("# ") || is_block_end(l) {
            i += 1;
            continue;
        }
        if is_block_start(l) {
            let mut j = i + 1;
            while j < lines.len() {
                let m = lines[j].trim();
                if m.is_empty() || is_block_start(m) {
                    break;
                }
                j += 1;
                if is_block_end(m) {
                    break;
                }
            }
            out.push(Unit { id: format!("B{}", i + 1), first: i, last: j - 1, block: true });
            i = j;
        } else {
            out.push(Unit { id: format!("L{}", i + 1), first: i, last: i, block: false });
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_lines_and_blocks() {
        let text = "Debo entregar el informe\nLas trincheras están en Dropbox\n\n## Reunión Estructuras · 2026-09-24 15:00\n- 15:03 revisar vigas\n- 15:10 enviar planos\n## fin · 15:42\nComprar pan\n## Ideas paper\nComparar amortiguamiento\n\nOtra cosa";
        let u = units(text);
        let ids: Vec<&str> = u.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, vec!["L1", "L2", "B4", "L8", "B9", "L12"]);
        assert_eq!((u[2].first, u[2].last), (3, 6)); // la reunión incluye "## fin"
        assert_eq!((u[4].first, u[4].last), (8, 9)); // el bloque termina en la línea en blanco
        assert!(is_capture("2026-09-24") && is_capture("Sin título 2") && !is_capture("Trincheras"));
    }
}
