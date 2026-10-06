//! Trabajar cada cosa de «Tu día» (y de la Semana) ahí mismo, sin buscarla:
//!
//! - tareas: ir a la línea donde está escrita, cambiarle la fecha, seguimiento, convertirla en
//!   una nota o borrarla;
//! - eventos (de las notas o de los calendarios): marcarlos hechos, seguimiento, ir a su línea,
//!   tomar notas o convertirlos en tarea.
//!
//! Los eventos de los calendarios no se pueden cambiar allá: lo hecho y lo convertido en tarea
//! se anota en `.nodex/eventos.txt` («hecho» o «tarea», TAB, la clave del evento).

use super::*;
use chrono::Datelike;
use tracking::Target;

/// Lo anotado de los eventos.
pub(super) const EVENTS_FILE: &str = "eventos.txt";
/// En la memoria de egui: la clave de lo que está recibiendo un seguimiento.
pub(super) const FOCUS: &str = "seguimiento-foco";

/// Mientras se anota un seguimiento, lo demás se atenúa (para ver a qué se le está anotando).
pub(super) fn dim_unless(ui: &mut Ui, key: &str) {
    let focus = ui.ctx().data(|d| d.get_temp::<Option<String>>(Id::new(FOCUS))).flatten();
    if focus.is_some_and(|f| f != key) {
        ui.multiply_opacity(0.3);
    }
}

/// Lo que se hace con una tarea o un evento de Tu día.
pub(super) enum ItemDo {
    /// Ir a la línea de la tarea (por su línea completa en tareas.txt).
    OpenTask(String),
    /// Cambiar la fecha (None = quitarla).
    TaskDue(String, Option<String>),
    TaskToNote(String),
    TaskDelete(String),
    OpenEvent(agenda::Event),
    EventDone(agenda::Event, bool),
    EventFollowUp(agenda::Event),
    EventToTask(agenda::Event),
}

/// La clave de un evento (para lo anotado y sus seguimientos).
pub(super) fn event_key(e: &agenda::Event) -> String {
    format!("evento:{}:{}", e.date, vault::fold(e.title.trim()))
}

/// Dónde empieza la línea `idx` (en caracteres), para abrir la nota ahí.
fn line_offset(text: &str, idx: usize) -> usize {
    text.split('\n').take(idx).map(|l| l.chars().count() + 1).sum()
}

fn day_plus(days: i64) -> String {
    (Local::now() + chrono::Duration::days(days)).format("%Y-%m-%d").to_string()
}

/// El lunes que viene.
fn next_monday() -> String {
    let wd = Local::now().weekday().num_days_from_monday() as i64;
    day_plus(7 - wd)
}

/// El menú de una tarea (⋯ o clic derecho).
pub(super) fn task_menu(ui: &mut Ui, t: &agenda::Task) -> Option<Action> {
    let mut action = None;
    let raw = &t.raw;
    let mut item = |ui: &mut Ui, label: String, what: ItemDo| {
        if ui.button(label).clicked() {
            action = Some(Action::Item(what));
            ui.close();
        }
    };
    let mut follow = false;
    if t.note.as_deref().is_some_and(|n| !n.is_empty()) {
        item(ui, format!("{}  {}", icon::ARROW_SQUARE_OUT, t!("Ir a donde está escrita")), ItemDo::OpenTask(raw.clone()));
    }
    if !t.done {
        ui.menu_button(format!("{}  {}", icon::CALENDAR_BLANK, t!("Cambiar fecha")), |ui| {
            item(ui, t!("Hoy").into(), ItemDo::TaskDue(raw.clone(), Some(today())));
            item(ui, t!("Mañana").into(), ItemDo::TaskDue(raw.clone(), Some(day_plus(1))));
            item(ui, t!("El lunes").into(), ItemDo::TaskDue(raw.clone(), Some(next_monday())));
            item(ui, t!("En una semana").into(), ItemDo::TaskDue(raw.clone(), Some(day_plus(7))));
            if t.due.is_some() {
                item(ui, t!("Sin fecha").into(), ItemDo::TaskDue(raw.clone(), None));
            }
        });
    }
    if ui.button(format!("{}  {}", icon::ARROW_ELBOW_DOWN_RIGHT, t!("Seguimiento"))).clicked() {
        follow = true;
        ui.close();
    }
    item(ui, format!("{}  {}", icon::FILE_TEXT, t!("Convertir en nota")), ItemDo::TaskToNote(raw.clone()));
    item(ui, format!("{}  {}", icon::TRASH, t!("Borrar la tarea")), ItemDo::TaskDelete(raw.clone()));
    if follow {
        action = Some(Action::FollowUp(raw.clone()));
    }
    action
}

