//! Corregir a mano: mover una nota a otro espacio, cambiar el nombre de un espacio o de una
//! etiqueta. Todo se puede deshacer.
//!
//! Muchas cosas apuntan a una nota por su ruta ("Obra/Muro"): las tareas y eventos, las
//! preguntas de la IA, las sugerencias de espacios, «Lo que hizo», los correos anotados y las
//! pestañas. Al mover o renombrar, `relink` las lleva al lugar nuevo.

use super::*;

/// La ruta relativa `r` con `old` cambiado por `new`, si es esa nota o está dentro de esa carpeta.
pub(super) fn relinked(r: &str, old: &str, new: &str) -> Option<String> {
    if r == old {
        return Some(new.to_string());
    }
    r.strip_prefix(old).filter(|rest| rest.starts_with('/')).map(|rest| format!("{new}{rest}"))
}

impl NotesApp {
    fn relinked_path(&self, p: &Path, old: &str, new: &str) -> Option<PathBuf> {
        relinked(&self.rel(p), old, new).map(|r| self.vault.root.join(format!("{r}.md")))
    }

    /// Todo lo que apunta a `old` (una nota, o un espacio y sus notas) pasa a apuntar a `new`.
    /// Las tareas y eventos no: cada caso los cambia a su manera.
    pub(super) fn relink(&mut self, old: &str, new: &str) {
        let fix = |s: &mut String| {
            if let Some(r) = relinked(s, old, new) {
                *s = r;
            }
        };
        for d in &mut self.doubts.pending {
            fix(&mut d.note);
            for c in &mut d.choices {
                fix(&mut c.unir_nota);
                if c.espacio == old {
                    c.espacio = new.to_string();
                }
            }
        }
        for x in self.ideas.ideas.iter_mut().flat_map(|i| i.refs.iter_mut()) {
            fix(&mut x.note);
        }
        for e in &mut self.activity.entries {
            fix(&mut e.note);
        }
        for m in &mut self.mail.store.mails {
            fix(&mut m.noted);
            if m.workspace == old {
                m.workspace = new.to_string();
            }
        }
        // Los enlaces `[[Espacio/Nota]]` (los que solo dicen el nombre siguen sirviendo).
        self.update_links(old, new, false);
        let root = self.vault.root.clone();
        let _ = self.doubts.save(&root);
        let _ = self.ideas.save(&root);
        let _ = self.activity.save(&root);
        self.mail.store.save();

        let tabs: Vec<tabs::Tab> = self
            .tabs
            .list
            .iter()
            .map(|t| match t {
                tabs::Tab::Note(p) => tabs::Tab::Note(self.relinked_path(p, old, new).unwrap_or_else(|| p.clone())),
                v => v.clone(),
            })
            .collect();
        self.tabs.list = tabs;
        if let Some(p) = self.relinked_path(&self.note.path, old, new) {
            self.note.path = p;
            self.note.title = vault::stem(&self.note.path);
        }
        if let Some(p) = self.meeting.as_ref().and_then(|m| self.relinked_path(&m.path, old, new)) {
            if let Some(m) = &mut self.meeting {
                m.path = p;
            }
        }
        let touched: HashSet<PathBuf> = self.touched.iter().map(|p| self.relinked_path(p, old, new).unwrap_or_else(|| p.clone())).collect();
        self.touched = touched;
        if self.ws == old {
            self.ws = new.to_string();
        }
        self.save_estado();
    }

