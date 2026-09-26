//! Inicio: un resumen de todo en una página. Anotar y preguntar rápido, lo de hoy, lo que la
//! IA tiene pendiente, las reuniones recientes con sus acuerdos abiertos, las notas recientes,
//! los espacios y la semana.


use super::*;

/// Una tarjeta con título e ícono.
fn card<R>(ui: &mut Ui, glyph: &str, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    // Ancho fijo: lo que no cabe se recorta, así las dos columnas no se montan.
    let inner = (ui.available_width() - 44.0).max(120.0);
    let r = Frame::new()
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(12)
        .inner_margin(Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(inner);
            ui.set_max_width(inner);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            ui.label(RichText::new(format!("{glyph}  {title}")).font(theme::bold(14.0)).color(TEXT));
            ui.add_space(6.0);
            add(ui)
        })
        .inner;
    ui.add_space(12.0);
    r
}

/// Fila clicable: título, dato a la derecha; Ctrl+clic abre en pestaña nueva.
fn note_row(ui: &mut Ui, glyph: &str, title: &str, right: &str, path: &Path) -> Option<Action> {
    let r = list_row(ui, glyph, title, right, false);
    let new_tab = ui.input(|i| i.modifiers.command) || r.middle_clicked();
    if r.clicked() || r.middle_clicked() {
        return Some(if new_tab { Action::OpenNewTab(path.to_path_buf()) } else { Action::Open(path.to_path_buf(), None) });
    }
    None
}

impl NotesApp {
    /// Anotar algo rápido: va al final de la nota de hoy del espacio actual (la IA lo ordena después).
    fn quick_capture(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let path = self.vault.note_path(&self.ws, &today());
        if path == self.note.path {
            let t = self.note.text.trim_end().to_string();
            self.note.text = if t.is_empty() { format!("{line}\n") } else { format!("{t}\n{line}\n") };
            self.note.dirty = true;
            self.save();
        } else {
            let t = vault::read_text(&path).unwrap_or_default();
            let t = t.trim_end();
            let new = if t.is_empty() { format!("{line}\n") } else { format!("{t}\n{line}\n") };
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            if let Err(e) = fs::write(&path, &new) {
                self.msg(format!("No se pudo anotar: {e}"));
                return;
            }
            if let Some(m) = vault::modified(&path) {
                self.vault.upsert(path.clone(), new, m);
            }
            self.touched.insert(path.clone());
        }
        self.msg(format!("Anotado en la nota de hoy de {}", self.ws));
    }

