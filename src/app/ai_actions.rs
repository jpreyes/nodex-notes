//! La IA hace lo que le pides en la conversación: marcar una tarea hecha, crear una tarea o un
//! evento, anotar algo en una nota, anotar un seguimiento o cambiar la fecha de una tarea.
//!
//! La respuesta trae las acciones en un bloque aparte (`ask::split_actions`); aquí se hacen al
//! tiro, quedan debajo de la respuesta («Hecho: …») y en «Lo que hizo», con Deshacer (vuelven las
//! notas y la agenda como estaban).

use super::ask_view::{Source, TaskKey};
use super::*;
use crate::ask::Accion;

impl NotesApp {
    /// La tarea de una clave («t4»), como está ahora.
    fn task_of(&self, key: &str, tasks: &HashMap<String, TaskKey>) -> Option<agenda::Task> {
        let k = tasks.get(key.trim())?;
        let all = self.agenda.tasks();
        match &k.id {
            Some(id) => all.into_iter().find(|t| t.id.as_deref() == Some(id.as_str())),
            None => all.into_iter().find(|t| t.text.eq_ignore_ascii_case(&k.text)),
        }
    }

    /// Escribe una nota (la abierta, en memoria) y recuerda cómo estaba, para deshacer.
    fn write_for_ai(&mut self, path: &Path, text: String, before: &mut Vec<(PathBuf, Option<String>)>) {
        let old = if *path == self.note.path { Some(self.note.text.clone()).filter(|t| !t.is_empty() || self.note.disk_mtime.is_some()) } else { vault::read_text(path).ok() };
        if !before.iter().any(|(p, _)| p == path) {
            before.push((path.to_path_buf(), old));
        }
        if *path == self.note.path {
            self.note.text = text;
            self.note.dirty = true;
            self.note.last_edit = Instant::now();
            self.save();
        } else {
            if let Some(d) = path.parent() {
                let _ = fs::create_dir_all(d);
            }
            if fs::write(path, &text).is_ok() {
                let m = vault::modified(path).unwrap_or_else(SystemTime::now);
                self.vault.upsert(path.to_path_buf(), text, m);
            }
        }
    }

    /// El texto de una nota (la abierta, como está en pantalla).
    fn text_of(&self, path: &Path) -> String {
        if *path == self.note.path { self.note.text.clone() } else { vault::read_text(path).unwrap_or_default() }
    }

