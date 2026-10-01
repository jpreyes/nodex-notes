//! "Tu día", en Inicio: lo atrasado, lo de hoy, lo de mañana y lo que viene en la semana
//! (tareas con fecha, eventos de las notas y de los calendarios agregados).

use super::*;

/// Una fila de evento: hora, título y espacio; un clic abre su nota.
fn event_row(ui: &mut Ui, e: &agenda::Event, root: &Path) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        let mut job = LayoutJob::default();
        let lead = match &e.time {
            Some(t) => format!("{t}  "),
            None => format!("{}  ", icon::CALENDAR_BLANK),
        };
        job.append(&lead, 0.0, fmt(FontId::proportional(14.0), MUTED));
        job.append(&e.title, 0.0, fmt(FontId::proportional(14.5), TEXT));
        if !e.project.is_empty() {
            job.append(&format!("   {}", e.project), 0.0, fmt(FontId::proportional(12.5), MUTED));
        }
        let r = clickable_line(ui, job);
        if let Some(n) = &e.note {
            if r.clicked() {
                action = Some(Action::Open(root.join(format!("{n}.md")), None));
            }
        } else if e.date == today() {
            // Evento de un calendario: tomar notas abre una reunión con su nombre.
            ui.add_space(6.0);
            if ui.link(RichText::new(format!("{} Tomar notas", icon::NOTE_PENCIL)).size(12.5)).clicked() {
                action = Some(Action::StartMeetingNamed(e.title.clone()));
            }
        }
    });
    action
}

fn day(offset: i64) -> String {
    (Local::now() + chrono::Duration::days(offset)).format("%Y-%m-%d").to_string()
}

impl NotesApp {
    /// Lo atrasado, hoy, mañana y esta semana, con sus accesos.
    pub(super) fn day_sections(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let (today, tomorrow, week) = (today(), day(1), day(7));
        let tasks = self.agenda.tasks();
        let pending: Vec<&agenda::Task> = tasks.iter().filter(|t| !t.done).collect();
        let due = |t: &&agenda::Task| t.due.clone().filter(|d| agenda::is_date(d));
        let pick = |f: &dyn Fn(&str) -> bool| -> Vec<agenda::Task> {
            let mut v: Vec<agenda::Task> = pending.iter().filter(|t| due(t).is_some_and(|d| f(&d))).map(|t| (*t).clone()).collect();
            v.sort_by(|a, b| a.due.cmp(&b.due));
            v
        };
        let overdue = pick(&|d| d < today.as_str());
        let for_today = pick(&|d| d == today);
        let for_tomorrow = pick(&|d| d == tomorrow);
        let this_week = pick(&|d| d > tomorrow.as_str() && d <= week.as_str());
        let undated = pending.iter().filter(|t| due(t).is_none()).count();
        let events = self.all_events();
        let ev = |f: &dyn Fn(&str) -> bool| -> Vec<agenda::Event> {
            let mut v: Vec<agenda::Event> = events.iter().filter(|e| f(&e.date)).cloned().collect();
            v.sort_by(|a, b| (a.date.clone(), a.time.clone()).cmp(&(b.date.clone(), b.time.clone())));
            v
        };
        let ev_today = ev(&|d| d == today);
        let ev_tomorrow = ev(&|d| d == tomorrow);
        let ev_week = ev(&|d| d > tomorrow.as_str() && d <= week.as_str());
        let root = self.vault.root.clone();
        let follows = self.follow_up_map();
        let nothing = overdue.is_empty() && for_today.is_empty() && for_tomorrow.is_empty() && this_week.is_empty()
            && ev_today.is_empty() && ev_tomorrow.is_empty() && ev_week.is_empty();

        let mut section_ui = |ui: &mut Ui, title: &str, color: Color32, tasks: &[agenda::Task], events: &[agenda::Event]| {
            if tasks.is_empty() && events.is_empty() {
                return;
            }
            ui.label(RichText::new(title).font(theme::bold(14.5)).color(color));
            ui.add_space(2.0);
            for e in events {
                if let Some(a) = event_row(ui, e, &root) {
                    action = Some(a);
                }
            }
            for t in tasks {
                if let Some(a) = task_row(ui, t, &today, &root, &follows) {
                    action = Some(a);
                }
            }
            ui.add_space(10.0);
        };
        section_ui(ui, &format!("{} Atrasadas", icon::WARNING_CIRCLE), RED, &overdue, &[]);
        section_ui(ui, "Hoy", ACCENT, &for_today, &ev_today);
        section_ui(ui, &format!("Mañana · {}", long_date(&tomorrow)), TEXT, &for_tomorrow, &ev_tomorrow);
        section_ui(ui, "Esta semana", TEXT, &this_week, &ev_week);
        if nothing {
            ui.label(RichText::new("Nada atrasado ni pendiente para esta semana.").color(MUTED));
            ui.add_space(8.0);
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button(format!("{} Escribir en la nota de hoy", icon::NOTE_PENCIL)).clicked() {
                action = Some(Action::Today);
            }
            if ui.link(RichText::new(format!("{} Revisión semanal", icon::CALENDAR_CHECK)).size(12.5)).clicked() {
                action = Some(Action::ShowTab(View::Week));
            }
            if undated > 0 && ui.link(RichText::new(format!("{} sin fecha", plural(undated, "tarea"))).size(12.5)).clicked() {
                action = Some(Action::ShowTab(View::Tasks));
            }
        });
        action
    }
}
