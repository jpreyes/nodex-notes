//! Un solo «Hoy»: las notas del día van en `Diario/`, una por día y fuera de los espacios; la IA
//! reparte lo que tienen a cada espacio.
//!
//! Las notas del día que quedaron dentro de los espacios (las de antes, o las que escribe otro
//! equipo con una versión anterior) se juntan solas en la nota del Diario de su día: su texto va
//! al final, nada se pierde, y el archivo original queda en la papelera. Se puede deshacer; si se
//! deshace, esas notas quedan aparte y no se vuelven a juntar (`.nodex/diario-aparte.txt`).

use super::*;
use std::collections::BTreeMap;

/// Se revisa cuando cambian las notas, pero no más seguido que esto.
const EVERY: Duration = Duration::from_secs(5);
/// Notas del día que no se juntan (se deshizo la vez que se juntaron), una ruta por línea.
const APART_FILE: &str = "diario-aparte.txt";

/// Agrega al final de `text` lo de otra nota del mismo día, separado por una línea en blanco.
/// Si todas sus líneas ya están (se juntó antes, en este u otro equipo), no agrega nada.
fn append_day(text: &str, add: &str) -> String {
    let have: HashSet<&str> = text.lines().map(str::trim_end).collect();
    if add.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).all(|l| have.contains(l)) {
        return text.to_string();
    }
    let (a, b) = (text.trim_end(), add.trim_start_matches(['\r', '\n']).trim_end());
    if a.is_empty() { format!("{b}\n") } else { format!("{a}\n\n{b}\n") }
}

impl NotesApp {
    /// Junta si cambió alguna nota desde la última vez (se llama cada segundo).
    pub(super) fn maybe_merge_days(&mut self) {
        if self.days_gen == Some(self.vault.generation) || self.days_at.elapsed() < EVERY {
            return;
        }
        self.merge_days();
    }

    fn apart_file(&self) -> PathBuf {
        self.vault.root.join(".nodex").join(APART_FILE)
    }

    /// Las notas del día que se dejaron aparte.
    fn apart(&self) -> HashSet<String> {
        vault::read_text(&self.apart_file()).map(|t| t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()).unwrap_or_default()
    }

    /// Deja estas notas del día aparte: no se vuelven a juntar (se llama al deshacer).
    pub(super) fn keep_apart(&mut self, rels: &[String]) {
        let mut all: Vec<String> = self.apart().into_iter().chain(rels.iter().cloned()).collect();
        all.sort();
        all.dedup();
        let file = self.apart_file();
        if let Some(dir) = file.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(file, all.join("\n") + "\n");
    }

