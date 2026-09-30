//! Las tareas siguen a sus líneas.
//!
//! Una tarea con línea en una nota ("- [ ] texto due:… ^id") vive en dos lugares: esa línea y
//! `tareas.txt` (unidas por el identificador). Normalmente cambian juntas, pero entre dos
//! equipos pueden quedar distintas: la nota se junta línea por línea y `tareas.txt` llega por
//! su lado. Aquí se emparejan, con reglas que nunca pierden trabajo:
//!
//! - la línea se movió a otra nota o espacio → la tarea apunta a donde está ahora;
//! - hecha en un lado y pendiente en el otro → queda hecha en los dos;
//! - la línea tiene fecha → vale la de la línea; si no tiene y la tarea sí, se escribe en la línea;
//! - hay una línea con casilla e identificador sin tarea → se crea la tarea.
//!
//! Las tareas sin línea (las agregadas en Tareas o en Microsoft To Do) no se tocan.

use super::*;
use std::collections::BTreeMap;

/// Se empareja cuando cambian las notas, pero no más seguido que esto.
const EVERY: Duration = Duration::from_secs(3);

enum Fix {
    /// Línea de tareas.txt para una tarea nueva.
    Add(String),
    /// Marcar hecha la tarea.
    Done(String),
    /// Marcar la casilla de la línea (nota, id).
    Check(String, String),
    /// Fecha de la tarea (id, fecha).
    Due(String, String),
    /// Escribir la fecha en la línea (nota, id, fecha).
    LineDue(String, String, String),
    /// La tarea pasa a otra nota (id, nota, espacio).
    Move(String, String, String),
}

impl NotesApp {
    /// Empareja si cambió alguna nota desde la última vez (se llama cada segundo).
    pub(super) fn maybe_reconcile_tasks(&mut self) {
        if self.tasks_gen == Some(self.vault.generation) || self.note.dirty || self.tasks_at.elapsed() < EVERY {
            return;
        }
        self.reconcile_tasks();
    }

