//! Seguimiento: lo que se hizo con una nota o una tarea, anotado debajo de ella con su fecha
//! (`  ↳ 2026-10-01: Se pidió a Gerdau, llega el lunes`). Queda como contexto para la IA (al
//! organizar, al preguntar y en la revisión semanal).
//!
//! - En el editor: clic derecho en una línea → «Seguimiento» (o Ctrl+Shift+Enter).
//! - En Tareas: el botón de seguimiento de cada tarea; sus seguimientos se ven debajo.
//! - Al marcar una tarea como hecha, la app pregunta «¿Qué se hizo?» (se puede omitir).
//!
//! Las tareas que no tienen una línea en una nota (creadas a mano, o de To Do) guardan su
//! seguimiento en `.nodex/seguimiento.txt` (`clave⇥fecha⇥texto`).
//!
//! Si un seguimiento de una tarea pendiente dice que ya se terminó («ya se envió», «listo»), la
//! app propone marcarla hecha (no la marca sola). Lo decide la IA (o, sin IA, unas palabras
//! claras); cada seguimiento se revisa una sola vez (`.nodex/seguimiento-revisado.txt`).

use super::*;
use std::rc::Rc;

pub(super) const STORE: &str = "seguimiento.txt";
/// Los seguimientos ya revisados (si dicen que la tarea se terminó): una huella por línea.
pub(super) const CHECKED: &str = "seguimiento-revisado.txt";

const DONE_SYSTEM: &str = "Decides si un seguimiento dice que una tarea ya está terminada (se hizo lo que la tarea pide). Responde solo «sí» o «no».";

/// Sin IA: ¿el seguimiento dice claramente que se terminó? (`None` = no se sabe)
pub(super) fn looks_done(text: &str) -> Option<bool> {
    let t = format!(" {} ", vault::fold(text).replace([',', '.', ';', ':', '!', '¡'], " "));
    const NOT: [&str; 14] = [
        " falta", " pendiente", " todavia no", " aun no", " no se ", " no han", " no ha ", " esperando", " espera ", " por confirmar", " sin respuesta",
        " no contesta", " no responde", " mañana ",
    ];
    if NOT.iter().any(|w| t.contains(w)) {
        return Some(false);
    }
    const DONE: [&str; 22] = [
        " listo ", " lista ", " hecho ", " hecha ", " terminad", " resuelt", " enviad", " entregad", " pagad", " se envio", " se entrego", " se pago",
        " se hizo", " ya se ", " completad", " cerrad", " ok ", " confirmad", " aprobad", " firmad", " recibid", " llego ",
    ];
    DONE.iter().any(|w| t.contains(w)).then_some(true)
}

/// Una sugerencia de marcar hecha: la tarea, su nombre y lo que dice el seguimiento.
pub(super) struct DoneSuggestion {
    target: Target,
    title: String,
    quote: String,
    /// Desde cuándo se ve (se va sola a los `ASK_FOR`, salvo con el mouse encima).
    at: Instant,
}

/// Cuánto quedan en pantalla «¿Qué se hizo?» y «¿La marco hecha?» si no se responden (con el
/// mouse encima, no se van). Si se van, no cambia nada.
const ASK_FOR: Duration = Duration::from_secs(30);

/// La clave con que se revisa un seguimiento.
fn target_key(t: &Target) -> String {
    match t {
        Target::Line { id: Some(id), .. } => id.clone(),
        Target::Line { text, .. } => format!("linea:{}", vault::fold(&line_title(text))),
        Target::Task(k) => k.clone(),
    }
}

/// La clave de una tarea: su identificador o, si no tiene, su texto.
pub(super) fn task_key(t: &agenda::Task) -> String {
    t.id.clone().unwrap_or_else(|| format!("tarea:{}", vault::fold(t.text.trim())))
}

/// Seguimientos por clave de tarea: (fecha, texto).
pub(super) type FollowUps = HashMap<String, Vec<(String, String)>>;