/// Una fila de evento: casilla, hora, título y espacio; y lo que se puede hacer con él.
/// `date` = mostrar el día (en la Semana).
pub(super) fn event_row(ui: &mut Ui, e: &agenda::Event, done: bool, follows: &tracking::FollowUps, date: bool) -> Option<Action> {
    ui.scope(|ui| {
        dim_unless(ui, &event_key(e));
        event_row_inner(ui, e, done, follows, date)
    })
    .inner
}

fn event_row_inner(ui: &mut Ui, e: &agenda::Event, done: bool, follows: &tracking::FollowUps, date: bool) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        let (glyph, color) = if done { (icon::CHECK_SQUARE, ACCENT()) } else { (icon::SQUARE, MUTED()) };
        let check = egui::Button::new(RichText::new(glyph).size(18.0).color(color)).frame(false);
        if ui.add(check).on_hover_text(if done { t!("Marcar pendiente") } else { t!("Marcar hecho") }).clicked() {
            action = Some(Action::Item(ItemDo::EventDone(e.clone(), !done)));
        }
        let mut job = LayoutJob::default();
        if date {
            job.append(&format!("{}  ", long_date(&e.date)), 0.0, fmt(FontId::proportional(13.5), MUTED()));
        }
        let lead = match &e.time {
            Some(t) => format!("{t}  "),
            None => format!("{}  ", icon::CALENDAR_BLANK),
        };
        job.append(&lead, 0.0, fmt(FontId::proportional(14.0), MUTED()));
        let mut body = fmt(FontId::proportional(14.5), if done { MUTED() } else { TEXT() });
        if done {
            body.strikethrough = Stroke::new(1.0, MUTED());
        }
        job.append(&e.title, 0.0, body);
        if !e.project.is_empty() {
            job.append(&format!("   {}", e.project), 0.0, fmt(FontId::proportional(12.5), MUTED()));
        }
        let r = ui.add(egui::Label::new(job).wrap().sense(Sense::click()));
        if r.clicked() && e.note.is_some() {
            action = Some(Action::Item(ItemDo::OpenEvent(e.clone())));
        }
        let menu = |ui: &mut Ui| -> Option<Action> {
            let mut a = None;
            let mut item = |ui: &mut Ui, label: String, what: Action| {
                if ui.button(label).clicked() {
                    a = Some(what);
                    ui.close();
                }
            };
            if e.note.is_some() {
                item(ui, format!("{}  {}", icon::ARROW_SQUARE_OUT, t!("Ir a donde está escrito")), Action::Item(ItemDo::OpenEvent(e.clone())));
            }
            item(ui, format!("{}  {}", icon::NOTE_PENCIL, t!("Tomar notas")), Action::StartMeetingNamed(e.title.clone()));
            item(ui, format!("{}  {}", icon::ARROW_ELBOW_DOWN_RIGHT, t!("Seguimiento")), Action::Item(ItemDo::EventFollowUp(e.clone())));
            item(ui, format!("{}  {}", icon::CHECK_SQUARE, t!("Convertir en tarea")), Action::Item(ItemDo::EventToTask(e.clone())));
            let label = if done { format!("{}  {}", icon::SQUARE, t!("Marcar pendiente")) } else { format!("{}  {}", icon::CHECK_SQUARE, t!("Marcar hecho")) };
            item(ui, label, Action::Item(ItemDo::EventDone(e.clone(), !done)));
            a
        };
        r.context_menu(|ui| {
            if let Some(a) = menu(ui) {
                action = Some(a);
            }
        });
        if e.note.is_some() {
            let b = egui::Button::new(RichText::new(icon::ARROW_SQUARE_OUT).size(14.0).color(MUTED())).frame(false);
            if ui.add(b).on_hover_text(t!("Ir a donde está escrito")).clicked() {
                action = Some(Action::Item(ItemDo::OpenEvent(e.clone())));
            }
        }
        let b = egui::Button::new(RichText::new(icon::ARROW_ELBOW_DOWN_RIGHT).size(14.0).color(MUTED())).frame(false);
        if ui.add(b).on_hover_text(t!("Anotar un seguimiento: qué se hizo")).clicked() {
            action = Some(Action::Item(ItemDo::EventFollowUp(e.clone())));
        }
        let b = egui::Button::new(RichText::new(icon::DOTS_THREE).size(16.0).color(MUTED())).frame(false);
        let r = ui.add(b).on_hover_text(t!("Más: tomar notas, convertir en tarea…"));
        egui::Popup::menu(&r).show(|ui| {
            if let Some(a) = menu(ui) {
                action = Some(a);
            }
        });
    });
    for (d, text) in follows.get(&event_key(e)).into_iter().flatten() {
        let when = if d.is_empty() { String::new() } else { format!("{}  ·  ", tracking::short_day(d)) };
        ui.horizontal_wrapped(|ui| {
            ui.add_space(30.0);
            ui.label(RichText::new(format!("{}  {when}{text}", icon::ARROW_ELBOW_DOWN_RIGHT)).size(12.5).color(theme::c(Color32::from_rgb(70, 110, 75))));
        });
    }
    action
}

