//! La Agenda como un calendario de mes: una cuadrícula de lunes a domingo con lo de cada día
//! (eventos de las notas y de los calendarios, y tareas pendientes con fecha). Un clic en un
//! día lo muestra debajo, con todo lo que se puede hacer con cada cosa (como en Tu día).

use super::*;
use chrono::{Datelike, Duration as Days};

const WEEKDAYS: [&str; 7] = ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"];
const MONTHS: [&str; 12] = ["Enero", "Febrero", "Marzo", "Abril", "Mayo", "Junio", "Julio", "Agosto", "Septiembre", "Octubre", "Noviembre", "Diciembre"];
/// Cuántas cosas se ven dentro de un día (las demás, «+N»).
const IN_CELL: usize = 3;

fn first_of(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap_or(d)
}

fn add_months(d: NaiveDate, n: i32) -> NaiveDate {
    let m = d.year() * 12 + d.month0() as i32 + n;
    NaiveDate::from_ymd_opt(m.div_euclid(12), m.rem_euclid(12) as u32 + 1, 1).unwrap_or(d)
}

/// Una cosa de un día, para la cuadrícula: (hora, texto, color, es tarea).
type Chip = (String, String, Color32, bool);

impl NotesApp {
    /// El mes, con sus flechas y el día elegido debajo.
    pub(super) fn month_calendar(&mut self, ui: &mut Ui, col_w: f32) -> Option<Action> {
        let mut action = None;
        let today_d = Local::now().date_naive();
        let shown = self.agenda_month.unwrap_or_else(|| first_of(today_d));
        let lead = shown.weekday().num_days_from_monday() as i64;
        let start = shown - Days::days(lead);
        let days_in_month = (add_months(shown, 1) - shown).num_days();
        let weeks = (lead + days_in_month + 6) / 7;
        let end = start + Days::days(weeks * 7 - 1);
        let key = |d: NaiveDate| d.format("%Y-%m-%d").to_string();

        // Lo de cada día.
        let marks = self.event_marks();
        let events: Vec<agenda::Event> = self
            .all_events()
            .into_iter()
            .filter(|e| marks.get(&day_items::event_key(e)).map(String::as_str) != Some("tarea"))
            .filter(|e| e.date >= key(start) && e.date <= key(end))
            .collect();
        let tasks: Vec<agenda::Task> = self
            .agenda
            .tasks()
            .into_iter()
            .filter(|t| !t.done && t.due.as_deref().is_some_and(|d| agenda::is_date(d) && d >= key(start).as_str() && d <= key(end).as_str()))
            .collect();
        let mut chips: HashMap<String, Vec<Chip>> = HashMap::new();
        for e in &events {
            let color = if self.is_external(e) { theme::tag_colors(&e.project).dot } else { ACCENT };
            chips.entry(e.date.clone()).or_default().push((e.time.clone().unwrap_or_default(), e.title.clone(), color, false));
        }
        for t in &tasks {
            let d = t.due.clone().unwrap_or_default();
            chips.entry(d).or_default().push((String::new(), agenda::display_text(&t.text), MUTED, true));
        }
        for v in chips.values_mut() {
            v.sort_by(|a, b| (a.3, a.0.is_empty(), &a.0).cmp(&(b.3, b.0.is_empty(), &b.0)));
        }

        // Mes, flechas y «Hoy».
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} {}", MONTHS[shown.month0() as usize], shown.year())).font(theme::bold(18.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(RichText::new(icon::CARET_RIGHT).size(14.0)).on_hover_text("Mes siguiente").clicked() {
                    self.agenda_month = Some(add_months(shown, 1));
                }
                if ui.button(RichText::new("Hoy").size(13.0)).clicked() {
                    self.agenda_month = None;
                    self.agenda_day = Some(key(today_d));
                }
                if ui.button(RichText::new(icon::CARET_LEFT).size(14.0)).on_hover_text("Mes anterior").clicked() {
                    self.agenda_month = Some(add_months(shown, -1));
                }
            });
        });
        ui.add_space(6.0);

        // La cuadrícula.
        let cell_w = (col_w / 7.0).floor();
        let cell_h = 92.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for d in WEEKDAYS {
                let (r, _) = ui.allocate_exact_size(egui::vec2(cell_w, 20.0), Sense::hover());
                ui.painter().text(r.center(), Align2::CENTER_CENTER, d, FontId::proportional(12.0), MUTED);
            }
        });
        let selected = self.agenda_day.clone().unwrap_or_else(|| key(today_d));
        for week in 0..weeks {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for wd in 0..7 {
                    let day = start + Days::days(week * 7 + wd);
                    let k = key(day);
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(cell_w, cell_h), Sense::click());
                    let p = ui.painter();
                    let in_month = day.month() == shown.month();
                    let fill = if k == selected { ACCENT_BG } else if resp.hovered() { HOVER } else if in_month { BG_EDITOR } else { BG_SIDE };
                    p.rect_filled(rect.shrink(1.0), 4, fill);
                    p.rect_stroke(rect.shrink(1.0), 4, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
                    let num_color = if !in_month { theme::BORDER } else if day == today_d { Color32::WHITE } else { TEXT };
                    let num_pos = egui::pos2(rect.left() + 14.0, rect.top() + 12.0);
                    if day == today_d {
                        p.circle_filled(num_pos, 10.0, ACCENT);
                    }
                    p.text(num_pos, Align2::CENTER_CENTER, day.day().to_string(), FontId::proportional(12.5), num_color);
                    let list = chips.get(&k).map(Vec::as_slice).unwrap_or(&[]);
                    let mut y = rect.top() + 26.0;
                    for (time, text, color, is_task) in list.iter().take(IN_CELL) {
                        let lead = if *is_task { format!("{} ", icon::SQUARE) } else if time.is_empty() { "● ".to_string() } else { format!("{time} ") };
                        let mut job = LayoutJob::default();
                        job.append(&lead, 0.0, fmt(FontId::proportional(10.5), *color));
                        job.append(text, 0.0, fmt(FontId::proportional(11.0), if in_month { TEXT } else { MUTED }));
                        job.wrap = egui::text::TextWrapping { max_width: cell_w - 10.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                        let g = p.layout_job(job);
                        p.galley(egui::pos2(rect.left() + 5.0, y), g, TEXT);
                        y += 16.0;
                    }
                    if list.len() > IN_CELL {
                        p.text(egui::pos2(rect.left() + 6.0, y), Align2::LEFT_TOP, format!("+{} más", list.len() - IN_CELL), FontId::proportional(10.5), MUTED);
                    }
                    if resp.clicked() {
                        self.agenda_day = Some(k.clone());
                    }
                    let tip = if list.is_empty() { "Nada este día".to_string() } else { list.iter().map(|c| if c.0.is_empty() { c.1.clone() } else { format!("{} {}", c.0, c.1) }).collect::<Vec<_>>().join("\n") };
                    resp.on_hover_text(tip);
                }
            });
        }

        // El día elegido, con todo lo que se puede hacer con cada cosa.
        ui.add_space(14.0);
        let sel = NaiveDate::parse_from_str(&selected, "%Y-%m-%d").unwrap_or(today_d);
        let label = if sel == today_d { format!("Hoy · {}", long_date(&selected)) } else { long_date(&selected) };
        ui.label(RichText::new(label).font(theme::bold(15.0)).color(if sel == today_d { ACCENT } else { TEXT }));
        ui.add_space(4.0);
        let follows = self.follow_up_map();
        let root = self.vault.root.clone();
        let day_events: Vec<&agenda::Event> = events.iter().filter(|e| e.date == selected).collect();
        let day_tasks: Vec<&agenda::Task> = tasks.iter().filter(|t| t.due.as_deref() == Some(selected.as_str())).collect();
        if day_events.is_empty() && day_tasks.is_empty() {
            ui.label(RichText::new("Nada este día.").color(MUTED));
        }
        for e in day_events {
            let done = marks.get(&day_items::event_key(e)).map(String::as_str) == Some("hecho");
            if let Some(a) = day_items::event_row(ui, e, done, &follows, false) {
                action = Some(a);
            }
        }
        let today_s = today();
        for t in day_tasks {
            if let Some(a) = task_row(ui, t, &today_s, &root, &follows) {
                action = Some(a);
            }
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months_go_around_the_year() {
        let d = NaiveDate::from_ymd_opt(2026, 12, 15).unwrap();
        assert_eq!(add_months(first_of(d), 1), NaiveDate::from_ymd_opt(2027, 1, 1).unwrap());
        assert_eq!(add_months(first_of(d), -12), NaiveDate::from_ymd_opt(2025, 12, 1).unwrap());
        assert_eq!(add_months(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), -1), NaiveDate::from_ymd_opt(2025, 12, 1).unwrap());
    }
}