/// A qué se le agrega un seguimiento.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Target {
    /// Una línea de una nota: por su identificador (`^id`) o, si no tiene, por su texto.
    Line { note: PathBuf, id: Option<String>, text: String },
    /// Una tarea sin línea en una nota (por su clave).
    Task(String),
}

/// La pregunta «¿Qué se hizo?», abajo al centro.
pub(super) struct Ask {
    title: String,
    target: Target,
    text: String,
    /// Se acaba de marcar hecha (si no, se pidió un seguimiento).
    done: bool,
    at: Instant,
    focus: bool,
}

/// «1 oct» de «2026-10-01».
pub(super) fn short_day(date: &str) -> String {
    NaiveDate::parse_from_str(date, "%Y-%m-%d").map(|d| format!("{} {}", d.day(), MESES[d.month0() as usize])).unwrap_or_else(|_| date.to_string())
}

/// Lo que se ve de una línea de tarea: sin casilla, fecha ni identificador.
pub(super) fn line_title(line: &str) -> String {
    let info = lines::parse(line);
    line[info.prefix..].split_whitespace().filter(|w| !w.starts_with("due:") && !(w.starts_with('^') && w.len() > 3)).collect::<Vec<_>>().join(" ")
}

impl NotesApp {
    /// Pregunta qué se hizo (o pide un seguimiento) para `target`.
    pub(super) fn ask_follow_up(&mut self, title: String, target: Target, done: bool) {
        self.follow_ask = Some(Ask { title, target, text: String::new(), done, at: Instant::now(), focus: true });
    }

    /// Anota un seguimiento. Devuelve si se pudo.
    pub(super) fn add_follow_up(&mut self, target: Target, what: &str) -> bool {
        let what = what.replace(['\t', '\n', '\r'], " ");
        let what = what.trim();
        if what.is_empty() {
            return false;
        }
        match target {
            Target::Line { note, id, text } => {
                let open = note == self.note.path;
                let old = if open { self.note.text.clone() } else { vault::read_text(&note).unwrap_or_default() };
                let idx = old.split('\n').map(|l| l.trim_end_matches('\r')).position(|l| match &id {
                    Some(id) => lines::id_of(l).as_deref() == Some(id.as_str()),
                    None => l.trim_end() == text.trim_end(),
                });
                let Some(idx) = idx else {
                    // La línea ya no está: queda con la tarea.
                    let key = id.unwrap_or_else(|| format!("tarea:{}", vault::fold(&line_title(&text))));
                    return self.add_follow_up(Target::Task(key), what);
                };
                let (new, _) = lines::add_follow_up(&old, idx, &today(), what);
                // Anotar un seguimiento no es contenido para reorganizar: la IA no la vuelve a mover.
                if self.analyzed.contains(&ai::fnv(&old)) {
                    self.analyzed.insert(ai::fnv(&new));
                    self.save_analyzed();
                }
                if open {
                    self.note.text = new;
                    self.note.dirty = true;
                    self.note.last_edit = Instant::now();
                    self.save();
                    true
                } else if fs::write(&note, &new).is_ok() {
                    let m = vault::modified(&note).unwrap_or_else(SystemTime::now);
                    self.vault.upsert(note, new, m);
                    true
                } else {
                    false
                }
            }
            Target::Task(key) => {
                let file = self.vault.root.join(".nodex").join(STORE);
                let line = format!("{key}\t{}\t{what}\n", today());
                let r = fs::create_dir_all(self.vault.root.join(".nodex")).and_then(|_| {
                    use std::io::Write;
                    fs::OpenOptions::new().create(true).append(true).open(&file)?.write_all(line.as_bytes())
                });
                self.follow_cache = None;
                r.is_ok()
            }
        }
    }