    /// Junta en el Diario las notas del día que están en los espacios. Devuelve cuántas juntó.
    pub(super) fn merge_days(&mut self) -> usize {
        self.days_gen = Some(self.vault.generation);
        self.days_at = Instant::now();
        let apart = self.apart();
        let meeting = self.meeting.as_ref().map(|m| m.path.clone());
        let mut by_day: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
        for n in self.vault.all_notes() {
            if n.workspace == vault::DIARY || !agenda::is_date(&n.title) {
                continue;
            }
            // La que se está escribiendo (o una reunión en curso) se junta después.
            if (n.path == self.note.path && self.note.dirty) || meeting.as_ref() == Some(&n.path) {
                continue;
            }
            if apart.contains(&self.rel(&n.path)) {
                continue;
            }
            by_day.entry(n.title.clone()).or_default().push(n.path.clone());
        }
        if by_day.is_empty() {
            return 0;
        }
        let root = self.vault.root.clone();
        let snapshot = self.agenda.snapshot();
        let (mut files, mut moved, mut merged_rels) = (Vec::new(), Vec::new(), Vec::new());
        let mut spaces: Vec<String> = Vec::new();
        let mut details = Vec::new();
        let mut first_target = String::new();
        for (day, mut strays) in by_day {
            strays.sort();
            let target = self.vault.diary_path(&day);
            let target_rel = self.rel(&target);
            let open = target == self.note.path;
            if open && self.note.dirty {
                continue;
            }
            // Un solo equipo junta cada día a la vez.
            if !crate::claims::take_note(&root, &target_rel, &self.machine, CLAIM_TTL) {
                continue;
            }
            let before = if open && self.note.disk_mtime.is_none() { None } else { vault::read_text(&target).ok() };
            let mut text = before.clone().unwrap_or_default();
            // Si lo que se junta ya estaba organizado, el resultado también.
            let mut organized = before.as_ref().is_none_or(|b| b.trim().is_empty() || self.analyzed.contains(&ai::fnv(b)));
            let mut taken = Vec::new();
            for p in &strays {
                let Ok(t) = vault::read_text(p) else { continue };
                organized &= t.trim().is_empty() || self.analyzed.contains(&ai::fnv(&t));
                text = append_day(&text, &t);
                taken.push(p.clone());
            }
            if taken.is_empty() {
                crate::claims::release_note(&root, &target_rel, &self.machine);
                continue;
            }
            if before.as_deref() != Some(text.as_str()) {
                if let Some(dir) = target.parent() {
                    let _ = fs::create_dir_all(dir);
                }
                if let Err(e) = fs::write(&target, &text) {
                    self.msg(format!("No se pudo juntar la nota del {}: {e}", long_date(&day)));
                    crate::claims::release_note(&root, &target_rel, &self.machine);
                    continue;
                }
                files.push((target.clone(), before));
            }
            if organized {
                self.analyzed.insert(ai::fnv(&text));
            }
            if let Some(m) = vault::modified(&target) {
                self.vault.upsert(target.clone(), text, m);
            }
            let mut from = Vec::new();
            for p in taken {
                let old_rel = self.rel(&p);
                let Ok(dest) = self.vault.trash(&p) else { continue };
                moved.push((p.clone(), dest));
                // Lo que apuntaba a la nota del espacio ahora apunta a la del Diario.
                let _ = self.agenda.retarget(Some(&old_rel), &[], &target_rel, "");
                for d in &mut self.doubts.pending {
                    if d.note == old_rel {
                        d.note = target_rel.clone();
                    }
                }
                for x in self.ideas.ideas.iter_mut().flat_map(|i| i.refs.iter_mut()) {
                    if x.note == old_rel {
                        x.note = target_rel.clone();
                    }
                }
                for t in &mut self.tabs.list {
                    if *t == tabs::Tab::Note(p.clone()) {
                        *t = tabs::Tab::Note(target.clone());
                    }
                }
                if self.touched.remove(&p) {
                    self.touched.insert(target.clone());
                }
                if self.note.path == p {
                    self.note = OpenNote::load(target.clone());
                }
                let ws = workspace_of(&p).unwrap_or_default();
                if !spaces.contains(&ws) {
                    spaces.push(ws.clone());
                }
                from.push(ws);
                merged_rels.push(old_rel);
            }
            if open && self.note.path == target {
                self.note = OpenNote::load(target.clone());
            }
            crate::claims::release_note(&root, &target_rel, &self.machine);
            if from.is_empty() {
                continue;
            }
            if first_target.is_empty() {
                first_target = target_rel.clone();
            }
            details.push(format!("{}: {}", day_heading(&day).unwrap_or(day), from.join(", ")));
        }
        let n = merged_rels.len();
        if n == 0 {
            return 0;
        }
        let _ = self.doubts.save(&root);
        let _ = self.ideas.save(&root);
        self.save_analyzed();
        self.gcal_dirty = true;
        self.sync_tab();
        self.vault.scan();
        self.days_gen = Some(self.vault.generation);
        self.undo = Some(Undo { files, renamed: None, agenda: snapshot, at: Instant::now(), moved, created_dir: None, apart: merged_rels, keep_tasks: Vec::new(), relinks: Vec::new() });
        let what = format!(
            "Juntó {} de {} en el Diario: ahora hay un solo «Hoy»",
            if n == 1 { "1 nota del día".to_string() } else { format!("{n} notas del día") },
            if spaces.len() == 1 { format!("«{}»", spaces[0]) } else { format!("{} espacios", spaces.len()) }
        );
        self.msg(what.clone());
        self.log_ai(crate::activity::Kind::Sincronizar, &first_target, what, details, true);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_what_is_new_only() {
        assert_eq!(append_day("", "Comprar pan\n"), "Comprar pan\n");
        assert_eq!(append_day("Llamar a Juan\n", "Comprar pan\n"), "Llamar a Juan\n\nComprar pan\n");
        // Ya estaba (se juntó antes en otro equipo): no se repite.
        assert_eq!(append_day("Llamar a Juan\n\nComprar pan\n", "Comprar pan\n"), "Llamar a Juan\n\nComprar pan\n");
    }

    /// Las notas del día de cada espacio se juntan en una sola, con sus tareas; Deshacer las
    /// devuelve y quedan aparte.
    #[test]
    fn daily_notes_of_each_space_become_one() {
        let dir = std::env::temp_dir().join(format!("nodex-un-hoy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("General")).unwrap();
        fs::create_dir_all(dir.join("Obra")).unwrap();
        fs::create_dir_all(dir.join("Diario")).unwrap();
        let (general, obra, diario) = (
            dir.join("General").join("2026-09-28.md"),
            dir.join("Obra").join("2026-09-28.md"),
            dir.join("Diario").join("2026-09-28.md"),
        );
        fs::write(&general, "Comprar pan\n").unwrap();
        fs::write(&obra, "- [ ] Enviar planos ^pl001\nLlamar a Juan\n").unwrap();
        fs::write(&diario, "Algo del Diario\n").unwrap();
        fs::write(dir.join("Obra").join("Muro.md"), "Una nota con título\n").unwrap();
        fs::write(dir.join("tareas.txt"), "2026-09-28 Enviar planos +Obra nota:Obra/2026-09-28 id:pl001\n").unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        assert!(!app.vault.workspaces.contains(&"Diario".to_string()), "el Diario no es un espacio");
        app.open(obra.clone(), None);
        assert_eq!(app.ws, "Obra");

        assert_eq!(app.merge_days(), 2);
        assert_eq!(fs::read_to_string(&diario).unwrap(), "Algo del Diario\n\nComprar pan\n\n- [ ] Enviar planos ^pl001\nLlamar a Juan\n");
        assert!(!general.exists() && !obra.exists() && dir.join("Obra").join("Muro.md").exists());
        // La nota abierta pasa a ser la del Diario; el espacio no cambia.
        assert_eq!(app.note.path, diario);
        assert_eq!(app.ws, "Obra");
        // La tarea apunta a la nota del Diario y conserva su espacio.
        let t = &app.agenda.tasks()[0];
        assert_eq!((t.note.as_deref(), t.project.as_str()), (Some("Diario/2026-09-28"), "Obra"));
        assert_eq!(app.reconcile_tasks(), 0, "nada que corregir: la tarea ya sigue a su línea");
        // Nada más que juntar.
        assert_eq!(app.merge_days(), 0);

        // Deshacer: vuelven las dos notas y el Diario queda como estaba; ya no se juntan.
        app.apply(Action::Undo);
        assert_eq!(fs::read_to_string(&general).unwrap(), "Comprar pan\n");
        assert!(obra.exists());
        assert_eq!(fs::read_to_string(&diario).unwrap(), "Algo del Diario\n");
        assert_eq!(app.agenda.tasks()[0].note.as_deref(), Some("Obra/2026-09-28"));
        app.vault.scan();
        assert_eq!(app.merge_days(), 0, "lo deshecho queda aparte");
        let _ = fs::remove_dir_all(&dir);
    }
}

