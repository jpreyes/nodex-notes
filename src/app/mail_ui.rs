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
/// `form`: el formulario abierto (correo, clave, servidor, y qué cuenta se edita; `None` = una nueva).
pub(super) fn accounts_panel(ui: &mut Ui, accounts: &[Account], state: &MailState, form: &mut Option<(String, String, String, Option<usize>)>) -> Option<Action> {
    let mut action = None;
    for (i, a) in accounts.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{}  {}", icon::ENVELOPE_SIMPLE, a.correo)).size(14.0));
            let (host, _) = mail::server_for(a);
            ui.label(RichText::new(host).size(12.5).color(MUTED()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Quitar").clicked() {
                    action = Some(Action::RemoveMailAccount(i));
                }
                if ui.button("Editar").on_hover_text("Cambiar el correo, la contraseña de aplicación o el servidor").clicked() {
                    *form = Some((a.correo.clone(), String::new(), a.servidor.clone(), Some(i)));
                }
                if ui.button("Probar").clicked() {
                    action = Some(Action::TestMailAccount(i));
                }
            });
        });
        let status = state.tests.get(&a.correo).cloned().or_else(|| state.errors.get(&a.correo).cloned().map(Err));
        match status {
            Some(Ok(())) => {
                ui.label(RichText::new(format!("{} Conexión correcta", icon::CHECK_CIRCLE)).size(12.5).color(SUCCESS()));
            }
            Some(Err(e)) => {
                ui.label(RichText::new(format!("{} {e}", icon::WARNING_CIRCLE)).size(12.5).color(RED()));
            }
            None => {}
        }
        ui.add_space(6.0);
    }
    match form {
        None => {
            if ui.button(format!("{}  Agregar correo", icon::PLUS)).clicked() {
                *form = Some((String::new(), String::new(), String::new(), None));
            }
        }
        Some((email, pass, server, editing)) => {
            let editing = *editing;
            let mut close = false;
            Frame::new().fill(BG_SIDE()).stroke(Stroke::new(1.0, theme::BORDER())).corner_radius(10).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if editing.is_some() {
                    ui.label(RichText::new("Cambiar la conexión").font(theme::bold(14.0)));
                }
                ui.add(egui::TextEdit::singleline(email).hint_text("tu@gmail.com").desired_width(f32::INFINITY));
                let hint = if editing.is_some() { "Contraseña de aplicación (vacía = se mantiene la actual)" } else { "Contraseña de aplicación (no la de siempre)" };
                ui.add(egui::TextEdit::singleline(pass).hint_text(hint).password(true).desired_width(f32::INFINITY));
                ui.add(egui::TextEdit::singleline(server).hint_text("Servidor IMAP (opcional: se deduce del correo)").desired_width(f32::INFINITY));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let ok = email.contains('@') && (editing.is_some() || !pass.trim().is_empty());
                    if ui.add_enabled(ok, egui::Button::new(if editing.is_some() { "Guardar" } else { "Agregar" })).clicked() {
                        let a = Account { correo: email.trim().to_string(), clave: pass.trim().to_string(), servidor: server.trim().to_string() };
                        action = Some(match editing {
                            Some(i) => Action::UpdateMailAccount(i, a),
                            None => Action::AddMailAccount(a),
                        });
                        close = true;
                    }
                    if ui.button("Cancelar").clicked() {
                        close = true;
                    }
                });
                ui.add_space(4.0);
                ui.label(RichText::new("¿Qué es la contraseña de aplicación? Una contraseña especial que se crea en tu cuenta y sirve solo para esta app; se puede borrar cuando quieras.").size(12.5).color(MUTED()));
                for (service, steps, url) in HELP {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(service).size(12.5).strong());
                        ui.label(RichText::new(steps).size(12.5).color(MUTED()));
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
/// Cómo se reconoce un correo en todos los equipos: su identificador de mensaje, o cuenta,
/// fecha y asunto si no lo trae.
fn mail_key(m: &Mail) -> String {
    let id = m.message_id.trim();
    if id.is_empty() { format!("{}|{}|{}", m.account, m.date, m.subject.trim()) } else { id.to_string() }
}

