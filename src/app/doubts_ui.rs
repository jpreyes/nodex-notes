//! Preguntas de la IA: la tarjeta con opciones y lo que pasa al responder.
//! Responder aplica la opción (mover, unir, fecha, etiquetas), anota lo aprendido
//! en aprendido.txt y se puede deshacer como cualquier cambio de la IA.

use super::*;
use crate::doubts::{self, Choice, Doubt};

/// Lo que se hizo en una tarjeta.
pub(super) enum Reply {
    Choose(String, usize),
    Text(String, String),
    Ignore(String),
    Open(PathBuf),
}

fn join_lines(lines: &[String], trailing: bool) -> String {
    let mut t = lines.join("\n");
    if trailing && !t.is_empty() {
        t.push('\n');
    }
    t
}

impl NotesApp {
    /// Número (1, 2…) de la nota de adentro a la que se refiere la pregunta, si todavía existe.
    pub(super) fn doubt_unit_number(text: &str, d: &Doubt) -> Option<usize> {
        let u = doubts::find_unit(text, &d.unit)?;
        lines::units(text).iter().position(|x| x.first == u.first).map(|i| i + 1)
    }

    /// Texto actual de una nota (el del editor si está abierta).
    fn note_text(&self, rel: &str) -> Option<String> {
        let path = self.vault.root.join(format!("{rel}.md"));
        if path == self.note.path {
            return Some(self.note.text.clone());
        }
        self.vault.get(&path).map(|n| n.text.clone()).or_else(|| vault::read_text(&path).ok())
    }

    /// Las preguntas pendientes que todavía apuntan a algo (su nota y su línea existen).
    pub(super) fn live_doubts(&self) -> Vec<(Doubt, usize)> {
        self.doubts
            .pending
            .iter()
            .filter_map(|d| {
                let text = self.note_text(&d.note)?;
                Some((d.clone(), Self::doubt_unit_number(&text, d)?))
            })
            .collect()
    }

    /// Quita las preguntas cuya nota o línea ya no existen.
    pub(super) fn prune_doubts(&mut self) {
        let live: Vec<String> = self.live_doubts().into_iter().map(|(d, _)| d.id).collect();
        let before = self.doubts.pending.len();
        self.doubts.pending.retain(|d| live.contains(&d.id));
        if self.doubts.pending.len() != before {
            let _ = self.doubts.save(&self.vault.root);
        }
    }

    /// Guarda las preguntas nuevas de un análisis (y descarta las viejas de esa nota que ya no aplican).
    pub(super) fn add_doubts(&mut self, old_rel: &str, new_rel: &str, new_text: &str, found: Vec<Doubt>) -> usize {
        for d in &mut self.doubts.pending {
            if d.note == old_rel {
                d.note = new_rel.to_string();
            }
        }
        self.doubts.pending.retain(|d| d.note != new_rel || doubts::find_unit(new_text, &d.unit).is_some());
        let mut added = 0;
        for d in found {
            if self.doubts.is_resolved(&d.unit) || doubts::find_unit(new_text, &d.unit).is_none() {
                continue;
            }
            self.doubts.pending.retain(|p| !(p.note == d.note && p.unit == d.unit));
            self.doubts.pending.push(d);
            added += 1;
        }
        let _ = self.doubts.save(&self.vault.root);
        added
    }

    pub(super) fn handle_reply(&mut self, r: Reply) {
        match r {
            Reply::Choose(id, i) => {
                let choice = self.doubts.pending.iter().find(|d| d.id == id).and_then(|d| d.choices.get(i).cloned());
                if let Some(c) = choice {
                    self.answer_doubt(&id, Some(c), None);
                }
            }
            Reply::Text(id, text) => self.answer_doubt(&id, None, Some(text)),
            Reply::Ignore(id) => {
                self.doubts.resolve(&id);
                let _ = self.doubts.save(&self.vault.root);
                self.msg("Pregunta descartada; no se volverá a preguntar");
            }
            Reply::Open(p) => self.open(p, None),
        }
        self.doubt_reply = None;
    }