    /// Hace las acciones; devuelve qué hizo (en palabras) y la entrada de «Lo que hizo».
    pub(super) fn apply_ai_actions(&mut self, acts: &[Accion], tasks: &HashMap<String, TaskKey>, sources: &HashMap<String, Source>) -> (Vec<String>, Option<String>) {
        if acts.is_empty() {
            return (Vec::new(), None);
        }
        self.save();
        let snapshot = self.agenda.snapshot();
        let mut files: Vec<(PathBuf, Option<String>)> = Vec::new();
        let mut done: Vec<String> = Vec::new();
        let ws_of = |app: &NotesApp, e: &str| app.vault.workspaces.iter().find(|w| w.eq_ignore_ascii_case(e.trim())).cloned().unwrap_or_else(|| app.home_ws());
        let today_s = today();
        for a in acts {
            match a.tipo.trim() {
                "hecha" => {
                    let Some(t) = self.task_of(&a.tarea, tasks) else { continue };
                    if t.done {
                        done.push(format!("«{}» ya estaba hecha", agenda::display_text(&t.text)));
                        continue;
                    }
                    match (&t.id, &t.note) {
                        (Some(id), Some(note)) => {
                            let path = self.vault.root.join(format!("{note}.md"));
                            let old = self.text_of(&path);
                            if !files.iter().any(|(p, _)| *p == path) {
                                files.push((path, Some(old)));
                            }
                            let _ = self.agenda.set_done_by_id(id, true, &today_s);
                            self.sync_task_line(note, id, true);
                        }
                        _ => {
                            let _ = self.agenda.toggle_task(&t.raw, &today_s);
                        }
                    }
                    done.push(format!("Marqué hecha «{}»", agenda::display_text(&t.text)));
                }
                "tarea" if !a.texto.trim().is_empty() => {
                    let due = Some(a.fecha.trim()).filter(|d| agenda::is_date(d));
                    let ws = ws_of(self, &a.espacio);
                    if self.agenda.add_task(agenda::format_task(&today_s, a.texto.trim(), &ws, due, "", None)).is_ok() {
                        let when = due.map(|d| format!(" · {}", long_date(d))).unwrap_or_default();
                        done.push(format!("Nueva tarea «{}»{when}", a.texto.trim()));
                    }
                }
                "evento" if !a.titulo.trim().is_empty() && agenda::is_date(a.fecha.trim()) => {
                    let time = Some(a.hora.trim()).filter(|h| agenda::is_time(h));
                    let title = if a.lugar.trim().is_empty() { a.titulo.trim().to_string() } else { format!("{} · {}", a.titulo.trim(), a.lugar.trim()) };
                    let ws = ws_of(self, &a.espacio);
                    if self.agenda.add_lines(&[], &[agenda::format_event(a.fecha.trim(), time, &title, &ws, "")]).is_ok() {
                        let when = time.map_or_else(|| "todo el día".to_string(), |h| format!("a las {h}"));
                        done.push(format!("Evento «{title}» · {}, {when}", long_date(a.fecha.trim())));
                    }
                }
                "anotar" if !a.texto.trim().is_empty() => {
                    let path = match sources.get(a.nota.trim()) {
                        Some(s) if !self.is_bloc_page(&s.path) => s.path.clone(),
                        _ => self.today_path(),
                    };
                    let old = self.text_of(&path);
                    let text = if old.trim().is_empty() { format!("{}\n", a.texto.trim()) } else { format!("{}\n{}\n", old.trim_end(), a.texto.trim()) };
                    self.write_for_ai(&path, text, &mut files);
                    done.push(format!("Anoté en «{}»: {}", display_title(&vault::stem(&path)), a.texto.trim()));
                }
                "seguimiento" if !a.texto.trim().is_empty() => {
                    let Some(t) = self.task_of(&a.tarea, tasks) else { continue };
                    if let (Some(_), Some(note)) = (&t.id, &t.note) {
                        let path = self.vault.root.join(format!("{note}.md"));
                        let old = self.text_of(&path);
                        if !files.iter().any(|(p, _)| *p == path) {
                            files.push((path, Some(old)));
                        }
                    }
                    let target = match (&t.id, &t.note) {
                        (Some(id), Some(note)) => super::tracking::Target::Line { note: self.vault.root.join(format!("{note}.md")), id: Some(id.clone()), text: String::new() },
                        _ => super::tracking::Target::Task(super::tracking::task_key(&t)),
                    };
                    if self.add_follow_up(target, a.texto.trim()) {
                        done.push(format!("Seguimiento en «{}»: {}", agenda::display_text(&t.text), a.texto.trim()));
                    }
                }
                "fecha" if agenda::is_date(a.fecha.trim()) => {
                    let Some(t) = self.task_of(&a.tarea, tasks) else { continue };
                    let Some(id) = t.id.clone() else {
                        done.push(format!("No pude cambiar la fecha de «{}» (cámbiala en Tareas)", agenda::display_text(&t.text)));
                        continue;
                    };
                    let date = a.fecha.trim().to_string();
                    let _ = self.agenda.set_due_by_id(&id, &date);
                    if let Some(note) = &t.note {
                        let path = self.vault.root.join(format!("{note}.md"));
                        let old = self.text_of(&path);
                        if !files.iter().any(|(p, _)| *p == path) {
                            files.push((path, Some(old)));
                        }
                        let (d, i) = (date.clone(), id.clone());
                        self.edit_task_line(note, &id, move |line| Some(lines::set_meta(line, Some(&d), Some(&i))));
                    }
                    done.push(format!("«{}» ahora vence el {}", agenda::display_text(&t.text), long_date(&date)));
                }
                _ => {}
            }
        }
        if done.is_empty() {
            return (done, None);
        }
        self.gcal_dirty = true;
        self.follow_ask = None; // lo que pediste ya dice qué se hizo
        let notes: Vec<String> = files.iter().map(|(p, _)| self.rel(p)).collect();
        self.undo = Some(Undo { files, renamed: None, agenda: snapshot, at: Instant::now(), moved: Vec::new(), created_dir: None, apart: Vec::new(), keep_tasks: Vec::new(), relinks: Vec::new() });
        let what = format!("Hizo lo que pediste: {}", plural(done.len(), "cambio"));
        self.log_ai(crate::activity::Kind::Pedido, notes.first().map_or("", |s| s.as_str()), what, done.clone(), true);
        (done, self.undo_entry.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Con la app: lo que pide la conversación se hace, y Deshacer lo revierte todo.
    #[test]
    fn the_ai_does_what_you_ask() {
        let dir = std::env::temp_dir().join(format!("nodex-ia-hace-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Docencia")).unwrap();
        let congreso = dir.join("Docencia").join("Congreso AICE.md");
        fs::write(&congreso, "- [ ] Subir mi presentación al Drive ^pres1\n- [ ] Pagar inscripción ^pago1\n").unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.agenda.add_task(agenda::format_task("2026-09-30", "Subir mi presentación al Drive", "Docencia", None, "Docencia/Congreso AICE", Some("pres1"))).unwrap();
        app.agenda.add_task(agenda::format_task("2026-09-30", "Pagar inscripción", "Docencia", None, "Docencia/Congreso AICE", Some("pago1"))).unwrap();
        let mut tasks = HashMap::new();
        tasks.insert("t1".to_string(), TaskKey { id: Some("pres1".into()), text: "Subir mi presentación al Drive".into(), note: Some("Docencia/Congreso AICE".into()) });
        tasks.insert("t2".to_string(), TaskKey { id: Some("pago1".into()), text: "Pagar inscripción".into(), note: Some("Docencia/Congreso AICE".into()) });
        let reply = "Listo.\n```acciones\n{\"acciones\": [\
            {\"tipo\": \"hecha\", \"tarea\": \"t1\"},\
            {\"tipo\": \"evento\", \"titulo\": \"18° Congreso AICE\", \"fecha\": \"2026-10-02\", \"lugar\": \"Villarrica\", \"espacio\": \"docencia\"},\
            {\"tipo\": \"anotar\", \"nota\": \"hoy\", \"texto\": \"Llevar el pendrive al congreso\"},\
            {\"tipo\": \"seguimiento\", \"tarea\": \"t2\", \"texto\": \"Pedí la factura\"},\
            {\"tipo\": \"fecha\", \"tarea\": \"t2\", \"fecha\": \"2026-10-05\"}]}\n```";
        let (_, acts) = crate::ask::split_actions(reply);
        let before_agenda = app.agenda.snapshot();
        let (done, entry) = app.apply_ai_actions(&acts, &tasks, &HashMap::new());
        assert_eq!(done.len(), 5, "{done:?}");
        assert!(entry.is_some());
        assert!(app.agenda.tasks().iter().find(|t| t.id.as_deref() == Some("pres1")).unwrap().done);
        let ev = app.agenda.events().into_iter().find(|e| e.title.contains("Congreso")).expect("el evento");
        assert_eq!((ev.date.as_str(), ev.time.clone(), ev.project.as_str()), ("2026-10-02", None, "Docencia"));
        assert_eq!(ev.title, "18° Congreso AICE · Villarrica");
        let note = fs::read_to_string(&congreso).unwrap();
        assert!(note.starts_with("- [x] Subir mi presentación al Drive ^pres1\n"), "{note}");
        assert!(note.contains("- [ ] Pagar inscripción due:2026-10-05 ^pago1\n  ↳ "), "{note}");
        assert!(note.contains(": Pedí la factura"), "{note}");
        let today_note = fs::read_to_string(app.today_path()).unwrap();
        assert!(today_note.contains("Llevar el pendrive al congreso"));
        assert!(app.follow_ask.is_none(), "no pregunta qué se hizo: ya lo dijiste");
        // Deshacer: todo como estaba.
        app.undo_ai();
        assert_eq!(app.agenda.snapshot(), before_agenda);
        assert_eq!(fs::read_to_string(&congreso).unwrap(), "- [ ] Subir mi presentación al Drive ^pres1\n- [ ] Pagar inscripción ^pago1\n");
        let _ = fs::remove_dir_all(&dir);
    }
}