/// Cómo empieza la línea de un correo en la nota de hoy: "Correo de Ana (26 sep): Visita."
pub(super) fn mail_prefix(m: &Mail) -> String {
    let d = NaiveDate::parse_from_str(m.date.get(..10).unwrap_or(""), "%Y-%m-%d")
        .map(|d| format!("{} {}", d.day(), MESES[d.month0() as usize]))
        .unwrap_or_default();
    let who = if m.sent { format!("Correo a {}", short_name(&m.to)) } else { format!("Correo de {}", short_name(&m.from)) };
    format!("{who} ({d}): {}.", m.subject.trim().trim_end_matches('.'))
}

/// El asunto sin «Re:», «RE:», «RV:», «Fwd:», «Resp:» (en minúsculas y sin tildes).
pub(super) fn base_subject(s: &str) -> String {
    let mut t = s.trim();
    loop {
        let lower = t.to_lowercase();
        let Some(p) = ["re:", "rv:", "fw:", "fwd:", "resp:", "res:", "aw:", "r:"].iter().find(|p| lower.starts_with(*p)) else { break };
        t = t[p.len()..].trim_start();
    }
    vault::fold(t.trim())
}

/// ¿El asunto dice que es una respuesta? («Re: …», «RV: …»)
fn is_reply_subject(s: &str) -> bool {
    base_subject(s) != vault::fold(s.trim())
}

/// Las direcciones de correo de un «De» o «Para».
fn addresses(s: &str) -> Vec<String> {
    s.split([',', ';', '<', '>', ' ']).filter(|w| w.contains('@')).map(|w| w.trim().trim_matches('"').to_lowercase()).collect()
}

/// Los correos anteriores del mismo hilo que `m` (a los que responde), el más antiguo primero:
/// por los encabezados de respuesta o, si no los hay, por el asunto («Re: …») y las personas.
pub(super) fn thread_of<'a>(m: &Mail, all: &'a [Mail]) -> Vec<&'a Mail> {
    let people: Vec<String> = addresses(&m.from).into_iter().chain(addresses(&m.to)).collect();
    let base = base_subject(&m.subject);
    let mut v: Vec<&Mail> = all
        .iter()
        .filter(|c| c.id != m.id && c.date <= m.date)
        .filter(|c| {
            let by_header = !c.message_id.is_empty() && m.reply_to.iter().any(|r| r.trim_matches(['<', '>']) == c.message_id.trim_matches(['<', '>']));
            let by_subject = is_reply_subject(&m.subject)
                && !base.is_empty()
                && base_subject(&c.subject) == base
                && addresses(&c.from).iter().chain(addresses(&c.to).iter()).any(|a| people.contains(a));
            by_header || by_subject
        })
        .collect();
    v.sort_by(|a, b| a.date.cmp(&b.date));
    v
}