    /// Empareja las líneas de tarea de las notas con tareas.txt. Devuelve cuántas cosas corrigió.
    pub(super) fn reconcile_tasks(&mut self) -> usize {
        self.tasks_gen = Some(self.vault.generation);
        self.tasks_at = Instant::now();
        // Dónde está cada línea de tarea: id -> (nota, espacio, día de la nota, línea).
        let mut places: BTreeMap<String, Vec<(String, String, String, vault::TaskLine)>> = BTreeMap::new();
        for n in self.vault.all_notes() {
            if n.task_lines().is_empty() {
                continue;
            }
            // Una copia en conflicto todavía sin juntar no cuenta (repite las líneas de su original).
            if n.path.file_name().is_some_and(|f| crate::conflicts::original_of(&f.to_string_lossy()).is_some()) {
                continue;
            }
            let rel = self.rel(&n.path);
            for tl in n.task_lines() {
                places.entry(tl.id.clone()).or_default().push((rel.clone(), n.workspace.clone(), n.day().to_string(), tl.clone()));
            }
        }
        if places.is_empty() {
            return 0;
        }
        let tasks: HashMap<String, agenda::Task> = self.agenda.tasks().into_iter().filter_map(|t| Some((t.id.clone()?, t))).collect();
        let mut fixes = Vec::new();
        for (id, found) in &places {
            let task = tasks.get(id);
            // Si la misma línea está en dos notas, se prefiere la nota a la que ya apunta la tarea.
            let (rel, ws, day, line) = found.iter().find(|p| task.is_some_and(|t| t.note.as_deref() == Some(p.0.as_str()))).unwrap_or(&found[0]);
            let Some(t) = task else {
                let new = agenda::format_task(day, &line.text, ws, line.due.as_deref(), rel, Some(id));
                fixes.push(Fix::Add(if line.done { format!("x {day} {new}") } else { new }));
                continue;
            };
            if t.note.as_deref() != Some(rel.as_str()) || t.project != *ws {
                fixes.push(Fix::Move(id.clone(), rel.clone(), ws.clone()));
            }
            match (line.done, t.done) {
                (true, false) => fixes.push(Fix::Done(id.clone())),
                (false, true) => fixes.push(Fix::Check(rel.clone(), id.clone())),
                _ => {}
            }
            let task_due = t.due.clone().filter(|d| agenda::is_date(d));
            match (&line.due, task_due) {
                (Some(d), other) if other.as_ref() != Some(d) => fixes.push(Fix::Due(id.clone(), d.clone())),
                (None, Some(d)) => fixes.push(Fix::LineDue(rel.clone(), id.clone(), d)),
                _ => {}
            }
        }
        let n = fixes.len();
        let today = today();
        for f in fixes {
            let r = match f {
                Fix::Add(line) => self.agenda.add_task(line),
                Fix::Done(id) => self.agenda.set_done_by_id(&id, true, &today).map(|_| ()),
                Fix::Due(id, due) => self.agenda.set_due_by_id(&id, &due).map(|_| ()),
                Fix::Move(id, rel, ws) => self.agenda.retarget(None, std::slice::from_ref(&id), &rel, &ws),
                Fix::Check(rel, id) => {
                    self.sync_task_line(&rel, &id, true);
                    Ok(())
                }
                Fix::LineDue(rel, id, due) => {
                    self.edit_task_line(&rel, &id, |l| Some(lines::set_meta(l, Some(&due), None)).filter(|new| new != l));
                    Ok(())
                }
            };
            if let Err(e) = r {
                self.msg(format!("No se pudo actualizar tareas.txt: {e}"));
            }
        }
        if n > 0 {
            self.gcal_dirty = true;
            // Las correcciones a las notas ya quedaron emparejadas: no hace falta otra vuelta.
            self.tasks_gen = Some(self.vault.generation);
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Las tareas siguen a sus líneas: se mueven con ellas, lo hecho en un lado queda hecho en
    /// los dos, la fecha de la línea manda y una línea sin tarea la recupera.
    #[test]
    fn tasks_follow_their_lines() {
        let dir = std::env::temp_dir().join(format!("nodex-tareas-lineas-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("General")).unwrap();
        fs::create_dir_all(dir.join("Obra")).unwrap();
        let muro = dir.join("Obra").join("Muro.md");
        fs::write(
            &muro,
            "- [ ] Enviar planos #planos due:2026-10-05 ^pl001\n- [x] Pedir acero ^ac002\n- [ ] Cubicar la losa ^lo003\n- [ ] Llamar a Juan ^ju004\nUna línea normal\n",
        )
        .unwrap();
        fs::write(
            dir.join("tareas.txt"),
            "2026-09-28 Enviar planos +General due:2026-10-02 nota:General/2026-09-28 id:pl001\n\
             2026-09-28 Pedir acero +Obra nota:Obra/Muro id:ac002\n\
             x 2026-09-29 2026-09-28 Llamar a Juan +Obra due:2026-10-01 nota:Obra/Muro id:ju004\n\
             2026-09-28 Tarea suelta, sin línea +General id:su005\n",
        )
        .unwrap();
        // Una copia en conflicto sin juntar no debe llevarse las tareas.
        fs::write(dir.join("General").join("Muro (copia en conflicto de JP 2026-09-30).md"), "- [ ] Enviar planos ^pl001\n").unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        assert!(app.reconcile_tasks() > 0);
        let t = |id: &str| app.agenda.tasks().into_iter().find(|t| t.id.as_deref() == Some(id)).unwrap();
        // La línea se movió a Obra/Muro y cambió de fecha: la tarea la sigue.
        let planos = t("pl001");
        assert_eq!((planos.note.as_deref(), planos.project.as_str(), planos.due.as_deref()), (Some("Obra/Muro"), "Obra", Some("2026-10-05")));
        // Hecha en la nota → hecha en Tareas. Hecha en Tareas → casilla marcada en la nota.
        assert!(t("ac002").done);
        let text = fs::read_to_string(&muro).unwrap();
        assert!(text.contains("- [x] Llamar a Juan"), "{text}");
        // La fecha que solo tenía la tarea queda escrita en la línea.
        assert!(text.contains("Llamar a Juan due:2026-10-01 ^ju004"), "{text}");
        // Una línea con casilla sin tarea la recupera; una tarea sin línea no se toca.
        let losa = t("lo003");
        assert_eq!((losa.text.as_str(), losa.note.as_deref(), losa.done), ("Cubicar la losa", Some("Obra/Muro"), false));
        assert_eq!(t("su005").text, "Tarea suelta, sin línea");
        assert_eq!(app.agenda.tasks().len(), 5);
        // Ya emparejadas: otra vuelta no cambia nada.
        assert_eq!(app.reconcile_tasks(), 0);
        let _ = fs::remove_dir_all(&dir);
    }
}
