//! La IA en todas partes: un panel a la derecha (Ctrl+J, o el botón ✦ abajo a la derecha) para
//! conversar sin dejar lo que se está haciendo. Sabe qué se está mirando (la nota abierta o la
//! vista) y propone pedidos para eso. Es la misma conversación de la ventana de la IA, y lo que
//! se le pide hacer (tareas, eventos, anotar, seguimientos) se hace igual, con Deshacer.

use super::*;

/// Ancho del panel (se puede cambiar arrastrando su borde).
pub(super) const WIDTH: f32 = 380.0;

impl NotesApp {
    /// Qué se está mirando, dicho para la IA y para el panel: (etiqueta, nota abierta).
    pub(super) fn side_context(&self) -> (String, Option<PathBuf>) {
        match &self.view {
            View::Editor if self.note.text.trim().is_empty() => ("una nota nueva, vacía".into(), None),
            View::Editor => (format!("la nota «{}»", display_title(&self.note.title)), Some(self.note.path.clone())),
            View::Home => ("Inicio (Tu día)".into(), None),
            View::Tasks => ("Tareas".into(), None),
            View::Agenda => match &self.agenda_day {
                Some(d) => (format!("la Agenda, el día {}", long_date(d)), None),
                None => ("la Agenda".into(), None),
            },
            View::Week => ("la revisión semanal".into(), None),
            View::Mail => ("los correos".into(), None),
            View::Bloc => ("el Bloc".into(), None),
            View::Notes => ("la lista de notas".into(), None),
            View::Archive => ("las notas archivadas".into(), None),
            View::Trash => ("la papelera".into(), None),
            View::Tag(t) => (format!("la etiqueta #{t}"), None),
            View::Ai => ("la IA".into(), None),
        }
    }

