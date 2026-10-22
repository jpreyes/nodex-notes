//! «Copias en conflicto» de Dropbox: cuando dos equipos cambian el mismo archivo sin conexión,
//! Dropbox deja una versión con el nombre original y la otra como
//! "Nombre (copia en conflicto de JP 2026-09-30).md" (o "(JP's conflicted copy 2026-09-30)").
//! Aquí se reconocen por su nombre y se juntan las listas (tareas, agenda, lo aprendido);
//! las notas se juntan con `merge::union`.

use crate::agenda;
use std::collections::HashMap;

/// Si el nombre es el de una copia en conflicto, el nombre del archivo original.
/// "Obra (copia en conflicto de JP 2026-09-30).md" -> "Obra.md".
pub fn original_of(file_name: &str) -> Option<String> {
    let (stem, ext) = match file_name.rsplit_once('.') {
        Some((s, e)) if !e.contains(' ') && !e.contains(')') => (s, format!(".{e}")),
        _ => (file_name, String::new()),
    };
    // El último paréntesis del nombre (puede venir seguido de " (1)").
    let mut rest = stem.trim_end();
    loop {
        let inner_start = rest.strip_suffix(')')?.rfind('(')?;
        let inner = crate::vault::fold(&rest[inner_start + 1..rest.len() - 1]);
        let before = rest[..inner_start].trim_end();
        if inner.contains("conflicted copy") || inner.contains("copia en conflicto") || inner.contains("copia conflictiva") {
            return (!before.is_empty()).then(|| format!("{before}{ext}"));
        }
        // "(1)", "(2)"… después de la marca: se sigue mirando hacia atrás.
        if inner.chars().all(|c| c.is_ascii_digit()) && !inner.is_empty() {
            rest = before;
        } else {
            return None;
        }
    }
}

