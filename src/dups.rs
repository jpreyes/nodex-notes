//! Notas duplicadas: la misma cosa anotada dos veces (en la misma nota o en otra).
//!
//! Se comparan las palabras importantes de la primera línea de cada nota de adentro
//! (sin acentos, sin palabras de relleno), sin IA. Lo que se parece lo bastante se
//! pregunta ("¿es lo mismo que…?"); nunca se une solo.

use crate::doubts::{self, Choice, Doubt, Store};
use crate::lines;
use std::collections::{HashMap, HashSet};

/// Parecido mínimo (palabras en común / palabras en total) para preguntar.
const THRESHOLD: f32 = 0.7;
const MAX_PER_NOTE: usize = 3;

const STOP: &[&str] = &[
    "el", "la", "los", "las", "un", "una", "unos", "unas", "de", "del", "al", "a", "en", "y", "o", "que", "por", "para",
    "con", "sin", "se", "su", "sus", "mi", "mis", "lo", "le", "les", "es", "son", "esta", "este", "esto", "hay", "debo",
    "tengo", "hacer", "ya", "muy", "mas", "pero", "como", "sobre", "entre", "hasta", "desde",
];

fn fold(c: char) -> char {
    match c {
        'á' | 'à' | 'ä' => 'a',
        'é' | 'è' | 'ë' => 'e',
        'í' | 'ì' | 'ï' => 'i',
        'ó' | 'ò' | 'ö' => 'o',
        'ú' | 'ù' | 'ü' => 'u',
        c => c,
    }
}

/// Palabras importantes de un texto: minúsculas, sin acentos ni plurales simples.
pub fn words(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .chars()
        .map(fold)
        .collect::<String>()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| (w.chars().count() >= 3 || w.chars().all(|c| c.is_ascii_digit())) && !w.is_empty() && !STOP.contains(w))
        .map(|w| if w.len() > 4 && w.ends_with('s') { w[..w.len() - 1].to_string() } else { w.to_string() })
        .collect()
}

pub fn similarity(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    let shared = a.intersection(b).count();
    if shared < 2 {
        return 0.0;
    }
    shared as f32 / a.union(b).count() as f32
}

/// Identifica un par (sin importar el orden), para "son distintas".
pub fn pair(a: &str, b: &str) -> u64 {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    let (x, y) = if a <= b { (a, b) } else { (b, a) };
    crate::ai::fnv(&format!("{x}|{y}"))
}

/// Una nota de adentro para comparar.
struct Item {
    note: String,
    core: String,
    first: usize,
    words: HashSet<String>,
}

fn items(note: &str, text: &str) -> Vec<Item> {
    let ls: Vec<&str> = text.lines().collect();
    lines::units(text)
        .into_iter()
        .filter(|u| !u.block)
        .filter_map(|u| {
            let core = doubts::core(ls.get(u.first)?);
            let words = words(&core);
            (words.len() >= 2).then(|| Item { note: note.to_string(), core, first: u.first, words })
        })
        .collect()
}

fn title_of(rel: &str) -> String {
    rel.replace('/', " / ")
}

