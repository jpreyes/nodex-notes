//! Tu cuenta, en la app: entrar y salir, el plan, y la configuración que viaja con la cuenta
//! (se junta al entrar, al abrir, cada 10 minutos y unos segundos después de cada cambio).

use super::*;
use crate::account::{self, Info, SignedIn, Synced};
use std::sync::mpsc::Receiver;

/// Cada cuánto se junta la configuración con la de la cuenta, y cuánto se espera después de un cambio.
const SYNC_EVERY: Duration = Duration::from_secs(600);
const SYNC_AFTER_CHANGE: Duration = Duration::from_secs(5);
const INFO_EVERY: Duration = Duration::from_secs(1800);

#[derive(Default)]
pub(super) struct AccountState {
    pub info: Option<Result<Info, String>>,
    info_rx: Option<Receiver<Result<Info, String>>>,
    info_at: Option<Instant>,
    code_rx: Option<Receiver<Result<(), String>>>,
    /// Se mandó el código a este correo (se muestra el campo para escribirlo).
    pub code_sent_to: Option<String>,
    login_rx: Option<Receiver<Result<SignedIn, String>>>,
    /// Al entrar, pasar a la IA incluida.
    use_included: bool,
    pub busy: bool,
    pub error: Option<String>,
    /// La cuenta se creó (o ya existía) y espera aprobación.
    pub notice: Option<String>,
    sync_rx: Option<Receiver<Result<Synced, String>>>,
    sync_at: Option<Instant>,
    /// Hubo un cambio en la configuración y aún no se sube.
    pub dirty_at: Option<Instant>,
    /// Última vez que se juntó ("17:06") o por qué no se pudo.
    pub synced: Option<Result<String, String>>,
}

/// Qué muestra el formulario de la cuenta.
#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub(super) enum LoginMode {
    #[default]
    SignIn,
    Register,
    /// «¿Olvidaste tu clave?»: un código al correo y una clave nueva.
    Forgot,
}

/// El formulario «Iniciar sesión / Crear cuenta».
#[derive(Default)]
pub(super) struct LoginForm {
    pub mode: LoginMode,
    pub email: String,
    pub password: String,
    pub code: String,
    pub show: bool,
    /// Usar la propia clave de IA en vez de la IA de Notas.
    pub own_ai: bool,
}

/// Lo que pidió el formulario.
pub(super) enum LoginAction {
    SignIn { email: String, password: String, included: bool },
    Register { email: String, password: String, included: bool },
    SendCode(String),
    Reset { email: String, code: String, password: String, included: bool },
    Microsoft(bool),
}