    /// Los seguimientos de todas las tareas (los de las notas, por `^id`, y los sueltos).
    pub(super) fn follow_up_map(&mut self) -> Rc<FollowUps> {
        let file = self.vault.root.join(".nodex").join(STORE);
        let key = (self.vault.generation, vault::modified(&file));
        if let Some((k, m)) = &self.follow_cache {
            if *k == key {
                return m.clone();
            }
        }
        let mut map: FollowUps = HashMap::new();
        for n in self.vault.all_notes() {
            if !n.text.contains(lines::FOLLOW_MARK) {
                continue;
            }
            for (i, l) in n.text.lines().enumerate() {
                if let Some(id) = lines::id_of(l) {
                    let f = lines::follow_ups_after(&n.text, i);
                    if !f.is_empty() {
                        map.entry(id).or_default().extend(f);
                    }
                }
            }
        }
        for l in vault::read_text(&file).unwrap_or_default().lines() {
            let mut parts = l.splitn(3, '\t');
            if let (Some(k), Some(d), Some(t)) = (parts.next(), parts.next(), parts.next()) {
                map.entry(k.to_string()).or_default().push((d.to_string(), t.to_string()));
            }
        }
        for v in map.values_mut() {
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v.dedup();
        }
        let m = Rc::new(map);
        self.follow_cache = Some((key, m.clone()));
        m
    }

    /// Seguimiento de una tarea (desde Tareas o al marcarla hecha).
    pub(super) fn ask_task_follow_up(&mut self, t: &agenda::Task, done: bool) {
        let target = match (&t.id, &t.note) {
            (Some(id), Some(note)) => Target::Line { note: self.vault.root.join(format!("{note}.md")), id: Some(id.clone()), text: String::new() },
            _ => Target::Task(task_key(t)),
        };
        self.ask_follow_up(agenda::display_text(&t.text), target, done);
    }

    /// En el editor: «↳ fecha: » debajo de la línea `idx`, con el cursor ahí para escribir.
    pub(super) fn start_follow_up_line(&mut self, idx: usize) {
        let line = self.note.text.split('\n').nth(idx).unwrap_or("").trim_end_matches('\r').to_string();
        if line.trim().is_empty() || lines::is_follow_up(&line) {
            return;
        }
        let (new, at) = lines::add_follow_up(&self.note.text, idx, &today(), "");
        self.note.text = new;
        self.note.dirty = true;
        self.note.last_edit = Instant::now();
        let start: usize = self.note.text.split('\n').take(at).map(|l| l.chars().count() + 1).sum();
        let len = self.note.text.split('\n').nth(at).map_or(0, |l| l.trim_end_matches('\r').chars().count());
        self.pending_cursor = Some(start + len);
        self.focus_editor = true;
    }

