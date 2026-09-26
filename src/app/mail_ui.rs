//! Correos: se revisan cada 15 minutos (hilo aparte), la IA los lee por lotes y lo que
//! encuentra (compromisos, fechas) va solo a Tareas y Agenda, con Deshacer. La vista
//! Correos muestra cada correo con su resumen y las verificaciones de compromisos.

use super::*;
use crate::mail::{self, Account, Fulfill, Mail};
use crate::mail_ai;
use std::sync::mpsc::{self, Receiver};

enum Msg {
    Fetched(String, Result<(Vec<Mail>, HashMap<String, u32>), String>),
    FetchDone,
    Analyzed(Result<String, String>),
    Tested(String, Result<(), String>),
}

#[derive(Default)]
pub(super) struct MailState {
    pub(super) store: mail::Store,
    rx: Option<Receiver<Msg>>,
    fetching: bool,
    analyzing: bool,
    /// Lote que está leyendo la IA: (clave "c1", id del correo) y tareas ("t1", tarea).
    batch: Vec<(String, String)>,
    batch_tasks: Vec<(String, agenda::Task)>,
    pub(super) errors: HashMap<String, String>,
    ai_error: Option<String>,
    /// Hay que revisar (se pidió, llegó un correo o es la hora diaria).
    due: bool,
    started: bool,
    /// Hilos que esperan correo nuevo (IMAP IDLE): avisos, cómo pararlos y para qué cuentas.
    watch_rx: Option<Receiver<(String, Result<(), String>)>>,
    watch_stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    watch_key: String,
    pub(super) last_ok: Option<DateTime<Local>>,
    /// Correo abierto en la vista.
    open: Option<String>,
    /// Ver también los boletines y avisos.
    show_bulk: bool,
    /// Resultado de "Probar" por cuenta.
    pub(super) tests: HashMap<String, Result<(), String>>,
}

impl MailState {
    pub(super) fn load() -> MailState {
        MailState { store: mail::Store::load(), ..MailState::default() }
    }

    /// Verificaciones sin responder: (correo, índice).
    pub(super) fn open_checks(&self) -> Vec<(Mail, usize)> {
        self.store.mails.iter().flat_map(|m| m.fulfills.iter().enumerate().filter(|(_, f)| !f.resolved).map(move |(i, _)| (m.clone(), i))).collect()
    }

    pub(super) fn busy(&self) -> bool {
        self.fetching || self.analyzing
    }

    /// "Revisar ahora" (y la IA vuelve a intentar).
    pub(super) fn request(&mut self) {
        self.due = true;
        self.ai_error = None;
    }

