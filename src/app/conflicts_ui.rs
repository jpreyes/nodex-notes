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
/// Datos internos (`.nodex/`) cuyas copias en conflicto se saben juntar.
const INTERNAL: [&str; 8] =
    ["dudas.json", "espacios.json", "actividad.json", "analizadas.txt", "correos-anotados.txt", "diario-aparte.txt", "todo.json", "google.json"];

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
            // Los datos internos (.nodex/) también.
            for e in fs::read_dir(self.vault.root.join(".nodex")).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if conflicts::original_of(&name).is_some_and(|o| INTERNAL.contains(&o.as_str())) {
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
        if copy.parent() == Some(self.vault.root.join(".nodex").as_path()) {
            return self.merge_internal_copy(copy, &main, &original);
        }
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
        self.undo = Some(Undo { files, renamed: None, agenda: snapshot, at: Instant::now(), moved: vec![(copy.to_path_buf(), trashed)], created_dir: None, apart: Vec::new(), relinks: Vec::new() });
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

impl NotesApp {
    /// Copia en conflicto de un archivo de `.nodex/`: se suma lo que falta y la copia va a la papelera.
    fn merge_internal_copy(&mut self, copy: &Path, main: &Path, original: &str) -> bool {
        let Ok(text) = vault::read_text(copy) else { return true };
        let root = self.vault.root.clone();
        match original {
            "dudas.json" => {
                if let Ok(other) = serde_json::from_str::<doubts::Store>(&text) {
                    self.doubts.absorb(&other);
                    let _ = self.doubts.save(&root);
                }
            }
            "espacios.json" => {
                if let Ok(other) = serde_json::from_str::<crate::spaces::Ideas>(&text) {
                    self.ideas.absorb(&other);
                    let _ = self.ideas.save(&root);
                }
            }
            "actividad.json" => {
                if let Ok(other) = serde_json::from_str::<crate::activity::Log>(&text) {
                    self.activity.absorb(&other);
                    let _ = self.activity.save(&root);
                }
            }
            "analizadas.txt" => {
                self.analyzed.extend(text.lines().filter_map(|l| u64::from_str_radix(l.trim(), 16).ok()));
                self.save_analyzed();
            }
            "correos-anotados.txt" | "diario-aparte.txt" => {
                let merged = conflicts::merge_lines(&vault::read_text(main).unwrap_or_default(), &text);
                let _ = fs::write(main, merged);
            }
            // Qué evento de Google o tarea de To Do corresponde a cada cosa: se suman las parejas que falten.
            _ => {
                let (Ok(mut a), Ok(b)) = (
                    serde_json::from_str::<serde_json::Value>(&vault::read_text(main).unwrap_or_else(|_| "{}".into())),
                    serde_json::from_str::<serde_json::Value>(&text),
                ) else {
                    return true;
                };
                if let (Some(a), Some(b)) = (a.as_object_mut(), b.as_object()) {
                    for (k, vb) in b {
                        match (a.get_mut(k).and_then(|v| v.as_object_mut()), vb.as_object()) {
                            (Some(ma), Some(mb)) => {
                                for (kk, vv) in mb {
                                    ma.entry(kk.clone()).or_insert_with(|| vv.clone());
                                }
                            }
                            _ => {
                                a.entry(k.clone()).or_insert_with(|| vb.clone());
                            }
                        }
                    }
                }
                let _ = fs::write(main, serde_json::to_string_pretty(&a).unwrap_or_default());
            }
        }
        self.vault.trash(copy).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Una nota que otro equipo está organizando no se toca; al soltarla, sí.
    #[test]
    fn a_note_is_organized_by_one_device() {
        let dir = std::env::temp_dir().join(format!("nodex-un-equipo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("General")).unwrap();
        let path = dir.join("General").join("2026-09-30.md");
        let text = "Entregar el informe el viernes\n";
        fs::write(&path, text).unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.machine = "pc-b".into();
        let analysis = || serde_json::from_str::<Analysis>(r#"{"unidades": [{"id": "L1", "etiquetas": ["informe"]}]}"#).unwrap();
        // El otro equipo (pc-a) la tomó: aquí no se pide ni se aplica.
        assert!(crate::claims::take_note(&dir, "General/2026-09-30", "pc-a", 60));
        assert!(!crate::claims::take_note(&dir, "General/2026-09-30", &app.machine, 60));
        app.apply_analysis(path.clone(), ai::fnv(text), analysis());
        assert_eq!(fs::read_to_string(&path).unwrap(), text, "la organiza el otro equipo");
        // Cuando la suelta, se organiza aquí.
        crate::claims::release_note(&dir, "General/2026-09-30", "pc-a");
        app.apply_analysis(path.clone(), ai::fnv(text), analysis());
        assert!(fs::read_to_string(&path).unwrap().contains("#informe"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Dos equipos con la app abierta sobre la misma carpeta: lo que guarda uno no borra lo
    /// del otro, y lo que uno quita no reaparece.
    #[test]
    fn two_devices_do_not_overwrite_shared_data() {
        let dir = std::env::temp_dir().join(format!("nodex-dos-equipos-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("General")).unwrap();
        fs::write(dir.join("General").join("A.md"), "Entregar el LaVet\nLlamar a Pedro\n").unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut pc1 = NotesApp::new(cfg.clone(), None, egui::Context::default());
        let mut pc2 = NotesApp::new(cfg, None, egui::Context::default());
        let doubt = |id: &str, unit: &str| doubts::Doubt { id: id.into(), note: "General/A".into(), unit: unit.into(), question: "¿?".into(), ..Default::default() };

        // Cada equipo agrega lo suyo; el segundo en guardar no borra lo del primero.
        pc1.doubts.pending.push(doubt("d1", "Entregar el LaVet"));
        pc1.doubts.save(&dir).unwrap();
        pc1.log_ai(Kind::Organizar, "General/A", "en el PC 1".into(), vec![], false);
        pc1.analyzed.insert(1);
        pc1.save_analyzed();
        pc2.doubts.pending.push(doubt("d2", "Llamar a Pedro"));
        pc2.doubts.save(&dir).unwrap();
        pc2.log_ai(Kind::Organizar, "General/A", "en el PC 2".into(), vec![], false);
        pc2.analyzed.insert(2);
        pc2.save_analyzed();
        let ids = |s: &doubts::Store| {
            let mut v: Vec<String> = s.pending.iter().map(|d| d.id.clone()).collect();
            v.sort();
            v
        };
        let on_disk = doubts::Store::load(&dir);
        assert_eq!(ids(&on_disk), ["d1", "d2"]);
        assert_eq!(crate::activity::Log::load(&dir).entries.len(), 2);
        assert_eq!(load_analyzed(&dir), HashSet::from([1, 2]));

        // El PC 1 se entera de lo que hizo el PC 2.
        pc1.sync_stores();
        assert_eq!(pc1.doubts.pending.len(), 2);
        assert_eq!(pc1.activity.entries.len(), 2);
        assert!(pc1.analyzed.contains(&2));

        // El PC 1 responde una pregunta y manda una nota a analizarse de nuevo: en el PC 2
        // desaparecen, y cuando el PC 2 guarda algo suyo no reaparecen.
        pc1.doubts.resolve("d1");
        pc1.doubts.save(&dir).unwrap();
        pc1.analyzed.remove(&1);
        pc1.save_analyzed();
        pc2.sync_stores();
        assert_eq!(ids(&pc2.doubts), ["d2"]);
        assert!(!pc2.analyzed.contains(&1));
        pc2.doubts.pending.push(doubt("d3", "Otra"));
        pc2.doubts.save(&dir).unwrap();
        let on_disk = doubts::Store::load(&dir);
        assert_eq!(ids(&on_disk), ["d2", "d3"]);
        assert!(on_disk.is_resolved("Entregar el LaVet"));

        // Un archivo a medio escribir (no se entiende) no borra nada de lo que hay en memoria.
        fs::write(dir.join(".nodex").join("dudas.json"), "{ \"pending\": [").unwrap();
        pc2.sync_stores();
        assert_eq!(pc2.doubts.pending.len(), 2);

        // Una copia en conflicto de los datos internos se suma y va a la papelera.
        pc2.doubts.save(&dir).unwrap();
        let copy = dir.join(".nodex").join("dudas (copia en conflicto de JP 2026-09-30).json");
        let mut other = doubts::Store::default();
        other.pending.push(doubt("d9", "De la copia"));
        fs::write(&copy, serde_json::to_string(&other).unwrap()).unwrap();
        pc2.resolve_conflicts(true);
        assert!(!copy.exists());
        assert!(pc2.doubts.pending.iter().any(|d| d.id == "d9"));
        let _ = fs::remove_dir_all(&dir);
    }

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