    /// Aplica una respuesta: la opción elegida o una respuesta escrita.
    pub(super) fn answer_doubt(&mut self, id: &str, choice: Option<Choice>, free: Option<String>) {
        let Some(d) = self.doubts.pending.iter().find(|d| d.id == id).cloned() else { return };
        self.save();
        let root = self.vault.root.clone();
        let path = root.join(format!("{}.md", d.note));
        let Ok(text) = vault::read_text(&path) else {
            self.doubts.resolve(id);
            let _ = self.doubts.save(&root);
            return;
        };
        let snapshot = self.agenda.snapshot();
        let mut files: Vec<(PathBuf, Option<String>)> = vec![(path.clone(), Some(text.clone()))];
        let mut renamed: Option<(PathBuf, PathBuf)> = None;
        let mut done: Vec<String> = Vec::new();
        let mut written: Vec<(PathBuf, String)> = Vec::new();
        let trailing = text.ends_with('\n');

        if let Some(answer) = free.filter(|a| !a.trim().is_empty()) {
            // Respuesta escrita: se anota y la IA vuelve a revisar la nota con ese dato.
            let _ = doubts::learn(&root, &format!("Sobre «{}»: {}", d.unit, answer.trim()));
            self.analyzed.remove(&ai::fnv(&text));
            self.save_analyzed();
            self.touched.insert(path.clone());
            self.doubts.resolve(id);
            let _ = self.doubts.save(&root);
            self.msg("Anotado en aprendido.txt; la IA vuelve a revisar la nota");
            return;
        }
        let Some(c) = choice else { return };
        if doubts::find_unit(&text, &d.unit).is_none() {
            self.doubts.resolve(id);
            let _ = self.doubts.save(&root);
            self.msg("Esa línea cambió; la pregunta se descartó");
            return;
        }
        let ws_of_note = workspace_of(&path).unwrap_or_else(|| self.ws.clone());
        let mut t = text.clone();

        // Etiquetas y fecha, en la primera línea de la nota de adentro.
        if let Some(u) = doubts::find_unit(&t, &d.unit) {
            let mut ls: Vec<String> = t.lines().map(str::to_string).collect();
            let have: HashSet<String> = ls[u.first..=u.last].iter().flat_map(|l| tags::line_tags(l)).collect();
            let add: Vec<String> = c.etiquetas.iter().filter(|t| !have.contains(*t)).map(|t| format!("#{t}")).collect();
            if !add.is_empty() {
                ls[u.first] = lines::insert_words(&ls[u.first], &add.join(" "));
                done.push(add.join(" "));
            }
            if agenda::is_date(&c.fecha) && !u.block {
                let head = ls[u.first].clone();
                let task_id = lines::id_of(&head).unwrap_or_else(new_task_id);
                ls[u.first] = lines::set_meta(&lines::make_task(&head), Some(&c.fecha), Some(&task_id));
                let updated = self.agenda.set_due_by_id(&task_id, &c.fecha).unwrap_or(false);
                if !updated {
                    let line = agenda::format_task(&today(), &d.unit, &ws_of_note, Some(&c.fecha), &d.note, Some(&task_id));
                    let _ = self.agenda.add_task(line);
                }
                self.gcal_dirty = true;
                done.push(format!("fecha {}", long_date(&c.fecha)));
            }
            t = join_lines(&ls, trailing);
        }

        // Unirla, como detalle, a otra nota de adentro.
        if !c.de.is_empty() {
            if let (Some(u), Some(p)) = (doubts::find_unit(&t, &d.unit), doubts::find_unit(&t, &c.de)) {
                if u.first != p.first && !u.block && !p.block && !(p.first..=p.last).contains(&u.first) {
                    let mut ls: Vec<String> = t.lines().map(str::to_string).collect();
                    let mut moved: Vec<String> = ls.drain(u.first..=u.last).collect();
                    if lines::parse(&moved[0]).level == 0 {
                        moved[0] = lines::with_level(&moved[0], 1);
                    }
                    let at = if u.first > p.last { p.last + 1 } else { p.last + 1 - moved.len() };
                    ls.splice(at..at, moved);
                    t = join_lines(&ls, trailing);
                    done.push(format!("unida a «{}»", c.de));
                }
            }
        }

        // Moverla a otro espacio (y nota).
        let ws = self.vault.workspaces.iter().find(|w| w.eq_ignore_ascii_case(c.espacio.trim())).cloned();
        let capture = capture::is_capture(&vault::stem(&path));
        match ws {
            Some(ws) if capture && !c.nota.trim().is_empty() => {
                if let Some(u) = doubts::find_unit(&t, &d.unit) {
                    let mut ls: Vec<String> = t.lines().map(str::to_string).collect();
                    let chunk: Vec<String> = ls.drain(u.first..=u.last).collect();
                    let title = vault::sanitize(c.nota.trim());
                    let target = self
                        .vault
                        .notes_in(&ws)
                        .iter()
                        .find(|n| n.title.eq_ignore_ascii_case(&title))
                        .map(|n| n.path.clone())
                        .unwrap_or_else(|| self.vault.unique_path(&ws, &title));
                    let before = vault::read_text(&target).ok();
                    let body = chunk.join("\n");
                    let new_t = match before.as_deref().map(str::trim_end).filter(|b| !b.is_empty()) {
                        Some(b) if u.block => format!("{b}\n\n{body}\n"),
                        Some(b) => format!("{b}\n{body}\n"),
                        None => format!("{body}\n"),
                    };
                    if let Some(dir) = target.parent() {
                        let _ = fs::create_dir_all(dir);
                    }
                    if fs::write(&target, &new_t).is_ok() {
                        let ids: Vec<String> = chunk.iter().filter_map(|l| lines::id_of(l)).collect();
                        let rel = self.rel(&target);
                        let _ = self.agenda.retarget(None, &ids, &rel, &ws);
                        self.gcal_dirty = true;
                        files.push((target.clone(), before));
                        written.push((target.clone(), new_t));
                        done.push(format!("→ {ws}/{}", vault::stem(&target)));
                        let rest = join_lines(&ls, trailing);
                        t = if rest.trim().is_empty() { String::new() } else { rest.trim_start_matches('\n').to_string() };
                    }
                }
            }
            Some(ws) if !capture && ws != ws_of_note => renamed = Some((path.clone(), self.vault.unique_path(&ws, &vault::stem(&path)))),
            _ => {}
        }

        if !c.dato.trim().is_empty() {
            let _ = doubts::learn(&root, &c.dato);
            done.push("aprendido".into());
        }

        // Escribir la nota (o mandarla a la papelera si quedó vacía) y moverla si corresponde.
        if t.trim().is_empty() {
            let _ = self.vault.trash(&path);
        } else if t != text {
            let _ = fs::write(&path, &t);
            written.push((path.clone(), t.clone()));
        }
        if let Some((from, to)) = &renamed {
            if let Some(dir) = to.parent() {
                let _ = fs::create_dir_all(dir);
            }
            match fs::rename(from, to) {
                Ok(()) => {
                    let ws = workspace_of(to).unwrap_or_default();
                    let _ = self.agenda.retarget(Some(&d.note), &[], &self.rel(to), &ws);
                    self.gcal_dirty = true;
                    done.push(format!("→ {ws}"));
                    if self.note.path == *from {
                        self.note.path = to.clone();
                    }
                }
                Err(e) => {
                    renamed = None;
                    self.msg(format!("No se pudo mover la nota: {e}"));
                }
            }
        }

        // Estado: lo escrito aquí no se vuelve a analizar; la nota abierta se recarga.
        for (_, t) in &written {
            self.analyzed.insert(ai::fnv(t));
        }
        self.save_analyzed();
        self.doubts.resolve(id);
        let _ = self.doubts.save(&root);
        self.vault.scan();
        let current = self.note.path.clone();
        if written.iter().any(|(p, _)| *p == current) || current == path || renamed.as_ref().is_some_and(|(_, to)| *to == current) {
            self.note = OpenNote::load(current.clone());
            if let Some(ws) = workspace_of(&current) {
                self.ws = ws;
            }
        }
        self.undo = Some(Undo { files, renamed, agenda: snapshot, at: Instant::now() });
        let what = if done.is_empty() { "listo".to_string() } else { done.join(" · ") };
        self.msg(format!("Respuesta aplicada: {what}"));
    }

