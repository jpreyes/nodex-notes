//! Lo que respondió la IA, aplicado al texto de una nota (sin tocar el disco):
//!
//! - cada nota de adentro (línea o bloque "##") recibe sus propias etiquetas en su línea;
//! - la línea de donde sale una tarea pasa a tener casilla, fecha e identificador
//!   ("- [ ] Entregar informe due:2026-09-26 ^k3f9a");
//! - una línea que es detalle de otra se une a ella con sangría;
//! - en las notas de captura, cada nota (con sus detalles) puede irse a otra nota.

use crate::ai::{AiUnit, Analysis};
use crate::lines;
use crate::tags;
use std::collections::{HashMap, HashSet};

pub const MEETING_TAG: &str = "reunión";

/// Deja una etiqueta como "palabra-compuesta" (sin '#', minúsculas).
pub fn clean_tag(t: &str) -> String {
    let t = t.trim().trim_start_matches('#').to_lowercase().replace(' ', "-");
    t.chars().filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect::<String>().trim_matches('-').to_string()
}

pub struct Plan<D> {
    /// Lo que queda en la nota.
    pub source: String,
    /// Destinos y el texto que se agrega a cada uno, en orden.
    pub moves: Vec<(D, String)>,
    /// Unidad (id) -> índice en `moves` si se fue a otra nota.
    pub placed: HashMap<String, usize>,
    /// Identificador puesto en la línea de cada tarea (mismo orden que `Analysis::tareas`).
    pub task_ids: Vec<Option<String>>,
    pub tags_added: usize,
    pub grouped: usize,
}

fn norm(id: &str) -> String {
    id.trim().to_uppercase()
}

