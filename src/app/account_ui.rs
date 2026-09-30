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
    sync_rx: Option<Receiver<Result<Synced, String>>>,
    sync_at: Option<Instant>,
    /// Hubo un cambio en la configuración y aún no se sube.
    pub dirty_at: Option<Instant>,
    /// Última vez que se juntó ("17:06") o por qué no se pudo.
    pub synced: Option<Result<String, String>>,
}

impl NotesApp {
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
            self.acct.error = Some("Falta la dirección del servidor de Notas".into());
            return;
        };
        self.acct.error = None;
        self.acct.busy = true;
        self.acct.code_sent_to = Some(email.trim().to_lowercase());
        self.acct.code_rx = Some(account::send_code(base, email, self.ctx.clone()));
    }

    pub(super) fn account_sign_in(&mut self, email: String, code: String, use_included: bool) {
        let Some(base) = account::server(&self.cfg) else { return };
        self.acct.error = None;
        self.acct.busy = true;
        self.acct.use_included = use_included;
        self.acct.login_rx = Some(account::sign_in_code(base, email, code, self.ctx.clone()));
    }

    pub(super) fn account_sign_in_microsoft(&mut self, use_included: bool) {
        let Some(base) = account::server(&self.cfg) else {
            self.acct.error = Some("Falta la dirección del servidor de Notas".into());
            return;
        };
        self.acct.error = None;
        self.acct.busy = true;
        self.acct.use_included = use_included;
        self.acct.login_rx = Some(account::sign_in_microsoft(base, self.ctx.clone()));
        self.msg("Se abrió el navegador para entrar con Microsoft");
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
        self.msg(format!("Saliste de la cuenta {who} en este equipo (tu configuración queda como está)"));
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
                Ok(()) => self.msg("Te enviamos un código a tu correo"),
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
                    }
                    self.save_config();
                    self.restart_ai();
                    self.ai_usage = None;
                    self.ai_usage_at = long_ago();
                    self.acct.code_sent_to = None;
                    self.msg(format!("Entraste como {} · {}", s.cuenta.correo, s.cuenta.plan_label()));
                    self.acct.info = Some(Ok(s.cuenta));
                    self.acct.info_at = Some(Instant::now());
                    self.account_sync_now();
                }
                Err(e) => self.acct.error = Some(e),
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
            self.msg(format!("No se pudo guardar la configuración: {e}"));
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
                "calendarios" => "calendarios",
                "correos" | "correo_al_llegar" | "correo_diario" => "correo",
                "google_client" | "google_token" => "Google Calendar",
                "microsoft_token" => "Microsoft To Do",
                _ => "ajustes",
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        self.msg(format!("Llegó tu configuración de la cuenta: {}", what.join(", ")));
    }
}