/// Preguntas "¿es lo mismo que…?" para las notas de adentro de `notes` (las recién escritas),
/// comparándolas con todas (`all`: nota relativa y texto). No repite lo pendiente ni lo ya respondido.
pub fn find(notes: &[(String, String)], all: &[(String, String)], store: &Store, today: &str, mut new_id: impl FnMut() -> String) -> Vec<Doubt> {
    let pool: Vec<Item> = all.iter().flat_map(|(n, t)| items(n, t)).collect();
    let mut index: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, it) in pool.iter().enumerate() {
        for w in &it.words {
            index.entry(w.as_str()).or_default().push(i);
        }
    }
    // Pares ya preguntados (pendientes).
    let mut asked: HashSet<u64> = store
        .pending
        .iter()
        .flat_map(|d| d.choices.iter().filter(|c| !c.unir.is_empty()).map(move |c| pair(&d.unit, &c.unir)))
        .collect();
    let mut out = Vec::new();
    for (note, text) in notes {
        let mut per_note = 0;
        for a in items(note, text) {
            if per_note >= MAX_PER_NOTE || store.is_resolved(&a.core) {
                continue;
            }
            // Candidatas: comparten al menos una palabra.
            let mut cands: HashSet<usize> = HashSet::new();
            for w in &a.words {
                cands.extend(index.get(w.as_str()).into_iter().flatten().copied());
            }
            let best = cands
                .into_iter()
                .map(|i| &pool[i])
                // No consigo misma; en la misma nota, solo con una anterior.
                .filter(|b| !(b.note == a.note && b.first >= a.first))
                .map(|b| (similarity(&a.words, &b.words), b))
                .filter(|(s, _)| *s >= THRESHOLD)
                .max_by(|x, y| x.0.total_cmp(&y.0).then(y.1.first.cmp(&x.1.first)));
            let Some((_, b)) = best else { continue };
            let key = pair(&a.core, &b.core);
            if store.not_dups.contains(&key) || !asked.insert(key) {
                continue;
            }
            let same = b.note == a.note;
            let base = Choice { unir_nota: b.note.clone(), unir: b.core.clone(), ..Choice::default() };
            let choices = vec![
                Choice {
                    label: if same { "Unir con la de arriba".into() } else { format!("Unir en «{}»", title_of(&b.note)) },
                    ..base.clone()
                },
                Choice { label: if same { "Unir aquí abajo".into() } else { "Unir aquí".into() }, al_reves: true, ..base.clone() },
                Choice { label: "Son distintas".into(), distintas: true, ..base },
            ];
            let place = if same { "más arriba en esta nota".to_string() } else { format!("en {}", title_of(&b.note)) };
            out.push(Doubt {
                id: new_id(),
                note: a.note.clone(),
                unit: a.core.clone(),
                question: format!("¿Es lo mismo que «{}» ({place})?", b.core),
                choices,
                created: today.to_string(),
            });
            per_note += 1;
        }
    }
    out
}

/// Resultado de unir dos notas de adentro.
#[derive(Debug, PartialEq)]
pub struct Merged {
    /// Texto nuevo de la nota que se queda (si es la misma nota, también el de la otra).
    pub keep: String,
    /// Texto nuevo de la nota de donde se quitó (igual a `keep` si es la misma nota).
    pub drop: String,
    /// Tarea que se borra de tareas.txt (las dos eran tareas).
    pub remove_task: Option<String>,
    /// Tarea que pasa a la nota que se queda.
    pub moved_task: Option<String>,
}

