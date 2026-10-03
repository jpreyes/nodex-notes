//! "Tu día", en Inicio: lo atrasado, lo de hoy, lo de mañana y lo que viene en la semana
//! (tareas con fecha, eventos de las notas y de los calendarios agregados).

use super::*;

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
        // Los eventos que se convirtieron en tarea ya no se ven como evento.
        let marks = self.event_marks();
        let events: Vec<agenda::Event> =
            self.all_events().into_iter().filter(|e| marks.get(&day_items::event_key(e)).map(String::as_str) != Some("tarea")).collect();
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
                let done = marks.get(&day_items::event_key(e)).map(String::as_str) == Some("hecho");
                if let Some(a) = day_items::event_row(ui, e, done, &follows, false) {
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