impl NotesApp {
    /// Lo anotado de los eventos: clave -> «hecho» o «tarea».
    pub(super) fn event_marks(&self) -> HashMap<String, String> {
        let text = vault::read_text(&self.vault.root.join(".nodex").join(EVENTS_FILE)).unwrap_or_default();
        let mut out = HashMap::new();
        for l in text.lines() {
            if let Some((state, key)) = l.split_once('\t') {
                if state == "-" {
                    out.remove(key);
                } else {
                    out.insert(key.to_string(), state.to_string());
                }
            }
        }
        out
    }

    fn mark_event(&mut self, e: &agenda::Event, state: &str) {
        let path = self.vault.root.join(".nodex").join(EVENTS_FILE);
        let _ = fs::create_dir_all(self.vault.root.join(".nodex"));
        let mut text = vault::read_text(&path).unwrap_or_default();
        text += &format!("{state}\t{}\n", event_key(e));
        let _ = fs::write(&path, text);
    }

    pub(super) fn item_do(&mut self, what: ItemDo) {
        match what {
            ItemDo::OpenTask(raw) => self.open_task_line(&raw),
            ItemDo::TaskDue(raw, due) => self.task_due(&raw, due),
            ItemDo::TaskToNote(raw) => self.task_to_note(&raw),
            ItemDo::TaskDelete(raw) => self.task_delete(&raw),
            ItemDo::OpenEvent(e) => self.open_event_line(&e),
            ItemDo::EventDone(e, done) => {
                self.mark_event(&e, if done { "hecho" } else { "-" });
                if done {
                    self.ask_follow_up(e.title.clone(), Target::Task(event_key(&e)), true);
                }
            }
            ItemDo::EventFollowUp(e) => self.ask_follow_up(e.title.clone(), Target::Task(event_key(&e)), false),
            ItemDo::EventToTask(e) => self.event_to_task(&e),
        }
    }

    /// Abre la nota de una tarea en su línea (la del `^id`).
    fn open_task_line(&mut self, raw: &str) {
        let Some(t) = agenda::parse_task(raw) else { return };
        let Some(note) = t.note.filter(|n| !n.is_empty()) else {
            self.msg(t!("Esta tarea no está escrita en ninguna nota"));
            return;
        };
        let path = self.vault.root.join(format!("{note}.md"));
        if !path.is_file() {
            self.msg(tf!("La nota «{note}» ya no está (¿se movió o se archivó?)", note = note));
            return;
        }
        let text = vault::read_text(&path).unwrap_or_default();
        let at = t.id.as_deref().and_then(|id| text.split('\n').position(|l| lines::id_of(l).as_deref() == Some(id)));
        self.open_in_tab(path, at.map(|i| line_offset(&text, i)));
    }