/// Une `drop_unit` (en `drop_text`) a `keep_unit` (en `keep_text`): la que se queda recibe
/// sus etiquetas, sus detalles y, si hace falta, su casilla y su fecha; la otra se quita.
pub fn merge(keep_text: &str, keep_unit: &str, drop_text: &str, drop_unit: &str, same: bool) -> Option<Merged> {
    let k = doubts::find_unit(keep_text, keep_unit)?;
    let d = doubts::find_unit(drop_text, drop_unit)?;
    if same && k.first == d.first {
        return None;
    }
    let kl: Vec<String> = keep_text.lines().map(str::to_string).collect();
    let dl: Vec<String> = drop_text.lines().map(str::to_string).collect();
    let dropped: Vec<String> = dl[d.first..=d.last].to_vec();
    let mut head = kl[k.first].clone();
    let dhead = &dropped[0];

    // Etiquetas que la otra tenía.
    let have: HashSet<String> = kl[k.first..=k.last].iter().flat_map(|l| crate::tags::line_tags(l)).collect();
    let add: Vec<String> = dropped.iter().flat_map(|l| crate::tags::line_tags(l)).filter(|t| !have.contains(t)).collect::<Vec<_>>();
    let mut seen = HashSet::new();
    let add: Vec<String> = add.into_iter().filter(|t| seen.insert(t.clone())).map(|t| format!("#{t}")).collect();
    if !add.is_empty() {
        head = lines::insert_words(&head, &add.join(" "));
    }
    // Tareas: queda una sola, con la fecha que hubiera.
    let (ki, di) = (lines::parse(&head), lines::parse(dhead));
    let (mut remove_task, mut moved_task) = (None, None);
    match (ki.check.is_some(), di.check.is_some()) {
        (true, true) => {
            let due = lines::due_of(dhead).filter(|_| lines::due_of(&head).is_none());
            head = lines::set_meta(&head, due.as_deref(), None);
            remove_task = lines::id_of(dhead);
        }
        (false, true) => {
            let mut h = lines::make_task(&head);
            if di.check == Some(true) {
                h = lines::toggle_check(&h);
            }
            head = lines::set_meta(&h, lines::due_of(dhead).as_deref(), lines::id_of(dhead).as_deref());
            moved_task = lines::id_of(dhead);
        }
        _ => {}
    }
    // Detalles de la otra que no estén ya.
    let have_core: HashSet<String> = kl[k.first..=k.last].iter().map(|l| doubts::core(l)).collect();
    let details: Vec<String> = dropped[1..].iter().filter(|l| !have_core.contains(&doubts::core(l))).cloned().collect();

    let trailing = |t: &str| t.ends_with('\n');
    let join = |ls: Vec<String>, t: &str| {
        let mut s = ls.join("\n");
        if trailing(t) && !s.is_empty() {
            s.push('\n');
        }
        s
    };
    let rebuild = |src: &[String], with_keep: bool, with_drop: bool| -> Vec<String> {
        let mut out = Vec::new();
        for (i, l) in src.iter().enumerate() {
            if with_drop && (d.first..=d.last).contains(&i) {
                continue;
            }
            if with_keep && i == k.first {
                out.push(head.clone());
            } else {
                out.push(l.clone());
            }
            if with_keep && i == k.last {
                out.extend(details.iter().cloned());
            }
        }
        out
    };
    if same {
        let t = join(rebuild(&kl, true, true), keep_text);
        return Some(Merged { keep: t.clone(), drop: t, remove_task, moved_task });
    }
    Some(Merged { keep: join(rebuild(&kl, true, false), keep_text), drop: join(rebuild(&dl, false, true), drop_text), remove_task, moved_task })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similar_lines_are_found_once() {
        let a = words("Debo entregar el informe de revisión de las trincheras");
        let b = words("Entregar informe revision trincheras #trincheras");
        assert!(similarity(&a, &b) >= THRESHOLD, "{a:?} {b:?}");
        assert!(similarity(&a, &words("Comprar pan integral")) < THRESHOLD);

        let all = vec![
            ("Consorcio/Trincheras".to_string(), "Entregar informe revisión trincheras\nRevisar taludes\n".to_string()),
            ("General/2026-09-25".to_string(), "Debo entregar el informe de revisión de las trincheras\nComprar pan\n".to_string()),
        ];
        let mut n = 0;
        let found = find(&all[1..], &all, &Store::default(), "2026-09-25", || {
            n += 1;
            format!("d{n}")
        });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].unit, "Debo entregar el informe de revisión de las trincheras");
        assert_eq!(found[0].choices[0].label, "Unir en «Consorcio / Trincheras»");
        // Ya pendiente, o "son distintas": no se vuelve a preguntar.
        let store = Store { pending: found.clone(), ..Store::default() };
        assert!(find(&all[1..], &all, &store, "x", || "e".into()).is_empty());
        let store = Store { not_dups: vec![pair(&found[0].unit, &found[0].choices[0].unir)], ..Store::default() };
        assert!(find(&all[1..], &all, &store, "x", || "e".into()).is_empty());
    }

    #[test]
    fn merging_keeps_one_with_everything() {
        let keep = "Entregar informe trincheras #informe\nRevisar taludes\n";
        let drop = "- [ ] Debo entregar el informe de las trincheras #trincheras due:2026-09-30 ^k3f9a\n  en Dropbox\nOtra cosa\n";
        let m = merge(keep, "Entregar informe trincheras", drop, "Debo entregar el informe de las trincheras", false).unwrap();
        assert_eq!(m.keep, "- [ ] Entregar informe trincheras #informe #trincheras due:2026-09-30 ^k3f9a\n  en Dropbox\nRevisar taludes\n");
        assert_eq!(m.drop, "Otra cosa\n");
        assert_eq!((m.moved_task.as_deref(), m.remove_task), (Some("k3f9a"), None));

        // En la misma nota, y las dos con tarea: queda una.
        let t = "- [ ] Llamar a Juan por los planos ^aaa11\nOtra\n- [ ] Llamar a Juan planos due:2026-10-01 ^bbb22\n";
        let m = merge(t, "Llamar a Juan por los planos", t, "Llamar a Juan planos", true).unwrap();
        assert_eq!(m.keep, "- [ ] Llamar a Juan por los planos due:2026-10-01 ^aaa11\nOtra\n");
        assert_eq!(m.remove_task.as_deref(), Some("bbb22"));
    }
}