    pub(super) fn home_view(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let today_s = today();
        let tomorrow = (Local::now() + chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
        let week_ago = (Local::now() - chrono::Duration::days(6)).format("%Y-%m-%d").to_string();
        let tasks = self.agenda.tasks();
        let pending: Vec<&agenda::Task> = tasks.iter().filter(|t| !t.done).collect();
        let due = |t: &agenda::Task| t.due.clone().filter(|d| agenda::is_date(d));
        let overdue = pending.iter().filter(|t| due(t).is_some_and(|d| d < today_s)).count();
        let mut soon: Vec<&agenda::Task> = pending.iter().copied().filter(|t| due(t).is_some_and(|d| d <= tomorrow)).collect();
        soon.sort_by(|a, b| a.due.cmp(&b.due));
        let events: Vec<agenda::Event> = self.all_events().into_iter().filter(|e| e.date == today_s || e.date == tomorrow).collect();
        let asks = self.doubts.pending.len();
        let ideas = self.ideas.ready().count();
        let notes = self.vault.all_notes();
        let recent: Vec<(PathBuf, String, String, SystemTime)> =
            notes.iter().take(8).map(|n| (n.path.clone(), n.title.clone(), n.workspace.clone(), n.modified)).collect();
        let meetings: Vec<(PathBuf, String, SystemTime, usize)> = notes
            .iter()
            .filter(|n| is_meeting(&n.text))
            .take(5)
            .map(|n| {
                let rel = self.rel(&n.path);
                let open = pending.iter().filter(|t| t.note.as_deref() == Some(rel.as_str())).count();
                (n.path.clone(), n.title.clone(), n.modified, open)
            })
            .collect();
        let spaces: Vec<(String, usize, usize)> = self
            .vault
            .workspaces
            .iter()
            .map(|w| {
                let n = self.vault.notes_in(w).len();
                let p = pending.iter().filter(|t| t.project == *w).count();
                (w.clone(), n, p)
            })
            .collect();
        let written = notes
            .iter()
            .filter(|n| {
                let d: DateTime<Local> = n.modified.into();
                d.format("%Y-%m-%d").to_string() >= week_ago
            })
            .count();
        let done_week = tasks.iter().filter(|t| t.done && t.done_on.as_deref().is_some_and(|d| d >= week_ago.as_str())).count();
        let root = self.vault.root.clone();
        let mut capture: Option<String> = None;
        let mut question: Option<String> = None;

        Self::column(ui, "inicio", |ui, col_w| {
            let hour = chrono::Timelike::hour(&Local::now());
            let hello = if hour < 12 { "Buenos días" } else if hour < 20 { "Buenas tardes" } else { "Buenas noches" };
            ui.label(RichText::new(hello).font(theme::bold(26.0)));
            let mut status = vec![long_date(&today_s)];
            if !soon.is_empty() {
                status.push(format!("{} para hoy y mañana", plural(soon.len(), "tarea")));
            }
            if overdue > 0 {
                status.push(format!("{overdue} atrasada{}", if overdue == 1 { "" } else { "s" }));
            }
            if asks + ideas > 0 {
                status.push(format!("la IA tiene {}", plural(asks + ideas, "pregunta")));
            }
            ui.label(RichText::new(status.join("  ·  ")).size(13.0).color(MUTED));
            ui.add_space(14.0);

            // Anotar y preguntar, sin salir de aquí.
            let boxed = |ui: &mut Ui, text: &mut String, id: &str, hint: String| -> bool {
                let mut sent = false;
                Frame::new().fill(Color32::WHITE).stroke(Stroke::new(1.0, theme::BORDER)).corner_radius(10).inner_margin(Margin::symmetric(10, 7)).show(ui, |ui| {
                    let r = ui.add(egui::TextEdit::singleline(text).id(Id::new(id)).frame(Frame::NONE).hint_text(hint).desired_width(f32::INFINITY).font(FontId::proportional(14.5)));
                    sent = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !text.trim().is_empty();
                });
                sent
            };
            if boxed(ui, &mut self.home_capture, "inicio-anotar", format!("{}  Anota algo rápido… (va a la nota de hoy de {})", icon::NOTE_PENCIL, self.ws)) {
                capture = Some(std::mem::take(&mut self.home_capture));
            }
            ui.add_space(6.0);
            if boxed(ui, &mut self.home_question, "inicio-preguntar", format!("{}  Pregúntale a tus notas…", icon::CHAT_CIRCLE_TEXT)) {
                question = Some(std::mem::take(&mut self.home_question));
            }
            ui.add_space(16.0);

            let two = col_w > 620.0;
            let mut act_l = None;
            let mut act_r = None;
            let mut left = |ui: &mut Ui| {
                card(ui, icon::TRAY, "Hoy", |ui| {
                    if soon.is_empty() && events.is_empty() {
                        ui.label(RichText::new("Nada para hoy ni mañana.").color(MUTED));
                    }
                    for e in &events {
                        let when = if e.date == today_s { "hoy" } else { "mañana" };
                        let time = e.time.clone().map(|t| format!(" {t}")).unwrap_or_default();
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{}  {}  ·  {when}{time}", icon::CALENDAR_BLANK, e.title)).size(13.5));
                            if e.note.is_none() && e.date == today_s && ui.link(RichText::new(format!("{} Tomar notas", icon::NOTE_PENCIL)).size(12.0)).clicked() {
                                act_l = Some(Action::StartMeetingNamed(e.title.clone()));
                            }
                        });
                    }
                    for t in soon.iter().take(6) {
                        if let Some(a) = task_row(ui, t, &today_s, &root) {
                            act_l = Some(a);
                        }
                    }
                    if overdue > 0 {
                        ui.label(RichText::new(format!("{} {overdue} atrasada{}", icon::WARNING_CIRCLE, if overdue == 1 { "" } else { "s" })).size(13.0).color(RED));
                    }
                    if ui.link(RichText::new("Ver Hoy").size(12.5)).clicked() {
                        act_l = Some(Action::ShowTab(View::Today));
                    }
                });
                card(ui, icon::USERS, "Reuniones recientes", |ui| {
                    if meetings.is_empty() {
                        ui.label(RichText::new("Todavía no hay reuniones.").color(MUTED));
                    }
                    for (p, title, m, open) in &meetings {
                        let right = if *open > 0 { format!("{} abiertos · {}", open, short_date(*m)) } else { short_date(*m) };
                        if let Some(a) = note_row(ui, icon::USERS, title, &right, p) {
                            act_l = Some(a);
                        }
                    }
                    if ui.link(RichText::new(format!("{} Nueva reunión (Ctrl+R)", icon::PLUS)).size(12.5)).clicked() {
                        act_l = Some(Action::StartMeeting);
                    }
                });
                card(ui, icon::CLOCK, "Notas recientes", |ui| {
                    for (p, title, ws, m) in &recent {
                        let right = format!("{ws} · {}", short_date(*m));
                        if let Some(a) = note_row(ui, icon::FILE_TEXT, title, &right, p) {
                            act_l = Some(a);
                        }
                    }
                    ui.label(RichText::new("Ctrl+clic: abrir en otra pestaña").size(11.5).color(MUTED));
                });
            };
            let mut right = |ui: &mut Ui| {
                card(ui, icon::SPARKLE, "La IA", |ui| {
                    if asks + ideas == 0 {
                        ui.label(RichText::new("Nada pendiente: todo organizado.").color(MUTED));
                    } else {
                        if asks > 0 {
                            ui.label(RichText::new(format!("{} esperando tu respuesta", plural(asks, "pregunta"))).size(13.5));
                        }
                        if ideas > 0 {
                            ui.label(RichText::new(format!("{} de espacio nuevo", plural(ideas, "sugerencia"))).size(13.5));
                        }
                        if ui.link(RichText::new("Responder en Hoy").size(12.5)).clicked() {
                            act_r = Some(Action::ShowTab(View::Today));
                        }
                    }
                    if let Some(p) = &self.in_flight {
                        ui.label(RichText::new(format!("Organizando «{}»…", vault::stem(p))).size(12.5).color(ACCENT));
                    }
                });
                card(ui, icon::FOLDER_SIMPLE, "Espacios", |ui| {
                    for (w, n, p) in &spaces {
                        let right = if *p > 0 { format!("{} · {}", plural(*n, "nota"), plural(*p, "tarea")) } else { plural(*n, "nota") };
                        if list_row(ui, icon::FOLDER_SIMPLE, w, &right, *w == self.ws).clicked() {
                            act_r = Some(Action::SelectWorkspace(w.clone()));
                        }
                    }
                });
                card(ui, icon::CALENDAR_CHECK, "Esta semana", |ui| {
                    ui.label(RichText::new(format!("{} escritas  ·  {} hechas", plural(written, "nota"), plural(done_week, "tarea"))).size(13.5));
                    if ui.link(RichText::new("Revisión semanal").size(12.5)).clicked() {
                        act_r = Some(Action::ShowTab(View::Week));
                    }
                });
            };
            if two {
                ui.columns(2, |cols| {
                    left(&mut cols[0]);
                    right(&mut cols[1]);
                });
            } else {
                left(ui);
                right(ui);
            }
            if let Some(a) = act_l.or(act_r) {
                action = Some(a);
            }
        });
        if let Some(line) = capture {
            self.quick_capture(&line);
        }
        if let Some(q) = question {
            self.show_in_tab(View::Ask);
            self.ask(q);
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_capture_goes_to_todays_note() {
        let dir = std::env::temp_dir().join(format!("nodex-inicio-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("General")).unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.ws = "General".into();
        app.note = OpenNote::load(dir.join("General").join("Otra.md"));
        app.quick_capture("Llamar a Juan");
        app.quick_capture("Comprar pan");
        let p = dir.join("General").join(format!("{}.md", today()));
        assert_eq!(fs::read_to_string(&p).unwrap(), "Llamar a Juan\nComprar pan\n");
        assert!(app.touched.contains(&p), "la IA la organizará");
        let _ = fs::remove_dir_all(&dir);
    }
}
