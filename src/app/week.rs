//! Revisión semanal: lo que pasó en los últimos 7 días y lo que viene en los próximos 7,
//! con un resumen de la IA (a pedido) que cita sus fuentes y se puede guardar como nota.

use super::ask_view::{plain, Turn};
use super::*;
use crate::ask;
use std::sync::mpsc::Receiver;

#[derive(Default)]
pub(super) struct WeekState {
    pub(super) turn: Option<Turn>,
    pub(super) rx: Option<Receiver<ask::Msg>>,
}

/// "2026-W39": la semana (ISO) de hoy.
pub(super) fn week_label() -> String {
    let w = Local::now().iso_week();
    format!("{}-W{:02}", w.year(), w.week())
}

fn day(offset: i64) -> String {
    (Local::now() + chrono::Duration::days(offset)).format("%Y-%m-%d").to_string()
}

/// "19 – 25 sep"
fn range_label(from: &str, to: &str) -> String {
    let parse = |d: &str| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok();
    match (parse(from), parse(to)) {
        (Some(a), Some(b)) if a.month() == b.month() => format!("{} – {} {}", a.day(), b.day(), crate::i18n::meses()[b.month0() as usize]),
        (Some(a), Some(b)) => format!("{} {} – {} {}", a.day(), crate::i18n::meses()[a.month0() as usize], b.day(), crate::i18n::meses()[b.month0() as usize]),
        _ => format!("{from} – {to}"),
    }
}

fn metric(ui: &mut Ui, value: usize, label: &str, color: Color32) {
    Frame::new().fill(BG_SIDE()).corner_radius(8).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(value.to_string()).font(theme::bold(20.0)).color(color));
            ui.label(RichText::new(label).size(12.5).color(MUTED()));
        });
    });
}

impl NotesApp {
    /// ¿Toca ofrecer la revisión de esta semana? (desde el lunes, hasta que se abra).
    pub(super) fn week_pending(&self) -> bool {
        self.week_seen != week_label()
    }

    /// Pide a la IA el resumen de la semana (solo con las notas editadas en los últimos 7 días).
    pub(super) fn start_week_summary(&mut self) {
        if self.week.rx.is_some() {
            return;
        }
        self.save();
        let (from, to) = (day(-6), today());
        let question = format!(
            "Haz mi revisión semanal del {from} al {to}. Agrupa por espacio (proyecto): qué se avanzó, qué se decidió en reuniones y qué quedó pendiente, en pocas líneas cada uno. Después, las tareas atrasadas o sin fecha que conviene resolver. Termina con 3 prioridades concretas para la próxima semana. Sé breve y cita las fuentes."
        );
        let (input, mut turn) = self.build_request(question, Vec::new(), Some(&from));
        if input.docs.is_empty() && input.tasks.is_empty() {
            turn.answer = Some(Err(t!("no hay notas ni tareas de esta semana para resumir").into()));
            self.week.turn = Some(turn);
            return;
        }
        turn.progress = tf!("Leyendo {n} de la semana…", n = plural(input.docs.len(), "nota"));
        self.week.rx = Some(ask::start(&self.cfg, input, self.ctx.clone()));
        self.week.turn = Some(turn);
    }