    fn stop_watchers(&mut self) {
        if let Some(s) = self.watch_stop.take() {
            s.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.watch_rx = None;
        self.watch_key.clear();
    }
}

/// Pasos para crear la contraseña de aplicación: (servicio, pasos, enlace).
const HELP: [(&str, &str, &str); 3] = [
    ("Gmail", "Activa la verificación en dos pasos y crea una contraseña de aplicación (16 letras) en tu cuenta de Google.", "https://myaccount.google.com/apppasswords"),
    ("Outlook / Hotmail", "Activa la verificación en dos pasos y crea una contraseña de aplicación en la seguridad de tu cuenta Microsoft.", "https://account.live.com/proofs/AppPassword"),
    ("iCloud", "En tu cuenta de Apple → Inicio de sesión y seguridad → Contraseñas específicas de apps.", "https://account.apple.com"),
];

/// Cuentas de correo con su estado y el formulario para agregar una (en Configuración → Correo).
pub(super) fn accounts_panel(ui: &mut Ui, accounts: &[Account], state: &MailState, form: &mut Option<(String, String, String)>) -> Option<Action> {
    let mut action = None;
    for (i, a) in accounts.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{}  {}", icon::ENVELOPE_SIMPLE, a.correo)).size(14.0));
            let (host, _) = mail::server_for(a);
            ui.label(RichText::new(host).size(12.0).color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Quitar").clicked() {
                    action = Some(Action::RemoveMailAccount(i));
                }
                if ui.button("Probar").clicked() {
                    action = Some(Action::TestMailAccount(i));
                }
            });
        });
        let status = state.tests.get(&a.correo).cloned().or_else(|| state.errors.get(&a.correo).cloned().map(Err));
        match status {
            Some(Ok(())) => {
                ui.label(RichText::new(format!("{} Conexión correcta", icon::CHECK_CIRCLE)).size(12.5).color(SUCCESS));
            }
            Some(Err(e)) => {
                ui.label(RichText::new(format!("{} {e}", icon::WARNING_CIRCLE)).size(12.5).color(RED));
            }
            None => {}
        }
        ui.add_space(6.0);
    }
    match form {
        None => {
            if ui.button(format!("{}  Agregar correo", icon::PLUS)).clicked() {
                *form = Some((String::new(), String::new(), String::new()));
            }
        }
        Some((email, pass, server)) => {
            let mut close = false;
            Frame::new().fill(BG_SIDE).stroke(Stroke::new(1.0, theme::BORDER)).corner_radius(10).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(egui::TextEdit::singleline(email).hint_text("tu@gmail.com").desired_width(f32::INFINITY));
                ui.add(egui::TextEdit::singleline(pass).hint_text("Contraseña de aplicación (no la de siempre)").password(true).desired_width(f32::INFINITY));
                ui.add(egui::TextEdit::singleline(server).hint_text("Servidor IMAP (opcional: se deduce del correo)").desired_width(f32::INFINITY));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let ok = email.contains('@') && !pass.trim().is_empty();
                    if ui.add_enabled(ok, egui::Button::new("Agregar")).clicked() {
                        action = Some(Action::AddMailAccount(Account { correo: email.trim().to_string(), clave: pass.trim().to_string(), servidor: server.trim().to_string() }));
                        close = true;
                    }
                    if ui.button("Cancelar").clicked() {
                        close = true;
                    }
                });
                ui.add_space(4.0);
                ui.label(RichText::new("¿Qué es la contraseña de aplicación? Una contraseña especial que se crea en tu cuenta y sirve solo para esta app; se puede borrar cuando quieras.").size(12.0).color(MUTED));
                for (service, steps, url) in HELP {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(service).size(12.5).strong());
                        ui.label(RichText::new(steps).size(12.5).color(MUTED));
                        if ui.link(RichText::new("Abrir").size(12.5)).clicked() {
                            gcal::open_browser(url);
                        }
                    });
                }
            });
            if close {
                *form = None;
            }
        }
    }
    action
}

/// "Juan Pérez <juan@x.cl>" -> "Juan Pérez"
fn short_name(s: &str) -> String {
    let first = s.split(',').next().unwrap_or(s).trim();
    match first.split_once('<') {
        Some((name, _)) if !name.trim().is_empty() => name.trim().trim_matches('"').to_string(),
        Some((_, addr)) => addr.trim_end_matches('>').to_string(),
        None => first.to_string(),
    }
}