    /// Mueve una nota a otro espacio, con sus tareas y eventos.
    pub(super) fn move_note(&mut self, path: PathBuf, ws: String) {
        if vault::in_diary(&path) || workspace_of(&path).as_deref() == Some(ws.as_str()) || !self.vault.workspaces.contains(&ws) {
            return;
        }
        if path == self.note.path {
            self.save();
        }
        let title = vault::stem(&path);
        let target = self.vault.unique_path(&ws, &title);
        let (old_rel, new_rel) = (self.rel(&path), self.rel(&target));
        if !path.exists() {
            // Una nota nueva que aún no se guarda: solo cambia dónde se guardará.
            if self.note.path == path {
                self.relink(&old_rel, &new_rel);
                self.ws = ws;
            }
            return;
        }
        let snapshot = self.agenda.snapshot();
        if let Some(dir) = target.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Err(e) = fs::rename(&path, &target) {
            self.msg(format!("No se pudo mover la nota: {e}"));
            return;
        }
        if let Err(e) = self.agenda.retarget(Some(&old_rel), &[], &new_rel, &ws) {
            self.msg(format!("No se pudo actualizar tareas.txt: {e}"));
        }
        self.gcal_dirty = true;
        self.relink(&old_rel, &new_rel);
        if self.note.path == target {
            self.ws = ws.clone();
            self.save_estado();
        }
        self.vault.scan();
        self.undo = Some(Undo {
            files: Vec::new(),
            renamed: None,
            agenda: snapshot,
            at: Instant::now(),
            moved: vec![(path, target)],
            created_dir: None,
            apart: Vec::new(),
            relinks: vec![(old_rel, new_rel)],
        });
        self.undo_entry = None;
        self.msg(format!("«{title}» movida a {ws}"));
    }

    /// Cambia el nombre de un espacio (su carpeta), con sus notas, tareas y eventos.
    pub(super) fn rename_space(&mut self, old: String, name: &str) {
        let new = vault::sanitize(name);
        if new == old || name.trim().is_empty() {
            return;
        }
        if vault::is_reserved_dir(&new) {
            self.msg(format!("«{new}» es una carpeta reservada (Diario, Adjuntos o Plantillas); elige otro nombre"));
            return;
        }
        if self.vault.workspaces.iter().any(|w| *w != old && w.eq_ignore_ascii_case(&new)) {
            self.msg(format!("Ya hay un espacio «{new}»"));
            return;
        }
        self.save();
        let (from, to) = (self.vault.root.join(&old), self.vault.root.join(&new));
        let snapshot = self.agenda.snapshot();
        if let Err(e) = fs::rename(&from, &to) {
            self.msg(format!("No se pudo cambiar el nombre: {e}"));
            return;
        }
        if let Err(e) = self.agenda.rename_space(&old, &new) {
            self.msg(format!("No se pudo actualizar tareas.txt: {e}"));
        }
        self.gcal_dirty = true;
        self.relink(&old, &new);
        self.vault.scan();
        self.undo = Some(Undo {
            files: Vec::new(),
            renamed: None,
            agenda: snapshot,
            at: Instant::now(),
            moved: vec![(from, to)],
            created_dir: None,
            apart: Vec::new(),
            relinks: vec![(old.clone(), new.clone())],
        });
        self.undo_entry = None;
        self.msg(format!("El espacio «{old}» ahora se llama «{new}»"));
    }

    /// Cambia el nombre de una etiqueta en todas las notas (si ya existe la nueva, se juntan).
    pub(super) fn rename_tag(&mut self, old: String, name: &str) {
        let new = organize::clean_tag(name);
        if new.is_empty() || new == old {
            return;
        }
        self.save();
        let paths: Vec<PathBuf> = self.vault.all_notes().iter().filter(|n| n.tags().iter().any(|(t, _)| *t == old)).map(|n| n.path.clone()).collect();
        let mut files = Vec::new();
        for p in paths {
            let Ok(text) = vault::read_text(&p) else { continue };
            let Some(new_text) = tags::rename(&text, &old, &new) else { continue };
            if let Err(e) = fs::write(&p, &new_text) {
                self.msg(format!("No se pudo cambiar {}: {e}", vault::stem(&p)));
                continue;
            }
            // Cambiar una etiqueta no es contenido nuevo: la IA no la vuelve a organizar.
            if self.analyzed.contains(&ai::fnv(&text)) {
                self.analyzed.insert(ai::fnv(&new_text));
            }
            if let Some(m) = vault::modified(&p) {
                self.vault.upsert(p.clone(), new_text, m);
            }
            if p == self.note.path {
                self.note = OpenNote::load(p.clone());
            }
            files.push((p, Some(text)));
        }
        if files.is_empty() {
            return;
        }
        self.save_analyzed();
        if self.view == View::Tag(old.clone()) {
            self.view = View::Tag(new.clone());
        }
        let n = files.len();
        self.undo = Some(Undo {
            files,
            renamed: None,
            agenda: self.agenda.snapshot(),
            at: Instant::now(),
            moved: Vec::new(),
            created_dir: None,
            apart: Vec::new(),
            relinks: Vec::new(),
        });
        self.undo_entry = None;
        self.msg(format!("#{old} ahora es #{new} en {}", plural(n, "nota")));
    }