    /// Pedidos rápidos según lo que se mira.
    fn side_suggestions(&self) -> &'static [&'static str] {
        match &self.view {
            View::Editor if !self.note.text.trim().is_empty() => &["Resume esta nota", "¿Qué tareas salen de aquí?", "¿Qué falta por hacer de esto?", "Agenda las fechas que dice"],
            View::Tasks => &["¿Qué hago primero hoy?", "¿Qué está atrasado y qué hago con eso?", "Agrupa mis tareas por proyecto"],
            View::Agenda | View::Week => &["¿Qué tengo esta semana?", "¿Qué tengo mañana?", "¿Tengo algo que choque?"],
            View::Mail => &["¿Qué correos debo responder?", "¿Qué me pidieron por correo esta semana?"],
            _ => &["¿Qué tengo para hoy?", "¿Qué está atrasado?", "¿Qué tengo esta semana?"],
        }
    }

    /// Pregunta desde el panel, con lo que se está mirando como contexto.
    pub(super) fn ask_here(&mut self, q: String) {
        self.ask_context = Some(self.side_context());
        self.ask(q);
        self.ask_context = None;
    }

    pub(super) fn toggle_side_ai(&mut self) {
        self.side_ai = !self.side_ai;
        self.side_focus = self.side_ai;
    }

    /// El botón ✦ abajo a la derecha, cuando el panel está cerrado.
    pub(super) fn side_ai_button(&mut self, ctx: &egui::Context) {
        if self.side_ai || self.view == View::Ai {
            return;
        }
        let busy = self.ask.busy();
        let mut open = false;
        egui::Area::new(Id::new("ia-boton")).anchor(Align2::RIGHT_BOTTOM, egui::vec2(-22.0, -40.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            let glyph = if busy { icon::HOURGLASS_MEDIUM } else { icon::SPARKLE };
            let b = egui::Button::new(RichText::new(glyph).size(20.0).color(Color32::WHITE)).fill(ACCENT).corner_radius(22).min_size(egui::vec2(44.0, 44.0));
            if ui.add(b).on_hover_text("Pregúntale o pídele algo a la IA sobre lo que estás viendo (Ctrl+J)").clicked() {
                open = true;
            }
        });
        if open {
            self.toggle_side_ai();
        }
    }

    /// El panel de la derecha: la conversación, pedidos rápidos y el campo para escribir.
    pub(super) fn side_ai_panel(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let (label, _) = self.side_context();
        let busy = self.ask.busy();
        let tasks_now = self.agenda.tasks();
        let (mut send, mut close, mut big, mut new_chat) = (None, false, false, false);

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} IA", icon::SPARKLE)).font(theme::bold(16.0)).color(ACCENT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let small = |g: &str| egui::Button::new(RichText::new(g).size(15.0).color(MUTED)).frame(false);
                if ui.add(small(icon::X)).on_hover_text("Cerrar (Ctrl+J)").clicked() {
                    close = true;
                }
                if ui.add(small(icon::ARROWS_OUT_SIMPLE)).on_hover_text("Abrir en la ventana de la IA").clicked() {
                    big = true;
                }
                if !busy && !self.ask.turns.is_empty() && ui.add(small(icon::PLUS)).on_hover_text("Nueva conversación (esta queda guardada)").clicked() {
                    new_chat = true;
                }
            });
        });
        ui.label(RichText::new(format!("{} Viendo {label}", icon::EYE)).size(12.0).color(MUTED));
        ui.add_space(6.0);
        let r = ui.max_rect();
        ui.painter().hline(r.x_range(), ui.cursor().top(), Stroke::new(1.0, theme::BORDER));
        ui.add_space(4.0);

        // La conversación (la misma de la ventana de la IA).
        let reserve = 150.0;
        egui::ScrollArea::vertical()
            .id_salt("ia-lado")
            .max_height((ui.available_height() - reserve).max(80.0))
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                let w = ui.available_width();
                ui.set_width(w);
                if self.ask.turns.is_empty() {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Pregúntale o pídele algo sobre lo que estás viendo: resumir, sacar tareas, agendar, anotar un seguimiento… Lo que haga se puede deshacer.")
                            .size(13.0)
                            .color(MUTED),
                    );
                }
                for turn in &self.ask.turns {
                    ui.add_space(8.0);
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        Frame::new().fill(Color32::WHITE).corner_radius(12).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
                            ui.set_max_width(w * 0.85);
                            ui.add(egui::Label::new(RichText::new(&turn.question).size(13.5).color(TEXT)).wrap());
                        });
                    });
                    ui.add_space(6.0);
                    match &turn.answer {
                        None => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(RichText::new(&turn.progress).size(13.0).color(MUTED));
                            });
                        }
                        Some(Err(e)) => {
                            ui.label(RichText::new(format!("No se pudo responder: {e}")).size(13.0).color(RED));
                        }
                        Some(Ok(_)) => {
                            if let Some(a) = self.answer_ui(ui, turn, &tasks_now) {
                                action = Some(a);
                            }
                            if !turn.done.is_empty() {
                                ui.add_space(4.0);
                                Frame::new().fill(Color32::from_rgb(232, 245, 234)).corner_radius(8).inner_margin(Margin::symmetric(8, 5)).show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    for d in &turn.done {
                                        ui.add(egui::Label::new(RichText::new(format!("{} {d}", icon::CHECK)).size(12.5).color(Color32::from_rgb(46, 98, 56))).wrap());
                                    }
                                    let can_undo = turn.entry.is_some() && self.undo_entry == turn.entry && self.undo.as_ref().is_some_and(|u| u.at.elapsed() < UNDO_WINDOW);
                                    if can_undo && ui.button(RichText::new(format!("{} Deshacer", icon::ARROW_COUNTER_CLOCKWISE)).size(12.5)).clicked() {
                                        action = Some(Action::Undo);
                                    }
                                });
                            }
                        }
                    }
                }
                ui.add_space(8.0);
            });

        // Pedidos rápidos para lo que se está viendo.
        ui.add_space(6.0);
        if !busy {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
                for s in self.side_suggestions() {
                    if ui.add(egui::Button::new(RichText::new(*s).size(12.0)).corner_radius(12)).clicked() {
                        send = Some(s.to_string());
                    }
                }
            });
            ui.add_space(6.0);
        }

        // Escribir: Enter envía, Shift+Enter hace otra línea.
        let id = Id::new("ia-lado-campo");
        let focused = ui.memory(|m| m.has_focus(id));
        let enter = focused && !ui.input(|i| i.modifiers.shift) && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
        let mut submit = false;
        Frame::new()
            .fill(Color32::WHITE)
            .stroke(Stroke::new(1.0, if focused { ACCENT } else { theme::BORDER }))
            .corner_radius(10)
            .inner_margin(Margin::symmetric(8, 5))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::multiline(&mut self.side_input)
                            .id(id)
                            .frame(Frame::NONE)
                            .hint_text("Pregunta o pide algo…")
                            .desired_rows(2)
                            .desired_width(ui.available_width() - 30.0)
                            .font(FontId::proportional(14.0)),
                    );
                    if std::mem::take(&mut self.side_focus) {
                        r.request_focus();
                    }
                    let color = if busy || self.side_input.trim().is_empty() { MUTED } else { ACCENT };
                    if ui.add(egui::Button::new(RichText::new(icon::PAPER_PLANE_RIGHT).size(17.0).color(color)).frame(false)).on_hover_text("Enviar (Enter)").clicked() {
                        submit = true;
                    }
                });
            });
        if (enter || submit) && !busy && !self.side_input.trim().is_empty() {
            send = Some(std::mem::take(&mut self.side_input));
            self.side_focus = true;
        }
        ui.add_space(8.0);

        if close {
            self.side_ai = false;
        }
        if big {
            action = Some(Action::ShowAi(ai_view::AiTab::Chat));
        }
        if new_chat {
            self.new_chat();
        }
        if let Some(q) = send {
            self.ask_here(q);
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Desde la IA de la derecha, la pregunta dice qué se está mirando y la nota abierta va
    /// siempre, aunque haya tantas notas que solo se manden las más relevantes.
    #[test]
    fn asks_about_what_is_on_screen() {
        let dir = std::env::temp_dir().join(format!("nodex-ia-lado-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("Obra")).unwrap();
        // Muchas notas que hablan de otra cosa, y la que se está mirando.
        let relleno = "Informe de la losa del edificio, cubicación y planos del segundo piso. ".repeat(10);
        for i in 0..200 {
            fs::write(dir.join("Obra").join(format!("Losa {i}.md")), format!("{relleno}\n")).unwrap();
        }
        let muro = dir.join("Obra").join("Muro de contención.md");
        fs::write(&muro, "Revisar el drenaje del muro con el inspector\n").unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.open_in_tab(muro.clone(), None);
        let (label, note) = app.side_context();
        assert_eq!((label.as_str(), note.as_ref()), ("la nota «Muro de contención»", Some(&muro)));

        app.ask_context = Some(app.side_context());
        let (input, turn) = app.build_request("Resume esta nota de la losa".into(), Vec::new(), None);
        app.ask_context = None;
        let doc = input.docs.iter().find(|d| d.path == muro).expect("la nota abierta va siempre");
        assert!(input.docs.len() <= crate::ask::MAX_CANDIDATES + 1);
        assert!(input.question.contains(&format!("estoy viendo la nota «Muro de contención» [{}]", doc.key)), "{}", input.question);
        assert_eq!(turn.question, "Resume esta nota de la losa", "lo que se ve es solo la pregunta");
        let _ = fs::remove_dir_all(&dir);
    }
}