    /// Abre la nota de un evento en la línea que lo nombra.
    fn open_event_line(&mut self, e: &agenda::Event) {
        let Some(note) = &e.note else { return };
        let path = self.vault.root.join(format!("{note}.md"));
        if !path.is_file() {
            self.msg(tf!("La nota «{note}» ya no está (¿se movió o se archivó?)", note = note));
            return;
        }
        let text = vault::read_text(&path).unwrap_or_default();
        let want = vault::fold(e.title.trim());
        let words: Vec<&str> = want.split_whitespace().filter(|w| w.chars().count() > 3).collect();
        // La línea que más palabras del título tiene.
        let at = text
            .split('\n')
            .enumerate()
            .map(|(i, l)| {
                let l = vault::fold(l);
                (i, if l.contains(&want) { usize::MAX } else { words.iter().filter(|w| l.contains(*w)).count() })
            })
            .filter(|(_, n)| *n > 0)
            .max_by_key(|(i, n)| (*n, std::cmp::Reverse(*i)))
            .map(|(i, _)| i);
        self.open_in_tab(path, at.map(|i| line_offset(&text, i)));
    }

    /// Cambia (o quita) la fecha de una tarea, aquí y en su línea.
    fn task_due(&mut self, raw: &str, due: Option<String>) {
        let Some(t) = agenda::parse_task(raw) else { return };
        let bare: Vec<&str> = raw.split_whitespace().filter(|w| !w.starts_with("due:")).collect();
        let mut new = bare.join(" ");
        if let Some(d) = &due {
            new += &format!(" due:{d}");
        }
        if let Err(e) = self.agenda.replace_task(raw, Some(new)) {
            self.msg(tf!("No se pudo escribir tareas.txt: {e}", e = e));
            return;
        }
        if let (Some(note), Some(id)) = (t.note.as_deref().filter(|n| !n.is_empty()), t.id.as_deref()) {
            let due = due.clone();
            self.edit_task_line(note, id, |line| {
                let bare: String = line.split(' ').filter(|w| !w.starts_with("due:")).collect::<Vec<_>>().join(" ");
                let new = match &due {
                    Some(d) => lines::set_meta(&bare, Some(d), None),
                    None => bare,
                };
                (new != line).then_some(new)
            });
        }
        self.gcal_dirty = true;
        let what = agenda::display_text(&t.text);
        self.msg(match &due {
            Some(d) => tf!("«{what}» para el {date}", what = what, date = long_date(d)),
            None => tf!("«{what}» quedó sin fecha", what = what),
        });
    }

    /// La línea de la tarea en su nota pasa a ser texto normal (sin casilla ni identificador).
    /// Devuelve (nota, cómo estaba) para poder deshacer.
    fn unlink_task_line(&mut self, t: &agenda::Task) -> Option<(PathBuf, String)> {
        let note = t.note.as_deref().filter(|n| !n.is_empty())?;
        let id = t.id.as_deref()?;
        let path = self.vault.root.join(format!("{note}.md"));
        let before = if path == self.note.path { self.note.text.clone() } else { vault::read_text(&path).ok()? };
        self.edit_task_line(note, id, |line| {
            let info = lines::parse(line);
            let text: Vec<&str> = line[info.prefix..].split_whitespace().filter(|w| !w.starts_with('^') && !w.starts_with("due:")).collect();
            Some(format!("{}{}", &line[..info.indent], text.join(" ")))
        });
        Some((path, before))
    }