    /// La tarjeta de una pregunta. `note_label` = mostrar de qué nota es (en la vista Hoy).
    pub(super) fn doubt_card(&mut self, ui: &mut Ui, d: &Doubt, number: usize, note_label: Option<String>) -> Option<Reply> {
        let mut reply = None;
        Frame::new()
            .fill(Color32::from_rgb(244, 248, 254))
            .stroke(Stroke::new(1.0, Color32::from_rgb(200, 220, 246)))
            .corner_radius(10)
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    if note_label.is_none() {
                        ui.label(RichText::new(format!("{} La IA pregunta ·", icon::SPARKLE)).size(12.5).color(ACCENT));
                    }
                    ui.label(RichText::new(format!("Nota {number}")).size(12.5).color(MUTED));
                    if let Some(label) = &note_label {
                        let r = ui.add(egui::Label::new(RichText::new(format!("· {label}")).size(12.5).color(MUTED)).sense(Sense::click()));
                        if r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Abrir la nota").clicked() {
                            reply = Some(Reply::Open(self.vault.root.join(format!("{}.md", d.note))));
                        }
                    }
                });
                ui.label(RichText::new(&d.question).size(14.5).color(TEXT));
                ui.label(RichText::new(format!("«{}»", d.unit)).size(12.5).color(MUTED).italics());
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                    for (i, c) in d.choices.iter().enumerate() {
                        let b = egui::Button::new(RichText::new(&c.label).size(13.0)).fill(Color32::WHITE).corner_radius(8);
                        let mut tip = Vec::new();
                        if !c.espacio.is_empty() {
                            tip.push(if c.nota.is_empty() { format!("mover a {}", c.espacio) } else { format!("mover a {}/{}", c.espacio, c.nota) });
                        }
                        if !c.de.is_empty() {
                            tip.push(format!("unir a «{}»", c.de));
                        }
                        if !c.fecha.is_empty() {
                            tip.push(format!("fecha {}", long_date(&c.fecha)));
                        }
                        if !c.etiquetas.is_empty() {
                            tip.push(c.etiquetas.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" "));
                        }
                        if !c.dato.is_empty() {
                            tip.push(format!("recordar: {}", c.dato));
                        }
                        let r = ui.add(b);
                        let r = if tip.is_empty() { r } else { r.on_hover_text(tip.join(" · ")) };
                        if r.clicked() {
                            reply = Some(Reply::Choose(d.id.clone(), i));
                        }
                    }
                    ui.add_space(6.0);
                    if ui.link(RichText::new("Otra respuesta…").size(12.5)).clicked() {
                        self.doubt_reply = Some((d.id.clone(), String::new()));
                    }
                    if ui.link(RichText::new("Ignorar").size(12.5).color(MUTED)).on_hover_text("No volver a preguntar esto").clicked() {
                        reply = Some(Reply::Ignore(d.id.clone()));
                    }
                });
                if let Some((rid, text)) = &mut self.doubt_reply {
                    if *rid == d.id {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let r = ui.add(
                                egui::TextEdit::singleline(text)
                                    .hint_text("Escribe la respuesta (la IA la recordará)")
                                    .desired_width(ui.available_width() - 80.0),
                            );
                            if !r.has_focus() && !r.lost_focus() && text.is_empty() {
                                r.request_focus();
                            }
                            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                            if (ui.button("Enviar").clicked() || enter) && !text.trim().is_empty() {
                                reply = Some(Reply::Text(d.id.clone(), text.clone()));
                            }
                        });
                    }
                }
            });
        ui.add_space(8.0);
        reply
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La IA duda: deja una pregunta. Al responder se aplica la opción, se anota lo aprendido,
    /// la tarea sigue a su línea, no se vuelve a preguntar y Deshacer lo revierte.
    #[test]
    fn doubts_are_asked_answered_and_learned() {
        let dir = std::env::temp_dir().join(format!("nodex-dudas-app-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        for ws in ["General", "Docencia", "Consorcio"] {
            fs::create_dir_all(dir.join(ws)).unwrap();
        }
        let daily = dir.join("General").join("2026-09-25.md");
        let original = "Informe de trincheras\nDebo entregar la próxima semana el LaVet\n";
        fs::write(&daily, original).unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        let a: Analysis = serde_json::from_str(
            r#"{"unidades": [{"id": "L2", "etiquetas": ["lavet"]}],
              "tareas": [{"texto": "Entregar el LaVet", "fecha": "2026-10-02", "unidad": "L2"}],
              "dudas": [{"unidad": "L2", "pregunta": "¿De qué proyecto es el LaVet?",
                 "opciones": [{"texto": "Docencia", "espacio": "Docencia", "nota": "LaVet", "fecha": "2026-09-29", "dato": "LaVet es un proyecto de Docencia"},
                              {"texto": "Consorcio", "espacio": "Consorcio", "nota": "LaVet"}]}]}"#,
        )
        .unwrap();
        app.apply_analysis(daily.clone(), ai::fnv(original), a);
        // La nota queda con su casilla y la pregunta queda pendiente (y guardada).
        let d = fs::read_to_string(&daily).unwrap();
        assert!(d.contains("- [ ] Debo entregar la próxima semana el LaVet #lavet due:2026-10-02 ^"), "{d}");
        assert_eq!(app.doubts.pending.len(), 1);
        assert_eq!(doubts::Store::load(&dir).pending[0].unit, "Debo entregar la próxima semana el LaVet");
        assert_eq!(app.live_doubts()[0].1, 2, "es la nota 2");

        // Elegir "Docencia": la línea se va a Docencia/LaVet con la fecha nueva, y su tarea también.
        let id = app.doubts.pending[0].id.clone();
        app.handle_reply(Reply::Choose(id, 0));
        let moved = fs::read_to_string(dir.join("Docencia").join("LaVet.md")).unwrap();
        assert!(moved.starts_with("- [ ] Debo entregar la próxima semana el LaVet #lavet due:2026-09-29 ^"), "{moved}");
        assert_eq!(fs::read_to_string(&daily).unwrap(), "Informe de trincheras\n");
        let task = app.agenda.tasks().into_iter().next().unwrap();
        assert_eq!((task.due.as_deref(), task.note.as_deref(), task.project.as_str()), (Some("2026-09-29"), Some("Docencia/LaVet"), "Docencia"));
        assert_eq!(doubts::learned(&dir), vec!["LaVet es un proyecto de Docencia"]);
        assert!(app.doubts.pending.is_empty() && app.doubts.is_resolved("Debo entregar la próxima semana el LaVet"));

        // Una nueva pregunta sobre la misma línea ya no se hace.
        let again = doubts::from_ai(&moved, "Docencia/LaVet", &serde_json::from_str::<Analysis>(r#"{"dudas": [{"unidad": "L1", "pregunta": "¿?", "opciones": ["a"]}]}"#).unwrap(), "2026-09-25", || "x".into());
        assert_eq!(app.add_doubts("Docencia/LaVet", "Docencia/LaVet", &moved, again), 0);

        // Deshacer devuelve la línea a la nota del día.
        app.undo_ai();
        assert!(fs::read_to_string(&daily).unwrap().contains("Debo entregar la próxima semana el LaVet"));
        assert!(!dir.join("Docencia").join("LaVet.md").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
