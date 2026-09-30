//! Juntar solas las «copias en conflicto» que deja Dropbox (ver `conflicts.rs`).
//!
//! Se espera un rato desde que aparece la copia (Dropbox todavía puede estar moviendo
//! archivos), se junta con el archivo original sin perder ninguna línea, la copia va a la
//! papelera y queda en «Lo que hizo», con Deshacer.

use super::*;
use crate::activity::Kind;
use crate::conflicts;

/// Tiempo que se espera desde que se ve una copia en conflicto antes de juntarla.
const SETTLE: Duration = Duration::from_secs(45);
/// Si el archivo original no aparece en este tiempo, la copia pasa a ser el original.
const ORPHAN: Duration = Duration::from_secs(600);
/// Cada cuánto se buscan copias aunque no haya cambiado ninguna nota (las de tareas.txt).
const LOOK_EVERY: Duration = Duration::from_secs(30);

impl NotesApp {
    /// Busca copias en conflicto y junta las que ya llevan un rato. Se llama cada segundo;
    /// `force` = no esperar (para las pruebas).
    pub(super) fn resolve_conflicts(&mut self, force: bool) {
        // Buscar: cuando cambió alguna nota, o cada tanto.
        if force || self.conflicts_gen != self.vault.generation || self.conflicts_at.elapsed() >= LOOK_EVERY {
            self.conflicts_gen = self.vault.generation;
            self.conflicts_at = Instant::now();
            let mut found: Vec<PathBuf> = self
                .vault
                .all_notes()
                .iter()
                .filter(|n| n.title.contains('('))
                .filter(|n| n.path.file_name().is_some_and(|f| conflicts::original_of(&f.to_string_lossy()).is_some()))
                .map(|n| n.path.clone())
                .collect();
            let lists = [agenda::TASKS_FILE, agenda::AGENDA_FILE, doubts::LEARNED_FILE];
            for e in fs::read_dir(&self.vault.root).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if conflicts::original_of(&name).is_some_and(|o| lists.contains(&o.as_str())) {
                    found.push(e.path());
                }
            }
            self.conflict_seen.retain(|p, _| found.contains(p));
            for p in found {
                self.conflict_seen.entry(p).or_insert_with(Instant::now);
            }
        }
        let ready: Vec<(PathBuf, Duration)> =
            self.conflict_seen.iter().map(|(p, t)| (p.clone(), t.elapsed())).filter(|(_, age)| force || *age >= SETTLE).collect();
        for (copy, age) in ready {
            if self.merge_conflict(&copy, force || age >= ORPHAN) {
                self.conflict_seen.remove(&copy);
            }
        }
    }

    /// Junta una copia en conflicto con su original. Devuelve si quedó resuelta.
    fn merge_conflict(&mut self, copy: &Path, adopt_orphan: bool) -> bool {
        let Some(name) = copy.file_name().map(|f| f.to_string_lossy().into_owned()) else { return true };
        let Some(original) = conflicts::original_of(&name) else { return true };
        let main = copy.with_file_name(&original);
        let is_note = copy.parent() != Some(self.vault.root.as_path());
        if main == self.note.path || copy == self.note.path {
            self.save();
        }
        let Ok(copy_text) = vault::read_text(copy) else { return true }; // ya no está
        let snapshot = self.agenda.snapshot();

        // Sin original (Dropbox aún no lo trae, o se borró): se espera; pasado un rato, la copia
        // pasa a ser el original.
        let Ok(before) = vault::read_text(&main) else {
            if !adopt_orphan || fs::rename(copy, &main).is_err() {
                return false;
            }
            self.vault.scan();
            if self.note.path == copy {
                self.note = OpenNote::load(main.clone());
            }
            self.msg(format!("«{}» era una copia en conflicto sin original: quedó con su nombre", vault::stem(&main)));
            return true;
        };

        let merged = match original.as_str() {
            _ if is_note => crate::merge::union(&before, &copy_text),
            agenda::TASKS_FILE => conflicts::merge_tasks(&before, &copy_text),
            _ => conflicts::merge_lines(&before, &copy_text),
        };
        if merged != before && fs::write(&main, &merged).is_err() {
            return false;
        }
        let Ok(trashed) = self.vault.trash(copy) else { return false };
        let added = merged.lines().count().saturating_sub(before.lines().count());

        let mut files = Vec::new();
        if is_note {
            files.push((main.clone(), Some(before)));
            self.vault.scan();
            if self.note.path == main || self.note.path == copy {
                self.note = OpenNote::load(main.clone());
            }
            self.touched.insert(main.clone());
        } else if original == doubts::LEARNED_FILE {
            files.push((main.clone(), Some(before)));
        } else {
            let _ = self.agenda.write_ics();
            self.gcal_dirty = true;
        }
        self.undo = Some(Undo { files, renamed: None, agenda: snapshot, at: Instant::now(), moved: vec![(copy.to_path_buf(), trashed)], created_dir: None });
        let what = if is_note { format!("«{}»", display_title(&vault::stem(&main))) } else { original.clone() };
        let detail = match added {
            0 => "No había nada nuevo en la copia".to_string(),
            1 => "1 línea de la copia se agregó".to_string(),
            n => format!("{n} líneas de la copia se agregaron"),
        };
        let note = if is_note { self.rel(&main) } else { String::new() };
        self.log_ai(
            Kind::Sincronizar,
            &note,
            format!("Se juntó una copia en conflicto de {what}"),
            vec![detail, format!("La copia quedó en la papelera: {name}")],
            true,
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Las copias en conflicto de Dropbox se juntan con su original sin perder líneas, la copia
    /// va a la papelera y se puede deshacer.
    #[test]
    fn dropbox_conflicted_copies_are_merged() {
        let dir = std::env::temp_dir().join(format!("nodex-copias-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Obra")).unwrap();
        let main = dir.join("Obra").join("Muro.md");
        let copy = dir.join("Obra").join("Muro (copia en conflicto de JP 2026-09-30).md");
        fs::write(&main, "Revisar planos\nPedir acero\nLlamar a Juan\n").unwrap();
        fs::write(&copy, "Revisar planos\nLlamar a Juan\nCubicar la losa\n").unwrap();
        fs::write(dir.join("tareas.txt"), "2026-09-28 Enviar planos +Obra id:pl001\n").unwrap();
        let tasks_copy = dir.join("tareas (JP's conflicted copy 2026-09-30).txt");
        fs::write(&tasks_copy, "x 2026-09-30 2026-09-28 Enviar planos +Obra id:pl001\n2026-09-30 Otra +Obra id:ot002\n").unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());

        // Recién vistas: todavía no se tocan (Dropbox puede estar moviendo archivos).
        app.resolve_conflicts(false);
        assert!(copy.exists() && tasks_copy.exists());
        assert_eq!(app.conflict_seen.len(), 2);

        app.resolve_conflicts(true);
        assert_eq!(fs::read_to_string(&main).unwrap(), "Revisar planos\nPedir acero\nLlamar a Juan\nCubicar la losa\n");
        assert!(!copy.exists() && !tasks_copy.exists(), "las copias van a la papelera");
        assert_eq!(fs::read_dir(dir.join(".papelera")).unwrap().count(), 2);
        let tasks = app.agenda.tasks();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().any(|t| t.id.as_deref() == Some("pl001") && t.done), "lo hecho en un lado queda hecho");
        assert!(app.conflict_seen.is_empty());
        assert!(app.vault.all_notes().iter().all(|n| !n.title.contains("conflicto")));
        let last = app.activity.entries.last().unwrap();
        assert_eq!(last.kind, Kind::Sincronizar);

        // Deshacer devuelve la última (la nota o las tareas, según el orden en que se juntaron).
        app.undo_ai();
        assert!(copy.exists() || tasks_copy.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
