//! Notas de captura (la nota del día y las "Sin título"): cada nota que tienen dentro
//! (una línea con sus líneas con sangría, o un bloque "##") se mueve a su propia nota.
//! La división en unidades está en `lines.rs`.

/// ¿Es una nota de captura? La nota del día (título "AAAA-MM-DD") y las "Sin título".
pub fn is_capture(title: &str) -> bool {
    title.starts_with("Sin título") || crate::agenda::is_date(title.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lines::units;

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