    pub(super) fn week_view(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        if self.week_pending() {
            self.week_seen = week_label();
            self.save_estado();
        }
        let (from, to, next) = (day(-6), today(), day(7));
        let tasks = self.agenda.tasks();
        let done: Vec<&agenda::Task> = tasks.iter().filter(|t| t.done && t.done_on.as_deref().is_some_and(|d| d >= from.as_str())).collect();
        let pending: Vec<&agenda::Task> = tasks.iter().filter(|t| !t.done).collect();
        let dated = |t: &&agenda::Task| t.due.clone().filter(|d| agenda::is_date(d));
        let overdue: Vec<&agenda::Task> = pending.iter().copied().filter(|t| dated(t).is_some_and(|d| d < to)).collect();
        let undated: Vec<&agenda::Task> = pending.iter().copied().filter(|t| dated(t).is_none()).collect();
        let mut coming: Vec<&agenda::Task> = pending.iter().copied().filter(|t| dated(t).is_some_and(|d| d >= to && d <= next)).collect();
        coming.sort_by(|a, b| a.due.cmp(&b.due));
        let events: Vec<agenda::Event> = self.all_events().into_iter().filter(|e| e.date >= to && e.date <= next).collect();
        let notes: Vec<(PathBuf, String, String, bool)> = self
            .vault
            .all_notes()
            .into_iter()
            .filter(|n| {
                let d: DateTime<Local> = n.modified.into();
                d.format("%Y-%m-%d").to_string() >= from
            })
            .map(|n| (n.path.clone(), n.title.clone(), n.workspace.clone(), is_meeting(&n.text)))
            .collect();
        let meetings = notes.iter().filter(|n| n.3).count();
        let asks = self.doubts.pending.len() + self.ideas.ready().count();
        let root = self.vault.root.clone();
        let follows = self.follow_up_map();
        let busy = self.week.rx.is_some();
        let mut ask_ai = false;
        let mut save = false;

        Self::column(ui, "week", |ui, _| {
            view_header(ui, t!("Revisión semanal"), &tf!("Últimos 7 días ({range}) y los próximos 7", range = range_label(&from, &to)));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                metric(ui, notes.len(), t!("notas escritas"), TEXT());
                metric(ui, done.len(), t!("tareas hechas"), SUCCESS());
                metric(ui, meetings, t!("reuniones"), TEXT());
                metric(ui, overdue.len(), t!("atrasadas"), if overdue.is_empty() { TEXT() } else { RED() });
                metric(ui, undated.len(), t!("sin fecha"), TEXT());
            });
            ui.add_space(16.0);

            // Resumen de la IA (a pedido).
            ui.label(RichText::new(format!("{} {}", icon::SPARKLE, t!("Resumen de la semana"))).font(theme::bold(15.0)).color(ACCENT()));
            ui.add_space(4.0);
            match &self.week.turn {
                None => {
                    ui.label(RichText::new(t!("La IA lee las notas de la semana y arma un resumen por proyecto, con lo pendiente y 3 prioridades para la próxima.")).size(13.0).color(MUTED()));
                    ui.add_space(4.0);
                    if ui.add_enabled(self.ai.is_ok(), egui::Button::new(format!("{} {}", icon::SPARKLE, t!("Hacer el resumen")))).clicked() {
                        ask_ai = true;
                    }
                }
                Some(turn) => match &turn.answer {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new(&turn.progress).color(MUTED()));
                        });
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(tf!("No se pudo hacer el resumen: {e}", e = e)).color(RED()));
                        if ui.button(t!("Intentar de nuevo")).clicked() {
                            ask_ai = true;
                        }
                    }
                    Some(Ok(_)) => {
                        if let Some(a) = self.answer_ui(ui, turn, &tasks) {
                            action = Some(a);
                        }
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let small = |t: String| egui::Button::new(RichText::new(t).size(12.5).color(MUTED())).frame(false);
                            if ui.add(small(format!("{} {}", icon::FLOPPY_DISK, t!("Guardar como nota")))).clicked() {
                                save = true;
                            }
                            if ui.add(small(format!("{} {}", icon::COPY, t!("Copiar")))).clicked() {
                                ui.ctx().copy_text(plain(turn));
                            }
                            if !busy && ui.add(small(format!("{} {}", icon::ARROW_CLOCKWISE, t!("Rehacer")))).clicked() {
                                ask_ai = true;
                            }
                        });
                    }
                },
            }
            ui.add_space(18.0);

            let section = |ui: &mut Ui, title: &str, color: Color32, list: &[&agenda::Task], limit: usize| -> Option<Action> {
                let mut action = None;
                if list.is_empty() {
                    return None;
                }
                ui.label(RichText::new(title).font(theme::bold(15.0)).color(color));
                ui.add_space(4.0);
                for t in list.iter().take(limit) {
                    if let Some(a) = task_row(ui, t, &to, &root, &follows) {
                        action = Some(a);
                    }
                }
                if list.len() > limit && ui.link(RichText::new(tf!("y {n} más en Tareas", n = list.len() - limit)).size(12.5)).clicked() {
                    action = Some(Action::Show(View::Tasks));
                }
                ui.add_space(14.0);
                action
            };
            if let Some(a) = section(ui, &format!("{} {}", icon::WARNING_CIRCLE, t!("Atrasadas")), RED(), &overdue, 20) {
                action = Some(a);
            }
            if let Some(a) = section(ui, t!("Próximos 7 días"), ACCENT(), &coming, 20) {
                action = Some(a);
            }
            if !events.is_empty() {
                ui.label(RichText::new(t!("Agenda de los próximos 7 días")).font(theme::bold(15.0)));
                ui.add_space(4.0);
                let marks = self.event_marks();
                for e in &events {
                    let state = marks.get(&super::day_items::event_key(e)).map(String::as_str);
                    if state == Some("tarea") || state == Some("quitado") {
                        continue;
                    }
                    if let Some(a) = super::day_items::event_row(ui, e, state == Some("hecho"), &follows, true) {
                        action = Some(a);
                    }
                }
                ui.add_space(14.0);
            }
            if let Some(a) = section(ui, t!("Sin fecha (¿les pones una?)"), TEXT(), &undated, 10) {
                action = Some(a);
            }
            if let Some(a) = section(ui, &format!("{} {}", icon::CHECK_CIRCLE, t!("Hechas esta semana")), SUCCESS(), &done, 30) {
                action = Some(a);
            }
            if !notes.is_empty() {
                ui.label(RichText::new(t!("Notas de la semana")).font(theme::bold(15.0)));
                ui.add_space(4.0);
                for (path, title, ws, meeting) in notes.iter().take(30) {
                    let glyph = if *meeting { icon::USERS } else { icon::FILE_TEXT };
                    let mut job = LayoutJob::default();
                    job.append(&format!("{glyph}  "), 0.0, fmt(FontId::proportional(13.5), MUTED()));
                    job.append(title, 0.0, fmt(FontId::proportional(14.5), TEXT()));
                    job.append(&format!("   {ws}"), 0.0, fmt(FontId::proportional(12.5), MUTED()));
                    if clickable_line(ui, job).clicked() {
                        action = Some(Action::Open(path.clone(), None));
                    }
                }
                ui.add_space(14.0);
            }
            if asks > 0 && ui.link(RichText::new(format!("{} {}", icon::SPARKLE, tf!("La IA tiene {n}", n = plural(asks, "pregunta o sugerencia")))).size(13.0)).clicked() {
                action = Some(Action::ShowAi(super::ai_view::AiTab::Asks));
            }
        });
        if ask_ai {
            self.start_week_summary();
        }
        if save {
            if let Some(turn) = &self.week.turn {
                let text = format!("Revisión semanal ({})\n\n{}", range_label(&from, &to), plain(turn));
                let title = format!("Revisión semanal {}", to);
                self.save_text_note(&title, text);
            }
        }
        if self.esc(ui) {
            action = Some(Action::CloseResults);
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(range_label("2026-09-19", "2026-09-25"), "19 – 25 sep");
        assert_eq!(range_label("2026-09-28", "2026-10-04"), "28 sep – 4 oct");
        assert!(week_label().contains("-W"));
        let t = agenda::parse_task("x 2026-09-25 2026-09-20 Entregar informe +General").unwrap();
        assert_eq!((t.done, t.done_on.as_deref(), t.text.as_str()), (true, Some("2026-09-25"), "Entregar informe"));
    }
}