    /// La tarea pasa a ser una nota en su espacio (con sus seguimientos); con Deshacer.
    fn task_to_note(&mut self, raw: &str) {
        let Some(t) = agenda::parse_task(raw) else { return };
        let title_full = agenda::display_text(&t.text);
        let title: String = title_full.chars().take(70).collect();
        let ws = self
            .vault
            .workspaces
            .iter()
            .find(|w| w.eq_ignore_ascii_case(&t.project))
            .cloned()
            .unwrap_or_else(|| self.home_ws());
        let path = self.vault.unique_path(&ws, title.trim_end_matches(['.', ' ']));
        let mut body = format!("{title_full}\n");
        let follows = self.follow_up_map();
        for (d, f) in follows.get(&tracking::task_key(&t)).into_iter().flatten() {
            body += &format!("  {} {d}: {f}\n", lines::FOLLOW_MARK);
        }
        if let Some(n) = t.note.as_deref().filter(|n| !n.is_empty()) {
            body += &format!("\nVenía de [[{}]]\n", vault::stem(Path::new(n)));
        }
        let snapshot = self.agenda.snapshot();
        if let Err(e) = fs::write(&path, &body) {
            self.msg(tf!("No se pudo crear la nota: {e}", e = e));
            return;
        }
        let mut files = vec![(path.clone(), None)];
        files.extend(self.unlink_task_line(&t).map(|(p, b)| (p, Some(b))));
        let _ = self.agenda.replace_task(raw, None);
        self.gcal_dirty = true;
        self.vault.scan();
        self.undo = Some(Undo {
            files,
            renamed: None,
            agenda: snapshot,
            at: Instant::now(),
            moved: Vec::new(),
            created_dir: None,
            apart: Vec::new(),
            keep_tasks: Vec::new(),
            relinks: Vec::new(),
        });
        self.undo_entry = None;
        self.msg(tf!("«{name}» ahora es una nota en {ws} (Deshacer, abajo)", name = vault::stem(&path), ws = ws));
        self.open_in_tab(path, None);
    }

    /// Borra la tarea; en su nota queda el texto, sin casilla. Con Deshacer.
    fn task_delete(&mut self, raw: &str) {
        let Some(t) = agenda::parse_task(raw) else { return };
        let snapshot = self.agenda.snapshot();
        let files: Vec<(PathBuf, Option<String>)> = self.unlink_task_line(&t).map(|(p, b)| (p, Some(b))).into_iter().collect();
        if let Err(e) = self.agenda.replace_task(raw, None) {
            self.msg(tf!("No se pudo escribir tareas.txt: {e}", e = e));
            return;
        }
        self.gcal_dirty = true;
        self.undo = Some(Undo {
            files,
            renamed: None,
            agenda: snapshot,
            at: Instant::now(),
            moved: Vec::new(),
            created_dir: None,
            apart: Vec::new(),
            keep_tasks: Vec::new(),
            relinks: Vec::new(),
        });
        self.undo_entry = None;
        self.msg(tf!("Tarea «{name}» borrada (Deshacer, abajo)", name = agenda::display_text(&t.text)));
    }