/// La línea de una nota donde quedó anotado alguno de estos correos (por cómo empieza): la nota
/// y la línea. Se busca primero el más antiguo.
fn find_mail_line<'a>(notes: impl Iterator<Item = (&'a Path, &'a str)> + Clone, prefixes: &[String]) -> Option<(PathBuf, String)> {
    for p in prefixes {
        for (path, text) in notes.clone() {
            if let Some(l) = text.lines().find(|l| l.contains(p.as_str()) && !lines::is_follow_up(l)) {
                return Some((path.to_path_buf(), l.trim_end_matches('\r').to_string()));
            }
        }
    }
    None
}

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
        // Se avisa a los otros equipos que estos correos los está viendo este (para no anotarlos dos veces).
        let keys: Vec<String> = batch.iter().filter(|m| !m.bulk).map(mail_key).collect();
        crate::claims::claim_mails(&self.vault.root, &keys, &self.machine);
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

    /// Lo que entendió la IA: el resumen de cada correo y, si importa, una línea en la nota de hoy
    /// de su espacio. El organizador de notas hace el resto: la lleva a su nota, le pone
    /// etiquetas, casilla y fecha a sus tareas, manda las citas a la Agenda y sugiere espacios.
    fn apply_mail_results(&mut self, results: Vec<mail_ai::MailResult>) {
        let snapshot = self.agenda.snapshot();
        let mut entries: Vec<(String, String, String)> = Vec::new(); // (espacio, línea, id del correo)
        // Comienzo de cada línea ("Correo de X (26 sep): asunto."), para no repetirla si ya está.
        let mut prefixes: HashMap<String, String> = HashMap::new();
        let owners = crate::claims::mail_owners(&self.vault.root);
        let me = self.machine.clone();
        let batch = std::mem::take(&mut self.mail.batch);
        let pending = std::mem::take(&mut self.mail.batch_tasks);
        // De cada correo, cómo empiezan las líneas de los correos a los que responde.
        let threads: HashMap<String, Vec<String>> = batch
            .iter()
            .filter_map(|(_, id)| {
                let m = self.mail.store.mails.iter().find(|m| m.id == *id)?;
                let prev: Vec<String> = thread_of(m, &self.mail.store.mails).into_iter().filter(|c| !c.noted.is_empty()).map(mail_prefix).collect();
                (!prev.is_empty()).then(|| (id.clone(), prev))
            })
            .collect();
        // Respuestas a correos ya anotados: (nota, línea del original, seguimiento, fecha, correo).
        let mut replies: Vec<(PathBuf, String, String, String, String)> = Vec::new();
        let root = self.vault.root.clone();
        for (key, id) in &batch {
            let Some(m) = self.mail.store.mails.iter_mut().find(|m| m.id == *id) else { continue };
            m.analyzed = true;
            let Some(r) = results.iter().find(|r| r.id.eq_ignore_ascii_case(key)) else { continue };
            m.summary = r.resumen.trim().to_string();
            m.important = r.importante;
            m.workspace = self.vault.workspaces.iter().find(|w| w.eq_ignore_ascii_case(r.espacio.trim())).cloned().unwrap_or_default();
            m.items.clear();
            for c in r.compromisos.iter().filter(|c| !c.que.trim().is_empty()) {
                let me = mail_ai::is_me(&c.quien);
                let due = Some(c.fecha.trim()).filter(|d| agenda::is_date(d));
                let when = due.map(|d| format!(" · {}", long_date(d))).unwrap_or_default();
                m.items.push(if me { format!("Tú: {}{when}", c.que.trim()) } else { format!("{}: {}{when}", c.quien.trim(), c.que.trim()) });
            }
            for e in r.eventos.iter().filter(|e| agenda::is_date(e.fecha.trim()) && !e.titulo.trim().is_empty()) {
                let time = Some(e.hora.trim()).filter(|h| agenda::is_time(h));
                m.items.push(format!("{} · {}{}", e.titulo.trim(), long_date(e.fecha.trim()), time.map(|t| format!(" {t}")).unwrap_or_default()));
            }
            // Si importa (pide algo, fija una fecha o trae un compromiso), se anota.
            let relevant = m.important || !r.compromisos.is_empty() || !r.eventos.is_empty();
            // Otro equipo ya tomó este correo: lo anota él.
            if relevant && m.noted.is_empty() && owners.get(&mail_key(m)).is_some_and(|o| *o != me) {
                m.noted = "otro equipo".into();
            }
            // Responde a un correo que ya está en las notas: va como su seguimiento («↳»).
            let original = threads.get(&m.id).and_then(|prev| {
                let open = std::iter::once((self.note.path.as_path(), self.note.text.as_str()));
                let others = self.vault.notes_iter().filter(|n| n.path != self.note.path).map(|n| (n.path.as_path(), n.text.as_str()));
                find_mail_line(open.chain(others), prev)
            });
            // (Una respuesta en un hilo anotado se anota aunque por sí sola no parezca importante.)
            if (relevant || original.is_some()) && m.noted.is_empty() && !m.summary.is_empty() {
                if let Some((path, line)) = original {
                    let who = if m.sent { "Respondiste".to_string() } else { format!("Respuesta de {}", short_name(&m.from)) };
                    let date = m.date.get(..10).filter(|d| agenda::is_date(d)).map_or_else(today, str::to_string);
                    replies.push((path.clone(), line, format!("{who}: {}", m.summary.replace('\n', " ")), date, m.id.clone()));
                    m.noted = path.strip_prefix(&root).unwrap_or(&path).with_extension("").to_string_lossy().replace('\\', "/");
                } else {
                    let prefix = mail_prefix(m);
                    let line = format!("{prefix} {}", m.summary.replace('\n', " "));
                    prefixes.insert(m.id.clone(), prefix);
                    entries.push((m.workspace.clone(), line, m.id.clone()));
                }
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
        // Las respuestas: un seguimiento bajo la línea del correo original, donde esté.
        let mut files: Vec<(PathBuf, Option<String>)> = Vec::new();
        let mut details: Vec<String> = Vec::new();
        let mut followed = 0;
        for (path, line, follow, date, _) in &replies {
            let open = *path == self.note.path;
            let old = if open { self.note.text.clone() } else { vault::read_text(path).unwrap_or_default() };
            let Some(idx) = old.split('\n').position(|l| l.trim_end_matches('\r') == line) else { continue };
            let (new, _) = lines::add_follow_up(&old, idx, date, follow);
            if self.analyzed.contains(&ai::fnv(&old)) {
                self.analyzed.insert(ai::fnv(&new));
                self.save_analyzed();
            }
            if open {
                self.note.text = new;
                self.note.dirty = true;
                self.save();
            } else if fs::write(path, &new).is_ok() {
                if let Some(mt) = vault::modified(path) {
                    self.vault.upsert(path.clone(), new, mt);
                }
            } else {
                continue;
            }
            if !files.iter().any(|(p, _)| p == path) {
                files.push((path.clone(), Some(old)));
            }
            followed += 1;
            details.push(format!("↳ en «{}»: {}", vault::stem(path), follow.chars().take(200).collect::<String>()));
            // Si es una tarea pendiente, ¿la respuesta dice que se terminó?
            if lines::parse(line).check == Some(false) {
                let target = super::tracking::Target::Line { note: path.clone(), id: lines::id_of(line), text: line.clone() };
                self.check_done(target, super::tracking::line_title(line), follow.clone());
            }
        }
        if entries.is_empty() && files.is_empty() {
            self.mail.store.save();
            return;
        }
        // Una línea por correo al final de la nota de hoy (la IA lleva cada una a su espacio).
        for path in [self.today_path()] {
            let lines: Vec<&(String, String, String)> = entries.iter().collect();
            let open = path == self.note.path;
            let before = if open { Some(self.note.text.clone()).filter(|t| !t.is_empty() || self.note.disk_mtime.is_some()) } else { vault::read_text(&path).ok() };
            let base = before.clone().unwrap_or_default();
            // Si la nota ya tiene esa línea (la anotó otro equipo y llegó por Dropbox), no se repite.
            let add = lines
                .iter()
                .filter(|(_, _, id)| !prefixes.get(id).is_some_and(|p| base.contains(p.as_str())))
                .map(|(_, l, _)| l.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if add.is_empty() {
                let rel = self.rel(&path);
                for (_, _, id) in lines {
                    if let Some(m) = self.mail.store.mails.iter_mut().find(|m| m.id == *id) {
                        m.noted = rel.clone();
                    }
                }
                continue;
            }
            let text = if base.trim().is_empty() { format!("{add}\n") } else { format!("{}\n{add}\n", base.trim_end()) };
            if open {
                self.note.text = text.clone();
                self.note.dirty = true;
                self.save();
            } else {
                if let Some(dir) = path.parent() {
                    let _ = fs::create_dir_all(dir);
                }
                if let Err(e) = fs::write(&path, &text) {
                    self.msg(format!("No se pudo anotar el correo: {e}"));
                    continue;
                }
                if let Some(mt) = vault::modified(&path) {
                    self.vault.upsert(path.clone(), text, mt);
                }
            }
            // La IA la ordena como cualquier nota del día.
            self.touched.insert(path.clone());
            if !files.iter().any(|(p, _)| *p == path) {
                files.push((path.clone(), before));
            }
            let rel = self.rel(&path);
            for (_, _, id) in lines {
                if let Some(m) = self.mail.store.mails.iter_mut().find(|m| m.id == *id) {
                    m.noted = rel.clone();
                }
            }
        }
        self.mail.store.save();
        if files.is_empty() {
            return; // todo estaba anotado ya
        }
        let notes: Vec<String> = files.iter().map(|(p, _)| self.rel(p)).collect();
        self.undo = Some(Undo { files, renamed: None, agenda: snapshot, at: Instant::now(), moved: Vec::new(), created_dir: None, apart: Vec::new(), keep_tasks: Vec::new(), relinks: Vec::new() });
        let replies_text = if followed > 0 { format!("{} como seguimiento del correo original", plural(followed, "respuesta")) } else { String::new() };
        let new_text = if entries.is_empty() { String::new() } else { format!("{} en la nota de hoy; la IA los ordena", plural(entries.len(), "correo anotado")) };
        self.msg(format!("Correo · {}", [new_text, replies_text].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")));
        details.extend(entries.iter().map(|(_, l, _)| l.chars().take(220).collect::<String>()));
        let what = format!("Anotó {} en {}", plural(entries.len() + followed, "correo"), notes.join(", "));
        self.log_ai(crate::activity::Kind::Correo, notes.first().map_or("", |s| s.as_str()), what, details, true);
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
            view_header(ui, "Correos", "Lo que la IA encontró en tu bandeja de entrada y en tus enviados");
            if accounts.is_empty() {
                ui.label(RichText::new("Agrega tu correo y la IA revisará tus correos para no perder compromisos ni fechas.").color(MUTED()));
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
                ui.label(RichText::new(format!("{} · {status}", accounts.join(", "))).size(12.5).color(MUTED()));
                if self.mail.busy() {
                    ui.spinner();
                } else if ui.link(RichText::new("Revisar ahora").size(12.5)).clicked() {
                    action = Some(Action::CheckMail);
                }
            });
            for (a, e) in &self.mail.errors {
                ui.label(RichText::new(format!("{} {a}: {e}", icon::WARNING_CIRCLE)).size(12.5).color(RED()));
            }
            if let Some(e) = &self.mail.ai_error {
                ui.label(RichText::new(format!("{} La IA no pudo leer los correos: {e}", icon::WARNING_CIRCLE)).size(12.5).color(RED()));
            }
            ui.add_space(10.0);

            // Verificar compromisos.
            for (m, i) in &checks {
                let f = &m.fulfills[*i];
                Frame::new().fill(theme::c(Color32::from_rgb(236, 248, 241))).stroke(Stroke::new(1.0, theme::c(Color32::from_rgb(170, 220, 190)))).corner_radius(10).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(format!("{} Verificar un compromiso", icon::CHECK_CIRCLE)).size(12.5).color(SUCCESS()));
                    ui.label(RichText::new(format!("¿Se cumplió «{}»?", f.task_text)).size(14.5));
                    ui.label(RichText::new(format!("Llegó «{}» de {} · {}", m.subject, short_name(&m.from), m.date)).size(12.5).color(MUTED()));
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
                let mut head = egui::Rect::NOTHING;
                let r = Frame::new().inner_margin(Margin::symmetric(4, 6)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    head = ui
                        .vertical(|ui| {
                            ui.horizontal(|ui| {
                                let color = if m.important { TEXT() } else { MUTED() };
                                ui.label(RichText::new(&who).size(14.0).strong().color(color));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let time = m.date.get(11..).unwrap_or("");
                                    ui.label(RichText::new(if m.sent { format!("{time} · enviado") } else { time.to_string() }).size(12.5).color(MUTED()));
                                });
                            });
                            ui.label(RichText::new(&m.subject).size(14.0));
                        })
                        .response
                        .rect;
                    if !m.summary.is_empty() {
                        ui.label(RichText::new(&m.summary).size(12.5).color(MUTED()));
                    } else if !m.analyzed {
                        ui.label(RichText::new("La IA todavía no lo lee…").size(12.5).color(MUTED()));
                    }
                    if !m.items.is_empty() || !m.workspace.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
                            for it in &m.items {
                                let c = theme::tag_colors(it.split(':').next().unwrap_or(it));
                                ui.label(RichText::new(format!("{} {it}", icon::CHECK_SQUARE)).size(12.5).color(c.text).background_color(c.bg));
                            }
                            if !m.noted.is_empty() {
                                ui.label(RichText::new(format!("→ anotado en {}", m.noted.replace('/', " / "))).size(12.5).color(MUTED()));
                            } else if !m.workspace.is_empty() {
                                ui.label(RichText::new(format!("→ {}", m.workspace)).size(12.5).color(MUTED()));
                            }
                        });
                    }
                    if self.mail.open.as_deref() == Some(m.id.as_str()) {
                        ui.add_space(6.0);
                        Frame::new().fill(BG_SIDE()).corner_radius(8).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(format!("De: {}\nPara: {}", m.from, m.to)).size(12.5).color(MUTED()));
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
                let hit = ui.interact(head, Id::new(("correo", &m.id)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                if hit.hovered() {
                    ui.painter().rect_stroke(r.response.rect, 6, Stroke::new(1.0, theme::BORDER()), egui::StrokeKind::Inside);
                }
                if hit.clicked() {
                    toggle_open = Some(m.id.clone());
                }
            }
            if self.mail.store.mails.is_empty() {
                ui.label(RichText::new("Todavía no hay correos: la primera revisión trae los de la última semana.").color(MUTED()));
            }
            if bulk > 0 {
                ui.add_space(8.0);
                let label = if self.mail.show_bulk { "Ocultar boletines y correos sin compromisos".to_string() } else { format!("{} sin compromisos (boletines, avisos…): ver", plural(bulk, "correo")) };
                if ui.link(RichText::new(label).size(12.5).color(MUTED())).clicked() {
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

    /// Un correo importante se anota en la nota de hoy (una vez) y verifica compromisos.
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
        // Queda anotado en la nota de hoy (una sola, en el Diario), y la IA lo llevará a su espacio.
        let daily = dir.join("Diario").join(format!("{}.md", today()));
        let text = fs::read_to_string(&daily).unwrap();
        assert_eq!(text, "Correo de Juan (26 sep): Planos rev. B. Juan manda los planos y pide la cubicación\n");
        assert_eq!(app.mail.store.mails[0].noted, format!("Diario/{}", today()));
        assert!(app.touched.contains(&daily));
        // No se anota dos veces.
        app.mail.batch = vec![("c1".into(), "jp@gmail.com:INBOX:3".into())];
        app.apply_mail_results(mail_ai::parse_reply(r#"{"correos": [{"id": "c1", "resumen": "otra vez", "importante": true}]}"#).unwrap());
        assert_eq!(fs::read_to_string(&daily).unwrap().lines().count(), 1);
        // Verificar: "Sí" marca la tarea de Juan como hecha.
        assert_eq!(app.mail.open_checks().len(), 1);
        app.mail_fulfill("jp@gmail.com:INBOX:3", 0, true);
        assert!(app.agenda.tasks().iter().find(|t| t.id.as_deref() == Some("cic01")).unwrap().done);
        assert!(app.mail.open_checks().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    /// La respuesta a un correo ya anotado va como seguimiento bajo la línea del original (donde
    /// la haya llevado la IA), no como una línea nueva en la nota de hoy.
    #[test]
    fn replies_become_follow_ups_of_the_original() {
        let dir = std::env::temp_dir().join(format!("nodex-correo-respuesta-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("Docencia")).unwrap();
        let taller = dir.join("Docencia").join("Taller.md");
        fs::write(&taller, "Clases de taller\n- [ ] Correo de Félix Westermeier (28 sep): Horario de consultas. Pide un rato para consultas ^fx001 #taller\nOtra cosa\n").unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.ai = Err("sin IA".into());
        let felix = "Félix Westermeier <felix@uni.cl>";
        let original = Mail {
            id: "jp:INBOX:1".into(),
            from: felix.into(),
            to: "jp@uni.cl".into(),
            subject: "Horario de consultas".into(),
            date: "2026-09-28 09:00".into(),
            message_id: "abc@uni.cl".into(),
            noted: format!("Diario/2026-09-28"),
            analyzed: true,
            ..Mail::default()
        };
        // Mi respuesta (con los encabezados) y otra de Félix que solo dice «RE:» en el asunto.
        let mine = Mail { id: "jp:Sent:2".into(), sent: true, from: "jp@uni.cl".into(), to: felix.into(), subject: "Re: Horario de consultas".into(), date: "2026-09-28 11:00".into(), message_id: "def@uni.cl".into(), reply_to: vec!["abc@uni.cl".into()], ..Mail::default() };
        let his = Mail { id: "jp:INBOX:3".into(), from: felix.into(), to: "jp@uni.cl".into(), subject: "RE: Horario de consultas".into(), date: "2026-09-28 12:00".into(), ..Mail::default() };
        app.mail.store.mails = vec![original, mine, his];
        app.mail.batch = vec![("c1".into(), "jp:Sent:2".into()), ("c2".into(), "jp:INBOX:3".into())];
        let reply = r#"{"correos": [{"id": "c1", "resumen": "Le confirmé un espacio entre las 15:40 y las 16:20", "importante": true},
            {"id": "c2", "resumen": "Félix agradece y vendrá a la oficina", "importante": false}]}"#;
        app.apply_mail_results(mail_ai::parse_reply(reply).unwrap());
        assert_eq!(
            fs::read_to_string(&taller).unwrap(),
            "Clases de taller\n- [ ] Correo de Félix Westermeier (28 sep): Horario de consultas. Pide un rato para consultas ^fx001 #taller\n  ↳ 2026-09-28: Respondiste: Le confirmé un espacio entre las 15:40 y las 16:20\n  ↳ 2026-09-28: Respuesta de Félix Westermeier: Félix agradece y vendrá a la oficina\nOtra cosa\n"
        );
        assert!(!dir.join("Diario").join(format!("{}.md", today())).exists(), "no se agregan líneas nuevas a la nota de hoy");
        assert!(app.mail.store.mails[1..].iter().all(|m| m.noted == "Docencia/Taller"));
        // Deshacer quita los seguimientos.
        app.undo_ai();
        assert!(!fs::read_to_string(&taller).unwrap().contains('↳'));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reply_subjects() {
        assert_eq!(base_subject("RE: Re: Horario de consultas"), "horario de consultas");
        assert_eq!(base_subject("RV: Fwd: Planos"), "planos");
        assert!(is_reply_subject("Re: Planos") && !is_reply_subject("Planos"));
        assert_eq!(addresses("Félix <felix@uni.cl>, ana@obra.cl"), vec!["felix@uni.cl", "ana@obra.cl"]);
    }

    /// Con el correo configurado en dos equipos, el mismo correo no se anota dos veces.
    #[test]
    fn a_mail_is_noted_by_one_device_only() {
        let dir = std::env::temp_dir().join(format!("nodex-correo-dos-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("NODEX_CONFIG_DIR", std::env::temp_dir().join(format!("nodex-config-{}", std::process::id()))) };
        fs::create_dir_all(dir.join("General")).unwrap();
        let cfg = Config { carpeta_notas: dir.clone(), proveedor: "ollama".into(), modelo: "x".into(), ia_automatica: false, ..Config::default() };
        let mut app = NotesApp::new(cfg, None, egui::Context::default());
        app.machine = "pc-b".into();
        let mail = |uid: &str, msg: &str, subject: &str| Mail {
            id: format!("jp@gmail.com:INBOX:{uid}"),
            from: "Ana <ana@obra.cl>".into(),
            subject: subject.into(),
            date: "2026-09-29 10:00".into(),
            message_id: msg.into(),
            ..Mail::default()
        };
        app.mail.store.mails = vec![mail("1", "<uno@x>", "Planos"), mail("2", "<dos@x>", "Visita"), mail("3", "<tres@x>", "Acta")];
        app.mail.batch = (1..=3).map(|i| (format!("c{i}"), format!("jp@gmail.com:INBOX:{i}"))).collect();
        // El otro equipo (pc-a) ya tomó el primero; el segundo ya está escrito en la nota de hoy
        // (llegó por Dropbox); el tercero es de este equipo.
        crate::claims::claim_mails(&dir, &["<uno@x>".into()], "pc-a");
        let daily = dir.join("Diario").join(format!("{}.md", today()));
        fs::create_dir_all(dir.join("Diario")).unwrap();
        fs::write(&daily, "Correo de Ana (29 sep): Visita. Ana propone visitar la obra el jueves\n").unwrap();
        app.vault.scan();
        let reply = r#"{"correos": [{"id": "c1", "resumen": "Manda planos", "importante": true},
            {"id": "c2", "resumen": "Propone una visita", "importante": true},
            {"id": "c3", "resumen": "Envía el acta", "importante": true}]}"#;
        app.apply_mail_results(mail_ai::parse_reply(reply).unwrap());
        let text = fs::read_to_string(&daily).unwrap();
        assert_eq!(text, "Correo de Ana (29 sep): Visita. Ana propone visitar la obra el jueves\nCorreo de Ana (29 sep): Acta. Envía el acta\n");
        let noted: Vec<&str> = app.mail.store.mails.iter().map(|m| m.noted.as_str()).collect();
        assert_eq!(noted[0], "otro equipo");
        assert!(noted[1].starts_with("Diario/") && noted[2].starts_with("Diario/"));
        let _ = fs::remove_dir_all(&dir);
    }
}
