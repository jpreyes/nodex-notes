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

use super::*;
use std::rc::Rc;

pub(super) const STORE: &str = "seguimiento.txt";

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
        let area = egui::Area::new(Id::new("seguimiento")).anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -44.0)).order(egui::Order::Foreground).show(ctx, |ui| {
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
        } else if a.done && a.at.elapsed() > Duration::from_secs(25) {
            close = true;
        }
        ctx.request_repaint_after(Duration::from_secs(1));
        if save {
            let Some(a) = self.follow_ask.take() else { return };
            if self.add_follow_up(a.target, &a.text) {
                self.msg(format!("Seguimiento anotado en «{}»", a.title.chars().take(50).collect::<String>()));
            } else {
                self.msg("No se pudo anotar el seguimiento");
            }
        } else if close {
            self.follow_ask = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