    /// La pregunta «¿Qué se hizo?».
    pub(super) fn follow_up_window(&mut self, ctx: &egui::Context) {
        let Some(a) = &mut self.follow_ask else { return };
        let (mut save, mut close) = (false, false);
        let area = egui::Area::new(Id::new("seguimiento-tarea")).anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -44.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            Frame::new()
                .fill(Color32::WHITE)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(12)
                .shadow(egui::epaint::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(28) })
                .inner_margin(Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.set_width(480.0);
                    ui.horizontal(|ui| {
                        let (glyph, color, head) = if a.done { (icon::CHECK_CIRCLE, SUCCESS, "Hecha") } else { (icon::ARROW_ELBOW_DOWN_RIGHT, ACCENT, "Seguimiento") };
                        ui.label(RichText::new(glyph).size(16.0).color(color));
                        let title: String = a.title.chars().take(60).collect();
                        ui.add(egui::Label::new(RichText::new(format!("{head}: «{title}»")).font(theme::bold(14.0))).truncate());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add(egui::Button::new(RichText::new(icon::X).size(13.0).color(MUTED)).frame(false)).on_hover_text("Omitir (Esc)").clicked() {
                                close = true;
                            }
                        });
                    });
                    ui.label(RichText::new("¿Qué se hizo? Queda anotado debajo, con la fecha, y la IA lo tiene en cuenta.").size(12.5).color(MUTED));
                    ui.add_space(4.0);
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut a.text)
                            .hint_text("Por ejemplo: se pidió a Gerdau, llega el lunes")
                            .desired_width(f32::INFINITY)
                            .margin(Margin::symmetric(8, 5)),
                    );
                    if std::mem::take(&mut a.focus) {
                        r.request_focus();
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        save = true;
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let b = egui::Button::new(RichText::new("Anotar").color(Color32::WHITE)).fill(ACCENT);
                        if ui.add_enabled(!a.text.trim().is_empty(), b).clicked() {
                            save = true;
                        }
                        if ui.button(if a.done { "Omitir" } else { "Cancelar" }).clicked() {
                            close = true;
                        }
                    });
                });
        });
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            close = true;
        }
        // Al marcar hecha sin escribir nada, la pregunta se va sola al rato.
        if area.response.contains_pointer() || !a.text.is_empty() {
            a.at = Instant::now();
        } else if a.done && a.at.elapsed() > ASK_FOR {
            close = true;
        }
        ctx.request_repaint_after(Duration::from_secs(1));
        if save {
            let Some(a) = self.follow_ask.take() else { return };
            if self.add_follow_up(a.target.clone(), &a.text) {
                self.msg(format!("Seguimiento anotado en «{}»", a.title.chars().take(50).collect::<String>()));
                // En una tarea pendiente: ¿dice que ya se terminó?
                if !a.done {
                    self.check_done(a.target, a.title, a.text);
                }
            } else {
                self.msg("No se pudo anotar el seguimiento");
            }
        } else if close {
            self.follow_ask = None;
        }
    }
}

impl NotesApp {
    fn checked_file(&self) -> PathBuf {
        self.vault.root.join(".nodex").join(CHECKED)
    }