/// Junta dos listas de líneas: las del archivo principal y, al final, las de la copia que no
/// estaban (iguales exactas no se repiten).
pub fn merge_lines(main: &str, copy: &str) -> String {
    let mut out: Vec<&str> = main.lines().filter(|l| !l.trim().is_empty()).collect();
    for l in copy.lines().filter(|l| !l.trim().is_empty()) {
        if !out.contains(&l) {
            out.push(l);
        }
    }
    let mut text = out.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

/// Junta dos versiones de tareas.txt. Una tarea es la misma si tiene el mismo `id:` (o, sin
/// identificador, el mismo texto y la misma nota). Si en un lado está hecha, queda hecha;
/// las que solo están en la copia se agregan al final.
pub fn merge_tasks(main: &str, copy: &str) -> String {
    let key = |l: &str| -> Option<(Option<String>, String, Option<String>)> {
        let t = agenda::parse_task(l)?;
        Some(match t.id {
            Some(id) => (Some(id), String::new(), None),
            None => (None, t.text.to_lowercase(), t.note),
        })
    };
    let done = |l: &str| agenda::parse_task(l).is_some_and(|t| t.done);
    let mut out: Vec<String> = main.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect();
    for l in copy.lines().filter(|l| !l.trim().is_empty()) {
        let k = key(l);
        match out.iter().position(|o| k.is_some() && key(o) == k) {
            // La misma tarea: si en la copia está hecha y aquí no, gana la hecha.
            Some(i) if done(l) && !done(&out[i]) => out[i] = l.to_string(),
            Some(_) => {}
            None if !out.iter().any(|o| o == l) => out.push(l.to_string()),
            None => {}
        }
    }
    let mut text = out.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

/// Junta dos versiones de tareas.txt que partieron de `base` (al sincronizar): tarea por tarea
/// (por su `id:`, o su texto y su nota), lo que cambió un solo lado gana; si cambiaron los dos,
/// gana la hecha (o la de aquí); lo que se borró en un lado sin cambiar en el otro, se va; lo
/// nuevo de los dos queda. Así no vuelven tareas borradas ni quedan dos versiones de una.
pub fn merge_tasks3(base: &str, mine: &str, theirs: &str) -> String {
    fn key(l: &str) -> String {
        match agenda::parse_task(l) {
            Some(t) => match t.id {
                Some(id) => format!("id:{id}"),
                None => format!("t:{}|{}", t.text.to_lowercase(), t.note.unwrap_or_default()),
            },
            None => format!("l:{l}"),
        }
    }
    fn map(text: &str) -> (Vec<String>, HashMap<String, String>) {
        let mut order = Vec::new();
        let mut m = HashMap::new();
        for l in text.lines().filter(|l| !l.trim().is_empty()) {
            let k = key(l);
            if m.insert(k.clone(), l.to_string()).is_none() {
                order.push(k);
            }
        }
        (order, m)
    }
    let done = |l: &str| agenda::parse_task(l).is_some_and(|t| t.done);
    let (_, b) = map(base);
    let (mine_order, o) = map(mine);
    let (theirs_order, t) = map(theirs);
    let mut out = Vec::new();
    for k in mine_order.iter().chain(theirs_order.iter().filter(|k| !o.contains_key(*k))) {
        let line = match (b.get(k), o.get(k), t.get(k)) {
            (_, Some(x), Some(y)) if x == y => Some(x),
            (Some(b), Some(x), Some(y)) if x == b => Some(y),
            (Some(b), Some(x), Some(y)) if y == b => Some(x),
            (_, Some(x), Some(y)) => Some(if done(y) && !done(x) { y } else { x }),
            // Borrada en un lado: se va, salvo que el otro la haya cambiado.
            (Some(b), Some(x), None) => (x != b).then_some(x),
            (Some(b), None, Some(y)) => (y != b).then_some(y),
            (None, Some(x), None) => Some(x),
            (None, None, Some(y)) => Some(y),
            (_, None, None) => None,
        };
        out.extend(line.cloned());
    }
    let mut text = out.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

/// Lo mismo para una lista de líneas sueltas (agenda.txt, aprendido.txt): quedan las que siguen
/// en los dos lados y las nuevas de cada uno; las que un lado borró, se van.
pub fn merge_lines3(base: &str, mine: &str, theirs: &str) -> String {
    let set = |s: &str| s.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect::<Vec<_>>();
    let (b, o, t) = (set(base), set(mine), set(theirs));
    let mut out: Vec<String> = Vec::new();
    for l in o.iter().chain(t.iter()) {
        if out.contains(l) {
            continue;
        }
        let (in_b, in_o, in_t) = (b.contains(l), o.contains(l), t.contains(l));
        if (in_o && in_t) || !in_b {
            out.push(l.clone());
        }
    }
    let mut text = out.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Las tareas se juntan con la versión de la que partieron: lo borrado no vuelve, lo hecho
    /// queda hecho, lo cambiado en un lado gana y no quedan dos versiones de una misma tarea.
    #[test]
    fn merges_task_lists_with_their_base() {
        let base = "2026-10-01 Enviar planos +Obra id:pl001\n2026-10-01 Llamar a Pedro +General id:pe002\n2026-10-01 Comprar pan +General\n2026-10-01 Pagar luz +General id:lu004\n";
        // Aquí: se hizo «Enviar planos» y se borró «Comprar pan».
        let mine = "x 2026-10-02 2026-10-01 Enviar planos +Obra id:pl001\n2026-10-01 Llamar a Pedro +General id:pe002\n2026-10-01 Pagar luz +General id:lu004\n";
        // Allá: «Llamar a Pedro» tiene fecha, se borró «Pagar luz» y hay una nueva.
        let theirs = "2026-10-01 Enviar planos +Obra id:pl001\n2026-10-01 Llamar a Pedro +General due:2026-10-05 id:pe002\n2026-10-01 Comprar pan +General\n2026-10-02 Nueva del celular +General id:nu005\n";
        assert_eq!(
            merge_tasks3(base, mine, theirs),
            "x 2026-10-02 2026-10-01 Enviar planos +Obra id:pl001\n2026-10-01 Llamar a Pedro +General due:2026-10-05 id:pe002\n2026-10-02 Nueva del celular +General id:nu005\n"
        );
        // Cambiada en los dos: gana la hecha.
        let a = "2026-10-01 Llamar a Pedro +General due:2026-10-06 id:pe002\n";
        let b = "x 2026-10-02 2026-10-01 Llamar a Pedro +General id:pe002\n";
        assert_eq!(merge_tasks3("2026-10-01 Llamar a Pedro +General id:pe002\n", a, b), b);
        // Sin base (los dos la crearon): quedan las de los dos, sin repetir.
        assert_eq!(merge_tasks3("", "2026-10-01 A +G id:a1\n", "2026-10-01 A +G id:a1\n2026-10-01 B +G id:b2\n"), "2026-10-01 A +G id:a1\n2026-10-01 B +G id:b2\n");
        // Agenda y otras listas.
        let base = "2026-10-03 Visita +Obra\n2026-10-04 Clase +U\n";
        assert_eq!(merge_lines3(base, "2026-10-03 Visita +Obra\n2026-10-05 Dentista +G\n", "2026-10-03 Visita +Obra\n2026-10-04 Clase +U\n2026-10-06 Webinar +U\n"), "2026-10-03 Visita +Obra\n2026-10-05 Dentista +G\n2026-10-06 Webinar +U\n");
    }

    #[test]
    fn recognizes_conflicted_copies() {
        let o = |n: &str| original_of(n);
        assert_eq!(o("Obra (copia en conflicto de JP 2026-09-30).md").as_deref(), Some("Obra.md"));
        assert_eq!(o("Obra (Copia en conflicto de JPLaptop 2026-09-30) (1).md").as_deref(), Some("Obra.md"));
        assert_eq!(o("2026-09-30 (JP's conflicted copy 2026-09-30).md").as_deref(), Some("2026-09-30.md"));
        assert_eq!(o("tareas (JP Reyes's conflicted copy 2026-09-30).txt").as_deref(), Some("tareas.txt"));
        assert_eq!(o("Reunión (avance) (copia en conflicto de JP 2026-09-30).md").as_deref(), Some("Reunión (avance).md"));
        // No lo son.
        assert_eq!(o("Obra.md"), None);
        assert_eq!(o("Reunión (avance).md"), None);
        assert_eq!(o("Notas sobre una copia en conflicto.md"), None);
        assert_eq!(o("Obra (2).md"), None);
        assert_eq!(o("(copia en conflicto de JP 2026-09-30).md"), None, "sin nombre original");
    }

    #[test]
    fn merges_task_lists() {
        let main = "2026-09-28 Enviar planos +Obra due:2026-10-02 nota:Obra/Muro id:pl001\n2026-09-28 Llamar a Pedro +General id:pe002\n2026-09-29 Comprar pan +General\n";
        let copy = "x 2026-09-30 2026-09-28 Enviar planos +Obra due:2026-10-02 nota:Obra/Muro id:pl001\n2026-09-28 Llamar a Pedro +General due:2026-10-05 id:pe002\n2026-09-29 Comprar pan +General\n2026-09-30 Nueva del otro equipo +General id:nu003\n";
        let merged = merge_tasks(main, copy);
        assert_eq!(
            merged,
            "x 2026-09-30 2026-09-28 Enviar planos +Obra due:2026-10-02 nota:Obra/Muro id:pl001\n2026-09-28 Llamar a Pedro +General id:pe002\n2026-09-29 Comprar pan +General\n2026-09-30 Nueva del otro equipo +General id:nu003\n"
        );
        // Hacerlo dos veces no cambia nada.
        assert_eq!(merge_tasks(&merged, copy), merged);
        assert_eq!(merge_lines("a\nb\n", "b\nc\n"), "a\nb\nc\n");
    }
}