/// El formulario. `can` = hay servidor; `code_sent` = a qué correo se mandó el código; `notice` =
/// la cuenta espera aprobación.
pub(super) fn login_form(ui: &mut Ui, f: &mut LoginForm, busy: bool, can: bool, error: Option<&str>, notice: Option<&str>, code_sent: Option<&str>) -> Option<LoginAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        if ui.selectable_label(f.mode == LoginMode::SignIn, RichText::new(t!("Iniciar sesión")).size(14.0)).clicked() {
            f.mode = LoginMode::SignIn;
        }
        if ui.selectable_label(f.mode == LoginMode::Register, RichText::new(t!("Crear cuenta")).size(14.0)).clicked() {
            f.mode = LoginMode::Register;
        }
        if f.mode == LoginMode::Forgot {
            ui.label(RichText::new(t!("·  Recuperar tu clave")).size(14.0).color(MUTED()));
        }
    });
    ui.add_space(8.0);
    let field = |ui: &mut Ui, label: &str| ui.label(RichText::new(label).size(12.5).color(MUTED()));
    field(ui, t!("Correo"));
    let er = ui.add(egui::TextEdit::singleline(&mut f.email).hint_text(t!("tu@correo.cl")).desired_width(300.0).margin(Margin::symmetric(8, 5)));
    let email_ok = f.email.contains('@') && f.email.contains('.');
    let waiting_code = f.mode == LoginMode::Forgot && code_sent.is_none();
    let mut submit = er.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && waiting_code;
    if f.mode == LoginMode::Forgot {
        if let Some(to) = code_sent {
            ui.add_space(4.0);
            field(ui, &tf!("Código (lo enviamos a {to}; revisa también el spam)", to = to));
            ui.add(egui::TextEdit::singleline(&mut f.code).hint_text("123456").desired_width(120.0).margin(Margin::symmetric(8, 5)));
        }
    }
    if !waiting_code {
        ui.add_space(4.0);
        field(ui, if f.mode == LoginMode::SignIn { t!("Clave") } else { t!("Clave (al menos 8 caracteres)") });
        ui.horizontal(|ui| {
            let pr = ui.add(egui::TextEdit::singleline(&mut f.password).password(!f.show).desired_width(268.0).margin(Margin::symmetric(8, 5)));
            submit |= pr.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            let eye = if f.show { icon::EYE_SLASH } else { icon::EYE };
            if ui.add(egui::Button::new(RichText::new(eye).size(15.0).color(MUTED())).frame(false)).on_hover_text(if f.show { t!("Ocultar") } else { t!("Mostrar") }).clicked() {
                f.show = !f.show;
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(t!("IA:")).size(13.0).color(MUTED()));
            ui.radio_value(&mut f.own_ai, false, RichText::new(t!("La de Notas (incluida)")).size(13.0));
            ui.radio_value(&mut f.own_ai, true, RichText::new(t!("Mi propia clave")).size(13.0));
        });
    }
    ui.add_space(8.0);
    let pw_ok = if f.mode == LoginMode::SignIn { !f.password.is_empty() } else { f.password.chars().count() >= 8 };
    let (label, ready) = match f.mode {
        LoginMode::SignIn => (t!("Entrar"), email_ok && pw_ok),
        LoginMode::Register => (t!("Crear cuenta"), email_ok && pw_ok),
        LoginMode::Forgot if waiting_code => (t!("Enviarme un código"), email_ok),
        LoginMode::Forgot => (t!("Guardar la clave y entrar"), f.code.trim().len() >= 6 && pw_ok),
    };
    let ready = ready && can && !busy;
    ui.horizontal(|ui| {
        let b = egui::Button::new(RichText::new(label).color(theme::c(Color32::WHITE))).fill(ACCENT()).min_size(egui::vec2(140.0, 30.0));
        if ui.add_enabled(ready, b).clicked() || (submit && ready) {
            let (email, password, included) = (f.email.trim().to_string(), f.password.clone(), !f.own_ai);
            action = Some(match f.mode {
                LoginMode::SignIn => LoginAction::SignIn { email, password, included },
                LoginMode::Register => LoginAction::Register { email, password, included },
                LoginMode::Forgot if waiting_code => LoginAction::SendCode(email),
                LoginMode::Forgot => LoginAction::Reset { email, code: f.code.trim().to_string(), password, included },
            });
        }
        if busy {
            ui.spinner();
        }
        ui.add_space(8.0);
        match f.mode {
            LoginMode::SignIn => {
                if ui.link(RichText::new(t!("¿Olvidaste tu clave?")).size(12.5)).clicked() {
                    f.mode = LoginMode::Forgot;
                    f.password.clear();
                }
            }
            LoginMode::Forgot => {
                if ui.link(RichText::new(t!("Volver")).size(12.5)).clicked() {
                    f.mode = LoginMode::SignIn;
                }
            }
            LoginMode::Register => {}
        }
    });
    if let Some(n) = notice {
        ui.add_space(6.0);
        ui.label(RichText::new(format!("{} {n}", icon::HOURGLASS_MEDIUM)).size(13.0).color(ACCENT()));
    } else if let Some(e) = error {
        ui.add_space(6.0);
        ui.label(RichText::new(format!("{} {e}", icon::WARNING_CIRCLE)).size(12.5).color(RED()));
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(t!("o")).size(12.5).color(MUTED()));
        if ui.add_enabled(can && !busy, egui::Button::new(RichText::new(format!("{}  {}", icon::WINDOWS_LOGO, t!("Entrar con Microsoft"))).size(12.5))).clicked() {
            action = Some(LoginAction::Microsoft(!f.own_ai));
        }
    });
    action
}

