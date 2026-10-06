//! «Por agendar»: los eventos que la IA saca de un correo no entran solos a la agenda. Quedan
//! como propuesta, arriba en Tu día y en la Agenda, con «Agendar» y «Descartar».
//!
//! Lo decidido se anota en `.nodex/por-agendar.txt`, una línea por cambio
//! («pendiente|si|no», TAB, la clave del evento, TAB, su línea de agenda.txt); vale la última de
//! cada clave. Lo descartado (y lo que se quita de la agenda a mano) no vuelve aunque la IA
//! reorganice la nota; lo agendado se queda.

use super::*;

pub(super) const FILE: &str = "por-agendar.txt";

/// La clave de un evento: día, hora y título (sin mayúsculas ni tildes).
pub(super) fn key_of(line: &str) -> Option<String> {
    let e = agenda::parse_event(line)?;
    Some(key_event(&e))
}

pub(super) fn key_event(e: &agenda::Event) -> String {
    format!("{}|{}|{}", e.date, e.time.clone().unwrap_or_default(), vault::fold(e.title.trim()))
}

/// ¿La nota de adentro `unit` (por su id, «L3») de `text` es un correo anotado?
pub(super) fn is_mail_unit(text: &str, unit: &str) -> bool {
    let ls: Vec<&str> = text.lines().collect();
    lines::units(text).into_iter().find(|u| u.id.eq_ignore_ascii_case(unit.trim())).and_then(|u| ls.get(u.first).copied()).is_some_and(|l| {
        let info = lines::parse(l);
        let t = l.get(info.prefix..).unwrap_or(l).trim_start();
        t.starts_with("Correo de ") || t.starts_with("Correo a ")
    })
}

/// Una propuesta pendiente.
pub(super) struct Proposal {
    pub key: String,
    pub line: String,
    pub event: agenda::Event,
}

impl NotesApp {
    fn proposals_path(&self) -> PathBuf {
        self.vault.root.join(".nodex").join(FILE)
    }

    /// Lo decidido de cada evento: clave -> (estado, línea).
    pub(super) fn proposal_states(&self) -> HashMap<String, (String, String)> {
        let mut out = HashMap::new();
        for l in vault::read_text(&self.proposals_path()).unwrap_or_default().lines() {
            let mut p = l.splitn(3, '\t');
            if let (Some(state), Some(key), Some(line)) = (p.next(), p.next(), p.next()) {
                out.insert(key.to_string(), (state.to_string(), line.to_string()));
            }
        }
        out
    }

    fn mark_proposal(&self, state: &str, key: &str, line: &str) {
        let path = self.proposals_path();
        let _ = fs::create_dir_all(self.vault.root.join(".nodex"));
        let mut text = vault::read_text(&path).unwrap_or_default();
        text += &format!("{state}\t{key}\t{line}\n");
        let _ = fs::write(&path, text);
    }

    /// Antes de escribir en la agenda lo que organizó la IA: lo descartado no entra; lo que salió
    /// de un correo y no se ha agendado queda por agendar. `mail(i)` dice si el evento `i` salió
    /// de un correo.
    pub(super) fn gate_events(&mut self, events: Vec<String>, mail: impl Fn(usize) -> bool) -> Vec<String> {
        let states = self.proposal_states();
        let mut keep = Vec::new();
        let mut proposed = 0;
        for (i, line) in events.into_iter().enumerate() {
            let Some(key) = key_of(&line) else {
                keep.push(line);
                continue;
            };
            match states.get(&key).map(|s| s.0.as_str()) {
                Some("si") => keep.push(line),
                Some("no") => {}
                Some(_) => {}
                None if mail(i) => {
                    self.mark_proposal("pendiente", &key, &line);
                    proposed += 1;
                }
                None => keep.push(line),
            }
        }
        if proposed > 0 {
            self.msg(if proposed == 1 { t!("Un correo trae algo para la agenda: está en «Por agendar», en Tu día").to_string() } else { tf!("{n} cosas de correos esperan en «Por agendar», en Tu día", n = proposed) });
        }
        keep
    }

    /// Las propuestas pendientes, de hoy en adelante, por fecha.
    pub(super) fn proposals(&self) -> Vec<Proposal> {
        let today = today();
        let mut v: Vec<Proposal> = self
            .proposal_states()
            .into_iter()
            .filter(|(_, (s, _))| s == "pendiente")
            .filter_map(|(key, (_, line))| {
                let event = agenda::parse_event(&line)?;
                (event.date >= today).then_some(Proposal { key, line, event })
            })
            .collect();
        v.sort_by(|a, b| (&a.event.date, &a.event.time).cmp(&(&b.event.date, &b.event.time)));
        v
    }

