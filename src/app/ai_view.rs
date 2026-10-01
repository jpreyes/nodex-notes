//! La ventana de la IA (✦, Ctrl+K): todo lo de la IA en un solo lugar.
//! - Conversar: preguntas sobre tus notas (ver ask_view).
//! - Preguntas: lo que la IA no supo con seguridad, sugerencias de espacios y duplicados.
//! - Lo que hizo: cada cambio que aplicó, con su detalle y Deshacer (activity.rs).

use super::*;
use crate::activity::{Entry, Kind};

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub(super) enum AiTab {
    #[default]
    Chat,
    Asks,
    Log,
}

/// Cuánto se ve el aviso de un cambio de la IA (no se va mientras el mouse está encima).
const TOAST_FOR: Duration = Duration::from_secs(15);

/// Aviso de lo que la IA acaba de hacer sola (organizar una nota, anotar correos).
pub(super) struct Toast {
    kind: Kind,
    entry: String,
    text: String,
    details: Vec<String>,
    at: Instant,
}

fn kind_icon(k: Kind) -> (&'static str, Color32) {
    match k {
        Kind::Organizar => (icon::SPARKLE, ACCENT),
        Kind::Correo => (icon::ENVELOPE_SIMPLE, ACCENT),
        Kind::Respuesta => (icon::CHAT_CIRCLE_TEXT, SUCCESS),
        Kind::Duplicado => (icon::COPY, SUCCESS),
        Kind::Espacio => (icon::FOLDER_SIMPLE_PLUS, SUCCESS),
        Kind::Error => (icon::WARNING_CIRCLE, RED),
        Kind::Sincronizar => (icon::ARROWS_MERGE, SUCCESS),
        Kind::Pedido => (icon::CHECK_CIRCLE, SUCCESS),
    }
}