impl NotesApp {
    /// Lo que pidió el formulario de la cuenta.
    pub(super) fn account_login(&mut self, a: LoginAction) {
        let Some(base) = account::server(&self.cfg) else {
            self.acct.error = Some(t!("Falta la dirección del servidor de Notas").into());
            return;
        };
        let ctx = self.ctx.clone();
        let (rx, included) = match a {
            LoginAction::SendCode(email) => return self.account_send_code(email),
            LoginAction::Microsoft(included) => return self.account_sign_in_microsoft(included),
            LoginAction::SignIn { email, password, included } => (account::sign_in_password(base, email, password, ctx), included),
            LoginAction::Register { email, password, included } => (account::register(base, email, password, ctx), included),
            LoginAction::Reset { email, code, password, included } => (account::reset_password(base, email, code, password, ctx), included),
        };
        self.acct.error = None;
        self.acct.notice = None;
        self.acct.busy = true;
        self.acct.use_included = included;
        self.acct.login_rx = Some(rx);
    }

    /// ¿Hay una sesión de cuenta en este equipo?
    pub(super) fn signed_in(&self) -> bool {
        !self.cfg.cuenta.is_empty() && self.cfg.codigo_ia.starts_with("ns-")
    }

    /// Manda el código al correo (con el correo vacío, vuelve a pedir el correo).
    pub(super) fn account_send_code(&mut self, email: String) {
        if email.trim().is_empty() {
            self.acct.code_sent_to = None;
            self.acct.error = None;
            return;
        }
        let Some(base) = account::server(&self.cfg) else {
            self.acct.error = Some(t!("Falta la dirección del servidor de Notas").into());
            return;
        };
        self.acct.error = None;
        self.acct.busy = true;
        self.acct.code_sent_to = Some(email.trim().to_lowercase());
        self.acct.code_rx = Some(account::send_code(base, email, self.ctx.clone()));
    }

    pub(super) fn account_sign_in_microsoft(&mut self, use_included: bool) {
        let Some(base) = account::server(&self.cfg) else {
            self.acct.error = Some(t!("Falta la dirección del servidor de Notas").into());
            return;
        };
        self.acct.error = None;
        self.acct.busy = true;
        self.acct.use_included = use_included;
        self.acct.login_rx = Some(account::sign_in_microsoft(base, self.ctx.clone()));
        self.msg(t!("Se abrió el navegador para entrar con Microsoft"));
    }

    pub(super) fn account_sign_out(&mut self) {
        if let Some(base) = account::server(&self.cfg) {
            account::sign_out(base, self.cfg.codigo_ia.clone());
        }
        let who = std::mem::take(&mut self.cfg.cuenta);
        self.cfg.codigo_ia.clear();
        self.save_config();
        self.restart_ai();
        self.acct = AccountState::default();
        self.ai_usage = None;
        self.restart_sync();
        self.msg(tf!("Saliste de la cuenta {who} en este equipo (tu configuración queda como está)", who = who));
    }

    /// Junta la configuración ahora.
    pub(super) fn account_sync_now(&mut self) {
        self.acct.sync_at = None;
        self.acct.dirty_at = Some(Instant::now() - SYNC_AFTER_CHANGE);
    }