    /// «Agendar»: pasa a la agenda (y a Google Calendar, si está conectado).
    pub(super) fn schedule(&mut self, key: &str) {
        let Some((_, line)) = self.proposal_states().remove(key) else { return };
        if let Err(e) = self.agenda.add_lines(&[], std::slice::from_ref(&line)) {
            self.msg(tf!("No se pudo escribir la agenda: {e}", e = e));
            return;
        }
        self.mark_proposal("si", key, &line);
        self.gcal_dirty = true;
        if let Some(e) = agenda::parse_event(&line) {
            self.msg(tf!("Agendado: «{title}», {when}", title = e.title, when = long_date(&e.date)));
        }
    }

    /// «Descartar» (no vuelve a proponerse).
    pub(super) fn dismiss(&mut self, key: &str) {
        let Some((_, line)) = self.proposal_states().remove(key) else { return };
        self.mark_proposal("no", key, &line);
    }

    /// «Quitar de la agenda» un evento de las notas (y que no vuelva aunque se reorganice la nota).
    pub(super) fn unschedule(&mut self, e: &agenda::Event) {
        let line = agenda::format_event(&e.date, e.time.as_deref(), &e.title, &e.project, e.note.as_deref().unwrap_or(""));
        if self.agenda.events().contains(e) {
            let _ = self.agenda.remove_event(e);
        }
        self.mark_proposal("no", &key_event(e), &line);
        self.gcal_dirty = true;
        self.msg(tf!("«{title}» salió de la agenda", title = e.title));
    }

    /// Una sola vez: los eventos futuros que ya habían entrado solos desde un correo pasan a «Por
    /// agendar» (un evento es «de un correo» si su nota tiene una línea «Correo de…» que habla de
    /// lo mismo). Devuelve cuántos.
    pub(super) fn migrate_mail_events(&mut self) -> usize {
        let marker = self.vault.root.join(".nodex").join("por-agendar-revisado.txt");
        if marker.exists() {
            return 0;
        }
        let today = today();
        let states = self.proposal_states();
        let mut moved = Vec::new();
        for e in self.agenda.events().into_iter().filter(|e| e.date >= today) {
            let Some(note) = e.note.as_deref().filter(|n| !n.is_empty()) else { continue };
            if states.contains_key(&key_event(&e)) {
                continue;
            }
            let text = vault::read_text(&self.vault.root.join(format!("{note}.md"))).unwrap_or_default();
            let title = crate::dups::words(&e.title);
            let from_mail = text.lines().any(|l| {
                let info = lines::parse(l);
                let t = l.get(info.prefix..).unwrap_or(l).trim_start();
                (t.starts_with("Correo de ") || t.starts_with("Correo a ")) && {
                    let w = crate::dups::words(t);
                    let shared = title.intersection(&w).count();
                    shared >= 2 || (shared >= 1 && title.len() <= 2)
                }
            });
            if from_mail {
                moved.push(e);
            }
        }
        for e in &moved {
            let line = agenda::format_event(&e.date, e.time.as_deref(), &e.title, &e.project, e.note.as_deref().unwrap_or(""));
            let _ = self.agenda.remove_event(e);
            if !self.proposal_states().contains_key(&key_event(e)) {
                self.mark_proposal("pendiente", &key_event(e), &line);
            }
        }
        let _ = fs::create_dir_all(self.vault.root.join(".nodex"));
        let _ = fs::write(&marker, format!("{today}\n"));
        if !moved.is_empty() {
            self.gcal_dirty = true;
            self.msg(tf!("Pasé {n} a «Por agendar» (en Tu día): venían de correos y habían entrado solos a la agenda", n = plural(moved.len(), "evento")));
        }
        moved.len()
    }