    /// Deja una nota completa como tarea (con su título; la tarea abre la nota).
    pub(super) fn note_to_task(&mut self, path: PathBuf) {
        if path == self.note.path {
            self.save();
        }
        let stem = vault::stem(&path);
        let title = if agenda::is_date(&stem) { format!("Revisar la nota del {}", long_date(&stem)) } else { stem };
        let rel = self.rel(&path);
        if self.agenda.tasks().iter().any(|t| !t.done && t.note.as_deref() == Some(rel.as_str()) && t.text == title) {
            self.msg(format!("«{title}» ya es una tarea"));
            return;
        }
        let ws = self.space_of(&path);
        match self.agenda.add_task(agenda::format_task(&today(), &title, &ws, None, &rel, Some(&new_task_id()))) {
            Ok(()) => {
                self.gcal_dirty = true;
                self.msg(format!("«{title}» quedó como tarea (en Tareas)"));
            }
            Err(e) => self.msg(format!("No se pudo escribir tareas.txt: {e}")),
        }
    }

    /// Ventana para cambiar el nombre de una etiqueta.
    pub(super) fn rename_tag_window(&mut self, ctx: &egui::Context) -> Option<Action> {
        let (old, name) = self.renaming_tag.as_mut()?;
        let old = old.clone();
        let mut action = None;
        let mut close = false;
        let modal = egui::Modal::new(Id::new("renombrar-etiqueta")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.label(RichText::new(format!("Cambiar el nombre de #{old}")).font(theme::bold(16.0)));
            ui.add_space(4.0);
            ui.label(RichText::new("Cambia en todas las notas. Si ya existe la etiqueta nueva, se juntan.").size(13.0).color(MUTED));
            ui.add_space(8.0);
            let r = ui.add(egui::TextEdit::singleline(name).hint_text("Nombre nuevo").desired_width(f32::INFINITY));
            if !r.has_focus() && !r.lost_focus() && ui.memory(|m| m.focused().is_none()) {
                r.request_focus();
            }
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new("Cambiar").color(Color32::WHITE)).fill(ACCENT)).clicked() || enter {
                    action = Some(Action::RenameTag(old.clone(), name.clone()));
                    close = true;
                }
                if ui.button("Cancelar").clicked() {
                    close = true;
                }
            });
        });
        if close || modal.should_close() {
            self.renaming_tag = None;
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relinks_notes_and_folders() {
        assert_eq!(relinked("Obra/Muro", "Obra/Muro", "General/Muro").as_deref(), Some("General/Muro"));
        assert_eq!(relinked("Obra/Muro", "Obra", "Obras").as_deref(), Some("Obras/Muro"));
        assert_eq!(relinked("Obra 2/Muro", "Obra", "Obras"), None);
        assert_eq!(relinked("Obra/Muro de piedra", "Obra/Muro", "X/Muro"), None);
    }

    fn app(dir: &Path) -> NotesApp {
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.to_path_buf(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        NotesApp::new(cfg, None, egui::Context::default())
    }

    /// Mover una nota se lleva sus tareas, sus preguntas y su pestaña; Deshacer la devuelve.
    #[test]
    fn move_a_note_with_everything_and_undo() {
        let dir = std::env::temp_dir().join(format!("nodex-mover-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Obra")).unwrap();
        fs::create_dir_all(dir.join("General")).unwrap();
        let muro = dir.join("Obra").join("Muro.md");
        fs::write(&muro, "- [ ] Cubicar ^cu001\nOtra línea\n").unwrap();
        fs::write(dir.join("tareas.txt"), "2026-09-30 Cubicar +Obra nota:Obra/Muro id:cu001\n").unwrap();
        let mut app = app(&dir);
        app.doubts.pending.push(crate::doubts::Doubt { id: "d1".into(), note: "Obra/Muro".into(), unit: "Otra línea".into(), ..Default::default() });
        app.open_in_tab(muro.clone(), None);

        app.apply(Action::MoveNote(muro.clone(), "General".into()));
        let moved = dir.join("General").join("Muro.md");
        assert!(moved.exists() && !muro.exists());
        assert_eq!(app.note.path, moved);
        assert_eq!(app.ws, "General");
        assert!(app.tabs.list.contains(&tabs::Tab::Note(moved.clone())));
        let t = &app.agenda.tasks()[0];
        assert_eq!((t.project.as_str(), t.note.as_deref()), ("General", Some("General/Muro")));
        assert_eq!(app.doubts.pending[0].note, "General/Muro");
        assert_eq!(app.reconcile_tasks(), 0);

        app.apply(Action::Undo);
        assert!(muro.exists() && !moved.exists());
        assert_eq!(app.note.path, muro);
        assert_eq!(app.agenda.tasks()[0].note.as_deref(), Some("Obra/Muro"));
        assert_eq!(app.doubts.pending[0].note, "Obra/Muro");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Cambiar el nombre de un espacio lleva sus notas, tareas y eventos; Deshacer lo devuelve.
    #[test]
    fn rename_a_space_and_undo() {
        let dir = std::env::temp_dir().join(format!("nodex-renombrar-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Obra Talca")).unwrap();
        fs::create_dir_all(dir.join("Otra")).unwrap();
        let muro = dir.join("Obra Talca").join("Muro.md");
        fs::write(&muro, "Hola\n").unwrap();
        fs::write(dir.join("tareas.txt"), "2026-09-30 Cubicar +Obra_Talca nota:Obra%20Talca/Muro id:cu001\n2026-09-30 Otra +Otra id:ot002\n").unwrap();
        fs::write(dir.join("agenda.txt"), "2026-10-01 10:00 Visita +Obra_Talca nota:Obra%20Talca/Muro\n").unwrap();
        let mut app = app(&dir);
        app.open(muro.clone(), None);

        app.apply(Action::RenameWorkspace("Obra Talca".into(), "Obra LaVet".into()));
        assert!(app.vault.workspaces.contains(&"Obra LaVet".to_string()) && !app.vault.workspaces.contains(&"Obra Talca".to_string()));
        assert_eq!(app.ws, "Obra LaVet");
        assert_eq!(app.note.path, dir.join("Obra LaVet").join("Muro.md"));
        assert_eq!(
            fs::read_to_string(dir.join("tareas.txt")).unwrap(),
            "2026-09-30 Cubicar +Obra_LaVet nota:Obra%20LaVet/Muro id:cu001\n2026-09-30 Otra +Otra id:ot002\n"
        );
        assert_eq!(fs::read_to_string(dir.join("agenda.txt")).unwrap(), "2026-10-01 10:00 Visita +Obra_LaVet nota:Obra%20LaVet/Muro\n");
        // No se puede usar el nombre de otro espacio ni el del Diario.
        app.apply(Action::RenameWorkspace("Obra LaVet".into(), "otra".into()));
        app.apply(Action::RenameWorkspace("Obra LaVet".into(), "Diario".into()));
        assert!(app.vault.workspaces.contains(&"Obra LaVet".to_string()));

        app.apply(Action::Undo);
        assert!(muro.exists());
        assert_eq!(app.ws, "Obra Talca");
        assert_eq!(app.note.path, muro);
        assert!(fs::read_to_string(dir.join("tareas.txt")).unwrap().contains("+Obra_Talca nota:Obra%20Talca/Muro"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Un calendario cambia de nombre sin volver a agregarlo, y una cuenta de correo cambia su
    /// servidor sin tener que escribir de nuevo la contraseña.
    #[test]
    fn edit_calendar_and_mail_account() {
        let dir = std::env::temp_dir().join(format!("nodex-editar-cuentas-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("General")).unwrap();
        let mut app = app(&dir);
        app.cfg.calendarios = vec![crate::calendars::Subscription { nombre: "Trabajo".into(), url: "https://x.cl/a.ics".into() }];
        app.cfg.correos = vec![crate::mail::Account { correo: "a@x.cl".into(), clave: "secreta".into(), servidor: String::new() }];
        app.apply(Action::EditCalendar(0, "Universidad".into(), "https://x.cl/a.ics".into()));
        assert_eq!(app.cfg.calendarios[0].nombre, "Universidad");
        app.apply(Action::UpdateMailAccount(0, crate::mail::Account { correo: "a@x.cl".into(), clave: String::new(), servidor: "imap.x.cl".into() }));
        assert_eq!((app.cfg.correos[0].clave.as_str(), app.cfg.correos[0].servidor.as_str()), ("secreta", "imap.x.cl"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Una línea o una nota se vuelven tarea a mano.
    #[test]
    fn lines_and_notes_become_tasks() {
        let dir = std::env::temp_dir().join(format!("nodex-a-tarea-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Obra")).unwrap();
        let muro = dir.join("Obra").join("Muro.md");
        fs::write(&muro, "Revisar #planos del muro
  detalle
").unwrap();
        let mut app = app(&dir);
        app.open(muro.clone(), None);
        app.line_to_task(0);
        let text = fs::read_to_string(&muro).unwrap();
        let id = lines::id_of(text.lines().next().unwrap()).expect("con identificador");
        assert!(text.starts_with("- [ ] Revisar #planos del muro ^"), "{text}");
        let t = app.agenda.tasks().into_iter().find(|t| t.id.as_deref() == Some(id.as_str())).unwrap();
        assert_eq!((t.text.as_str(), t.project.as_str(), t.note.as_deref()), ("Revisar planos del muro", "Obra", Some("Obra/Muro")));
        assert_eq!(app.reconcile_tasks(), 0, "la tarea ya calza con su línea");
        // Otra vez: la marca hecha.
        app.line_to_task(0);
        assert!(fs::read_to_string(&muro).unwrap().starts_with("- [x] "));
        assert!(app.agenda.tasks()[0].done);
        // La nota completa, como tarea (una sola vez).
        app.apply(Action::NoteToTask(muro.clone()));
        app.apply(Action::NoteToTask(muro.clone()));
        let tasks: Vec<_> = app.agenda.tasks().into_iter().filter(|t| t.text == "Muro").collect();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].note.as_deref(), Some("Obra/Muro"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Cambiar el nombre de una etiqueta la cambia en todas las notas, y se puede deshacer.
    #[test]
    fn rename_a_tag_everywhere_and_undo() {
        let dir = std::env::temp_dir().join(format!("nodex-etiqueta-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Obra")).unwrap();
        fs::create_dir_all(dir.join("General")).unwrap();
        let (a, b) = (dir.join("Obra").join("A.md"), dir.join("General").join("B.md"));
        fs::write(&a, "Revisar #Vigas\n").unwrap();
        fs::write(&b, "Otra #vigas y #obra\n").unwrap();
        let mut app = app(&dir);
        app.apply(Action::RenameTag("vigas".into(), "Losas".into()));
        assert_eq!(fs::read_to_string(&a).unwrap(), "Revisar #losas\n");
        assert_eq!(fs::read_to_string(&b).unwrap(), "Otra #losas y #obra\n");
        app.apply(Action::Undo);
        assert_eq!(fs::read_to_string(&a).unwrap(), "Revisar #Vigas\n");
        let _ = fs::remove_dir_all(&dir);
    }
}
