//! Calendarios agregados con su enlace ICS: la lista (con "+ Agregar calendario"), que se
//! muestra en la Agenda y en Configuración → Calendar.

use super::*;
use crate::calendars::{Calendars, Subscription};

/// Los pasos para sacar el enlace en cada servicio: (servicio, pasos).
const HELP: [(&str, &str); 3] = [
    (
        "Google Calendar",
        "En calendar.google.com: ⚙ Configuración → en «Configuración de mis calendarios» elige el calendario → «Integrar el calendario» → copia la «Dirección secreta en formato iCal».",
    ),
    (
        "Outlook",
        "En outlook.com: ⚙ Configuración → Calendario → Calendarios compartidos → «Publicar un calendario»: elige el calendario y «Puede ver todos los detalles», Publicar, y copia el enlace ICS.",
    ),
    ("iCloud / Apple", "En la app Calendario: comparte el calendario, activa «Calendario público» y copia el enlace (webcal://…)."),
];

/// Lista de calendarios con su estado y el formulario para agregar uno.
pub(super) fn calendars_panel(ui: &mut Ui, subs: &[Subscription], cals: &Calendars, form: &mut Option<(String, String)>) -> Option<Action> {
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        for (i, s) in subs.iter().enumerate() {
            let c = theme::tag_colors(&s.nombre);
            let url = crate::calendars::normalize(&s.url);
            let error = cals.errors.get(&url);
            let state = match (error, cals.loaded(&s.url)) {
                (Some(e), _) => format!("No se pudo leer: {e}"),
                (None, true) => "Al día".to_string(),
                (None, false) => "Descargando…".to_string(),
            };
            let label = if error.is_some() { format!("{}  {}", icon::WARNING_CIRCLE, s.nombre) } else { format!("●  {}", s.nombre) };
            let chip = egui::Button::new(RichText::new(label).size(12.5).color(if error.is_some() { RED } else { c.text })).fill(c.bg).stroke(Stroke::new(1.0, c.border)).corner_radius(11);
            let r = ui.add(chip).on_hover_text(format!("{state}\n{url}\nClic derecho: quitar"));
            r.context_menu(|ui| {
                if ui.button(format!("{}  Quitar este calendario", icon::TRASH)).clicked() {
                    action = Some(Action::RemoveCalendar(i));
                    ui.close();
                }
                if ui.button(format!("{}  Actualizar ahora", icon::ARROW_CLOCKWISE)).clicked() {
                    action = Some(Action::RefreshCalendars);
                    ui.close();
                }
            });
        }
        if form.is_none() {
            let b = egui::Button::new(RichText::new(format!("{}  Agregar calendario", icon::PLUS)).size(12.5)).corner_radius(11);
            if ui.add(b).on_hover_text("Pega el enlace ICS de Google Calendar, Outlook, iCloud…").clicked() {
                *form = Some((String::new(), String::new()));
            }
        }
        if cals.busy() {
            ui.spinner();
        }
    });
    let mut close = false;
    if let Some((name, url)) = form {
        ui.add_space(6.0);
        Frame::new().fill(BG_SIDE).stroke(Stroke::new(1.0, theme::BORDER)).corner_radius(10).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Agregar un calendario").font(theme::bold(14.0)));
            ui.add_space(4.0);
            ui.add(egui::TextEdit::singleline(name).hint_text("Nombre (por ejemplo: Trabajo)").desired_width(f32::INFINITY));
            let r = ui.add(egui::TextEdit::singleline(url).hint_text("Enlace ICS: https://…/basic.ics  o  webcal://…").desired_width(f32::INFINITY));
            let valid = {
                let u = url.trim().to_lowercase();
                u.starts_with("https://") || u.starts_with("http://") || u.starts_with("webcal://")
            };
            if !url.trim().is_empty() && !valid {
                ui.label(RichText::new("El enlace debe empezar con https:// o webcal://").size(12.0).color(RED));
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if (ui.add_enabled(valid, egui::Button::new("Agregar")).clicked() || (enter && valid)) && valid {
                    action = Some(Action::AddCalendar(name.trim().to_string(), url.trim().to_string()));
                    close = true;
                }
                if ui.button("Cancelar").clicked() {
                    close = true;
                }
            });
            egui::CollapsingHeader::new(RichText::new("¿Dónde saco el enlace?").size(12.5)).default_open(subs.is_empty()).show(ui, |ui| {
                for (service, steps) in HELP {
                    ui.label(RichText::new(service).size(12.5).strong());
                    ui.label(RichText::new(steps).size(12.5).color(MUTED));
                    ui.add_space(4.0);
                }
                ui.label(RichText::new("Solo se lee: la app no cambia esos calendarios. Se actualizan al abrir la app y cada 15 minutos.").size(12.0).color(MUTED));
            });
        });
    }
    if close {
        *form = None;
    }
    action
}

impl NotesApp {
    /// Todos los eventos: los de la agenda de las notas y los de los calendarios agregados.
    pub(super) fn all_events(&self) -> Vec<agenda::Event> {
        let mut e = self.agenda.events();
        e.extend(self.cals.as_agenda(&self.cfg.calendarios));
        e.sort_by(|a, b| (a.date.clone(), a.time.clone()).cmp(&(b.date.clone(), b.time.clone())));
        e
    }

    /// ¿Este evento viene de un calendario agregado? (su "espacio" es el nombre del calendario).
    pub(super) fn is_external(&self, e: &agenda::Event) -> bool {
        e.note.is_none() && self.cfg.calendarios.iter().any(|c| c.nombre == e.project)
    }

    pub(super) fn add_calendar(&mut self, name: String, url: String) {
        let name = if name.trim().is_empty() { format!("Calendario {}", self.cfg.calendarios.len() + 1) } else { name.trim().to_string() };
        if self.cfg.calendarios.iter().any(|c| crate::calendars::normalize(&c.url) == crate::calendars::normalize(&url)) {
            self.msg("Ese calendario ya está agregado");
            return;
        }
        self.cfg.calendarios.push(Subscription { nombre: name.clone(), url });
        self.save_config();
        self.cals.last = None; // se descarga en el próximo ciclo
        self.msg(format!("Calendario «{name}» agregado; descargando…"));
    }

    pub(super) fn remove_calendar(&mut self, i: usize) {
        if i < self.cfg.calendarios.len() {
            let c = self.cfg.calendarios.remove(i);
            self.save_config();
            self.msg(format!("Calendario «{}» quitado", c.nombre));
        }
    }

    /// Descarga los calendarios al abrir y cada 15 minutos; recibe lo descargado.
    pub(super) fn handle_calendars(&mut self) {
        self.cals.poll();
        let due = self.cals.last.is_none_or(|t| t.elapsed() >= crate::calendars::REFRESH);
        if due && !self.cals.busy() && !self.cfg.calendarios.is_empty() {
            let subs = self.cfg.calendarios.clone();
            self.cals.refresh(&subs, self.ctx.clone());
        }
    }
}