    /// Las propuestas, con sus botones (arriba en Tu día y en la Agenda).
    pub(super) fn proposals_ui(&mut self, ui: &mut Ui) -> Option<Action> {
        let list = self.proposals();
        if list.is_empty() {
            return None;
        }
        let mut action = None;
        ui.label(RichText::new(format!("{} {}", icon::CALENDAR_PLUS, tf!("Por agendar ({n})", n = list.len()))).font(theme::bold(14.5)).color(ACCENT()));
        ui.label(RichText::new(t!("Vienen de correos: no están en tu agenda hasta que las agendes.")).size(12.5).color(MUTED()));
        ui.add_space(2.0);
        for p in &list {
            ui.horizontal_wrapped(|ui| {
                let when = match &p.event.time {
                    Some(t) => format!("{} {t}", long_date(&p.event.date)),
                    None => long_date(&p.event.date),
                };
                ui.label(RichText::new(format!("{}  {when}", icon::ENVELOPE_SIMPLE)).size(13.0).color(MUTED()));
                ui.label(RichText::new(&p.event.title).size(14.5).color(TEXT()));
                if !p.event.project.is_empty() {
                    ui.label(RichText::new(&p.event.project).size(12.5).color(MUTED()));
                }
                if p.event.note.is_some() {
                    let b = egui::Button::new(RichText::new(icon::ARROW_SQUARE_OUT).size(14.0).color(MUTED())).frame(false);
                    if ui.add(b).on_hover_text(t!("Ver el correo en su nota")).clicked() {
                        action = Some(Action::Item(day_items::ItemDo::OpenEvent(p.event.clone())));
                    }
                }
                let b = egui::Button::new(RichText::new(format!("{} {}", icon::CALENDAR_CHECK, t!("Agendar"))).size(12.5).color(theme::c(Color32::WHITE))).fill(ACCENT());
                if ui.add(b).clicked() {
                    action = Some(Action::Item(day_items::ItemDo::Schedule(p.key.clone())));
                }
                if ui.add(egui::Button::new(RichText::new(t!("Descartar")).size(12.5))).on_hover_text(t!("No va a la agenda y no se vuelve a proponer")).clicked() {
                    action = Some(Action::Item(day_items::ItemDo::Dismiss(p.key.clone())));
                }
            });
        }
        ui.add_space(10.0);
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lo que la IA saca de un correo queda por agendar; al agendarlo entra (y se queda aunque se
    /// reorganice la nota); lo descartado o quitado no vuelve.
    #[test]
    fn mail_events_wait_to_be_scheduled() {
        let dir = std::env::temp_dir().join(format!("nodex-por-agendar-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("General")).unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());

        let text = "Visita a obra el jueves\nCorreo de Eric (5 oct): Reunión cabañas. Propone reunirse el 9 a las 21:00.\n";
        assert!(!is_mail_unit(text, "L1") && is_mail_unit(text, "L2"));
        let day = (Local::now() + chrono::Duration::days(3)).format("%Y-%m-%d").to_string();
        let obra = agenda::format_event(&day, Some("10:00"), "Visita a obra", "General", "General/x");
        let eric = agenda::format_event(&day, Some("21:00"), "Reunión con Eric", "General", "General/x");
        let kept = app.gate_events(vec![obra.clone(), eric.clone()], |i| i == 1);
        assert_eq!(kept, vec![obra.clone()], "lo del correo no entra solo");
        assert_eq!(app.proposals().len(), 1);

        // Agendar: entra a la agenda y la próxima vez que la IA lo saque, se queda.
        let key = app.proposals()[0].key.clone();
        app.apply(Action::Item(day_items::ItemDo::Schedule(key)));
        assert!(app.proposals().is_empty());
        assert!(app.agenda.events().iter().any(|e| e.title == "Reunión con Eric"));
        assert_eq!(app.gate_events(vec![eric.clone()], |_| true), vec![eric.clone()]);

        // Quitar de la agenda: sale y no vuelve.
        let ev = app.agenda.events().into_iter().find(|e| e.title == "Reunión con Eric").unwrap();
        app.apply(Action::Item(day_items::ItemDo::EventRemove(ev)));
        assert!(!app.agenda.events().iter().any(|e| e.title == "Reunión con Eric"));
        assert!(app.gate_events(vec![eric.clone()], |_| true).is_empty());

        // Descartar una propuesta: no vuelve a proponerse.
        let otro = agenda::format_event(&day, None, "Almuerzo", "General", "General/x");
        app.gate_events(vec![otro.clone()], |_| true);
        let key = app.proposals()[0].key.clone();
        app.apply(Action::Item(day_items::ItemDo::Dismiss(key)));
        assert!(app.proposals().is_empty());
        assert!(app.gate_events(vec![otro], |_| true).is_empty());

        // Lo que ya había entrado solo desde un correo pasa a «Por agendar» (una sola vez).
        fs::write(dir.join("General").join("Consejo.md"), "Correo de Rectoría (5 oct): Citación. Sesión ordinaria del Consejo Académico el miércoles.\nPreparar la minuta\n").unwrap();
        let consejo = agenda::format_event(&day, Some("15:00"), "Sesión ordinaria del Consejo Académico", "General", "General/Consejo");
        let minuta = agenda::format_event(&day, Some("09:00"), "Preparar la minuta", "General", "General/Consejo");
        app.agenda.add_lines(&[], &[consejo, minuta]).unwrap();
        let _ = fs::remove_file(dir.join(".nodex").join("por-agendar-revisado.txt"));
        assert_eq!(app.migrate_mail_events(), 1);
        assert!(app.agenda.events().iter().any(|e| e.title == "Preparar la minuta"));
        assert!(app.proposals().iter().any(|p| p.event.title == "Sesión ordinaria del Consejo Académico"));
        assert_eq!(app.migrate_mail_events(), 0, "una sola vez");
        let _ = fs::remove_dir_all(&dir);
    }
}