    /// Revisa (una vez) si el seguimiento `follow` dice que la tarea ya se terminó.
    pub(super) fn check_done(&mut self, target: Target, title: String, follow: String) {
        let key = format!("{:016x}", ai::fnv(&format!("{}\t{}", target_key(&target), follow.trim())));
        let file = self.checked_file();
        if self.done_checked.is_none() {
            self.done_checked = Some(vault::read_text(&file).unwrap_or_default().lines().map(|l| l.trim().to_string()).collect());
        }
        if !self.done_checked.as_mut().is_some_and(|s| s.insert(key.clone())) {
            return;
        }
        let _ = fs::create_dir_all(self.vault.root.join(".nodex")).and_then(|_| {
            use std::io::Write;
            fs::OpenOptions::new().create(true).append(true).open(&file)?.write_all(format!("{key}\n").as_bytes())
        });
        match looks_done(&follow) {
            Some(false) => {}
            quick if self.ai.is_err() => {
                if quick == Some(true) {
                    self.done_suggest = Some(DoneSuggestion { target, title, quote: follow, at: Instant::now() });
                }
            }
            _ => {
                let (cfg, ctx) = (self.cfg.clone(), self.ctx.clone());
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let user = format!("Tarea: «{title}»\nSeguimiento: «{follow}»\n¿El seguimiento dice que la tarea ya está terminada?");
                    let yes = match ai::complete(&cfg, DONE_SYSTEM, &user) {
                        Ok(a) => vault::fold(a.trim()).trim_start_matches(['«', '"', '*']).starts_with('s'),
                        // Sin conexión: las palabras claras.
                        Err(_) => looks_done(&follow) == Some(true),
                    };
                    let _ = tx.send(yes.then_some(DoneSuggestion { target, title, quote: follow, at: Instant::now() }));
                    ctx.request_repaint();
                });
                self.done_checks.push(rx);
            }
        }
    }

    /// ¿Sigue pendiente esa tarea (o esa línea)?
    fn still_pending(&self, target: &Target) -> bool {
        match target {
            Target::Line { id: Some(id), note, .. } => {
                let task = self.agenda.tasks().into_iter().find(|t| t.id.as_deref() == Some(id.as_str()));
                match task {
                    Some(t) => !t.done,
                    None => self.line_of(note, target).is_some_and(|l| lines::parse(&l).check == Some(false)),
                }
            }
            Target::Line { note, .. } => self.line_of(note, target).is_some_and(|l| lines::parse(&l).check == Some(false)),
            Target::Task(k) => self.agenda.tasks().iter().any(|t| !t.done && task_key(t) == *k),
        }
    }

    /// La línea de la nota a la que apunta `target`.
    fn line_of(&self, note: &Path, target: &Target) -> Option<String> {
        let text = if *note == self.note.path { self.note.text.clone() } else { vault::read_text(note).ok()? };
        let found = text.lines().find(|l| match target {
            Target::Line { id: Some(id), .. } => lines::id_of(l).as_deref() == Some(id.as_str()),
            Target::Line { text, .. } => l.trim_end() == text.trim_end() || line_title(l) == line_title(text),
            Target::Task(_) => false,
        });
        found.map(str::to_string)
    }

    /// Desde `poll`: las respuestas de la IA, y los seguimientos escritos en la nota abierta.
    pub(super) fn poll_done_checks(&mut self) {
        let mut ready = Vec::new();
        self.done_checks.retain(|rx| match rx.try_recv() {
            Ok(s) => {
                ready.extend(s);
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(_) => false,
        });
        for mut s in ready {
            if self.done_suggest.is_none() && self.still_pending(&s.target) {
                s.at = Instant::now();
                self.done_suggest = Some(s);
            }
        }
        // Lo escrito en la nota abierta («↳ hoy: …» bajo una tarea pendiente), cuando se deja de escribir.
        if self.view != View::Editor || self.note.dirty || self.note.last_edit.elapsed() < Duration::from_secs(6) {
            return;
        }
        let hash = ai::fnv(&self.note.text);
        if self.done_scanned == Some(hash) {
            return;
        }
        self.done_scanned = Some(hash);
        let today = today();
        let text = self.note.text.clone();
        for (i, l) in text.lines().enumerate() {
            if lines::parse(l).check != Some(false) {
                continue;
            }
            let Some((date, follow)) = lines::follow_ups_after(&text, i).pop() else { continue };
            if date == today && !follow.is_empty() {
                let target = Target::Line { note: self.note.path.clone(), id: lines::id_of(l), text: l.to_string() };
                self.check_done(target, line_title(l), follow);
            }
        }
    }

    /// Marca hecha la tarea (o la línea) de una sugerencia, sin volver a preguntar qué se hizo.
    fn mark_done(&mut self, target: &Target) {
        self.gcal_dirty = true;
        match target {
            Target::Line { id: Some(id), note, .. } => {
                if let Err(e) = self.agenda.set_done_by_id(id, true, &today()) {
                    self.msg(format!("No se pudo actualizar tareas.txt: {e}"));
                }
                let rel = crate::history::rel_of(&self.vault.root, note);
                self.sync_task_line(&rel, id, true);
            }
            Target::Line { note, .. } => {
                let Some(line) = self.line_of(note, target) else { return };
                let open = *note == self.note.path;
                let old = if open { self.note.text.clone() } else { vault::read_text(note).unwrap_or_default() };
                let Some(idx) = old.split('\n').position(|l| l.trim_end_matches('\r') == line) else { return };
                let new = editor::replace_line(&old, idx, &lines::toggle_check(&line));
                if open {
                    self.note.text = new;
                    self.note.dirty = true;
                    self.save();
                } else if fs::write(note, &new).is_ok() {
                    let m = vault::modified(note).unwrap_or_else(SystemTime::now);
                    self.vault.upsert(note.clone(), new, m);
                }
                self.complete_children(note, idx);
            }
            Target::Task(k) => {
                if let Some(t) = self.agenda.tasks().into_iter().find(|t| !t.done && task_key(t) == *k) {
                    if let Err(e) = self.agenda.toggle_task(&t.raw, &today()) {
                        self.msg(format!("No se pudo actualizar tareas.txt: {e}"));
                    }
                }
            }
        }
    }

    /// «¿La marco hecha?», abajo al centro (si no hay otra pregunta ahí).
    pub(super) fn done_suggest_window(&mut self, ctx: &egui::Context) {
        if self.follow_ask.is_some() {
            // Espera a que se responda la otra (el tiempo cuenta desde que se ve).
            if let Some(s) = &mut self.done_suggest {
                s.at = Instant::now();
            }
            return;
        }
        let Some(s) = &self.done_suggest else { return };
        let (mut yes, mut no) = (false, false);
        let area = egui::Area::new(Id::new("sugerir-hecha")).anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -44.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            Frame::new()
                .fill(Color32::WHITE)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(12)
                .shadow(egui::epaint::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(28) })
                .inner_margin(Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.set_width(480.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::SPARKLE).size(16.0).color(ACCENT));
                        let title: String = s.title.chars().take(60).collect();
                        ui.add(egui::Label::new(RichText::new(format!("¿Marco «{title}» como hecha?")).font(theme::bold(14.0))).truncate());
                    });
                    let quote: String = s.quote.chars().take(160).collect();
                    ui.label(RichText::new(format!("El seguimiento dice: «{quote}»")).size(13.0).color(MUTED));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let b = egui::Button::new(RichText::new(format!("{}  Marcar hecha", icon::CHECK)).color(Color32::WHITE)).fill(ACCENT);
                        if ui.add(b).clicked() {
                            yes = true;
                        }
                        if ui.button("No, sigue pendiente").clicked() {
                            no = true;
                        }
                    });
                });
        });
        // Con el mouse encima no se va; si no, a los 30 s (la tarea sigue pendiente).
        if let Some(s) = &mut self.done_suggest {
            if area.response.contains_pointer() {
                s.at = Instant::now();
            } else if s.at.elapsed() > ASK_FOR {
                no = true;
            }
        }
        ctx.request_repaint_after(Duration::from_secs(1));
        if yes {
            if let Some(s) = self.done_suggest.take() {
                self.mark_done(&s.target);
                self.msg(format!("«{}» marcada como hecha", s.title.chars().take(50).collect::<String>()));
            }
        } else if no {
            self.done_suggest = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Con la app: marcar la tarea madre en Tareas marca sus subtareas (en la nota y en Tareas).
    #[test]
    fn marking_the_parent_marks_its_subtasks() {
        let dir = std::env::temp_dir().join(format!("nodex-subtareas-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Docencia")).unwrap();
        let taller = dir.join("Docencia").join("Taller.md");
        fs::write(&taller, "- [ ] Preparar taller ^madre\n  [ ] Reservar sala ^hija1\n  - [ ] Imprimir guías\nOtra\n").unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.agenda.add_task(agenda::format_task("2026-10-01", "Preparar taller", "Docencia", None, "Docencia/Taller", Some("madre"))).unwrap();
        app.agenda.add_task(agenda::format_task("2026-10-01", "Reservar sala", "Docencia", None, "Docencia/Taller", Some("hija1"))).unwrap();
        let madre = app.agenda.tasks().into_iter().find(|t| t.text == "Preparar taller").unwrap();
        app.apply(Action::ToggleTask(madre.raw));
        assert_eq!(fs::read_to_string(&taller).unwrap(), "- [x] Preparar taller ^madre\n  [x] Reservar sala ^hija1\n  - [x] Imprimir guías\nOtra\n");
        assert!(app.agenda.tasks().iter().all(|t| t.done), "la subtarea también en Tareas");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_words_say_if_it_is_done() {
        assert_eq!(looks_done("Listo, se envió por correo"), Some(true));
        assert_eq!(looks_done("Ya se pagó"), Some(true));
        assert_eq!(looks_done("Planos entregados a Juan"), Some(true));
        assert_eq!(looks_done("Falta su visto bueno"), Some(false));
        assert_eq!(looks_done("Todavía no contestan"), Some(false));
        assert_eq!(looks_done("Hablé con Juan"), None);
    }

    /// Con la app (sin IA): un seguimiento que dice «listo» en una tarea pendiente propone
    /// marcarla; se revisa una sola vez; «Marcar hecha» la marca sin volver a preguntar.
    #[test]
    fn suggests_marking_done() {
        let dir = std::env::temp_dir().join(format!("nodex-sugerir-hecha-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Obra")).unwrap();
        let muro = dir.join("Obra").join("Muro.md");
        fs::write(&muro, "- [ ] Enviar planos ^pl4n0\n").unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.ai = Err("sin IA".into());
        app.agenda.add_task(agenda::format_task("2026-10-01", "Enviar planos", "Obra", None, "Obra/Muro", Some("pl4n0"))).unwrap();
        let t = app.agenda.tasks().into_iter().next().unwrap();
        app.ask_task_follow_up(&t, false);
        let ask = app.follow_ask.take().unwrap();
        app.add_follow_up(ask.target.clone(), "Listo, se enviaron por correo");
        app.check_done(ask.target.clone(), ask.title.clone(), "Listo, se enviaron por correo".into());
        let s = app.done_suggest.take().expect("propone marcarla hecha");
        // Una sola vez por seguimiento.
        app.check_done(ask.target.clone(), ask.title, "Listo, se enviaron por correo".into());
        assert!(app.done_suggest.is_none());
        app.mark_done(&s.target);
        assert!(app.agenda.tasks()[0].done);
        assert!(fs::read_to_string(&muro).unwrap().starts_with("- [x] Enviar planos ^pl4n0\n"));
        assert!(app.follow_ask.is_none(), "no vuelve a preguntar qué se hizo");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Con la app: al marcar una tarea, el seguimiento va bajo su línea; una tarea sin línea, a
    /// `.nodex/seguimiento.txt`. Los dos se ven en Tareas.
    #[test]
    fn follow_ups_go_under_the_line_or_to_the_store() {
        let dir = std::env::temp_dir().join(format!("nodex-seguimiento-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Obra")).unwrap();
        let muro = dir.join("Obra").join("Muro.md");
        fs::write(&muro, "- [ ] Pedir acero ^ab12c\nOtra cosa\n").unwrap();
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.agenda.add_task(agenda::format_task("2026-10-01", "Pedir acero", "Obra", None, "Obra/Muro", Some("ab12c"))).unwrap();
        app.agenda.add_task(agenda::format_task("2026-10-01", "Llamar a Ulma", "Obra", None, "", None)).unwrap();

        // Marcar hecha pregunta qué se hizo.
        let acero = app.agenda.tasks().into_iter().find(|t| t.text == "Pedir acero").unwrap();
        app.apply(Action::ToggleTask(acero.raw.clone()));
        let ask = app.follow_ask.take().expect("pregunta qué se hizo");
        assert!(ask.done);
        assert!(app.add_follow_up(ask.target, "Pedido a Gerdau, llega el lunes"));
        let day = today();
        assert_eq!(fs::read_to_string(&muro).unwrap(), format!("- [x] Pedir acero ^ab12c\n  ↳ {day}: Pedido a Gerdau, llega el lunes\nOtra cosa\n"));

        let ulma = app.agenda.tasks().into_iter().find(|t| t.text == "Llamar a Ulma").unwrap();
        app.ask_task_follow_up(&ulma, false);
        let ask = app.follow_ask.take().unwrap();
        assert!(app.add_follow_up(ask.target, "No contestan"));

        let map = app.follow_up_map();
        assert_eq!(map.get("ab12c"), Some(&vec![(day.clone(), "Pedido a Gerdau, llega el lunes".to_string())]));
        assert_eq!(map.get(&task_key(&ulma)), Some(&vec![(day.clone(), "No contestan".to_string())]));
        let _ = fs::remove_dir_all(&dir);
    }
}
