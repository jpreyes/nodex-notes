//! Juntar dos versiones de una nota, línea por línea.
//!
//! `merge3` es como el de git: a partir de la versión que las dos tenían en común (la base),
//! toma lo que cambió cada una. Si las dos cambiaron las mismas líneas de forma distinta, se
//! quedan las dos versiones de esas líneas (primero la mía, después la otra) y se cuenta como
//! conflicto: nunca se pierde nada.
//!
//! `union` sirve cuando no hay base (por ejemplo, una «copia en conflicto» de Dropbox): conserva
//! todas las líneas de las dos versiones, en orden.

/// Con notas más largas que esto no se compara línea por línea (la tabla sería enorme).
const MAX_LINES: usize = 4000;

pub struct Merged {
    pub text: String,
    /// Tramos donde las dos versiones cambiaron lo mismo de forma distinta (quedaron las dos).
    pub conflicts: usize,
}

/// Pares (i, j) de líneas iguales de `a` y `b`, en orden: la subsecuencia común más larga.
pub fn common(a: &[&str], b: &[&str]) -> Vec<(usize, usize)> {
    // Lo igual del comienzo y del final no necesita tabla.
    let mut start = 0;
    while start < a.len() && start < b.len() && a[start] == b[start] {
        start += 1;
    }
    let mut end = 0;
    while end < a.len() - start && end < b.len() - start && a[a.len() - 1 - end] == b[b.len() - 1 - end] {
        end += 1;
    }
    let (am, bm) = (&a[start..a.len() - end], &b[start..b.len() - end]);
    let mut out: Vec<(usize, usize)> = (0..start).map(|i| (i, i)).collect();
    if !am.is_empty() && !bm.is_empty() {
        let (n, m) = (am.len(), bm.len());
        let mut t = vec![0u32; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                t[i * (m + 1) + j] = if am[i] == bm[j] { t[(i + 1) * (m + 1) + j + 1] + 1 } else { t[(i + 1) * (m + 1) + j].max(t[i * (m + 1) + j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if am[i] == bm[j] {
                out.push((start + i, start + j));
                i += 1;
                j += 1;
            } else if t[(i + 1) * (m + 1) + j] >= t[i * (m + 1) + j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    out.extend((0..end).map(|k| (a.len() - end + k, b.len() - end + k)));
    out
}

/// Junta `mine` y `theirs`, que parten de `base`.
pub fn merge3(base: &str, mine: &str, theirs: &str) -> Merged {
    if mine == theirs || theirs == base {
        return Merged { text: mine.to_string(), conflicts: 0 };
    }
    if mine == base {
        return Merged { text: theirs.to_string(), conflicts: 0 };
    }
    let (b, m, t): (Vec<&str>, Vec<&str>, Vec<&str>) = (base.split('\n').collect(), mine.split('\n').collect(), theirs.split('\n').collect());
    if b.len().max(m.len()).max(t.len()) > MAX_LINES {
        // Demasiado larga para comparar: se conservan las dos completas.
        return Merged { text: format!("{}\n{}", mine.trim_end_matches('\n'), theirs), conflicts: 1 };
    }
    // Dónde quedó cada línea de la base en cada versión (si sigue ahí).
    let mut in_mine = vec![None; b.len()];
    for (i, j) in common(&b, &m) {
        in_mine[i] = Some(j);
    }
    let mut in_theirs = vec![None; b.len()];
    for (i, j) in common(&b, &t) {
        in_theirs[i] = Some(j);
    }
    let mut out: Vec<&str> = Vec::new();
    let mut conflicts = 0;
    let (mut b0, mut m0, mut t0) = (0, 0, 0);
    for bi in 0..=b.len() {
        // Un punto firme: una línea de la base que las dos conservan (o el final).
        let (mi, ti) = if bi == b.len() {
            (m.len(), t.len())
        } else {
            match (in_mine[bi], in_theirs[bi]) {
                (Some(mi), Some(ti)) => (mi, ti),
                _ => continue,
            }
        };
        let (cb, cm, ct) = (&b[b0..bi], &m[m0..mi], &t[t0..ti]);
        if cm == cb {
            out.extend_from_slice(ct); // solo cambió la otra
        } else if ct == cb || cm == ct {
            out.extend_from_slice(cm); // solo cambié yo, o cambiamos lo mismo
        } else {
            // Las dos cambiaron este tramo. Se respeta lo que cada una borró de la base y se
            // conservan las líneas nuevas de ambas (primero las mías).
            let base_in_mine: Vec<(usize, usize)> = common(cb, cm);
            let kept_by_theirs: Vec<usize> = common(cb, ct).into_iter().map(|(i, _)| i).collect();
            let theirs_from_base: Vec<usize> = common(cb, ct).into_iter().map(|(_, j)| j).collect();
            let mut mine_new = 0;
            for (j, line) in cm.iter().enumerate() {
                match base_in_mine.iter().find(|(_, mj)| *mj == j) {
                    // Una línea de la base que yo conservé: queda solo si la otra también la conservó.
                    Some((bi, _)) if !kept_by_theirs.contains(bi) => {}
                    Some(_) => out.push(line),
                    None => {
                        mine_new += 1;
                        out.push(line);
                    }
                }
            }
            let mut theirs_new = 0;
            for (j, line) in ct.iter().enumerate() {
                if !theirs_from_base.contains(&j) && !cm.contains(line) {
                    theirs_new += 1;
                    out.push(line);
                }
            }
            // Conflicto de verdad: las dos escribieron algo distinto en el mismo lugar.
            if mine_new > 0 && theirs_new > 0 {
                conflicts += 1;
            }
        }
        if bi < b.len() {
            out.push(b[bi]);
            (b0, m0, t0) = (bi + 1, mi + 1, ti + 1);
        }
    }
    Merged { text: out.join("\n"), conflicts }
}

/// Todas las líneas de las dos versiones, en orden (sin base): lo común una vez, y lo que tiene
/// solo una u otra, también. Puede hacer reaparecer una línea que una de las dos había borrado.
pub fn union(a: &str, b: &str) -> String {
    if a == b || b.trim().is_empty() {
        return a.to_string();
    }
    if a.trim().is_empty() {
        return b.to_string();
    }
    let (la, lb): (Vec<&str>, Vec<&str>) = (a.split('\n').collect(), b.split('\n').collect());
    if la.len().max(lb.len()) > MAX_LINES {
        return format!("{}\n{}", a.trim_end_matches('\n'), b);
    }
    let mut out: Vec<&str> = Vec::new();
    let (mut i, mut j) = (0, 0);
    for (ci, cj) in common(&la, &lb).into_iter().chain([(la.len(), lb.len())]) {
        out.extend_from_slice(&la[i..ci]);
        out.extend(lb[j..cj].iter().filter(|l| !l.trim().is_empty() || ci == la.len()));
        if ci < la.len() {
            out.push(la[ci]);
        }
        (i, j) = (ci + 1, cj + 1);
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_changes_on_different_lines() {
        let base = "uno\ndos\ntres\n";
        let mine = "uno\ndos\ntres\ncuatro\n"; // agregué al final
        let theirs = "cero\nuno\ndos cambiado\ntres\n"; // el otro agregó arriba y cambió una línea
        let m = merge3(base, mine, theirs);
        assert_eq!((m.text.as_str(), m.conflicts), ("cero\nuno\ndos cambiado\ntres\ncuatro\n", 0));
        // Da lo mismo quién es quién.
        assert_eq!(merge3(base, theirs, mine).text, m.text);
    }

    /// La IA de otro equipo sacó líneas de la nota del día mientras aquí se escribía una nueva:
    /// las líneas movidas no vuelven y la nueva se conserva.
    #[test]
    fn lines_moved_elsewhere_stay_moved() {
        let base = "Llamar a Pedro\nRevisar planos del muro\nComprar pan\n";
        let mine = "Llamar a Pedro\nRevisar planos del muro\nComprar pan\nIdea nueva para el curso\n";
        let theirs = "Comprar pan\n"; // la IA se llevó dos líneas a sus notas
        let m = merge3(base, mine, theirs);
        assert_eq!((m.text.as_str(), m.conflicts), ("Comprar pan\nIdea nueva para el curso\n", 0));
        // Aunque se las haya llevado todas.
        let m = merge3(base, mine, "");
        assert_eq!((m.text.as_str(), m.conflicts), ("Idea nueva para el curso\n", 0));
        // Y si además cambié una de las que se llevó, mi cambio no se pierde.
        let mine = "Llamar a Pedro HOY\nRevisar planos del muro\nComprar pan\n";
        let m = merge3(base, mine, "Comprar pan\n");
        assert_eq!((m.text.as_str(), m.conflicts), ("Llamar a Pedro HOY\nComprar pan\n", 0));
    }

    #[test]
    fn same_line_changed_twice_keeps_both() {
        let base = "uno\nEntregar informe\ntres";
        let m = merge3(base, "uno\nEntregar informe el viernes\ntres", "uno\nEntregar informe a Juan\ntres");
        assert_eq!((m.text.as_str(), m.conflicts), ("uno\nEntregar informe el viernes\nEntregar informe a Juan\ntres", 1));
        // Lo mismo en las dos: una sola vez.
        let m = merge3(base, "uno\nX\ntres", "uno\nX\ntres");
        assert_eq!((m.text.as_str(), m.conflicts), ("uno\nX\ntres", 0));
        // Una borra una línea y la otra la cambia: gana lo que quedó escrito.
        let m = merge3(base, "uno\ntres", "uno\nEntregar informe YA\ntres");
        assert_eq!((m.text.as_str(), m.conflicts), ("uno\nEntregar informe YA\ntres", 0));
    }

    #[test]
    fn trivial_cases() {
        assert_eq!(merge3("a", "a", "b").text, "b");
        assert_eq!(merge3("a", "b", "a").text, "b");
        assert_eq!(merge3("", "mío\n", "otro\n").text, "mío\notro\n");
        assert_eq!(merge3("a\nb\n", "", "a\nb\nc\n").text, "c\n", "borré todo y el otro agregó: queda lo agregado");
    }

    #[test]
    fn union_keeps_every_line() {
        assert_eq!(union("uno\ndos\ntres\n", "uno\ntres\ncuatro\n"), "uno\ndos\ntres\ncuatro\n");
        assert_eq!(union("a\n", "a\n"), "a\n");
        assert_eq!(union("", "b\n"), "b\n");
        assert_eq!(union("a\nb", "c\nb"), "a\nc\nb");
    }
}