    /// Se llama cada segundo: respuestas del servidor, y juntar la configuración cuando toca.
    pub(super) fn poll_account(&mut self) {
        if let Some(r) = self.acct.code_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.acct.code_rx = None;
            self.acct.busy = false;
            match r {
                Ok(()) => self.msg(t!("Te enviamos un código a tu correo")),
                Err(e) => {
                    self.acct.code_sent_to = None;
                    self.acct.error = Some(e);
                }
            }
        }
        if let Some(r) = self.acct.login_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.acct.login_rx = None;
            self.acct.busy = false;
            match r {
                Ok(s) => {
                    self.cfg.codigo_ia = s.token;
                    self.cfg.cuenta = s.cuenta.correo.clone();
                    if self.cfg.servidor_ia.trim().is_empty() {
                        if let Some(base) = account::server(&self.cfg) {
                            self.cfg.servidor_ia = base;
                        }
                    }
                    if self.acct.use_included {
                        self.cfg.proveedor = "notas".into();
                        if self.cfg.modelo.trim().is_empty() || !ai::PROVIDERS.iter().any(|p| p.0 == "notas" && p.2.contains(&self.cfg.modelo.as_str())) {
                            self.cfg.modelo = "incluida".into();
                        }
                    }
                    self.save_config();
                    self.restart_ai();
                    self.ai_usage = None;
                    self.ai_usage_at = long_ago();
                    self.acct.code_sent_to = None;
                    let ai_note = if self.acct.use_included {
                        String::new()
                    } else if self.cfg.proveedor == "notas" || self.cfg.clave_api.trim().is_empty() {
                        t!(" · pon tu clave de IA en Configuración → Inteligencia artificial").to_string()
                    } else {
                        t!(" · con tu propia clave de IA").to_string()
                    };
                    self.msg(tf!("Entraste como {email} · {plan}{note}", email = s.cuenta.correo, plan = s.cuenta.plan_label(), note = ai_note));
                    self.acct.info = Some(Ok(s.cuenta));
                    self.acct.info_at = Some(Instant::now());
                    self.account_sync_now();
                    self.restart_sync();
                }
                // Espera aprobación: no es un error.
                Err(e) => match e.strip_prefix(account::PENDING) {
                    Some(n) => {
                        self.acct.notice = Some(n.to_string());
                        self.acct.error = None;
                    }
                    None => self.acct.error = Some(e),
                },
            }
        }
        if !self.signed_in() {
            return;
        }
        let Some(base) = account::server(&self.cfg) else { return };
        // Plan y prueba.
        if let Some(r) = self.acct.info_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.acct.info_rx = None;
            self.acct.info = Some(r);
        }
        if self.acct.info_rx.is_none() && self.acct.info_at.is_none_or(|t| t.elapsed() >= INFO_EVERY) {
            self.acct.info_at = Some(Instant::now());
            self.acct.info_rx = Some(account::fetch_info(base.clone(), self.cfg.codigo_ia.clone(), self.ctx.clone()));
        }
        // La configuración.
        if let Some(r) = self.acct.sync_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.acct.sync_rx = None;
            match r {
                Ok(s) => {
                    let names: Vec<String> = s.apply.iter().map(|(n, _)| n.clone()).collect();
                    for (n, v) in &s.apply {
                        account::apply(&mut self.cfg, n, v);
                    }
                    account::save_state(&s.state);
                    if !names.is_empty() {
                        self.after_account_changes(&names);
                    }
                    self.acct.synced = Some(Ok(Local::now().format("%H:%M").to_string()));
                }
                Err(e) => self.acct.synced = Some(Err(e)),
            }
        }
        let due = self.acct.sync_at.is_none_or(|t| t.elapsed() >= SYNC_EVERY) || self.acct.dirty_at.is_some_and(|t| t.elapsed() >= SYNC_AFTER_CHANGE);
        if self.acct.sync_rx.is_none() && due {
            self.acct.sync_at = Some(Instant::now());
            self.acct.dirty_at = None;
            let local = account::local_values(&self.cfg);
            self.acct.sync_rx = Some(account::sync(base, self.cfg.codigo_ia.clone(), self.vault.root.clone(), local, account::load_state(), self.ctx.clone()));
        }
    }

    /// Llegó configuración de la cuenta: se guarda y se reinicia lo que depende de ella.
    fn after_account_changes(&mut self, names: &[String]) {
        if let Err(e) = config::save(&self.cfg) {
            self.msg(tf!("No se pudo guardar la configuración: {e}", e = e));
        }
        if names.iter().any(|n| n == "calendarios") {
            self.cals.last = None;
        }
        if names.iter().any(|n| n.starts_with("google")) {
            self.restart_gcal();
        }
        if names.iter().any(|n| n == "microsoft_token") {
            self.todo = crate::todo::ToDo::start(self.vault.root.clone(), self.ctx.clone());
            self.todo_hash = 0;
        }
        if names.iter().any(|n| n == "ia_automatica") {
            self.ai_auto = self.cfg.ia_automatica;
        }
        let what: Vec<&str> = names
            .iter()
            .map(|n| match n.as_str() {
                "calendarios" => t!("calendarios"),
                "correos" | "correo_al_llegar" | "correo_diario" => t!("correo"),
                "google_client" | "google_token" => "Google Calendar",
                "microsoft_token" => "Microsoft To Do",
                _ => t!("ajustes"),
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        self.msg(tf!("Llegó tu configuración de la cuenta: {what}", what = what.join(", ")));
    }
}