    /// El evento pasa a ser una tarea para ese día (y deja de verse como evento). Con Deshacer.
    fn event_to_task(&mut self, e: &agenda::Event) {
        let ws = self
            .vault
            .workspaces
            .iter()
            .find(|w| w.eq_ignore_ascii_case(&e.project))
            .cloned()
            .unwrap_or_else(|| self.home_ws());
        let mut line = format!("{} {} {} due:{}", today(), e.title.trim(), agenda::project_token(&ws), e.date);
        if let Some(n) = &e.note {
            line += &format!(" nota:{}", agenda::encode_note(n));
        }
        line += &format!(" id:{}", new_task_id());
        let snapshot = self.agenda.snapshot();
        let marks = self.vault.root.join(".nodex").join(EVENTS_FILE);
        let marks_before = vault::read_text(&marks).ok();
        if let Err(err) = self.agenda.add_task(line) {
            self.msg(tf!("No se pudo escribir tareas.txt: {e}", e = err));
            return;
        }
        // Un evento de las notas se quita de la agenda; uno de un calendario se anota.
        if e.note.is_some() && self.agenda.events().contains(e) {
            let _ = self.agenda.remove_event(e);
        } else {
            self.mark_event(e, "tarea");
        }
        self.gcal_dirty = true;
        self.undo = Some(Undo {
            files: vec![(marks, marks_before)],
            renamed: None,
            agenda: snapshot,
            at: Instant::now(),
            moved: Vec::new(),
            created_dir: None,
            apart: Vec::new(),
            keep_tasks: Vec::new(),
            relinks: Vec::new(),
        });
        self.undo_entry = None;
        self.msg(tf!("«{name}» ahora es una tarea para el {date} (Deshacer, abajo)", name = e.title.trim(), date = long_date(&e.date)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Desde Tu día: cambiar la fecha, convertir una tarea en nota y un evento en tarea, marcar un
    /// evento hecho y borrar una tarea (su línea queda como texto).
    #[test]
    fn work_items_from_your_day() {
        let dir = std::env::temp_dir().join(format!("nodex-tu-dia-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("Obra")).unwrap();
        let muro = dir.join("Obra").join("Muro.md");
        fs::write(&muro, "Visita\n- [ ] Enviar planos due:2026-10-02 ^pl001\n- [ ] Llamar a Juan ^ju002\nReunión con el inspector el lunes\n").unwrap();
        fs::write(
            dir.join("tareas.txt"),
            "2026-10-01 Enviar planos +Obra due:2026-10-02 nota:Obra/Muro id:pl001\n2026-10-01 Llamar a Juan +Obra nota:Obra/Muro id:ju002\n",
        )
        .unwrap();
        fs::write(dir.join("agenda.txt"), "2026-10-05 10:00 Reunión con el inspector +Obra nota:Obra/Muro\n").unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        let task = |app: &NotesApp, id: &str| app.agenda.tasks().into_iter().find(|t| t.id.as_deref() == Some(id));

        // Fecha nueva: en tareas.txt y en la línea.
        let raw = task(&app, "pl001").unwrap().raw;
        app.apply(Action::Item(ItemDo::TaskDue(raw, Some("2026-10-09".into()))));
        assert_eq!(task(&app, "pl001").unwrap().due.as_deref(), Some("2026-10-09"));
        assert!(fs::read_to_string(&muro).unwrap().contains("Enviar planos due:2026-10-09 ^pl001"));

        // Ir a la línea de la tarea.
        let raw = task(&app, "pl001").unwrap().raw;
        app.apply(Action::Item(ItemDo::OpenTask(raw)));
        assert_eq!(app.note.path, muro);

        // Tarea -> nota: la tarea se va, su línea queda como texto y hay una nota nueva.
        let raw = task(&app, "ju002").unwrap().raw;
        app.apply(Action::Item(ItemDo::TaskToNote(raw)));
        assert!(task(&app, "ju002").is_none());
        let nueva = dir.join("Obra").join("Llamar a Juan.md");
        assert!(fs::read_to_string(&nueva).unwrap().starts_with("Llamar a Juan\n"));
        assert!(fs::read_to_string(&muro).unwrap().contains("\nLlamar a Juan\n"));
        app.apply(Action::Undo);
        assert!(task(&app, "ju002").is_some() && !nueva.exists());
        assert!(fs::read_to_string(&muro).unwrap().contains("- [ ] Llamar a Juan ^ju002"));

        // Evento -> tarea para ese día (y sale de la agenda).
        let ev = app.agenda.events().remove(0);
        app.apply(Action::Item(ItemDo::EventToTask(ev.clone())));
        assert!(app.agenda.events().is_empty());
        let t = app.agenda.tasks().into_iter().find(|t| t.text == "Reunión con el inspector").unwrap();
        assert_eq!((t.due.as_deref(), t.note.as_deref()), (Some("2026-10-05"), Some("Obra/Muro")));

        // Un evento de un calendario, hecho (y su seguimiento queda con él).
        let cal = agenda::Event { date: "2026-10-04".into(), time: Some("09:45".into()), title: "Ortodoncia".into(), project: "Trabajo".into(), note: None, mail: None };
        app.apply(Action::Item(ItemDo::EventDone(cal.clone(), true)));
        assert_eq!(app.event_marks().get(&event_key(&cal)).map(String::as_str), Some("hecho"));
        assert!(app.add_follow_up(Target::Task(event_key(&cal)), "Fue bien"));
        assert_eq!(app.follow_up_map()[&event_key(&cal)][0].1, "Fue bien");
        app.apply(Action::Item(ItemDo::EventDone(cal.clone(), false)));
        assert!(!app.event_marks().contains_key(&event_key(&cal)));

        // Borrar una tarea: en la nota queda el texto, sin casilla.
        let raw = task(&app, "pl001").unwrap().raw;
        app.apply(Action::Item(ItemDo::TaskDelete(raw)));
        assert!(task(&app, "pl001").is_none());
        assert!(fs::read_to_string(&muro).unwrap().contains("\nEnviar planos\n"));
        let _ = fs::remove_dir_all(&dir);
    }
}
