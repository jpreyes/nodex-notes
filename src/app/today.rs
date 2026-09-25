//! Vista "Hoy": lo atrasado, lo de hoy, lo de mañana y lo que viene en la semana.
//! Se muestra sola la primera vez que se abre la app cada día (si hay algo que mostrar).

use super::*;

/// Una fila de evento: hora, título y espacio; un clic abre su nota.
fn event_row(ui: &mut Ui, e: &agenda::Event, root: &Path) -> Option<Action> {
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
    match &e.note {
        Some(n) if r.clicked() => Some(Action::Open(root.join(format!("{n}.md")), None)),
        _ => None,
    }
}

fn day(offset: i64) -> String {
    (Local::now() + chrono::Duration::days(offset)).format("%Y-%m-%d").to_string()
}

impl NotesApp {
    /// ¿Hay algo atrasado, para hoy o para mañana?
    pub(super) fn has_something_today(&self) -> bool {
        let tomorrow = day(1);
        self.agenda.tasks().iter().any(|t| !t.done && t.due.as_deref().is_some_and(|d| agenda::is_date(d) && *d <= *tomorrow))
            || self.agenda.events().iter().any(|e| e.date >= today() && e.date <= tomorrow)
    }

    pub(super) fn today_view(&mut self, ui: &mut Ui) -> Option<Action> {
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
        let events = self.agenda.events();
        let ev = |f: &dyn Fn(&str) -> bool| -> Vec<agenda::Event> {
            let mut v: Vec<agenda::Event> = events.iter().filter(|e| f(&e.date)).cloned().collect();
            v.sort_by(|a, b| (a.date.clone(), a.time.clone()).cmp(&(b.date.clone(), b.time.clone())));
            v
        };
        let ev_today = ev(&|d| d == today);
        let ev_tomorrow = ev(&|d| d == tomorrow);
        let ev_week = ev(&|d| d > tomorrow.as_str() && d <= week.as_str());
        let root = self.vault.root.clone();
        let nothing = overdue.is_empty() && for_today.is_empty() && for_tomorrow.is_empty() && this_week.is_empty()
            && ev_today.is_empty() && ev_tomorrow.is_empty() && ev_week.is_empty();

        Self::column(ui, "today", |ui, _| {
            let subtitle = {
                let mut s = long_date(&today);
                if let Some(first) = s.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                s
            };
            if view_header(ui, "Hoy", &subtitle) {
                action = Some(Action::CloseResults);
            }
            let mut section_ui = |ui: &mut Ui, title: &str, color: Color32, tasks: &[agenda::Task], events: &[agenda::Event]| {
                if tasks.is_empty() && events.is_empty() {
                    return;
                }
                ui.label(RichText::new(title).font(theme::bold(15.0)).color(color));
                ui.add_space(4.0);
                for e in events {
                    if let Some(a) = event_row(ui, e, &root) {
                        action = Some(a);
                    }
                }
                for t in tasks {
                    if let Some(a) = task_row(ui, t, &today, &root) {
                        action = Some(a);
                    }
                }
                ui.add_space(16.0);
            };
            section_ui(ui, &format!("{} Atrasadas", icon::WARNING_CIRCLE), RED, &overdue, &[]);
            section_ui(ui, "Hoy", ACCENT, &for_today, &ev_today);
            section_ui(ui, &format!("Mañana · {}", long_date(&tomorrow)), TEXT, &for_tomorrow, &ev_tomorrow);
            section_ui(ui, "Esta semana", TEXT, &this_week, &ev_week);
            if nothing {
                ui.label(RichText::new("Nada atrasado ni pendiente para esta semana.").color(MUTED));
                ui.add_space(16.0);
            }
            ui.horizontal(|ui| {
                if ui.button(format!("{} Escribir en la nota de hoy", icon::NOTE_PENCIL)).clicked() {
                    action = Some(Action::Today);
                }
                if undated > 0 && ui.link(RichText::new(format!("{} sin fecha", plural(undated, "tarea"))).size(12.5)).clicked() {
                    action = Some(Action::Show(View::Tasks));
                }
            });
        });
        if self.esc(ui) {
            action = Some(Action::CloseResults);
        }
        action
    }
}