/// `dest` dice a dónde va una unidad (`None` = se queda); `new_id` inventa identificadores de tarea.
pub fn plan<D: Clone + PartialEq>(
    text: &str,
    a: &Analysis,
    mut dest: impl FnMut(&AiUnit) -> Option<D>,
    mut new_id: impl FnMut() -> String,
) -> Plan<D> {
    let lines: Vec<&str> = text.lines().collect();
    let units = lines::units(text);
    let index: HashMap<String, usize> = units.iter().enumerate().map(|(i, u)| (u.id.clone(), i)).collect();
    let mut ai_of: HashMap<usize, AiUnit> = HashMap::new();
    for au in &a.unidades {
        if let Some(&i) = index.get(&norm(&au.id)) {
            ai_of.entry(i).or_insert_with(|| au.clone());
        }
    }
    // Nota con título que es una reunión: su bloque "##" (o su primera nota) lleva #reunión y el resumen.
    if a.es_reunion && !units.is_empty() {
        let i = units.iter().position(|u| u.block).unwrap_or(0);
        let au = ai_of.entry(i).or_insert_with(|| AiUnit { id: units[i].id.clone(), ..AiUnit::default() });
        au.es_reunion = true;
        if au.resumen.trim().is_empty() {
            au.resumen = a.resumen.clone();
        }
    }
    // Respuesta al estilo antiguo (etiquetas de toda la nota): van a la primera nota.
    if a.unidades.is_empty() && !a.etiquetas.is_empty() && !units.is_empty() {
        let au = ai_of.entry(0).or_insert_with(|| AiUnit { id: units[0].id.clone(), ..AiUnit::default() });
        au.etiquetas.extend(a.etiquetas.iter().cloned());
    }

    let mut body: Vec<Vec<String>> = units.iter().map(|u| lines[u.first..=u.last].iter().map(|l| l.to_string()).collect()).collect();
    let mut tags_added = 0;

    // 1. Etiquetas (y resumen de reunión) de cada unidad.
    for (i, u) in units.iter().enumerate() {
        let Some(au) = ai_of.get(&i) else { continue };
        let have: HashSet<String> = body[i].iter().flat_map(|l| tags::line_tags(l)).collect();
        let mut add: Vec<String> = Vec::new();
        if au.es_reunion {
            add.push(MEETING_TAG.into());
        }
        add.extend(au.etiquetas.iter().map(|t| clean_tag(t)).filter(|t| !t.is_empty()).take(3));
        let mut seen = HashSet::new();
        add.retain(|t| !have.contains(t) && seen.insert(t.clone()));
        let hashes = add.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ");
        tags_added += add.len();
        if u.block {
            let closed = body[i].iter().any(|l| lines::is_block_end(l));
            let has_summary = body[i].iter().any(|l| l.trim_start().starts_with("### Resumen"));
            if au.es_reunion && closed && !has_summary && !au.resumen.trim().is_empty() {
                // El resumen va después de "## fin" (y antes de una línea de etiquetas que ya exista).
                let at = body[i].iter().rposition(|l| lines::is_block_end(l)).map_or(body[i].len(), |p| p + 1);
                body[i].splice(at..at, ["### Resumen".to_string(), au.resumen.trim().to_string()]);
            }
            if !hashes.is_empty() {
                match body[i].last_mut().filter(|l| lines::is_tag_only(l)) {
                    Some(last) => *last = format!("{} {hashes}", last.trim_end()),
                    None => body[i].push(hashes),
                }
            }
        } else if !hashes.is_empty() {
            body[i][0] = lines::insert_words(&body[i][0], &hashes);
        }
    }

    // 2. Tareas: la línea de la nota pasa a tener casilla, fecha e identificador.
    let mut task_ids = vec![None; a.tareas.len()];
    let mut marked: HashSet<usize> = HashSet::new();
    for (ti, t) in a.tareas.iter().enumerate() {
        let Some(&i) = index.get(&norm(&t.unidad)) else { continue };
        if units[i].block || !marked.insert(i) {
            continue;
        }
        let head = &body[i][0];
        let info = lines::parse(head);
        if info.check == Some(true) {
            continue;
        }
        let id = lines::id_of(head).unwrap_or_else(&mut new_id);
        let due = Some(t.fecha.trim()).filter(|d| crate::agenda::is_date(d) && lines::due_of(head).is_none());
        body[i][0] = lines::set_meta(&lines::make_task(head), due, Some(&id));
        task_ids[ti] = Some(id);
    }

    // 3. Detalles: una unidad que completa a otra se une a ella con sangría.
    let mut parent: HashMap<usize, usize> = HashMap::new();
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    for (&i, au) in &ai_of {
        let Some(&p) = index.get(&norm(&au.de)) else { continue };
        if p == i || units[i].block || units[p].block {
            continue;
        }
        parent.insert(i, p);
    }
    // Cadenas (C de B, B de A): todo va a la raíz; y sin ciclos.
    let root = |mut i: usize| {
        let mut steps = 0;
        while let Some(&p) = parent.get(&i) {
            i = p;
            steps += 1;
            if steps > units.len() {
                return None;
            }
        }
        Some(i)
    };
    let mut grouped = 0;
    let mut absorbed: HashSet<usize> = HashSet::new();
    let mut child_list: Vec<usize> = parent.keys().copied().collect();
    child_list.sort();
    for i in child_list {
        let Some(r) = root(i) else { continue };
        if r == i {
            continue;
        }
        if lines::parse(&body[i][0]).level == 0 {
            body[i][0] = lines::with_level(&body[i][0], 1);
        }
        children.entry(r).or_default().push(i);
        absorbed.insert(i);
        grouped += 1;
    }
    let full = |i: usize| -> Vec<String> {
        let mut v = body[i].clone();
        for &c in children.get(&i).map(Vec::as_slice).unwrap_or(&[]) {
            v.extend(body[c].iter().cloned());
        }
        v
    };

    // 4. Destinos: cada unidad (con sus detalles) puede irse a otra nota.
    let mut moves: Vec<(D, String)> = Vec::new();
    let mut placed: HashMap<String, usize> = HashMap::new();
    let mut gone: HashSet<usize> = HashSet::new();
    for (i, u) in units.iter().enumerate() {
        if absorbed.contains(&i) {
            continue;
        }
        let Some(d) = ai_of.get(&i).and_then(&mut dest) else { continue };
        let chunk = full(i).join("\n");
        let chunk = if u.block { format!("\n{}\n", chunk.trim_end()) } else { chunk };
        let mi = match moves.iter().position(|(x, _)| *x == d) {
            Some(mi) => {
                moves[mi].1 = format!("{}\n{chunk}", moves[mi].1);
                mi
            }
            None => {
                moves.push((d, chunk));
                moves.len() - 1
            }
        };
        gone.insert(i);
        placed.insert(u.id.clone(), mi);
        for &c in children.get(&i).map(Vec::as_slice).unwrap_or(&[]) {
            placed.insert(units[c].id.clone(), mi);
        }
    }
    for (_, chunk) in &mut moves {
        *chunk = chunk.replace("\n\n\n", "\n\n").trim_matches('\n').to_string();
    }

    // 5. Lo que queda en la nota, en su orden.
    let start_of: HashMap<usize, usize> = units.iter().enumerate().map(|(i, u)| (u.first, i)).collect();
    let in_unit: HashSet<usize> = units.iter().flat_map(|u| u.first..=u.last).collect();
    let mut out: Vec<String> = Vec::new();
    for (li, l) in lines.iter().enumerate() {
        if let Some(&i) = start_of.get(&li) {
            if !absorbed.contains(&i) && !gone.contains(&i) {
                out.extend(full(i));
            }
        } else if !in_unit.contains(&li) {
            out.push(l.to_string());
        }
    }
    let mut clean: Vec<String> = Vec::new();
    for l in out {
        if l.trim().is_empty() && clean.last().is_none_or(|p| p.trim().is_empty()) {
            continue;
        }
        clean.push(l);
    }
    let source = clean.join("\n").trim().to_string();
    let source = if source.is_empty() { source } else { source + "\n" };
    Plan { source, moves, placed, task_ids, tags_added, grouped }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analysis(json: &str) -> Analysis {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn titled_note_gets_tags_tasks_and_groups_per_line() {
        let text = "Debo entregar el informe de revisión de las trincheras\nLas trincheras están en la carpeta dropbox /workspace/x\nDebo entregar mañana el informe a la UTalca\nDebo entregar la próxima semana el LaVet\n";
        let a = analysis(
            r##"{"unidades": [
                {"id": "L1", "etiquetas": ["informe", "trincheras"]},
                {"id": "L2", "etiquetas": ["trincheras"], "de": "L1"},
                {"id": "L3", "etiquetas": ["informe", "#UTalca"]},
                {"id": "L4", "etiquetas": ["lavet"]}],
              "tareas": [
                {"texto": "Entregar el informe de trincheras", "fecha": "", "unidad": "L1"},
                {"texto": "Entregar el informe a la UTalca", "fecha": "2026-09-26", "unidad": "L3"},
                {"texto": "Entregar el LaVet", "fecha": "2026-10-02", "unidad": "L4"}]}"##,
        );
        let mut n = 0;
        let p = plan(text, &a, |_| None::<()>, || {
            n += 1;
            format!("id{n}")
        });
        assert_eq!(
            p.source,
            "- [ ] Debo entregar el informe de revisión de las trincheras #informe #trincheras ^id1\n\
             \x20 Las trincheras están en la carpeta dropbox /workspace/x #trincheras\n\
             - [ ] Debo entregar mañana el informe a la UTalca #informe #utalca due:2026-09-26 ^id2\n\
             - [ ] Debo entregar la próxima semana el LaVet #lavet due:2026-10-02 ^id3\n"
        );
        assert_eq!(p.task_ids, vec![Some("id1".into()), Some("id2".into()), Some("id3".into())]);
        assert_eq!(p.grouped, 1);
        assert_eq!(lines::units(&p.source).len(), 3);

        // Una segunda pasada no duplica nada y reutiliza los identificadores.
        let p2 = plan(&p.source, &analysis(r#"{"unidades": [{"id": "L1", "etiquetas": ["informe"]}], "tareas": [{"texto": "x", "fecha": "", "unidad": "L1"}]}"#), |_| None::<()>, || "nuevo".into());
        assert_eq!(p2.source, p.source);
        assert_eq!(p2.task_ids, vec![Some("id1".into())]);
    }

    #[test]
    fn capture_units_move_with_their_details() {
        let text = "Revisar vigas\nComprar pan\n  integral\nLa viga del eje 3 falla\n\n## Reunión X · 2026-09-24 15:00\n- 15:03 algo\n## fin · 15:30\n";
        let a = analysis(
            r#"{"unidades": [
                {"id": "L1", "espacio": "Obra", "nota": "Vigas"},
                {"id": "L4", "espacio": "Obra", "nota": "Otra", "de": "L1"},
                {"id": "B6", "espacio": "Obra", "nota": "Reunión X", "es_reunion": true, "resumen": "Se habló."}]}"#,
        );
        let p = plan(text, &a, |au| Some(au.nota.clone()), || "id".into());
        assert_eq!(p.source, "Comprar pan\n  integral\n");
        assert_eq!(p.moves[0], ("Vigas".to_string(), "Revisar vigas\n  La viga del eje 3 falla".to_string()));
        assert_eq!(p.moves[1].1, "## Reunión X · 2026-09-24 15:00\n- 15:03 algo\n## fin · 15:30\n### Resumen\nSe habló.\n#reunión");
        assert_eq!(p.placed.get("L4"), Some(&0));
        assert_eq!(clean_tag("#Muro Contención"), "muro-contención");
    }
}