impl NotesApp {
    /// Revisa el correo cada 15 minutos y le pasa a la IA lo nuevo, de a lotes.
    pub(super) fn handle_mail(&mut self) {
        let mut msgs = Vec::new();
        if let Some(rx) = &self.mail.rx {
            msgs.extend(rx.try_iter());
        }
        for m in msgs {
            match m {
                Msg::Fetched(account, Ok((mails, uids))) => {
                    self.mail.errors.remove(&account);
                    self.mail.store.merge(mails);
                    self.mail.store.last_uid.extend(uids);
                    self.mail.store.save();
                }
                Msg::Fetched(account, Err(e)) => {
                    self.mail.errors.insert(account, e);
                }
                Msg::FetchDone => {
                    self.mail.fetching = false;
                    self.mail.rx = None;
                    self.mail.last_ok = Some(Local::now());
                }
                Msg::Analyzed(r) => {
                    self.mail.analyzing = false;
                    self.mail.rx = None;
                    match r.and_then(|t| mail_ai::parse_reply(&t)) {
                        Ok(results) => {
                            self.mail.ai_error = None;
                            self.apply_mail_results(results);
                        }
                        Err(e) => self.mail.ai_error = Some(e),
                    }
                }
                Msg::Tested(account, r) => {
                    self.mail.tests.insert(account, r);
                    self.mail.rx = None;
                }
            }
        }
        if self.cfg.correos.is_empty() {
            self.mail.stop_watchers();
            return;
        }
        // Avisos de correo nuevo (IMAP IDLE), uno por cuenta; se reinician si cambian las cuentas.
        let key = if self.cfg.correo_al_llegar { self.cfg.correos.iter().map(|a| format!("{}|{}|{}", a.correo, a.clave, a.servidor)).collect::<Vec<_>>().join(";") } else { String::new() };
        if key != self.mail.watch_key {
            self.mail.stop_watchers();
            if !key.is_empty() {
                let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let (tx, rx) = mpsc::channel();
                for a in self.cfg.correos.clone() {
                    let (tx, stop, ctx) = (tx.clone(), stop.clone(), self.ctx.clone());
                    std::thread::spawn(move || {
                        let email = a.correo.clone();
                        mail::watch(a, stop, |r| {
                            let _ = tx.send((email.clone(), r));
                            ctx.request_repaint();
                        });
                    });
                }
                self.mail.watch_rx = Some(rx);
                self.mail.watch_stop = Some(stop);
                self.mail.watch_key = key;
            }
        }
        if let Some(rx) = &self.mail.watch_rx {
            for (account, r) in rx.try_iter() {
                match r {
                    Ok(()) => self.mail.due = true,
                    Err(e) => {
                        self.mail.errors.insert(account, format!("aviso de correo nuevo: {e}"));
                    }
                }
            }
        }
        // Al abrir: lo que llegó mientras la app estaba cerrada (si avisa al llegar).
        if !self.mail.started {
            self.mail.started = true;
            self.mail.due |= self.cfg.correo_al_llegar;
        }
        // Una vez al día, a la hora elegida (o al abrir la app, si a esa hora estaba cerrada).
        if let Ok(at) = chrono::NaiveTime::parse_from_str(self.cfg.correo_diario.trim(), "%H:%M") {
            let now = Local::now();
            let day = now.format("%Y-%m-%d").to_string();
            if now.time() >= at && self.mail.store.last_daily != day {
                self.mail.store.last_daily = day;
                self.mail.store.save();
                self.mail.due = true;
            }
        }
        if self.mail.rx.is_some() {
            return;
        }
        // Revisar.
        if self.mail.due {
            self.mail.due = false;
            self.mail.fetching = true;
            let accounts = self.cfg.correos.clone();
            let last_uid = self.mail.store.last_uid.clone();
            let ctx = self.ctx.clone();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                for a in accounts {
                    let r = mail::fetch(&a, &last_uid);
                    let _ = tx.send(Msg::Fetched(a.correo.clone(), r));
                    ctx.request_repaint();
                }
                let _ = tx.send(Msg::FetchDone);
                ctx.request_repaint();
            });
            self.mail.rx = Some(rx);
            return;
        }
        // Que la IA lea lo nuevo (si no falló en esta vuelta).
        if self.ai.is_err() || self.mail.ai_error.is_some() {
            return;
        }
        for m in self.mail.store.mails.iter_mut().filter(|m| m.bulk && !m.analyzed) {
            m.analyzed = true;
        }
        let batch: Vec<Mail> = self.mail.store.mails.iter().filter(|m| !m.analyzed).take(mail_ai::BATCH).cloned().collect();
        if batch.is_empty() {
            return;
        }
        let pending: Vec<(String, agenda::Task)> =
            self.agenda.tasks().into_iter().filter(|t| !t.done).take(60).enumerate().map(|(i, t)| (format!("t{}", i + 1), t)).collect();
        let refs: Vec<&Mail> = batch.iter().collect();
        let facts = self.ai_facts();
        let (system, user) = mail_ai::prompt(&refs, &pending, &self.vault.workspaces, &facts, &today());
        self.mail.batch = batch.iter().enumerate().map(|(i, m)| (format!("c{}", i + 1), m.id.clone())).collect();
        self.mail.batch_tasks = pending;
        self.mail.analyzing = true;
        let cfg = self.cfg.clone();
        let ctx = self.ctx.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Analyzed(ai::complete(&cfg, &system, &user)));
            ctx.request_repaint();
        });
        self.mail.rx = Some(rx);
    }

    /// Lo que entendió la IA: resumen en cada correo; compromisos y fechas a la agenda (con Deshacer).
    fn apply_mail_results(&mut self, results: Vec<mail_ai::MailResult>) {
        let snapshot = self.agenda.snapshot();
        let created = today();
        let (mut tasks, mut events) = (Vec::new(), Vec::new());
        let mut found = 0;
        let batch = std::mem::take(&mut self.mail.batch);
        let pending = std::mem::take(&mut self.mail.batch_tasks);
        for (key, id) in &batch {
            let Some(m) = self.mail.store.mails.iter_mut().find(|m| m.id == *id) else { continue };
            m.analyzed = true;
            let Some(r) = results.iter().find(|r| r.id.eq_ignore_ascii_case(key)) else { continue };
            m.summary = r.resumen.trim().to_string();
            m.important = r.importante;
            m.workspace = self.vault.workspaces.iter().find(|w| w.eq_ignore_ascii_case(r.espacio.trim())).cloned().unwrap_or_default();
            for c in r.compromisos.iter().filter(|c| !c.que.trim().is_empty()) {
                let me = mail_ai::is_me(&c.quien);
                let who = c.quien.split_whitespace().collect::<Vec<_>>().join("_");
                let text = if me { c.que.trim().to_string() } else { format!("{} @{who}", c.que.trim()) };
                let due = Some(c.fecha.trim()).filter(|d| agenda::is_date(d));
                let tid = new_task_id();
                tasks.push(agenda::format_mail_task(&created, &text, &m.workspace, due, &m.id, &tid));
                m.task_ids.push(tid);
                let when = due.map(|d| format!(" · {}", long_date(d))).unwrap_or_default();
                m.items.push(if me { format!("Tú: {}{when}", c.que.trim()) } else { format!("{}: {}{when}", c.quien.trim(), c.que.trim()) });
                found += 1;
            }
            for e in r.eventos.iter().filter(|e| agenda::is_date(e.fecha.trim()) && !e.titulo.trim().is_empty()) {
                let time = Some(e.hora.trim()).filter(|h| agenda::is_time(h));
                let line = agenda::format_mail_event(e.fecha.trim(), time, &e.titulo, &m.workspace, &m.id);
                events.push(line.clone());
                m.event_lines.push(line);
                m.items.push(format!("{} · {}{}", e.titulo.trim(), long_date(e.fecha.trim()), time.map(|t| format!(" {t}")).unwrap_or_default()));
                found += 1;
            }
            for k in &r.cumple {
                if let Some((_, t)) = pending.iter().find(|(pk, _)| pk.eq_ignore_ascii_case(k.trim())) {
                    let task_id = t.id.clone().unwrap_or_else(|| t.raw.clone());
                    if !m.fulfills.iter().any(|f| f.task_id == task_id) {
                        m.fulfills.push(Fulfill { task_id, task_text: t.text.clone(), resolved: false });
                    }
                }
            }
        }
        self.mail.store.save();
        if tasks.is_empty() && events.is_empty() {
            return;
        }
        if let Err(e) = self.agenda.add_lines(&tasks, &events) {
            self.msg(format!("No se pudo escribir tareas/agenda: {e}"));
            return;
        }
        self.gcal_dirty = true;
        self.undo = Some(Undo { files: Vec::new(), renamed: None, agenda: snapshot, at: Instant::now(), moved: Vec::new(), created_dir: None });
        self.msg(format!("Correo · {} en Tareas y Agenda", plural(found, "compromiso o fecha")));
    }

    /// "Sí, se cumplió": marca la tarea (y su casilla en la nota).
    pub(super) fn mail_fulfill(&mut self, mail_id: &str, i: usize, done: bool) {
        let Some(f) = self.mail.store.mails.iter_mut().find(|m| m.id == mail_id).and_then(|m| m.fulfills.get_mut(i)) else { return };
        f.resolved = true;
        let task_id = f.task_id.clone();
        self.mail.store.save();
        if !done {
            return;
        }
        let task = self.agenda.tasks().into_iter().find(|t| t.id.as_deref() == Some(task_id.as_str()) || t.raw == task_id);
        if let Some(t) = task.filter(|t| !t.done) {
            self.apply(Action::ToggleTask(t.raw.clone()));
            self.msg(format!("Hecho: {}", t.text));
        }
    }

    /// Quita lo que la IA sacó de un correo.
    pub(super) fn mail_remove_items(&mut self, mail_id: &str) {
        let Some(m) = self.mail.store.mails.iter_mut().find(|m| m.id == mail_id) else { return };
        let (ids, events) = (std::mem::take(&mut m.task_ids), std::mem::take(&mut m.event_lines));
        m.items.clear();
        self.mail.store.save();
        for id in ids {
            let _ = self.agenda.remove_by_id(&id);
        }
        let _ = self.agenda.remove_events(&events);
        self.gcal_dirty = true;
        self.msg("Se quitó lo que la IA sacó de ese correo");
    }

    /// Guarda un correo como nota (en su espacio, o en el actual).
    pub(super) fn mail_save_note(&mut self, mail_id: &str) {
        let Some(m) = self.mail.store.mails.iter().find(|m| m.id == mail_id).cloned() else { return };
        let ws = if m.workspace.is_empty() { self.ws.clone() } else { m.workspace.clone() };
        let text = format!("Correo de {} para {} · {}\n\n{}\n", m.from, m.to, m.date, m.body);
        self.ws = ws;
        self.save_text_note(&format!("Correo · {}", m.subject.chars().take(50).collect::<String>()), text);
    }

    pub(super) fn test_mail_account(&mut self, i: usize) {
        let Some(a) = self.cfg.correos.get(i).cloned() else { return };
        if self.mail.rx.is_some() {
            self.msg("El correo se está revisando; prueba en un momento");
            return;
        }
        self.mail.tests.remove(&a.correo);
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Tested(a.correo.clone(), mail::test_login(&a)));
            ctx.request_repaint();
        });
        self.mail.rx = Some(rx);
    }

    pub(super) fn mail_view(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        let accounts: Vec<String> = self.cfg.correos.iter().map(|a| a.correo.clone()).collect();
        let checks = self.mail.open_checks();
        let mut toggle_open: Option<String> = None;
        let mut toggle_bulk = false;
        Self::column(ui, "correos", |ui, _| {
            if view_header(ui, "Correos", "Lo que la IA encontró en tu bandeja de entrada y en tus enviados") {
                action = Some(Action::CloseResults);
            }
            if accounts.is_empty() {
                ui.label(RichText::new("Agrega tu correo y la IA revisará tus correos para no perder compromisos ni fechas.").color(MUTED));
                ui.add_space(6.0);
                if ui.button(format!("{}  Agregar correo", icon::PLUS)).clicked() {
                    action = Some(Action::OpenSettings(Section::Mail));
                }
                return;
            }
            ui.horizontal_wrapped(|ui| {
                let status = if self.mail.fetching {
                    "revisando…".to_string()
                } else if self.mail.analyzing {
                    "la IA está leyendo…".to_string()
                } else if let Some(t) = self.mail.last_ok {
                    format!("revisado a las {}", t.format("%H:%M"))
                } else {
                    "sin revisar todavía".to_string()
                };
                let mut when = Vec::new();
                if self.cfg.correo_al_llegar {
                    when.push("al llegar un correo".to_string());
                }
                if !self.cfg.correo_diario.trim().is_empty() {
                    when.push(format!("a diario a las {}", self.cfg.correo_diario.trim()));
                }
                let status = if when.is_empty() { status } else { format!("{status} · revisa {}", when.join(" y ")) };
                ui.label(RichText::new(format!("{} · {status}", accounts.join(", "))).size(12.5).color(MUTED));
                if self.mail.busy() {
                    ui.spinner();
                } else if ui.link(RichText::new("Revisar ahora").size(12.5)).clicked() {
                    action = Some(Action::CheckMail);
                }
            });
            for (a, e) in &self.mail.errors {
                ui.label(RichText::new(format!("{} {a}: {e}", icon::WARNING_CIRCLE)).size(12.5).color(RED));
            }
            if let Some(e) = &self.mail.ai_error {
                ui.label(RichText::new(format!("{} La IA no pudo leer los correos: {e}", icon::WARNING_CIRCLE)).size(12.5).color(RED));
            }
            ui.add_space(10.0);

            // Verificar compromisos.
            for (m, i) in &checks {
                let f = &m.fulfills[*i];
                Frame::new().fill(Color32::from_rgb(236, 248, 241)).stroke(Stroke::new(1.0, Color32::from_rgb(170, 220, 190))).corner_radius(10).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(format!("{} Verificar un compromiso", icon::CHECK_CIRCLE)).size(12.5).color(SUCCESS));
                    ui.label(RichText::new(format!("¿Se cumplió «{}»?", f.task_text)).size(14.5));
                    ui.label(RichText::new(format!("Llegó «{}» de {} · {}", m.subject, short_name(&m.from), m.date)).size(12.5).color(MUTED));
                    ui.horizontal(|ui| {
                        if ui.button("Sí, marcar hecho").clicked() {
                            action = Some(Action::MailFulfill(m.id.clone(), *i, true));
                        }
                        if ui.button("Todavía no").clicked() {
                            action = Some(Action::MailFulfill(m.id.clone(), *i, false));
                        }
                    });
                });
                ui.add_space(8.0);
            }

            let bulk = self.mail.store.mails.iter().filter(|m| m.bulk || (m.analyzed && m.items.is_empty() && !m.important)).count();
            let mut day = String::new();
            for m in self.mail.store.mails.iter().filter(|m| self.mail.show_bulk || !(m.bulk || (m.analyzed && m.items.is_empty() && !m.important))) {
                let d = m.date.get(..10).unwrap_or("").to_string();
                if d != day {
                    day = d.clone();
                    ui.add_space(8.0);
                    ui.label(RichText::new(if d == today() { format!("Hoy · {}", long_date(&d)) } else { long_date(&d) }).font(theme::bold(14.0)));
                }
                let who = if m.sent { format!("Tú → {}", short_name(&m.to)) } else { short_name(&m.from) };
                let r = Frame::new().inner_margin(Margin::symmetric(4, 6)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let color = if m.important { TEXT } else { MUTED };
                        ui.label(RichText::new(&who).size(14.0).strong().color(color));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let time = m.date.get(11..).unwrap_or("");
                            ui.label(RichText::new(if m.sent { format!("{time} · enviado") } else { time.to_string() }).size(12.0).color(MUTED));
                        });
                    });
                    ui.label(RichText::new(&m.subject).size(14.0));
                    if !m.summary.is_empty() {
                        ui.label(RichText::new(&m.summary).size(12.5).color(MUTED));
                    } else if !m.analyzed {
                        ui.label(RichText::new("La IA todavía no lo lee…").size(12.0).color(MUTED));
                    }
                    if !m.items.is_empty() || !m.workspace.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
                            for it in &m.items {
                                let c = theme::tag_colors(it.split(':').next().unwrap_or(it));
                                ui.label(RichText::new(format!("{} {it}", icon::CHECK_SQUARE)).size(12.0).color(c.text).background_color(c.bg));
                            }
                            if !m.workspace.is_empty() {
                                ui.label(RichText::new(format!("→ {}", m.workspace)).size(12.0).color(MUTED));
                            }
                        });
                    }
                    if self.mail.open.as_deref() == Some(m.id.as_str()) {
                        ui.add_space(6.0);
                        Frame::new().fill(BG_SIDE).corner_radius(8).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(format!("De: {}\nPara: {}", m.from, m.to)).size(12.0).color(MUTED));
                            ui.add_space(4.0);
                            ui.label(RichText::new(m.body.chars().take(3000).collect::<String>()).size(13.0));
                        });
                        ui.horizontal(|ui| {
                            if ui.button(format!("{} Guardar como nota", icon::FLOPPY_DISK)).clicked() {
                                action = Some(Action::MailSaveNote(m.id.clone()));
                            }
                            if !m.task_ids.is_empty() || !m.event_lines.is_empty() {
                                if ui.button(format!("{} Quitar lo que sacó la IA", icon::TRASH)).clicked() {
                                    action = Some(Action::MailRemoveItems(m.id.clone()));
                                }
                            }
                            if m.account.to_lowercase().ends_with("@gmail.com") && !m.message_id.is_empty() {
                                if ui.button(format!("{} Abrir en Gmail", icon::ARROW_SQUARE_OUT)).clicked() {
                                    gcal::open_browser(&format!("https://mail.google.com/mail/u/0/#search/rfc822msgid%3A{}", m.message_id.trim_matches(['<', '>'])));
                                }
                            }
                        });
                    }
                });
                let hit = ui.interact(r.response.rect, Id::new(("correo", &m.id)), Sense::click());
                if hit.hovered() {
                    ui.painter().rect_stroke(r.response.rect, 6, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
                }
                if hit.clicked() {
                    toggle_open = Some(m.id.clone());
                }
            }
            if self.mail.store.mails.is_empty() {
                ui.label(RichText::new("Todavía no hay correos: la primera revisión trae los de la última semana.").color(MUTED));
            }
            if bulk > 0 {
                ui.add_space(8.0);
                let label = if self.mail.show_bulk { "Ocultar boletines y correos sin compromisos".to_string() } else { format!("{} sin compromisos (boletines, avisos…): ver", plural(bulk, "correo")) };
                if ui.link(RichText::new(label).size(12.5).color(MUTED)).clicked() {
                    toggle_bulk = true;
                }
            }
        });
        if let Some(id) = toggle_open {
            self.mail.open = if self.mail.open.as_deref() == Some(id.as_str()) { None } else { Some(id) };
        }
        if toggle_bulk {
            self.mail.show_bulk = !self.mail.show_bulk;
        }
        if self.esc(ui) {
            action = Some(Action::CloseResults);
        }
        action
    }

    /// Abrir la vista Correos con un correo abierto.
    pub(super) fn open_mail(&mut self, id: String) {
        self.mail.open = Some(id);
        self.mail.show_bulk = true;
        self.show_in_tab(View::Mail);
    }

    /// Para Preguntar: los correos recientes, en una línea cada uno.
    pub(super) fn mail_context(&self) -> Vec<String> {
        let since = (Local::now() - chrono::Duration::days(30)).format("%Y-%m-%d").to_string();
        self.mail
            .store
            .mails
            .iter()
            .filter(|m| !m.bulk && m.date >= since)
            .take(80)
            .map(|m| {
                let who = if m.sent { format!("enviado a {}", short_name(&m.to)) } else { format!("de {}", short_name(&m.from)) };
                let what = if m.summary.is_empty() { m.body.chars().take(200).collect::<String>().replace('\n', " ") } else { m.summary.clone() };
                format!("{} · {who} · «{}» · {what}", m.date, m.subject)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lo que entiende la IA de un correo va a Tareas y Agenda, se puede quitar y verifica compromisos.
    #[test]
    fn mail_results_become_tasks_and_checks() {
        let dir = std::env::temp_dir().join(format!("nodex-correo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("Consorcio")).unwrap();
        fs::write(dir.join("tareas.txt"), "2026-09-24 enviar planos @Juan +Consorcio due:2026-09-30 nota:Consorcio/Reunión id:cic01\n").unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.mail.store.mails = vec![Mail { id: "jp@gmail.com:INBOX:3".into(), from: "Juan <juan@obra.cl>".into(), subject: "Planos rev. B".into(), date: "2026-09-26 09:12".into(), ..Mail::default() }];
        app.mail.batch = vec![("c1".into(), "jp@gmail.com:INBOX:3".into())];
        app.mail.batch_tasks = app.agenda.tasks().into_iter().enumerate().map(|(i, t)| (format!("t{}", i + 1), t)).collect();
        let results = mail_ai::parse_reply(
            r#"{"correos": [{"id": "c1", "resumen": "Juan manda los planos y pide la cubicación", "importante": true, "espacio": "consorcio",
               "compromisos": [{"quien": "yo", "que": "Enviar la cubicación", "fecha": "2026-09-29"}],
               "eventos": [{"titulo": "Visita a obra", "fecha": "2026-10-01", "hora": "10:00"}], "cumple": ["t1"]}]}"#,
        )
        .unwrap();
        app.apply_mail_results(results);
        let m = &app.mail.store.mails[0];
        assert!(m.analyzed && m.important && m.workspace == "Consorcio");
        assert_eq!(m.items.len(), 2);
        let tasks = app.agenda.tasks();
        let t = tasks.iter().find(|t| t.mail.is_some()).unwrap();
        assert_eq!((t.text.as_str(), t.due.as_deref(), t.project.as_str()), ("Enviar la cubicación", Some("2026-09-29"), "Consorcio"));
        assert_eq!(app.agenda.events()[0].title, "Visita a obra");
        // Verificar: "Sí" marca la tarea de Juan como hecha.
        assert_eq!(app.mail.open_checks().len(), 1);
        app.mail_fulfill("jp@gmail.com:INBOX:3", 0, true);
        assert!(app.agenda.tasks().iter().find(|t| t.id.as_deref() == Some("cic01")).unwrap().done);
        assert!(app.mail.open_checks().is_empty());
        // Quitar lo que sacó la IA.
        app.mail_remove_items("jp@gmail.com:INBOX:3");
        assert!(app.agenda.tasks().iter().all(|t| t.mail.is_none()));
        assert!(app.agenda.events().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