/// "Hoy", "Ayer" o "Jueves 24 sep".
fn day_label(day: &str) -> String {
    let yesterday = (Local::now() - chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
    if day == today() {
        "Hoy".into()
    } else if day == yesterday {
        "Ayer".into()
    } else {
        let mut s = long_date(day);
        if let Some(first) = s.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        s
    }
}

impl NotesApp {
    /// Anota un cambio en "Lo que hizo". `undoable`: es el cambio que deshace el botón Deshacer.
    pub(super) fn log_ai(&mut self, kind: Kind, note: &str, text: String, details: Vec<String>, undoable: bool) {
        let entry = Entry {
            id: new_task_id(),
            at: Local::now().format("%Y-%m-%d %H:%M").to_string(),
            kind,
            note: note.to_string(),
            text,
            details,
            undone: false,
            rechazado: false,
        };
        let (text, details) = (entry.text.clone(), entry.details.clone());
        let id = self.activity.add(entry);
        if undoable {
            self.undo_entry = Some(id.clone());
        }
        let _ = self.activity.save(&self.vault.root);
        // Lo que hizo sola se avisa a la vista; lo que respondiste tú, no hace falta.
        if matches!(kind, Kind::Organizar | Kind::Correo | Kind::Error | Kind::Sincronizar) {
            self.toast = Some(Toast { kind, entry: id, text, details, at: Instant::now() });
        }
    }

    /// El aviso, arriba a la derecha: qué hizo, a dónde fue cada cosa, Ver y Deshacer.
    pub(super) fn toast_ui(&mut self, ctx: &egui::Context) -> Option<Action> {
        let t = self.toast.as_ref()?;
        if t.at.elapsed() > TOAST_FOR {
            self.toast = None;
            return None;
        }
        let mut action = None;
        let mut close = false;
        let can_undo = self.undo.as_ref().is_some_and(|u| u.at.elapsed() < UNDO_WINDOW) && self.undo_entry.as_deref() == Some(t.entry.as_str());
        let (glyph, color) = kind_icon(t.kind);
        let title = match t.kind {
            Kind::Correo => "La IA anotó correos en tu nota de hoy",
            Kind::Error => "La IA no pudo terminar",
            Kind::Sincronizar if t.text.contains("«Hoy»") => "Las notas del día, en un solo «Hoy»",
            Kind::Sincronizar => "Se juntaron dos versiones",
            _ => "La IA ordenó tu nota",
        };
        let r = egui::Area::new(Id::new("ia-aviso"))
            .anchor(Align2::RIGHT_TOP, egui::vec2(-18.0, 44.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(Color32::WHITE)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .corner_radius(12)
                    .shadow(egui::epaint::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(28) })
                    .inner_margin(Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.set_width(340.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(glyph).size(16.0).color(color));
                            ui.label(RichText::new(title).font(theme::bold(14.0)).color(TEXT));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let b = egui::Button::new(RichText::new(icon::X).size(13.0).color(MUTED)).frame(false);
                                if ui.add(b).on_hover_text("Cerrar el aviso (lo que hizo la IA se queda; se puede deshacer en «Lo que hizo»)").clicked() {
                                    close = true;
                                }
                            });
                        });
                        ui.label(RichText::new(&t.text).size(13.0).color(TEXT));
                        for d in t.details.iter().take(4) {
                            ui.label(RichText::new(format!("·  {d}")).size(12.5).color(MUTED));
                        }
                        if t.details.len() > 4 {
                            ui.label(RichText::new(format!("·  y {} más", t.details.len() - 4)).size(12.5).color(MUTED));
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.link(RichText::new("Ver lo que hizo").size(13.0)).clicked() {
                                action = Some(Action::ShowAi(AiTab::Log));
                                close = true;
                            }
                            if can_undo {
                                ui.add_space(8.0);
                                let b = egui::Button::new(RichText::new(format!("{} Deshacer", icon::ARROW_COUNTER_CLOCKWISE)).size(13.0));
                                if ui.add(b).on_hover_text("Vuelve a como estaba. La IA no lo intenta de nuevo con este texto, pero no aprende nada (para que aprenda: «No, gracias»)").clicked() {
                                    action = Some(Action::Undo);
                                    close = true;
                                }
                            }
                            // «No, gracias»: deshace y le enseña a no repetirlo.
                            if matches!(t.kind, Kind::Organizar | Kind::Correo) {
                                ui.add_space(4.0);
                                let b = egui::Button::new(RichText::new("No, gracias").size(13.0));
                                if ui.add(b).on_hover_text("Deshace esto y además la IA aprende a no repetirlo (te pregunta qué debería hacer)").clicked() {
                                    action = Some(Action::Reject(t.entry.clone()));
                                    close = true;
                                }
                            }
                        });
                    });
            });
        // Mientras se lee (mouse encima), no se va.
        if r.response.contains_pointer() {
            if let Some(t) = &mut self.toast {
                t.at = Instant::now();
            }
        }
        if close {
            self.toast = None;
        }
        ctx.request_repaint_after(Duration::from_millis(500));
        action
    }

    /// Notas que la IA todavía no organizó.
    pub(super) fn unorganized(&self) -> Vec<PathBuf> {
        let meeting = self.meeting.as_ref().map(|m| m.path.clone());
        self.vault
            .all_notes()
            .into_iter()
            .filter(|n| Some(&n.path) != meeting.as_ref() && !self.analyzed.contains(&n.hash()))
            .filter(|n| n.text.split_whitespace().count() >= 3)
            .map(|n| n.path.clone())
            .collect()
    }

    /// Cuántas notas faltan por organizar (se cuenta de nuevo solo si cambió algo).
    fn unorganized_count(&mut self) -> usize {
        let key = (self.vault.generation, self.analyzed.len());
        if let Some((k, n)) = self.unorganized_count.filter(|(k, _)| *k == key) {
            let _ = k;
            return n;
        }
        let n = self.unorganized().len();
        self.unorganized_count = Some((key, n));
        n
    }

    /// Preguntas y sugerencias de la IA que esperan respuesta.
    pub(super) fn pending_asks(&self) -> usize {
        self.doubts.pending.len() + self.ideas.ready().count() + self.suggestion_count
    }

    pub(super) fn ai_view(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let asks = self.pending_asks();
        let pending = self.unorganized_count();
        let w = ui.available_width();
        let col_w = (w - 64.0).clamp(200.0, COLUMN_MAX);

        // Encabezado fijo: estado de la IA y las tres secciones.
        ui.horizontal_top(|ui| {
            ui.add_space((w - col_w) / 2.0);
            ui.vertical(|ui| {
                ui.set_width(col_w);
                ui.add_space(22.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::SPARKLE).size(24.0).color(ACCENT));
                    ui.label(RichText::new("IA").font(theme::bold(26.0)));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let b = egui::Button::new(RichText::new(icon::GEAR).size(16.0).color(MUTED)).frame(false);
                        if ui.add(b).on_hover_text("Configurar la IA").clicked() {
                            action = Some(Action::OpenSettings(Section::Ai));
                        }
                    });
                });
                if let Some(a) = self.ai_status(ui, pending) {
                    action = Some(a);
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let asks_label = if asks > 0 { format!("{}  Preguntas  {asks}", icon::QUESTION) } else { format!("{}  Preguntas", icon::QUESTION) };
                    for (tab, label) in [
                        (AiTab::Chat, format!("{}  Conversar", icon::CHAT_CIRCLE_TEXT)),
                        (AiTab::Asks, asks_label),
                        (AiTab::Log, format!("{}  Lo que hizo", icon::CLOCK_COUNTER_CLOCKWISE)),
                    ] {
                        let selected = self.ai_tab == tab;
                        let color = if selected { ACCENT } else if tab == AiTab::Asks && asks > 0 { TEXT } else { MUTED };
                        let b = egui::Button::selectable(selected, RichText::new(label).size(14.0).color(color))
                            .corner_radius(8)
                            .min_size(egui::vec2(0.0, 30.0));
                        if ui.add(b).clicked() && !selected {
                            self.ai_tab = tab;
                            self.ask.focus = tab == AiTab::Chat;
                        }
                    }
                });
                ui.add_space(8.0);
            });
        });
        let r = ui.min_rect();
        ui.painter().hline(r.x_range(), ui.cursor().top(), Stroke::new(1.0, theme::BORDER));

        let body = match self.ai_tab {
            AiTab::Chat => self.chat_view(ui),
            AiTab::Asks => self.asks_view(ui),
            AiTab::Log => self.log_view(ui),
        };
        if body.is_some() {
            action = body;
        }
        if self.esc(ui) {
            action = Some(Action::CloseResults);
        }
        action
    }

    /// Modelo, qué está haciendo y cuántas notas faltan por organizar.
    /// IA incluida: pide al servidor cuánto va del mes (al abrir, cada 10 minutos y después de
    /// usar la IA, no más de una vez por minuto) y avisa una vez al pasar el 80 %.
    pub(super) fn poll_ai_usage(&mut self) {
        if !ai::is_included(&self.cfg) {
            self.ai_usage = None;
            return;
        }
        if let Some(rx) = &self.ai_usage_rx {
            if let Ok(r) = rx.try_recv() {
                self.ai_usage_rx = None;
                if let Ok(u) = &r {
                    if u.percent() >= 80 && self.ai_usage_warned != u.mes {
                        self.ai_usage_warned = u.mes.clone();
                        let when = long_date(&u.renueva);
                        let text = if u.percent() >= 100 {
                            format!("Llegaste al límite de IA incluida de este mes; vuelve el {when}. Tus notas se guardan igual.")
                        } else {
                            format!("Usaste el {} % de la IA incluida de este mes (vuelve a cero el {when}).", u.percent())
                        };
                        self.msg(text);
                    }
                }
                self.ai_usage = Some(r);
            }
            return;
        }
        let used_ai = self.ai.as_ref().is_ok_and(|a| a.busy) || self.in_flight.is_some();
        let every = if self.ai_usage.is_none() { Duration::ZERO } else if used_ai { Duration::from_secs(60) } else { Duration::from_secs(600) };
        if self.ai_usage_at.elapsed() >= every {
            self.ai_usage_at = Instant::now();
            self.ai_usage_rx = Some(ai::fetch_usage(&self.cfg, self.ctx.clone()));
        }
    }

    /// «IA incluida: 40 % del mes»
    pub(super) fn usage_label(&self) -> Option<(String, Color32)> {
        match self.ai_usage.as_ref()? {
            Ok(u) => {
                let color = if u.percent() >= 100 { RED } else if u.percent() >= 80 { WARN } else { MUTED };
                Some((format!("{} % del mes usado · vuelve a cero el {}", u.percent(), long_date(&u.renueva)), color))
            }
            Err(e) => Some((format!("no se pudo ver el uso: {e}"), RED)),
        }
    }

    fn ai_status(&self, ui: &mut Ui, pending: usize) -> Option<Action> {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let ai = match &self.ai {
                Ok(ai) => ai,
                Err(e) => {
                    ui.label(RichText::new(format!("No disponible: {e}")).size(13.0).color(RED));
                    if ui.link(RichText::new("Configurar").size(13.0)).clicked() {
                        action = Some(Action::OpenSettings(Section::Ai));
                    }
                    return;
                }
            };
            ui.label(RichText::new(&ai.label).size(13.0).color(MUTED));
            if let Some((text, color)) = self.usage_label() {
                ui.label(RichText::new(format!("({text})")).size(12.5).color(color));
            }
            // La prueba de la cuenta: cuántos días le quedan (en color de aviso los últimos 3).
            if let Some(Ok(info)) = self.acct.info.as_ref().filter(|_| ai::is_included(&self.cfg)) {
                if info.plan == "prueba" {
                    let color = if info.dias_prueba <= 3 { WARN } else { MUTED };
                    ui.label(RichText::new(format!("· {}", info.plan_label())).size(12.5).color(color));
                }
            }
            ui.label(RichText::new("·").size(13.0).color(MUTED));
            if let Some(p) = &self.in_flight {
                ui.spinner();
                let progress = if self.backlog_total > 0 {
                    format!(" ({}/{})", self.backlog_total - self.backlog.len(), self.backlog_total)
                } else {
                    String::new()
                };
                ui.label(RichText::new(format!("Organizando «{}»{progress}", vault::stem(p))).size(13.0).color(ACCENT));
                return;
            }
            if pending == 0 {
                ui.label(RichText::new(format!("{} Todo organizado", icon::CHECK_CIRCLE)).size(13.0).color(SUCCESS));
            } else {
                let what = if pending == 1 { "1 nota sin organizar".to_string() } else { format!("{pending} notas sin organizar") };
                ui.label(RichText::new(what).size(13.0).color(TEXT));
                let b = egui::Button::new(RichText::new(format!("{} Organizar ahora", icon::SPARKLE)).size(13.0));
                if ui.add(b).on_hover_text("La IA pone etiquetas, tareas y fechas, y lleva cada nota a su espacio").clicked() {
                    action = Some(Action::Organize);
                }
            }
            if !self.ai_auto {
                ui.label(RichText::new("· organizar sola: apagado").size(12.5).color(MUTED));
            }
            if let Some(e) = &self.ai_error {
                ui.label(RichText::new(format!("· último error: {e}")).size(12.5).color(RED));
            }
        });
        action
    }

    /// Preguntas de la IA y sugerencias de espacios, para responder aquí mismo.
    fn asks_view(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let asks = self.live_doubts();
        let ideas = self.ready_ideas();
        let learned = doubts::learned(&self.vault.root).len();
        let root = self.vault.root.clone();
        let mut reply = None;
        let mut space_reply = None;
        let mut find_dups = false;
        let suggested = self.suggestion_count > 0;
        Self::column_at(ui, "ai-asks", 18.0, |ui, _| {
            if let Some(a) = self.suggestions_ui(ui) {
                action = Some(a);
            }
            if asks.is_empty() && ideas.is_empty() && !suggested {
                ui.add_space(8.0);
                ui.label(RichText::new(format!("{} No hay preguntas pendientes.", icon::CHECK_CIRCLE)).size(15.0).color(SUCCESS));
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Cuando la IA no esté segura de algo (a qué proyecto va una nota, de qué fecha habla, si dos notas son la misma), te preguntará aquí. Lo que respondas lo recuerda.")
                        .size(13.0)
                        .color(MUTED),
                );
                ui.add_space(18.0);
            }
            if !ideas.is_empty() {
                ui.label(RichText::new("Sugerencias").font(theme::bold(15.0)).color(SUCCESS));
                ui.add_space(6.0);
                for i in &ideas {
                    if let Some(r) = self.idea_card(ui, i) {
                        space_reply = Some(r);
                    }
                }
                ui.add_space(10.0);
            }
            if !asks.is_empty() {
                ui.label(RichText::new("Sobre tus notas").font(theme::bold(15.0)).color(ACCENT));
                ui.add_space(6.0);
                for (d, n) in &asks {
                    let label = d.note.replace('/', " / ");
                    if let Some(r) = self.doubt_card(ui, d, *n, Some(label)) {
                        reply = Some(r);
                    }
                }
                ui.add_space(10.0);
            }
            ui.horizontal_wrapped(|ui| {
                if ui.button(format!("{} Buscar notas repetidas", icon::COPY)).on_hover_text("Revisa todas las notas y pregunta por las que parecen duplicadas").clicked() {
                    find_dups = true;
                }
                if learned > 0 {
                    let text = format!("{} Lo que aprendió ({})", icon::LIGHTBULB, learned);
                    if ui.link(RichText::new(text).size(12.5)).on_hover_text("aprendido.txt: lo que respondiste; la IA lo lee siempre (puedes editarlo)").clicked() {
                        action = Some(Action::OpenExternal(root.join(doubts::LEARNED_FILE)));
                    }
                }
            });
        });
        if let Some(r) = reply {
            self.handle_reply(r);
        }
        if let Some(r) = space_reply {
            self.handle_space_reply(r);
        }
        if find_dups {
            self.scan_duplicates();
        }
        action
    }

    /// "Lo que hizo": cada cambio de la IA, por día, con su detalle.
    fn log_view(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let root = self.vault.root.clone();
        let can_undo = self.undo.as_ref().is_some_and(|u| u.at.elapsed() < UNDO_WINDOW);
        let undo_id = self.undo_entry.clone().filter(|_| can_undo);
        let days = self.activity.by_day();
        let (n, bad) = super::trust::month_accuracy(&self.activity);
        Self::column_at(ui, "ai-log", 18.0, |ui, col_w| {
            if n > 0 {
                let pct = (n - bad) * 100 / n;
                let text = format!(
                    "Este mes: {} · {} · acertó en el {pct} %",
                    plural(n, "cambio de la IA"),
                    if bad == 1 { "1 deshecho o que no quisiste".to_string() } else { format!("{bad} deshechos o que no quisiste") }
                );
                ui.label(RichText::new(text).size(13.0).color(if pct >= 90 { SUCCESS } else if pct >= 70 { MUTED } else { WARN }));
                ui.add_space(6.0);
            }
            if days.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new("Todavía no hay cambios.").size(15.0).color(MUTED));
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Cuando la IA organice una nota, anote un correo, o respondas una de sus preguntas, verás aquí qué hizo y dónde.")
                        .size(13.0)
                        .color(MUTED),
                );
            }
            for (day, entries) in &days {
                ui.add_space(6.0);
                ui.label(RichText::new(day_label(day)).font(theme::bold(15.0)).color(if *day == today() { ACCENT } else { TEXT }));
                ui.add_space(4.0);
                for e in entries {
                    let (glyph, color) = kind_icon(e.kind);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.label(RichText::new(glyph).size(16.0).color(if e.undone { MUTED } else { color }));
                        ui.label(RichText::new(e.at.get(11..).unwrap_or("")).size(12.5).color(MUTED));
                        ui.vertical(|ui| {
                            ui.set_width(col_w - 90.0);
                            let mut job = LayoutJob::default();
                            let mut f = fmt(FontId::proportional(14.0), if e.undone { MUTED } else { TEXT });
                            if e.undone {
                                f.strikethrough = Stroke::new(1.0, MUTED);
                            }
                            job.append(&e.text, 0.0, f);
                            job.wrap.max_width = col_w - 90.0;
                            ui.label(job);
                            for d in e.details.iter().take(12) {
                                ui.label(RichText::new(format!("·  {d}")).size(12.5).color(MUTED));
                            }
                            if e.details.len() > 12 {
                                ui.label(RichText::new(format!("·  y {} más", e.details.len() - 12)).size(12.5).color(MUTED));
                            }
                            ui.horizontal(|ui| {
                                let path = root.join(format!("{}.md", e.note));
                                if !e.note.is_empty() && path.is_file() && ui.link(RichText::new(format!("{} Abrir {}", icon::FILE_TEXT, e.note)).size(12.5)).clicked() {
                                    action = Some(Action::Open(path, None));
                                }
                                if e.rechazado {
                                    ui.label(RichText::new("no lo quisiste").size(12.5).color(MUTED));
                                } else if e.undone {
                                    ui.label(RichText::new("deshecho").size(12.5).color(MUTED));
                                } else {
                                    if undo_id.as_deref() == Some(e.id.as_str()) {
                                        let b = egui::Button::new(RichText::new(format!("{} Deshacer", icon::ARROW_COUNTER_CLOCKWISE)).size(12.5));
                                        if ui.add(b).clicked() {
                                            action = Some(Action::Undo);
                                        }
                                    }
                                    if matches!(e.kind, Kind::Organizar | Kind::Correo) && ui.link(RichText::new("No, gracias").size(12.5)).on_hover_text("La IA lo tendrá en cuenta para no repetirlo").clicked() {
                                        action = Some(Action::Reject(e.id.clone()));
                                    }
                                }
                            });
                        });
                    });
                    ui.add_space(8.0);
                }
            }
        });
        action
    }
}
